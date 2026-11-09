Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-math-and-anomaly-band-alarm-survives-detector-delete-and-fires-out-of-band cloudwatch.awsquery-set-alarm-state-invalid-state-value-rejected-state-unchanged cloudwatch.describe-alarm-contributors-reflects-composite-members cloudwatch.alarm-contributors-ranking-order cloudwatch.describe-alarm-contributors-server-side-ranking cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period cloudwatch.put-metric-alarm-update-rejected-leaves-prior-config-untouched
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-math-and-anomaly-band-alarm-survives-detector-delete-and-fires-out-of-band|run#29|AssertionError|assert "X" not in gone and "Y" not in gone, (
Issue-Key: cloudwatch.awsquery-set-alarm-state-invalid-state-value-rejected-state-unchanged|run#13|AssertionError|assert query_error["Code"] == "ValidationError", (
Issue-Key: cloudwatch.describe-alarm-contributors-reflects-composite-members|_client_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.alarm-contributors-ranking-order|run#13|AssertionError|raise AssertionError(
Issue-Key: cloudwatch.describe-alarm-contributors-server-side-ranking|run#15|AssertionError|raise AssertionError(
Issue-Key: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period|run#24|AssertionError|assert message == _BAD_PERIOD_MESSAGE, (
Issue-Key: cloudwatch|PutMetricAlarm|ValidationError
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch: 6 net-new causes in the control plane and error envelope — DeleteAnomalyDetector does not delete, the awsQuery error code is ValidationException, DescribeAlarmContributors answers 200 for a composite, Metrics Insights queries see no published metrics, the Period refusal message is embellished and error responses carry no RequestId

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAnomalyDetector

## Description
Cause 1 of 6: DeleteAnomalyDetector returns success but the detector is still listed.
AWS: DeleteAnomalyDetector removes the detector; a subsequent DescribeAnomalyDetectors no longer returns it (and an anomaly-band alarm built on it is then evaluated without a detector). Smithy (cloudwatch 2010-08-01): DeleteAnomalyDetector(AnomalyDetectorId | SingleMetricAnomalyDetector | MetricMathAnomalyDetector) -> DeleteAnomalyDetectorOutput, with ResourceNotFoundException/404 for an unknown detector — so a successful return means it is gone.
Emulator: the call returns 200 and DescribeAnomalyDetectors still lists the detector (recorded run: still present with StateValue PENDING_TRAINING).
Why: a delete that does not delete leaks resources into the account's detector quota and keeps band alarms bound to a detector the caller believes is gone; a client cannot distinguish a deleted detector from a live one.
Fix: the DeleteAnomalyDetector handler's state mutation (it must remove the detector from the anomaly-detector store, not just answer success).
Failing check: cloudwatch.alarm-math-and-anomaly-band-alarm-survives-detector-delete-and-fires-out-of-band.

Cause 2 of 6: over the awsQuery protocol an out-of-enum SetAlarmState StateValue is refused with Error.Code 'ValidationException' instead of 'ValidationError'.
AWS: CloudWatch is an awsQueryCompatible service (Smithy: protocols awsJson1_0/awsQuery/rpcv2Cbor, protocol_modifiers awsQueryCompatible, sigv4Name monitoring). On the Query wire a protocol-level enum validation failure carries Error.Code 'ValidationError' inside the <ErrorResponse> envelope; 'ValidationException' is the JSON-protocol shape name, which awsQuery must map to its query code. Smithy also models SetAlarmState's modelled refusals with explicit awsQueryError codes (InvalidFormatFault -> 'InvalidFormat'/400, ResourceNotFound -> 'ResourceNotFound'/404), confirming that query codes, not shape names, go on this wire.
Emulator: Error.Code is 'ValidationException' on the awsQuery response.
Why: Query/SDK v2-era clients branch on Error.Code. 'ValidationException' is not a code CloudWatch's Query API emits, so client-side error handling and retry classification take the wrong branch.
Fix: the awsQuery error serializer — map the error shape to its query code (awsQueryError trait, or 'ValidationError' for protocol validation failures) instead of emitting the shape name.
Failing check: cloudwatch.awsquery-set-alarm-state-invalid-state-value-rejected-state-unchanged.

Cause 3 of 6: DescribeAlarmContributors answers 200 with an empty AlarmContributors for an alarm that cannot have contributors, where AWS refuses the call.
AWS: Smithy models DescribeAlarmContributors(AlarmName required) with errors InvalidNextToken/400 and ResourceNotFoundException/404; AWS refuses the call for an alarm that is not a contributor-bearing (Metrics-Insights) alarm — a composite alarm's members are not contributors — rather than answering an empty list.
Emulator: the call SUCCEEDS with {'AlarmContributors': []} and HTTP 200.
Why: a caller cannot tell 'this alarm has no contributors right now' from 'this alarm can never have contributors'. Code that polls for contributors spins forever on a 200 instead of failing fast.
Fix: the DescribeAlarmContributors handler's validation — classify the alarm before answering and raise the modelled refusal. Same operation as the open issue 20261108T191814Z-cloudwatch-describealarmcontributors-answers-200-for-an-alarm-aws.md, which does not carry this check's replay key.
Failing check: cloudwatch.describe-alarm-contributors-reflects-composite-members.

