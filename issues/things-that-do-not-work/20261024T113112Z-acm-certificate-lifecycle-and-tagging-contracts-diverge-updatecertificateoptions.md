Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.update-certificate-options-pending-validation-fails acm.update-certificate-options-validation-method-change-while-pending-rejected acm.certificate-transparency-logging-preference-deprecated-field-ignored acm.resend-validation-email-wrong-domain-rejected acm.request-certificate-dns-validation-token-shape acm.op-contract-removeTagsFromCertificate acm.tag-resource-quota-exceeded-rejected acm.untag-resource-invalid-key-validation-rejected acm.untag-resource-nonexistent-arn-rejected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.update-certificate-options-pending-validation-fails|run#6|AssertionError|assert code == "InvalidStateException", f"expected InvalidStateException, got {code!r}"
Issue-Key: acm|UpdateCertificateOptions|ValidationException
Issue-Key: acm|ResendValidationEmail|InvalidStateException
Issue-Key: acm.request-certificate-dns-validation-token-shape|run#22|AssertionError|assert _CNAME_VALUE_RE.match(rr["Value"]), (
Issue-Key: acm.op-contract-removeTagsFromCertificate|run#13|KeyError|assert acm.list_tags_for_certificate(CertificateArn=arn)["Tags"] == []
Issue-Key: acm.tag-resource-quota-exceeded-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: acm.untag-resource-invalid-key-validation-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: acm.untag-resource-nonexistent-arn-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# ACM certificate lifecycle and tagging contracts diverge: UpdateCertificateOptions error class, EMAIL certificates driven to FAILED, the DNS challenge target shape, and three tag-API error envelopes (7 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCertificateOptions

## Description
Seven distinct root causes in ACM's certificate state machine, DNS-challenge generator and tag-API error envelopes. Each is grounded in behavioral checks that pass on real AWS. Fix all seven before submitting.

Cause 1 of 7: UpdateCertificateOptions on a PENDING_VALIDATION certificate is refused with the wrong error class.
AWS: Smithy UpdateCertificateOptions(CertificateArn required, Options required CertificateOptions) -> Unit, errors ConflictException, InvalidArnException, InvalidStateException, LimitExceededException, ResourceNotFoundException, ValidationException (all HTTP 400). The operation may only be called once the certificate is Issued/Valid, so a certificate still in Status PENDING_VALIDATION is refused with HTTP 400 InvalidStateException; the message is the service's fixed string for that guard, 'The selected certificate is not Issued and/or Valid.'
Emulator: HTTP 400 with Error.Code 'ValidationException' (the state guard is being reported through the request-validation envelope instead of the modeled state error).
Why: InvalidStateException is retryable-after-issuance and tells a client to wait for the certificate to be issued; ValidationException tells it the REQUEST is malformed and must never be retried. CT-logging opt-out tooling therefore aborts permanently on a certificate that would have become updatable.
Fix: the UpdateCertificateOptions handler's state guard - raise the modeled InvalidStateException (not ValidationException) when the certificate is not Issued/Valid.
Traces: acm.update-certificate-options-pending-validation-fails, acm.update-certificate-options-validation-method-change-while-pending-rejected, acm.certificate-transparency-logging-preference-deprecated-field-ignored.

Cause 2 of 7: an EMAIL-validated certificate is driven to Status FAILED while it is still awaiting approval, so ResendValidationEmail is refused.
AWS: Smithy ResendValidationEmail(CertificateArn, Domain, ValidationDomain all required) -> Unit, errors InvalidArnException, InvalidDomainValidationOptionsException, InvalidStateException, ResourceNotFoundException, ValidationException (400). Recorded behavior: a certificate requested with ValidationMethod='EMAIL' stays in Status PENDING_VALIDATION with DomainValidationOptions[*].ValidationEmails populated, and ResendValidationEmail for the certificate's OWN domain answers HTTP 200.
Emulator: the certificate reaches Status FAILED, so the positive control is refused with HTTP 400 InvalidStateException, message 'The certificate arn:aws:acm:us-west-2:000000000000:certificate/6f339a6b-... has status FAILED, so it is no longer awaiting domain validation.'
Why: no email-validated certificate can ever be approved in the emulator, and the operation's whole happy path is unreachable; a client is told to abandon a request real AWS would have kept pending. Related: the open issue 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md records the same premature-FAILED transition on the DNS path (its Cause 4) - this cause is the EMAIL path of the same state machine and must be closed too.
Fix: the certificate validation state machine - keep an EMAIL-validation certificate in PENDING_VALIDATION instead of transitioning it to FAILED.
Traces: acm.resend-validation-email-wrong-domain-rejected.

