# Hashgraph-like Consensus

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

A lightweight Rust library for making binary decisions in peer-to-peer or gossipsub networks.
Perfect for group governance, voting systems, or any scenario where you need distributed agreement.

## What is this?

This library helps groups of peers vote on proposals and reach consensus,
even when some peers are offline or trying to cause trouble.
It's based on the [Hashgraph-like Consensus Protocol RFC](https://github.com/vacp2p/rfc-index/blob/main/vac/raw/consensus-hashgraphlike.md), which means:

- **Fast**: Reaches consensus in O(log n) rounds, so it scales well
- **Secure**: Works correctly even if up to 1/3 of peers are malicious (Byzantine fault tolerant)
- **Simple**: Easy to embed in your application with a clean API

## How it works

1. Someone creates a proposal (like "Should we upgrade to version 2?")
2. Peers vote yes or no, with each vote cryptographically signed
3. Votes link together in a hashgraph structure (like a blockchain, but more efficient)
4. Once enough votes are collected, consensus is reached and everyone knows the result

The library handles all the tricky parts: validating signatures,
checking vote chains, managing timeouts, and determining when consensus is reached.

## Quick start

```rust
use hashgraph_like_consensus::{
    scope::ScopeID,
    service::DefaultConsensusService,
    types::CreateProposalRequest,
};
use alloy::signers::local::PrivateKeySigner;

#[tokio::main]
async fn main() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("example-scope");
    let owner = PrivateKeySigner::random();

    let proposal = service.create_proposal(
        &scope,
        CreateProposalRequest::new(
            "Upgrade contract".into(),
            "Switch to v2".into(),
            owner.address().as_slice().to_vec(),
            3,       // expected voters
            60,      // expiration in seconds
            true,    // tie-breaker favors YES on equality
        ).unwrap(),
    ).await.unwrap();

    let vote = service.cast_vote(&scope, proposal.proposal_id, true, owner).await.unwrap();
    println!("Recorded vote {:?}", vote.vote_id);
}
```

## API Overview

### Configuring a Scope

#### Scope vs Proposal Relationship

``` bash
Scope (Group/Channel)
  ├── ScopeConfig (defaults for all proposals)
  └── Proposals
       ├── Proposal 1 → Session 1 (inherits scope config)
       ├── Proposal 2 → Session 2 (inherits scope config)
       └── Proposal 3 → Session 3 (overrides scope config)
```

Scopes carry defaults (network type, thresholds, timeouts)
so you don't have to pass config on every proposal. Use the builder to initialize or update a scope:

```rust
use hashgraph_like_consensus::{
    scope::ScopeID,
    scope_config::NetworkType,
    service::DefaultConsensusService,
};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let service = DefaultConsensusService::default();
let scope = ScopeID::from("team_votes");

service
    .scope(&scope)
    .await?
    .with_network_type(NetworkType::P2P)
    .with_threshold(0.75)
    .with_timeout(120)
    .with_liveness_criteria(false)
    .initialize()
    .await?;
# Ok(())
# }
```

### Creating a Service

```rust
// Simple: use defaults (in-memory storage, 10 max sessions per scope)
let service = DefaultConsensusService::default();

// Custom: set your own session limit
let service = DefaultConsensusService::new_with_max_sessions(20);

// Advanced: plug in your own storage and event bus
let service = ConsensusService::new_with_components(
    my_storage,
    my_event_bus,
    10
);
```

### Working with Proposals

**Create a proposal** - Start a new voting session:

```rust
let proposal = service.create_proposal(
    &scope,
    CreateProposalRequest::new(
        "Upgrade contract".into(),
        "Switch to v2".into(),
        owner.address().as_slice().to_vec(),
        3,    // expected voters
        60,   // expiration in seconds
        true, // tie-breaker: YES wins on equality
    )?
).await?;
```

**Process incoming proposals** - When you receive a proposal from the network:

```rust
service.process_incoming_proposal(&scope, proposal).await?;
```

**List proposals** - See what's active or finalized:

