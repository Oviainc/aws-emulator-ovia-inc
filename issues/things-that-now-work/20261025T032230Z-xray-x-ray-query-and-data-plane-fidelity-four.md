Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.get-insight-events-unknown-id-fails xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods xray.get-trace-summaries-time-range-type-selects-start-or-end-time xray.rootcause-entity-filter-comparing-name-to-a-number-rejected xray.rootcause-entity-filter-separates-dependency-bound-from-pipeline-bound-frames xray.get-trace-summaries-number-keyword-filters-derive-from-the-submitted-span xray.encryption-config-kms-enforced-on-subsequent-segments xray.start-trace-retrieval-unknown-destination-not-found xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.get-insight-events-unknown-id-fails|run#9|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: xray|GetTimeSeriesServiceStatistics|InvalidRequestException
Issue-Key: xray|GetTraceSummaries|InvalidRequestException
Issue-Key: xray.get-trace-summaries-number-keyword-filters-derive-from-the-submitted-span|run#35|AssertionError|assert summary["HasFault"] is False, (
Issue-Key: xray.encryption-config-kms-enforced-on-subsequent-segments|run#16|AssertionError|assert cfg["KeyId"] == key_arn, f"expected KeyId to resolve to the created key's ARN {key_arn!r}, got {cfg['KeyId']!r}"
Issue-Key: xray.start-trace-retrieval-unknown-destination-not-found|run#13|AssertionError|assert code == "InvalidRequestException", f"unexpected error code: {code}"
Issue-Key: xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id|run#26|AssertionError|assert callee["State"] == "passive", (
Issue-Commit: 01142ac518ef08b9ccb64fede7482782162b2994

# X-Ray query and data-plane fidelity: four surfaces refuse modeled input, derived summary/graph fields are wrong, encryption KeyId is not resolved, and CancelTraceRetrieval mis-codes a bad token (8 causes)

> _Note: this issue was identified against the code base at commit `01142ac518ef08b9ccb64fede7482782162b2994`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
Eight distinct root causes in the X-Ray service (`SERVICES = ["xray"]`, all grounded in failing behavioral checks of the xray suite). Service `xray` is bootstrapped and answering (81/127 xray checks pass), so these are fidelity defects, not a bootstrap gap. None of the four open xray issues covers any of these: 20261022T045628Z (account-config over-permissiveness) covers PutEncryptionConfig's KMS-key VALIDATION and tag/destination refusals, not the KeyId echoed back by GetEncryptionConfig; 20261022T142349Z covers group/trace-segment-destination RESPONSE fields; 20261022T031509Z covers the CloudFormation path; 20261022T013648Z covers the PutResourcePolicy stub.

Cause 1 of 8: GetInsightEvents is an unimplemented scaffold stub, so an insight read answers 501 instead of the modeled 400 refusal.
AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models GetInsightEvents with http POST /InsightEvents (success 200) and errors InvalidRequestException (client, httpError 400) and ThrottledException (429). A well-formed InsightId (satisfying the modeled pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$) that names no existing insight is refused with InvalidRequestException (recorded message, an example: 'Insight not found'), never 501 and never an empty InsightEvents list.
Emulator: the call is refused with error code `NotImplemented` — services/service-com-amazonaws-xray-awsxray/src/ops/get_insight_events.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() returns Scaffolded::not_implemented("GetInsightEvents"). NOTE: the suite did NOT mark this check ALREADY FILED (the check swallows the ClientError into its own assertion, so the stub phrase never reaches the report) and no open issue names this operation, so it is filed here.
Why: a client reading an insight's event timeline gets a 501 with an unmodeled error code; it cannot distinguish 'this insight does not exist' from 'the service is broken', and 501 is not in the operation's error set so SDK error handling for the operation never matches.
Fix: implement the handler in services/service-com-amazonaws-xray-awsxray/src/ops/get_insight_events.rs (returning InvalidRequestException for an unknown insight id) and delete the SCAFFOLDED marker.
Traces: xray.get-insight-events-unknown-id-fails.

