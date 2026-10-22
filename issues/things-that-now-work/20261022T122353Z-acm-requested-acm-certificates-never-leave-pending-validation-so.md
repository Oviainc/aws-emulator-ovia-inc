Issue-Source: trace-sweep-divergence
Issue-Trace: acm-certificate-email-with-inline-tags
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-certificate-email-with-inline-tags acm-request-certificate-dns-lifecycle
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DescribeCertificate|2xx->2xx|$
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.DomainValidationOptions[*].ValidationStatus
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.FailureReason
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Status
Issue-Related: 20261022T122353Z-acm-describecertificate-omits-modeled-certificate-metadata-and-mis-forms.md
Issue-Commit: 30db2a8818f1221c3278e244a1430bae2ca83b41

# Requested ACM certificates never leave PENDING_VALIDATION, so polls never terminate

> _Note: this issue was identified against the code base at commit `30db2a8818f1221c3278e244a1430bae2ca83b41`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
A certificate requested from the emulator stays in PENDING_VALIDATION indefinitely: there is no validation-state progression, so a client that polls DescribeCertificate for a terminal status never gets one. Both replayed polls hit the 60s cap. Two causes, one per validation method.

Cause 1 of 2 - EMAIL-validated certificate never reaches a terminal status.
AWS: Smithy CertificateDetail.Status targets CertificateStatus and FailureReason targets FailureReason, documented to exist 'only when the certificate status is FAILED'. The recorded response for this request reaches Status='FAILED' with FailureReason='ADDITIONAL_VERIFICATION_REQUIRED', DomainValidationOptions[0].ValidationStatus='FAILED', ValidationEmails=[] and RenewalEligibility='INELIGIBLE' inside the poll window.
Emulator: Status stays 'PENDING_VALIDATION', DomainValidationOptions[0].ValidationStatus stays 'PENDING_VALIDATION', FailureReason is never set, and the fixture's poll is capped at 60s without ever matching.
Why: any client (CLI waiter, SDK waiter, hand-rolled poll loop) that waits for ISSUED or FAILED hangs until its own timeout instead of observing a terminal state, and never sees FailureReason at all.
Fix: certificate validation state machine in the ACM service state - a requested AMAZON_ISSUED certificate must transition out of PENDING_VALIDATION to a terminal status and set FailureReason/ValidationStatus accordingly. Hypothesis: the recorded account cannot verify these probe domains, which is why the terminal state is FAILED/ADDITIONAL_VERIFICATION_REQUIRED rather than ISSUED; the defect is the absence of any transition, not the particular terminal value.

Cause 2 of 2 - DNS-validated certificate never reaches a terminal status.
AWS: same members; the recorded response reaches Status='FAILED', FailureReason='ADDITIONAL_VERIFICATION_REQUIRED', DomainValidationOptions[0].ValidationStatus='FAILED' with no ResourceRecord asserted at that point.
Emulator: Status and ValidationStatus remain 'PENDING_VALIDATION' (with a synthesized CNAME ResourceRecord) for the whole 60s poll.
Why: identical - DNS validation flows that poll to completion never complete.
Fix: same state machine.

Note: both polls additionally show the CertificateDetail shaping gaps (missing Issuer/Subject/SignatureAlgorithm/KeyUsages/ExtendedKeyUsages/Options, KeyAlgorithm 'RSA_2048' vs 'RSA-2048') filed separately as the DescribeCertificate projection issue; fixing the projection alone will NOT make these two steps pass, because the status never becomes terminal.

Traces: acm-certificate-email-with-inline-tags#task#2 (representative), acm-request-certificate-dns-lifecycle#task#2.

## Evidence
acm-certificate-email-with-inline-tags#task#2: verdict mismatch, HTTP 200 both sides, '$: poll capped at 60s'. Expected $.Certificate.Status='FAILED', FailureReason='ADDITIONAL_VERIFICATION_REQUIRED', DomainValidationOptions[0].ValidationStatus='FAILED', ValidationEmails=[]; actual Status='PENDING_VALIDATION', ValidationStatus='PENDING_VALIDATION', no FailureReason, ValidationEmails=[administrator@, hostmaster@, postmaster@, webmaster@, admin@ ...].
acm-request-certificate-dns-lifecycle#task#2: verdict mismatch, HTTP 200 both sides, '$: poll capped at 60s'. Expected Status='FAILED', FailureReason='ADDITIONAL_VERIFICATION_REQUIRED', DomainValidationOptions[0].ValidationStatus='FAILED'; actual Status='PENDING_VALIDATION', ValidationStatus='PENDING_VALIDATION' with ResourceRecord CNAME _<hash>.<domain>. -> _<hash>.acm-validations.aws.
Smithy: CertificateDetail.Status (enum CertificateStatus), CertificateDetail.FailureReason (enum FailureReason, 'exists only when the certificate status is FAILED'), DomainValidation.ValidationStatus (enum DomainStatus with PENDING_VALIDATION/SUCCESS/FAILED).

## Steps to Reproduce
1. Replay the ACM wire corpus against the emulator.
2. RequestCertificate with ValidationMethod=EMAIL (acm-certificate-email-with-inline-tags) and with ValidationMethod=DNS (acm-request-certificate-dns-lifecycle).
3. Poll DescribeCertificate on the returned ARN waiting for a terminal $.Certificate.Status.
4. Observe the status never leaves PENDING_VALIDATION and the replay reports 'poll capped at 60s' at acm-certificate-email-with-inline-tags#task#2 and acm-request-certificate-dns-lifecycle#task#2.
