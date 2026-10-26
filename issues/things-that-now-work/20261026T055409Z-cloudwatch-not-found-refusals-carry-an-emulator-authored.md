Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.resource-not-found-error-family cloudwatch.double-delete-returns-not-found-not-success
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.resource-not-found-error-family|_assert_error#6|AssertionError|assert err.get("Message", "") == message, f"{op}: expected message {message!r}, got {err.get('Message', '')!r}"
Issue-Key: cloudwatch.double-delete-returns-not-found-not-success|run#29|AssertionError|assert gone_msg == "", f"unexpected GetMetricStream message {gone_msg!r}"
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# CloudWatch not-found refusals carry an emulator-authored Message where AWS sends none: SetAlarmState (ResourceNotFound) and GetMetricStream (ResourceNotFoundException) (2 causes)

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Two net-new root causes in the CloudWatch emulator's not-found refusal path. Both checks get a long way first (the alarm store, the dashboard store, the metric-stream store, the insight-rule store and the anomaly-detector store all behave, and in both checks the preceding legs — including the exact not-found CODE — pass); the divergence is purely that the emulator invents an explanatory Message for an error class where real AWS sends an EMPTY message, so the error code is the entire contract.

Neither failing check is marked "ALREADY FILED" or "OWNED by" in the replay report.

=== Cause 1 of 2: SetAlarmState's ResourceNotFound on a missing alarm carries an invented message ===
Failing check id: cloudwatch.resource-not-found-error-family.
AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801) models SetAlarmState with AlarmName (@length 1..255, required), StateValue (enum, required), StateReason (@length 0..1023, required) and StateReasonData (@length 0..4000); its modeled errors are InvalidFormatFault (client, httpError 400, awsQueryError InvalidFormat) and ResourceNotFound (client, httpError 404, awsQueryError code ResourceNotFound). The ResourceNotFound shape's `message` member carries NO documentation, and the recorded behavior for SetAlarmState against an alarm name that does not exist is HTTP 404, Error.Code 'ResourceNotFound', Error.Message EMPTY ("") — AWS emits no text at all for this class. There is no fixed text and there are no variable parts: the requirement is the absence of a message.
Emulator: the code is correct (ResourceNotFound), but the refusal carries an emulator-authored sentence of the form 'alarm <AlarmName> does not exist' — fixed text "alarm " + " does not exist" with the alarm name taken from the request (the replay shows 'alarm sim2real-behav-cwprobe-alarm-missing-0aa94bb2fe35 does not exist'; the suffix is a per-run generated name and is not part of any requirement). AssertionError in the check at line 28/71: "SetAlarmState: expected message '', got 'alarm ... does not exist'". Everything before it in that check passes, including the DeleteAlarms-is-idempotent-for-a-missing-name leg immediately above.
Why: a client (or a parity harness, or an SDK error-equality comparison) that reads Error.Message for this refusal sees text AWS never sends. Error handling that branches on "message present" vs "message absent", or that exact-matches AWS's error payload, diverges on an otherwise correct 404 — and the rest of the not-found family in this check (GetDashboard, DescribeAlarmContributors, DisassociateDatasetKmsKey, GetDataset) DOES carry a real AWS message, so "CloudWatch always explains" is the wrong generalization an emulator must not make.
Fix: the SetAlarmState handler's not-found refusal in the cloudwatch crate — construct ResourceNotFound (404) with no message body for a missing alarm. Already-open related issue that does NOT cover this check: 20261025T215433Z-cloudwatch-6-net-new-root-causes-a-setalarmstate.md describes this same SetAlarmState defect as its cause 5, but it is grounded in, and its Issue-Trace-Ids list only, cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm — this check id appears nowhere in it, and the report did not mark this check owned, so it is recorded here so the whole not-found family is re-replayed when the handler is fixed.
Traces: cloudwatch.resource-not-found-error-family.