Cause 2 of 8: GetTimeSeriesServiceStatistics hard-refuses any request carrying EntitySelectorExpression.
AWS: Smithy models GetTimeSeriesServiceStatistics.EntitySelectorExpression (EntitySelectorExpression, @length 1..500) as an ordinary optional input member; the operation answers 200 with TimeSeriesServiceStatistics bucketed by Period over [StartTime, EndTime), restricted to the entities the selector names. Its only modeled errors are InvalidRequestException (400) and ThrottledException (429) — a well-formed selector expression is not an invalid request.
Emulator: HTTP 400 InvalidRequestException with 'EntitySelectorExpression is not evaluated by this emulator yet; omit it to aggregate every completed request in [StartTime, EndTime)'. The call never returns statistics.
Why: a modeled, documented input member is unusable: every dashboard or agent that scopes service statistics to one entity gets a 400 instead of its time series, and the refusal is indistinguishable (same code) from a genuinely malformed request.
Fix: evaluate EntitySelectorExpression in the operation's handler (services/service-com-amazonaws-xray-awsxray/src/ops/get_time_series_service_statistics.rs) against the stored segment/service index, instead of refusing when the member is present.
Traces: xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods.

Cause 3 of 8: GetTraceSummaries supports only TimeRangeType=TraceId and refuses the other modeled enum members.
AWS: Smithy shape TimeRangeType is an enum with values [TraceId, Event, Service]; GetTraceSummaries.TimeRangeType targets it. All three are accepted — the member selects which timestamp the [StartTime, EndTime) window is applied to — and the operation answers 200 with TraceSummaries. The modeled errors are InvalidRequestException (400) and ThrottledException (429); a legal enum member is not an invalid request.
Emulator: HTTP 400 InvalidRequestException 'TimeRangeType `Service` is not supported by this emulator yet; only `TraceId` is'.
Why: two thirds of a modeled enum are unreachable, so a client that queries by service or event time — the normal way to find traces that FINISHED in a window — is refused outright rather than getting the differently-windowed result set.
Fix: handle every TimeRangeType member in the GetTraceSummaries handler's time-window selection (services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs).
Traces: xray.get-trace-summaries-time-range-type-selects-start-or-end-time.

Cause 4 of 8: the filter-expression evaluator does not implement rootcause entity blocks, so those filters are refused.
AWS: GetTraceSummaries.FilterExpression accepts X-Ray's filter language including the rootcause keywords with an entity block, e.g. `rootcause.responsetime.entity { name = "X" }`; a well-formed expression is evaluated and answers 200 with the matching subset, while a type-incorrect comparison inside the block (comparing an entity name to a number) is refused with InvalidRequestException (400) by the expression PARSER. Recorded behavior only for the grammar; Smithy models FilterExpression as a plain string with only InvalidRequestException/ThrottledException on the operation.
Emulator: both the well-formed and the type-incorrect expression are refused identically with HTTP 400 InvalidRequestException 'Invalid filter expression: `rootcause.responsetime.entity` with a `{ … }` block is not evaluated by this emulator yet'. The emulator therefore cannot answer a root-cause-scoped query at all, and cannot distinguish a bad expression from an unimplemented one.
Why: root-cause filtering is how a client separates dependency-bound from pipeline-bound latency; every such query fails, and because the unsupported-feature refusal reuses the validation error code, a client's retry/repair logic is misled into thinking its expression is malformed.
Fix: implement the rootcause `{ … }` entity-block production in the filter-expression parser/evaluator used by GetTraceSummaries (services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs and the shared filter-expression module it calls).
Traces: xray.rootcause-entity-filter-separates-dependency-bound-from-pipeline-bound-frames, xray.rootcause-entity-filter-comparing-name-to-a-number-rejected.

Cause 5 of 8: TraceSummary.HasFault (and the FaultRootCauses that follow it) are inferred from the root segment's HTTP status instead of the segment's explicit fault boolean.
AWS: recorded behavior (the check records a real-AWS replay, us-west-2 2026-09-17, that explicitly REFUTED status-code derivation): a segment document declaring http.response.status 500 with NO `fault` member summarises with HasFault=false and no FaultRootCauses; GetTraceSummaries takes the boolean the segment sets literally. The model's prose for TraceSummary.HasFault ('The root segment document has a 500 series error') describes the X-Ray SDK's own client-side convention, not a server-side derivation — a segment that sets "fault": true is the path that produces HasFault=true.
Emulator: for exactly that segment it returns HasFault=true and synthesises FaultRootCauses with ClientImpacting=true (observed summary: {'HasFault': True, 'HasError': False, 'Http': {'HttpStatus': 500, ...}, 'FaultRootCauses': [{'Services': [...], 'ClientImpacting': True}]}).
Why: the fault flag and the fault root causes are what alarms, console filters and agents key on; inferring them from the status code invents faults for every instrumented 5xx that the producer deliberately did not mark as a fault, and makes the `fault` member of a submitted segment unobservable.
Fix: derive HasFault/HasError/HasThrottle and the *RootCauses lists from the submitted segment's fault/error/throttle booleans in the trace-summary projection (services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs and the summary-building state code), not from http.response.status.
Traces: xray.get-trace-summaries-number-keyword-filters-derive-from-the-submitted-span.

