# Hub certificate approval

`gecko attach` displays the hub origin and the leaf certificate's SHA-256
fingerprint before asking for approval. Verify the fingerprint with the hub
administrator through an independent channel. Certificate discovery exchanges
only TLS handshake messages: it sends neither an HTTP request nor an attach code
or bearer token. It has a ten-second deadline. `--trust-hub` is explicit approval
of the displayed certificate for that invocation; it does not disable verification.

`config.json` keeps approved HTTPS origins as objects in `trustedHubs`:

```json
{
  "trustedHubs": [
    {
      "hub": "https://hub.example:8443",
      "fingerprint": "<uppercase colon-separated SHA-256 of the leaf certificate DER>"
    }
  ]
}
```

Legacy string entries remain readable. An HTTPS entry without a fingerprint
requires approval again, including before a stored token can be used or refreshed.
Loopback HTTP entries keep their existing behavior. Reapproval replaces only that
origin's record; unrelated entries remain intact.

HTTP token exchange, refresh, SignalR negotiation and WebSocket TLS use the same
`ServerCertVerifier` implementation:

1. If the operating system's verifier accepts the certificate, accept it with its
   normal chain, server-name and validity checks. A stale pin does not override
   successful system validation.
2. Otherwise accept only an exact SHA-256 match for the approved **leaf** certificate
   at the approved HTTPS origin (scheme, host and effective port). Its WSS endpoint
   shares the same approval. Neither intermediates nor another origin inherit it.
3. This explicit pin is the alternative identity proof. In that fallback, name
   mismatch and expiration/not-yet-valid errors are also overridden. This supports
   installations whose self-signed certificate names a gateway instead of its
   address. Renewing even the same key changes the certificate and requires
   reapproval when system validation still fails.
4. TLS 1.2/1.3 handshake signatures and supported signature schemes always delegate
   to the platform verifier. Possession of the private key remains required;
   pinning does not accept arbitrary handshake signatures.

A certificate rejected by both paths stops the call with `untrusted-hub` (exit 3),
shows the newly observed fingerprint and directs the user to run `gecko attach`
again. No application credentials are sent and the stored approval is not changed.
An interactive attach asks again; noninteractive attach requires explicit
`--trust-hub`. There is no verification-off option and no CA-file setting.

Pins are scoped before configuring either transport. HTTP redirects are not
followed, so a token request cannot silently move its code or approval to another
origin. HTTPS origins always use SignalR over WSS, including local installations;
the development transport is plain WS and remains for loopback HTTP hubs.

API references for the locked rustls 0.23 line:
[ServerCertVerifier](https://docs.rs/rustls/latest/rustls/client/danger/trait.ServerCertVerifier.html)
separates certificate identity verification from handshake signature verification;
[platform Verifier](https://docs.rs/rustls-platform-verifier/0.7.0/rustls_platform_verifier/struct.Verifier.html)
provides the system validation and signature implementations.

Tests use local TLS servers and public, disposable DER identities under
`crates/testkit/fixtures/tls/`. The `first` and `rotated` certificates differ;
`expired` exercises the documented validity exception. Tests use extra trust roots
only in memory to cover successful system verification, without changing the host
trust store. No new package or CA-file capability is needed.
