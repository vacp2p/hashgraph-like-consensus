//! Pluggable signature scheme for vote authentication.
//!
//! Each vote in the consensus protocol is authenticated by a signature over
//! its canonical encoding. The crate is agnostic to the cryptographic scheme:
//! embedders pick a concrete [`ConsensusSignatureScheme`] type that defines
//! how peers sign and how the service verifies.
//!
//! The wire layer ([`Vote::vote_owner`] and [`Vote::signature`] in the
//! protobuf) is byte-flexible; the scheme gives those bytes their meaning.
//!
//! [`ethereum::EthereumConsensusSigner`] provides a default ECDSA-secp256k1
//! implementation matching the historical behavior of the crate. It is the
//! scheme used by [`DefaultConsensusService`] but the core service is fully
//! generic — pick a different scheme to integrate Ed25519, an HSM, or any
//! other signing system.
//!
//! [`Vote::vote_owner`]: crate::protos::consensus::v1::Vote::vote_owner
//! [`Vote::signature`]: crate::protos::consensus::v1::Vote::signature
//! [`DefaultConsensusService`]: crate::service::DefaultConsensusService

use std::future::Future;

mod ethereum;
pub use ethereum::EthereumConsensusSigner;

/// A signature scheme that the consensus service uses to sign and verify votes.
///
/// Implementors play two roles:
///
/// - **As a signer instance**: a value carrying private state (key material,
///   HSM handle, etc.) produces signatures via [`identity`](Self::identity)
///   and [`sign`](Self::sign). One such instance exists per peer that needs
///   to cast votes.
/// - **As a scheme type**: the type itself is used by the service to verify
///   incoming signatures via the static [`verify`](Self::verify) method.
///   Verification needs no instance — only the public bytes from the wire.
///
/// All peers on a network must agree on the scheme, since they all verify
/// each other's signatures using the same `verify` rule.
pub trait ConsensusSignatureScheme: Send + Sync {
    /// Stable identity bytes for this signer (e.g. address, public key, account id).
    ///
    /// Written into [`Vote::vote_owner`] when the signer casts a vote and
    /// passed back into [`verify`](Self::verify) when the vote is checked.
    ///
    /// [`Vote::vote_owner`]: crate::protos::consensus::v1::Vote::vote_owner
    fn identity(&self) -> &[u8];

    /// Sign `payload` and return the raw signature bytes.
    ///
    /// Length and encoding are scheme-specific.
    fn sign(
        &self,
        payload: &[u8],
    ) -> impl Future<Output = Result<Vec<u8>, ConsensusSchemeError>> + Send;

    /// Verify that `signature` over `payload` was produced by the holder of
    /// `identity`.
    ///
    /// This is a free function on the scheme type — no instance needed. The
    /// service calls it for every incoming vote.
    ///
    /// Returns `Ok(true)` when the signature is valid, `Ok(false)` when it is
    /// well-formed but does not match, and `Err` when the inputs are malformed
    /// (wrong signature length, bad encoding, etc.).
    fn verify(
        identity: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, ConsensusSchemeError>;
}

/// Error returned by [`ConsensusSignatureScheme`] operations.
#[derive(Debug, thiserror::Error)]
pub enum ConsensusSchemeError {
    /// The scheme could not produce a signature for the given payload.
    #[error("Signing failed: {0}")]
    Sign(String),
    /// The scheme rejected the inputs (wrong signature length, unparseable
    /// bytes, identity of unexpected shape, etc.).
    #[error("Verification rejected inputs: {0}")]
    Verify(String),
}
