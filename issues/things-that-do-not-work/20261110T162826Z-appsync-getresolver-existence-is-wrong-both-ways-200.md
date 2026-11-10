Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-cfg-resolver-batch-and-caching-rules
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-cfg-resolver-batch-and-caching-rules appsync-datasource-function-resolver-pipeline appsync-unit-resolver-code-evaluation appsync-resolver-mapping-template-unicode-round-trip
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetResolver|4xx->2xx|$.message
Issue-Key: appsync|GetResolver|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|GetResolver|4xx->2xx|status
Issue-Key: appsync|GetResolver|2xx->4xx|$.resolver
Issue-Key: appsync|GetResolver|2xx->4xx|status
Issue-Commit: e88affa0c81a58fb8423ccb328f6804dccd04335

# AppSync GetResolver existence is wrong both ways: 200 for an absent resolver, 404 for one AWS serves (2 causes)

> _Note: this issue was identified against the code base at commit `e88affa0c81a58fb8423ccb328f6804dccd04335`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResolver

## Description
Two net-new causes, one shared fault: GetResolver's existence decision does not track the resolvers the emulator actually holds — it serves a resolver that is gone and refuses one that is present.

Cause 1 of 2 — GetResolver answers 200 for a resolver that no longer exists.
AWS: Smithy: GetResolver (GET /v1/apis/{apiId}/types/{typeName}/resolvers/{fieldName}) models NotFoundException (httpError 404), documented "The resource specified in the request was not found". After DeleteResolver (or for a field that never had a resolver) AWS answers 404 with x-amzn-errortype NotFoundException and message 'No resolver found.' — wholly fixed text, no request-derived parts.
Emulator: answers 200 with a resolver body; $.message and x-amzn-errortype are absent.
Why: a client cannot detect a deleted resolver — delete/recreate flows, existence probes and "create if absent" logic all take the wrong branch, and the 404 error path of every SDK wrapper is never reached.
Fix: GetResolver's handler/state lookup — a miss (including after DeleteResolver) must raise NotFoundException with message 'No resolver found.' instead of returning a stale or synthesised record. Related open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md covers not-found/bad-request class inversion elsewhere and does not own this step.
Traces: appsync-cfg-resolver-batch-and-caching-rules#task#10 (same cluster: appsync-datasource-function-resolver-pipeline#task#22, appsync-unit-resolver-code-evaluation#task#19).

Cause 2 of 2 — GetResolver answers 404 for a resolver AWS serves, when its requestMappingTemplate is a multi-line template containing non-ASCII characters.
AWS: Smithy: requestMappingTemplate targets MappingTemplate (@length 1..65536, @pattern "^.*$"); VTL templates are arbitrary UTF-8 text and routinely contain newlines and non-ASCII comment text, so CreateResolver accepts such a template and GetResolver returns 200 with the Resolver and the template round-tripped byte for byte (the recording shows a '## comment:' line with accented, CJK, emoji, quoted and backslash characters followed by a multi-line JSON body).
Emulator: GetResolver answers 404 and $.resolver is absent, so the resolver is not retrievable at all. Hypothesis: the create of this resolver in the same fixture (task#4) was itself refused by the emulator, leaving nothing to read; that refusal is owned by 20261029T200818Z-appsync-resolver-function-data-source-plane-dependency-refusals.md and the multi-line @pattern handling is described in 20261109T020601Z-appsync-rejects-every-multi-line-vtl-mapping-template.md — neither owns this GetResolver step, and the requirement it pins is that after the create is accepted GetResolver must answer 200 with the UTF-8 template preserved exactly (newlines, multi-byte characters, escaped quotes and backslashes).
Why: real resolver templates are multi-line and often non-ASCII; a client that creates one and then reads it back gets 404, so it concludes the resolver does not exist and re-creates or aborts the deployment, and no template round-trip can be verified.
Fix: CreateResolver's MappingTemplate validation (multiline/UTF-8-safe pattern application) plus the resolver state write and GetResolver's lookup, so a created resolver is found and its template is returned unmodified.
Traces: appsync-resolver-mapping-template-unicode-round-trip#task#5 (same cluster: #task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1:
- appsync-cfg-resolver-batch-and-caching-rules#task#10 GetResolver: expected 404, got 200; $.message missing (expected 'No resolver found.'); $headers.x-amzn-errortype missing. Cluster also covers appsync-datasource-function-resolver-pipeline#task#22 and appsync-unit-resolver-code-evaluation#task#19.
- appsync-resolver-mapping-template-unicode-round-trip#task#5 GetResolver: expected 200, got 404; $.resolver missing — the recorded resolver is kind UNIT on Query/echo with requestMappingTemplate '## comment: héllo 世界 🎉 "quoted" \\backslash\\ Ω≈ç√' followed by a multi-line 2018-05-29 Invoke body, responseMappingTemplate '$util.toJson($context.result)'. Cluster also covers #task#7.

## Steps to Reproduce
Cause 1: CreateGraphqlApi, CreateResolver on Query/<field>, DeleteResolver for it, then GetResolver on the same apiId/typeName/fieldName — the emulator answers 200 with a resolver body instead of 404 NotFoundException 'No resolver found.'.
Cause 2: CreateGraphqlApi and a data source, CreateResolver on Query/echo whose requestMappingTemplate is a multi-line VTL template beginning with a '##' comment containing non-ASCII characters (accented, CJK, emoji, escaped quotes, backslashes), then GetResolver for Query/echo — the emulator answers 404 with no resolver instead of 200 with the template returned byte for byte.
Replay: appsync-cfg-resolver-batch-and-caching-rules#task#10, appsync-resolver-mapping-template-unicode-round-trip#task#5.
