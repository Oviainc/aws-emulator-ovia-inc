Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the acm (AWS Certificate Manager) emulator

_Filed: 2026-10-21T21:18:48Z_
_Operation: service-com-amazonaws-acm-certificatemanager | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 40 operations in `services/service-com-amazonaws-acm-certificatemanager/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Nothing works end to end.

This issue defines the FIRST, SMALL slice: the public-certificate lifecycle of ACM's primary resource (a certificate), implemented statefully over the awsJson1_1 wire protocol (`POST /`, `X-Amz-Target: CertificateManager.<Op>`).

### Happy path to implement (in order)

1. **RequestCertificate** — input `DomainName` (required), `ValidationMethod` (DNS|EMAIL|HTTP), `SubjectAlternativeNames`, `IdempotencyToken`, `KeyAlgorithm`, `Options`, `Tags`. Must mint a fresh ARN of the form `arn:aws:acm:<region>:<account>:certificate/<uuid>`, persist a new certificate record, and return that `CertificateArn`. Reusing the same `IdempotencyToken` with the same request parameters must return the SAME ARN (no second certificate created); a different token creates a new one.
2. **DescribeCertificate** — input `CertificateArn` (required); output `Certificate` (CertificateDetail) read back from the stored record: `CertificateArn`, `DomainName`, `SubjectAlternativeNames` (DomainName plus any SANs), `Status` = `PENDING_VALIDATION` for a requested public cert, `Type` = `AMAZON_ISSUED`, `CreatedAt`, `KeyAlgorithm` (default `RSA_2048`), `RenewalEligibility`, `Options`, and one `DomainValidationOptions` entry per domain carrying `DomainName`, `ValidationMethod` and, for DNS validation, a `ResourceRecord` {Name, Type=CNAME, Value} that is stable across repeated describes. Unknown-but-well-formed ARN ⇒ `ResourceNotFoundException` (400); malformed ARN ⇒ `InvalidArnException` (400).
3. **AddTagsToCertificate** / **ListTagsForCertificate** — `CertificateArn` + `Tags` (list<Tag>, 1..50) in; tags must be stored on the certificate record so `ListTagsForCertificate` returns exactly what was added (merged by key, later value wins). Tagging a nonexistent ARN ⇒ `ResourceNotFoundException`.
4. **ListCertificates** — output `CertificateSummaryList` (list<CertificateSummary>) built from the stored map, one summary per certificate with at least `CertificateArn`, `DomainName`, `SubjectAlternativeNameSummaries`, `Status`, `Type`, `KeyAlgorithm`, `CreatedAt`, `RenewalEligibility`, `InUse=false`, `Exported=false`. Must honour `CertificateStatuses` filtering, `MaxItems` (1..1000) truncation with an opaque `NextToken` that resumes exactly where the previous page stopped, and `SortBy=CREATED_AT` with `SortOrder` ASCENDING/DESCENDING.
5. **DeleteCertificate** — `CertificateArn` (required); removes the record and its tags; the certificate must then be invisible to `ListCertificates` and `DescribeCertificate` must answer `ResourceNotFoundException`. Deleting the same ARN twice ⇒ `ResourceNotFoundException` on the second call.

No canned, hardcoded or echoed responses: each of the five operations must compute its response from the persisted store. In particular `DescribeCertificate` must return what `RequestCertificate` created (not an echo of the describe input), `ListCertificates` must enumerate the live store (not a fixed list), and a minted ARN with no stored record behind it is a failure.

## State Model

Service state lives behind `crate::state::service_state(&*self.engine)` (the guard shown in every stub's header doc):

```text
AcmState {
    region: String,                  // from the engine/config, used to build ARNs
    account_id: String,              // 12-digit, used to build ARNs
    certificates: BTreeMap<String /*certificate ARN*/, Certificate>,
    idempotency: HashMap<String /*IdempotencyToken*/, String /*certificate ARN*/>,
}

Certificate {
    arn: String,                     // arn:aws:acm:<region>:<account>:certificate/<uuid>
    domain_name: String,
    subject_alternative_names: Vec<String>,   // domain_name first, then requested SANs
    status: CertificateStatus,       // PENDING_VALIDATION on request; ISSUED for imported
    cert_type: CertificateType,      // AMAZON_ISSUED | IMPORTED
    key_algorithm: KeyAlgorithm,     // default RSA_2048
    validation_method: Option<ValidationMethod>,
    domain_validations: Vec<DomainValidationRecord>,
    options: Option<CertificateOptions>,
    certificate_authority_arn: Option<String>,
    managed_by: Option<CertificateManagedBy>,
    renewal_eligibility: RenewalEligibility,
    in_use_by: Vec<String>,          // empty in the first slice
    created_at: SystemTime,
    issued_at: Option<SystemTime>,
    not_before: Option<SystemTime>,
    not_after: Option<SystemTime>,
    tags: BTreeMap<String, String>,  // key -> value, surfaced by ListTagsForCertificate
}

