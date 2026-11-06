// Self-signed certificates, public-key pins and rustls configuration.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rcgen::PublicKeyData as _;
use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// A certificate, its private key and the key pin that identifies it.
#[derive(Clone)]
pub struct TlsMaterial {
	/// Certificate in DER form.
	pub cert_der: Vec<u8>,
	/// PKCS#8 private key in DER form.
	pub key_der: Vec<u8>,
	/// Base64 SHA-256 of the public key (SubjectPublicKeyInfo).
	pub pin: String,
}

impl std::fmt::Debug for TlsMaterial {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("TlsMaterial")
			.field("pin", &self.pin)
			.finish_non_exhaustive()
	}
}

fn provider() -> Arc<CryptoProvider> {
	Arc::new(ring::default_provider())
}

/// Installs the `ring` provider as the process default so other rustls users (for
/// example reqwest's platform verifier) find one. Safe to call repeatedly.
pub fn ensure_default_provider() {
	let _ = ring::default_provider().install_default();
}

/// Hashes a SubjectPublicKeyInfo into the pin format used by curl: base64 of SHA-256.
pub fn pin_of_spki(spki_der: &[u8]) -> String {
	STANDARD.encode(Sha256::digest(spki_der))
}

fn generate(hosts: &[String]) -> Result<(String, String, Vec<u8>)> {
	let mut names: Vec<String> = hosts.to_vec();
	for h in ["localhost", "127.0.0.1"] {
		if !names.iter().any(|n| n == h) {
			names.push(h.to_owned());
		}
	}
	let key = rcgen::KeyPair::generate().map_err(|e| Error::Tls(e.to_string()))?;
	let mut params = rcgen::CertificateParams::new(names).map_err(|e| Error::Tls(e.to_string()))?;
	params
		.distinguished_name
		.push(rcgen::DnType::CommonName, "Mailslot");
	let cert = params
		.self_signed(&key)
		.map_err(|e| Error::Tls(e.to_string()))?;
	Ok((
		cert.pem(),
		key.serialize_pem(),
		key.subject_public_key_info(),
	))
}

fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
	let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
	STANDARD
		.decode(body.trim())
		.map_err(|e| Error::Tls(e.to_string()))
}

/// Loads the certificate stored in `dir`, or creates a new self-signed one valid for
/// `hosts`, `localhost` and `127.0.0.1`.
pub async fn load_or_generate(dir: &Path, hosts: &[String]) -> Result<TlsMaterial> {
	let cert_path = dir.join("cert.pem");
	let key_path = dir.join("key.pem");
	if let (Ok(cert_pem), Ok(key_pem)) = (
		tokio::fs::read_to_string(&cert_path).await,
		tokio::fs::read_to_string(&key_path).await,
	) && let Ok(key) = rcgen::KeyPair::from_pem(&key_pem)
		&& let Ok(cert_der) = pem_to_der(&cert_pem)
	{
		return Ok(TlsMaterial {
			cert_der,
			key_der: key.serialize_der(),
			pin: pin_of_spki(&key.subject_public_key_info()),
		});
	}
	let (cert_pem, key_pem, spki) = generate(hosts)?;
	tokio::fs::create_dir_all(dir).await?;
	tokio::fs::write(&cert_path, &cert_pem).await?;
	{
		use tokio::io::AsyncWriteExt;
		let mut opts = tokio::fs::OpenOptions::new();
		opts.write(true).create(true).truncate(true);
		#[cfg(unix)]
		opts.mode(0o600);
		let mut f = opts.open(&key_path).await?;
		f.write_all(key_pem.as_bytes()).await?;
		f.flush().await?;
	}
	let key = rcgen::KeyPair::from_pem(&key_pem).map_err(|e| Error::Tls(e.to_string()))?;
	Ok(TlsMaterial {
		cert_der: pem_to_der(&cert_pem)?,
		key_der: key.serialize_der(),
		pin: pin_of_spki(&spki),
	})
}

