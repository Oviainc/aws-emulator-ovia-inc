Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-stack-lifecycle-basic-crud
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-stack-lifecycle-basic-crud cfnrep-changeset-replacement-preview cloudformation-changeset-template-url-update-diff cloudformation-change-set-create-and-execute cloudformation-stack-events-and-errors cloudformation-stack-create-update-delete-lifecycle cloudformation-stack-resource-inspection
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Parameters.member.ParameterValue
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.StackStatus
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member[*]
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member[*].ResourceChange.Details.member[*].CausingEntity
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member[*].ResourceChange.Details.member[*].ChangeSource
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member[*].ResourceChange.Details.member[*].Evaluation
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*]
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].EventType
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].LogicalResourceId
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].PhysicalResourceId
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].ResourceProperties
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].ResourceStatus
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].ResourceStatusReason
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents.member[*].ResourceType
Issue-Key: cloudformation|DescribeStackResource|2xx->2xx|$.DescribeStackResourceResponse.DescribeStackResourceResult.StackResourceDetail.Metadata
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation change-set previews, operation events and the update lifecycle diverge from the recording

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
A plain parameter-only UpdateStack rolls itself back, and the two surfaces a client uses to understand an operation — the change-set preview and the operation event stream — report different content, ordering and enum values from the recording. 7 net-new root causes, one service.

The replay reported owned_by=[] and no stub for all seven. The open issues "CloudFormation never rolls a failed stack back: failed creates/updates end COMPLETE or park in *_FAILED ...", "CloudFormation emits no per-resource stack events, and resource-property (schema) validation is never performed (2 causes)" and "CloudFormation stack events and resource views are incomplete and physical resource ids are synthesized names ..." are the nearest neighbours and carry none of these fixture#step bindings — and causes 4/5 here are the opposite symptom: events ARE emitted, with the wrong type, order and content.

Cause 1 of 7: UpdateStack rolls back a successful parameter-only update.
AWS: an UpdateStack that only changes a parameter value on a healthy stack ends UPDATE_COMPLETE, and DescribeStacks then reports the stack's Parameters with the NEW ParameterValue.
Emulator: DescribeStacks reports StackStatus UPDATE_ROLLBACK_COMPLETE and the parameter still at its previous value; the poll terminates on the rollback status.
Why: no stack can be updated — every update reverts, so a client's deploy appears to succeed as an API call and then silently un-applies.
Fix: the UpdateStack execution path (whatever marks the update failed) and the stack parameter state it writes.
Traces: cloudformation-stack-lifecycle-basic-crud#task#13.

