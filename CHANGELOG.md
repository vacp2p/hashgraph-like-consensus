# Changelog

## 0.4.0

**Breaking** — `ConsensusService` now holds its peer's signer instead of
taking one per call. `cast_vote` and friends drop the signer parameter.

### Changed

- `ConsensusService<Scope, Storage, Event, Signer>` now stores a `Signer`
  instance (previously `PhantomData<fn() -> Signer>`). The signer represents
  this peer's identity for the lifetime of the service; storage, event bus,
  and signer are all peer-scoped held state.
- `ConsensusSignatureScheme` now requires `Clone + Send + Sync + 'static`,
  matching `ConsensusStorage` and `ConsensusEventBus`.
- `cast_vote` and `cast_vote_and_get_proposal` drop the `signer` parameter
  and use the service's held signer:
  ```rust
  // before
  service.cast_vote(&scope, id, choice, signer).await?;
  // after
  service.cast_vote(&scope, id, choice).await?;
  ```
- `ConsensusService::new_with_components` now takes the signer:
  `new_with_components(storage, event_bus, signer, max_sessions_per_scope)`.
- `DefaultConsensusService::new(signer)` and
  `new_with_max_sessions(signer, max)` require a signer. `Default::default()`
  is removed (no sensible identity to fabricate).
- `utils::build_vote` now takes `signer: &Signer` (was by value).

### Added

- `ConsensusService::signer(&self) -> &Signer` accessor, mirroring
  `storage()` and `event_bus()`.

### Migration

```rust
// before
let service = DefaultConsensusService::default();
service.cast_vote(&scope, id, true, signer).await?;

// after
let signer = EthereumConsensusSigner::new(PrivateKeySigner::random());
let service = DefaultConsensusService::new(signer);
service.cast_vote(&scope, id, true).await?;
```

For multi-peer setups (one process simulating many identities), construct
one `ConsensusService` per peer with shared storage and (optionally) a
shared event bus — see the README "Service shape" section.

## 0.3.0

**Breaking** — the crate is no longer hard-coded to Ethereum signing.

### Added

- New [`signing`](src/signing.rs) module with a single
  `ConsensusSignatureScheme` trait. Each peer implements it for signing
  (`identity`, `sign`) and the same type's static `verify` method is used by
  the service to validate incoming votes. Embedders can plug in Ed25519,
  HSM-backed signers, libchat accounts, or any other scheme without forking
  the crate.
- `signing::ConsensusSchemeError` — a `thiserror`-based enum with `Sign` and
  `Verify` variants, returned by both scheme operations. Implementors
  construct variants directly (e.g. `ConsensusSchemeError::Sign(msg)`).
- `signing::EthereumConsensusSigner` provides the default ECDSA-secp256k1
  implementation matching the historical behavior of the crate. It wraps an
  `alloy::signers::local::PrivateKeySigner` and verifies via recoverable
  signature recovery against 20-byte addresses.
- New `tests/custom_scheme_tests.rs` exercises the generic path end-to-end
  with a non-Ethereum stub scheme.

### Changed

- `ConsensusService` now has a fourth generic parameter `Signer:
  ConsensusSignatureScheme` carried as `PhantomData`. The scheme is selected
  at construction (typically via the `DefaultConsensusService` alias for
  Ethereum, or by spelling out the type explicitly for custom schemes).
- `cast_vote` / `cast_vote_and_get_proposal` now take `signer: Signer` instead of a
  bare `alloy_signer::Signer` — the signer must impl the scheme the service
  is parameterized over.
- `utils::build_vote`, `utils::validate_proposal`, and
  `utils::validate_vote` are generic over the scheme; downstream callers
  pick a type at the call site (turbofish or inference).
- `ConsensusSession::from_proposal` / `initialize_with_votes` accept the
  scheme as a generic parameter instead of receiving an explicit verifier.

### Removed

- The hard-coded `SIGNATURE_LENGTH` constant and 65-byte signature assumption
  in `utils.rs`. Length and format are now scheme-specific.
- `ConsensusError::MismatchedLength` and `ConsensusError::InvalidSignature`
  variants. Scheme-specific length/encoding errors surface as
  `ConsensusError::FailedToVerifyVote(ConsensusVerifyError)`.
- `From<alloy_signer::Error>` for `ConsensusError`. Sign failures now go
  through `ConsensusSignError` (wrapping any concrete error type).

### Migration

If you previously did:

```rust
use alloy::signers::local::PrivateKeySigner;
use hashgraph_like_consensus::service::DefaultConsensusService;

let service = DefaultConsensusService::default();
let signer = PrivateKeySigner::random();
service.cast_vote(&scope, proposal_id, true, signer).await?;
```

Wrap the signer in `EthereumConsensusSigner`:

```rust
use alloy::signers::local::PrivateKeySigner;
use hashgraph_like_consensus::{
    service::DefaultConsensusService,
    signing::EthereumConsensusSigner,
};

let service = DefaultConsensusService::default();
let signer = EthereumConsensusSigner::new(PrivateKeySigner::random());
service.cast_vote(&scope, proposal_id, true, signer).await?;
```

If you call `validate_proposal` directly, add a turbofish:

```rust
use hashgraph_like_consensus::{signing::EthereumConsensusSigner, utils::validate_proposal};

validate_proposal::<EthereumConsensusSigner>(&proposal)?;
```

## 0.2.0

- Removed the `ConsensusServiceAPI` trait.
- Timeout liveness fix.
- Storage query defaults.
- Visibility cleanup.
