use alloy_signer::Signer;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    error::ConsensusError,
    events::ConsensusEventBus,
    protos::consensus::v1::{Proposal, Vote},
    scope::ConsensusScope,
    service::ConsensusService,
    session::{ConsensusConfig, ConsensusSession, CreateProposalRequest},
    storage::ConsensusStorage,
    utils::{create_vote_for_proposal, validate_vote},
};

impl<Scope, S, E> ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    pub async fn create_proposal(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
    ) -> Result<Proposal, ConsensusError> {
        let proposal = request.into_proposal()?;
        let proposal_id = proposal.proposal_id;

        let config = ConsensusConfig::default();
        let session = ConsensusSession::new(proposal.clone(), config.clone());
        self.save_session(scope, session).await?;
        self.enforce_scope_limit(scope).await?;
        self.spawn_timeout_task(scope.clone(), proposal_id, config.consensus_timeout);

        Ok(proposal)
    }

    /// Cast a vote and return the signed vote (works for any participant).
    pub async fn cast_vote<SN: Signer + Sync>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> Result<Vote, ConsensusError> {
        let session = self.get_session(scope, proposal_id).await?;

        // RFC Section 2.5.4
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        if now >= session.proposal.expiration_time {
            return Err(ConsensusError::VoteExpired);
        }

        let voter_address = signer.address().as_slice().to_vec();
        if session.votes.contains_key(&voter_address) {
            return Err(ConsensusError::UserAlreadyVoted);
        }

        let vote = create_vote_for_proposal(&session.proposal, choice, signer).await?;
        let vote_clone = vote.clone();

        let transition = self
            .update_session(scope, proposal_id, move |session| {
                session.add_vote(vote_clone)
            })
            .await?;

        self.handle_transition(scope, proposal_id, transition);
        Ok(vote)
    }

    /// Cast a vote and get the updated proposal snapshot. Useful immediately after creation.
    pub async fn cast_vote_and_get_proposal<SN: Signer + Sync>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> Result<Proposal, ConsensusError> {
        self.cast_vote(scope, proposal_id, choice, signer).await?;
        let session = self.get_session(scope, proposal_id).await?;
        Ok(session.proposal)
    }

    pub async fn process_incoming_proposal(
        &self,
        scope: &Scope,
        proposal: Proposal,
    ) -> Result<(), ConsensusError> {
        if self.get_session(scope, proposal.proposal_id).await.is_ok() {
            return Err(ConsensusError::ProposalAlreadyExist);
        }

        let (session, transition) =
            ConsensusSession::from_proposal(proposal, ConsensusConfig::default())?;
        self.handle_transition(scope, session.proposal.proposal_id, transition);

        self.save_session(scope, session).await?;
        self.enforce_scope_limit(scope).await?;
        Ok(())
    }

    pub async fn process_incoming_vote(
        &self,
        scope: &Scope,
        vote: Vote,
    ) -> Result<(), ConsensusError> {
        let session = self.get_session(scope, vote.proposal_id).await?;
        validate_vote(&vote, session.proposal.expiration_time)?;
        let proposal_id = vote.proposal_id;
        let transition = self
            .update_session(scope, proposal_id, move |session| session.add_vote(vote))
            .await?;

        self.handle_transition(scope, proposal_id, transition);
        Ok(())
    }
}