DomainValidationRecord {
    domain_name: String,
    validation_status: DomainStatus,   // PENDING_VALIDATION
    validation_method: ValidationMethod,
    validation_domain: Option<String>,
    resource_record: Option<ResourceRecord>,  // {name, type: CNAME, value} — stable per domain
}
```

Pagination tokens for `ListCertificates` are derived from the last emitted ARN in the sorted key order, so a `NextToken` resumes deterministically.

Everything else — `ImportCertificate`, `GetCertificate`, `ExportCertificate`, `RenewCertificate`, `RevokeCertificate`, `UpdateCertificateOptions`, `ResendValidationEmail`, `SearchCertificates`, `TagResource`/`UntagResource`, the account-configuration ops and the whole `*Acme*` family — stays 501 for now and will be filed as individual gap issues.

Done when a client script (steps below) runs end to end against the emulator with correct, stateful bodies, not merely 2xx.

## Evidence

`services/service-com-amazonaws-acm-certificatemanager/OPERATIONS.md` lists all 40 modeled operations with their input/output members and errors, and the wire protocol (awsJson1_1, `X-Amz-Target: CertificateManager.<Op>`, sigv4 name `acm`): RequestCertificate (in `DomainName` required, `ValidationMethod`, `SubjectAlternativeNames`, `IdempotencyToken`, `Tags`, `KeyAlgorithm`; out `CertificateArn`), DescribeCertificate (out `Certificate` of CertificateDetail; errors InvalidArnException/ResourceNotFoundException), ListCertificates (@paginated inputToken=NextToken outputToken=NextToken items=CertificateSummaryList, `MaxItems` range 1..1000, `SortBy`/`SortOrder`), AddTagsToCertificate and ListTagsForCertificate (`Tags` list<Tag> 1..50, ResourceNotFoundException), DeleteCertificate (ResourceNotFoundException/ResourceInUseException). `smithy_query` for `CertificateDetail`, `CertificateSummary` and `DomainValidation` confirms the field names/types quoted above (Status, Type, CreatedAt, DomainValidationOptions.ResourceRecord, SubjectAlternativeNameSummaries, InUse, Exported). Handler proof: `src/ops/request_certificate.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 239 `Err(crate::scaffolded::Scaffolded::not_implemented("RequestCertificate"))`; every other file in `src/ops/` is the same generated shape. No tests exist asserting any ACM behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `acm`.
2. `aws acm request-certificate --domain-name www.example.com --validation-method DNS --subject-alternative-names example.com --idempotency-token tok1` — expect 200 with a `CertificateArn` of the form arn:aws:acm:<region>:<account>:certificate/<uuid> (currently HTTP 501 NotImplemented).
3. Repeat the identical request-certificate call with `--idempotency-token tok1` — expect the SAME CertificateArn and no second certificate.
4. `aws acm describe-certificate --certificate-arn <arn>` — expect Certificate.DomainName=www.example.com, SubjectAlternativeNames containing example.com, Status=PENDING_VALIDATION, Type=AMAZON_ISSUED, CreatedAt set, and a DomainValidationOptions entry per domain with ValidationMethod=DNS and a stable ResourceRecord CNAME.
5. `aws acm add-tags-to-certificate --certificate-arn <arn> --tags Key=env,Value=dev` then `aws acm list-tags-for-certificate --certificate-arn <arn>` — expect exactly the env=dev tag back.
6. `aws acm list-certificates` — expect the certificate's summary (ARN, DomainName, Status, Type, CreatedAt). Request a second certificate for other.example.com, then `aws acm list-certificates --max-items 1` — expect one summary plus a NextToken, and passing that NextToken returns the other certificate with no repeats.
7. `aws acm list-certificates --certificate-statuses ISSUED` — expect the PENDING_VALIDATION certificates filtered out.
8. `aws acm delete-certificate --certificate-arn <arn>` — expect 200; then `aws acm describe-certificate --certificate-arn <arn>` must return ResourceNotFoundException (HTTP 400) and the ARN must be gone from list-certificates.
9. `aws acm describe-certificate --certificate-arn arn:aws:acm:us-east-1:123456789012:certificate/does-not-exist` — expect ResourceNotFoundException, and a malformed ARN must yield InvalidArnException.
