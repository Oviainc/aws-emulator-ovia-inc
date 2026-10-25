Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.deploy-cloudformation-composite-alarm-derives-state cloudwatch.deploy-cloudformation-composite-alarm-evaluates-children cloudwatch.deploy-cloudwatch-1-alarm-rename-replaces-and-dangling-composite-rejected cloudwatch.deploy-cloudformation-alarm-update-changes-threshold-behavior cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store cloudwatch.rpcv2cbor-protocol-discriminator-and-framing-refusals cloudwatch.metric-stream-stop-gates-firehose-delivery-until-restarted cloudwatch.get-metric-data-invalid-scan-by-value-rejected cloudwatch.invalid-next-token-family
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.deploy-cloudformation-composite-alarm-derives-state|run#11|AssertionError|assert len(described["MetricAlarms"]) == 1, f"expected 1 metric alarm, got {described['MetricAlarms']!r}"
Issue-Key: cloudwatch.deploy-cloudformation-composite-alarm-evaluates-children|run#12|AssertionError|assert len(composites) == 1, "CloudFormation did not create the composite alarm"
Issue-Key: cloudwatch.deploy-cloudwatch-1-alarm-rename-replaces-and-dangling-composite-rejected|run#29|AssertionError|assert [a["AlarmName"] for a in old_desc] == [old_alarm], (
Issue-Key: cloudwatch.deploy-cloudformation-alarm-update-changes-threshold-behavior|_poll_alarm_state#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store|run#21|AssertionError|assert status == 200, f"CBOR PutMetricData returned {status!r}, body {decoded!r}"
Issue-Key: cloudwatch.rpcv2cbor-protocol-discriminator-and-framing-refusals|run#15|AssertionError|assert status == 404, (
Issue-Key: cloudwatch.metric-stream-stop-gates-firehose-delivery-until-restarted|poll#14|Timeout|time.sleep(_POLL_S)
Issue-Key: cloudwatch.get-metric-data-invalid-scan-by-value-rejected|run#10|AssertionError|assert code == "ValidationError", f"expected ValidationError for an out-of-enum ScanBy, got {code!r}"
Issue-Key: cloudwatch.invalid-next-token-family|run#6|AssertionError|assert message == f"Invalid NextToken : {bogus_token}", f"DescribeAlarmHistory: unexpected InvalidNextToken message {message!r}"
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# CloudWatch: CFN-deployed alarms are never created, the rpc-v2-cbor protocol is not served, metric streams do not gate Firehose delivery, and two refusals use the wrong code/message (5 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Five net-new root causes left over after the alarm-evaluation, PutMetricData, read-path, metric-stream-config and delete-semantics clusters.

Cause 1 of 5: CloudFormation-deployed AWS::CloudWatch::Alarm / AWS::CloudWatch::CompositeAlarm resources are never registered in CloudWatch.
AWS: when a stack declaring these resource types reaches CREATE_COMPLETE, DescribeAlarms reports the alarms it declared, with the declared Threshold/ComparisonOperator, and a composite alarm derives its state from the children the stack also created. An UpdateStack that changes an alarm's Threshold changes the alarm's behavior.
Emulator: the stack settles successfully, but DescribeAlarms returns [] for the declared alarm, the composite alarm does not exist, and the alarm-update check hangs to the 60 s cap because no alarm is there to re-evaluate.
Why: every alarm provisioned through infrastructure-as-code silently does not exist, while the stack reports success — the worst failure mode for a deployment pipeline, since nothing surfaces until the alarm is needed. Open issue 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md reports the same class for Logs/ECS resources; it does not carry these CloudWatch replay keys.
Fix: AWS::CloudWatch::Alarm / AWS::CloudWatch::CompositeAlarm resource provisioning in the CloudFormation resource handlers, which must call through to the cloudwatch alarm state.
Traces: cloudwatch.deploy-cloudformation-composite-alarm-derives-state, cloudwatch.deploy-cloudformation-composite-alarm-evaluates-children, cloudwatch.deploy-cloudwatch-1-alarm-rename-replaces-and-dangling-composite-rejected, cloudwatch.deploy-cloudformation-alarm-update-changes-threshold-behavior.

