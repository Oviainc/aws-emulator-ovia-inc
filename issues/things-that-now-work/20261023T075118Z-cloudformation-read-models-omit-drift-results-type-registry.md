Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-stack-policy-and-termination-protection
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-stack-policy-and-termination-protection cloudformation-stack-resource-inspection cloudformation-type-registry-reads
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DriftInformation.LastCheckTimestamp
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DriftInformation.StackDriftStatus
Issue-Key: cloudformation|DescribeStackResources|2xx->2xx|$.DescribeStackResourcesResponse.DescribeStackResourcesResult.StackResources.member[*].PhysicalResourceId
Issue-Key: cloudformation|DescribeType|2xx->2xx|$.DescribeTypeResponse.DescribeTypeResult.SchemaHandlerPermissions
Issue-Key: cloudformation|DescribeType|2xx->2xx|$.DescribeTypeResponse.DescribeTypeResult.SourceUrl
Issue-Key: cloudformation|DescribeType|2xx->2xx|$.DescribeTypeResponse.DescribeTypeResult.TimeCreated
Issue-Commit: 6c57fad11ffc4b94eeb9d9eccf7474a9457dce26

# CloudFormation read models omit drift results, type-registry metadata and consistent physical resource ids (3 causes)

> _Note: this issue was identified against the code base at commit `6c57fad11ffc4b94eeb9d9eccf7474a9457dce26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three read paths return structurally incomplete stack/registry data after the write that should have populated them. 3 distinct causes, all in cloudformation. The open issue 20261022T043551Z-cloudformation-read-paths-return-unprocessed-or-unstored-stack.md covers GetTemplate/GetStackPolicy/DescribeStackEvents and does not carry any of the replay keys below; 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md is adjacent to cause 2 but likewise does not carry this fixture step.

Cause 1 of 3 - DescribeStacks still reports StackDriftStatus=NOT_CHECKED after drift detection completed (fixture cloudformation-stack-policy-and-termination-protection, step task#11).
AWS: once a drift-detection operation on the stack has completed, DescribeStacks reports Stacks.member.DriftInformation.StackDriftStatus as the detected result (IN_SYNC in this recording) and sets DriftInformation.LastCheckTimestamp to the time the detection was initiated. Smithy: StackDriftInformation.StackDriftStatus is a required enum (DRIFTED | NOT_CHECKED | IN_SYNC | UNKNOWN) and its doc states NOT_CHECKED means CloudFormation has not checked; LastCheckTimestamp is documented as the most recent time a drift detection operation was initiated on the stack.
Emulator: StackDriftStatus stays 'NOT_CHECKED' and LastCheckTimestamp is absent, so a completed detection leaves no trace in the stack summary.
Why: a client that runs DetectStackDrift and then polls DescribeStacks to learn the outcome never sees a result; drift dashboards and compliance gates read NOT_CHECKED forever and cannot tell 'in sync' from 'never checked'.
Fix: the drift-detection handler's state write plus the DescribeStacks projection of DriftInformation - the detection result and its timestamp must be persisted onto the stack and surfaced by the read path.
Traces: cloudformation-stack-policy-and-termination-protection#task#11.

Cause 2 of 3 - DescribeStackResources reports PhysicalResourceIds that do not match the ids the same run recorded for those logical resources (fixture cloudformation-stack-resource-inspection, step task#5).
AWS: StackResource.PhysicalResourceId is the name or unique identifier of the real underlying resource instance (Smithy: StackResource.PhysicalResourceId -> PhysicalResourceId, doc 'the name or unique identifier that corresponds to a physical instance ID of a resource supported by CloudFormation'), and it is the same value every other path reports for that logical id - which is why the fixture asserts it against the id captured earlier in the run ({{physical_id_ProbeBucket}}, {{physical_id_ProbeTopic}}) rather than against a literal.
Emulator: DescribeStackResources answers with locally synthesized names ('<stack>-ProbeBucket-5FFF7D8A8375', 'arn:aws:sns:...:<stack>-ProbeTopic-6FDA79C21053' in this recording) that do not agree with the physical ids the same run captured for those logical resources, so a resource's identity is not stable across the stack's read paths.
Why: a client reads PhysicalResourceId from a stack and then addresses that resource in S3/SNS; a value no other path agrees on makes every such hand-off fail. Generated id text itself is not the requirement - agreement between the paths is.
Fix: the stack's resource state - one physical id per logical resource, assigned when the resource is provisioned and returned identically by DescribeStackResources and the other resource read paths.
Traces: cloudformation-stack-resource-inspection#task#5.

Cause 3 of 3 - DescribeType omits SourceUrl, TimeCreated and SchemaHandlerPermissions (fixture cloudformation-type-registry-reads, step task#3).
AWS: DescribeType for a registered type returns SourceUrl and TimeCreated (Smithy: DescribeType output members SourceUrl -> OptionalSecureUrl, TimeCreated -> Timestamp) and, recorded behavior only, a SchemaHandlerPermissions member (not present in this model snapshot, emitted empty in the recording).
Emulator: all three members are absent from DescribeTypeResult.
Why: a client inspecting the registry cannot tell when a type version was created or where its source lives; code that reads TimeCreated to order versions gets a null.
Fix: the DescribeType handler/serializer and the type-registry state it reads - populate TimeCreated for every registered type and carry SourceUrl/SchemaHandlerPermissions through from the registry record. The recorded SourceUrl value is an example of a public type's source, not a required constant.
Traces: cloudformation-type-registry-reads#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

Cause 1 - cloudformation-stack-policy-and-termination-protection#task#11 DescribeStacks, status 200 vs 200: Stacks.member.DriftInformation.StackDriftStatus expected 'IN_SYNC', got 'NOT_CHECKED'; Stacks.member.DriftInformation.LastCheckTimestamp missing (expected a shaped timestamp, got None).
Cause 2 - cloudformation-stack-resource-inspection#task#5 DescribeStackResources, 200 vs 200: StackResources.member[0].PhysicalResourceId expected the run-captured {{physical_id_ProbeBucket}}, got 'sim2real-probe-cloudformation-stack-inspect-bf1d5b13-ProbeBucket-5FFF7D8A8375'; StackResources.member[1].PhysicalResourceId expected {{physical_id_ProbeTopic}}, got 'arn:aws:sns:us-west-2:000000000000:sim2real-probe-cloudformation-stack-inspect-bf1d5b13-ProbeTopic-6FDA79C21053'.
Cause 3 - cloudformation-type-registry-reads#task#3 DescribeType, 200 vs 200: SourceUrl missing (expected a source URL, got None); TimeCreated missing (expected a shaped timestamp, got None); SchemaHandlerPermissions missing (expected '', got None).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay cloudformation-stack-policy-and-termination-protection through step task#11 (drift detection on the stack, then DescribeStacks) and read Stacks.member.DriftInformation.
3. Replay cloudformation-stack-resource-inspection through step task#5 (DescribeStackResources) and compare each StackResources.member PhysicalResourceId with the id captured for that logical id earlier in the fixture.
4. Replay cloudformation-type-registry-reads step task#3 (DescribeType) and check for SourceUrl, TimeCreated and SchemaHandlerPermissions in DescribeTypeResult.
