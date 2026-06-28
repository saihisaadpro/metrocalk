//! M11.5 (ADR-044) — the **cryptographic provenance signing backing** behind the [`ProvenanceVerifier`]
//! trait: the real-crypto counterpart of the dependency-free [`crate::provenance::ContentAddressTrust`].
//!
//! It implements the **C2PA trust model** — a signed **assertion set** bound to a **hard content hash** —
//! with a real **Ed25519** signature, giving the two guarantees C2PA exists for: **integrity** (a tampered
//! asset is rejected — the content no longer matches what's signed) and **authorship** (only an asset signed
//! by a **trusted** key passes — a re-signed forgery by an attacker key is rejected). Sealing signs the
//! [`Provenance::canonical_assertions`] (content hash + identity) and stores the signature + signer public
//! key on the record; verifying re-derives the assertions from the stored record + the actual bytes and
//! checks both the binding and the signature.
//!
//! **Why Ed25519, not the `c2pa` crate (ADR-044 crate choice).** `/assets` is **wasm-clean by invariant**
//! (ADR-006) and the official `c2pa` crate is native-only (C/openssl-leaning) and doesn't embed in `.glb`;
//! pulling it would break the wasm fence and ride a heavy native tree. `ed25519-dalek` is **pure Rust** (no
//! C, no openssl), so the *same* cryptographic guarantee is available natively today and could verify in the
//! browser too. The literal **C2PA/JUMBF wire format + the CAI trust list** (interop with external Content
//! Credentials validators) ride **on top of** this signed envelope as a **named seam** — the trust *model*
//! is real here; the wire-format interop is the next layer. Behind the `signing` feature so the default
//! crate stays minimal; `ed25519_dalek::` is confined to this module (invariant 5, CI grep-gated).

use crate::provenance::{Provenance, ProvenanceVerifier, TamperError};
use crate::store::AssetId;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

const BACKEND: &str = "signed-manifest";

/// A cryptographic-provenance backing (ADR-044). A **signing** instance ([`from_secret`](Self::from_secret))
/// can both seal (sign) and verify; a **verify-only** instance ([`verifier`](Self::verifier)) only checks
/// signatures against a trusted-key set. The trusted set is provisioned by the caller (a project /
/// marketplace identity) — the CAI trust-list integration is the named interop seam.
pub struct SignedProvenanceTrust {
    /// Present on a signing instance; absent on a verify-only one.
    signing_key: Option<SigningKey>,
    /// The public keys `verify` accepts a signature from (the forgery guard).
    trusted: Vec<VerifyingKey>,
}

impl SignedProvenanceTrust {
    /// A **signing** backing from a 32-byte secret seed; it trusts its own public key (so it verifies what
    /// it signs). Real key provisioning (where the project/marketplace private key lives) is the named seam.
    #[must_use]
    pub fn from_secret(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let trusted = vec![signing_key.verifying_key()];
        Self {
            signing_key: Some(signing_key),
            trusted,
        }
    }

    /// A **verify-only** backing trusting the given public keys (hex, 32-byte Ed25519). Unparseable keys
    /// are skipped. It cannot seal.
    #[must_use]
    pub fn verifier(trusted_pubkeys_hex: &[String]) -> Self {
        let trusted = trusted_pubkeys_hex
            .iter()
            .filter_map(|h| verifying_key_from_hex(h))
            .collect();
        Self {
            signing_key: None,
            trusted,
        }
    }

    /// The hex public key this backing signs with — distribute it as a trusted key to verifiers. `None` for
    /// a verify-only backing.
    #[must_use]
    pub fn public_key_hex(&self) -> Option<String> {
        self.signing_key
            .as_ref()
            .map(|k| to_hex(&k.verifying_key().to_bytes()))
    }
}

impl ProvenanceVerifier for SignedProvenanceTrust {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    /// Stamp the content address and (on a signing instance) sign the canonical assertions, recording the
    /// signature + signer on the record. A verify-only instance only stamps the content hash.
    fn seal(&self, bytes: &[u8], mut record: Provenance) -> Provenance {
        record.content_hash = AssetId::of_bytes(bytes).as_str().to_string();
        if let Some(sk) = &self.signing_key {
            let sig: Signature = sk.sign(&record.canonical_assertions());
            record.signature = Some(to_hex(&sig.to_bytes()));
            record.signer = Some(to_hex(&sk.verifying_key().to_bytes()));
        }
        record
    }

