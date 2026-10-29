Issue-Source: trace-sweep-divergence
Issue-Trace: cfnrep-drift-modified-property-differences
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnrep-drift-modified-property-differences cloudformation-stack-policy-and-termination-protection cloudformation-stack-drift-detection-workflow
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DetectStackResourceDrift|2xx->2xx|$.DetectStackResourceDriftResponse.DetectStackResourceDriftResult.StackResourceDrift.ActualProperties
Issue-Key: cloudformation|DetectStackResourceDrift|2xx->2xx|$.DetectStackResourceDriftResponse.DetectStackResourceDriftResult.StackResourceDrift.ExpectedProperties
Issue-Key: cloudformation|DetectStackResourceDrift|2xx->2xx|$.DetectStackResourceDriftResponse.DetectStackResourceDriftResult.StackResourceDrift.PropertyDifferences
Issue-Key: cloudformation|DescribeStackResourceDrifts|2xx->2xx|$.DescribeStackResourceDriftsResponse.DescribeStackResourceDriftsResult.StackResourceDrifts.member.ActualProperties
Issue-Key: cloudformation|DescribeStackResourceDrifts|2xx->2xx|$.DescribeStackResourceDriftsResponse.DescribeStackResourceDriftsResult.StackResourceDrifts.member.ExpectedProperties
Issue-Key: cloudformation|DescribeStackResourceDrifts|2xx->2xx|$.DescribeStackResourceDriftsResponse.DescribeStackResourceDriftsResult.StackResourceDrifts.member[*].ActualProperties
Issue-Key: cloudformation|DescribeStackResourceDrifts|2xx->2xx|$.DescribeStackResourceDriftsResponse.DescribeStackResourceDriftsResult.StackResourceDrifts.member[*].ExpectedProperties
Issue-Key: cloudformation|DescribeStackResourceDrifts|2xx->2xx|$.DescribeStackResourceDriftsResponse.DescribeStackResourceDriftsResult.StackResourceDrifts.member[*].PropertyDifferences
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[*].DriftInformation.StackResourceDriftStatus
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DriftInformation.LastCheckTimestamp
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DriftInformation.StackDriftStatus
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation drift reporting returns live resource attributes and never records drift status

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackResourceDrift

## Description
Drift detection answers 200 everywhere but its payload is not a drift report: ActualProperties is the resource's entire live attribute set instead of the template-declared properties, ExpectedProperties/PropertyDifferences are missing or key-ordered differently, and the stack- and resource-level drift status members never reflect that detection ran. 8 net-new root causes, one service, all in the drift comparator/serializers.

The replay reported owned_by=[] and no stub for all eight. The open issue "cloudformation + ec2: the CDK VPC diff/drift scenario never produces a terminal result ..." is a behavioral neighbour and carries none of these fixture#step bindings.

Cause 1 of 8: DetectStackResourceDrift's ActualProperties is the live attribute set, and ExpectedProperties is key-ordered differently.
AWS: Smithy: StackResourceDrift.ActualProperties / .ExpectedProperties (Properties — 'a JSON structure that contains the actual/expected property values of the stack resource'; 'only resource properties explicitly defined in the stack template are checked for drift'). So for a template-declared SNS topic the recorded ActualProperties contains only the template's properties with their live values, and ExpectedProperties contains the same property names with their template values; both are rendered with the property names in the same (sorted) order AWS uses.
Emulator: ActualProperties is the full SNS GetTopicAttributes attribute map (Policy, Owner, TopicArn, EffectiveDeliveryPolicy, SubscriptionsConfirmed/Pending/Deleted, ...), and ExpectedProperties carries the template's properties in template order rather than AWS's order.
Why: a client diffing ExpectedProperties against ActualProperties sees dozens of phantom additions on every resource, so drift output is unusable and never matches a recorded baseline.
Fix: the drift comparator — project the live resource onto the template-declared property set before serializing, and serialize both property maps in AWS's key order.
Traces: cfnrep-drift-modified-property-differences#task#4.

Cause 2 of 8: the same projection/ordering defect on an in-sync resource, plus PropertyDifferences omitted.
AWS: Smithy: StackResourceDrift.PropertyDifferences — 'present only for resources whose StackResourceDriftStatus is MODIFIED'. The recorded body for a steady resource still carries the member (as an empty list) together with the projected ActualProperties/ExpectedProperties.
Emulator: PropertyDifferences absent; ActualProperties is the live attribute map; ExpectedProperties key order differs.
Why: a client that reads PropertyDifferences unconditionally finds no member at all on in-sync resources.
Fix: the DetectStackResourceDrift serializer (emit the empty list) plus the comparator from cause 1.
Traces: cfnrep-drift-modified-property-differences#task#5.

Cause 3 of 8: DescribeStackResourceDrifts repeats the projection/ordering defect.
AWS: as cause 1, for each StackResourceDrifts entry.
Emulator: StackResourceDrifts.member.ActualProperties is the live SNS attribute map; ExpectedProperties key order differs.
Why/Fix: as cause 1 — this is a second serializer over the same drift records, so it does not follow from fixing DetectStackResourceDrift.
Traces: cfnrep-drift-modified-property-differences#task#9.

Cause 4 of 8: the projection defect again on the stack-policy scenario's DetectStackResourceDrift, with PropertyDifferences omitted.
AWS: as causes 1 and 2 — for a topic declared with only TopicName the recorded ActualProperties contains only that property.
Emulator: the full live SNS attribute map; PropertyDifferences absent.
Why/Fix: as cause 1.
Traces: cloudformation-stack-policy-and-termination-protection#task#9.