Cause 3 of 7: the DNS validation CNAME target is one label short.
AWS: Smithy DomainValidation.ResourceRecord{Name, Type=CNAME, Value}. Recorded behavior: Value is '_<32 hex>.<label>.acm-validations.<domain>.' - a token label, then a per-validation label, then the acm-validations domain (check regex ^_[0-9a-f]{32}\.[a-z0-9]+\.acm-validations(?:\.[a-z0-9-]+)+\.$).
Emulator: Value is '_f4466bd685be89318b2c88a21b8a9398.acm-validations.aws.' - the middle label is missing.
Why: automation that parses the CNAME target, or publishes it and then verifies the delegation structure, rejects the record, and any renewal flow keyed on the ACM validation sub-domain shape breaks.
Fix: the validation-record generator behind RequestCertificate / the certificate state that mints ResourceRecord values. The open issue 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md records the same generator defect as its Cause 2 from two other checks; this cause carries the third check, so close them together.
Traces: acm.request-certificate-dns-validation-token-shape.

Cause 4 of 7: ListTagsForCertificate omits the Tags member when the tag set is empty.
AWS: Smithy ListTagsForCertificate -> ListTagsForCertificateResponse{Tags: TagList}. Recorded behavior: a certificate with no tags answers HTTP 200 with Tags present and empty ([]).
Emulator: HTTP 200 with no 'Tags' key at all, so the client raises KeyError: 'Tags' on the documented shape - here observed right after RemoveTagsFromCertificate removed the last tag.
Why: a tag-reconciliation loop cannot distinguish 'no tags' from 'field absent', and every client doing response['Tags'] crashes.
Fix: the ListTagsForCertificate response serializer - emit the empty list rather than omitting the member. The open issue 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md records this as its Cause 6 from two other checks; this cause carries a third, so close them together.
Traces: acm.op-contract-removeTagsFromCertificate.

Cause 5 of 7: AddTagsToCertificate's over-quota rejection carries the wrong message.
AWS: Smithy AddTagsToCertificate(CertificateArn, Tags required TagList len 1..50) declares TooManyTagsException (HTTP 400). The rule producing the text is known and fixed - exceeding the 50-user-tag-per-certificate quota yields exactly 'The resultant tag set must not have more than 50 user tags.' (a constant service string for that quota rule, not a value derived from the request).
Emulator: the status (400) and code (TooManyTagsException) are correct, but the message is 'a certificate may carry at most 50 tags; this request would leave 51' - an emulator-authored sentence that also leaks the computed resultant count.
Why: the enforcement is right, so the only divergence is the envelope text produced by a known rule; clients and fixtures that match ACM's quota message byte-for-byte diverge on every over-quota call.
Fix: the TooManyTagsException construction in the certificate-tagging path (AddTagsToCertificate).
Traces: acm.tag-resource-quota-exceeded-rejected.

Cause 6 of 7: UntagResource's constraint-violation ValidationException enumerates only one of the member's constraints.
AWS: Smithy UntagResource(ResourceArn, TagKeys required TagKeyList len 1..50) whose member TagKey is a string with length 1..128 and pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$, errors ResourceNotFoundException and ValidationException (400). The message is built from the request and the model by the standard constraint-violation rule, which lists EVERY constraint on the offending member: "1 validation error detected: Value of the input at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*), Member must not be null]".
Emulator: status 400 and code ValidationException are correct, but the bracketed constraint list contains only the pattern clause - "... Member must satisfy constraint: [Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)]" - dropping the two length clauses and the not-null clause.
Why: the rule, not the recorded text, is the requirement: the list is derived from the member's modeled constraints, so an emulator that emits a partial list is building the envelope the wrong way and no client that parses ACM's constraint report sees the same set of violated constraints.
Fix: the shared constraint-violation message builder for this service's ValidationException - enumerate all of the member's constraints (length bounds, pattern, nullability).
Traces: acm.untag-resource-invalid-key-validation-rejected.

Cause 7 of 7: the ACME-resource not-found message says 'resource' instead of naming the resource type from the ARN.
AWS: Smithy UntagResource declares ResourceNotFoundException (HTTP 400). The message is built from the request - AWS names the resource TYPE parsed out of the supplied ARN: 'Could not find acme-endpoint arn:aws:acm:<region>:<account>:acme-endpoint/<id>.'
Emulator: status 400 and code ResourceNotFoundException are correct, but the message is the generic 'Could not find resource arn:aws:acm:us-west-2:000000000000:acme-endpoint/54b65d76959d4af097345f9ba28ce2fa.' - the type token is replaced by the literal word 'resource'.
Why: the rule that produces the text is known (it is derived from the ARN in the request), so it is a requirement; tooling that distinguishes which ACM resource type was missing - and fixtures that match ACM's not-found text - diverge on every miss. Note: this check appears in the trace list of 20261022T143245Z-acm-seven-modeled-operations-answer-http-501-notimplemented.md (its Cause 3, UntagResource answering 501); UntagResource now answers the modeled error, and this is the remaining divergence in that answer.
Fix: the not-found error construction shared by TagResource/UntagResource/ListTagsForResource - derive the resource-type token from the ARN.
Traces: acm.untag-resource-nonexistent-arn-rejected.

