Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.delete-metric-stream-removes-config-permanently cloudwatch.metric-streams-batch-tolerates-unknown-name cloudwatch.start-metric-streams-all-or-nothing-rejected cloudwatch.start-metric-streams-invalid-name-rejects-entire-batch cloudwatch.start-metric-streams-partial-invalid-name-is-silently-ignored
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.delete-metric-stream-removes-config-permanently|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudwatch.metric-streams-batch-tolerates-unknown-name|run#9|AssertionError|assert _error(lambda: cw.get_metric_stream(Name=bogus_name)) == ("ResourceNotFoundException", ""), (
Issue-Key: cloudwatch.start-metric-streams-all-or-nothing-rejected|run#12|AssertionError|assert pre_message == "", pre_message
Issue-Key: cloudwatch.start-metric-streams-invalid-name-rejects-entire-batch|run#13|AssertionError|assert bogus_message == "", bogus_message
Issue-Key: cloudwatch.start-metric-streams-partial-invalid-name-is-silently-ignored|run#14|AssertionError|assert bogus_message == "", bogus_message
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# GetMetricStream's ResourceNotFoundException carries an emulator-authored Message where AWS sends an empty one, failing 5 more metric-stream checks

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStream

## Description
One net-new root cause, grounded in 5 failing behavioral checks that the replay report marks NEITHER "ALREADY FILED" nor "OWNED by".

Cause 1 of 1: GetMetricStream's not-found refusal invents a Message for an error class whose contract is an EMPTY message.
Failing check ids: cloudwatch.delete-metric-stream-removes-config-permanently, cloudwatch.metric-streams-batch-tolerates-unknown-name, cloudwatch.start-metric-streams-all-or-nothing-rejected, cloudwatch.start-metric-streams-invalid-name-rejects-entire-batch, cloudwatch.start-metric-streams-partial-invalid-name-is-silently-ignored.

AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801) models GetMetricStream with a single required input member Name (target MetricStreamName, @length min 1 max 255) and the errors InternalServiceFault (server, httpError 500, awsQueryError InternalServiceError), InvalidParameterCombinationException (client, 400, awsQueryError InvalidParameterCombination), InvalidParameterValueException (client, 400, awsQueryError InvalidParameterValue), MissingRequiredParameterException (client, 400, awsQueryError MissingParameter) and ResourceNotFoundException (client, httpError 404, awsQueryError code ResourceNotFoundException). The model documents no text for that error's `message` member. Recorded behavior: GetMetricStream naming a stream that does not exist (never created, or deleted by DeleteMetricStream) answers HTTP 404 with Error.Code 'ResourceNotFoundException' and Error.Message EMPTY (""). There is no fixed text and there are no variable parts for this class — the requirement is the ABSENCE of a message, so the error code is the entire contract. The sibling StartMetricStreams / StopMetricStreams operations (Names required, no ResourceNotFoundException modeled at all) confirm the shape of the surrounding contract: an unknown name in a batch is silently ignored and is NEVER materialized into a listable stream, so the only way a client can prove the name stayed unknown is that GetMetricStream's 404 payload is byte-identical to AWS's.

Emulator: the error CODE is right (ResourceNotFoundException) and every preceding leg of all five checks passes — PutMetricStream is accepted with fabricated Firehose/role ARNs, the stream reaches State='running', StopMetricStreams leaves the config retrievable with State='stopped', DeleteMetricStream is a clean no-op, and the mixed-batch Start/Stop calls return 200. The divergence is purely the payload: the refusal carries an emulator-authored sentence of the form 'the metric stream <Name> does not exist' — fixed emulator text "the metric stream " + " does not exist", with the stream name substituted from the request. (Per-run generated names observed in this replay: sim2realbehavdelstream09278ce68176, sim2real-cwms-missing-<hex>, sim2real-ms-missing-34481361ed46, sim2real-cwms-nonexistent-61bb95b78ead, sim2real-cw-bogus-stream-d58bca934ec8 — those suffixes are per-run values, not part of any requirement.)

Why: a client that exact-matches AWS's error payload, or that branches on "message present" vs "message absent" to distinguish an explained refusal from a bare not-found, diverges on an otherwise correct 404. Worse, the string is load-bearing for the metric-stream batch contract: all five checks use the empty-message 404 as the proof that an unknown name named in a Start/Stop batch was NOT fabricated into a real stream. Because the assertion aborts each check, the whole tail of each is left ungraded — the name-reuse-after-delete leg, the mixed-batch "valid name still transitions" legs, the idempotent solo Stop leg and the StartMetricStreams-is-not-all-or-nothing legs never run. Note that CloudWatch does NOT always explain: GetDashboard's ResourceNotFound carries 'Dashboard <DashboardName> does not exist' and DeleteAnomalyDetector's carries 'Anomaly detector not found', so "always emit an explanatory message" is exactly the wrong generalization for this service.

Fix: the GetMetricStream handler's not-found path in the cloudwatch crate (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) — construct ResourceNotFoundException with HTTP 404 and NO message body when the named stream is absent from the metric-stream store. Two already-open issues describe the same defect but do NOT cover these five checks and so must not be treated as covering them: 20261025T020208Z-cloudwatch-firehose-s3-getmetricstream-s-not-found-error-carries-a-message.md (filed under the cloudwatch__firehose__s3 service token, grounded in a different check) and 20261026T055409Z-cloudwatch-not-found-refusals-carry-an-emulator-authored.md (whose Issue-Trace-Ids list only cloudwatch.resource-not-found-error-family and cloudwatch.double-delete-returns-not-found-not-success). None of the five check ids below appears in either, and the report marked none of them owned, so they are recorded here so the whole metric-stream batch family is re-replayed when the handler is fixed.

