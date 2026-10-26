Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.tag-resource-unicode-value-round-trip
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|TagResource|ValidationException
Issue-Commit: 9c68a0ec46013b79f54cdf26e8927f90f40b555f

# TagResource's constraint-violation ValidationException echoes the offending tag value instead of ACM's "Value of the input at '<path>'" form (1 cause)

> _Note: this issue was identified against the code base at commit `9c68a0ec46013b79f54cdf26e8927f90f40b555f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause in ACM's request-validation message builder, grounded in a behavioral check that passes on real AWS.

Cause 1 of 1: the ValidationException raised for a tag value that violates the modelled TagValue pattern is built with the request's value interpolated into the sentence, instead of ACM's value-free "Value of the input at '<member path>'" form.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) TagResource(ResourceArn, Tags: TagList) with Tag.Value -> TagValue, a string with @length max 256 and @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$; the operation declares ValidationException (HTTP 400). The message is produced by the standard constraint-violation rule, which is built from the request and the model: the count of violations, the JSON-pointer-style path of the offending member, and the violated constraint - and for this service's tag members AWS does NOT echo the supplied value, it writes the literal phrase "Value of the input at": "1 validation error detected: Value of the input at 'tags.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)". The same value-free form is recorded on the sibling tag operation of this service (UntagResource: "Value of the input at 'tagKeys' ..."), so the form is the rule for the ACM tag surface rather than one recording's text.

Emulator: the HTTP status (400) and Error.Code (ValidationException) are correct, and the rejection is clean (no partial or sanitized tag is left behind, and the in-charset unicode value round-trips byte-for-byte), but the message interpolates the rejected value: "1 validation error detected: Value 'rocket-\U0001F680-emoji' at 'tags.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)". The path and the constraint clause match; only the value-echoing prefix diverges.

Why: the rule that produces the sentence - not the recorded text - is the requirement here, and the emulator is building the envelope a different way: it emits a template AWS does not use for this surface and leaks the caller-supplied value back into the error string. Clients and fixtures that match ACM's constraint report diverge on every out-of-charset tag value, and a value that is itself non-renderable (an emoji, a control sequence) ends up echoed into logs and error surfaces where AWS deliberately omits it.

Fix: the shared constraint-violation message builder behind this crate's ValidationException, as reached from the TagResource handler's tag validation - emit the "Value of the input at '<path>'" prefix for these tag members rather than interpolating the request value. Related but distinct: open issue 20261024T113112Z-acm-certificate-lifecycle-and-tagging-contracts-diverge-updatecertificateoptions.md Cause 6 concerns the same builder's bracketed CONSTRAINT LIST on UntagResource (only the pattern clause enumerated); that issue does not carry this check or the value-echoing prefix on TagResource, so this cause must be closed in addition to it.

Traces: acm.tag-resource-unicode-value-round-trip.

## Evidence
Cause 1: acm.tag-resource-unicode-value-round-trip - AssertionError: expected message "1 validation error detected: Value of the input at 'tags.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)", got "1 validation error detected: Value 'rocket-\U0001F680-emoji' at 'tags.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)".
Everything else in the check matched: the in-charset unicode value "caf\u00e9-\u30c6\u30b9\u30c8-\u03a9" set by TagResource came back unchanged from ListTagsForResource, the out-of-charset value was refused with Error.Code == 'ValidationException' at HTTP 400, and ListTagsForResource afterwards showed no partial/sanitized 'sim2real-bad-tag' entry - so the only divergence is the message built by the validation envelope.
Corroborating recording of the same value-free form on the sibling operation of this service: UntagResource's recorded AWS message is "1 validation error detected: Value of the input at 'tagKeys' failed to satisfy constraint: ..." (check acm.untag-resource-invalid-key-validation-rejected, owned by 20261024T113112Z-acm-certificate-lifecycle-and-tagging-contracts-diverge-updatecertificateoptions.md).
Smithy citations: Tag.Value -> TagValue (string, @length max 256, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$); Tag.Key -> TagKey (@length 1..128, same pattern); TagResource declares ValidationException (HTTP 400).

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite (check acm.tag-resource-unicode-value-round-trip).
2. acm.create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', CertificateAuthority={'PublicCertificateAuthority': {}}) and poll describe_acme_endpoint until Status is ACTIVE.
3. acm.tag_resource(ResourceArn=<endpoint arn>, Tags=[{'Key': 'sim2real-unicode', 'Value': 'caf\u00e9-\u30c6\u30b9\u30c8-\u03a9'}]) and confirm list_tags_for_resource returns the value unchanged (this part already passes).
4. acm.tag_resource(ResourceArn=<endpoint arn>, Tags=[{'Key': 'sim2real-bad-tag', 'Value': 'rocket-\U0001F680-emoji'}]) - an emoji (Unicode category So) is outside the TagValue pattern's charset.
5. Inspect Error.Message on the resulting 400 ValidationException: the emulator writes "Value 'rocket-\U0001F680-emoji' at 'tags.1.member.value' ..." where AWS writes "Value of the input at 'tags.1.member.value' ...".
6. Re-replay acm.tag-resource-unicode-value-round-trip to confirm the fix.
