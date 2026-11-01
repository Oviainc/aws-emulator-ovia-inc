Issue-Source: trace-sweep-divergence
Issue-Trace: cwa-metrics-exactly-one-return-data
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwa-metrics-exactly-one-return-data
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutMetricAlarm|4xx->4xx|$.__type
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-alarm-writes-and-pagination-skip-validation-or.md
Issue-Commit: aee06a4ef48d73bb626f6728ef04d56e3c8e005d

# PutMetricAlarm validation refusal omits x-amzn-query-error and sends an unqualified __type

> _Note: this issue was identified against the code base at commit `aee06a4ef48d73bb626f6728ef04d56e3c8e005d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
1 cause: PutMetricAlarm's request-validation refusal is returned without the awsQueryCompatible error envelope, so the legacy query error code is unrecoverable and the Coral type is unqualified.

Cause 1 of 1: PutMetricAlarm refusing a Metrics (MetricDataQueries) list that does not designate exactly one query with ReturnData=true.

AWS: Smithy: service com.amazonaws.cloudwatch#GraniteServiceVersion20100801 (sdkId CloudWatch, endpointPrefix/sigv4Name monitoring, version 2010-08-01) declares protocols [awsJson1_0, awsQuery, rpcv2Cbor] with protocol_modifiers [awsQueryCompatible]; PutMetricAlarm's own modeled error list carries only LimitExceededFault (awsQueryError code LimitExceeded, httpResponseCode 400), so a request-shape/constraint violation on PutMetricAlarm is reported as the protocol's Coral request-validation fault. Under awsQueryCompatible AWS answers a JSON/CBOR request with BOTH (a) the response header 'x-amzn-query-error: <QueryErrorCode>;<Sender|Receiver>' — here the fixed text 'ValidationError;Sender' — and (b) a '__type' that is the FULLY NAMESPACED Coral shape id, here the fixed text 'com.amazon.coral.validate#ValidationException'. The recorded response for this step is HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and that header; the only variable part of such a refusal is the message's echoed member name/value, and that part already matches.

Emulator: answers the correct HTTP 400 and the correct message, but sets '__type' to the bare local name 'ValidationError' (no 'com.amazon.coral.validate#' namespace) and emits NO 'x-amzn-query-error' response header at all.

Why: a client relying on AWS's behavior cannot classify the failure. A query-era/query-compatible SDK (botocore's awsQueryCompatible error shim and every AWS SDK that resolves monitoring over the query wire) reads the legacy code out of x-amzn-query-error; with the header absent it sees no error code and degrades to a generic/unknown failure, so caller branching and retry policy keyed on 'ValidationError' vs 'LimitExceeded' never fires. A client matching on the Coral '__type' sees an unmodelled type id and cannot determine the sender-vs-receiver fault, so a client-side input bug is indistinguishable from a service fault.

Fix: the CloudWatch crate's error serializer — the component that turns a refusal into the HTTP response for the PutMetricAlarm handler's validation path: it must namespace __type to the Coral shape id and set x-amzn-query-error from the error's query code plus the Sender/Receiver fault.

Traces: cwa-metrics-exactly-one-return-data#task#2.

Related but NOT covering: open issue 20261029T203422Z-monitoring-cloudwatch-error-envelope-omits-x-amzn-query-error.md describes the same service-wide serializer fault, but its replay keys cover only DeleteAlarms, PutLogAlarm, PutMetricData, DescribeAlarms, ListAlarmMuteRules, AssociateDatasetKmsKey, DisassociateDatasetKmsKey and GetDataset — it carries no key for PutMetricAlarm, so this step is not exercised by it.

## Evidence
Replay (simparity-wire-aws-v1), 1 cluster, 1 affected step.
PutMetricAlarm, cwa-metrics-exactly-one-return-data#task#2: expected_status 400, actual_status 400 (status matches); $.__type expected 'com.amazon.coral.validate#ValidationException', got 'ValidationError'; $headers.x-amzn-query-error expected 'ValidationError;Sender', got None.
divergence_key: service=monitoring|PutMetricAlarm|status=400->400|keys=$.__type,$headers.x-amzn-query-error
Fixed text of the expected envelope: __type 'com.amazon.coral.validate#ValidationException' and header value 'ValidationError;Sender' (error code token + fault). No per-run value (request id, timestamp, generated name) is involved in this divergence.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture cwa-metrics-exactly-one-return-data, step task#2: a PutMetricAlarm whose Metrics list does not mark exactly one MetricDataQuery with ReturnData=true.
3. Inspect the 400 response: read the 'x-amzn-query-error' response header (expected 'ValidationError;Sender', emulator sends none) and the '__type' member of the JSON body (expected 'com.amazon.coral.validate#ValidationException', emulator sends 'ValidationError').
