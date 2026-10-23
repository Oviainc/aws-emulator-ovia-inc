Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-vocabulary-filter-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-vocabulary-filter-lifecycle transcribe-tag-resource-list-untag-roundtrip
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|CreateVocabularyFilter|2xx->2xx|status
Issue-Key: transcribe|UntagResource|2xx->2xx|status
Issue-Key: transcribe|DeleteVocabulary|2xx->2xx|status
Issue-Key: transcribe|DeleteVocabularyFilter|2xx->2xx|status
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Transcribe awsJson1_1 success responses use the inert REST @http status instead of 200

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVocabularyFilter

## Description
This issue bundles FOUR causes with one root cause. A fixer must close all four before submitting.

Shared root cause: Amazon Transcribe speaks awsJson1_1. smithy_query(query="service_info") reports protocols [awsJson1_1], and every operation's http binding comes back flagged httpBindingsInert ("RPC protocol (awsJson/awsQuery/ec2Query/rpcv2Cbor): @http/httpLabel/httpQuery/httpHeader are ignored on the wire"). A successful awsJson1_1 response is therefore always HTTP 200 — which is exactly what the recorded fixtures show. The emulator instead publishes each operation's REST-shaped modeled code as the success status: see services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs line 160, `const SUCCESS_STATUS: u16 = 204u16;`, and lines 82-93 where the output schema is built `.with_http(HttpTrait::new("DELETE", "/vocabularies/{VocabularyName}", Some(204u16)))` — the file's own comment states "Remove it and the status falls back to the protocol default, so a modelled 204 answers 200", i.e. the REST code was opted into deliberately and is wrong for this protocol.

Shared Fix: the response serializer / `SUCCESS_STATUS` + `with_http(...)` opt-in on the output schemas of these four operations in services/service-com-amazonaws-transcribe-transcribe/src/ops/, so an awsJson1_1 success answers 200 regardless of the model's @http code. The response BODY is correct in all four cases; only the status line diverges.

Cause 1 of 4: CreateVocabularyFilter.
AWS: HTTP 200 with the CreateVocabularyFilterResponse JSON body (recorded). Smithy: CreateVocabularyFilter http {method POST, uri /vocabularyFilters/{VocabularyFilterName}, code 201}, httpBindingsInert true.
Emulator: HTTP 201.
Why: a client that gates on `status == 200` (or on a 200-only success predicate in a hand-rolled or proxy layer) treats a successful create as unhandled, and any cache/gateway in front keys 201 differently from the 200 AWS returns.
Fix: CreateVocabularyFilter's success status / output-schema http opt-in.
Traces: transcribe-vocabulary-filter-lifecycle#task#2.

Cause 2 of 4: UntagResource.
AWS: HTTP 200 with an empty JSON body (recorded). Smithy: UntagResource http code 204, httpBindingsInert true.
Emulator: HTTP 204.
Why: 204 means 'no content', so a client or proxy skips body parsing entirely and an awsJson1_1 SDK that expects a 200 JSON envelope on success mis-handles the response.
Fix: UntagResource's success status / output-schema http opt-in.
Traces: transcribe-tag-resource-list-untag-roundtrip#task#4.

Cause 3 of 4: DeleteVocabulary.
AWS: HTTP 200 with an empty JSON body (recorded). Smithy: DeleteVocabulary http code 204, httpBindingsInert true; output shape Unit.
Emulator: HTTP 204 (src/ops/delete_vocabulary.rs line 160).
Why: as cause 2; a teardown step asserting 200 on a successful delete fails against the emulator.
Fix: DeleteVocabulary's success status / output-schema http opt-in.
Traces: transcribe-tag-resource-list-untag-roundtrip#teardown#7.

Cause 4 of 4: DeleteVocabularyFilter.
AWS: HTTP 200 with an empty JSON body (recorded). Smithy: DeleteVocabularyFilter http code 204, httpBindingsInert true; output shape Unit.
Emulator: HTTP 204.
Why: as cause 3, for the filter family.
Fix: DeleteVocabularyFilter's success status / output-schema http opt-in.
Traces: transcribe-vocabulary-filter-lifecycle#teardown#8.

## Evidence
Replay of trace set simparity-wire-aws-v1, 4 divergence clusters, status-only (bodies matched):
- CreateVocabularyFilter (transcribe-vocabulary-filter-lifecycle#task#2): expected 200, got 201.
- UntagResource (transcribe-tag-resource-list-untag-roundtrip#task#4): expected 200, got 204.
- DeleteVocabulary (transcribe-tag-resource-list-untag-roundtrip#teardown#7): expected 200, got 204.
- DeleteVocabularyFilter (transcribe-vocabulary-filter-lifecycle#teardown#8): expected 200, got 204.
Model: protocols [awsJson1_1]; each of these operations reports httpBindingsInert true with modeled codes 201/204/204/204. Code: services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs:160 `const SUCCESS_STATUS: u16 = 204u16;` and :89-93 `.with_http(... Some(204u16))`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture transcribe-vocabulary-filter-lifecycle step task#2: CreateVocabularyFilter with a fresh name and Words — the response line is HTTP 201, not the HTTP 200 awsJson1_1 requires.
3. Fixture transcribe-tag-resource-list-untag-roundtrip step task#4: UntagResource on the tagged resource — HTTP 204 instead of 200.
4. Same fixture, teardown#7: DeleteVocabulary on the vocabulary created by the fixture — HTTP 204 instead of 200.
5. Fixture transcribe-vocabulary-filter-lifecycle teardown#8: DeleteVocabularyFilter — HTTP 204 instead of 200.
