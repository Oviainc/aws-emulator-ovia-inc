Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the acm-pca emulator

_Filed: 2026-10-21T21:53:21Z_
_Operation: service-com-amazonaws-acmpca-acmprivateca | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 23 operations of ACM Private CA (`ACMPrivateCA.<Op>` over awsJson1_1, POST /) are unmodified scaffolds answering HTTP 501 `NotImplemented` and holding zero state. Implement a FIRST stateful slice: the private-CA lifecycle that every client script starts with — create a CA, activate it with its own certificate, issue an end-entity certificate from it, and read that certificate back. Every one of these must compute its response from persisted state held in the shared engine store (`crate::state::service_state(&*self.engine)`); no canned ARNs, no echoed inputs, no random id that is never stored.

Happy path, in order:

1. **CreateCertificateAuthority** — input `CertificateAuthorityConfiguration` (required: `KeyAlgorithm`, `SigningAlgorithm`, `Subject`), `CertificateAuthorityType` (ROOT|SUBORDINATE, required), optional `RevocationConfiguration`, `Tags`, `UsageMode`, `KeyStorageSecurityStandard`, `IdempotencyToken`. Mint an ARN of the form `arn:aws:acm-pca:<region>:<account>:certificate-authority/<uuid>`, store the CA record with `Status = PENDING_CERTIFICATE`, `CreatedAt`/`LastStateChangeAt` = now, `Serial`, `Type`, the supplied configuration and tags, and return `CertificateAuthorityArn`. Reject an unknown/ill-formed ARN-less or malformed config with `InvalidArgsException`.
2. **GetCertificateAuthorityCsr** — input `CertificateAuthorityArn`. Return the stored/derived `Csr` (a stable PEM string generated at create time and persisted on the CA, so repeated calls return the same CSR). Unknown ARN → `ResourceNotFoundException`; ARN not matching the modeled pattern → `InvalidArnException`.
3. **ImportCertificateAuthorityCertificate** — input `CertificateAuthorityArn`, `Certificate` (blob, required), optional `CertificateChain`. Persist the bytes on the CA, set `Status = ACTIVE`, set `NotBefore`/`NotAfter` and bump `LastStateChangeAt`. Importing into a CA that is not `PENDING_CERTIFICATE` → `InvalidStateException`.
4. **DescribeCertificateAuthority** — input `CertificateAuthorityArn`; output the full `CertificateAuthority` structure read back from state: `Arn`, `OwnerAccount`, `CreatedAt`, `LastStateChangeAt`, `Type`, `Serial`, `Status`, `NotBefore`, `NotAfter`, `CertificateAuthorityConfiguration`, `RevocationConfiguration`, `UsageMode`, `KeyStorageSecurityStandard`. The values must be exactly what step 1/3 stored (status must read `PENDING_CERTIFICATE` before import and `ACTIVE` after).
5. **IssueCertificate** — input `CertificateAuthorityArn`, `Csr` (blob, required), `SigningAlgorithm` (required), `Validity` (required: `Value`+`Type` of DAYS|MONTHS|YEARS|ABSOLUTE|END_DATE), optional `TemplateArn`, `ValidityNotBefore`, `ApiPassthrough`, `IdempotencyToken`. Only an `ACTIVE` CA may issue (otherwise `InvalidStateException`); unknown CA → `ResourceNotFoundException`. Allocate a hex serial, store an issued-certificate record under the CA (serial, PEM body, chain = CA cert + chain, requested validity resolved to NotBefore/NotAfter, status ISSUED), and return `CertificateArn` = `<ca-arn>/certificate/<serial>`. Equal `IdempotencyToken` within the idempotency window must return the SAME `CertificateArn` rather than minting a second certificate (op is `@idempotent`).
6. **GetCertificate** — input `CertificateAuthorityArn`, `CertificateArn`. Look the issued record up under that CA and return its `Certificate` plus `CertificateChain`. A certificate ARN unknown to that CA → `ResourceNotFoundException`; a `CertificateArn` whose CA prefix does not match the supplied `CertificateAuthorityArn` must not resolve.

Also wire **GetCertificateAuthorityCertificate** as the trivial read of the bytes imported in step 3 if it falls out for free; everything else (ListCertificateAuthorities paging, RevokeCertificate/audit reports, Tag/Untag/ListTags, permissions and resource policies, Update/Delete/RestoreCertificateAuthority) stays 501 and is covered by later per-operation gap issues.

## State Model

Store one struct in the service state (`src/state.rs`), guarded by the existing lock:

