//! Origin-scoped certificate pins, shared by HTTP and WebSocket. A pin is an
//! explicit identity exception (including name and validity), never a TLS bypass:
//! the peer must still prove possession of the certificate's private key.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use url::Url;

/// One approved hub origin and its exact leaf certificate's SHA-256 fingerprint.
#[derive(Debug, Clone)]
pub struct TlsTrust {
    origin: Url,
    fingerprint: Option<String>,
    observed: Arc<Mutex<Option<String>>>,
    rejected: Arc<Mutex<bool>>,
}

impl TlsTrust {
    pub fn new(origin: &str, fingerprint: Option<String>) -> Result<Self, String> {
        Ok(Self {
            origin: Url::parse(origin).map_err(|e| e.to_string())?,
            fingerprint,
            observed: Arc::default(),
            rejected: Arc::default(),
        })
    }

    pub fn observed_fingerprint(&self) -> Option<String> {
        self.observed
            .lock()
            .expect("certificate observation lock")
            .clone()
    }

    /// A typed trust failure's diagnostic, with the newly observed fingerprint.
    pub fn rejection(&self) -> Option<String> {
        (*self.rejected.lock().expect("certificate rejection lock")).then(|| {
            format!(
                "{} certificate is not trusted; SHA-256: {}. Verify the fingerprint and run gecko attach again to approve it",
                self.origin.origin().ascii_serialization(),
                self.observed_fingerprint().unwrap_or_default()
            )
        })
    }

    fn matches(&self, target: &Url) -> bool {
        matches!(target.scheme(), "https" | "wss")
            && self.origin.scheme() == "https"
            && self.origin.host() == target.host()
            && self.origin.port_or_known_default() == target.port_or_known_default()
    }

    pub(crate) fn config(&self, target: &Url) -> Result<rustls::ClientConfig, String> {
        super::net::install_crypto_provider();
        let system = rustls_platform_verifier::Verifier::new(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .map_err(|e| e.to_string())?;
        let verifier: Arc<dyn ServerCertVerifier> = if self.matches(target) {
            Arc::new(PinnedVerifier {
                system: Arc::new(system),
                trust: self.clone(),
            })
        } else {
            Arc::new(system)
        };
        Ok(rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth())
    }
}

pub(crate) fn fingerprint(cert: &CertificateDer<'_>) -> String {
    ring::digest::digest(&ring::digest::SHA256, cert.as_ref())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[derive(Debug)]
struct PinnedVerifier {
    system: Arc<dyn ServerCertVerifier>,
    trust: TlsTrust,
}

#[cfg(test)]
mod tests;

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let observed = fingerprint(end_entity);
        *self
            .trust
            .observed
            .lock()
            .expect("certificate observation lock") = Some(observed.clone());
        let system = self.system.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        if system.is_ok() || self.trust.fingerprint.as_deref() == Some(observed.as_str()) {
            *self
                .trust
                .rejected
                .lock()
                .expect("certificate rejection lock") = false;
            return Ok(ServerCertVerified::assertion());
        }
        *self
            .trust
            .rejected
            .lock()
            .expect("certificate rejection lock") = true;
        system
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.system.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.system.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.system.supported_verify_schemes()
    }
}
