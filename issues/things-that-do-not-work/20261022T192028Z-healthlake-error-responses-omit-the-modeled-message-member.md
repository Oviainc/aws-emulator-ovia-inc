Issue-Source: trace-sweep-divergence
Issue-Trace: healthlake-fhir-datastore-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: healthlake-fhir-datastore-errors healthlake-resource-tagging-errors
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|CreateFHIRDatastore|4xx->4xx|$.Message
Issue-Key: healthlake|DescribeFHIRDatastore|4xx->4xx|$.Message
Issue-Key: healthlake|DescribeFHIRDatastore|4xx->4xx|status
Issue-Key: healthlake|DeleteFHIRDatastore|4xx->4xx|$.Message
Issue-Key: healthlake|DeleteFHIRDatastore|4xx->4xx|status
Issue-Key: healthlake|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: healthlake|TagResource|4xx->4xx|$.Message
Issue-Key: healthlake|TagResource|4xx->4xx|status
Issue-Key: healthlake|ListTagsForResource|4xx->4xx|status
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# HealthLake error responses omit the modeled Message member and answer not-found with 404 instead of the recorded 400 (6 causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFHIRDatastore

## Description
Replay of the HealthLake wire fixtures (simparity-wire-aws-v1) scored 0/8 steps passed: every error-path step of the control-plane and tagging fixtures diverges. Two emulator defects produce all six divergence clusters: (a) the error body the emulator serializes for 400 client errors has no `Message` member at all, and (b) the datastore/resource not-found path answers HTTP 404 where the recorded service answers HTTP 400 (with the fixed service text). All six causes must be closed before submitting.

NOTE on adjacent open issues: the open behavioral issue "HealthLake control-plane error semantics diverge: not-found and enum-validation messages are emulator-authored..." (20261022T185152Z-healthlake-control-plane-error-semantics-diverge-not-found.md) covers the WORDING of the not-found and enum-validation messages only. It does not carry any of these wire replay keys and says nothing about the HTTP status of the not-found path or about the `Message` member being absent from the serialized 400 body, so those requirements are recorded here. The three open `healthlake ... stub` issues cover unrelated unimplemented operations.