    /// Reject the asset unless: (1) the bytes still hash to the recorded content address (the **hard
    /// binding** — integrity), (2) the record carries a signature by a **trusted** signer (authorship —
    /// defeats a re-sign-with-attacker-key forgery), and (3) that signature verifies over the canonical
    /// assertions (so tampering any identity field is caught).
    fn verify(&self, bytes: &[u8], sealed: &Provenance) -> Result<(), TamperError> {
        let actual = AssetId::of_bytes(bytes).as_str().to_string();
        if actual != sealed.content_hash {
            return Err(TamperError {
                backend: BACKEND,
                expected: sealed.content_hash.clone(),
                actual,
            });
        }
        let (Some(sig_hex), Some(signer_hex)) = (&sealed.signature, &sealed.signer) else {
            return Err(TamperError {
                backend: BACKEND,
                expected: "a signed manifest".into(),
                actual: "unsigned".into(),
            });
        };
        let Some(vk) = verifying_key_from_hex(signer_hex) else {
            return Err(TamperError {
                backend: BACKEND,
                expected: "a valid signer key".into(),
                actual: "unparseable signer".into(),
            });
        };
        if !self.trusted.iter().any(|t| t.to_bytes() == vk.to_bytes()) {
            return Err(TamperError {
                backend: BACKEND,
                expected: "a trusted signer".into(),
                actual: format!("untrusted signer {signer_hex}"),
            });
        }
        let Some(sig) = signature_from_hex(sig_hex) else {
            return Err(TamperError {
                backend: BACKEND,
                expected: "a valid signature".into(),
                actual: "unparseable signature".into(),
            });
        };
        vk.verify(&sealed.canonical_assertions(), &sig)
            .map_err(|_| TamperError {
                backend: BACKEND,
                expected: "a valid signature over the provenance".into(),
                actual: "signature does not verify (tampered / forged)".into(),
            })
    }
}

// ── hex helpers (no extra dependency — keys + signatures are small fixed-width byte strings) ─────────────

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}"); // writing to a String is infallible
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn verifying_key_from_hex(h: &str) -> Option<VerifyingKey> {
    let arr: [u8; 32] = from_hex(h)?.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

fn signature_from_hex(h: &str) -> Option<Signature> {
    let arr: [u8; 64] = from_hex(h)?.try_into().ok()?;
    Some(Signature::from_bytes(&arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::AssetKind;

    fn signing() -> SignedProvenanceTrust {
        SignedProvenanceTrust::from_secret(&[7u8; 32])
    }

    #[test]
    fn seal_then_verify_accepts_a_faithfully_signed_asset() {
        let t = signing();
        let bytes = b"a faithful asset payload";
        let sealed = t.seal(bytes, Provenance::imported("hero.glb", "", 0));
        assert!(
            sealed.signature.is_some() && sealed.signer.is_some(),
            "seal signs the record"
        );
        assert_eq!(sealed.kind, Some(AssetKind::Imported));
        assert_eq!(t.backend(), "signed-manifest");
        t.verify(bytes, &sealed)
            .expect("a faithfully-signed, untouched asset verifies");
    }

    #[test]
    fn a_tampered_byte_breaks_the_content_binding() {
        let t = signing();
        let mut bytes = b"the original, signed asset bytes".to_vec();
        let sealed = t.seal(&bytes, Provenance::imported("hero.glb", "", 0));
        bytes[5] ^= 0x01;
        let err = t
            .verify(&bytes, &sealed)
            .expect_err("a tampered asset is rejected");
        assert_eq!(err.expected, sealed.content_hash);
        assert_ne!(err.actual, err.expected, "the bytes hash differently now");
    }

    #[test]
    fn tampering_a_provenance_field_breaks_the_signature() {
        let t = signing();
        let bytes = b"asset bytes";
        let mut sealed = t.seal(bytes, Provenance::imported("hero.glb", "", 0));
        // Forge the AI-generated flag AFTER signing: the content hash is unchanged, but the assertion set
        // no longer matches the signature → rejected (the metadata is signed, not just the bytes).
        sealed.ai_generated = true;
        assert!(
            t.verify(bytes, &sealed).is_err(),
            "a tampered provenance field is caught by the signature"
        );
    }

    #[test]
    fn an_untrusted_signer_is_rejected_but_the_trusted_one_verifies() {
        let signer = SignedProvenanceTrust::from_secret(&[1u8; 32]);
        let bytes = b"a validly signed asset";
        let sealed = signer.seal(bytes, Provenance::imported("hero.glb", "", 0));

        // A verifier trusting a DIFFERENT key rejects it — even though the signature itself is valid. This
        // is the forgery guard: an attacker re-signing a tampered asset with their own key doesn't pass.
        let other = SignedProvenanceTrust::from_secret(&[2u8; 32]);
        let wrong = SignedProvenanceTrust::verifier(&[other.public_key_hex().unwrap()]);
        assert!(
            wrong.verify(bytes, &sealed).is_err(),
            "an untrusted signer is rejected"
        );

        // The key-distribution round-trip: a verifier trusting the signer's public key accepts it.
        let right = SignedProvenanceTrust::verifier(&[signer.public_key_hex().unwrap()]);
        right
            .verify(bytes, &sealed)
            .expect("a verifier trusting the signer's key accepts the asset");
    }

    #[test]
    fn an_unsigned_record_is_rejected() {
        let t = signing();
        let bytes = b"asset";
        // A content-address-only record (no signature) is not authorship-verified — the signed backing
        // requires a signature (use `ContentAddressTrust` for an integrity-only check).
        let mut unsigned = Provenance::imported("hero.glb", "", 0);
        unsigned.content_hash = AssetId::of_bytes(bytes).as_str().to_string();
        assert!(
            t.verify(bytes, &unsigned).is_err(),
            "an unsigned asset does not pass the signed backing"
        );
    }

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 15, 16, 200, 255];
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert!(from_hex("xyz").is_none(), "non-hex is rejected");
        assert!(from_hex("abc").is_none(), "odd length is rejected");
    }
}
