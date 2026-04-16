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
//! # What the library does NOT do
//!
//! This library handles **consensus calculation** (vote validation, hashgraph
//! verification, threshold math, liveness rules). It performs **no I/O and no
//! orchestration**. Your application is responsible for:
//!
//! - **Network propagation** — gossip proposals and votes to peers yourself;
//!   call [`process_incoming_proposal`](service::ConsensusService::process_incoming_proposal)
//!   / [`process_incoming_vote`](service::ConsensusService::process_incoming_vote) on receipt.
//! - **Timeout scheduling** — schedule a timer per proposal and call
//!   [`handle_consensus_timeout`](service::ConsensusService::handle_consensus_timeout)
//!   when it fires. Without this, proposals with offline voters stay `Active`
//!   forever and silent-peer liveness logic never runs.
//! - **`expected_voters_count` accuracy** — this drives all threshold math;
//!   a wrong value produces wrong results.
//! - **Session eviction awareness** — the default service keeps at most 10
//!   sessions per scope; older sessions are silently dropped.
//!
//! # Architecture
//!
//! [`ConsensusService`](service::ConsensusService) is the single entry point.
//! It is generic over two pluggable backends:
//!
//! - [`ConsensusStorage`](storage::ConsensusStorage) — where sessions and votes
//!   are persisted (in-memory, database, etc.)
//! - [`ConsensusEventBus`](events::ConsensusEventBus) — how consensus events
//!   are delivered (broadcast channel, message queue, etc.)
//!
//! All consensus business logic lives in the service. The backends are pure
//! infrastructure — swap them without changing any consensus behavior.
//! Use [`storage()`](service::ConsensusService::storage) and
//! [`event_bus()`](service::ConsensusService::event_bus) to access the backends
//! directly for reads, cleanup, and event subscription.
//!
//! # Getting started
//!
//! The main entry point is [`service::DefaultConsensusService`] (a type alias for
//! [`service::ConsensusService`] with in-memory storage and broadcast events).
//!
//! ```rust,no_run
//! use hashgraph_like_consensus::{
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
//!             true, // liveness: silent peers count as YES at timeout
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
//! | [`service`] | [`ConsensusService`](service::ConsensusService) and [`DefaultConsensusService`](service::DefaultConsensusService) |
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

pub mod error;
pub mod events;
pub mod scope;
pub mod scope_config;
pub mod service;
pub mod service_stats;
pub mod session;
pub mod storage;
pub mod types;
pub mod utils;
