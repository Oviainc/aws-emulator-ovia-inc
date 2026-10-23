Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.tag-resource-nonexistent-arn-not-found b2bi.list-tags-for-resource-malformed-arn-error b2bi.test-conversion-unparsable-source-content-error b2bi.test-mapping-invalid-jsonata-template-rejected b2bi.create-transformer-idempotent-token-concurrent-dedup b2bi.create-transformer-mixed-legacy-and-new-fields-error
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.tag-resource-nonexistent-arn-not-found|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: b2bi.list-tags-for-resource-malformed-arn-error|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: b2bi.test-conversion-unparsable-source-content-error|run#3|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: b2bi.test-mapping-invalid-jsonata-template-rejected|run#6|AssertionError|assert error_code == "ValidationException", (
Issue-Key: b2bi.create-transformer-idempotent-token-concurrent-dedup|run#9|AssertionError|assert results, f"expected at least one of {racers} concurrent racers to succeed, all failed: {error_codes}"
Issue-Key: b2bi.create-transformer-mixed-legacy-and-new-fields-error|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# b2bi tagging, test-harness and transformer operations answer NotImplemented where AWS returns a modeled rejection or idempotent result (5 causes)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Five distinct b2bi operations answer 501/NotImplemented where the recorded AWS behavior is a modeled rejection or an idempotent success, so the suite's negative and idempotency classes cannot be exercised at all. Each cause below is a separate operation and must be closed before this issue is done. Where the suite already filed a bare "implement the scaffold stub" issue for the same operation (ListTagsForResource, TestMapping, CreateTransformer), that issue is named in the cause; this issue records the behavior the implementation has to produce, which the stub issue does not state.

Cause 1 of 5: TagResource is a stub.
AWS: TagResource takes ResourceARN (required, length 1..1011) and Tags (required) and models ValidationException (httpError 400) and ResourceNotFoundException (404) (Smithy: TagResource errors = InternalServer/ResourceNotFound/Throttling/Validation). Recorded behavior: tagging a well-formed b2bi profile ARN whose profile has been deleted is refused with ValidationException and stores no tag.
Emulator: every TagResource call returns the scaffold NotImplemented error instead of the operation's modeled outcome.
Why: a client cannot tag any b2bi resource, and its ValidationException handling for stale ARNs is never reached.
Fix: implement the TagResource handler (services/service-com-amazonaws-b2bi-b2bi/src/ops/tag_resource.rs) including ARN parsing/validation and the stale-ARN ValidationException path.
Traces: b2bi.tag-resource-nonexistent-arn-not-found.

Cause 2 of 5: ListTagsForResource is a stub, so the malformed-ARN rejection is unreachable.
AWS: ListTagsForResource takes ResourceARN (required, length 1..1011) and models ValidationException (400) and ResourceNotFoundException (404) (Smithy). Recorded behavior: a syntactically malformed ARN is refused with ValidationException (message example "Invalid ARN this-is-not-an-arn-at-all"), which is a distinct path from the well-formed-but-missing ARN ResourceNotFoundException.
Emulator: returns NotImplemented for every call. Related already-filed issue: "Implement b2bi ListTagsForResource: unimplemented scaffold stub" (filed by the suite for b2bi.create-profile-with-tags-visible-without-tag-resource-call); it does not state the two-way malformed-vs-missing ARN classification required here.
Why: clients cannot read tags at all, and cannot distinguish a client-side ARN syntax error (400) from a missing resource (404).
Fix: implement the ListTagsForResource handler (services/service-com-amazonaws-b2bi-b2bi/src/ops/list_tags_for_resource.rs) with ARN syntax validation -> ValidationException and unresolvable-but-well-formed -> ResourceNotFoundException.
Traces: b2bi.list-tags-for-resource-malformed-arn-error.

Cause 3 of 5: TestConversion is a stub, so unparsable source content is not rejected.
AWS: TestConversion requires source and target and models ValidationException (httpError 400) (Smithy: TestConversion errors = AccessDenied/InternalServer/ResourceNotFound/Throttling/Validation; success 200 with convertedFileContent). Recorded behavior: source content declared as JSON that is not valid JSON is refused with ValidationException.
Emulator: returns NotImplemented, so the check saw code 'NotImplemented' instead of 'ValidationException'.
Why: the whole conversion test harness is unusable, and a client cannot tell malformed input from a service failure.
Fix: implement the TestConversion handler (services/service-com-amazonaws-b2bi-b2bi/src/ops/test_conversion.rs) with source-content parsing that raises ValidationException for unparsable content.
Traces: b2bi.test-conversion-unparsable-source-content-error.

