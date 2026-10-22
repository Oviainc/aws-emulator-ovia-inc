Issue-Source: trace-sweep-divergence
Issue-Trace: cfnrep-outputs-getatt-sub-export
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnrep-outputs-getatt-sub-export cloudformation-stack-create-update-delete-lifecycle cloudformation-stack-recovery-operation-errors cloudformation-stack-events-and-errors cloudformation-stack-lifecycle-basic-crud cloudformation-stack-policy-and-termination-protection cloudformation-stack-utils-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DeploymentConfig
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.LastOperations
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.NotificationARNs
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.RollbackConfiguration
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Tags
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Parameters
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.EnableTerminationProtection
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DriftInformation.LastCheckTimestamp
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DriftInformation.StackDriftStatus
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# DescribeStacks Stack serialization drops Outputs, Parameters, Tags and tracked flag state

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
SEVEN net-new causes, all in the DescribeStacks response path: the Stack member the emulator serializes is missing a fixed set of modeled members on every 200, and three members that ARE emitted carry stale/default values because the mutation that set them was never recorded on the stack. Status is 200 in all seven; the body is wrong.

Shared AWS reference — Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): DescribeStacks output Stacks -> list<Stack>; Stack members include Outputs (list<Output>), Parameters (list<Parameter>), Tags (list<Tag>), NotificationARNs (list, length 0..5), RollbackConfiguration, DeploymentConfig, LastOperations (list), EnableTerminationProtection (boolean) and DriftInformation (StackDriftInformation). AWS emits the empty collections as present-but-empty elements and populates the rest.