Traces: cloudwatch.delete-metric-stream-removes-config-permanently (check line 47), cloudwatch.metric-streams-batch-tolerates-unknown-name (line 33), cloudwatch.start-metric-streams-all-or-nothing-rejected (line 57), cloudwatch.start-metric-streams-invalid-name-rejects-entire-batch (line 53), cloudwatch.start-metric-streams-partial-invalid-name-is-silently-ignored (line 53).

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: 125/412 passed (30.3%); cloudwatch 117/254 (46%). All five check ids are [mismatch] in the failing list and NONE is marked "ALREADY FILED" or "OWNED by".

Expected (real AWS, all five): GetMetricStream Name=<absent-name> -> HTTP 404, Error.Code='ResourceNotFoundException', Error.Message='' (empty).
Actual (emulator, all five): Error.Code='ResourceNotFoundException' (correct) with Error.Message='the metric stream <Name> does not exist'.

1. cloudwatch.delete-metric-stream-removes-config-permanently, check line 47:
   AssertionError: expected message '', got 'the metric stream sim2realbehavdelstream09278ce68176 does not exist'
   Passed first: PutMetricStream (OutputFormat=json, Tags) accepted; StopMetricStreams left the config retrievable with State='stopped'; DeleteMetricStream succeeded; GetMetricStream then raised the correct code.
   Never graded as a result: the "the freed name can be reused immediately by a fresh PutMetricStream" leg (not soft-deleted).

2. cloudwatch.metric-streams-batch-tolerates-unknown-name, check line 33:
   assert _error(lambda: cw.get_metric_stream(Name=bogus_name)) == ("ResourceNotFoundException", "") ->
   AssertionError: "an unknown name named in a Stop batch must not be fabricated into a real, listable stream"
   Passed first: StopMetricStreams(Names=[real, bogus]) returned 200 and the real stream polled to State='stopped'.
   Never graded as a result: the idempotent solo StopMetricStreams leg and the StartMetricStreams mixed-batch leg.

3. cloudwatch.start-metric-streams-all-or-nothing-rejected, check line 57:
   assert pre_message == "" -> AssertionError: the metric stream sim2real-ms-missing-34481361ed46 does not exist
   Passed first: PutMetricStream accepted; explicit StopMetricStreams polled the stream to baseline State='stopped'.
   Never graded as a result: the check's reconciled core claim — StartMetricStreams is NOT all-or-nothing for nonexistent names; the call succeeds, the real stream transitions to 'running', and the unknown name is silently ignored (AWS's documented all-or-nothing guarantee is scoped to permission failures only).

4. cloudwatch.start-metric-streams-invalid-name-rejects-entire-batch, check line 53:
   assert bogus_message == "" -> AssertionError: the metric stream sim2real-cwms-nonexistent-61bb95b78ead does not exist

5. cloudwatch.start-metric-streams-partial-invalid-name-is-silently-ignored, check line 53:
   assert bogus_message == "" -> AssertionError: the metric stream sim2real-cw-bogus-stream-d58bca934ec8 does not exist

Message anatomy: the emulator's fixed text is "the metric stream " ... " does not exist"; the single variable part is the Name taken verbatim from the request. AWS's requirement for ResourceNotFoundException on GetMetricStream has NO fixed text at all.

## Steps to Reproduce
A. Deleted stream (cloudwatch.delete-metric-stream-removes-config-permanently):
1. cloudwatch:PutMetricStream Name=<n>, FirehoseArn=arn:aws:firehose:<region>:<account>:deliverystream/<n>, RoleArn=arn:aws:iam::<account>:role/<n>, OutputFormat=json (the ARNs need not resolve).
2. cloudwatch:StopMetricStreams Names=[<n>]; cloudwatch:GetMetricStream Name=<n> must still return the config with State='stopped'.
3. cloudwatch:DeleteMetricStream Name=<n>.
4. cloudwatch:GetMetricStream Name=<n>.
   Expected: HTTP 404, Error.Code='ResourceNotFoundException', Error.Message='' (empty/absent).
   Observed: Error.Code='ResourceNotFoundException' with Error.Message='the metric stream <n> does not exist'.
5. Then cloudwatch:PutMetricStream with the SAME Name=<n> must succeed (the name is genuinely freed).

B. Unknown name in a Start/Stop batch (the other four checks):
1. cloudwatch:PutMetricStream Name=<real> as above; pick <bogus> as a unique name that was never created.
2. cloudwatch:StopMetricStreams Names=[<real>, <bogus>] -> expect HTTP 200; poll cloudwatch:GetMetricStream Name=<real> to State='stopped'.
3. cloudwatch:GetMetricStream Name=<bogus>.
   Expected: HTTP 404, Error.Code='ResourceNotFoundException', Error.Message='' (empty) — this empty payload is the proof the bogus name was never materialized.
   Observed: the same code with Error.Message='the metric stream <bogus> does not exist'.
4. cloudwatch:StartMetricStreams Names=[<real>, <bogus>] -> expect HTTP 200 (NOT a rejection); poll GetMetricStream Name=<real> to State='running'; GetMetricStream Name=<bogus> must again be a 404 with an empty message.

Contrast legs that must KEEP their text when fixing: GetDashboard on a deleted dashboard -> ResourceNotFound + 'Dashboard <DashboardName> does not exist'; a second DeleteAnomalyDetector -> ResourceNotFoundException + 'Anomaly detector not found'.

Re-replay all five check ids in trace_ids to confirm the fix.
