use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

use crate::{error::ConsensusError, scope::ConsensusScope, session::ConsensusSession};

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
}

/// In-memory implementation of [`ConsensusStorage`].
pub struct InMemoryConsensusStorage<Scope>
where
    Scope: ConsensusScope,
{
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
            .and_then(|group| group.get(&proposal_id))
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
            .and_then(|group| group.remove(&proposal_id)))
    }

    async fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> Result<Vec<ConsensusSession>, ConsensusError> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .get(scope)
            .map(|group| group.values().cloned().collect())
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
}
