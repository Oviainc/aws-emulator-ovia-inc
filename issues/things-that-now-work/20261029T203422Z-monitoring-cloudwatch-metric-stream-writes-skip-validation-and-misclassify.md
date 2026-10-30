Issue-Source: trace-sweep-divergence
Issue-Trace: monitoring-metrics-export-and-enrichment-pipeline
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: monitoring-metrics-export-and-enrichment-pipeline
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutMetricStream|4xx->2xx|$.__type
Issue-Key: monitoring|PutMetricStream|4xx->2xx|$.message
Issue-Key: monitoring|PutMetricStream|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutMetricStream|4xx->2xx|status
Issue-Key: monitoring|PutMetricStream|4xx->4xx|$.__type
Issue-Key: monitoring|PutMetricStream|4xx->4xx|$.message
Issue-Key: monitoring|PutMetricStream|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|StartMetricStreams|4xx->2xx|$
Issue-Key: monitoring|StartMetricStreams|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|StartMetricStreams|4xx->2xx|status
Issue-Key: monitoring|StopMetricStreams|4xx->2xx|$
Issue-Key: monitoring|StopMetricStreams|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|StopMetricStreams|4xx->2xx|status
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$.__type
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$.message
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetMetricStream|4xx->4xx|status
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch metric-stream writes skip validation and misclassify a missing stream

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
5 causes, one shared fault: the metric-stream operations do not perform the service's own InvalidParameterValue checks — role and name validity — so invalid writes are accepted, the one check that does run fires in the wrong order, and a missing stream is reported as not-found instead of an invalid name.

SHARED AWS CONTRACT: Smithy declares InvalidParameterValueException (awsQueryError code InvalidParameterValue, 400) on PutMetricStream, StartMetricStreams, StopMetricStreams and GetMetricStream, and the recordings show the service refusing with __type 'com.amazon.coral.service#InvalidParameterValueException' plus header 'x-amzn-query-error: InvalidParameterValue;Sender'. AWS validates the metric-stream RoleArn and the stream Name as parameter VALUES and uses short fixed messages for both classes: 'Invalid Role ARN' and 'Invalid Name' (no variable parts).

Cause 1 of 5: PutMetricStream with a RoleArn that is not an IAM role ARN. AWS: 400 InvalidParameterValueException, header 'InvalidParameterValue;Sender', message 'Invalid Role ARN'. Emulator: 200 — the stream is created with an unusable role. Why: the export pipeline is created and reported healthy while nothing can ever be delivered through it, so the failure only surfaces as silent data loss. Fix: PutMetricStream's RoleArn validation. Trace: monitoring-metrics-export-and-enrichment-pipeline#task#2.

Cause 2 of 5: PutMetricStream with BOTH a bad RoleArn and an out-of-enum OutputFormat. AWS: 400 InvalidParameterValueException with 'Invalid Role ARN' — the role check runs FIRST and wins. Smithy: PutMetricStream.OutputFormat targets MetricStreamOutputFormat (enum json / opentelemetry0.7 / opentelemetry1.0). Emulator: 400 but __type 'ValidationError' with the generic enum message "1 validation error detected: Value 'xml' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [json, opentelemetry0.7, opentelemetry1.0]", and no query-error header. Why: the validation ORDER differs, so a caller fixing the reported fault (the output format) still gets a refusal, and the reported error class is a shape validation rather than the service's parameter-value error. Fix: PutMetricStream's validation ordering — the role check must precede enum/shape checks — plus the InvalidParameterValue classification. Trace: monitoring-metrics-export-and-enrichment-pipeline#task#3.

Cause 3 of 5: StartMetricStreams naming a stream whose name is not a valid metric-stream name. AWS: 400 InvalidParameterValueException, header 'InvalidParameterValue;Sender', message 'Invalid Name'. Emulator: 200 with an empty body — the start is reported successful. Why: a caller believes it started a stream that does not and cannot exist. Fix: StartMetricStreams' per-name validation. Trace: monitoring-metrics-export-and-enrichment-pipeline#task#4.

Cause 4 of 5: StopMetricStreams with the same invalid name. AWS: 400 InvalidParameterValueException, 'Invalid Name'. Emulator: 200, empty body. Same check, different operation, so Cause 3 alone does not close it. Trace: monitoring-metrics-export-and-enrichment-pipeline#task#5.

Cause 5 of 5: GetMetricStream for a name that is not a valid metric-stream name. AWS: 400 with __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender' and message 'Invalid Name' — the name is rejected as a value before any existence lookup. Emulator: 404 with __type 'ResourceNotFoundException', header 'ResourceNotFoundException;Sender' and NO message member. Why: the status class changes from 400 to 404 and the message disappears, so a client cannot tell 'you asked for a name that cannot exist' from 'this stream was deleted', and retry/create-if-absent logic takes the wrong branch. Fix: GetMetricStream's name validation must run before the existence check (Smithy models both InvalidParameterValueException and ResourceNotFoundException on this operation). Trace: monitoring-metrics-export-and-enrichment-pipeline#task#6.

## Evidence
Replay (simparity-wire-aws-v1), fixture monitoring-metrics-export-and-enrichment-pipeline, 5 clusters.
1. PutMetricStream task#2: status expected 400, got 200; $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got None; $.message expected 'Invalid Role ARN', got None; header expected 'InvalidParameterValue;Sender', got None.
2. PutMetricStream task#3: $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got 'ValidationError'; $.message expected 'Invalid Role ARN', got "1 validation error detected: Value 'xml' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [json, opentelemetry0.7, opentelemetry1.0]"; header missing.
3. StartMetricStreams task#4: status expected 400, got 200; body empty (expected __type 'com.amazon.coral.service#InvalidParameterValueException', message 'Invalid Name'); header missing.
4. StopMetricStreams task#5: status expected 400, got 200; body empty (same expected envelope); header missing.
5. GetMetricStream task#6: status expected 400, got 404; $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got 'ResourceNotFoundException'; $.message expected 'Invalid Name', got None; header expected 'InvalidParameterValue;Sender', got 'ResourceNotFoundException;Sender'. (also task#8)

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutMetricStream with a RoleArn that is not an IAM role ARN (monitoring-metrics-export-and-enrichment-pipeline#task#2) — observe 200.
3. PutMetricStream with that same bad RoleArn AND OutputFormat='xml' (task#3) — observe the outputFormat enum error instead of 'Invalid Role ARN'.
4. StartMetricStreams (task#4) and StopMetricStreams (task#5) naming an invalid metric-stream name — observe 200.
5. GetMetricStream for that invalid name (task#6) — observe 404 ResourceNotFoundException with no message instead of 400 'Invalid Name'.
