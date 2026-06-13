//! ed25519 signing for receipts.
//!
//! ed25519 (RFC 8032) is deterministic: the same key over the same message
//! yields the same signature. So signing preserves the byte-identical-receipt
//! property; it adds an attestation layer without introducing nondeterminism.
//!
//! A signed receipt is a canonical-CBOR envelope:
//!   { 0: version, 1: receipt-bytes, 2: signature(64), 3: public-key(32) }
//! The `receipt-id` (the trust anchor) remains the SHA-256 of the *unsigned*
//! canonical receipt bytes; the signature attests "this checker build produced
//! exactly these bytes", verifiable by anyone holding the public key.
//!
//! Secrets posture: the 32-byte seed is the private key. This module never
//! writes it anywhere. Callers supply it (e.g. from the OS keychain via an
//! env var); `keygen` returns fresh material for the caller to store.

use crate::cbor::{self, CborError, Value};
use ed25519_dalek::{Signature, Signer, SigningKey as DalekSigning, VerifyingKey};

pub struct SigningKey(DalekSigning);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    BadSeedLength,
    BadHex,
}

impl SigningKey {
    /// Generate fresh key material from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).expect("OS CSPRNG");
        SigningKey(DalekSigning::from_bytes(&seed))
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        SigningKey(DalekSigning::from_bytes(seed))
    }

    /// Parse a 64-char hex seed (e.g. from `EQUIV_SIGNING_KEY`).
    pub fn from_hex(s: &str) -> Result<Self, SignError> {
        let s = s.trim();
        if s.len() != 64 {
            return Err(SignError::BadSeedLength);
        }
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| SignError::BadHex)?;
        }
        Ok(SigningKey::from_seed(&seed))
    }

    /// The secret seed. Handle as a secret; never log or persist to the repo.
    pub fn seed(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.0.sign(msg).to_bytes()
    }
}

/// Verify a detached signature.
pub fn verify(msg: &[u8], sig: &[u8; 64], public_key: &[u8; 32]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    vk.verify_strict(msg, &Signature::from_bytes(sig)).is_ok()
}

/// A signed receipt: the canonical receipt bytes plus an ed25519 attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedReceipt {
    pub receipt: Vec<u8>,
    pub signature: [u8; 64],
    pub public_key: [u8; 32],
}

const SK_VERSION: u64 = 0;
const SK_RECEIPT: u64 = 1;
const SK_SIG: u64 = 2;
const SK_PUBKEY: u64 = 3;

impl SignedReceipt {
    /// Sign any receipt's canonical bytes.
    pub fn sign(receipt_bytes: Vec<u8>, key: &SigningKey) -> Self {
        let signature = key.sign(&receipt_bytes);
        SignedReceipt {
            receipt: receipt_bytes,
            signature,
            public_key: key.public_key(),
        }
    }

    /// Does the signature attest these exact receipt bytes?
    pub fn verify(&self) -> bool {
        verify(&self.receipt, &self.signature, &self.public_key)
    }

    /// The trust anchor: SHA-256 of the unsigned receipt bytes.
    pub fn receipt_id(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&self.receipt);
        h.finalize().into()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let entries = vec![
            (SK_VERSION, Value::Uint(0)),
            (SK_RECEIPT, Value::Bytes(self.receipt.clone())),
            (SK_SIG, Value::Bytes(self.signature.to_vec())),
            (SK_PUBKEY, Value::Bytes(self.public_key.to_vec())),
        ];
        cbor::to_bytes(&Value::Map(entries))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CborError> {
        let Value::Map(entries) = cbor::from_bytes(bytes)? else {
            return Err(CborError::UnsupportedMajorType(0));
        };
        let mut receipt = None;
        let mut sig: Option<[u8; 64]> = None;
        let mut pk: Option<[u8; 32]> = None;
        for (k, v) in entries {
            match (k, v) {
                (SK_VERSION, Value::Uint(0)) => {}
                (SK_RECEIPT, Value::Bytes(b)) => receipt = Some(b),
                (SK_SIG, Value::Bytes(b)) => sig = b.try_into().ok(),
                (SK_PUBKEY, Value::Bytes(b)) => pk = b.try_into().ok(),
                _ => return Err(CborError::UnsupportedMajorType(0)),
            }
        }
        Ok(SignedReceipt {
            receipt: receipt.ok_or(CborError::Truncated)?,
            signature: sig.ok_or(CborError::Truncated)?,
            public_key: pk.ok_or(CborError::Truncated)?,
        })
    }
}