Cause 1 of 7 — Outputs, and the always-present empty members, missing (fixture cfnrep-outputs-getatt-sub-export, task#3).
AWS: for a CREATE_COMPLETE stack declaring three Outputs (a plain value, a Ref with Description+ExportName, and an Fn::Sub over GetAtt and pseudo-parameters), DescribeStacks returns Stacks.member.Outputs with one Output per declared output, plus NotificationARNs, RollbackConfiguration, Tags as empty elements, DeploymentConfig {Mode, DisableRollback} and LastOperations {OperationId, OperationType}. That Ref/GetAtt/Sub resolve to the provisioned resource's identifiers is recorded behavior only (resource attribute mapping is not modeled); the modeled part is that the members appear at all.
Emulator: Outputs, NotificationARNs, DeploymentConfig, RollbackConfiguration, LastOperations and Tags are all absent from the Stack.
Why: Outputs are the documented hand-off from a stack to its consumers, so every post-deploy lookup, cross-stack export and Sub/GetAtt-derived identifier is unobtainable even though the stack reports CREATE_COMPLETE. The absent empty collections additionally make SDK/strict-XML consumers see a different Stack shape than AWS's, and DeploymentConfig/LastOperations carry no operation history at all.
Fix: the DescribeStacks serializer plus stack-output evaluation and operation-history recording in the CloudFormation stack state (services/service-com-amazonaws-cloudformation-cloudformation): resolve declared Outputs against provisioned resources, always emit the empty collections, and populate DeploymentConfig/LastOperations.
Related open issue which does NOT cover this: 20261022T034308Z-cloudformation-service-catalog-appregist-cloudformation-stack-outputs-are-never-produced-a-create.md reports missing Outputs from the BEHAVIORAL corpus (an SNS-topic template, check id cloudformation__service-catalog-appregistry....). These wire clusters were not reported as owned by it, and they additionally pin the empty-collection, Parameters, LastOperations, termination-protection and drift members, so they are filed here.
Traces: cfnrep-outputs-getatt-sub-export#task#3.

Cause 2 of 7 — the same empty/default member set missing across a full create-update-delete lifecycle (cluster of 4 steps, incl. cloudformation-stack-create-update-delete-lifecycle#task#5 and #task#15 and cloudformation-stack-recovery-operation-errors#task#5). AWS: NotificationARNs, DeploymentConfig, RollbackConfiguration, LastOperations, Tags all present. Emulator: all six absent on every describe of the lifecycle. Why/Fix: as cause 1. Traces: cloudformation-stack-create-update-delete-lifecycle#task#3.

Cause 3 of 7 — same omission on the stack-events fixture's describe (NotificationARNs, DeploymentConfig, RollbackConfiguration, Tags). Why/Fix: as cause 1. Traces: cloudformation-stack-events-and-errors#task#7.

Cause 4 of 7 — Parameters list not returned. AWS: a stack created with Parameters=[{ParameterKey: TagValue, ParameterValue: initial}] reports them back under Stack.Parameters. Emulator: Parameters absent (together with the empty-collection set). Why: a client cannot read back the parameter values a stack was deployed with, so parameter-preserving updates and drift/diff tooling have nothing to read. Fix: persist resolved stack parameters in stack state and serialize Stack.Parameters. Traces: cloudformation-stack-lifecycle-basic-crud#task#3.

Cause 5 of 7 — EnableTerminationProtection not tracked after UpdateTerminationProtection. AWS: after termination protection is enabled, DescribeStacks reports EnableTerminationProtection true (and LastOperations OperationType UPDATE_STACK). Emulator: returns 'false' — the flag the mutation set is not reflected. Why: a client that gates destructive flows on the reported flag believes an explicitly protected stack is unprotected (and see the companion issue where DeleteStack also ignores it). Fix: store the termination-protection flag in stack state on UpdateTerminationProtection/CreateStack and serialize it. Traces: cloudformation-stack-lifecycle-basic-crud#task#13.

Cause 6 of 7 — DriftInformation never advances past NOT_CHECKED and EnableTerminationProtection stale. AWS: after a completed DetectStackDrift, Stack.DriftInformation is {StackDriftStatus: IN_SYNC, LastCheckTimestamp: <ts>}; the same stack reports EnableTerminationProtection true. Emulator: DriftInformation.StackDriftStatus 'NOT_CHECKED' with no LastCheckTimestamp, EnableTerminationProtection 'false', plus the missing empty-collection set. Why: drift results are unreadable from the stack summary, so a client cannot tell a drift-checked stack from an unchecked one. Fix: record drift-detection outcome (status + timestamp) onto stack state and serialize Stack.DriftInformation. Traces: cloudformation-stack-policy-and-termination-protection#task#11.

Cause 7 of 7 — Outputs missing again on a simple single-output stack (EnvOutput), with the same empty-collection omissions, confirming the gap is in the generic output-evaluation layer rather than one template form. Traces: cloudformation-stack-utils-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1; all seven clusters are verdict=mismatch with expected_status 200 and actual_status 200 (body-only divergence).

Cause 1 — cfnrep-outputs-getatt-sub-export#task#3:
  $.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs: missing (expected member list of 3 Outputs: TopicNameOut; TopicArn with Description 'Ref of the topic' and ExportName ...-topic-arn; Composite from an Fn::Sub over GetAtt + pseudo params), got None
  ...member.NotificationARNs: missing (expected '')
  ...member.DeploymentConfig: missing (expected {'Mode': 'STANDARD', 'DisableRollback': 'false'})
  ...member.RollbackConfiguration: missing (expected '')
  ...member.LastOperations: missing (expected {'member': {'OperationId': ..., 'OperationType': 'CREATE_STACK'}})
  ...member.Tags: missing (expected '')

Cause 2 — cloudformation-stack-create-update-delete-lifecycle#task#3 (cluster also covers #task#5, #task#15, cloudformation-stack-recovery-operation-errors#task#5): NotificationARNs, DeploymentConfig, RollbackConfiguration, LastOperations, Tags all 'missing'.

Cause 3 — cloudformation-stack-events-and-errors#task#7: NotificationARNs, DeploymentConfig, RollbackConfiguration, Tags 'missing'.

Cause 4 — cloudformation-stack-lifecycle-basic-crud#task#3: additionally
  ...member.Parameters: missing (expected {'member': {'ParameterKey': 'TagValue', 'ParameterValue': 'initial'}}), got None

Cause 5 — cloudformation-stack-lifecycle-basic-crud#task#13:
  ...member.EnableTerminationProtection: value_mismatch (expected 'true', got 'false')
  ...member.LastOperations: missing (expected OperationType 'UPDATE_STACK')

Cause 6 — cloudformation-stack-policy-and-termination-protection#task#11:
  ...member.DriftInformation.StackDriftStatus: value_mismatch (expected 'IN_SYNC', got 'NOT_CHECKED')
  ...member.DriftInformation.LastCheckTimestamp: missing
  ...member.EnableTerminationProtection: value_mismatch (expected 'true', got 'false')

Cause 7 — cloudformation-stack-utils-lifecycle#task#6:
  ...member.Outputs: missing (expected {'member': {'OutputKey': 'EnvOutput', 'OutputValue': 'probe'}}), got None

Generated ids, ARNs and timestamps in the expected documents are environment-specific examples; the requirement is the presence and population of the modeled Stack members.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateStack with a template declaring Outputs (e.g. fixture cfnrep-outputs-getatt-sub-export: a topic plus three Outputs using Ref, GetAtt and Fn::Sub) and poll to CREATE_COMPLETE.
3. Call DescribeStacks(StackName=...). Observe the Stack has no Outputs, NotificationARNs, RollbackConfiguration, Tags, DeploymentConfig or LastOperations members (AWS emits all six, the collections as empty elements).
4. CreateStack with Parameters=[{ParameterKey: TagValue, ParameterValue: initial}] (cloudformation-stack-lifecycle-basic-crud#task#3) and DescribeStacks: Stack.Parameters is absent.
5. Call UpdateTerminationProtection(EnableTerminationProtection=true) then DescribeStacks (cloudformation-stack-lifecycle-basic-crud#task#13 / cloudformation-stack-policy-and-termination-protection#task#11): EnableTerminationProtection still reports 'false'.
6. Run DetectStackDrift to completion then DescribeStacks: DriftInformation stays {StackDriftStatus: NOT_CHECKED} with no LastCheckTimestamp.
7. Re-replay the representative cfnrep-outputs-getatt-sub-export#task#3 and all six bindings in Issue-Trace-Ids to confirm the fix.