Cause 2 of 5: the smithy-rpc-v2-cbor protocol is not served, and an absent protocol discriminator answers the wrong status.
AWS: CloudWatch serves both awsJson1.0 and smithy-rpc-v2-cbor over the same metric store. A request carrying `smithy-protocol: rpc-v2-cbor` is routed by that header and must NOT require `x-amz-target`; a request with no recognized protocol discriminator is answered 404. Recorded behavior: the CBOR PutMetricData succeeds and the same datapoints are readable through awsJson1.0; the discriminator-less request answers 404.
Emulator: the CBOR PutMetricData answers 400 with body {"__type":"SerializationException","message":"missing required header `x-amz-target`"} — the CBOR framing is never recognized, so the two protocols are not one metric store; and omitting `smithy-protocol` answers 400 with the same SerializationException instead of 404.
Why: any client configured for rpc-v2-cbor cannot write or read metrics at all, and the 400-for-404 substitution makes an unrouted request look like a malformed one.
Fix: protocol dispatch for the cloudwatch service — recognition of the `smithy-protocol: rpc-v2-cbor` discriminator and CBOR request framing, and the not-found status for an unrecognized discriminator.
Traces: cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store, cloudwatch.rpcv2cbor-protocol-discriminator-and-framing-refusals.

Cause 3 of 5: StopMetricStreams does not gate Firehose delivery, and restarting does not resume it.
AWS: a stopped metric stream delivers no records to its Firehose destination, and StartMetricStreams resumes delivery. Recorded behavior: delivery observably quiesces and resumes.
Emulator: the check hangs to the 60 s cap waiting for the delivery transition — no delivery boundary is ever observable. Open issue 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md reports the export path producing nothing under a different service token; it does not carry this replay key.
Why: a client cannot use stop/start to pause an export, and cannot verify that a paused stream stopped billing/delivering.
Fix: the metric-stream export runtime and its state gating in the StartMetricStreams/StopMetricStreams handlers.
Traces: cloudwatch.metric-stream-stop-gates-firehose-delivery-until-restarted.

Cause 4 of 5: GetMetricData with an out-of-enum ScanBy is refused with an unmodeled ValidationException.
AWS: an out-of-enum ScanBy is refused 400 with the operation's modeled validation error (InvalidParameterValue); GetMetricData does not model ValidationException.
Emulator: answers ValidationException.
Why: an error code the operation does not model, so SDK error mapping and typed handlers never match it.
Fix: enum rejection in the GetMetricData handler's input validation must surface the modeled error.
Traces: cloudwatch.get-metric-data-invalid-scan-by-value-rejected.

Cause 5 of 5: DescribeAlarmHistory's InvalidNextToken message is rewritten.
AWS: a NextToken this service did not issue is refused with AWS's InvalidNextToken message for the operation.
Emulator: refuses with 'NextToken is not a token this service issued'.
Why: message-level divergence on a refusal the recording states, so clients asserting the text diverge.
Fix: the refusal message in the DescribeAlarmHistory pagination-token validation.
Traces: cloudwatch.invalid-next-token-family.

## Evidence
Cause 1: "AssertionError: expected 1 metric alarm, got []"; "AssertionError: CloudFormation did not create the composite alarm"; "AssertionError: DescribeAlarms for the deployed alarm returned []"; and "Error: Check timed out after 60.0s" for the alarm-update check.
Cause 2: "AssertionError: CBOR PutMetricData returned 400, body ':\"SerializationException\",\"message\":\"missing required header `x-amz-target`\"}'"; "AssertionError: omitting smithy-protocol gave 400, expected 404 -- the header is the routing discriminator on real CloudWatch. body b'{\"__type\":\"SerializationException\",\"message\":\"missing required header `x-amz-target`\"}'".
Cause 3: "Error: Check timed out after 60.0s" (cloudwatch.metric-stream-stop-gates-firehose-delivery-until-restarted).
Cause 4: "AssertionError: expected ValidationError for an out-of-enum ScanBy, got 'ValidationException'".
Cause 5: "AssertionError: DescribeAlarmHistory: unexpected InvalidNextToken message 'NextToken is not a token this service issued'".

## Steps to Reproduce
1. CreateStack with an AWS::CloudWatch::Alarm and an AWS::CloudWatch::CompositeAlarm; wait for CREATE_COMPLETE; DescribeAlarms -> [].
2. POST a CBOR-encoded PutMetricData with header `smithy-protocol: rpc-v2-cbor` and no `x-amz-target` -> 400 SerializationException. POST with no protocol discriminator -> 400 instead of 404.
3. PutMetricStream + StartMetricStreams, publish data, StopMetricStreams, publish more, StartMetricStreams -> no delivery boundary is observable; the check hangs.
4. GetMetricData(ScanBy='NotAnEnumValue') -> ValidationException instead of InvalidParameterValue.
5. DescribeAlarmHistory(NextToken='garbage') -> InvalidNextToken with a rewritten message.
