//! 原worker上的固定身份令牌服务；所有可发送事实最终绑定原publication fence。
#![cfg_attr(not(test), allow(dead_code))]
use super::*;
use crate::catalog::control::{CreateState, OperationKind};
use crate::catalog::tokens::{
    TokenCommand, TokenIdentityView, TokenObservation, TokenTicket, TokenWork,
};
use crate::newapi::tokens::{
    NativeTokenPage, RevealedToken, TokenOwner, TokenOwnerScope, TokenStatus,
};
use serde::Serialize;

mod access;
mod execution;

pub const GROUP_CHECK_SCHEMA_VERSION: i64 = 1;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupCheckReport {
    pub schema_version: i64,
    pub group_id: GroupId,
    pub revision: i64,
    pub ready: bool,
    pub error_code: Option<ControlErrorCode>,
}
pub struct TokenOwners {
    issuer: Option<(TokenOwner, Arc<NewApiClient>)>,
    legacy: Option<TokenOwner>,
}
impl TokenOwners {
    pub(crate) fn new(
        issuer: Option<(Arc<NewApiClient>, i64)>,
        legacy_id: Option<i64>,
    ) -> ControlResult<Self> {
        require(
            (issuer.is_some() || legacy_id.is_some())
                && issuer.as_ref().is_none_or(|(_, id)| *id > 0)
                && legacy_id.is_none_or(|id| {
                    id > 0 && issuer.as_ref().is_none_or(|(_, other)| id != *other)
                }),
            ControlErrorCode::InvalidInput,
        )?;
        Ok(Self {
            issuer: issuer.map(|(client, user_id)| {
                (
                    TokenOwner {
                        user_id,
                        scope: TokenOwnerScope::Issuer,
                    },
                    client,
                )
            }),
            legacy: legacy_id.map(|user_id| TokenOwner {
                user_id,
                scope: TokenOwnerScope::LegacyAdmin,
            }),
        })
    }
}
fn require(ok: bool, code: ControlErrorCode) -> ControlResult<()> {
    if ok {
        Ok(())
    } else {
        Err(error(code))
    }
}
#[derive(Clone, Copy)]
struct Frame {
    epoch: u64,
    start: i64,
    end: i64,
}
impl Frame {
    fn guard(&self, worker: &ReconcileWorker, stop: &AtomicBool) -> bool {
        worker.gate(self.epoch, self.start, self.end, stop)
    }
}
struct TokenAdmission {
    frame: Frame,
    owner: TokenOwner,
    group: Option<(GroupId, i64)>,
    member: Option<(MembershipId, Revision)>,
}
fn eligible(core: &Core, work: &WorkSnapshot, at: i64) -> bool {
    work.managed
        && work.group.ready
        && !work.source.manual_paused
        && work.source.retired_at.is_none()
        && !work.source_retire_requested
        && !work.target.retire_requested
        && work.member.lifecycle != MembershipLifecycle::Retired
        && work.pending_write == PendingWrite::None
        && work.target.status_directive != StatusDirective::Disabled
        && !core
            .source_disabled
            .get(&work.source.source_id)
            .copied()
            .unwrap_or(false)
        && core
            .permits
            .get(&work.member.membership_id)
            .is_some_and(|p| p.valid_for(work, at))
}
impl TokenAdmission {
    fn check(&self, core: &mut Core, at: i64) -> ControlResult<()> {
        require(
            self.frame.start <= at && at < self.frame.end && core.backend.ready_at(at),
            ControlErrorCode::NotReady,
        )?;
        if let Some((id, revision)) = self.group {
            let group = core.store.group(id)?;
            require(
                group.ready && group.revision == revision,
                ControlErrorCode::RevisionConflict,
            )?;
        }
        if let Some((id, revision)) = self.member {
            let work = core.store.load_work(id, false)?;
            require(
                work.revision() == revision && eligible(core, &work, at),
                ControlErrorCode::NotReady,
            )?;
        }
        Ok(())
    }
}
impl ReconcileWorker {
    pub(crate) fn new_with_token_owners(
        store: CatalogStore,
        client: Arc<NewApiClient>,
        config: Arc<Config>,
        owners: TokenOwners,
    ) -> ControlResult<Arc<Self>> {
        // Step T5C.3: 固定同实例、同预算身份；角色/余额与当前组资格由真实请求核验。
        require(
            owners
                .issuer
                .as_ref()
                .is_none_or(|(_, issuer)| client.shares_management_scope(issuer)),
            ControlErrorCode::InvalidInput,
        )?;
        let mut worker = Self::new(store, client, config)?;
        Arc::get_mut(&mut worker)
            .ok_or_else(|| error(ControlErrorCode::Storage))?
            .token_owners = Some(owners);
        Ok(worker)
    }
    fn token_client(&self, scope: TokenOwnerScope) -> ControlResult<(&NewApiClient, TokenOwner)> {
        let owners = self
            .token_owners
            .as_ref()
            .ok_or_else(|| error(ControlErrorCode::NotReady))?;
        match scope {
            TokenOwnerScope::Issuer => owners
                .issuer
                .as_ref()
                .map(|(owner, client)| (client.as_ref(), *owner))
                .ok_or_else(|| error(ControlErrorCode::NotReady)),
            TokenOwnerScope::LegacyAdmin => Ok((
                &self.client,
                owners
                    .legacy
                    .ok_or_else(|| error(ControlErrorCode::NotReady))?,
            )),
        }
    }
    fn owner_client(&self, owner: TokenOwner) -> ControlResult<&NewApiClient> {
        let (client, configured) = self.token_client(owner.scope)?;
        require(configured == owner, ControlErrorCode::AuthFailed)?;
        Ok(client)
    }
    fn token_core<T>(&self, read: impl FnOnce(&mut Core) -> ControlResult<T>) -> ControlResult<T> {
        let mut core = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        core.store.usable()?;
        read(&mut core)
    }
    fn token_frame(&self) -> ControlResult<Frame> {
        self.token_core(|core| {
            let start = now();
            let epoch = self.publication.load(Ordering::SeqCst);
            require(
                epoch.is_multiple_of(2) && core.backend.ready_at(start),
                ControlErrorCode::NotReady,
            )?;
            Ok(Frame {
                epoch,
                start,
                end: core.backend.expires_at(),
            })
        })
    }
    async fn positive_admission(
        &self,
        owner: TokenOwner,
        group_name: &str,
        managed: bool,
        spend: bool,
        expiry: Option<i64>,
        frame: Frame,
        stop: &AtomicBool,
    ) -> ControlResult<TokenAdmission> {
        let frame = Frame {
            end: frame
                .end
                .min(frame.start.saturating_add(30))
                .min(expiry.unwrap_or(JSON_SAFE)),
            ..frame
        };
        let guard = || frame.guard(self, stop);
        let client = self.owner_client(owner)?;
        let api = client.managed_tokens(owner);
        let api = api.fenced(&guard);
        let profile = api.profile().await.map_err(|e| error(code(&e)))?;
        require(!spend || profile.quota > 0, ControlErrorCode::NotReady)?;
        let group = self.token_core(|core| {
            require(
                self.publication.load(Ordering::SeqCst) == frame.epoch,
                ControlErrorCode::RevisionConflict,
            )?;
            core.store.group_for_name(group_name)
        })?;
        require(!managed || group.is_some(), ControlErrorCode::NotReady)?;
        let mut admission = TokenAdmission {
            frame,
            owner,
            group: None,
            member: None,
        };
        if let Some(group) = group {
            require(
                api.available_groups()
                    .await
                    .map_err(|e| error(code(&e)))?
                    .iter()
                    .any(|name| name == group_name),
                ControlErrorCode::NotReady,
            )?;
            let candidates = self.token_core(|core| {
                require(
                    self.publication.load(Ordering::SeqCst) == frame.epoch,
                    ControlErrorCode::RevisionConflict,
                )?;
                require(group.ready, ControlErrorCode::NotReady)?;
                let candidates = core
                    .store
                    .load_group_work(group.group_id)?
                    .into_iter()
                    .filter(|w| eligible(core, w, now()))
                    .map(|w| {
                        let end = core.permits[&w.member.membership_id].expires_at();
                        (w, end)
                    })
                    .collect::<Vec<_>>();
                Ok(candidates)
            })?;
            for (work, deadline) in candidates {
                let channel = work
                    .member
                    .channel_id
                    .ok_or_else(|| error(ControlErrorCode::NotReady))?;
                let api = self.client.managed_channels();
                let api = api.fenced(&guard);
                let observed = api
                    .get_channel(channel.get())
                    .await
                    .and_then(|d| d.observation())
                    .map_err(|e| error(code(&e)))?;
                require(
                    identity_matches(&work.identity, channel, &observed)
                        && crate::model_catalog::model_sets_equal(
                            &observed.models,
                            &work.models.join(","),
                        )
                        && (work.source.kind != SourceKind::Volcano || observed.auto_ban == 0),
                    ControlErrorCode::BindingDrift,
                )?;
                require(fresh(&observed, now()), ControlErrorCode::NotReady)?;
                if observed.status != 1 || observed.priority != work.target.desired_priority {
                    continue;
                }
                admission.group = Some((group.group_id, group.revision));
                admission.member = Some((work.member.membership_id, work.revision()));
                admission.frame.end = admission.frame.end.min(deadline).min(observation_deadline(
                    &StampedObservation {
                        revision: work.revision(),
                        channel: observed,
                    },
                ));
                break;
            }
            require(admission.member.is_some(), ControlErrorCode::NotReady)?;
        }
        self.token_core(|core| {
            require(
                self.publication.load(Ordering::SeqCst) == frame.epoch,
                ControlErrorCode::RevisionConflict,
            )?;
            admission.check(core, now())
        })?;
        Ok(admission)
    }
    pub(crate) fn group_references(
        &self,
        id: GroupId,
    ) -> ControlResult<crate::catalog::groups::GroupReferences> {
        self.token_core(|core| core.store.group_references(id))
    }
    pub(crate) async fn check_group(
        &self,
        id: GroupId,
        revision: i64,
        register: bool,
    ) -> ControlResult<GroupCheckReport> {
        require(!self.policy_config.dry_run, ControlErrorCode::NotReady)?;
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let _lane = self
            .lane
            .try_lock()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        // 正常后台发布不得拆断多步原生组注册；人工控制仍可通过原fence撤销它。
        let _automatic = self
            .automatic_updates
            .try_write()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let frame = self.token_frame()?;
        require(
            !register || frame.end.saturating_sub(frame.start) >= 30,
            ControlErrorCode::NotReady,
        )?;
        let (group, works) = self.token_core(|core| {
            require(
                self.publication.load(Ordering::SeqCst) == frame.epoch,
                ControlErrorCode::RevisionConflict,
            )?;
            let group = core.store.group(id)?;
            require(
                group.revision == revision,
                ControlErrorCode::RevisionConflict,
            )?;
            Ok((group, core.store.load_group_work(id)?))
        })?;
        let result = self
            .check_group_native(&group, &works, register, frame)
            .await;
        self.mutate(Some(frame.epoch), |core| {
            let problem = result
                .err()
                .map(|e| e.code)
                .or_else(|| (!core.backend.ready_at(now())).then_some(ControlErrorCode::NotReady));
            let ready = problem.is_none();
            let saved = core.store.set_group_ready(id, revision, ready, now())?;
            if saved.revision != revision {
                let ids: HashSet<_> = works.iter().map(|w| w.member.membership_id).collect();
                core.permits.retain(|id, _| !ids.contains(id));
                core.observations.retain(|id, _| !ids.contains(id));
                core.input_epoch = self.publication.load(Ordering::SeqCst) + 1;
            }
            Ok(GroupCheckReport {
                schema_version: GROUP_CHECK_SCHEMA_VERSION,
                group_id: id,
                revision: saved.revision,
                ready,
                error_code: problem,
            })
        })
    }
    async fn check_group_native(
        &self,
        group: &GroupView,
        works: &[WorkSnapshot],
        register: bool,
        frame: Frame,
    ) -> ControlResult<()> {
        let stop = AtomicBool::new(false);
        let guard = || frame.guard(self, &stop);
        let (client, owner) = self.token_client(TokenOwnerScope::Issuer)?;
        let api = client.managed_tokens(owner);
        let api = api.fenced(&guard);
        let profile = api.profile().await.map_err(|e| error(code(&e)))?;
        let groups = self.client.managed_groups(&guard);
        if register {
            groups
                .register_for(&group.newapi_group, &profile.account_group)
                .await
                .map_err(|e| error(code(&e)))?;
        } else {
            require(
                groups
                    .registered()
                    .await
                    .map_err(|e| error(code(&e)))?
                    .contains(&group.newapi_group),
                ControlErrorCode::NotReady,
            )?;
        }
        require(
            api.available_groups()
                .await
                .map_err(|e| error(code(&e)))?
                .contains(&group.newapi_group),
            ControlErrorCode::NotReady,
        )?;
        let channels = self.client.managed_channels();
        let channels = channels.fenced(&guard);
        let scan = channels
            .scan_group(&group.newapi_group, None)
            .await
            .map_err(|e| error(code(&e)))?;
        require(scan.complete, ControlErrorCode::InvalidResponse)?;
        let mut observed = HashMap::new();
        for document in scan.channels {
            let item = document.observation().map_err(|e| error(code(&e)))?;
            require(
                observed.insert(item.channel_id, item).is_none(),
                ControlErrorCode::InvalidResponse,
            )?;
        }
        Self::validate_group_configuration(works, &observed)
    }
    pub(in crate::reconcile::worker) fn validate_group_configuration(
        works: &[WorkSnapshot],
        observed: &HashMap<i64, crate::newapi::managed_channels::ChannelObservation>,
    ) -> ControlResult<()> {
        let mut configured = 0;
        for work in works {
            if let Some(id) = work.member.channel_id {
                let actual = observed
                    .get(&id.get())
                    .ok_or_else(|| error(ControlErrorCode::BindingDrift))?;
                let valid = identity_matches(&work.identity, id, actual)
                    && crate::model_catalog::model_sets_equal(
                        &actual.models,
                        &work.models.join(","),
                    )
                    && (work.source.kind != SourceKind::Volcano || actual.auto_ban == 0);
                let active = !work.target.retire_requested
                    && !work.source_retire_requested
                    && work.source.retired_at.is_none()
                    && work.member.lifecycle != MembershipLifecycle::Retired;
                if actual.status == 1 {
                    require(valid && active, ControlErrorCode::BindingDrift)?;
                }
                if valid
                    && active
                    && work.pending_write == PendingWrite::None
                    && matches!(actual.status, 1 | 2)
                {
                    configured += 1;
                }
            }
        }
        let known: HashSet<_> = works
            .iter()
            .filter_map(|w| w.member.channel_id.map(ChannelId::get))
            .collect();
        for observed in observed.values().filter(|c| c.status == 1) {
            require(
                known.contains(&observed.channel_id),
                ControlErrorCode::BindingDrift,
            )?;
        }
        require(configured > 0, ControlErrorCode::NotReady)
    }
}