=== Cause 2 of 2: GetMetricStream's ResourceNotFoundException on a deleted stream carries an invented message ===
Failing check id: cloudwatch.double-delete-returns-not-found-not-success.
AWS: Smithy models GetMetricStream with Name (MetricStreamName, @length 1..255, required) and errors InternalServiceFault (500), InvalidParameterCombinationException (400), InvalidParameterValueException (400), MissingRequiredParameterException (400) and ResourceNotFoundException (client, httpError 404, awsQueryError code ResourceNotFoundException). The recorded behavior for GetMetricStream naming a stream that was deleted is HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message EMPTY ("") — again no text, no fixed part, no substituted values. (The same check records the contrast explicitly: DeleteAnomalyDetector's second delete DOES carry the fixed text 'Anomaly detector not found', and GetDashboard carries 'Dashboard <DashboardName> does not exist' with the name from the request; those legs pass.)
Emulator: the code is correct (ResourceNotFoundException), but the refusal carries 'the metric stream <Name> does not exist' — fixed text "the metric stream " + " does not exist" with the stream name from the request (the replay shows 'the metric stream sim2real-behav-cwprobe-mstream-dd-d2c4d2cd4f89 does not exist'; that suffix is a per-run generated name). AssertionError at check line 142: "unexpected GetMetricStream message". Everything before it passes: PutMetricStream with a fabricated Firehose/role ARN is accepted, the stream reaches State=running, and both DeleteMetricStream calls are idempotent no-ops.
Why: the double-delete contract a client relies on is "the resource is genuinely gone, and the refusal is exactly AWS's refusal". A fabricated message makes the 404 payload non-identical to AWS's, breaking exact-match error handling, and it also blocks the rest of this check (the DeleteAlarmMuteRule, DeleteAnomalyDetector and DeleteInsightRules idempotency legs never run), so an entire family of delete semantics is left unverified behind one wrong string.
Fix: the GetMetricStream handler's not-found refusal in the cloudwatch crate — return ResourceNotFoundException (404) with no message body for a missing stream. Already-open related issue that does NOT cover this check: 20261025T020208Z-cloudwatch-firehose-s3-getmetricstream-s-not-found-error-carries-a-message.md describes the same GetMetricStream defect but is filed under the cloudwatch__firehose__s3 service token and is grounded in a different check; this check id is not in it and the report did not mark this check owned.
Traces: cloudwatch.double-delete-returns-not-found-not-success.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: 119/412 passed (28.9%); cloudwatch 111/254 (44%). Both check ids below are in the failing list and NEITHER is marked "ALREADY FILED" or "OWNED by".

Cause 1 — cloudwatch.resource-not-found-error-family [mismatch].
Passed first: cloudwatch:DeleteAlarms against a nonexistent alarm name was a no-op success (not a refusal), matching AWS.
Expected: cloudwatch:SetAlarmState against that nonexistent alarm -> HTTP 404, Error.Code 'ResourceNotFound', Error.Message '' (empty).
Actual: Error.Code 'ResourceNotFound' (correct) with Error.Message 'alarm sim2real-behav-cwprobe-alarm-missing-0aa94bb2fe35 does not exist'. AssertionError raised in the check's _assert_error helper (line 28, called from line 71): "SetAlarmState: expected message '', got 'alarm sim2real-behav-cwprobe-alarm-missing-0aa94bb2fe35 does not exist'". Fixed text in the emulator's string is "alarm " ... " does not exist"; the only variable part is the AlarmName from the request. AWS's requirement here has NO fixed text — the message is empty.
Because the check aborts at this first SetAlarmState assertion, the remaining documented not-found codes in the family were never graded: GetDashboard -> ResourceNotFound + 'Dashboard <name> does not exist', GetAlarmMuteRule -> ResourceNotFoundException + '', ListAlarmMuteRules -> ResourceNotFoundException + '', GetInsightRuleReport -> InvalidParameterValue + 'RULE_NAME: <rule>', ListTagsForResource -> ResourceNotFoundException + '', DescribeAlarmContributors -> ResourceNotFoundException + 'No alarm for name <name> was found', GetDataset (cross-account) -> AccessDenied + 'Caller account <acct> is not authorized to access resource in account 111122223333', DisassociateDatasetKmsKey -> ResourceNotFoundException + 'No CMK association found for dataset default.'

Cause 2 — cloudwatch.double-delete-returns-not-found-not-success [mismatch].
Passed first: DeleteAlarms twice (second a no-op success) with the alarm not resurrected; PutDashboard + DeleteDashboards twice, then GetDashboard -> ResourceNotFound with the exact message 'Dashboard <name> does not exist'; PutMetricStream with a syntactically fabricated Firehose ARN and role ARN accepted; GetMetricStream polled to State='running'; DeleteMetricStream twice, both clean; and GetMetricStream then raised code 'ResourceNotFoundException' (correct).
Expected: that ResourceNotFoundException's Error.Message is '' (empty).
Actual: AssertionError at check line 142: "unexpected GetMetricStream message 'the metric stream sim2real-behav-cwprobe-mstream-dd-d2c4d2cd4f89 does not exist'". Fixed text "the metric stream " ... " does not exist", variable part = the Name from the request.
Not reached as a result: the GetAlarmMuteRule-after-double-delete empty-message leg, the DeleteAnomalyDetector leg (which must answer ResourceNotFoundException with the fixed text 'Anomaly detector not found'), and the DeleteInsightRules idempotency leg (second delete must return an empty Failures list).

## Steps to Reproduce
Cause 1 (SetAlarmState ResourceNotFound message):
1. Pick an alarm name that does not exist in the account/region (e.g. any unique name; do NOT create it).
2. cloudwatch:DeleteAlarms AlarmNames=[<name>] — expected to be a no-op success (this passes today).
3. cloudwatch:SetAlarmState AlarmName=<name>, StateValue=OK, StateReason='probe'.
4. Expected: HTTP 404, Error.Code='ResourceNotFound', Error.Message='' (absent/empty).
   Observed: Error.Code='ResourceNotFound' with Error.Message='alarm <name> does not exist'.

Cause 2 (GetMetricStream ResourceNotFoundException message):
1. cloudwatch:PutMetricStream Name=<n>, FirehoseArn=arn:aws:firehose:<region>:<account>:deliverystream/<n>, RoleArn=arn:aws:iam::<account>:role/<n>, OutputFormat=json (the ARNs need not resolve).
2. Poll cloudwatch:GetMetricStream Name=<n> until State='running'.
3. cloudwatch:DeleteMetricStream Name=<n>; call it a SECOND time — both must be clean no-op successes.
4. cloudwatch:GetMetricStream Name=<n>.
5. Expected: HTTP 404, Error.Code='ResourceNotFoundException', Error.Message='' (absent/empty).
   Observed: Error.Code='ResourceNotFoundException' with Error.Message='the metric stream <n> does not exist'.
6. Contrast legs that must KEEP their text (same check, currently unreached): GetDashboard on a deleted dashboard -> ResourceNotFound + 'Dashboard <DashboardName> does not exist'; second DeleteAnomalyDetector -> ResourceNotFoundException + 'Anomaly detector not found'.

Re-replay both check ids in trace_ids to confirm the fix.
