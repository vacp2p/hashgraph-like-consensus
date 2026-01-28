use alloy_signer::Signer;

use crate::{
    error::ConsensusError,
    events::ConsensusEventBus,
    protos::consensus::v1::{Proposal, Vote},
    scope::ConsensusScope,
    session::ConsensusConfig,
    storage::ConsensusStorage,
    types::CreateProposalRequest,
};

pub trait ConsensusServiceAPI<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    fn create_proposal(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;
    fn create_proposal_with_config(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
        config: Option<ConsensusConfig>,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;

    fn cast_vote<SN: Signer + Sync + Send>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> impl Future<Output = Result<Vote, ConsensusError>> + Send;
    fn cast_vote_and_get_proposal<SN: Signer + Sync + Send>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;

    fn process_incoming_proposal(
        &self,
        scope: &Scope,
        proposal: Proposal,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;
    fn process_incoming_vote(
        &self,
        scope: &Scope,
        vote: Vote,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;
}
