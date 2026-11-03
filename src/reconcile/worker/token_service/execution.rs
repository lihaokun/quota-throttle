use super::*;

struct TokenExecutionGuard<'a> {
    worker: &'a ReconcileWorker,
    ticket: Option<TokenTicket>,
}
impl TokenExecutionGuard<'_> {
    fn settle(&mut self, outcome: ExecutionOutcome) -> ControlResult<()> {
        let ticket = self
            .ticket
            .as_ref()
            .ok_or_else(|| error(ControlErrorCode::Storage))?;
        self.worker.mutate(None, |core| {
            core.store.settle_token_execution(ticket, outcome, now())
        })?;
        self.ticket = None;
        Ok(())
    }
}
impl Drop for TokenExecutionGuard<'_> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            let _ = self.worker.mutate(None, |core| {
                core.store
                    .settle_token_execution(&ticket, ExecutionOutcome::Uncertain, now())
            });
        }
    }
}
impl ReconcileWorker {
    pub(crate) fn create_token(
        &self,
        id: OperationId,
        group_id: GroupId,
        expected_group_revision: i64,
        display_name: String,
        expires_at: Option<i64>,
    ) -> ControlResult<OperationView> {
        self.submit_token_built(id, |core, id| {
            let owner = match core.store.operation(id) {
                Ok(view)
                    if view.kind == OperationKind::CreateToken
                        && view.target_type == crate::catalog::control::TargetType::Token =>
                {
                    core.store.token_work(id, now())?.owner()
                }
                Ok(_) => return Err(error(ControlErrorCode::IdempotencyConflict)),
                Err(e) if e.code == ControlErrorCode::NotFound => {
                    self.token_client(TokenOwnerScope::Issuer)?.1
                }
                Err(e) => return Err(e),
            };
            Ok(TokenCommand::CreateToken {
                owner,
                group_id,
                expected_group_revision,
                display_name,
                expires_at,
            })
        })
    }
    pub(crate) fn set_token_enabled(
        &self,
        id: OperationId,
        token_id: ManagedTokenId,
        expected_revision: i64,
        enabled: bool,
    ) -> ControlResult<OperationView> {
        self.submit_token_command(
            id,
            TokenCommand::SetTokenEnabled {
                token_id,
                expected_revision,
                enabled,
            },
        )
    }
    pub(crate) fn delete_token(
        &self,
        id: OperationId,
        token_id: ManagedTokenId,
        expected_revision: i64,
    ) -> ControlResult<OperationView> {
        self.submit_token_command(
            id,
            TokenCommand::DeleteToken {
                token_id,
                expected_revision,
            },
        )
    }
    fn submit_token_command(
        &self,
        id: OperationId,
        command: TokenCommand,
    ) -> ControlResult<OperationView> {
        self.submit_token_built(id, |_, _| Ok(command))
    }
    fn submit_token_built(
        &self,
        id: OperationId,
        build: impl FnOnce(&mut Core, &OperationId) -> ControlResult<TokenCommand>,
    ) -> ControlResult<OperationView> {
        // Step T5C.4: 持久提交仍由B幂等；本层不在重放之前添加网络准入。
        require(!self.policy_config.dry_run, ControlErrorCode::NotReady)?;
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        self.publish(|core| {
            let command = build(core, &id)?;
            if let Some(view) = core.store.replayed_token_operation(&id, &command)? {
                return Ok(view);
            }
            let owner = match &command {
                TokenCommand::CreateToken { owner, .. } => *owner,
                TokenCommand::SetTokenEnabled { token_id, .. }
                | TokenCommand::DeleteToken { token_id, .. } => {
                    let view = core.store.token_identity(*token_id)?;
                    TokenOwner {
                        user_id: view
                            .owner_user_id
                            .parse()
                            .map_err(|_| error(ControlErrorCode::Storage))?,
                        scope: view.owner_scope,
                    }
                }
            };
            self.owner_client(owner)?;
            core.store.submit_token_operation(id, command, now())
        })
    }
    pub(in crate::reconcile::worker) async fn retry_token_auth(
        &self,
        id: &OperationId,
    ) -> ControlResult<()> {
        let owner = self.token_core(|core| Ok(core.store.token_work(id, now())?.owner()))?;
        self.owner_client(owner)?
            .retry_management_authentication()
            .await
            .map_err(|e| error(code(&e)))
    }
    fn token_problem(&self, work: &TokenWork, reason: ControlErrorCode) -> ControlResult<()> {
        self.mutate(None, |core| {
            let at = now();
            if at.checked_sub(work.loaded_at()).is_some_and(|age| age > 60) {
                core.store.note_token_delay(work, at)?;
            } else {
                core.store.note_token_problem(work, reason, at)?;
            }
            Ok(())
        })
    }
    async fn token_admission(
        &self,
        work: &TokenWork,
        frame: Frame,
        stop: &AtomicBool,
    ) -> ControlResult<TokenAdmission> {
        let client = self.owner_client(work.owner())?;
        if work.kind() == OperationKind::CreateToken {
            let spec = work.create_spec()?;
            return self
                .positive_admission(
                    work.owner(),
                    &spec.group,
                    true,
                    true,
                    spec.expires_at,
                    frame,
                    stop,
                )
                .await;
        }
        if work.enabled() == Some(true) {
            let guard = || frame.guard(self, stop);
            let api = client.managed_tokens(work.owner());
            let api = api.fenced(&guard);
            let value = api
                .get(
                    work.native_token_id()
                        .ok_or_else(|| error(ControlErrorCode::NotReady))?,
                )
                .await
                .map_err(|e| error(code(&e)))?
                .ok_or_else(|| error(ControlErrorCode::NotFound))?;
            self.token_core(|core| core.store.validate_token_work(work, &value, now()))?;
            return self
                .positive_admission(
                    work.owner(),
                    &value.explicit_group,
                    work.group_id().is_some(),
                    true,
                    value.expires_at,
                    frame,
                    stop,
                )
                .await;
        }
        Ok(TokenAdmission {
            frame,
            owner: work.owner(),
            group: None,
            member: None,
        })
    }
    async fn dispatch_token(
        &self,
        ticket: &TokenTicket,
        frame: Frame,
        stop: &AtomicBool,
    ) -> std::result::Result<(), ManagementError> {
        let guard = || !self.policy_config.dry_run && frame.guard(self, stop);
        let client = self
            .owner_client(ticket.owner())
            .map_err(|_| management_storage())?;
        let api = client.managed_tokens(ticket.owner());
        let api = api.fenced(&guard);
        if let Some(enabled) = ticket.enabled() {
            api.set_enabled(
                ticket.native_token_id().ok_or_else(management_storage)?,
                enabled,
            )
            .await
        } else if ticket.is_delete() {
            api.delete(ticket.native_token_id().ok_or_else(management_storage)?)
                .await
        } else {
            api.create(&ticket.create_spec().map_err(|_| management_storage())?)
                .await
        }
    }
    async fn write_token(
        &self,
        work: &TokenWork,
        admission: TokenAdmission,
        stop: &AtomicBool,
    ) -> ControlResult<()> {
        // Step T5C.5: prepare提交先于票据；post-commit检查失败仍交给锁外guard结算。
        require(!self.policy_config.dry_run, ControlErrorCode::NotReady)?;
        let epoch = admission.frame.epoch;
        let (ticket, checked) = self.mutate(Some(epoch), |core| {
            admission.check(core, now())?;
            require(
                admission.owner == work.owner(),
                ControlErrorCode::AuthFailed,
            )?;
            let ticket = core.store.prepare_token_execution(work, now())?;
            let checked = core
                .store
                .token_send_allowed(&ticket, now())
                .and_then(|allowed| require(allowed, ControlErrorCode::NotReady));
            Ok((ticket, checked))
        })?;
        let mut pending = TokenExecutionGuard {
            worker: self,
            ticket: Some(ticket.clone()),
        };
        if let Err(e) = checked {
            pending.settle(ExecutionOutcome::NotSent(e.code))?;
            return Err(e);
        }
        let frame = Frame {
            epoch: epoch + 2,
            ..admission.frame
        };
        let result = self.dispatch_token(&ticket, frame, stop).await;
        let outcome = match &result {
            Ok(()) => ExecutionOutcome::Responded(None),
            Err(e) => match e.delivery {
                DeliveryEvidence::NotSent => ExecutionOutcome::NotSent(code(e)),
                DeliveryEvidence::ResponseReceived => ExecutionOutcome::Responded(Some(code(e))),
                DeliveryEvidence::Uncertain => ExecutionOutcome::Uncertain,
            },
        };
        pending.settle(outcome)?;
        result.map_err(|e| error(code(&e)))
    }
    async fn observe_token(
        &self,
        work: &TokenWork,
        frame: Frame,
        stop: &AtomicBool,
    ) -> ControlResult<bool> {
        // Step T5C.6: 读取使用网络前的Work，失败也节流；旧事实不能套新Work确认。
        let client = self.owner_client(work.owner())?;
        let guard = || frame.guard(self, stop);
        let api = client.managed_tokens(work.owner());
        let api = api.fenced(&guard);
        let observation = if work.kind() == OperationKind::CreateToken {
            if work.create_state() == CreateState::Never {
                self.token_problem(work, ControlErrorCode::NotReady)?;
                return Ok(false);
            }
            api.lookup_created(&work.create_spec()?)
                .await
                .map(TokenObservation::Creation)
        } else {
            api.get(
                work.native_token_id()
                    .ok_or_else(|| error(ControlErrorCode::NotReady))?,
            )
            .await
            .map(TokenObservation::Existing)
        };
        let observation = match observation {
            Ok(value) => value,
            Err(e) => return Err(error(code(&e))),
        };
        if now()
            .checked_sub(work.loaded_at())
            .is_some_and(|age| age > 60)
        {
            self.token_problem(work, ControlErrorCode::NotReady)?;
            return Ok(false);
        }
        let view = self.mutate(None, |core| {
            core.store.confirm_token(work, observation, now())
        })?;
        Ok(view.phase == OperationPhase::Succeeded)
    }
    pub(in crate::reconcile::worker) async fn token_step(
        &self,
        id: &OperationId,
        stop: &AtomicBool,
    ) -> ControlResult<bool> {
        // caller已持lane；只保护本项，不冻结整个批次，也不阻止人工撤销。
        let _automatic = self
            .automatic_updates
            .try_write()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let work = self.token_core(|core| core.store.token_work(id, now()))?;
        if matches!(
            work.phase(),
            OperationPhase::Succeeded | OperationPhase::Failed
        ) {
            return Ok(false);
        }
        let frame = match self.token_frame() {
            Ok(frame) => frame,
            Err(e) => {
                self.token_problem(&work, e.code)?;
                return Err(e);
            }
        };
        if work.phase() != OperationPhase::Queued
            || (work.kind() == OperationKind::CreateToken
                && work.create_state() != CreateState::Never)
        {
            let result = self.observe_token(&work, frame, stop).await;
            if let Err(e) = &result {
                if e.code == ControlErrorCode::Storage {
                    return result;
                }
                if let Err(note) = self.token_problem(&work, e.code) {
                    if note.code == ControlErrorCode::Storage {
                        return Err(note);
                    }
                }
            }
            return result;
        }
        let admission = match self.token_admission(&work, frame, stop).await {
            Ok(admission) => admission,
            Err(e) => {
                self.token_problem(&work, e.code)?;
                return Err(e);
            }
        };
        match self.write_token(&work, admission, stop).await {
            Ok(()) => Ok(true),
            Err(e) => {
                // 若已prepare/结算，证据版本已变化；旧Work拒绝note，不能盖掉交付结论。
                if let Err(note) = self.token_problem(&work, e.code) {
                    if note.code == ControlErrorCode::Storage {
                        return Err(note);
                    }
                }
                Err(e)
            }
        }
    }
    pub(in crate::reconcile::worker) async fn token_round(
        &self,
        stop: &AtomicBool,
        report: &mut RoundReport,
    ) -> ControlResult<()> {
        if self.token_owners.is_none() || self.policy_config.dry_run {
            return Ok(());
        }
        let ids = self.token_core(|core| core.store.pending_token_operations(now()))?;
        let mut seen = HashSet::new();
        for id in ids {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            report.visited += 1;
            match self.token_step(&id, stop).await {
                Ok(true) => report.progressed += 1,
                Ok(false) => report.waiting += 1,
                Err(e) if e.code == ControlErrorCode::Storage => return Err(e),
                Err(e) if crate::catalog::confirmation::terminal(e.code) => report.failed += 1,
                Err(_) => report.waiting += 1,
            }
        }
        Ok(())
    }
}
