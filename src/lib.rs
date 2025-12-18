//! A lightweight consensus library for making binary decisions in P2P networks.
//!
//! This library implements the [Hashgraph-like Consensus Protocol](https://github.com/vacp2p/rfc-index/blob/main/vac/raw/consensus-hashgraphlike.md),
//! which lets groups of peers vote on proposals and reach agreement even when some peers are
//! unreliable or malicious. It's designed to be fast (O(log n) rounds), secure (Byzantine fault
//! tolerant), and easy to embed in your application.
//!
//! ## How it works
//!
//! When someone wants to make a decision, they create a proposal. Other peers vote yes or no,
//! and each vote includes cryptographic signatures and links to previous votes (creating a hashgraph).
//! Once enough votes are collected, the proposal reaches consensus and everyone knows the result.
//!
//! The main entry point is [`service::ConsensusService`], which handles creating proposals,
//! processing votes, and managing timeouts. You can use the default in-memory storage or plug in
//! your own storage backend.

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
pub mod service_consensus;
pub mod service_stats;
pub mod session;
pub mod storage;
pub mod types;
pub mod utils;
