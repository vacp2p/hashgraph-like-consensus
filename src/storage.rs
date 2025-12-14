use async_stream::try_stream;
use futures::Stream;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

use crate::{
    error::ConsensusError, scope::ConsensusScope, scope_config::ScopeConfig,
    session::ConsensusSession,
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
    fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Option<ConsensusSession>, ConsensusError>> + Send;

    fn remove_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Option<ConsensusSession>, ConsensusError>> + Send;

    fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<Option<Vec<ConsensusSession>>, ConsensusError>> + Send;

    fn stream_scope_sessions<'a>(
        &'a self,
        scope: &'a Scope,
    ) -> impl Stream<Item = Result<ConsensusSession, ConsensusError>> + Send + 'a;

    fn replace_scope_sessions(
        &self,
        scope: &Scope,
        sessions: Vec<ConsensusSession>,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    fn list_scopes(
        &self,
    ) -> impl Future<Output = Result<Option<Vec<Scope>>, ConsensusError>> + Send;

    fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> impl Future<Output = Result<R, ConsensusError>> + Send
    where
        R: Send,
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError> + Send;

    fn update_scope_sessions<F>(
        &self,
        scope: &Scope,
        mutator: F,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send
    where
        F: FnOnce(&mut Vec<ConsensusSession>) -> Result<(), ConsensusError> + Send;

    /// Get scope configuration (defaults for proposals in this scope)
    fn get_scope_config(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<Option<ScopeConfig>, ConsensusError>> + Send;

    /// Set scope configuration
    fn set_scope_config(
        &self,
        scope: &Scope,
        config: ScopeConfig,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// Update scope configuration
    fn update_scope_config<F>(
        &self,
        scope: &Scope,
        updater: F,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send
    where
        F: FnOnce(&mut ScopeConfig) -> Result<(), ConsensusError> + Send;
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