Cause 1 of 6: CreateFHIRDatastore enum-validation 400 carries no Message member. Fixture healthlake-fhir-datastore-errors, step task#1.
AWS: Smithy: CreateFHIRDatastoreRequest.DatastoreTypeVersion is required and targets the FHIRVersion enum; CreateFHIRDatastore declares ValidationException (errorKind client, httpError 400) whose single member is `Message` (HealthLakeString, length 0..10000). Recorded behavior: an out-of-set DatastoreTypeVersion is refused with HTTP 400 and a populated Message naming the member and the permitted value set, e.g. "1 validation error detected: Value at 'datastoreTypeVersion' failed to satisfy constraint: Member must satisfy enum value set: [R4]" (text is an example of the member+constraint rule, not a literal requirement).
Emulator: status 400 is correct, but `$.Message` is absent from the response body (replay detail: "$.Message: missing (expected ... , got None)"). The modeled error member is simply not serialized.
Why: an SDK client reads ValidationException.Message to learn WHICH member it got wrong; with the member absent the exception surfaces with an empty/None message, so callers, logs and retry heuristics have nothing to act on and cannot distinguish one validation failure from another.
Fix: the validation/error-serialization path for CreateFHIRDatastore must populate the modeled `Message` member on the ValidationException it renders (the operation's validation path in the healthlake crate's ops module plus the crate's shared error-rendering helper).
Traces: healthlake-fhir-datastore-errors#task#1.

Cause 2 of 6: DescribeFHIRDatastore not-found answers 404 instead of the recorded 400, with an emulator-authored message. Fixture healthlake-fhir-datastore-errors, step task#2.
AWS: Smithy: DescribeFHIRDatastore declares ResourceNotFoundException (errorKind client, doc "The requested data store was not found.", single `Message` member). Recorded behavior: a well-formed but never-created DatastoreId is refused with HTTP 400 and the fixed text "Could not find specified datastore." — a constant service text that does not quote the requested id. (The on-disk model carries httpError 404 for that shape, but the recording is what the service puts on the wire for this operation, so 400 is the requirement here. Hypothesis: the real front end renders this error with the protocol's default client status rather than the modeled override.)
Emulator: HTTP 404, and Message is built from the request, e.g. "Datastore with ID 00000000000000000000000000000000 does not exist" (recorded example; the id varies).
Why: a client branching on the HTTP status — proxies, SDK retry/error classifiers, and any code distinguishing 4xx classes — sees a different class of response than AWS produces, and the message echoes the caller's id where the service deliberately does not.
Fix: the not-found branch of DescribeFHIRDatastore's handler / the crate's error mapping must emit the recorded status for ResourceNotFoundException on this operation and the service's fixed not-found text instead of formatting the requested id into it.
Traces: healthlake-fhir-datastore-errors#task#2.

Cause 3 of 6: DeleteFHIRDatastore not-found answers 404 instead of the recorded 400, with an emulator-authored message. Fixture healthlake-fhir-datastore-errors, step task#4.
AWS: Smithy: DeleteFHIRDatastore declares ResourceNotFoundException (client error, single `Message`). Recorded behavior: deleting a never-created DatastoreId is refused with HTTP 400 and the same fixed text "Could not find specified datastore.".
Emulator: HTTP 404 with "Datastore with ID 00000000000000000000000000000000 does not exist" (example).
Why: same as Cause 2 — delete-if-exists flows that key off the HTTP status mis-classify the refusal, and the response leaks the requested id.
Fix: DeleteFHIRDatastore's not-found branch must use the same corrected status and fixed text as Cause 2 (shared not-found error construction in the healthlake crate).
Traces: healthlake-fhir-datastore-errors#task#4.

Cause 4 of 6: ListTagsForResource malformed-ARN 400 carries no Message member. Fixture healthlake-resource-tagging-errors, step task#1.
AWS: Smithy: ListTagsForResourceRequest.ResourceARN is required, length 1..1011, pattern ^arn:aws(...)?:healthlake:[a-z0-9-]+:\\d{12}:(datastore/fhir/.{32}|dataTransformationProfile/[a-f0-9]{32})$; the operation declares ValidationException (httpError 400) with a single `Message` member. Recorded behavior (message is recorded behavior only): a value that is not an ARN at all is refused with HTTP 400 and a populated Message built from the request, e.g. "com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': not-a-valid-arn" — the requirement is that the modeled Message member is present and describes the ARN-syntax failure.
Emulator: status 400 is correct but `$.Message` is missing from the body (replay detail: "$.Message: missing ... got None").
Why: a caller that passed a bad ARN gets an exception with no message, so neither a human nor an automated handler can tell a syntax rejection from any other 400.
Fix: the ResourceARN validation path for the tagging operations must populate `Message` on the ValidationException it renders (same error-rendering helper as Cause 1).
Traces: healthlake-resource-tagging-errors#task#1.

Cause 5 of 6: TagResource on an unknown datastore ARN answers 404 instead of the recorded 400, with an emulator-authored message. Fixture healthlake-resource-tagging-errors, step task#2.
AWS: Smithy: TagResource declares ResourceNotFoundException (client error, single `Message`) and ValidationException. Recorded behavior: a syntactically valid datastore ARN that names no existing datastore is refused with HTTP 400 and the fixed text "Could not find specified datastore.".
Emulator: HTTP 404 with a message built from the request, e.g. "Resource arn:aws:healthlake:us-west-2:000000000000:datastore/fhir/00000000000000000000000000000000 does not exist" (example).
Why: tagging code that distinguishes "resource gone" by status class mis-classifies the refusal, and the body echoes the caller's ARN where the service returns a constant sentence.
Fix: TagResource's resource-lookup failure path must return the recorded status and the service's fixed datastore not-found text (shared not-found error construction).
Traces: healthlake-resource-tagging-errors#task#2.

Cause 6 of 6: ListTagsForResource on an unknown datastore ARN answers 404 instead of the recorded 400, with an emulator-authored message. Fixture healthlake-resource-tagging-errors, step task#4.
AWS: Smithy: ListTagsForResource declares ResourceNotFoundException (client error, single `Message`). Recorded behavior: a valid-but-unknown datastore ARN is refused with HTTP 400 and the fixed text "Could not find specified datastore.".
Emulator: HTTP 404 with "Resource arn:aws:healthlake:us-west-2:000000000000:datastore/fhir/00000000000000000000000000000000 does not exist" (example).
Why: same as Cause 5, on the read path — a client listing tags for a deleted datastore sees the wrong status class and an id-echoing message.
Fix: ListTagsForResource's resource-lookup failure path must use the same corrected status and fixed text (shared not-found error construction).
Traces: healthlake-resource-tagging-errors#task#4.

## Evidence
Replay summary (trace_set simparity-wire-aws-v1): emulator_up=true, fixtures_replayed=2, steps_total=8, steps_passed=0, steps_mismatched=8, divergence_count=6, state_reset_failures=0, bootstrap_signal=false.

Cause 1 — healthlake-fhir-datastore-errors#task#1, CreateFHIRDatastore: expected 400, got 400; $.Message: missing (expected "1 validation error detected: Value at 'datastoreTypeVersion' failed to satisfy constraint: Member must satisfy enum value set: [R4]", got None).

Cause 2 — healthlake-fhir-datastore-errors#task#2, DescribeFHIRDatastore: status expected 400, got 404; $.Message value_mismatch (expected 'Could not find specified datastore.', got 'Datastore with ID 00000000000000000000000000000000 does not exist').

Cause 3 — healthlake-fhir-datastore-errors#task#4, DeleteFHIRDatastore: status expected 400, got 404; $.Message value_mismatch (expected 'Could not find specified datastore.', got 'Datastore with ID 00000000000000000000000000000000 does not exist').

Cause 4 — healthlake-resource-tagging-errors#task#1, ListTagsForResource: expected 400, got 400; $.Message: missing (expected "com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': not-a-valid-arn", got None).

Cause 5 — healthlake-resource-tagging-errors#task#2, TagResource: status expected 400, got 404; $.Message value_mismatch (expected 'Could not find specified datastore.', got 'Resource arn:aws:healthlake:us-west-2:000000000000:datastore/fhir/00000000000000000000000000000000 does not exist').

Cause 6 — healthlake-resource-tagging-errors#task#4, ListTagsForResource: status expected 400, got 404; $.Message value_mismatch (expected 'Could not find specified datastore.', got 'Resource arn:aws:healthlake:us-west-2:000000000000:datastore/fhir/00000000000000000000000000000000 does not exist').

Model references used: healthlake ValidationException (errorKind client, httpError 400, single Message member, HealthLakeString length 0..10000) and ResourceNotFoundException (errorKind client, single Message member, doc "The requested data store was not found."); CreateFHIRDatastore.DatastoreTypeVersion -> FHIRVersion enum (required); Tag/ListTagsForResource.ResourceARN required with the healthlake ARN pattern.

## Steps to Reproduce
Replay the wire corpus: replay_wire_corpus() against the built emulator; fixtures healthlake-fhir-datastore-errors and healthlake-resource-tagging-errors. Per cause:

Cause 1 (healthlake-fhir-datastore-errors#task#1): CreateFHIRDatastore with DatastoreTypeVersion outside the FHIRVersion enum. Expect 400 with a populated Message naming 'datastoreTypeVersion' and the permitted value set; observe 400 with no Message member in the body.

Cause 2 (…#task#2): DescribeFHIRDatastore with a pattern-valid DatastoreId that was never created (e.g. 32 zeros). Expect HTTP 400, ResourceNotFoundException, Message 'Could not find specified datastore.'; observe HTTP 404 and an id-echoing message.

Cause 3 (…#task#4): DeleteFHIRDatastore with the same never-created DatastoreId. Expect HTTP 400 and the fixed not-found text; observe HTTP 404 and an id-echoing message.

Cause 4 (healthlake-resource-tagging-errors#task#1): ListTagsForResource with ResourceARN='not-a-valid-arn'. Expect 400 with a populated Message describing the ARN-syntax failure; observe 400 with no Message member.

Cause 5 (…#task#2): TagResource with a syntactically valid healthlake datastore ARN whose datastore id does not exist. Expect HTTP 400 and 'Could not find specified datastore.'; observe HTTP 404 and an ARN-echoing message.

Cause 6 (…#task#4): ListTagsForResource with that same unknown datastore ARN. Expect HTTP 400 and 'Could not find specified datastore.'; observe HTTP 404 and an ARN-echoing message.
