Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.create-acme-domain-validation-malformed-domain-rejected acm.domain-validation-optional-field-empty-vs-absent-preserved
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.create-acme-domain-validation-malformed-domain-rejected|run#5|AssertionError|assert message == "1 validation error detected: Value 'invalid_domain_name.example.com' at 'domainName' failed to satisfy constraint: Member must satisfy regula
Issue-Key: acm.domain-validation-optional-field-empty-vs-absent-preserved|run#9|AssertionError|assert message == "2 validation errors detected: Value '' at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must have
Issue-Commit: 778841c19d7a134e4f15d723511a5118bd1b828c

# ACM constraint-violation ValidationException messages omit the rejected value: "Value of the input at '<path>'" instead of AWS's "Value '<value>' at '<path>'" (1 cause)

> _Note: this issue was identified against the code base at commit `778841c19d7a134e4f15d723511a5118bd1b828c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAcmeDomainValidation

## Description
One net-new root cause in how ACM words a ValidationException raised by a modeled input-constraint violation (@pattern, @length). Two failing checks share it; both get the error code, the HTTP status, the detected-error count, the member path and the constraint prose right, and diverge only in the clause that is supposed to quote the value that was rejected.

Cause 1 of 1: every per-error clause of ACM's constraint-violation ValidationException message renders the literal placeholder "Value of the input at '<memberPath>'" where AWS renders "Value '<the rejected value>' at '<memberPath>'", so the request value that failed the constraint never appears in the message.
Failing check ids for this cause: acm.create-acme-domain-validation-malformed-domain-rejected, acm.domain-validation-optional-field-empty-vs-absent-preserved.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) declares CreateAcmeDomainValidation.DomainName -> DomainName, required, @length min 1 max 253, @pattern ^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?$, and CreateAcmeDomainValidation.PrevalidationOptions -> PrevalidationOptions whose DnsPrevalidationOptions.HostedZoneId is an optional string with @length min 1 and @pattern Z[A-Z0-9]+; the operation models ValidationException (HTTP 400, awsQueryError code ValidationError, doc "The supplied input failed to satisfy constraints of an Amazon Web Services service."). The model does not state the message text, so for the text: recorded behavior only. AWS answers HTTP 400 ValidationException with a message of the form

    <N> validation error[s] detected: <clause>[; <clause> ...]

where each clause is

    Value '<the value the request sent for that member>' at '<lowerCamel member path>' failed to satisfy constraint: <constraint prose>

Anatomy of that message. Fixed text: "validation error detected: " / "validation errors detected: ", the per-clause skeleton "Value '" … "' at '" … "' failed to satisfy constraint: ", the clause separator "; ", and the constraint prose itself ("Member must satisfy regular expression pattern: <pattern>", "Member must have length greater than or equal to <min>"). Variable parts, all taken from the request and the model: the error count N, the rejected value quoted in single quotes (verbatim as the request sent it — including the empty string, which renders as ''), the lowerCamel member path of the offending member (e.g. domainName, prevalidationOptions.dnsPrevalidation.hostedZoneId) and the constraint's own parameter (the pattern source, the length bound). Nothing per-run (request id, timestamp, generated ARN) appears. Note AWS emits ONE clause per violated constraint on the same member, so an empty HostedZoneId that violates both @length and @pattern yields "2 validation errors detected: …; …" — the emulator already reproduces that count and ordering correctly.

Emulator: HTTP 400 ValidationException (correct class and status) whose every clause reads "Value of the input at '<memberPath>' failed to satisfy constraint: …" — the quoted request value is replaced by the fixed words "of the input". Observed verbatim:
  * malformed DomainName "invalid_domain_name.example.com": "1 validation error detected: Value of the input at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-z0-9]([a-z0-9-]*[a-z0-9])?\\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?" where AWS sends "1 validation error detected: Value 'invalid_domain_name.example.com' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-z0-9]([a-z0-9-]*[a-z0-9])?\\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?".
  * empty HostedZoneId: "2 validation errors detected: Value of the input at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must have length greater than or equal to 1; Value of the input at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must satisfy regular expression pattern: Z[A-Z0-9]+" where AWS sends the same message with each "Value of the input at" replaced by "Value '' at".

Why: a client relying on AWS's behavior cannot see WHICH value was rejected. ACM collapses every input-constraint refusal onto one error code (ValidationException), so the message is the only discriminator a caller has; AWS's form lets a client (or a user-facing error mapper, log scraper or test assertion) echo the offending value back, and batch/retry logic that parses the quoted value to decide what to correct and resend finds nothing to parse. Any assertion keyed on AWS's "Value '<x>' at '<path>' failed to satisfy constraint: …" form — the shape AWS uses service-wide for constraint violations — fails here even though the status, code, error count, path and constraint prose are all right, so a test that passes against AWS fails against the emulator.

Fix: the shared constraint-violation message renderer that ACM's requests reach in the framework's centralized deserialize -> validate -> handle walk (the validate step documented in services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_domain_validation.rs lines 21-26), so each clause substitutes the rejected member value in single quotes instead of the fixed words "of the input"; if that renderer must stay value-free for other services, ACM's own refusal-wording seam (`override_refusal` in services/service-com-amazonaws-acm-certificatemanager/src/service.rs) is where ACM's form belongs. Not in the individual operation handlers — the defect is in the one template both checks hit through two different members and two different constraint kinds.

This is distinct from the already-open 20261028T025913Z-acm-cross-account-arn-validationexception-message-drops-aws.md, which concerns the separate "Resource identifier for <type> has a validation error: …" envelope for cross-account ARNs and not the modeled-constraint clause template; neither that issue nor 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md carries these two checks.

Traces: acm.create-acme-domain-validation-malformed-domain-rejected, acm.domain-validation-optional-field-empty-vs-absent-preserved.

## Evidence
Failing check acm.create-acme-domain-validation-malformed-domain-rejected, verdict [mismatch]:

  File "check_acm_create_acme_domain_validation_malformed_domain_rejected.py", line 50, in run
    assert message == "1 validation error detected: Value 'invalid_domain_name.example.com' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-z0-9]([a-z0-9-]*[a-z0-9])?\\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?"
  AssertionError: expected the exact ValidationException message AWS returns, got "1 validation error detected: Value of the input at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-z0-9]([a-z0-9-]*[a-z0-9])?\\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?"

The preceding assertion `assert code == "ValidationException"` PASSED, so the status (400) and error code are correct and the divergence is confined to the message. The check's later assertions (nothing was created for the rejected domain; a valid domain on the same endpoint still creates a live record with a /acme-domain-validation/ ARN and a live Status) were not reached.

Failing check acm.domain-validation-optional-field-empty-vs-absent-preserved, verdict [mismatch]:

  File "check_acm_domain_validation_optional_field_empty_vs_absent_preserved.py", line 39, in run
    assert message == "2 validation errors detected: Value '' at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must satisfy regular expression pattern: Z[A-Z0-9]+"
  AssertionError: expected the exact ValidationException message AWS returns, got "2 validation errors detected: Value of the input at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must have length greater than or equal to 1; Value of the input at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must satisfy regular expression pattern: Z[A-Z0-9]+"

Here too `assert code == "ValidationException"` PASSED first. This check sends the empty string with botocore parameter validation disabled (Config(parameter_validation=False)) so the value reaches the service. Note the error count (2), the clause order (length before pattern), the member path and the constraint prose all MATCH AWS; only the "Value '<value>' at" clause head diverges — the same single template defect as the first check, reached through a different member and a different constraint kind.

Smithy citations (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): CreateAcmeDomainValidation.DomainName -> DomainName, required, @length 1..253, @pattern ^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?$; CreateAcmeDomainValidation.PrevalidationOptions -> PrevalidationOptions (required), DNS prevalidation's HostedZoneId @length min 1 / @pattern Z[A-Z0-9]+; ValidationException = client error, httpError 400, awsQueryError code ValidationError, member `message` -> ValidationExceptionMessage.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Create an ACME endpoint and wait for it to be ACTIVE: acm.create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', Contact='NOT_REQUIRED', CertificateAuthority={'PublicCertificateAuthority': {}}, Tags=[...]), then poll acm.describe_acme_endpoint(AcmeEndpointArn=<arn>) until Status == 'ACTIVE'.
3. Pattern violation: acm.create_acme_domain_validation(AcmeEndpointArn=<arn>, DomainName='invalid_domain_name.example.com', PrevalidationOptions={'DnsPrevalidation': {}}) — the underscore violates the modeled DomainName @pattern.
   Observe HTTP 400 ValidationException (correct) whose Error.Message is "1 validation error detected: Value of the input at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-z0-9]([a-z0-9-]*[a-z0-9])?\\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?". AWS answers the same status and code with the clause head "Value 'invalid_domain_name.example.com' at 'domainName' …" — i.e. the rejected value quoted in place of the words "of the input".
4. Length+pattern violation on a nested member: build a client with botocore Config(parameter_validation=False) so the empty string is not rejected client-side, then call acm.create_acme_domain_validation(AcmeEndpointArn=<arn>, DomainName=<a fresh lower-case domain>, PrevalidationOptions={'DnsPrevalidation': {'HostedZoneId': ''}}).
   Observe HTTP 400 ValidationException with "2 validation errors detected: Value of the input at 'prevalidationOptions.dnsPrevalidation.hostedZoneId' failed to satisfy constraint: Member must have length greater than or equal to 1; Value of the input at '…hostedZoneId' failed to satisfy constraint: Member must satisfy regular expression pattern: Z[A-Z0-9]+". AWS sends the identical message with each clause head reading "Value '' at '…'".
5. Re-replay acm.create-acme-domain-validation-malformed-domain-rejected and acm.domain-validation-optional-field-empty-vs-absent-preserved to confirm the fix; both checks continue past the message assertion to verify that nothing was created for the rejected input and that the valid/omitted-field control still creates a live VALIDATING record with PrevalidationType 'DNS'.
