Issue-Source: trace-sweep-divergence
Issue-Trace: acm-acm-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-acm-error-paths
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|RequestCertificate|4xx->2xx|$.__type
Issue-Key: acm|RequestCertificate|4xx->2xx|$.message
Issue-Key: acm|RequestCertificate|4xx->2xx|status
Issue-Commit: 30db2a8818f1221c3278e244a1430bae2ca83b41

# RequestCertificate accepts a ValidationDomain that is not a superdomain

> _Note: this issue was identified against the code base at commit `30db2a8818f1221c3278e244a1430bae2ca83b41`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestCertificate

## Description
Cause 1 of 1 - RequestCertificate does not validate DomainValidationOptions[].ValidationDomain against the requested DomainName, so a request AWS rejects is accepted and a certificate is created.
AWS: Smithy DomainValidationOption.ValidationDomain is documented as 'This must be the same as the DomainName value or a superdomain of the DomainName value', and RequestCertificate models InvalidDomainValidationOptionsException (HTTP 400). The recording shows HTTP 400 with $.__type='InvalidDomainValidationOptionsException' when ValidationDomain is unrelated to DomainName (recorded message, as an example of the rule: 'The validation domain, totally-unrelated.org, must be a superdomain of the requested domain, <domain>.').
Emulator: HTTP 200 - the request succeeds and returns a CertificateArn; neither $.__type nor $.message is present.
Why: a client's negative path silently becomes a positive one. The caller believes a certificate was requested with a validation domain that AWS would never have accepted, leaks a certificate into state that real ACM never creates, and any error handling for InvalidDomainValidationOptionsException is dead code. Subsequent steps then operate on a resource that would not exist against AWS.
Fix: input validation in the RequestCertificate handler - for each DomainValidationOptions entry, reject with InvalidDomainValidationOptionsException (400) unless ValidationDomain equals DomainName or is a dot-boundary suffix (superdomain) of it.
Traces: acm-acm-error-paths#task#1.

## Evidence
acm-acm-error-paths#task#1 (RequestCertificate): status expected 400, got 200; $.__type missing (expected 'InvalidDomainValidationOptionsException', got None); $.message missing (expected 'The validation domain, totally-unrelated.org, must be a superdomain of the requested domain, sim2real-probe-acm-acmerr-10066c6d.example.com.', got None - quoted as an example of the documented rule, not as required text).
Smithy: RequestCertificate errors include InvalidDomainValidationOptionsException (client, httpError 400); DomainValidationOption.ValidationDomain doc: 'This must be the same as the DomainName value or a superdomain of the DomainName value.'

## Steps to Reproduce
1. Replay the ACM wire corpus against the emulator.
2. Call RequestCertificate with DomainName=<some>.example.com, ValidationMethod=EMAIL and DomainValidationOptions=[{DomainName: <some>.example.com, ValidationDomain: totally-unrelated.org}].
3. The emulator returns HTTP 200 with a CertificateArn; AWS returns HTTP 400 InvalidDomainValidationOptionsException (acm-acm-error-paths#task#1).