Cause 5 of 8: DetectStackResourceDrift omits ExpectedProperties and PropertyDifferences entirely for a resource with no declared properties.
AWS: both members are present in the recorded body — ExpectedProperties as an empty JSON object and PropertyDifferences as an empty list — for a resource whose template block declares no properties.
Emulator: both members absent.
Why: a client cannot distinguish 'no declared properties' from 'drift report incomplete'.
Fix: the DetectStackResourceDrift response serializer.
Traces: cloudformation-stack-drift-detection-workflow#task#7.

Cause 6 of 8: DescribeStackResourceDrifts omits the same two members per entry.
AWS: as cause 5, per StackResourceDrifts entry (and with the projected ActualProperties of cause 1).
Emulator: per-entry ExpectedProperties and PropertyDifferences absent; one entry's ActualProperties is the live SNS attribute map.
Why/Fix: as causes 1 and 5, in the DescribeStackResourceDrifts serializer.
Traces: cloudformation-stack-drift-detection-workflow#task#6.

Cause 7 of 8: ListStackResources reports resource drift as IN_SYNC before any detection has run.
AWS: Smithy: StackResourceDriftStatus NOT_CHECKED means 'CloudFormation hasn't checked'; a resource on which drift detection has not been performed is reported with DriftInformation.StackResourceDriftStatus = NOT_CHECKED.
Emulator: every resource summary reports IN_SYNC.
Why: a client reads a never-checked stack as verified in sync, so it skips the drift detection it should run.
Fix: the per-resource drift-information state (default NOT_CHECKED until a detection records a result) and the ListStackResources serializer.
Traces: cloudformation-stack-drift-detection-workflow#task#4.

Cause 8 of 8: DescribeStacks never records that stack drift detection ran.
AWS: after a DetectStackDrift operation completes, the stack's DriftInformation carries StackDriftStatus = IN_SYNC (for an undrifted stack) and a LastCheckTimestamp of the detection.
Emulator: StackDriftStatus stays NOT_CHECKED and LastCheckTimestamp is absent.
Why: the result of a completed drift detection is unobservable on the stack, so a client cannot tell whether detection ever succeeded — the exact inverse of cause 7.
Fix: the DetectStackDrift completion path must write the stack's drift information, and DescribeStacks must serialize LastCheckTimestamp.
Traces: cloudformation-stack-policy-and-termination-protection#task#11.

## Evidence
Replay of simparity-wire-aws-v1, 8 mismatch clusters, all 200->200:
- cfnrep-drift-modified-property-differences#task#4 DetectStackResourceDrift: StackResourceDrift.ActualProperties value_mismatch — expected '{"DisplayName":"...","TopicName":"..."}', got the full SNS attribute map ('{"Policy":"{Version:2008-10-17,...}","Owner":...,"SubscriptionsPending":...,"TopicArn":...,"EffectiveDeliveryPolicy":...,"SubscriptionsConfirmed":...,"DisplayName":...,"SubscriptionsDeleted":...}'); .ExpectedProperties value_mismatch — same two properties in a different key order.
- cfnrep-drift-modified-property-differences#task#5 DetectStackResourceDrift: .PropertyDifferences missing (expected an empty list); .ActualProperties/.ExpectedProperties as above.
- cfnrep-drift-modified-property-differences#task#9 DescribeStackResourceDrifts: StackResourceDrifts.member.ActualProperties / .ExpectedProperties the same divergence.
- cloudformation-stack-policy-and-termination-protection#task#9 DetectStackResourceDrift: .PropertyDifferences missing; .ActualProperties expected '{"TopicName":"..."}', got the full SNS attribute map.
- cloudformation-stack-drift-detection-workflow#task#7 DetectStackResourceDrift: .PropertyDifferences missing (expected ''), .ExpectedProperties missing (expected '{}').
- cloudformation-stack-drift-detection-workflow#task#6 DescribeStackResourceDrifts: member[0].PropertyDifferences/.ExpectedProperties missing; member[1].PropertyDifferences/.ExpectedProperties missing and member[1].ActualProperties expected '{}', got the full SNS attribute map.
- cloudformation-stack-drift-detection-workflow#task#4 ListStackResources: StackResourceSummaries.member[0]/[1].DriftInformation.StackResourceDriftStatus value_mismatch (expected 'NOT_CHECKED', got 'IN_SYNC').
- cloudformation-stack-policy-and-termination-protection#task#11 DescribeStacks: Stacks.member.DriftInformation.StackDriftStatus value_mismatch (expected 'IN_SYNC', got 'NOT_CHECKED'); .DriftInformation.LastCheckTimestamp missing.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Create a stack declaring an AWS::SNS::Topic with TopicName and DisplayName, mutate DisplayName out of band, then call DetectStackResourceDrift and DescribeStackResourceDrifts — ActualProperties is the topic's whole GetTopicAttributes map, ExpectedProperties is key-ordered differently, and PropertyDifferences/ExpectedProperties are absent on the in-sync cases.
3. Call ListStackResources on a stack on which no drift detection has run — DriftInformation.StackResourceDriftStatus is IN_SYNC instead of NOT_CHECKED.
4. Run DetectStackDrift to completion, then DescribeStacks — the stack's DriftInformation still says NOT_CHECKED and carries no LastCheckTimestamp.
