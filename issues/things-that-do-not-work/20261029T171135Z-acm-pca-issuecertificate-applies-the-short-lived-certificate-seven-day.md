Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca__iam.template-arn-condition-gates-a-real-signed-issue-certificate
Issue-Key: acm-pca|IssueCertificate|InvalidArgsException
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# IssueCertificate applies the SHORT_LIVED_CERTIFICATE seven-day validity cap to the CA's own root CA certificate, so a short-lived-mode CA can never be activated

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IssueCertificate

## Description
One net-new root cause.

Cause 1 of 1: ACM Private CA's `IssueCertificate` rejects the self-signed root CA certificate that activates a CA created with `UsageMode=SHORT_LIVED_CERTIFICATE`, because the emulator applies the seven-day short-lived validity cap to EVERY issuance, including the request under `arn:aws:acm-pca:::template/RootCACertificate/V1` that the CA signs for itself. Failing check: acm-pca__iam.template-arn-condition-gates-a-real-signed-issue-certificate.

AWS: `UsageMode` on CreateCertificateAuthority is documented (Smithy: CreateCertificateAuthorityRequest$UsageMode — "Specifies whether the CA issues general-purpose certificates ... or short-lived certificates that may optionally omit revocation because they expire quickly. Short-lived certificate validity is limited to seven days. The default value is GENERAL_PURPOSE."). That limit governs the certificates the CA ISSUES to subjects; it is not a limit on the CA certificate itself. The documented root-CA activation procedure (CreateCertificateAuthority -> GetCertificateAuthorityCsr -> IssueCertificate with TemplateArn `template/RootCACertificate/V1` -> ImportCertificateAuthorityCertificate) is the same for a short-lived-mode CA, and AWS's short-lived-CA walkthrough installs that root certificate with a multi-year validity (`Validity Value=10,Type=YEARS`). Smithy: IssueCertificateRequest$Validity documents only one hierarchy constraint — "The end of the validity period configured on a certificate must not exceed the limit set on its parents in the CA hierarchy" — and a root CA has no parent. Recorded behavior: `IssueCertificate(CertificateAuthorityArn=<short-lived ROOT CA>, TemplateArn=arn:aws:acm-pca:::template/RootCACertificate/V1, SigningAlgorithm=SHA256WITHRSA, Csr=<the CA's own CSR>, Validity={Value:3,Type:YEARS})` answers HTTP 200 with a `CertificateArn`, the certificate is then retrievable with GetCertificate and importable, and DescribeCertificateAuthority reports `Status=ACTIVE`.

Emulator: the same call is refused with HTTP 400 `InvalidArgsException` and the message `Validity may not exceed seven days for a certificate authority whose UsageMode is SHORT_LIVED_CERTIFICATE` (that message is entirely fixed text — it carries no values from the request or state). The refusal is unconditional on the template: the handler already distinguishes the CA signing its own root certificate (it waives the `InvalidStateException` for a CA that is still `PENDING_CERTIFICATE` via `signs_its_own_root_certificate`), but the usage-mode window check that follows does not consult the template at all, so the one issuance that is exempt is the one that is blocked.

Why: a client following AWS's documented procedure can never activate a short-lived-mode private CA against this emulator. The CA is stranded in `PENDING_CERTIFICATE`, every later IssueCertificate for a subject correctly fails with `InvalidStateException`, and GetCertificate/audit-report flows have nothing to read. The error is also misclassified: a caller that inspects `InvalidArgsException` concludes its `Validity` argument is wrong and shortens it, which cannot work, since a root CA certificate of <= 7 days is not what the activation step asks for. Any test or deployment that uses short-lived certificate mode (the mode whose whole point is that issued certs expire quickly) is blocked at setup, not at the behavior under test.

Fix: the validity validation in the `IssueCertificate` handler. The short-lived seven-day cap must be scoped to issuances the CA makes for a subject and must NOT be applied when the request is the CA's own CA certificate — i.e. when the named `TemplateArn` is a CA-certificate template (`template/RootCACertificate/*`, and the subordinate CA templates), the same condition the handler already computes to waive the non-ACTIVE state check. The other validity rules in that handler (the window must resolve after the start of the period, at least one second long) are unaffected.

Traces: acm-pca__iam.template-arn-condition-gates-a-real-signed-issue-certificate — fails during setup in `_activate_root_ca`, at the `issue_certificate(TemplateArn=arn:aws:acm-pca:::template/RootCACertificate/V1, Validity={Value:3,Type:YEARS})` call, so the check's actual subject (an identity policy conditioned on `acm-pca:TemplateArn` admitting one template and denying another with `AccessDeniedException`) is never reached. Handler and line citations: services/service-com-amazonaws-acmpca-acmprivateca/src/ops/issue_certificate.rs lines 371-380 (the unconditional short-lived cap) against lines 262-281 (the root-certificate exception the same handler already recognises).

## Evidence
Request (setup step of the check; CA was created with CertificateAuthorityType=ROOT, UsageMode=SHORT_LIVED_CERTIFICATE, KeyAlgorithm=RSA_2048, SigningAlgorithm=SHA256WITHRSA, and had reached Status=PENDING_CERTIFICATE):
  IssueCertificate
    CertificateAuthorityArn = <the short-lived ROOT CA>
    Csr                     = <the CSR returned by GetCertificateAuthorityCsr for that CA>
    SigningAlgorithm        = SHA256WITHRSA
    TemplateArn             = arn:aws:acm-pca:::template/RootCACertificate/V1
    Validity                = {"Value": 3, "Type": "YEARS"}

Expected (AWS): HTTP 200, body `{"CertificateArn": "<ca-arn>/certificate/<serial>"}`; GetCertificate then returns the PEM and ImportCertificateAuthorityCertificate drives the CA to Status=ACTIVE.

Actual (emulator): HTTP 400
  Error code:    InvalidArgsException
  Error message: "Validity may not exceed seven days for a certificate authority whose UsageMode is SHORT_LIVED_CERTIFICATE"
  (fixed text in full; no request- or state-derived values)

As surfaced by the replay runner:
  botocore.errorfactory.InvalidArgsException: An error occurred (InvalidArgsException) when calling the IssueCertificate operation: Validity may not exceed seven days for a certificate authority whose UsageMode is SHORT_LIVED_CERTIFICATE

The CA therefore stays in PENDING_CERTIFICATE and the check's own precondition assertion ("the CA must be ACTIVE before any authorization conclusion is drawn") can never be satisfied.

## Steps to Reproduce
1. CreateCertificateAuthority with CertificateAuthorityType=ROOT, UsageMode=SHORT_LIVED_CERTIFICATE, CertificateAuthorityConfiguration={KeyAlgorithm: RSA_2048, SigningAlgorithm: SHA256WITHRSA, Subject: {CommonName: <any>}}.
2. Poll DescribeCertificateAuthority until Status == PENDING_CERTIFICATE.
3. GetCertificateAuthorityCsr for that CA and keep the returned CSR PEM.
4. Call IssueCertificate on the same CA with Csr=<that CSR>, SigningAlgorithm=SHA256WITHRSA, TemplateArn=arn:aws:acm-pca:::template/RootCACertificate/V1, Validity={Value: 3, Type: YEARS}.
   -> AWS: HTTP 200 with a CertificateArn. Emulator: HTTP 400 InvalidArgsException "Validity may not exceed seven days for a certificate authority whose UsageMode is SHORT_LIVED_CERTIFICATE".
5. (Control, to show the cap itself is otherwise right) repeat step 4 with TemplateArn=arn:aws:acm-pca:::template/EndEntityCertificate/V1 and Validity={Value: 30, Type: DAYS} on an ACTIVE short-lived CA: the seven-day refusal IS correct there and must be kept.
6. Replay check acm-pca__iam.template-arn-condition-gates-a-real-signed-issue-certificate to confirm the fix; with the root-certificate issuance accepted, the CA reaches ACTIVE and the check proceeds to its acm-pca:TemplateArn authorization beats.
