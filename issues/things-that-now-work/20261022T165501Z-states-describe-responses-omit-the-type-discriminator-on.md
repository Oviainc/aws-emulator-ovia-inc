Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-encryption-and-logging-config-validation
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-encryption-and-logging-config-validation sfn-jsonata-variable-references sfn-state-machine-create-idempotent-repeat sfn-state-machine-deleting-rejections states-definition-validation-workflow states-execution-failure-and-redrive states-execution-lifecycle-success-and-abort states-express-state-machine-type-restrictions states-state-machine-lifecycle states-state-machine-lifecycle-crud states-state-machine-versioning-and-aliases stepfunctions-state-machine-crud-and-tagging stepfunctions-version-alias-blue-green-rollout sfn-start-execution-idempotent-while-running stepfunctions-redrive-execution-error-paths stepfunctions-standard-execution-start-monitor-stop states-standard-execution-lifecycle
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.encryptionConfiguration.__type
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.loggingConfiguration.__type
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.tracingConfiguration.__type
Issue-Key: states|DescribeExecution|2xx->2xx|$.inputDetails.__type
Issue-Key: states|DescribeExecution|2xx->2xx|$.outputDetails.__type
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# states Describe responses omit the __type discriminator on nested structures

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStateMachine

## Description
Step Functions is an awsJson1_0 service (Smithy: service AWSStepFunctions, protocols [awsJson1_0], sdkId SFN, endpointPrefix/arnNamespace 'states'). Every recording of this API shows AWS emitting a `__type` member on the nested structures it serializes inside a response body, carrying the shape id in AWS's internal SWF base namespace (`com.amazonaws.swf.base.model#<Shape>`; the model's xmlNamespace http://swf.amazonaws.com/doc/2015-07-20/ is the same lineage). The emulator's JSON serializer emits the nested structures' data members but never the `__type` key, on every nested structure of every Describe response. One root cause, three places it is observed. Note: the RESOLVED issue 20261022T120501Z-states-describeexecution-omits-redrive-fields-and-execution-data-details.md made DescribeExecution return inputDetails/outputDetails at all, but it did not add the discriminator, and a resolved issue covers no live failure.

Cause 1 of 3: DescribeStateMachine's configuration sub-structures carry no __type.
AWS: Smithy: DescribeStateMachine output members loggingConfiguration -> LoggingConfiguration, tracingConfiguration -> TracingConfiguration, encryptionConfiguration -> EncryptionConfiguration (all plain structures). Recorded behavior only for the discriminator: the response serializes each of these with `__type` = 'com.amazonaws.swf.base.model#LoggingConfiguration' / '#TracingConfiguration' / '#EncryptionConfiguration' alongside the structure's own members, with status 200.
Emulator: status 200 and the three structures are present, but `$.loggingConfiguration.__type`, `$.tracingConfiguration.__type` and `$.encryptionConfiguration.__type` are all absent.
Why: a client or recording-based test that keys off the discriminator to identify the serialized shape (SDK-independent deserializers, response-shape fixtures, proxies that re-serialize AWS JSON) sees a structurally different body than AWS returns, for the single most-used read in the service - this cluster alone covers 20 recorded steps across 14 fixtures.
Fix: the DescribeStateMachine response serializer in the sfn service (the nested-structure serialization path, not the handler or state), so that nested structures emit their shape discriminator.
Traces: sfn-encryption-and-logging-config-validation#task#7 (cluster also covers sfn-jsonata-variable-references#task#3..#5, sfn-state-machine-create-idempotent-repeat#task#6, sfn-state-machine-deleting-rejections#task#5, states-definition-validation-workflow#task#7, states-execution-failure-and-redrive#task#6/#9/#13, states-execution-lifecycle-success-and-abort#task#5, states-express-state-machine-type-restrictions#task#5/#6, states-state-machine-lifecycle#task#3, states-state-machine-lifecycle-crud#task#3/#6, states-state-machine-versioning-and-aliases#task#8, stepfunctions-state-machine-crud-and-tagging#task#3/#10, stepfunctions-version-alias-blue-green-rollout#task#17).

Cause 2 of 3: DescribeExecution's inputDetails carries no __type while the execution is RUNNING.
AWS: Smithy: DescribeExecution output member inputDetails -> CloudWatchEventsExecutionDataDetails (structure with a single boolean member `included`, documented 'Always true for API calls'). Recorded behavior: the 200 body serializes inputDetails with `__type` = 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails' plus included:true.
Emulator: status 200, inputDetails present with its boolean, but `$.inputDetails.__type` absent.
Why: same as cause 1, on the execution read; 9 recorded steps across 7 fixtures.
Fix: the DescribeExecution response serializer in the sfn service (same nested-structure serialization path).
Traces: sfn-start-execution-idempotent-while-running#task#7 (cluster also covers sfn-start-execution-idempotent-while-running#task#10, sfn-state-machine-deleting-rejections#task#10, states-execution-failure-and-redrive#task#7/#10, states-execution-lifecycle-success-and-abort#task#14, stepfunctions-redrive-execution-error-paths#task#4, stepfunctions-standard-execution-start-monitor-stop#task#4/#11).

Cause 3 of 3: once the execution is terminal, BOTH inputDetails and outputDetails lack __type.
AWS: Smithy: DescribeExecution output members inputDetails and outputDetails both -> CloudWatchEventsExecutionDataDetails. Recorded behavior: both are serialized with `__type` = 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', status 200.
Emulator: status 200, both structures present, `$.inputDetails.__type` and `$.outputDetails.__type` both absent.
Why: confirms the omission is systematic over every nested structure rather than one forgotten member, so a fix that special-cases one member leaves the rest diverging.
Fix: the same nested-structure serialization path, applied for every structure member of the output shape.
Traces: states-execution-failure-and-redrive#task#14 (cluster also covers states-execution-lifecycle-success-and-abort#task#6, states-standard-execution-lifecycle#task#4).

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures 0), all three clusters verdict=mismatch with expected and actual status BOTH 200 - only the body diverges:
- sfn-encryption-and-logging-config-validation#task#7 DescribeStateMachine: $.encryptionConfiguration.__type missing (expected 'com.amazonaws.swf.base.model#EncryptionConfiguration'), $.loggingConfiguration.__type missing (expected '...#LoggingConfiguration'), $.tracingConfiguration.__type missing (expected '...#TracingConfiguration'). affected_count 20.
- sfn-start-execution-idempotent-while-running#task#7 DescribeExecution: $.inputDetails.__type missing (expected 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails'). affected_count 9.
- states-execution-failure-and-redrive#task#14 DescribeExecution: $.inputDetails.__type and $.outputDetails.__type both missing (same expected value). affected_count 3.
Smithy evidence: protocols [awsJson1_0]; CloudWatchEventsExecutionDataDetails / LoggingConfiguration / TracingConfiguration / EncryptionConfiguration are all plain nested structures of the two output shapes.

## Steps to Reproduce
1. Replay fixture sfn-encryption-and-logging-config-validation; at step task#7 DescribeStateMachine returns 200 - compare the body against the recorded one: the three configuration objects have no __type key.
2. Replay fixture sfn-start-execution-idempotent-while-running; at step task#7 DescribeExecution returns 200 with inputDetails lacking __type.
3. Replay fixture states-execution-failure-and-redrive; at step task#14 DescribeExecution returns 200 with both inputDetails and outputDetails lacking __type.
4. Re-replay all three steps after the serializer change; the 20+9+3 affected steps listed under Traces should all pass.
