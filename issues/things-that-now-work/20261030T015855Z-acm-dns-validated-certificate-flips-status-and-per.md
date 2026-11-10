Issue-Source: trace-sweep-divergence
Issue-Trace: acm-certificate-dns-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-certificate-dns-lifecycle
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.DomainValidationOptions[*].ValidationStatus
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Status
Issue-Commit: 6007c454e542516ce92f9fc20505b3e72db0d81f

# ACM DNS-validated certificate flips Status and per-domain ValidationStatus to FAILED while the request is still pending

> _Note: this issue was identified against the code base at commit `6007c454e542516ce92f9fc20505b3e72db0d81f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
One root cause (Cause 1 of 1): the ACM emulator's certificate state machine terminally FAILS a DNS-validated AMAZON_ISSUED certificate request that is still awaiting validation, instead of leaving it in PENDING_VALIDATION. Later in the same DNS lifecycle fixture, DescribeCertificate reports Certificate.Status = 'FAILED' and every Certificate.DomainValidationOptions[*].ValidationStatus = 'FAILED', where AWS still reports 'PENDING_VALIDATION' for all three.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) CertificateDetail.Status -> CertificateStatus enum [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED], documented as: "A certificate enters status PENDING_VALIDATION upon being requested, unless it fails for any of the reasons given in the troubleshooting topic Certificate request fails. ACM makes repeated attempts to validate a certificate for 72 hours and then times out. If a certificate shows status FAILED or VALIDATION_TIMED_OUT, delete the request, correct the issue with DNS validation or Email validation, and try again. If validation succeeds, the certificate enters status ISSUED." So the model and its documentation state the rule directly: a requested certificate stays PENDING_VALIDATION until validation succeeds (-> ISSUED), the 72-hour window elapses (-> VALIDATION_TIMED_OUT), or the request fails for one of the enumerated request-failure reasons. FAILED is reserved for that last case and is the only status for which CertificateDetail.FailureReason exists ("This value exists only when the certificate status is FAILED"), drawn from FailureReason [NO_AVAILABLE_CONTACTS, ADDITIONAL_VERIFICATION_REQUIRED, DOMAIN_NOT_ALLOWED, INVALID_PUBLIC_DOMAIN, DOMAIN_VALIDATION_DENIED, CAA_ERROR, PCA_*, SLR_NOT_FOUND, OTHER] — none of which is "the CNAME has not been published yet". Per-domain, DomainValidation.ValidationStatus -> DomainStatus enum [PENDING_VALIDATION, SUCCESS, FAILED], and an unobserved DNS challenge is PENDING_VALIDATION, not FAILED. Recorded behavior agrees: across the DNS lifecycle fixture, with the validation CNAME never published and well inside the 72-hour window, both the certificate Status and every per-domain ValidationStatus remain 'PENDING_VALIDATION'.

Emulator: at the same point in the lifecycle the emulator has already transitioned the certificate to the terminal 'FAILED' state and propagated 'FAILED' to each DomainValidationOptions entry. HTTP 200 and the response envelope are correct; only these status values diverge, so no client sees an error — it just reads a definitive wrong answer. Hypothesis: the emulator treats a validation attempt that does not immediately observe the challenge record as a request failure, collapsing "not validated yet" into FAILED rather than modelling the pending/retry window (and without the VALIDATION_TIMED_OUT state that AWS uses when the window really does elapse).

Why: PENDING_VALIDATION -> ISSUED is the entire DNS issuance workflow, and FAILED is terminal and documented as "delete the request, correct the issue, and try again". A client that polls DescribeCertificate waiting for ISSUED (the standard pattern, and what the certificate-issued waiter does) sees FAILED and stops polling, surfaces a hard provisioning error, or deletes and re-requests the certificate — on a request AWS would have gone on to issue. A client that branches on DomainValidationOptions[*].ValidationStatus to decide whether its published CNAME was accepted is told its record was rejected when ACM has not yet looked. Because the emulator reports FAILED without a FailureReason, a client that reads FailureReason to explain or classify the failure also gets None for a status where AWS guarantees the member is present, so the error cannot even be diagnosed. The inverted value is worse than a missing one: it reads as a definite, terminal verdict.

Fix: the ACM certificate state machine / DNS-validation progression in the ACM service crate — the stored certificate state that DescribeCertificate's CertificateDetail projection reads. A requested AMAZON_ISSUED certificate whose challenge has not been observed must remain Status 'PENDING_VALIDATION' with every DomainValidationOptions[*].ValidationStatus 'PENDING_VALIDATION'; only a modelled request-failure reason may move it to 'FAILED' (and then FailureReason must be populated), and only elapsing the validation window may move it to 'VALIDATION_TIMED_OUT'.

Traces: acm-certificate-dns-lifecycle#task#7 (the sole member of this cluster). Note the related open filing 20261029T192604Z-acm-certificate-read-projections-omit-and-mis-name.md, which owns task#2 of this same fixture: it covers how the read operations PROJECT validation fields (RequestedValidationConfiguration vs ActiveValidationConfiguration, ValidationEmails, CreatedAt, CT preference) and does not carry this fixture step or the Status/ValidationStatus state transition as a replay key. 20261029T093731Z-acm-dns-validated-certificate-issuance-is-unreachable-the.md covers hosted-zone discoverability for issuance, not the status the emulator reports for a pending request.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (78 fixtures, 388 steps, 343 passed, 44 mismatched). This cluster: verdict=mismatch, operation DescribeCertificate, expected_status 200 and actual_status 200 — a body-only divergence; divergence_key service=acm|DescribeCertificate|status=200->200|keys=$.Certificate.DomainValidationOptions[0].ValidationStatus,$.Certificate.DomainValidationOptions[1].ValidationStatus,$.Certificate.Status; affected_count 1 (acm-certificate-dns-lifecycle#task#7); stub_path empty and owned_by empty, so no stub issue and no open issue carries this replay key.

Reported detail, verbatim:
  $.Certificate.DomainValidationOptions[0].ValidationStatus: value_mismatch (expected 'PENDING_VALIDATION', got 'FAILED')
  $.Certificate.DomainValidationOptions[1].ValidationStatus: value_mismatch (expected 'PENDING_VALIDATION', got 'FAILED')
  $.Certificate.Status: value_mismatch (expected 'PENDING_VALIDATION', got 'FAILED')

All three paths are modelled enum members, so the diverging values are the requirement and nothing here is a per-run value: 'PENDING_VALIDATION' and 'FAILED' are literal CertificateStatus / DomainStatus enum tokens. The two DomainValidationOptions entries correspond to the apex name and its 'www.' SAN requested by this fixture; the generated domain name, certificate ARN and account id in the fixture are per-run values and are NOT part of the requirement — the requirement is that every entry for a not-yet-validated DNS challenge carries ValidationStatus 'PENDING_VALIDATION' and that Certificate.Status is 'PENDING_VALIDATION'. Earlier in this same fixture (task#2) the emulator itself still reported ValidationStatus 'PENDING_VALIDATION', which confirms the divergence is a later, incorrect transition into a terminal state rather than a constant mis-projection.

Smithy citations: CertificateDetail.Status -> CertificateStatus [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED] with the documented 72-hour retry-then-timeout rule quoted in the description; CertificateDetail.FailureReason -> FailureReason, "This value exists only when the certificate status is FAILED"; CertificateDetail.DomainValidationOptions -> DomainValidationList (len 1..1000), "This field exists only when the certificate type is AMAZON_ISSUED"; DomainValidation.ValidationStatus -> DomainStatus [PENDING_VALIDATION, SUCCESS, FAILED].

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1; this reproduces as verdict=mismatch at HTTP 200 on acm-certificate-dns-lifecycle#task#7.
2. Directly: acm.request_certificate(DomainName='<d>', SubjectAlternativeNames=['www.<d>'], ValidationMethod='DNS') and keep the returned CertificateArn.
3. Do NOT publish the validation CNAME, and stay well inside the 72-hour validation window. Drive the rest of the acm-certificate-dns-lifecycle fixture's steps against the same certificate.
4. Call acm.describe_certificate(CertificateArn=<arn>) and read Certificate.Status and Certificate.DomainValidationOptions[*].ValidationStatus.
5. Observed: Status == 'FAILED' and both DomainValidationOptions[0].ValidationStatus and [1].ValidationStatus == 'FAILED' (and Certificate.FailureReason absent, though AWS guarantees it when Status is FAILED). Expected (AWS): Status == 'PENDING_VALIDATION' and both per-domain ValidationStatus == 'PENDING_VALIDATION'.
6. Re-replay acm-certificate-dns-lifecycle#task#7 and confirm it passes before closing; task#2 of the same fixture belongs to issue 20261029T192604Z-acm-certificate-read-projections-omit-and-mis-name.md and is not closed by this fix.
