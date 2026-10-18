use super::*;

impl ReconcileWorker {
    pub(crate) fn token_identity(&self, id: ManagedTokenId) -> ControlResult<TokenIdentityView> {
        self.token_core(|core| core.store.token_identity(id))
    }
    pub(crate) fn token_identities(
        &self,
        after: Option<ManagedTokenId>,
    ) -> ControlResult<Vec<TokenIdentityView>> {
        self.token_core(|core| core.store.list_token_identities(after))
    }
    pub(crate) async fn list_tokens(
        &self,
        scope: TokenOwnerScope,
        page: usize,
    ) -> ControlResult<NativeTokenPage> {
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let frame = self.token_frame()?;
        let stop = AtomicBool::new(false);
        let guard = || frame.guard(self, &stop);
        let (client, owner) = self.token_client(scope)?;
        let api = client.managed_tokens(owner);
        let api = api.fenced(&guard);
        let result = api.list_page(page).await.map_err(|e| error(code(&e)))?;
        require(guard(), ControlErrorCode::RevisionConflict)?;
        Ok(result)
    }
    pub(crate) async fn register_token(
        &self,
        scope: TokenOwnerScope,
        id: i64,
        label: String,
    ) -> ControlResult<TokenIdentityView> {
        require(!self.policy_config.dry_run, ControlErrorCode::NotReady)?;
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let frame = self.token_frame()?;
        let stop = AtomicBool::new(false);
        let guard = || frame.guard(self, &stop);
        let (client, owner) = self.token_client(scope)?;
        let api = client.managed_tokens(owner);
        let api = api.fenced(&guard);
        let view = api
            .get(id)
            .await
            .map_err(|e| error(code(&e)))?
            .ok_or_else(|| error(ControlErrorCode::NotFound))?;
        self.mutate(Some(frame.epoch), |core| {
            require(
                frame.start <= now() && now() < frame.end && core.backend.ready_at(now()),
                ControlErrorCode::NotReady,
            )?;
            core.store.register_token(owner, &view, label, now())
        })
    }
    pub(crate) async fn reveal_token(
        &self,
        id: ManagedTokenId,
        revision: i64,
    ) -> ControlResult<RevealedToken> {
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let frame = self.token_frame()?;
        let stop = AtomicBool::new(false);
        let guard = || frame.guard(self, &stop);
        let (owner, native) =
            self.token_core(|core| core.store.token_reveal_target(id, revision))?;
        let client = self.owner_client(owner)?;
        let api = client.managed_tokens(owner);
        let api = api.fenced(&guard);
        let view = api
            .get(native)
            .await
            .map_err(|e| error(code(&e)))?
            .ok_or_else(|| error(ControlErrorCode::NotFound))?;
        let identity = self.token_core(|core| {
            core.store.validate_token_access(id, revision, &view)?;
            core.store.token_identity(id)
        })?;
        require(
            matches!(view.status, TokenStatus::Enabled | TokenStatus::Disabled)
                && view.expires_at.is_none_or(|at| at > now()),
            ControlErrorCode::NotReady,
        )?;
        let admission = self
            .positive_admission(
                owner,
                &view.explicit_group,
                identity.group_id.is_some(),
                false,
                view.expires_at,
                frame,
                &stop,
            )
            .await?;
        self.token_core(|core| {
            require(
                self.publication.load(Ordering::SeqCst) == admission.frame.epoch,
                ControlErrorCode::RevisionConflict,
            )?;
            core.store.validate_token_access(id, revision, &view)?;
            admission.check(core, now())
        })?;
        let guard = || admission.frame.guard(self, &stop);
        let api = client.managed_tokens(owner);
        let api = api.fenced(&guard);
        let secret = api.reveal(native).await.map_err(|e| error(code(&e)))?;
        self.token_core(|core| {
            require(guard(), ControlErrorCode::RevisionConflict)?;
            core.store.validate_token_access(id, revision, &view)?;
            admission.check(core, now())
        })?;
        Ok(secret)
    }
}
