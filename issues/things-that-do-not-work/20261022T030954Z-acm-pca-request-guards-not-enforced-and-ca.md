Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-ca-access-management-guards
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-ca-access-management-guards acm-pca-ca-not-ready-operation-guards acm-pca-ca-tagging-lifecycle
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|CreateCertificateAuthority|4xx->2xx|$.__type
Issue-Key: acm-pca|CreateCertificateAuthority|4xx->2xx|$.message
Issue-Key: acm-pca|CreateCertificateAuthority|4xx->2xx|status
Issue-Key: acm-pca|ImportCertificateAuthorityCertificate|4xx->2xx|$
Issue-Key: acm-pca|ImportCertificateAuthorityCertificate|4xx->2xx|status
Issue-Key: acm-pca|IssueCertificate|4xx->2xx|$.__type
Issue-Key: acm-pca|IssueCertificate|4xx->2xx|$.message
Issue-Key: acm-pca|IssueCertificate|4xx->2xx|status
Issue-Key: acm-pca|DescribeCertificateAuthority|2xx->2xx|$.CertificateAuthority.RevocationConfiguration
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# ACM PCA request guards not enforced and CA response fidelity gap

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCertificateAuthority

## Description
FOUR distinct causes, all in the acm-pca emulator. A fixer must close ALL FOUR before submitting. Three are missing request guards (the emulator answers 200 success where AWS Private CA rejects the request with a modeled 400 client error), and one is a response-shape gap on the CA resource. No open acm-pca issue exists; the only acm-named open issue is the bootstrap issue for the separate 'acm' (AWS Certificate Manager) service, which does not cover these.

=== Cause 1 of 4: CreateCertificateAuthority does not validate SigningAlgorithm against KeyAlgorithm ===
AWS: Private CA rejects a CertificateAuthorityConfiguration whose SigningAlgorithm is incompatible with the configured KeyAlgorithm (e.g. an ECDSA signing algorithm with an RSA key) with HTTP 400 and error code InvalidArgsException. Smithy: CreateCertificateAuthority input member CertificateAuthorityConfiguration (required) carries KeyAlgorithm and SigningAlgorithm ('Name of the algorithm your private CA uses to sign certificate requests'); modeled error InvalidArgsException, errorKind client, httpError 400 ('One or more of the specified arguments was not valid'). The recorded response is 400 with __type InvalidArgsException and a message that names the rejected algorithm, the key type and the valid algorithm list (example: "Signing algorithm SHA256WITHECDSA cannot be used with key type RSA_2048. Valid signing algorithms are: SHA256WITHRSA, ..."); the message is built from the request, so naming the offending algorithm and key type is the requirement, not the recorded wording.
Emulator: returns HTTP 200 with a created CertificateAuthorityArn, and emits no __type/message at all.
Why: a client that relies on AWS rejecting an unusable key/signing-algorithm pairing instead gets a CA that AWS would never have created; the configuration error surfaces much later (or never) instead of at the create call, and negative-path tests that assert InvalidArgsException see a success.
Fix: cross-field input validation for CreateCertificateAuthority in the acm-pca service crate (services/service-com-amazonaws-acmpca-acmprivateca) — the operation's validation layer must reject the pairing before the CA is persisted in state.
Traces: acm-pca-ca-access-management-guards#task#11.