Cause 4 of 6: a Metrics Insights (SELECT ... GROUP BY) query never returns freshly published metrics, so contributor ranking is never produced.
AWS: a metric published through PutMetricData is selectable by a Metrics Insights query, and a GROUP BY query over it returns one time series per group; an alarm over such a query reaches ALARM and DescribeAlarmContributors then ranks the contributing series server-side.
Emulator: the GROUP BY query never indexes the published contributor metrics inside the check's budget, so the multi-time-series alarm never reaches ALARM with contributor data and the ranking assertions are never reached.
Why: Metrics Insights alarms are unusable: the query that selects the data returns nothing, so the alarm cannot evaluate and contributor ranking (the whole point of DescribeAlarmContributors) is unreachable.
Fix: the Metrics Insights query engine/metric index — newly published datapoints must be visible to SELECT/GROUP BY evaluation in the same store PutMetricData writes to.
Failing checks: cloudwatch.alarm-contributors-ranking-order, cloudwatch.describe-alarm-contributors-server-side-ranking.

Cause 5 of 6: the Period-grammar refusal message appends the offending value, changing AWS's text.
AWS: an illegal Period is refused with exactly 'Period must be 10, 20, 30 or a multiple of 60' — the message's whole job is to state which periods ARE legal, and it has no variable part.
Emulator: the message is 'Period must be 10, 20, 30 or a multiple of 60; got 45' — AWS's fixed sentence plus an emulator-authored '; got <value>' trailer carrying the request's Period.
Why: the message is a requirement of the error class. Clients and tests that match CloudWatch's wording (and users reading it) see text AWS never emits.
Fix: the PutMetricAlarm Period validation's message construction — emit AWS's fixed sentence without the trailer.
Failing check: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period.

Cause 6 of 6: an error response carries no RequestId, so ResponseMetadata['RequestId'] is absent.
AWS: every CloudWatch error response carries the request's id (the <RequestId> element of the Query <ErrorResponse> envelope / x-amzn-RequestId), and the ValidationError message for a metric-math expression naming an undefined id even ends with a trailer that repeats it: 'Error in expression \'<id>\': ID \'<name>\' not found (Service: AmazonCloudWatch; Status Code: 400; Error Code: ValidationException; Request ID: <this request's id>; Proxy: null)'. The fixed text is that template; the expression id, the undefined name and the request id are variable parts taken from the request.
Emulator: the refusal is returned without a request id at all — the SDK's ResponseMetadata has no 'RequestId' key, so the client raises KeyError before the code/message can even be compared.
Why: the request id is how a caller correlates a failure with service-side records, and SDKs/retry wrappers read it on every error. Omitting it breaks error handling for every client that logs or echoes it, and makes the documented message trailer impossible to produce.
Fix: the error-envelope serializer — emit the request id on error responses (and include the backend trailer in the metric-math ValidationError message). The value itself is per-run; the field's presence is the requirement.
Failing check: cloudwatch.put-metric-alarm-update-rejected-leaves-prior-config-untouched.

## Evidence
Cause 1 — Expected: DescribeAnomalyDetectors no longer lists the detector after DeleteAnomalyDetector. Actual: 'AssertionError: DeleteAnomalyDetector returned but DescribeAnomalyDetectors still lists {\'X\': \'PENDING_TRAINING\'}'.
Cause 2 — Expected: Error.Code 'ValidationError' on the awsQuery wire. Actual: 'AssertionError: awsQuery Error.Code was \'ValidationException\', expected \'ValidationError\''.
Cause 3 — Expected: a ClientError (modelled refusal). Actual: 'AssertionError: expected a ClientError, but the call SUCCEEDED and returned {\'AlarmContributors\': [], \'ResponseMetadata\': {\'HTTPStatusCode\': 200, ...}}'.
Cause 4 — Expected: the GROUP BY query returns the published contributor series and the alarm reaches ALARM with contributor data. Actual: 'AssertionError: Metrics Insights GROUP BY query never indexed the freshly-published contributor metrics within budget'; 'AssertionError: the multi-time-series alarm never reached ALARM with contributor data within the 240s poll budget'.
Cause 5 — Expected message: 'Period must be 10, 20, 30 or a multiple of 60'. Actual: 'Period must be 10, 20, 30 or a multiple of 60; got 45'.
Cause 6 — Expected: exc.response['ResponseMetadata']['RequestId'] present on the refusal. Actual: 'KeyError: \'RequestId\'' raised while reading the error's ResponseMetadata.

## Steps to Reproduce
Cause 1: PutAnomalyDetector, DeleteAnomalyDetector, then DescribeAnomalyDetectors and observe the detector is still listed.
Cause 2: call SetAlarmState over the awsQuery protocol with StateValue outside the enum and read Error.Code from the <ErrorResponse>.
Cause 3: PutCompositeAlarm, then DescribeAlarmContributors(AlarmName=<the composite>) and observe HTTP 200 with AlarmContributors=[].
Cause 4: PutMetricData for several dimension values, PutMetricAlarm whose Metrics uses a Metrics Insights SELECT ... GROUP BY expression, then poll the alarm state and DescribeAlarmContributors.
Cause 5: PutMetricAlarm with Period=45 and read Error.Message.
Cause 6: PutMetricAlarm whose Metrics expression names an undefined query id and inspect the ClientError's ResponseMetadata for RequestId (and the message's trailer).
