Issue-Source: trace-sweep-divergence
Issue-Trace: cwq-query-protocol-alarm-round-trip-and-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwq-query-protocol-alarm-round-trip-and-errors
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutMetricAlarm|2xx->4xx|$.PutMetricAlarmResponse
Issue-Key: monitoring|PutMetricAlarm|2xx->4xx|$headers.content-type
Issue-Key: monitoring|PutMetricAlarm|2xx->4xx|status
Issue-Key: monitoring|DescribeAlarms|2xx->4xx|$.DescribeAlarmsResponse
Issue-Key: monitoring|DescribeAlarms|2xx->4xx|$headers.content-type
Issue-Key: monitoring|DescribeAlarms|2xx->4xx|status
Issue-Key: monitoring|SetAlarmState|2xx->4xx|$.SetAlarmStateResponse
Issue-Key: monitoring|SetAlarmState|2xx->4xx|$headers.content-type
Issue-Key: monitoring|SetAlarmState|2xx->4xx|status
Issue-Key: monitoring|PutMetricAlarm|4xx->4xx|$.ErrorResponse
Issue-Key: monitoring|PutMetricAlarm|4xx->4xx|$headers.content-type
Issue-Key: monitoring|DeleteAlarms|2xx->4xx|$.DeleteAlarmsResponse
Issue-Key: monitoring|DeleteAlarms|2xx->4xx|$headers.content-type
Issue-Key: monitoring|DeleteAlarms|2xx->4xx|status
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch awsQuery form-encoded protocol is not served at all

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
5 causes, one shared fault: the emulator serves only the JSON wire for CloudWatch and rejects every awsQuery (application/x-www-form-urlencoded) request, so no query-protocol client can use the service.

SHARED AWS CONTRACT — Smithy: com.amazonaws.cloudwatch#GraniteServiceVersion20100801 declares protocols [awsJson1_0, awsQuery, rpcv2Cbor] and xmlNamespace uri 'http://monitoring.amazonaws.com/doc/2010-08-01/'. A form-encoded request carrying Action=<Operation>&Version=2010-08-01 is answered with 'content-type: text/xml' and an XML document whose root is '<OperationName>Response>' containing the operation's result element plus '<ResponseMetadata><RequestId>…</RequestId></ResponseMetadata>'. Errors on that wire are an '<ErrorResponse><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>' document, also text/xml, with the status from the error's awsQueryError httpResponseCode.

SHARED EMULATOR BEHAVIOUR: every one of these requests is answered 400 with 'content-type: application/x-amz-json-1.0' and no XML envelope at all — the form-encoded body is never parsed, so the operation does not even run.

SHARED WHY: any SDK or tool pinned to the query protocol for monitoring (older AWS SDK majors, hand-rolled SigV4 form posts, the CLI's query-protocol paths) cannot create, read, mutate or delete a single alarm. Because the failure is a 400 rather than a protocol negotiation error, the client reports a bogus client-side validation fault rather than 'protocol unsupported'.

SHARED FIX: protocol routing/dispatch for the CloudWatch crate — add an awsQuery request parser (Action/Version form decoding into the operation input) and the matching XML response and ErrorResponse serializers; the JSON handlers themselves are already correct.

Cause 1 of 5: PutMetricAlarm over awsQuery. AWS: 200, text/xml, '<PutMetricAlarmResponse><ResponseMetadata><RequestId>…</RequestId></ResponseMetadata></PutMetricAlarmResponse>'. Emulator: 400, application/x-amz-json-1.0, no PutMetricAlarmResponse. Trace: cwq-query-protocol-alarm-round-trip-and-errors#task#1.

Cause 2 of 5: DescribeAlarms over awsQuery. AWS: 200, text/xml, '<DescribeAlarmsResponse><DescribeAlarmsResult>' with MetricAlarms/CompositeAlarms/LogAlarms members (empty lists serialized as empty elements) and ResponseMetadata. Emulator: 400 JSON. Trace: cwq-query-protocol-alarm-round-trip-and-errors#task#2.

Cause 3 of 5: SetAlarmState over awsQuery. AWS: 200, text/xml, '<SetAlarmStateResponse>' with ResponseMetadata only. Emulator: 400 JSON. Trace: cwq-query-protocol-alarm-round-trip-and-errors#task#3.

Cause 4 of 5: PutMetricAlarm over awsQuery with an out-of-enum ComparisonOperator. AWS: 400, text/xml ErrorResponse with Error.Type 'Sender', Error.Code 'ValidationError' and the constraint message "1 validation error detected: Value '<comparisonOperator>' at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [<the allowed operators, in the order the service renders them>]" — fixed text apart from the echoed value. Emulator: 400 but a JSON body with no ErrorResponse element and content-type application/x-amz-json-1.0. Trace: cwq-query-protocol-alarm-round-trip-and-errors#task#5.

Cause 5 of 5: DeleteAlarms over awsQuery. AWS: 200, text/xml, '<DeleteAlarmsResponse>' with ResponseMetadata. Emulator: 400 JSON. Trace: cwq-query-protocol-alarm-round-trip-and-errors#task#7.

## Evidence
Replay (simparity-wire-aws-v1), fixture cwq-query-protocol-alarm-round-trip-and-errors, 5 clusters, all with '$headers.content-type: value_mismatch (expected text/xml, got application/x-amz-json-1.0)':
- task#1 PutMetricAlarm: status expected 200, got 400; $.PutMetricAlarmResponse missing (expected {'ResponseMetadata': {'RequestId': …}}).
- task#2 DescribeAlarms: status expected 200, got 400; $.DescribeAlarmsResponse missing (expected DescribeAlarmsResult with LogAlarms/CompositeAlarms/MetricAlarms members + ResponseMetadata). Also task#4, task#6, task#8.
- task#3 SetAlarmState: status expected 200, got 400; $.SetAlarmStateResponse missing.
- task#5 PutMetricAlarm error: $.ErrorResponse missing (expected Error.Type 'Sender', Error.Code 'ValidationError', enum-constraint message).
- task#7 DeleteAlarms: status expected 200, got 400; $.DeleteAlarmsResponse missing.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. POST a SigV4-signed application/x-www-form-urlencoded body 'Action=PutMetricAlarm&Version=2010-08-01&AlarmName=…' to the monitoring endpoint (fixture cwq-query-protocol-alarm-round-trip-and-errors step task#1).
3. Observe 400 with content-type application/x-amz-json-1.0 instead of 200 text/xml with a <PutMetricAlarmResponse> document.
4. Repeat with Action=DescribeAlarms (task#2), Action=SetAlarmState (task#3), Action=DeleteAlarms (task#7), and Action=PutMetricAlarm with ComparisonOperator=GreaterThanSomething (task#5) to see the missing <ErrorResponse> document.
