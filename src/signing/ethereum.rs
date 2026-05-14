//! Default ECDSA-secp256k1 signing scheme.
//!
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy_signer::{Signature, Signer};

use super::{ConsensusSchemeError, ConsensusSignatureScheme};

/// Length of an Ethereum recoverable ECDSA signature in bytes (r || s || v).
const ETHEREUM_SIGNATURE_LENGTH: usize = 65;
/// Length of an Ethereum address in bytes.
const ETHEREUM_ADDRESS_LENGTH: usize = 20;

/// ECDSA-secp256k1 scheme backed by an alloy [`PrivateKeySigner`].
///
/// As a signer instance, holds a 32-byte private key and produces 65-byte
/// recoverable signatures over the secp256k1 curve. The signer's identity is
/// its 20-byte Ethereum address.
///
/// As a scheme type, `EthereumConsensusSigner::verify` is a stateless
/// signature check used by [`ConsensusService`](crate::service::ConsensusService)
/// when this scheme is selected.
#[derive(Debug, Clone)]
pub struct EthereumConsensusSigner {
    inner: PrivateKeySigner,
    address_bytes: Vec<u8>,
}

impl EthereumConsensusSigner {
    pub fn new(signer: PrivateKeySigner) -> Self {
        let address_bytes = signer.address().as_slice().to_vec();
        Self {
            inner: signer,
            address_bytes,
        }
    }

    pub fn inner(&self) -> &PrivateKeySigner {
        &self.inner
    }

    pub fn into_inner(self) -> PrivateKeySigner {
        self.inner
    }
}

impl From<PrivateKeySigner> for EthereumConsensusSigner {
    fn from(signer: PrivateKeySigner) -> Self {
        Self::new(signer)
    }
}

impl ConsensusSignatureScheme for EthereumConsensusSigner {
    fn identity(&self) -> &[u8] {
        &self.address_bytes
    }

    async fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, ConsensusSchemeError> {
        let signature = self
            .inner
            .sign_message(payload)
            .await
            .map_err(|e| ConsensusSchemeError::Sign(e.to_string()))?;
        Ok(signature.as_bytes().to_vec())
    }

    fn verify(
        identity: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, ConsensusSchemeError> {
        if signature.len() != ETHEREUM_SIGNATURE_LENGTH {
            return Err(ConsensusSchemeError::Verify(format!(
                "expected {ETHEREUM_SIGNATURE_LENGTH}-byte signature, got {}",
                signature.len()
            )));
        }
        if identity.len() != ETHEREUM_ADDRESS_LENGTH {
            return Err(ConsensusSchemeError::Verify(format!(
                "expected {ETHEREUM_ADDRESS_LENGTH}-byte address, got {}",
                identity.len()
            )));
        }

        let mut sig_bytes = [0u8; ETHEREUM_SIGNATURE_LENGTH];
        sig_bytes.copy_from_slice(signature);
        let signature = Signature::from_raw_array(&sig_bytes)
            .map_err(|e| ConsensusSchemeError::Verify(e.to_string()))?;
        let recovered = signature
            .recover_address_from_msg(payload)
            .map_err(|e| ConsensusSchemeError::Verify(e.to_string()))?;

        let mut addr = [0u8; ETHEREUM_ADDRESS_LENGTH];
        addr.copy_from_slice(identity);
        let expected = Address::from(addr);

        Ok(recovered == expected)
    }
}
