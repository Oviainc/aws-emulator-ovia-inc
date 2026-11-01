Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-composite-alarm-dependency-and-conflict-on-delete
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-composite-alarm-dependency-and-conflict-on-delete monitoring-composite-alarm-dependency-and-conflict cloudwatch-log-alarm-name-conflict cloudwatch-put-log-alarm-conflict-with-metric-alarm cwm-dimension-count-and-empty-value cwp-describe-alarms-prefix-pagination cwp-list-alarm-mute-rules-pagination cloudwatch-dataset-kms-association monitoring-dataset-kms-encryption-management
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DeleteAlarms|4xx->4xx|$.__type
Issue-Key: monitoring|DeleteAlarms|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutLogAlarm|4xx->4xx|$.__type
Issue-Key: monitoring|PutLogAlarm|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutMetricData|4xx->4xx|$.__type
Issue-Key: monitoring|PutMetricData|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DescribeAlarms|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|ListAlarmMuteRules|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DisassociateDatasetKmsKey|4xx->4xx|$.message
Issue-Key: monitoring|DisassociateDatasetKmsKey|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|AssociateDatasetKmsKey|4xx->4xx|$.message
Issue-Key: monitoring|AssociateDatasetKmsKey|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetDataset|4xx->4xx|$.message
Issue-Key: monitoring|GetDataset|4xx->4xx|$headers.x-amzn-query-error
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch error envelope omits x-amzn-query-error and unqualified __type

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAlarms

## Description
8 causes, one shared fault: the CloudWatch error serializer does not emit the awsQueryCompatible error envelope, and where it renders a constraint-violation message it prints the Smithy pattern verbatim instead of AWS's rendering.

SHARED AWS CONTRACT — Smithy: service com.amazonaws.cloudwatch#GraniteServiceVersion20100801 declares protocols [awsJson1_0, awsQuery, rpcv2Cbor] with protocol_modifiers [awsQueryCompatible]; every modeled error carries an awsQueryError trait (InvalidParameterValueException -> code InvalidParameterValue httpResponseCode 400; MissingRequiredParameterException -> MissingParameter; InvalidNextToken -> InvalidNextToken; ResourceNotFoundException -> ResourceNotFoundException). Under awsQueryCompatible AWS answers a JSON/CBOR request with BOTH (a) a response header 'x-amzn-query-error: <QueryErrorCode>;<Sender|Receiver>' so a query-era SDK can recover the legacy code, and (b) a '__type' that is the FULLY NAMESPACED Coral shape id — 'com.amazon.coral.validate#ValidationException' for request-shape/constraint violations and 'com.amazon.coral.service#<Name>Exception' for the service's own modeled errors. Both are fixed text for the error class; only the error code token varies.

SHARED EMULATOR BEHAVIOUR: no 'x-amzn-query-error' header is emitted on ANY error response, and '__type' carries a bare local name ('ValidationError', 'InvalidParameterValue', 'ValidationException') with no namespace.

