# Changelog

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
