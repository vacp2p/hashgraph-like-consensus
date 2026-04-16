# Hashgraph-like Consensus

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Crates.io](https://img.shields.io/crates/v/hashgraph-like-consensus.svg)](https://crates.io/crates/hashgraph-like-consensus)
[![GitHub Workflow Status](https://img.shields.io/github/actions/workflow/status/vacp2p/hashgraph-like-consensus/ci.yml?branch=main&label=CI)](https://github.com/vacp2p/hashgraph-like-consensus/actions)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](https://doc.rust-lang.org/edition-guide/)

A lightweight Rust library for making binary decisions in peer-to-peer or gossipsub networks.
Perfect for group governance, voting systems, or any scenario where you need distributed agreement.

## Features

- **Fast** - Reaches consensus in O(log n) rounds
- **Byzantine fault tolerant** - Correct even if up to 1/3 of peers are malicious
- **Pluggable storage** - In-memory by default; implement `ConsensusStorage` for persistence
- **Network-agnostic** - Works with both Gossipsub (fixed 2-round) and P2P (dynamic rounds) topologies
- **Event-driven** - Subscribe to consensus outcomes via a broadcast event bus
- **Cryptographic integrity** - Votes are signed with secp256k1 and chained in a hashgraph structure

Based on the [Hashgraph-like Consensus Protocol RFC](https://lip.logos.co/ift-ts/raw/consensus-hashgraphlike.html).

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
hashgraph-like-consensus = { git = "https://github.com/vacp2p/hashgraph-like-consensus" }
```

## Quick Start

```rust
use hashgraph_like_consensus::{
    scope::ScopeID,
    service::DefaultConsensusService,
    types::CreateProposalRequest,
};
use alloy::signers::local::PrivateKeySigner;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("example-scope");
    let signer = PrivateKeySigner::random();

    // Create a proposal
    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                "Upgrade contract".into(),   // name
                b"Switch to v2".to_vec(),     // payload (bytes)
                signer.address().as_slice().to_vec(), // owner
                3,                           // expected voters
                60,                          // expiration (seconds from now)
                true,                        // liveness: silent peers count as YES at timeout
            )?,
        )
        .await?;

    // Cast a vote
    let vote = service
        .cast_vote(&scope, proposal.proposal_id, true, signer)
        .await?;
    println!("Recorded vote {}", vote.vote_id);

    Ok(())
}
```

## Core Concepts

### Scopes and Proposals

A **scope** groups related proposals together and carries default configuration
(network type, threshold, timeout). Proposals inherit scope defaults unless
overridden individually.

``` text
Scope (group / channel)
  ├── ScopeConfig (defaults for all proposals)
  └── Proposals
       ├── Proposal 1 → Session (inherits scope config)
       ├── Proposal 2 → Session (inherits scope config)
       └── Proposal 3 → Session (overrides scope config)
```

### Network Types

| Type                    | Rounds               | Behavior                                          |
| ----------------------- | -------------------- | ------------------------------------------------- |
| **Gossipsub** (default) | Fixed 2 rounds       | Round 1 = proposal broadcast, Round 2 = all votes |
| **P2P**                 | Dynamic `ceil(2n/3)` | Each vote advances the round by one               |

## API Reference

### Creating a Service

```rust
use hashgraph_like_consensus::service::DefaultConsensusService;

// Default: in-memory storage, 10 max sessions per scope
let service = DefaultConsensusService::default();

// Custom session limit
let service = DefaultConsensusService::new_with_max_sessions(20);

// Fully custom: plug in your own storage and event bus
let service = ConsensusService::new_with_components(my_storage, my_event_bus, 10);
```

### Configuring a Scope

```rust
use hashgraph_like_consensus::{
    scope::ScopeID,
    scope_config::NetworkType,
    service::DefaultConsensusService,
};
use std::time::Duration;

let service = DefaultConsensusService::default();
let scope = ScopeID::from("team_votes");

// Initialize with the builder
service
    .scope(&scope)
    .await?
    .with_network_type(NetworkType::P2P)
    .with_threshold(0.75)
    .with_timeout(Duration::from_secs(120))
    .with_liveness_criteria(false)
    .initialize()
    .await?;

// Update later (single field)
service
    .scope(&scope)
    .await?
    .with_threshold(0.8)
    .update()
    .await?;
```

Built-in presets are also available:

```rust
// High confidence (threshold = 0.9)
service.scope(&scope).await?.strict_consensus().initialize().await?;

// Low latency (threshold = 0.6, timeout = 30 s)
service.scope(&scope).await?.fast_consensus().initialize().await?;
```

### Working with Proposals

```rust
// Create a proposal
let proposal = service
    .create_proposal(&scope, CreateProposalRequest::new(
        "Upgrade contract".into(),
        b"Switch to v2".to_vec(),
        owner_address,
        3,     // expected voters
        60,    // expiration (seconds from now)
        true,  // liveness: silent peers count as YES at timeout
    )?)
    .await?;

// Process a proposal received from the network
service.process_incoming_proposal(&scope, proposal).await?;

// List active proposals
let active: Option<Vec<Proposal>> = service.get_active_proposals(&scope).await?;

// List finalized proposals (proposal_id -> result)
let finalized: Option<HashMap<u32, bool>> = service.get_reached_proposals(&scope).await?;
```

### Casting and Processing Votes

```rust
// Cast your vote (yes = true, no = false)
let vote = service.cast_vote(&scope, proposal_id, true, signer).await?;

// Cast a vote and get the updated proposal (useful for gossiping)
let proposal = service
    .cast_vote_and_get_proposal(&scope, proposal_id, true, signer)
    .await?;

// Process a vote received from the network
service.process_incoming_vote(&scope, vote).await?;
```

### Checking Results

```rust
// Get the final consensus result (Ok(true) = YES, Ok(false) = NO)
let result: bool = service.get_consensus_result(&scope, proposal_id).await?;

// Check if enough votes have been collected
let enough: bool = service
    .has_sufficient_votes_for_proposal(&scope, proposal_id)
    .await?;
```

### Handling Timeouts

When a proposal's timeout fires, call `handle_consensus_timeout` to finalize it.
At timeout, silent peers (those who never voted) are counted toward quorum so that
the `liveness_criteria_yes` flag can take effect:

- **`liveness_criteria_yes = true`** — silent peers are counted as YES votes. A proposal
  passes unless there are enough explicit NO votes to block it.
- **`liveness_criteria_yes = false`** — silent peers are counted as NO votes. A proposal
  fails unless there are enough explicit YES votes to carry it.

The only case where timeout produces no result is a **tie** (equal YES and NO weight
after counting silent peers), which marks the session as failed.

```rust
// Schedule a timeout (typically via tokio::time::sleep)
tokio::time::sleep(config.consensus_timeout()).await;

match service.handle_consensus_timeout(&scope, proposal_id).await {
    Ok(true)  => println!("Consensus: YES"),
    Ok(false) => println!("Consensus: NO"),
    Err(ConsensusError::InsufficientVotesAtTimeout) => {
        println!("Tied — no consensus");
    }
    Err(e) => eprintln!("Error: {e}"),
}
```

During normal voting (before timeout), the quorum gate still requires `ceil(2n/3)`
actual votes — silent peers are not counted until timeout.

### Subscribing to Events

```rust
use hashgraph_like_consensus::types::ConsensusEvent;

let mut rx = service.subscribe_to_events();

tokio::spawn(async move {
    while let Ok((scope, event)) = rx.recv().await {
        match event {
            ConsensusEvent::ConsensusReached { proposal_id, result, timestamp } => {
                println!("Proposal {} → {}", proposal_id, if result { "YES" } else { "NO" });
            }
            ConsensusEvent::ConsensusFailed { proposal_id, timestamp } => {
                println!("Proposal {} failed to reach consensus", proposal_id);
            }
        }
    }
});
```

### Statistics

```rust
let stats = service.get_scope_stats(&scope).await;
println!(
    "Active: {}, Reached: {}, Failed: {}",
    stats.active_sessions, stats.consensus_reached, stats.failed_sessions
);
```

## Advanced Usage

### Custom Storage

Implement the `ConsensusStorage` trait to persist proposals to a database:

```rust
use hashgraph_like_consensus::storage::ConsensusStorage;

pub trait ConsensusStorage<Scope> {
    async fn save_session(&self, scope: &Scope, session: ConsensusSession) -> Result<()>;
    async fn get_session(&self, scope: &Scope, proposal_id: u32) -> Result<Option<ConsensusSession>>;
    // ... see storage.rs for the full trait
}
```

### Custom Event Bus

Implement `ConsensusEventBus` for alternative event delivery:

```rust
use hashgraph_like_consensus::events::ConsensusEventBus;

pub trait ConsensusEventBus<Scope> {
    type Receiver;
    fn subscribe(&self) -> Self::Receiver;
    fn publish(&self, scope: Scope, event: ConsensusEvent);
}
```

### Utility Functions

The `utils` module provides low-level helpers:

| Function                       | Description                                                                                                                                           |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `validate_proposal()`          | Validate a proposal and its votes                                                                                                                     |
| `validate_vote()`              | Verify a vote's signature and structure                                                                                                               |
| `validate_vote_chain()`        | Ensure parent/received hash chains are correct                                                                                                        |
| `has_sufficient_votes()`       | Quick threshold check (count-based)                                                                                                                   |
| `calculate_consensus_result()` | Determine result from collected votes using threshold and liveness rules. Accepts an `is_timeout` flag to count silent peers toward quorum at timeout |

## Building

```bash
# Build
cargo build

# Run tests
cargo test

# Generate docs
cargo doc --open
```

> **Note:** Requires a working `protoc` (Protocol Buffers compiler) since the library generates code from `.proto` files at build time.

## License

[MIT](https://opensource.org/licenses/MIT)
