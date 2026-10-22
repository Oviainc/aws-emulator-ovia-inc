Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-state-machine-create-idempotent-repeat
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-state-machine-create-idempotent-repeat sfn-state-machine-deleting-rejections states-definition-validation-workflow states-execution-failure-and-redrive states-execution-lifecycle-success-and-abort states-express-state-machine-type-restrictions states-state-machine-lifecycle states-state-machine-lifecycle-crud states-state-machine-versioning-and-aliases stepfunctions-state-machine-crud-and-tagging sfn-encryption-and-logging-config-validation sfn-jsonata-variable-references stepfunctions-version-alias-blue-green-rollout
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.encryptionConfiguration
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.loggingConfiguration
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.tracingConfiguration
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.encryptionConfiguration.__type
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.variableReferences
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.definition
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.revisionId
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# DescribeStateMachine omits configuration defaults, revisionId and variableReferences and ignores includedData

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStateMachine

## Description
DescribeStateMachine answers 200 but its projection is incomplete: the three configuration structures AWS always returns are missing, revisionId and variableReferences are never emitted, includedData is ignored, and an updated definition is not reflected. 5 causes.

Cause 1 of 5: loggingConfiguration, tracingConfiguration and encryptionConfiguration defaults are never returned (largest cluster, 13 steps).
AWS: Smithy: DescribeStateMachine output members loggingConfiguration -> LoggingConfiguration {level: LogLevel, includeExecutionData: boolean default false}, tracingConfiguration -> TracingConfiguration {enabled: boolean default false}, encryptionConfiguration -> EncryptionConfiguration {type: EncryptionType [AWS_OWNED_KEY, CUSTOMER_MANAGED_KMS_KEY] required}. The recording shows a state machine created without any of these options answering encryptionConfiguration {type: 'AWS_OWNED_KEY'}, loggingConfiguration {level: 'OFF', includeExecutionData: false} and tracingConfiguration {enabled: false}, each with the awsJson1_0 __type discriminator ('com.amazonaws.swf.base.model#...').
Emulator: all three members are absent from the body.
Why: clients that read the effective logging level, tracing flag or encryption type (e.g. to assert a state machine is AWS-owned-key encrypted, or to round-trip a describe into an update) get null and cannot tell 'default' from 'unsupported'.
Fix: the DescribeStateMachine response projection in the sfn service - emit the defaulted configuration structures (and their nested __type) rather than omitting absent options.
Traces: sfn-state-machine-create-idempotent-repeat#task#6 (also sfn-state-machine-deleting-rejections#task#5, states-definition-validation-workflow#task#7, states-execution-failure-and-redrive#task#6/#9/#13, states-execution-lifecycle-success-and-abort#task#5, states-express-state-machine-type-restrictions#task#5/#6, states-state-machine-lifecycle#task#3, states-state-machine-lifecycle-crud#task#3, states-state-machine-versioning-and-aliases#task#8, stepfunctions-state-machine-crud-and-tagging#task#3).

Cause 2 of 5: an explicitly configured encryptionConfiguration is returned without its type discriminator, and logging/tracing are still missing.
AWS: the recording shows encryptionConfiguration carrying __type 'com.amazonaws.swf.base.model#EncryptionConfiguration' and the defaulted loggingConfiguration {level: 'OFF', includeExecutionData: false} / tracingConfiguration {enabled: false} alongside it.
Emulator: encryptionConfiguration.__type absent and loggingConfiguration/tracingConfiguration absent.
Why: same class of breakage as cause 1 for a state machine that explicitly set encryption.
Fix: same projection/serializer.
Traces: sfn-encryption-and-logging-config-validation#task#7.