Cause 6 of 8: the account encryption config echoes the bare KMS key id instead of resolving it to the key's ARN.
AWS: recorded behavior only (Smithy documents EncryptionConfig.KeyId merely as 'The ID of the KMS key used for encryption, if applicable.'): PutEncryptionConfig(Type=KMS, KeyId=<bare key id>) is accepted, and the config read back through GetEncryptionConfig once Status=ACTIVE reports KeyId as the FULL key ARN of that key (arn:aws:kms:<region>:<account>:key/<key-id>), whatever identifier form the request used.
Emulator: GetEncryptionConfig returns KeyId as the bare key id exactly as submitted (observed: 'b14ec8ba-5651-42cb-90d5-0a805a6b1eab' where the created key's Arn was 'arn:aws:kms:us-west-2:000000000000:key/b14ec8ba-...'), i.e. the submitted value is stored and echoed with no resolution.
Why: a client that reads the encryption config to confirm WHICH key (and which account/region) protects its trace data, or that feeds the value straight into a KMS policy or a cross-account comparison, gets an unqualified id that does not identify a key; two accounts' configs become indistinguishable.
Fix: resolve the submitted KeyId (id, alias or ARN) to the key ARN when storing the account encryption config, and serialize that ARN in GetEncryptionConfig/PutEncryptionConfig (services/service-com-amazonaws-xray-awsxray/src/ops/put_encryption_config.rs and get_encryption_config.rs, plus the encryption-config state).
Traces: xray.encryption-config-kms-enforced-on-subsequent-segments.

Cause 7 of 8: CancelTraceRetrieval answers 404 ResourceNotFoundException for a corrupted RetrievalToken where AWS answers 400 InvalidRequestException.
AWS: Smithy models CancelTraceRetrieval with errors InvalidRequestException (client, httpError 400), ResourceNotFoundException (client, httpError 404) and ThrottledException (429). Recorded behavior (the check states it was not reproducible otherwise against real AWS across repeated attempts): a RetrievalToken that is a single-character mutation of a real, just-issued token — same charset, same length — is rejected as InvalidRequestException because the token is integrity-checked BEFORE any existence lookup; the 404 path is not reached for a corrupt token.
Emulator: the same mutated token is refused with error code ResourceNotFoundException (HTTP 404) — the token is treated as a plain lookup key, so an integrity failure is reported as a missing resource.
Why: the two codes mean different things to a client — 400 says 'this token is not a token, do not retry it', 404 says 'this job is gone, the token was fine' — so retry and error-reporting logic takes the wrong branch, and a transport-corrupted token is misdiagnosed as an expired job.
Fix: integrity-check the RetrievalToken and return InvalidRequestException before the existence lookup in services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs.
Traces: xray.start-trace-retrieval-unknown-destination-not-found (fails at the CancelTraceRetrieval leg, after StartTraceRetrieval succeeded).

Cause 8 of 8: GetTraceGraph gives an inferred (never-submitted) callee node State 'unknown' instead of 'passive'.
AWS: recorded behavior only (Smithy documents Service.State as 'The service's state' with no value list): in a per-trace graph, a node X-Ray infers from a remote-namespace subsegment — a service that never submitted a segment of its own — is reported with State 'passive', which is what distinguishes it from a node that actively reported segments; its Type is 'remote'.
Emulator: that inferred callee node is reported with State 'unknown'.
Why: 'passive' vs 'unknown' is the only signal a console or agent has for 'X-Ray knows this service only through someone else's subsegment'; reporting 'unknown' makes every inferred dependency look like a node whose state could not be determined, so downstream topology reasoning (and any UI that styles passive nodes) is wrong for every inferred node.
Fix: set the inferred node's State when building the per-trace topology in services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_graph.rs (the same inference that assigns Type='remote').
Traces: xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id.

## Evidence
Cause 1: xray.get-insight-events-unknown-id-fails — `assert code == "InvalidRequestException"` -> AssertionError: expected InvalidRequestException, got NotImplemented, for get_insight_events(InsightId="12345678-1234-4123-8123-123456789012") after a successful GetInsightSummaries positive control. Expected HTTP 400 InvalidRequestException.
Cause 2: xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods — botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetTimeSeriesServiceStatistics operation: EntitySelectorExpression is not evaluated by this emulator yet; omit it to aggregate every completed request in [StartTime, EndTime). Expected HTTP 200 with TimeSeriesServiceStatistics bucketed into two periods.
Cause 3: xray.get-trace-summaries-time-range-type-selects-start-or-end-time — botocore.errorfactory.InvalidRequestException ... when calling the GetTraceSummaries operation: TimeRangeType `Service` is not supported by this emulator yet; only `TraceId` is. Expected HTTP 200.
Cause 4: xray.rootcause-entity-filter-separates-dependency-bound-from-pipeline-bound-frames and xray.rootcause-entity-filter-comparing-name-to-a-number-rejected — both: botocore.errorfactory.InvalidRequestException ... when calling the GetTraceSummaries operation: Invalid filter expression: `rootcause.responsetime.entity` with a `{ … }` block is not evaluated by this emulator yet.
Cause 5: xray.get-trace-summaries-number-keyword-filters-derive-from-the-submitted-span — AssertionError: a segment that declares http.response.status 500 but carries no explicit `fault` member must summarise with HasFault false ... got {'HasFault': True, 'HasError': False, 'Http': {'HttpStatus': 500, 'HttpMethod': 'GET', ...}, 'FaultRootCauses': [{'Services': [{...'Inferred': False}], 'ClientImpacting': True}], ...}.
Cause 6: xray.encryption-config-kms-enforced-on-subsequent-segments — AssertionError: expected KeyId to resolve to the created key's ARN 'arn:aws:kms:us-west-2:000000000000:key/b14ec8ba-5651-42cb-90d5-0a805a6b1eab', got 'b14ec8ba-5651-42cb-90d5-0a805a6b1eab' (config had already settled Status=ACTIVE, Type=KMS).
Cause 7: xray.start-trace-retrieval-unknown-destination-not-found — AssertionError: unexpected error code: ResourceNotFoundException, at `assert code == "InvalidRequestException"` for cancel_trace_retrieval(RetrievalToken=<one-character mutation of a real token>). Expected HTTP 400 InvalidRequestException.
Cause 8: xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id — AssertionError: the callee never submitted a segment of its own, so X-Ray knows it only passively, got 'unknown', at `assert callee["State"] == "passive"`.

## Steps to Reproduce
1. Boot the emulator (run_behavioral_suite builds and boots it); use a us-west-2 xray client.
2. Cause 1: xray.get_insight_events(InsightId="12345678-1234-4123-8123-123456789012") — observe error code NotImplemented instead of HTTP 400 InvalidRequestException.
3. Cause 2: put two segments ~one period apart, then xray.get_time_series_service_statistics(StartTime=..., EndTime=..., Period=60, EntitySelectorExpression='service(id(name: "<svc>"))') — observe HTTP 400 instead of 200.
4. Cause 3: xray.get_trace_summaries(StartTime=..., EndTime=..., TimeRangeType="Service") — observe HTTP 400; repeat with "Event".
5. Cause 4: xray.get_trace_summaries(..., FilterExpression='rootcause.responsetime.entity { name = "QuartzAPI" }') — observe the 'not evaluated by this emulator yet' 400; then the type-incorrect variant (name = 7) and observe the identical message.
6. Cause 5: put_trace_segments with one segment carrying "http": {"response": {"status": 500}} and NO "fault" member; poll get_trace_summaries for it — observe HasFault=true and non-empty FaultRootCauses (expected HasFault=false, no FaultRootCauses).
7. Cause 6: kms.create_key, then xray.put_encryption_config(Type="KMS", KeyId=<bare KeyId>); poll xray.get_encryption_config until Status=ACTIVE — observe EncryptionConfig.KeyId is the bare id, not the key ARN.
8. Cause 7: enable the CloudWatchLogs trace-segment destination, StartTraceRetrieval for an ingested trace id to get a token, mutate one character of it, and call xray.cancel_trace_retrieval(RetrievalToken=<mutated>) — observe 404 ResourceNotFoundException instead of 400 InvalidRequestException.
9. Cause 8: put a segment with a `namespace: remote` subsegment, then xray.get_trace_graph(TraceIds=[that trace]) — observe the inferred callee node with State 'unknown' instead of 'passive'.
10. Re-replay the nine check ids in trace_ids to confirm the fixes.
