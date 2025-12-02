use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

use crate::{error::ConsensusError, scope::ConsensusScope, session::ConsensusSession};

/// Trait for storing and retrieving consensus sessions.
///
/// Implement this to use your own storage backend (database, file system, etc.).
/// The default `InMemoryConsensusStorage` stores everything in RAM, which is fine
/// for testing or single-node setups but won't persist across restarts.
#[async_trait::async_trait]
pub trait ConsensusStorage<Scope>: Send + Sync + 'static
where
    Scope: ConsensusScope,
{
    async fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> Result<(), ConsensusError>;

    async fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<Option<ConsensusSession>, ConsensusError>;

    async fn remove_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<Option<ConsensusSession>, ConsensusError>;

    async fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> Result<Vec<ConsensusSession>, ConsensusError>;

    async fn replace_scope_sessions(
        &self,
        scope: &Scope,
        sessions: Vec<ConsensusSession>,
    ) -> Result<(), ConsensusError>;

    async fn list_scopes(&self) -> Result<Vec<Scope>, ConsensusError>;

    async fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> Result<R, ConsensusError>
    where
        R: Send,
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError> + Send;

    async fn update_scope_sessions<F>(
        &self,
        scope: &Scope,
        mutator: F,
    ) -> Result<(), ConsensusError>
    where
        F: FnOnce(&mut Vec<ConsensusSession>) -> Result<(), ConsensusError> + Send;
}

pub struct InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    /// Scope -> proposal_id -> session map held in memory for tests and lightweight services.
    sessions: Arc<RwLock<HashMap<Scope, HashMap<u32, ConsensusSession>>>>,
}

impl<Scope> Default for InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    fn default() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl<Scope> InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl<Scope> ConsensusStorage<Scope> for InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
    async fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> Result<(), ConsensusError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions.entry(scope.clone()).or_insert_with(HashMap::new);
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
    ) -> Result<Vec<ConsensusSession>, ConsensusError> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .get(scope)
            .map(|scope| scope.values().cloned().collect())
            .unwrap_or_default())
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

    async fn list_scopes(&self) -> Result<Vec<Scope>, ConsensusError> {
        let sessions = self.sessions.read().await;
        Ok(sessions.keys().cloned().collect())
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
        let scope_sessions = sessions.entry(scope.clone()).or_insert_with(HashMap::new);

        let mut sessions_vec: Vec<ConsensusSession> = scope_sessions.values().cloned().collect();
        mutator(&mut sessions_vec)?;

        let new_map: HashMap<u32, ConsensusSession> = sessions_vec
            .into_iter()
            .map(|session| (session.proposal.proposal_id, session))
            .collect();

        *scope_sessions = new_map;
        Ok(())
    }
}