## Evidence
Cause 1: acm.update-certificate-options-pending-validation-fails - AssertionError: expected InvalidStateException, got 'ValidationException' (certificate confirmed Status PENDING_VALIDATION immediately before the call); acm.update-certificate-options-validation-method-change-while-pending-rejected - AssertionError: ValidationException on assert err.response['Error']['Code'] == 'InvalidStateException'; acm.certificate-transparency-logging-preference-deprecated-field-ignored - AssertionError: ValidationException on the same assertion.
Cause 2: acm.resend-validation-email-wrong-domain-rejected - botocore.errorfactory.InvalidStateException: An error occurred (InvalidStateException) when calling the ResendValidationEmail operation: The certificate arn:aws:acm:us-west-2:000000000000:certificate/6f339a6b-e711-44ec-84a7-424413091962 has status FAILED, so it is no longer awaiting domain validation - raised on the POSITIVE control (resend for the certificate's own domain, which must answer 200).
Cause 3: acm.request-certificate-dns-validation-token-shape - AssertionError: dnstok9bbfa7851f8e.sim2realprobe8b297862e2.com: ResourceRecord.Value='_f4466bd685be89318b2c88a21b8a9398.acm-validations.aws.' is not of the form _<32 hex>.<label>.acm-validations.<domain>.
Cause 4: acm.op-contract-removeTagsFromCertificate - KeyError: 'Tags' on assert acm.list_tags_for_certificate(CertificateArn=arn)['Tags'] == [] after the only tag was removed.
Cause 5: acm.tag-resource-quota-exceeded-rejected - AssertionError: expected message 'The resultant tag set must not have more than 50 user tags.', got 'a certificate may carry at most 50 tags; this request would leave 51' (code TooManyTagsException matched, and the 50-tag state was correctly preserved).
Cause 6: acm.untag-resource-invalid-key-validation-rejected - AssertionError: expected message "1 validation error detected: Value of the input at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*), Member must not be null]", got "1 validation error detected: Value of the input at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)]" (TagKeys=['bad#key!'] against an ACME endpoint ARN; code ValidationException matched).
Cause 7: acm.untag-resource-nonexistent-arn-rejected - AssertionError: expected message 'Could not find acme-endpoint arn:aws:acm:us-west-2:000000000000:acme-endpoint/54b65d76959d4af097345f9ba28ce2fa.', got 'Could not find resource arn:aws:acm:us-west-2:000000000000:acme-endpoint/54b65d76959d4af097345f9ba28ce2fa.' (code ResourceNotFoundException matched).
Smithy citations (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): UpdateCertificateOptions(CertificateArn, Options) -> Unit, errors ConflictException/InvalidArnException/InvalidStateException/LimitExceededException/ResourceNotFoundException/ValidationException; ResendValidationEmail(CertificateArn, Domain, ValidationDomain) -> Unit, errors incl. InvalidStateException/ResourceNotFoundException; ListTagsForCertificate -> ListTagsForCertificateResponse{Tags: TagList len 1..50}; AddTagsToCertificate errors incl. TooManyTagsException; UntagResource(ResourceArn, TagKeys: TagKeyList len 1..50) errors ResourceNotFoundException/ValidationException; TagKey length 1..128 pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Cause 1: acm.request_certificate(DomainName=<non-reserved domain>, ValidationMethod='DNS'), confirm DescribeCertificate Status == 'PENDING_VALIDATION', then acm.update_certificate_options(CertificateArn=<arn>, Options={'CertificateTransparencyLoggingPreference': 'DISABLED'}) - Error.Code is ValidationException instead of InvalidStateException.
3. Cause 2: acm.request_certificate(DomainName=<non-reserved domain>, ValidationMethod='EMAIL'), poll DescribeCertificate until DomainValidationOptions[0].ValidationEmails is populated, then acm.resend_validation_email(CertificateArn=<arn>, Domain=<that domain>, ValidationDomain=<that domain>) - the certificate is in FAILED and the call is refused with InvalidStateException instead of answering 200.
4. Cause 3: read DomainValidationOptions[*].ResourceRecord.Value for a DNS-validated request - the target has no label between the token and 'acm-validations'.
5. Cause 4: add one tag with AddTagsToCertificate, remove it with RemoveTagsFromCertificate, then acm.list_tags_for_certificate(CertificateArn=<arn>) - the response has no 'Tags' member.
6. Cause 5: fill a certificate to exactly 50 tags, then acm.add_tags_to_certificate one more - compare Error.Message to ACM's quota string.
7. Cause 6: create an ACME endpoint and call acm.untag_resource(ResourceArn=<endpoint arn>, TagKeys=['bad#key!']) - the bracketed constraint list holds only the pattern clause.
8. Cause 7: acm.untag_resource(ResourceArn='arn:aws:acm:<region>:<account>:acme-endpoint/<random hex>', TagKeys=['x']) - the message says 'resource' instead of 'acme-endpoint'.
9. Re-replay the 9 checks listed in trace_ids.