=== Cause 2 of 4: ImportCertificateAuthorityCertificate accepts a malformed certificate ===
AWS: a Certificate blob that is not a well-formed certificate is rejected with HTTP 400 and error code MalformedCertificateException. Smithy: ImportCertificateAuthorityCertificate input Certificate is a required CertificateBodyBlob (@length 1..32768); modeled error MalformedCertificateException, errorKind client, httpError 400 ('One or more fields in the certificate are invalid'). Recorded response: 400 {"__type":"MalformedCertificateException","message":"Malformed certificate."} (the message is an example; the status and code are the requirement).
Emulator: returns HTTP 200 with an empty body (the operation's modeled Unit output), i.e. the import is treated as having succeeded.
Why: the CA appears to clients to have been activated with an unusable certificate, so subsequent state-dependent operations (GetCertificateAuthorityCertificate, IssueCertificate) are evaluated against a CA state AWS would never have reached; a client's error handling for malformed PEM input is never exercised.
Fix: ImportCertificateAuthorityCertificate handler/validation in the acm-pca service crate — parse and validate the Certificate blob and raise MalformedCertificateException before mutating CA state.
Traces: acm-pca-ca-not-ready-operation-guards#task#6.

=== Cause 3 of 4: IssueCertificate ignores the CA state guard ===
AWS: IssueCertificate is only permitted when the CA is in a state that allows issuance; a CA that has not been activated is rejected with HTTP 400 and error code InvalidStateException. Smithy: IssueCertificate modeled error InvalidStateException, errorKind client, httpError 400; shape doc 'The state of the private CA does not allow this action to occur'. Recorded response: 400 with __type InvalidStateException (example message 'The certificate authority is not in a valid state for issuing certificates').
Emulator: returns HTTP 200 with a CertificateArn, with no __type/message — issuance succeeds against a CA that is not activated. Reproduced on two steps of the same fixture.
Why: clients get certificate ARNs for a CA AWS would have refused to issue from, so CA-lifecycle logic (activate-before-issue) is never enforced and retry/wait code paths around InvalidStateException are dead in the emulator.
Fix: CA-state precondition check in the IssueCertificate handler in the acm-pca service crate, read from the CA's persisted status in service state before the certificate is minted.
Traces: acm-pca-ca-not-ready-operation-guards#task#7 and acm-pca-ca-not-ready-operation-guards#task#8 (same cluster).

=== Cause 4 of 4: DescribeCertificateAuthority omits RevocationConfiguration ===
AWS: the CertificateAuthority returned by DescribeCertificateAuthority always carries RevocationConfiguration, with both sub-configurations present and explicitly disabled when the CA was created without revocation settings — CrlConfiguration.Enabled=false and OcspConfiguration.Enabled=false. Smithy: CertificateAuthority member RevocationConfiguration -> RevocationConfiguration{CrlConfiguration, OcspConfiguration}; CrlConfiguration.Enabled and OcspConfiguration.Enabled are both @required booleans.
Emulator: the CertificateAuthority structure is returned (HTTP 200) with $.CertificateAuthority.RevocationConfiguration absent entirely.
Why: a client that reads ca.RevocationConfiguration.CrlConfiguration.Enabled to decide whether CRL/OCSP is on hits a null dereference or an SDK deserialization gap on a member AWS always populates; code that round-trips a described CA into UpdateCertificateAuthority loses the revocation block.
Fix: the DescribeCertificateAuthority serializer / CA state projection in the acm-pca service crate — populate the default disabled RevocationConfiguration (both required Enabled flags false) when the CA has no explicit revocation settings.
Traces: acm-pca-ca-tagging-lifecycle#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting; bootstrap_signal false; state_reset_failures 0):

Cause 1 — acm-pca-ca-access-management-guards#task#11, CreateCertificateAuthority:
  expected status 400, actual 200
  $.__type: missing (expected 'InvalidArgsException')
  $.message: missing (expected a message naming the incompatible algorithm/key type, e.g. 'Signing algorithm SHA256WITHECDSA cannot be used with key type RSA_2048. Valid signing algorithms are: SHA256WITHRSA, SHA384WITHRSA, SHA512WITHRSA, SHA256WITHRSA_PSS, SHA384WITHRSA_PSS, SHA512WITHRSA_PSS.')

Cause 2 — acm-pca-ca-not-ready-operation-guards#task#6, ImportCertificateAuthorityCertificate:
  expected status 400, actual 200
  $: empty_body (expected {'__type': 'MalformedCertificateException', 'message': 'Malformed certificate.'}, got {})

Cause 3 — acm-pca-ca-not-ready-operation-guards#task#7 (cluster also covers #task#8), IssueCertificate:
  expected status 400, actual 200
  $.__type: missing (expected 'InvalidStateException')
  $.message: missing (example 'The certificate authority is not in a valid state for issuing certificates')

Cause 4 — acm-pca-ca-tagging-lifecycle#task#2, DescribeCertificateAuthority:
  status 200 both
  $.CertificateAuthority.RevocationConfiguration: missing (expected {'CrlConfiguration': {'Enabled': False}, 'OcspConfiguration': {'Enabled': False}}, got None)

Model grounding (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json): CreateCertificateAuthority errors include InvalidArgsException (client, 400); ImportCertificateAuthorityCertificate errors include MalformedCertificateException (client, 400); IssueCertificate errors include InvalidStateException (client, 400); CertificateAuthority.RevocationConfiguration -> CrlConfiguration.Enabled / OcspConfiguration.Enabled, both @required.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and watch these four steps:
   a. acm-pca-ca-access-management-guards step task#11 — CreateCertificateAuthority with a CertificateAuthorityConfiguration whose SigningAlgorithm is an ECDSA algorithm while KeyAlgorithm is RSA_2048; expect 400 InvalidArgsException, emulator answers 200 with a CertificateAuthorityArn.
   b. acm-pca-ca-not-ready-operation-guards step task#6 — ImportCertificateAuthorityCertificate with a malformed Certificate blob; expect 400 MalformedCertificateException, emulator answers 200 with an empty body.
   c. acm-pca-ca-not-ready-operation-guards step task#7 (and task#8) — IssueCertificate against a CA that has not been activated; expect 400 InvalidStateException, emulator answers 200 with a CertificateArn.
   d. acm-pca-ca-tagging-lifecycle step task#2 — DescribeCertificateAuthority on a CA created without revocation settings; expect CertificateAuthority.RevocationConfiguration = {CrlConfiguration:{Enabled:false}, OcspConfiguration:{Enabled:false}}, emulator omits the member.
3. After fixing, re-replay all four fixture steps (the representative acm-pca-ca-access-management-guards#task#11 plus the three trace_ids) and confirm each returns the modeled status/error code and the populated RevocationConfiguration.