Cause 4 of 5: TestMapping is a stub, so an invalid JSONata template is not rejected.
AWS: TestMapping models ValidationException (httpError 400). Recorded behavior: a syntactically invalid JSONata mapping template is refused with ValidationException rather than being accepted or failing opaquely.
Emulator: returns NotImplemented (check observed code NotImplemented, response None). Related already-filed issue: "Implement b2bi TestMapping: unimplemented scaffold stub" (filed for b2bi.test-mapping-jsonata-identity-roundtrip); it does not state the invalid-template rejection class required here.
Why: clients validating mapping templates before deploying a transformer get no signal, so broken templates reach production transformers.
Fix: implement the TestMapping handler (services/service-com-amazonaws-b2bi-b2bi/src/ops/test_mapping.rs) with JSONata template parsing -> ValidationException on a syntax error.
Traces: b2bi.test-mapping-invalid-jsonata-template-rejected.

Cause 5 of 5: CreateTransformer is a stub, so clientToken idempotency and the legacy/new-field exclusivity rule are both unreachable.
AWS: CreateTransformer carries an @idempotencyToken clientToken and models ConflictException (httpError 409) and ValidationException (400). Recorded behavior: (a) concurrent CreateTransformer calls sharing one clientToken and identical parameters yield exactly one transformer — losing racers get ConflictException, and a replay of the same clientToken after completion returns the original transformerId instead of minting a new one; (b) a request that mixes the deprecated legacy fields (fileFormat/mappingTemplate/ediType) with the new inputConversion field is refused with ValidationException and creates nothing (recorded message example: "A Transformer must use the fields input, mapping, and output, or the fields fileFormat, ...").
Emulator: every CreateTransformer call returns NotImplemented — all 5 concurrent racers failed with 'NotImplemented', and the mixed-fields request returned 'NotImplemented' instead of 'ValidationException'. Related already-filed issue: "Implement b2bi CreateTransformer: unimplemented scaffold stub" (filed for b2bi.create-partnership-invalid-email-format-error); it does not state the idempotency/conflict or field-exclusivity semantics required here.
Why: clients that retry with a clientToken would get duplicate transformers once the operation starts answering, and misuse of the deprecated field style is accepted instead of refused.
Fix: implement the CreateTransformer handler (services/service-com-amazonaws-b2bi-b2bi/src/ops/create_transformer.rs) with clientToken dedup in the transformer state (in-flight -> ConflictException, completed -> replay the original id) and mutually-exclusive legacy/new field validation -> ValidationException.
Traces: b2bi.create-transformer-idempotent-token-concurrent-dedup, b2bi.create-transformer-mixed-legacy-and-new-fields-error.

## Evidence
Suite: simparity-behavioral-aws-v1, b2bi 0/42.

Cause 1 — b2bi.tag-resource-nonexistent-arn-not-found [mismatch]: AssertionError: expected ValidationException, got 'NotImplemented' (TagResource on a deleted profile's ARN).
Cause 2 — b2bi.list-tags-for-resource-malformed-arn-error [mismatch]: AssertionError: expected ValidationException, got 'NotImplemented' (ListTagsForResource(ResourceARN='this-is-not-an-arn-at-all'); expected message example 'Invalid ARN this-is-not-an-arn-at-all').
Cause 3 — b2bi.test-conversion-unparsable-source-content-error [mismatch]: AssertionError: expected ValidationException, got 'NotImplemented'.
Cause 4 — b2bi.test-mapping-invalid-jsonata-template-rejected [mismatch]: AssertionError: expected ValidationException for a syntactically invalid JSONata template, got NotImplemented; response=None.
Cause 5 — b2bi.create-transformer-idempotent-token-concurrent-dedup [mismatch]: AssertionError: expected at least one of 5 concurrent racers to succeed, all failed: ['NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented']; b2bi.create-transformer-mixed-legacy-and-new-fields-error [mismatch]: AssertionError: expected ValidationException, got 'NotImplemented'.

All five return the scaffold body "operation <Op> is scaffolded but not implemented in this emulator" with the NotImplemented code.

## Steps to Reproduce
1. Boot the emulator and point a botocore b2bi client at it.
2. Cause 1: create_profile(...); create a second profile and delete it; tag_resource(ResourceARN=<deleted profile ARN>, Tags=[{'Key':'k','Value':'v'}]) -> expect ValidationException (400); emulator returns NotImplemented.
3. Cause 2: list_tags_for_resource(ResourceARN='this-is-not-an-arn-at-all') -> expect ValidationException (400).
4. Cause 3: test_conversion(source={... JSON-declared but invalid JSON ...}, target={...}) -> expect ValidationException (400).
5. Cause 4: test_mapping(... invalid JSONata template ...) -> expect ValidationException (400).
6. Cause 5: fire 5 concurrent create_transformer calls with one shared clientToken and identical parameters -> expect exactly one success and ConflictException for the losers, then replay the same clientToken and expect the original transformerId; separately call create_transformer mixing fileFormat/mappingTemplate/ediType with inputConversion -> expect ValidationException.
7. Re-replay the checks listed in trace_ids.