```rust
let active = service.get_active_proposals(&scope).await;
let finalized = service.get_reached_proposals(&scope).await;
```

### Casting Votes

**Cast your vote** - Vote yes or no on a proposal:

```rust
let vote = service.cast_vote(&scope, proposal_id, true, signer).await?;
```

**Vote and fetch proposal** - Useful for the creator who wants to gossip the updated proposal:

```rust
let proposal = service.cast_vote_and_get_proposal(&scope, proposal_id, true, signer).await?;
```

### Picking a config for your transport

- Gossipsub (default): `ConsensusConfig::gossipsub()` enforces the RFC’s
  two-round flow (round 1 = proposal, round 2 = everyone’s votes).
- P2P: use `ConsensusConfig::p2p()` to derive the round cap dynamically (ceil(2n/3) of expected voters)
  and advance one round per vote.

Pass configs via `create_proposal_with_config` (or helper methods) when you need explicit control.
Scope defaults are usually easier: set them once with
`service.scope(&scope).await?.with_network_type(...)...initialize().await?`,
then override per proposal only when needed.

**Process incoming votes** - When you receive votes from other peers:

```rust
service.process_incoming_vote(&scope, vote).await?;
```

### Checking Results

**Get consensus result** - See if a proposal has reached consensus:

```rust
if let Some(result) = service.get_consensus_result(&scope, proposal_id).await {
    println!("Consensus reached: {}", result);
}
```

**Check vote count** - See if enough votes have been collected:

```rust
let enough_votes = service
    .has_sufficient_votes_for_proposal(&scope, proposal_id)
    .await?;
```

### Events

**Subscribe to events** - Get notified when consensus is reached or fails:

```rust
use hashgraph_like_consensus::types::ConsensusEvent;

let mut receiver = service.subscribe_to_events();
tokio::spawn(async move {
    while let Ok((scope, event)) = receiver.recv().await {
        match event {
            ConsensusEvent::ConsensusReached { proposal_id, result } => {
                println!("Proposal {} reached consensus: {}", proposal_id, result);
            }
            ConsensusEvent::ConsensusFailed { proposal_id, reason } => {
                println!("Proposal {} failed: {}", proposal_id, reason);
            }
        }
    }
});
```

### Statistics

**Get scope statistics** - See how many proposals are active, finalized, etc:

```rust
let stats = service.get_scope_stats(&scope).await;
println!("Active: {}, Finalized: {}", stats.active_sessions, stats.consensus_reached);
```

### Advanced Usage

**Custom storage** - Want to persist proposals to a database? Implement the `ConsensusStorage` trait:

```rust
pub trait ConsensusStorage<Scope> {
    async fn save_session(&self, scope: &Scope, session: ConsensusSession) -> Result<()>;
    async fn get_session(&self, scope: &Scope, proposal_id: u32) -> Result<Option<ConsensusSession>>;
    // ... more methods
}
```

**Custom events** - Need different event handling? Implement `ConsensusEventBus`:

```rust
pub trait ConsensusEventBus<Scope> {
    type Receiver;
    fn subscribe(&self) -> Self::Receiver;
    fn publish(&self, scope: Scope, event: ConsensusEvent);
}
```

**Utility functions** - The `utils` module provides helpers for validation and ID generation:

- `validate_proposal()` - Check if a proposal and its votes are valid
- `validate_vote()` - Verify a single vote's signature and structure
- `validate_vote_chain()` - Ensure vote parent/received hash chains are correct
- `has_sufficient_votes()` - Quick threshold check (count-based only)
- `calculate_consensus_result(votes, expected_voters, consensus_threshold, liveness_criteria_yes)` -
  Determine the result from collected votes (returns `Some(result)` when consensus is reached,
  otherwise `None`), using the configured threshold and liveness rules

## Learn More

This library implements the [Hashgraph-like Consensus Protocol RFC](https://github.com/vacp2p/rfc-index/blob/main/vac/raw/consensus-hashgraphlike.md).
For details on how the protocol works, security guarantees, and edge cases, check out the RFC.
