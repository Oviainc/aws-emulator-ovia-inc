Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-ca-not-ready-operation-guards
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-ca-not-ready-operation-guards
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|GetCertificate|4xx->4xx|$.__type
Issue-Key: acm-pca|GetCertificate|4xx->4xx|$.message
Issue-Key: acm-pca|DescribeCertificateAuthorityAuditReport|4xx->4xx|$.message
Issue-Commit: 08e684d4fb970283a5b7f28a5e158bea42eb7c99

# ACM PCA not-found refusals diverge: GetCertificate classifies a missing certificate as InvalidStateException, and the audit-report ResourceNotFoundException sentence is miscased

> _Note: this issue was identified against the code base at commit `08e684d4fb970283a5b7f28a5e158bea42eb7c99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCertificate

## Description
TWO net-new root causes on acm-pca, both in the way the service reports a resource it cannot find. Neither failing step is marked with a stub_path and neither is OWNED by an open issue in this replay (the three other acm-pca divergences in the same replay ARE owned by 20261029T190010Z-acm-pca-resource-policy-wire-contract-diverges-a.md and are not filed here). The open issue 20261108T062219Z-acm-pca-representation-and-refusal-gaps-audit-report.md describes the audit-report casing defect as its cause 2, but it is grounded in the BEHAVIORAL corpus (simparity-behavioral-aws-v1) and carries none of this wire fixture's replay keys, so cause 2 below is filed to bind that defect to the wire step that reproduces it.

Cause 1 of 2: GetCertificate on a CA that does not yet hold its own CA certificate answers InvalidStateException for a certificate ARN it has no record of, where AWS answers ResourceNotFoundException.
AWS: Smithy models GetCertificate {CertificateAuthorityArn @required (Arn, @length 5..200, pattern ^arn:[\w+=/,.@-]+:acm-pca:...$), CertificateArn @required (same Arn constraints)} -> {Certificate, CertificateChain}, with errors InvalidArnException, InvalidStateException, RequestFailedException, RequestInProgressException and ResourceNotFoundException, all client errors with httpError 400. ResourceNotFoundException's doc is 'A resource such as a private CA, S3 bucket, certificate, audit report, or policy cannot be found.'; InvalidStateException's is 'The state of the private CA does not allow this action to occur.' Which one applies to a certificate ARN the CA has no record of is recorded behavior: the recording shows HTTP 400 with Error.Code 'ResourceNotFoundException' and Error.Message 'Could not find specified certificate.' — the whole sentence is fixed text, with no part taken from the request or from state — even though the named CA is in PENDING_CERTIFICATE and holds no certificate of its own. In other words AWS resolves the CERTIFICATE first and reports its absence; the CA's own readiness is not what the refusal is about.
Emulator: the same HTTP 400, but Error.Code 'InvalidStateException' and Error.Message 'The certificate authority <CertificateAuthorityArn> is not in the correct state to have issued certificates.' — fixed text around the request's CA ARN as its only variable part. This is deliberate in the handler: when the certificate is not in the CA's map it checks whether the CA has its own certificate installed and, if not, converts the lookup miss into a state refusal.
Why: the error CODE is the branch a client dispatches on. A caller polling for a certificate it just requested, or probing whether a certificate exists, catches ResourceNotFoundException; it instead receives InvalidStateException, which its handler does not expect, so the call surfaces as an unhandled error or as a (false) conclusion that the CA itself is unusable. The recorded guard sequence that exercises exactly this — a not-yet-activated CA being asked for an unknown certificate — therefore cannot be reproduced against the emulator, and the message a client logs or matches on is a sentence AWS never emits for this case.
Fix: the lookup/refusal precedence in GetCertificate's handler — report the missing certificate as ResourceNotFoundException with AWS's fixed sentence and stop deriving an InvalidStateException from the CA's missing own-certificate on a lookup miss; keep InvalidStateException for the states AWS actually refuses on.
Traces: acm-pca-ca-not-ready-operation-guards#task#8; services/service-com-amazonaws-acmpca-acmprivateca/src/ops/get_certificate.rs lines 208-222 (the `record.ca_certificate_pem.is_none()` branch) and the message constants in src/state.rs.

Cause 2 of 2: DescribeCertificateAuthorityAuditReport's ResourceNotFoundException message is miscased relative to AWS's fixed sentence.
AWS: Smithy models DescribeCertificateAuthorityAuditReport {CertificateAuthorityArn @required, AuditReportId @required (@length 36..36, pattern ^[a-z0-9]{8}-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{12}$)} -> {AuditReportStatus, S3BucketName, S3Key, CreatedAt}, with errors InvalidArgsException, InvalidArnException and ResourceNotFoundException (client, httpError 400). The text is recorded behavior: a well-formed AuditReportId naming no report on that CA is refused with HTTP 400, Error.Code 'ResourceNotFoundException' and Error.Message 'Could not find specified Audit Report.' — the entire sentence is fixed text, including the capitalised 'Audit Report'; nothing in it comes from the request or from state.
Emulator: the correct status (400) and the correct error code, but the message 'Could not find specified audit report.' — the same sentence with 'Audit Report' lower-cased.
Why: the error message is part of the contract. Any client, test or log matcher that compares the refusal text exactly sees a string production never returns, so an emulator recording is not comparable with a production one and a text-matching guard around audit-report lookups fails against the emulator while passing against AWS.
Fix: the ResourceNotFoundException message construction on DescribeCertificateAuthorityAuditReport's lookup path (the shared message constant it uses) — emit AWS's fixed sentence verbatim, with 'Audit Report' capitalised.
Traces: acm-pca-ca-not-ready-operation-guards#task#11; services/service-com-amazonaws-acmpca-acmprivateca/src/ops/describe_certificate_authority_audit_report.rs and the not-found message constant in src/state.rs.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator: 10 fixtures replayed, 87 steps, 82 passed, 5 mismatched (score 94.3%), emulator_up=true and bootstrap_signal=false — acm-pca is serving, so these are fidelity divergences and not a bootstrap gap. Three of the five mismatches are owned by an existing open issue; the two below are not.

Cause 1 — fixture acm-pca-ca-not-ready-operation-guards, step task#8, operation GetCertificate, verdict mismatch, expected status 400 / actual status 400:
  $.__type: value_mismatch (expected 'ResourceNotFoundException', got 'InvalidStateException')
  $.message: value_mismatch (expected 'Could not find specified certificate.', got 'The certificate authority arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/b6633b30-c914-4856-9560-0f0098374a0a is not in the correct state to have issued certificates.')
  Expected: Error.Code 'ResourceNotFoundException' with the fixed sentence 'Could not find specified certificate.' (no variable parts). Actual: Error.Code 'InvalidStateException' with a sentence whose only variable part is the request's CertificateAuthorityArn (the ARN above is a per-run value and is not itself a requirement).

Cause 2 — fixture acm-pca-ca-not-ready-operation-guards, step task#11, operation DescribeCertificateAuthorityAuditReport, verdict mismatch, expected status 400 / actual status 400:
  $.message: value_mismatch (expected 'Could not find specified Audit Report.', got 'Could not find specified audit report.')
  Status and Error.Code ('ResourceNotFoundException') match; the entire expected sentence is fixed text and differs only in the casing of 'Audit Report'.

## Steps to Reproduce
Common setup: acmpca.create_certificate_authority(CertificateAuthorityConfiguration={'KeyAlgorithm':'RSA_2048','SigningAlgorithm':'SHA256WITHRSA','Subject':{'CommonName': <name>}}, CertificateAuthorityType='ROOT') and poll DescribeCertificateAuthority until Status='PENDING_CERTIFICATE' (do NOT activate the CA).

Cause 1: against that PENDING_CERTIFICATE CA call acmpca.get_certificate(CertificateAuthorityArn=<CA ARN>, CertificateArn='<CA ARN>/certificate/<any 32-hex serial that was never issued>'). Expected: HTTP 400 with Error.Code 'ResourceNotFoundException' and Error.Message exactly 'Could not find specified certificate.' Actual: HTTP 400 with Error.Code 'InvalidStateException' and the message 'The certificate authority <CA ARN> is not in the correct state to have issued certificates.'

Cause 2: on any CA call acmpca.describe_certificate_authority_audit_report(CertificateAuthorityArn=<CA ARN>, AuditReportId='00000000-0000-0000-0000-000000000000') — a syntactically valid id naming no report. Expected: HTTP 400, Error.Code 'ResourceNotFoundException', Error.Message exactly 'Could not find specified Audit Report.' Actual: the same status and code with 'Could not find specified audit report.'

Replay shortcut: re-replay fixture acm-pca-ca-not-ready-operation-guards steps task#8 and task#11 from trace set simparity-wire-aws-v1.
