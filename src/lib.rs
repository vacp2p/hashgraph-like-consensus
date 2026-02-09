//! A lightweight consensus library for making binary decisions in P2P networks.
//!
//! This library implements the [Hashgraph-like Consensus Protocol](https://github.com/vacp2p/rfc-index/blob/main/vac/raw/consensus-hashgraphlike.md),
//! which lets groups of peers vote on proposals and reach agreement even when some peers are
//! unreliable or malicious. It's designed to be fast (O(log n) rounds), secure (Byzantine fault
//! tolerant), and easy to embed in your application.
//!
//! # How it works
//!
//! 1. Someone creates a [`types::CreateProposalRequest`] within a [`scope`].
//! 2. Peers cast votes (yes/no), each cryptographically signed and linked into a hashgraph.
//! 3. Once enough votes are collected the session transitions to
//!    [`session::ConsensusState::ConsensusReached`] and a [`types::ConsensusEvent`] is emitted.
//!
//! # Getting started
//!
//! The main entry point is [`service::DefaultConsensusService`] (a type alias for
//! [`service::ConsensusService`] with in-memory storage and broadcast events).
//!
//! ```rust,no_run
//! use hashgraph_like_consensus::{
//!     api::ConsensusServiceAPI,
//!     scope::ScopeID,
//!     service::DefaultConsensusService,
//!     types::CreateProposalRequest,
//! };
//! use alloy::signers::local::PrivateKeySigner;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let service = DefaultConsensusService::default();
//! let scope = ScopeID::from("my-scope");
//! let signer = PrivateKeySigner::random();
//!
//! let proposal = service
//!     .create_proposal(
//!         &scope,
//!         CreateProposalRequest::new(
//!             "Upgrade contract".into(),
//!             b"Switch to v2".to_vec(),
//!             signer.address().as_slice().to_vec(),
//!             3,    // expected voters
//!             60,   // expiration (seconds from now)
//!             true, // tie-breaker: YES wins on equality
//!         )?,
//!     )
//!     .await?;
//!
//! let vote = service
//!     .cast_vote(&scope, proposal.proposal_id, true, signer)
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`service`] | Main [`ConsensusService`](service::ConsensusService) and [`DefaultConsensusService`](service::DefaultConsensusService) |
//! | [`api`] | [`ConsensusServiceAPI`](api::ConsensusServiceAPI) trait defining the public contract |
//! | [`session`] | [`ConsensusSession`](session::ConsensusSession), [`ConsensusConfig`](session::ConsensusConfig), and [`ConsensusState`](session::ConsensusState) |
//! | [`scope`] | [`ConsensusScope`](scope::ConsensusScope) trait and [`ScopeID`](scope::ScopeID) alias |
//! | [`scope_config`] | Per-scope defaults ([`ScopeConfig`](scope_config::ScopeConfig), [`NetworkType`](scope_config::NetworkType)) |
//! | [`types`] | Request/event types ([`CreateProposalRequest`](types::CreateProposalRequest), [`ConsensusEvent`](types::ConsensusEvent)) |
//! | [`storage`] | [`ConsensusStorage`](storage::ConsensusStorage) trait and [`InMemoryConsensusStorage`](storage::InMemoryConsensusStorage) |
//! | [`events`] | [`ConsensusEventBus`](events::ConsensusEventBus) trait and [`BroadcastEventBus`](events::BroadcastEventBus) |
//! | [`error`] | [`ConsensusError`](error::ConsensusError) enum |
//! | [`utils`] | Low-level validation and hashing helpers |

pub mod protos {
    pub mod consensus {
        pub mod v1 {
            include!(concat!(env!("OUT_DIR"), "/consensus.v1.rs"));
        }
    }
}

pub mod api;
pub mod error;
pub mod events;
pub mod scope;
pub mod scope_config;
pub mod service;
pub mod service_consensus;
pub mod service_stats;
pub mod session;
pub mod storage;
pub mod types;
pub mod utils;