Cause 3 of 5: variableReferences is never returned for a JSONata state machine.
AWS: Smithy: DescribeStateMachine output variableReferences -> VariableReferences (map<StateName, VariableNameList>, sensitive). The recording shows AWS reporting, per state, the variable names that state references: {'Compose': ['greeting','target'], 'Emit': ['message','target']}.
Emulator: variableReferences absent (together with the three configuration structures of cause 1).
Why: a client that inspects which variables a state machine consumes (lint/deploy tooling) gets nothing, and cannot distinguish a JSONata definition with assignments from one without.
Fix: the DescribeStateMachine projection plus definition analysis for QueryLanguage=JSONata Assign/Output expressions.
Traces: sfn-jsonata-variable-references#task#3 (also sfn-jsonata-variable-references#task#5).

Cause 4 of 5: includedData is ignored, so a metadata-only describe leaks the definition.
AWS: Smithy: DescribeStateMachine input includedData -> IncludedData enum [ALL_DATA, METADATA_ONLY]; definition is marked sensitive. The recording shows the definition returned as '{}' for this step while the rest of the metadata is unchanged. Hypothesis: that step requests includedData=METADATA_ONLY, which blanks the definition.
Emulator: returns the full definition document verbatim (and again omits the three configuration structures).
Why: a caller that asks for metadata only still receives the sensitive state machine definition, and a caller that relies on '{}' to detect the metadata-only mode mis-detects it.
Fix: honour includedData in the DescribeStateMachine handler (blank the definition for METADATA_ONLY).
Traces: sfn-jsonata-variable-references#task#4.

Cause 5 of 5: an updated definition is not reflected and revisionId is never returned.
AWS: Smithy: DescribeStateMachine output definition (required) and revisionId -> RevisionId. The recording shows the definition the state machine was last updated to ('{"StartAt": "WaitState", "States": {"WaitState": {"Type": "Wait", "Seconds": 60, "End": true}}}') plus a revisionId that changes with each update.
Emulator: returns the original create-time definition (the pass-only document) and no revisionId, alongside the missing configuration structures.
Why: UpdateStateMachine appears to succeed but the state machine keeps running the old definition, and with no revisionId a client cannot detect or order revisions at all.
Fix: the UpdateStateMachine handler's state mutation (persist the new definition and bump a revision id) and the DescribeStateMachine projection.
Traces: states-state-machine-lifecycle-crud#task#6 (also stepfunctions-state-machine-crud-and-tagging#task#10, stepfunctions-version-alias-blue-green-rollout#task#17).

## Evidence
All five clusters answered HTTP 200 where 200 was expected; only the body diverges.
- sfn-state-machine-create-idempotent-repeat#task#6: expected encryptionConfiguration={__type:'com.amazonaws.swf.base.model#EncryptionConfiguration', type:'AWS_OWNED_KEY'}, loggingConfiguration={__type:'...#LoggingConfiguration', includeExecutionData:false, level:'OFF'}, tracingConfiguration={__type:'...#TracingConfiguration', enabled:false}; actual: all three None (13 affected steps).
- sfn-encryption-and-logging-config-validation#task#7: expected encryptionConfiguration.__type, loggingConfiguration, tracingConfiguration; actual None.
- sfn-jsonata-variable-references#task#3: expected variableReferences={'Compose':['greeting','target'],'Emit':['message','target']} plus the three configs; actual None.
- sfn-jsonata-variable-references#task#4: expected $.definition='{}'; actual the full JSONata definition document.
- states-state-machine-lifecycle-crud#task#6: expected $.definition='{"StartAt": "WaitState", ...Wait/Seconds 60...}' and $.revisionId present; actual the create-time pass-only definition and no revisionId.

## Steps to Reproduce
1. Replay fixture sfn-state-machine-create-idempotent-repeat, step task#6: CreateStateMachine with no logging/tracing/encryption options, then DescribeStateMachine -> none of the three configuration structures is present.
2. Replay sfn-jsonata-variable-references step task#3 (JSONata definition with Assign blocks) -> variableReferences missing; step task#4 (metadata-only describe) -> full definition returned instead of '{}'.
3. Replay states-state-machine-lifecycle-crud step task#6: UpdateStateMachine to the Wait definition, then DescribeStateMachine -> old definition, no revisionId.
