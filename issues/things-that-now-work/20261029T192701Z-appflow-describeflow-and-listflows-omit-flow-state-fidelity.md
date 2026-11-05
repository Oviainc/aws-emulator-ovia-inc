Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-describe-flow-reports-task-and-source-config-fidelity
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-describe-flow-reports-task-and-source-config-fidelity appflow-list-flows-reflects-flow-lifecycle
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.createdBy
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.destinationFlowConfigList[*].destinationConnectorProperties.S3.s3OutputFormatConfig.prefixConfig
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.flowStatus
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.kmsArn
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.lastUpdatedBy
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.metadataCatalogConfig
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.tasks[*].taskProperties
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.triggerConfig.triggerProperties
Issue-Key: appflow|ListFlows|2xx->2xx|$.flows[*]
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# AppFlow DescribeFlow and ListFlows omit flow state fidelity

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlow

## Description
AppFlow's two read paths answer 200 but do not report the flow the write path created: DescribeFlow drops flow status, identity and whole optional sub-structures, and ListFlows does not return the flow at all. The shared fault is that the flow record held by the emulator's appflow state is not projected into the read responses. 2 causes.

Cause 1 of 2: DescribeFlow under-reports flow status, principal and optional structures.
AWS: Smithy: DescribeFlow output members include flowStatus (enum FlowStatus: Active|Deprecated|Deleted|Draft|Errored|Suspended, "Indicates the current status of the flow"), createdBy (CreatedBy, @pattern ^\\S+$, "The ARN of the user who created the flow"), lastUpdatedBy (UpdatedBy), kmsArn, metadataCatalogConfig (MetadataCatalogConfig{glueDataCatalog}), triggerConfig (TriggerConfig, with triggerProperties), tasks (Tasks, each with taskProperties map) and destinationFlowConfigList (each destinationConnectorProperties.S3.s3OutputFormatConfig.prefixConfig = PrefixConfig{prefixType, prefixFormat, pathPrefixHierarchy}). Recorded behavior only for the values: once a flow with a complete S3 source and S3 destination config has been created and activated, DescribeFlow reports flowStatus "Active"; the optional nested structures and maps are PRESENT in the body with their own members null/empty (prefixConfig{prefixType,prefixFormat,pathPrefixHierarchy}, metadataCatalogConfig{glueDataCatalog}, triggerConfig.triggerProperties{Scheduled}, tasks[].taskProperties{}) and kmsArn is present; createdBy and lastUpdatedBy both carry the calling IAM principal's ARN in the form arn:aws:iam::<account>:user/<principal-name>.
Emulator: 200, but flowStatus "Draft" for an activated flow; prefixConfig, metadataCatalogConfig, triggerConfig.triggerProperties, tasks[0].taskProperties and kmsArn are all absent (serialized as null / omitted); createdBy and lastUpdatedBy render a synthesized principal name (arn:aws:iam::<account>:user/EMULATOR<hex-suffix>) instead of the caller's principal, so only the principal-name segment of the ARN diverges - the arn form and account are right.
Why: clients poll DescribeFlow's flowStatus to decide a flow is usable; a flow stuck at "Draft" after activation makes every status-gated workflow (wait-for-Active, start-then-verify) hang or bail out. SDK consumers that navigate into the optional structures - destinationFlowConfigList[0].destinationConnectorProperties.s3().s3OutputFormatConfig().prefixConfig(), triggerConfig.triggerProperties(), tasks[].taskProperties() - get a null where AWS always hands back a structure, so a fully configured destination reads as unconfigured and attribute lookups on it fail. A non-caller principal in createdBy/lastUpdatedBy breaks ownership and audit assertions.
Fix: the appflow DescribeFlow handler and its response serializer (emit the optional sub-structures/maps rather than eliding empty ones, and resolve createdBy/lastUpdatedBy from the request's caller identity), plus the appflow flow state machine that owns the CreateFlow/StartFlow status transition into Active.
Traces: appflow-describe-flow-reports-task-and-source-config-fidelity#task#7.

Cause 2 of 2: ListFlows does not enumerate an existing flow.
AWS: Smithy: ListFlows output flows is a FlowList of FlowDefinition{flowArn, flowName, flowStatus, sourceConnectorType, destinationConnectorType, triggerType, createdAt, lastUpdatedAt, createdBy, lastUpdatedBy, ...}; the operation is paginated on nextToken/maxResults (maxResults @range 1..100). Every flow that exists in the account and has not been deleted is enumerated, so a flow just created by CreateFlow appears as a FlowDefinition entry carrying its own name, its connector types and its current status (recorded: flows[0] = the created flow, sourceConnectorType "S3", destinationConnectorType "S3", triggerType "OnDemand", flowStatus "Active").
Emulator: 200 with no entry at index 0 - flows is empty/absent, so a flow created earlier in the same session is not enumerated at all.
Why: list-then-act clients cannot discover a flow they just created; existence checks over ListFlows report the flow as missing, idempotent create paths re-create it, and console-style enumeration shows an empty account.
Fix: the appflow ListFlows handler and the shared appflow flow store it reads - ListFlows must project the same records CreateFlow wrote into FlowDefinition entries.
Traces: appflow-list-flows-reflects-flow-lifecycle#task#7.

## Evidence
Cause 1 (appflow-describe-flow-reports-task-and-source-config-fidelity#task#7, DescribeFlow): expected status 200, actual 200, body diverges on 8 keys - $.flowStatus expected "Active", got "Draft"; $.destinationFlowConfigList[0].destinationConnectorProperties.S3.s3OutputFormatConfig.prefixConfig expected the structure {pathPrefixHierarchy, prefixFormat, prefixType}, got null; $.metadataCatalogConfig expected {glueDataCatalog}, got null; $.triggerConfig.triggerProperties expected {Scheduled}, got null; $.tasks[0].taskProperties expected {} (empty map present), got null; $.kmsArn expected present, got null; $.createdBy and $.lastUpdatedBy expected the caller principal ARN arn:aws:iam::<account>:user/<principal>, got arn:aws:iam::<account>:user/EMULATOR<hex>.
Cause 2 (appflow-list-flows-reflects-flow-lifecycle#task#7, ListFlows): expected status 200, actual 200; $.flows[0] expected a FlowDefinition for the created flow (flowName, sourceConnectorType S3, destinationConnectorType S3, triggerType OnDemand, flowStatus Active), got null - the list is empty.
Model basis: Smithy DescribeFlow output members flowStatus/createdBy/lastUpdatedBy/kmsArn/metadataCatalogConfig/triggerConfig/tasks/destinationFlowConfigList; FlowStatus enum includes Active and Draft; ListFlows output flows: FlowList of FlowDefinition.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: create a flow with a complete S3 source and S3 destination config (including s3OutputFormatConfig), activate it, then POST /describe-flow with its flowName. Observe flowStatus "Draft" instead of "Active", null for prefixConfig / metadataCatalogConfig / triggerConfig.triggerProperties / tasks[0].taskProperties / kmsArn, and a synthesized user/EMULATOR<hex> principal in createdBy and lastUpdatedBy. Fixture appflow-describe-flow-reports-task-and-source-config-fidelity, step task#7.
3. Cause 2: after the same CreateFlow, POST /list-flows with no nextToken. Observe an empty flows list rather than a FlowDefinition entry for the flow just created. Fixture appflow-list-flows-reflects-flow-lifecycle, step task#7.