/// Builds the server-side rustls configuration (HTTP/2 and HTTP/1.1 via ALPN).
pub fn server_config(material: &TlsMaterial) -> Result<Arc<rustls::ServerConfig>> {
	let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(material.key_der.clone()));
	let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
		.with_safe_default_protocol_versions()
		.map_err(|e| Error::Tls(e.to_string()))?
		.with_no_client_auth()
		.with_single_cert(vec![CertificateDer::from(material.cert_der.clone())], key)
		.map_err(|e| Error::Tls(e.to_string()))?;
	cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
	Ok(Arc::new(cfg))
}

/// Reads one DER element, returning (tag, content, rest).
fn der_element(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
	let tag = *input.first()?;
	let first = *input.get(1)?;
	let (len, header) = if first & 0x80 == 0 {
		(usize::from(first), 2)
	} else {
		let n = usize::from(first & 0x7f);
		if n == 0 || n > 4 {
			return None;
		}
		let mut len = 0usize;
		for b in input.get(2..2 + n)? {
			len = (len << 8) | usize::from(*b);
		}
		(len, 2 + n)
	};
	let content = input.get(header..header + len)?;
	Some((tag, content, &input[header + len..]))
}

/// Extracts the full SubjectPublicKeyInfo element (header included) from a certificate.
pub fn spki_of_cert(cert_der: &[u8]) -> Option<Vec<u8>> {
	let (_, cert, _) = der_element(cert_der)?;
	let (_, tbs, _) = der_element(cert)?;
	let mut rest = tbs;
	// Optional [0] version.
	if rest.first() == Some(&0xa0) {
		rest = der_element(rest)?.2;
	}
	// serial, signature, issuer, validity, subject
	for _ in 0..5 {
		rest = der_element(rest)?.2;
	}
	let (tag, content, _) = der_element(rest)?;
	if tag != 0x30 {
		return None;
	}
	let header = rest.len() - der_element(rest)?.2.len() - content.len();
	Some(rest[..header + content.len()].to_vec())
}

#[derive(Debug)]
struct PinVerifier {
	pin: Option<String>,
	insecure: bool,
	provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
	fn verify_server_cert(
		&self,
		end_entity: &CertificateDer<'_>,
		_intermediates: &[CertificateDer<'_>],
		_server_name: &ServerName<'_>,
		_ocsp: &[u8],
		_now: UnixTime,
	) -> std::result::Result<ServerCertVerified, rustls::Error> {
		if self.insecure {
			return Ok(ServerCertVerified::assertion());
		}
		let Some(pin) = &self.pin else {
			return Err(rustls::Error::General(
				"this link has no key pin; pass a pin or allow insecure".into(),
			));
		};
		let spki = spki_of_cert(end_entity.as_ref())
			.ok_or_else(|| rustls::Error::General("unreadable certificate".into()))?;
		if &pin_of_spki(&spki) == pin {
			Ok(ServerCertVerified::assertion())
		} else {
			Err(rustls::Error::General(
				"server key does not match the pinned key".into(),
			))
		}
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls12_signature(
			message,
			cert,
			dss,
			&self.provider.signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls13_signature(
			message,
			cert,
			dss,
			&self.provider.signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		self.provider
			.signature_verification_algorithms
			.supported_schemes()
	}
}

/// Builds a client configuration that trusts exactly one public key.
///
/// With `insecure` set, any certificate is accepted (the handshake signature is still
/// checked); use it only for explicit opt-in.
pub fn client_config(pin: Option<String>, insecure: bool) -> rustls::ClientConfig {
	let provider = provider();
	let verifier = Arc::new(PinVerifier {
		pin,
		insecure,
		provider: provider.clone(),
	});
	rustls::ClientConfig::builder_with_provider(provider)
		.with_safe_default_protocol_versions()
		.expect("ring supports the default protocol versions")
		.dangerous()
		.with_custom_certificate_verifier(verifier)
		.with_no_client_auth()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn material_roundtrips_and_pin_matches_certificate_key() {
		let dir = tempfile::tempdir().unwrap();
		let a = load_or_generate(dir.path(), &["192.168.1.2".into()])
			.await
			.unwrap();
		let b = load_or_generate(dir.path(), &[]).await.unwrap();
		assert_eq!(a.pin, b.pin);
		let spki = spki_of_cert(&a.cert_der).expect("spki");
		assert_eq!(pin_of_spki(&spki), a.pin);
		server_config(&a).unwrap();
	}
}
