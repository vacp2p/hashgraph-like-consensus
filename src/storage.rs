//! Storage trait and default in-memory implementation.
//!
//! Implement [`ConsensusStorage`] to persist consensus sessions to a database or
//! other durable backend. The provided [`InMemoryConsensusStorage`] keeps everything
//! in RAM and is suitable for testing or single-node deployments.

use async_stream::try_stream;
use futures::Stream;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

use crate::{
    error::ConsensusError,
    protos::consensus::v1::Proposal,
    scope::ConsensusScope,
    scope_config::ScopeConfig,
    session::{ConsensusConfig, ConsensusSession},
};

/// Trait for storing and retrieving consensus sessions.
///
/// Implement this to use your own storage backend (database, file system, etc.).
/// The default `InMemoryConsensusStorage` stores everything in RAM, which is fine
/// for testing or single-node setups but won't persist across restarts.
pub trait ConsensusStorage<Scope>: Clone + Send + Sync + 'static
where
    Scope: ConsensusScope,
{
    /// Persist a session (insert or overwrite by `proposal_id`).
    fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// Retrieve a session by proposal ID, or `None` if it doesn't exist.
    fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Option<ConsensusSession>, ConsensusError>> + Send;

    /// Remove and return a session, or `None` if not found.
    fn remove_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Option<ConsensusSession>, ConsensusError>> + Send;

    /// List all sessions in a scope, or `None` if the scope doesn't exist.
    fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<Option<Vec<ConsensusSession>>, ConsensusError>> + Send;

    /// Stream sessions in a scope one at a time (useful for large scopes).
    fn stream_scope_sessions<'a>(
        &'a self,
        scope: &'a Scope,
    ) -> impl Stream<Item = Result<ConsensusSession, ConsensusError>> + Send + 'a;

    /// Replace all sessions in a scope atomically.
    fn replace_scope_sessions(
        &self,
        scope: &Scope,
        sessions: Vec<ConsensusSession>,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// List all known scopes, or `None` if no scopes exist.
    fn list_scopes(
        &self,
    ) -> impl Future<Output = Result<Option<Vec<Scope>>, ConsensusError>> + Send;

    /// Apply a mutation to a single session in place.
    fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> impl Future<Output = Result<R, ConsensusError>> + Send
    where
        R: Send,
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError> + Send;

    /// Apply a mutation to all sessions in a scope (e.g. trimming old entries).
    fn update_scope_sessions<F>(
        &self,
        scope: &Scope,
        mutator: F,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send
    where
        F: FnOnce(&mut Vec<ConsensusSession>) -> Result<(), ConsensusError> + Send;

    /// Get the scope-level configuration, or `None` if not yet initialized.
    fn get_scope_config(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<Option<ScopeConfig>, ConsensusError>> + Send;

    /// Set (insert or overwrite) the scope-level configuration.
    fn set_scope_config(
        &self,
        scope: &Scope,
        config: ScopeConfig,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// Remove all data for a scope (sessions, config, everything).
    ///
    /// Called when a group is left or deleted. After this call, the scope
    /// behaves as if it was never initialized — creating new proposals on it
    /// starts fresh.
    fn delete_scope(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// Apply a mutation to an existing scope configuration.
    fn update_scope_config<F>(
        &self,
        scope: &Scope,
        updater: F,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send
    where
        F: FnOnce(&mut ScopeConfig) -> Result<(), ConsensusError> + Send;

    // ── Query helpers (default implementations) ────────────────────────
    //
    // These are derived from the primitives above. Storage implementors
    // get them for free — override only if your backend can do it faster.

    /// Get the consensus result for a proposal.
    ///
    /// Returns `Ok(true)` for YES, `Ok(false)` for NO.
    /// Returns [`SessionNotFound`](ConsensusError::SessionNotFound) if the
    /// proposal doesn't exist,
    /// [`ConsensusFailed`](ConsensusError::ConsensusFailed) if the session
    /// failed, or [`ConsensusNotReached`](ConsensusError::ConsensusNotReached)
    /// if voting is still active.
    fn get_consensus_result(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<bool, ConsensusError>> + Send {
        async move {
            use crate::session::ConsensusState;
            let session = self
                .get_session(scope, proposal_id)
                .await?
                .ok_or(ConsensusError::SessionNotFound)?;
            match session.state {
                ConsensusState::ConsensusReached(result) => Ok(result),
                ConsensusState::Failed => Err(ConsensusError::ConsensusFailed),
                ConsensusState::Active => Err(ConsensusError::ConsensusNotReached),
            }
        }
    }

    /// Get a proposal by ID.
    ///
    /// Returns [`SessionNotFound`](ConsensusError::SessionNotFound) if the
    /// proposal doesn't exist.
    fn get_proposal(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send {
        async move {
            let session = self
                .get_session(scope, proposal_id)
                .await?
                .ok_or(ConsensusError::SessionNotFound)?;
            Ok(session.proposal)
        }
    }

    /// Get the resolved configuration for a proposal.
    ///
    /// Returns [`SessionNotFound`](ConsensusError::SessionNotFound) if the
    /// proposal doesn't exist.
    fn get_proposal_config(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<ConsensusConfig, ConsensusError>> + Send {
        async move {
            let session = self
                .get_session(scope, proposal_id)
                .await?
                .ok_or(ConsensusError::SessionNotFound)?;
            Ok(session.config)
        }
    }

    /// Get all proposals that are still accepting votes.
    ///
    /// Returns an empty `Vec` if no active proposals exist or the scope is unknown.
    fn get_active_proposals(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<Vec<Proposal>, ConsensusError>> + Send {
        async move {
            let sessions = self.list_scope_sessions(scope).await?.unwrap_or_default();
            Ok(sessions
                .into_iter()
                .filter(|s| s.is_active())
                .map(|s| s.proposal)
                .collect())
        }
    }

    /// Get all proposals that reached consensus, with their results.
    ///
    /// Returns a map from `proposal_id` to result (`true` = YES, `false` = NO).
    /// Returns an empty map if no proposals reached consensus or the scope is unknown.
    fn get_reached_proposals(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<HashMap<u32, bool>, ConsensusError>> + Send {
        async move {
            let sessions = self.list_scope_sessions(scope).await?.unwrap_or_default();
            Ok(sessions
                .into_iter()
                .filter_map(|s| {
                    s.get_consensus_result()
                        .ok()
                        .map(|result| (s.proposal.proposal_id, result))
                })
                .collect())
        }
    }
}

/// In-memory storage for consensus sessions.
///
/// Stores all sessions in RAM using a hash map. This is the default storage implementation
/// and works well for testing or single-node setups. Data is lost when the process exits.
#[derive(Clone)]
pub struct InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    sessions: Arc<RwLock<HashMap<Scope, HashMap<u32, ConsensusSession>>>>,
    scope_configs: Arc<RwLock<HashMap<Scope, ScopeConfig>>>,
}

impl<Scope> Default for InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    fn default() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            scope_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl<Scope> InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    /// Create a new in-memory storage instance.
    ///
    /// This stores all consensus sessions in RAM. Perfect for testing or single-node setups,
    /// but data won't persist across restarts.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<Scope> ConsensusStorage<Scope> for InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope + Clone,
{
    async fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> Result<(), ConsensusError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions.entry(scope.clone()).or_default();
        entry.insert(session.proposal.proposal_id, session);
        Ok(())
    }

    async fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<Option<ConsensusSession>, ConsensusError> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .get(scope)
            .and_then(|scope| scope.get(&proposal_id))
            .cloned())
    }

    async fn remove_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<Option<ConsensusSession>, ConsensusError> {
        let mut sessions = self.sessions.write().await;
        Ok(sessions
            .get_mut(scope)
            .and_then(|scope| scope.remove(&proposal_id)))
    }

    async fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> Result<Option<Vec<ConsensusSession>>, ConsensusError> {
        let sessions = self.sessions.read().await;
        let result = sessions
            .get(scope)
            .map(|scope| scope.values().cloned().collect::<Vec<ConsensusSession>>());
        Ok(result)
    }

    fn stream_scope_sessions<'a>(
        &'a self,
        scope: &'a Scope,
    ) -> impl Stream<Item = Result<ConsensusSession, ConsensusError>> + Send + 'a {
        try_stream! {
            let guard = self.sessions.read().await;

            if let Some(inner_map) = guard.get(scope) {
                for session in inner_map.values() {
                    yield session.clone();
                }
            }
        }
    }

    async fn replace_scope_sessions(
        &self,
        scope: &Scope,
        sessions_list: Vec<ConsensusSession>,
    ) -> Result<(), ConsensusError> {
        let mut sessions = self.sessions.write().await;
        let new_map = sessions_list
            .into_iter()
            .map(|session| (session.proposal.proposal_id, session))
            .collect();
        sessions.insert(scope.clone(), new_map);
        Ok(())
    }

    async fn list_scopes(&self) -> Result<Option<Vec<Scope>>, ConsensusError> {
        let sessions = self.sessions.read().await;
        let result = sessions.keys().cloned().collect::<Vec<Scope>>();
        if result.is_empty() {
            return Ok(None);
        }
        Ok(Some(result))
    }

    async fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> Result<R, ConsensusError>
    where
        R: Send,
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError> + Send,
    {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(scope)
            .and_then(|scope_sessions| scope_sessions.get_mut(&proposal_id))
            .ok_or(ConsensusError::SessionNotFound)?;

        let result = mutator(session)?;
        Ok(result)
    }

    async fn update_scope_sessions<F>(
        &self,
        scope: &Scope,
        mutator: F,
    ) -> Result<(), ConsensusError>
    where
        F: FnOnce(&mut Vec<ConsensusSession>) -> Result<(), ConsensusError> + Send,
    {
        let mut sessions = self.sessions.write().await;
        let scope_sessions = sessions.entry(scope.clone()).or_default();

        let mut sessions_vec: Vec<ConsensusSession> = scope_sessions.values().cloned().collect();
        mutator(&mut sessions_vec)?;

        if sessions_vec.is_empty() {
            sessions.remove(scope);
            return Ok(());
        }

        let new_map: HashMap<u32, ConsensusSession> = sessions_vec
            .into_iter()
            .map(|session| (session.proposal.proposal_id, session))
            .collect();

        *scope_sessions = new_map;
        Ok(())
    }

    async fn get_scope_config(&self, scope: &Scope) -> Result<Option<ScopeConfig>, ConsensusError> {
        let configs = self.scope_configs.read().await;
        Ok(configs.get(scope).cloned())
    }

    async fn set_scope_config(
        &self,
        scope: &Scope,
        config: ScopeConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        let mut configs = self.scope_configs.write().await;
        configs.insert(scope.clone(), config);
        Ok(())
    }

    async fn delete_scope(&self, scope: &Scope) -> Result<(), ConsensusError> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(scope);
        drop(sessions);

        let mut configs = self.scope_configs.write().await;
        configs.remove(scope);
        Ok(())
    }

    async fn update_scope_config<F>(&self, scope: &Scope, updater: F) -> Result<(), ConsensusError>
    where
        F: FnOnce(&mut ScopeConfig) -> Result<(), ConsensusError> + Send,
    {
        let mut configs = self.scope_configs.write().await;
        let config = configs.entry(scope.clone()).or_default();
        updater(config)?;
        config.validate()?;
        Ok(())
    }
}