Cause 2 of 7: DescribeChangeSet(IncludePropertyValues) omits the before/after context and the dynamic change detail.
AWS: Smithy: DescribeChangeSet output Changes (Changes list) with IncludePropertyValues on the input. For a Modify change the recorded ResourceChange carries BeforeContext and AfterContext (JSON documents of the resource's properties before and after, with unresolved values rendered as {{changeSet:KNOWN_AFTER_APPLY}}) and TWO Details entries — one with Evaluation Dynamic / ChangeSource DirectModification and one with Evaluation Static / ChangeSource ResourceAttribute — each Target carrying Path, AttributeChangeType, BeforeValue, Name and Attribute.
Emulator: BeforeContext and AfterContext absent; a single Details entry whose Target carries only Attribute, Name and RequiresRecreation (no Path, AttributeChangeType, BeforeValue or AfterValue).
Why: the change-set preview is the artefact a reviewer approves before executing; without before/after context and the per-property target paths the preview does not show what will change.
Fix: the change-set diff builder and the DescribeChangeSet serializer.
Traces: cfnrep-changeset-replacement-preview#task#6.

Cause 3 of 7: DescribeChangeSet classifies change details the wrong way round.
AWS: for a property driven by a parameter reference, the recorded Details entries are ordered and classified as Evaluation Dynamic / ChangeSource DirectModification first, then Evaluation Static / ChangeSource ParameterReference with CausingEntity = the referenced parameter's logical name.
Emulator: the first entry is Static/ParameterReference and the second Dynamic/DirectModification, and the second entry has no CausingEntity.
Why: a reviewer (or policy gate) that reads Evaluation/ChangeSource to decide whether a change is safe reads the inverse classification, and cannot see which parameter causes the change.
Fix: the change-set detail classifier (evaluation/change-source assignment and CausingEntity attribution).
Traces: cfnrep-changeset-replacement-preview#task#7.

Cause 4 of 7: DescribeChangeSet omits PolicyAction on a Remove change and emits a synthesized physical id.
AWS: for a resource being removed the recorded ResourceChange carries PolicyAction (e.g. ReplaceAndDelete/Delete, derived from the resource's DeletionPolicy/UpdateReplacePolicy) and a PhysicalResourceId that is the real resource's id.
Emulator: PolicyAction absent and PhysicalResourceId is a stack-name-derived synthesized name.
Why: a reviewer cannot see whether the removal will delete or retain the resource — the single most consequential field in a destructive preview.
Fix: the change-set diff builder's policy-action derivation.
Traces: cloudformation-changeset-template-url-update-diff#task#9.

Cause 5 of 7: DescribeEvents emits the wrong EventType and the wrong event order/content for a change-set execution.
AWS: recorded behavior only for the enum value — every event of the operation carries EventType 'PROGRESS' (a value the Smithy EventType enum does not list; it declares STACK_EVENT, PROGRESS_EVENT, VALIDATION_ERROR, PROVISIONING_ERROR, HOOK_INVOCATION_ERROR). The recorded stream also leads with the stack-level events (LogicalResourceId = the stack name, ResourceType AWS::CloudFormation::Stack, PhysicalResourceId = the stack ARN) before the per-resource ones, carries ResourceProperties on resource events (the properties used to create the resource, '{}' where none), and carries ResourceStatusReason 'Resource creation Initiated' on the initiation event.
Emulator: EventType 'PROGRESS_EVENT'/'STACK_EVENT'; the stack-level and resource-level events are interleaved in the opposite order (member[1] is the AWS::S3::Bucket resource event where the recording has the stack event); ResourceProperties and ResourceStatusReason are absent; one PhysicalResourceId is the empty string; and the resource statuses at each index differ (CREATE_IN_PROGRESS where the recording has CREATE_COMPLETE).
Why: a client that follows an operation through DescribeEvents sees a different event type vocabulary, a different ordering and no resource properties, so progress and failure attribution are wrong.
Fix: the operation-event emitter (event type value, emission order, resource-properties/status-reason population) and the DescribeEvents serializer.
Traces: cloudformation-change-set-create-and-execute#task#8.

Cause 6 of 7: DescribeEvents diverges the same way for a plain create.
AWS: as cause 5 — EventType 'PROGRESS' throughout, stack-level events first, ResourceProperties carrying the resource's declared properties (e.g. {"Type":"String","Value":"<the parameter value from the template>"} for an AWS::SSM::Parameter).
Emulator: EventType PROGRESS_EVENT/STACK_EVENT; the stack event and the resource event appear at swapped indices; ResourceProperties absent; trailing events missing entirely.
Why/Fix: as cause 5 — this is a second recorded scenario over the same emitter, listed separately because it also shows the missing trailing events and the missing ResourceProperties on a non-S3 resource type.
Traces: cloudformation-stack-events-and-errors#task#5.

Cause 7 of 7: DescribeStackResource omits StackResourceDetail.Metadata.
AWS: Smithy: StackResourceDetail member Metadata — the recorded body always carries it, as '{}' for a resource whose template block declares no Metadata.
Emulator: Metadata absent.
Why: a client reading a resource's Metadata (for cfn-init and similar bootstrap data) finds no member at all rather than an empty document.
Fix: the DescribeStackResource response serializer.
Traces: cloudformation-stack-create-update-delete-lifecycle#task#7, cloudformation-stack-resource-inspection#task#3, cloudformation-stack-resource-inspection#task#6.

## Evidence
Replay of simparity-wire-aws-v1, 7 mismatch clusters, all 200->200:
- cloudformation-stack-lifecycle-basic-crud#task#13 DescribeStacks: Stacks.member.StackStatus value_mismatch (expected 'UPDATE_COMPLETE', got 'UPDATE_ROLLBACK_COMPLETE', and the poll hit 'UPDATE_ROLLBACK_COMPLETE' in fail_on after 2 attempts); Stacks.member.Parameters.member.ParameterValue value_mismatch (expected 'updated', got 'initial').
- cfnrep-changeset-replacement-preview#task#6 DescribeChangeSet: Changes.member[0]/[1] value_mismatch — expected ResourceChange with BeforeContext/AfterContext ('{{changeSet:KNOWN_AFTER_APPLY}}' for the unresolved value) and two Details entries (Dynamic/DirectModification and Static/ResourceAttribute) whose Targets carry Path, AttributeChangeType, BeforeValue, AfterValue, Name, Attribute; got a ResourceChange with no BeforeContext/AfterContext and a single Details entry whose Target carries only Attribute, Name, RequiresRecreation.
- cfnrep-changeset-replacement-preview#task#7 DescribeChangeSet: Details.member[0].Evaluation expected 'Dynamic', got 'Static'; member[0].ChangeSource expected 'DirectModification', got 'ParameterReference'; member[1].Evaluation expected 'Static', got 'Dynamic'; member[1].ChangeSource expected 'ParameterReference', got 'DirectModification'; member[1].CausingEntity missing (expected 'TopicNameParam').
- cloudformation-changeset-template-url-update-diff#task#9 DescribeChangeSet: Changes.member[1] expected ResourceChange with PolicyAction 'Delete' and a shaped PhysicalResourceId; got one with no PolicyAction and PhysicalResourceId '<stack>-Removed-ACC116284F0F'.
- cloudformation-change-set-create-and-execute#task#8 DescribeEvents: OperationEvents.member[1].EventType expected 'PROGRESS', got 'PROGRESS_EVENT'; member[1].LogicalResourceId expected the stack name, got 'Bucket'; member[1].ResourceType expected 'AWS::CloudFormation::Stack', got 'AWS::S3::Bucket'; member[1].PhysicalResourceId expected the stack ARN, got a synthesized bucket name; member[2].ResourceProperties missing (expected '{}'); member[2].ResourceStatus expected 'CREATE_COMPLETE', got 'CREATE_IN_PROGRESS'; member[3].ResourceStatusReason missing (expected 'Resource creation Initiated'); member[3].PhysicalResourceId got ''.
- cloudformation-stack-events-and-errors#task#5 DescribeEvents: member[1]/[4] LogicalResourceId, PhysicalResourceId, ResourceType and EventType swapped between the stack event and the 'ProbeParam' resource event; member[4].ResourceProperties missing (expected '{"Type":"String","Value":"probe-value"}'); member[5]/[6] missing entirely; every EventType expected 'PROGRESS'.
- cloudformation-stack-create-update-delete-lifecycle#task#7 DescribeStackResource: StackResourceDetail.Metadata missing (expected '{}') — 3 affected steps.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Create a stack with a parameter, UpdateStack changing only that parameter's value, then poll DescribeStacks — the stack reaches UPDATE_ROLLBACK_COMPLETE with the old ParameterValue instead of UPDATE_COMPLETE with the new one.
3. CreateChangeSet for a modify-and-remove update, then DescribeChangeSet with IncludePropertyValues=true — BeforeContext/AfterContext, the Dynamic/DirectModification detail entry, the Target Path/AttributeChangeType/Before+AfterValue, CausingEntity and PolicyAction are missing or inverted.
4. Execute the change set (and separately create a plain stack), then call DescribeEvents for the operation — EventType is PROGRESS_EVENT/STACK_EVENT instead of PROGRESS, the stack- and resource-level events are in the wrong order, and ResourceProperties/ResourceStatusReason are absent.
5. DescribeStackResource on any stack resource — StackResourceDetail.Metadata is absent instead of '{}'.