- `certificate_authorities: HashMap<String /* ca arn */, CertificateAuthorityRecord>`
- `CertificateAuthorityRecord { arn: String, owner_account: String, ca_type: CertificateAuthorityType, status: CertificateAuthorityStatus, serial: String, created_at: SystemTime, last_state_change_at: SystemTime, not_before: Option<SystemTime>, not_after: Option<SystemTime>, failure_reason: Option<FailureReason>, configuration: CertificateAuthorityConfiguration, revocation_configuration: Option<RevocationConfiguration>, usage_mode: Option<CertificateAuthorityUsageMode>, key_storage_security_standard: Option<KeyStorageSecurityStandard>, csr_pem: String, ca_certificate_pem: Option<String>, ca_certificate_chain_pem: Option<String>, restorable_until: Option<SystemTime>, tags: Vec<Tag>, next_serial: u64, certificates: HashMap<String /* certificate arn */, IssuedCertificate>, issue_idempotency: HashMap<String /* IdempotencyToken */, String /* certificate arn */>, create_idempotency_token: Option<String> }`
- `IssuedCertificate { arn: String, serial: String, certificate_pem: String, certificate_chain_pem: Option<String>, signing_algorithm: SigningAlgorithm, template_arn: Option<String>, not_before: SystemTime, not_after: SystemTime, issued_at: SystemTime, revoked: Option<(SystemTime, RevocationReason)> }`
- `create_idempotency: HashMap<String /* IdempotencyToken */, String /* ca arn */>` at the top level, so a repeated CreateCertificateAuthority with the same token returns the first CA's ARN.

Done = a client script can run steps 1→6 against the emulator and observe real state transitions: the describe between create and import reports `PENDING_CERTIFICATE`, the describe after import reports `ACTIVE` with the subject/key algorithm it supplied, and GetCertificate returns the exact PEM issued for the ARN IssueCertificate returned.

## Evidence

`services/service-com-amazonaws-acmpca-acmprivateca/OPERATIONS.md` lists all 23 operations with the input/output members and errors cited above (CreateCertificateAuthority -> CertificateAuthorityArn; GetCertificateAuthorityCsr -> Csr; ImportCertificateAuthorityCertificate; DescribeCertificateAuthority -> CertificateAuthority; IssueCertificate -> CertificateArn; GetCertificate -> Certificate/CertificateChain), the awsJson1_1 / `X-Amz-Target: ACMPrivateCA.<Op>` wire protocol, and the `@idempotent` trait on CreateCertificateAuthority and IssueCertificate. `smithy_query shape=CertificateAuthority,CertificateAuthorityConfiguration,Validity` supplies the exact member set of the describe payload and the status enum (ACTIVE | CREATING | DELETED | DISABLED | EXPIRED | FAILED | PENDING_CERTIFICATE). `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-acmpca-acmprivateca/src/ops/` matches all 23 handler files, i.e. every operation currently returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501, and no tests exercise any behavior.

## Steps to Reproduce

1. POST / with X-Amz-Target: ACMPrivateCA.CreateCertificateAuthority and body {"CertificateAuthorityConfiguration":{"KeyAlgorithm":"RSA_2048","SigningAlgorithm":"SHA256WITHRSA","Subject":{"CommonName":"example-root"}},"CertificateAuthorityType":"ROOT"} — expect 200 with a CertificateAuthorityArn; today 501 NotImplemented.
2. POST ACMPrivateCA.DescribeCertificateAuthority with that ARN — expect CertificateAuthority.Status == PENDING_CERTIFICATE and the Subject/KeyAlgorithm just supplied.
3. POST ACMPrivateCA.GetCertificateAuthorityCsr with that ARN — expect a non-empty Csr, identical on a second call.
4. POST ACMPrivateCA.ImportCertificateAuthorityCertificate with that ARN and a Certificate blob — expect 200.
5. POST ACMPrivateCA.DescribeCertificateAuthority again — expect Status == ACTIVE and NotBefore/NotAfter populated.
6. POST ACMPrivateCA.IssueCertificate with that ARN, a Csr blob, SigningAlgorithm SHA256WITHRSA, Validity {"Value":365,"Type":"DAYS"} and IdempotencyToken "tok-1" — expect a CertificateArn under the CA ARN; repeat with the same token and expect the identical CertificateArn.
7. POST ACMPrivateCA.GetCertificate with the CA ARN and that CertificateArn — expect the issued Certificate PEM and CertificateChain, not 501 and not an empty body.
8. POST ACMPrivateCA.GetCertificate with a CertificateArn that was never issued — expect ResourceNotFoundException (HTTP 400), not 501.