SHARED WHY: a client that reads the legacy query error code (botocore's query-compatible error shim, every AWS SDK built on the awsQuery wire for monitoring) gets no code at all and falls back to a generic failure, so retry/branch logic keyed on 'MissingParameter' vs 'InvalidParameterValue' vs 'InvalidNextToken' never fires. A client matching on the Coral '__type' sees an unknown type and cannot classify the fault as sender vs receiver.

SHARED FIX: the service-wide error serializer for the CloudWatch crate — the component that turns a modeled error into the HTTP response: it must set the x-amzn-query-error header from the error's awsQueryError code plus the sender/receiver fault, and namespace __type.

Cause 1 of 8: DeleteAlarms refusing the delete of an alarm referenced by a composite alarm. AWS: 400 with __type 'com.amazon.coral.validate#ValidationException' and header 'x-amzn-query-error: ValidationError;Sender'. Emulator: 400 with __type 'ValidationError' and no header. (Status and message already match.) Trace: cloudwatch-composite-alarm-dependency-and-conflict-on-delete#task#8.

Cause 2 of 8: PutLogAlarm refusing a log-alarm name that collides with an existing metric alarm. AWS: 400, __type 'com.amazon.coral.validate#ValidationException', header 'ValidationError;Sender'. Emulator: __type 'ValidationError', no header. Trace: cloudwatch-log-alarm-name-conflict#task#3.

Cause 3 of 8: PutMetricData refusing a dimension set over the per-metric cap. AWS: 400, __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender' (Smithy: PutMetricData errors include InvalidParameterValueException with awsQueryError code InvalidParameterValue). Emulator: __type 'InvalidParameterValue', no header. Trace: cwm-dimension-count-and-empty-value#task#2.

Cause 4 of 8: DescribeAlarms refusing an out-of-range MaxRecords / bad filter. AWS: 400 plus header 'ValidationError;Sender'. Emulator: correct status, correct body, no header. Trace: cwp-describe-alarms-prefix-pagination#task#5.

Cause 5 of 8: ListAlarmMuteRules refusing a bad pagination argument. AWS: 400 plus header 'ValidationError;Sender'. Emulator: no header. Trace: cwp-list-alarm-mute-rules-pagination#task#8.

Cause 6 of 8: DisassociateDatasetKmsKey with a DatasetIdentifier that is neither 'default' nor a dataset/default ARN. AWS: 400 ValidationError whose message is "1 validation error detected: Value '<datasetIdentifier>' at 'datasetIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (default|arn:[a-zA-Z0-9-]+:cloudwatch:[a-zA-Z0-9-]*:\\d{12}:dataset/default)" — fixed text apart from the echoed value; the printed pattern is the modelled pattern WITHOUT the ^ and $ anchors. Smithy: AssociateDatasetKmsKey/GetDataset/DisassociateDatasetKmsKey DatasetIdentifier @pattern "^(default|arn:[a-zA-Z0-9-]+:cloudwatch:[a-zA-Z0-9-]*:\\d{12}:dataset/default)$". Emulator: prints the pattern WITH the leading ^ and trailing $, and omits the 'ValidationError;Sender' header. Trace: cloudwatch-dataset-kms-association#task#2.

Cause 7 of 8: AssociateDatasetKmsKey with a malformed KmsKeyArn. AWS message: "1 validation error detected: Value '<kmsKeyArn>' at 'kmsKeyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-zA-Z0-9-]+:kms:[a-zA-Z0-9-]+:\\d{12}:key/[a-f0-9-]+" (anchors stripped). Emulator: anchored form '^arn:...$', plus no header. Trace: cloudwatch-dataset-kms-association#task#3.

Cause 8 of 8: GetDataset with a non-default DatasetIdentifier — same anchored-pattern leak and missing header as Cause 6, on the read path. Trace: cloudwatch-dataset-kms-association#task#5.

Related but NOT covering: open issue 20261029T182452Z-cloudwatch-request-validation-and-error-envelope-divergences-10 is a BEHAVIORAL (simparity-behavioral-aws-v1) issue and carries no wire replay keys for these fixtures.

## Evidence
Replay (simparity-wire-aws-v1), 8 clusters.
1. DeleteAlarms, cloudwatch-composite-alarm-dependency-and-conflict-on-delete#task#8: $.__type expected 'com.amazon.coral.validate#ValidationException', got 'ValidationError'; $headers.x-amzn-query-error expected 'ValidationError;Sender', got None. (also monitoring-composite-alarm-dependency-and-conflict#task#8)
2. PutLogAlarm, cloudwatch-log-alarm-name-conflict#task#3: same two diffs. (also cloudwatch-put-log-alarm-conflict-with-metric-alarm#task#3)
3. PutMetricData, cwm-dimension-count-and-empty-value#task#2: $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got 'InvalidParameterValue'; header expected 'InvalidParameterValue;Sender', got None.
4. DescribeAlarms, cwp-describe-alarms-prefix-pagination#task#5: header expected 'ValidationError;Sender', got None (also #task#6).
5. ListAlarmMuteRules, cwp-list-alarm-mute-rules-pagination#task#8: header expected 'ValidationError;Sender', got None.
6. DisassociateDatasetKmsKey, cloudwatch-dataset-kms-association#task#2: message expected pattern '(default|arn:...:dataset/default)', got '^(default|arn:...:dataset/default)$'; header missing. (also monitoring-dataset-kms-encryption-management#task#3)
7. AssociateDatasetKmsKey, cloudwatch-dataset-kms-association#task#3: expected 'arn:[a-zA-Z0-9-]+:kms:...key/[a-f0-9-]+', got '^arn:...$'; header missing. (also monitoring-dataset-kms-encryption-management#task#2)
8. GetDataset, cloudwatch-dataset-kms-association#task#5: same as 6. (also monitoring-dataset-kms-encryption-management#task#4)

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. Replay cloudwatch-composite-alarm-dependency-and-conflict-on-delete step task#8 (DeleteAlarms on an alarm referenced by a composite alarm) and inspect the response headers and the __type member.
3. Repeat for cloudwatch-log-alarm-name-conflict#task#3, cwm-dimension-count-and-empty-value#task#2, cwp-describe-alarms-prefix-pagination#task#5, cwp-list-alarm-mute-rules-pagination#task#8.
4. Replay cloudwatch-dataset-kms-association steps task#2, task#3 and task#5 and compare the printed regular-expression pattern in $.message against the recorded response.
