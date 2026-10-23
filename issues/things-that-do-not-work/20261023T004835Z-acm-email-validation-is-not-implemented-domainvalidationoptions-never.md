Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.request-certificate-email-validation-domain-options acm.resend-validation-email-wrong-domain-rejected acm.op-contract-resend-validation-email
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.request-certificate-email-validation-domain-options|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: acm.resend-validation-email-wrong-domain-rejected|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: acm.op-contract-resend-validation-email|_client_error#6|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded and returned {result!r}")
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# ACM EMAIL validation is not implemented: DomainValidationOptions never carries ValidationEmails, and ResendValidationEmail accepts every call with 200 (2 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResendValidationEmail

## Description
Two distinct root causes covering ACM's EMAIL validation method. RequestCertificate and ResendValidationEmail both answer (no stubs), but the EMAIL validation state behind them does not exist. Not covered by an open issue: 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md records the missing DomainValidationOptions[*].ValidationDomain and the DNS-validation engine (Causes 1-4 there); neither it nor any other open issue names ValidationEmails or ResendValidationEmail's gate.

Cause 1 of 2: an EMAIL-validated certificate never gets its role-account address list.
AWS: Smithy DomainValidation declares ValidationEmails (ValidationEmailList) — 'A list of email addresses that ACM used to send domain validation emails' — and ValidationDomain. Recorded behavior: for RequestCertificate(ValidationMethod='EMAIL') the DomainValidationOptions entry for the domain is populated asynchronously with ValidationMethod='EMAIL', ValidationDomain=<the domain> and the five standard role-account addresses for that domain (admin@, administrator@, hostmaster@, postmaster@, webmaster@<domain>).
Emulator: the entry's ValidationEmails never appears — both checks poll DescribeCertificate for `d['DomainValidationOptions'][0].get('ValidationEmails')` and never see it, so the check is killed by the harness deadline ('Check timed out after 60.0s') with no further progress. An emulator computes this synchronously, so the first probe should already carry it.
Why: EMAIL validation is the documented path for a domain the caller cannot put a DNS record in, and the address list is the only thing telling the operator where the approval mail went. A client that reads ValidationEmails gets nothing (KeyError/None), and any flow that waits for the list hangs until its own timeout instead of proceeding.
Fix: carry EMAIL validation into certificate state and the DescribeCertificate/ListCertificateDomainValidations projection in the ACM service crate (services/service-com-amazonaws-acm-certificatemanager/src/ops/request_certificate.rs and the DomainValidation projection) — derive the five role-account addresses for the validation domain.
Traces: acm.request-certificate-email-validation-domain-options, acm.resend-validation-email-wrong-domain-rejected (both time out on exactly this poll, before reaching their own assertions).

Cause 2 of 2: ResendValidationEmail is unconditionally accepted.
AWS: Smithy ResendValidationEmail(CertificateArn, Domain, ValidationDomain — all required) -> Unit models InvalidArnException, InvalidDomainValidationOptionsException, InvalidStateException, ResourceNotFoundException and ValidationException (all HTTP 400). Recorded behavior: immediately after RequestCertificate returns — before ACM has dispatched the initial validation email — the call for the certificate's own EMAIL validation domain is refused with 400 InvalidStateException ('The certificate request is still in progress. Try again later'); the gate is short-lived and a bounded retry of the identical call then succeeds with 200. A certificate that does not use EMAIL validation for the requested domain is likewise refused with 400 InvalidStateException once its request has settled, and a Domain that is not on the certificate at all is refused with 400 ResourceNotFoundException.
Emulator: HTTP 200 (empty body) for the very first call, immediately after RequestCertificate — no gate, no state check.
Why: overly permissive. The emulator reports that a validation email was re-sent for a certificate (and even for a domain/validation method) where AWS refuses, so a script that relies on the refusal to detect 'not ready yet' or 'wrong domain' proceeds on a false success and the operator waits for mail that AWS would never have sent.
Fix: the state/domain gate in services/service-com-amazonaws-acm-certificatemanager/src/ops/resend_validation_email.rs — refuse until the initial validation email has been dispatched, refuse a domain whose DomainValidationOptions entry is not EMAIL, and 404-class (ResourceNotFoundException) a Domain with no entry on the certificate.
Traces: acm.op-contract-resend-validation-email.

## Evidence
Cause 1: acm.request-certificate-email-validation-domain-options — 'Error: Check timed out after 60.0s' while polling `acm.describe_certificate(CertificateArn=arn)['Certificate']` for `d.get('DomainValidationOptions') and d['DomainValidationOptions'][0].get('ValidationEmails')` after acm.request_certificate(DomainName=<label>.acmqa.co, ValidationMethod='EMAIL'); the check then asserts ValidationMethod=='EMAIL', ValidationDomain==<domain> and set(ValidationEmails)=={admin,administrator,hostmaster,postmaster,webmaster}@<domain>. acm.resend-validation-email-wrong-domain-rejected — same timeout on the same poll.
Cause 2: acm.op-contract-resend-validation-email — AssertionError: expected a refusal, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}, for acm.resend_validation_email(CertificateArn=<EMAIL cert just created>, Domain=<its domain>, ValidationDomain=<its domain>); expected 400 InvalidStateException.
Smithy citations: DomainValidation{DomainName, ValidationEmails: ValidationEmailList, ValidationDomain, ValidationStatus, ResourceRecord, HttpRedirect, ValidationMethod}; ResendValidationEmail errors [InvalidArnException, InvalidDomainValidationOptionsException, InvalidStateException, ResourceNotFoundException, ValidationException] all HTTP 400.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Cause 1: arn = acm.request_certificate(DomainName=<a non-RFC2606 domain>, ValidationMethod='EMAIL')['CertificateArn']; poll acm.describe_certificate(CertificateArn=arn)['Certificate']['DomainValidationOptions'][0] — 'ValidationEmails' never appears (expected the five role-account addresses for the domain).
3. Cause 2: immediately after that RequestCertificate, call acm.resend_validation_email(CertificateArn=arn, Domain=<domain>, ValidationDomain=<domain>) — HTTP 200 instead of 400 InvalidStateException; repeat against a DNS-validated certificate's own domain (expected 400 InvalidStateException) and with a domain that is not on the certificate (expected 400 ResourceNotFoundException).
4. Re-replay the three checks in trace_ids.
