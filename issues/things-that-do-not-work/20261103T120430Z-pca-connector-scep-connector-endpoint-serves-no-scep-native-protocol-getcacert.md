Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: ce1a48ec1d7dd5bd67ab7b503fef081cc276f2f9

# Connector Endpoint serves no SCEP native protocol: GetCACert at the connector's HTTPS SCEP URL never answers 200 with the configured CA certificate

> _Note: this issue was identified against the code base at commit `ce1a48ec1d7dd5bd67ab7b503fef081cc276f2f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCACert

## Description
1 net-new root cause.

Cause 1 of 1: an ACTIVE connector reports an `Endpoint` (its "HTTPS public SCEP URL"), but nothing in the emulator serves the SCEP protocol at that URL, so a SCEP `GetCACert` request against it never returns 200 and never returns the CA certificate the connector was created with. Failing check id: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip.

AWS: Smithy: Connector.Endpoint (target String) — "The connector's HTTPS public SCEP URL"; Connector doc — "Connector for SCEP is a service that links AWS Private Certificate Authority to your SCEP-enabled devices. The connector brokers the exchange of certificates from AWS Private CA to your SCEP-enabled devices...". The SCEP operations themselves are NOT modeled operations of pca-connector-scep — they are the native RFC 8894 protocol served over that Endpoint URL, so this is recorded/protocol behavior only: a GET to `<Endpoint>?operation=GetCACert` on an ACTIVE connector answers HTTP 200 with a body that is either a single DER certificate (`application/x-x509-ca-cert`) or a DER PKCS#7 degenerate certificates-only structure (`application/x-x509-ca-ra-cert`), and that body contains the certificate of the private CA named by the connector's `CertificateAuthorityArn` — same subject and same serial number as the certificate ACM PCA holds for that CA. The recording's expectation is only "status 200 and the configured CA cert is present"; the concrete host name, the connector id in it, and the certificate's own subject/serial are per-run values and are NOT part of the requirement.

Emulator: the connector is created and settles to ACTIVE, and `GetConnector` reports an `Endpoint` derived purely as a host-name string (services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs, `connector_endpoint`, around lines 1168-1191: `https://{connector_id}.scep.pca-connector-scep.{region}.amazonaws.com`, or the `vpce.amazonaws.com` form). No request path, handler or routing seam exists behind that URL: the crate exposes only the modeled control-plane operations (src/ops/mod.rs is the generated per-operation table) and src/service.rs leaves `override_operation` returning `None` and `override_routing_error` returning `None`, so a GET to the connector endpoint with `?operation=GetCACert` names no modeled operation and can only fall through to the framework's routing failure (non-2xx) rather than a SCEP response. The replayed check polls that URL for a 200 for up to 90s and the check is killed by its budget: "Check timed out after 60.0s". Hypothesis: the time is spent entirely in that never-satisfiable poll, because it is the only loop in the check whose predicate (`status == 200` from the connector Endpoint) can never become true without a SCEP data plane; every other wait in the check (CA activation, RAM share ACTIVE, connector ACTIVE) is satisfied by operations the emulator already implements.

Why: the connector's whole purpose is the SCEP data plane. A client that follows the documented flow — create a private CA, share it, CreateConnector, read `Connector.Endpoint`, then point a SCEP-enabled device or MDM at that URL — gets an un-enrollable connector: GetCACert (and therefore GetCACaps and PKIOperation) never answer, so no device can fetch the CA certificate or enroll. The control plane reports a healthy ACTIVE connector with a plausible URL, so the failure is silent and only shows up as a device-side enrollment timeout. Tests and tooling that exercise enrollment end-to-end hang until their own deadline instead of failing fast.

Fix: this needs a SCEP data-plane surface for the connector endpoint, not a control-plane tweak. It belongs in the service's request-claiming/routing seam (src/service.rs `can_handle` + `override_operation`, which must recognise a request whose host is a connector's own `Endpoint` host rather than the service's regional API host) plus a new native-protocol handler module beside src/ops that implements at least `operation=GetCACert` by looking the connector up by the id in the endpoint host (src/state.rs `connector_endpoint` is where that host shape is defined, so the lookup belongs next to it), requiring the connector to be ACTIVE, and serving the certificate currently held by the ACM PCA CA named by that connector's `CertificateAuthorityArn` (DER, or a DER PKCS#7 certificates-only blob with the matching `Content-Type`). A connector that is not ACTIVE, or an endpoint host naming no connector, must answer a non-2xx rather than a certificate.

Traces: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip (mismatch, "Check timed out after 60.0s").

## Evidence
Check: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip (GROUP native-protocol, tags axis:happy-path, cap:c1, cap:c4, concern:data-plane, dp:endpoint, plane:data).

Expected (AWS): after CreateConnector on an ACTIVE shared private CA, `GetConnector(...)["Connector"]["Endpoint"]` is an HTTPS SCEP URL; `GET <Endpoint>?operation=GetCACert` answers HTTP 200 with a non-empty body, and parsing that body (DER x509 when the Content-Type is a plain CA cert, DER PKCS#7 when the Content-Type contains `ca-ra-cert`/`pkcs7`) yields a certificate whose subject and serial number equal those of the CA certificate CreateConnector's `CertificateAuthorityArn` refers to.

Actual (emulator): the connector does reach ACTIVE and does report an Endpoint, but the poll `_poll_until(_fetch, lambda r: r is not None and r[0] == 200, timeout_s=90, interval_s=5)` against that Endpoint never observes a 200; the check is terminated by its own budget with:

    Error: Check timed out after 60.0s

Corroborating source state (why no 200 is reachable):
- services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs lines ~1168-1191 (`connector_endpoint`): the Endpoint is only a formatted host string; nothing registers a path or handler for it.
- services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/service.rs lines 76-96: `can_handle` claims a request only when the resolved route key equals the service's signing name, and `override_operation` returns `None` unconditionally — there is no seam that maps a connector-endpoint host + `operation=GetCACert` query onto any handler.
- services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/mod.rs: handlers are exactly the generated modeled control-plane operations; `GetCACert` is not among them and cannot be, since SCEP is not in the Smithy model.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region us-west-2 in the recording; any region reproduces).
2. acm-pca: CreateCertificateAuthority (ROOT, RSA_2048/SHA256WITHRSA), wait for PENDING_CERTIFICATE, GetCertificateAuthorityCsr, IssueCertificate with template arn:aws:acm-pca:::template/RootCACertificate/V1, GetCertificate, ImportCertificateAuthorityCertificate, wait for ACTIVE. Keep the returned CA certificate PEM.
3. ram: CreateResourceShare for that CA ARN with principal pca-connector-scep.amazonaws.com and the AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority permission; wait for status ACTIVE.
4. pca-connector-scep: CreateConnector(CertificateAuthorityArn=<ca arn>); poll GetConnector until Status is ACTIVE and read Connector.Endpoint.
5. Issue an unsigned HTTP GET to `<Endpoint>?operation=GetCACert` (routed to the emulator).
6. Observe: no 200 is ever returned (the request names no modeled operation, so it falls through to the framework's routing failure), so a loop waiting for 200 spins until the caller's deadline — in the suite the check dies with "Check timed out after 60.0s". Expected: HTTP 200 whose DER/PKCS#7 body contains the certificate of the CA from step 2 (same subject and serial number).
