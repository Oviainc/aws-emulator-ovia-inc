Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__medialive.deploy-medialive-cloudwatch-alarm-template-group-and-template-round-trip-and-delete cloudformation__medialive.deploy-medialive-cloudwatch-alarm-template-invalid-comparison-operator-rejected-and-rolled-back cloudformation__medialive.deploy-medialive-network-round-trip-and-delete cloudformation__medialive.deploy-medialive-cluster-and-channel-placement-group-round-trip-and-delete cloudformation__medialive.deploy-medialive-eventbridge-rule-template-group-and-template-round-trip-and-delete cloudformation__medialive.deploy-medialive-eventbridge-rule-template-invalid-event-type-rejected-and-rolled-back cloudformation__medialive.deploy-medialive-sdi-source-round-trip-and-delete cloudformation__medialive.deploy-medialive-signal-map-round-trip-and-delete cloudformation__medialive.deploy-medialive-channel-round-trip-and-delete
Issue-Key: cloudformation__medialive|CreateStack|ValidationError
Issue-Key: cloudformation__medialive.deploy-medialive-channel-round-trip-and-delete|run#14|AssertionError|assert inputs == [(out["InputRef"], out["InputArn"])], f"MediaLive lists inputs {inputs!r}"
Issue-Commit: 5386ff6290ae18fab3ce660917360d5fb6557b5e

# cloudformation__medialive: 6 net-new causes — CreateStack refuses every AWS::MediaLive::* type beyond Channel/Input as unrecognized, and a CREATE_COMPLETE AWS::MediaLive::Input does not exist in MediaLive

> _Note: this issue was identified against the code base at commit `5386ff6290ae18fab3ce660917360d5fb6557b5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Six net-new root causes in the CloudFormation resource-type coverage for MediaLive. Causes 1-5 are each a distinct set of MediaLive resource types that the template-validation resource-type registry does not know, so CreateStack refuses the whole template at validation time (ValidationError, "Template format error: Unrecognized resource types: [...]") and no resource of that type can ever be provisioned, read back, or rolled back. Cause 6 is a type the registry DOES accept but never provisions. None of these are covered by an existing issue; the closest open issues are the same-shaped registry gaps for other services (e.g. 20261109T113214Z-cloudformation-mediatailor-createstack-refuses-every-aws-mediatailor-resource.md, 20261109T113512Z-cloudformation-network-firewall-aws-networkfirewall-firewallpolicy-and-rulegroup.md) and they do not name any AWS::MediaLive::* type.

Cause 1 of 6: AWS::MediaLive::CloudWatchAlarmTemplateGroup and AWS::MediaLive::CloudWatchAlarmTemplate are not registered resource types.
AWS: both types are supported CloudFormation resource types. A template declaring a group plus a template whose GroupIdentifier is {"Ref": <group>} reaches CREATE_COMPLETE; CFN provisions them with MediaLive CreateCloudWatchAlarmTemplateGroup and CreateCloudWatchAlarmTemplate (Smithy: CreateCloudWatchAlarmTemplate POST /prod/cloudwatch-alarm-templates, 201; required members ComparisonOperator, EvaluationPeriods, GroupIdentifier, MetricName, Name, Period, Statistic, TargetResourceType, Threshold, TreatMissingData), Ref yields the template id and GetAtt Arn the medialive:cloudwatch-alarm-template ARN, and GetCloudWatchAlarmTemplate/ListCloudWatchAlarmTemplates read the templated values back. A ComparisonOperator outside the enum CloudWatchAlarmTemplateComparisonOperator must NOT be refused at template parse: CFN fails that resource in early property validation and the stack ends ROLLBACK_COMPLETE, with a validation row whose ResourceName is the logical id, type PROPERTY_VALIDATION, status FAILED, action FAIL, path "/Resources/<logical id>/Properties/ComparisonOperator" and the fixed-text message "<value> is not a valid enum value. Supported values: [<the enum's members>]" (the value and the member list are taken from the request and the model; only the fixed wording is literal), and the rolled-back group/template are absent from ListCloudWatchAlarmTemplateGroups/ListCloudWatchAlarmTemplates.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MediaLive::CloudWatchAlarmTemplateGroup, AWS::MediaLive::CloudWatchAlarmTemplate]" and no stack is created at all, so neither the happy-path round trip nor the property-validation rollback can be observed.
Why: a client cannot deploy MediaLive monitoring templates from a template at all, and an IaC pipeline that relies on CFN's early property validation to reject a bad enum sees a hard template-format refusal of the entire stack instead of a per-resource PROPERTY_VALIDATION failure plus rollback.
Fix: the CloudFormation template-validation resource-type registry (the table of known AWS::<service>::<type> names) plus a provisioner for each type that calls the corresponding MediaLive create/delete operation and surfaces Ref/GetAtt Arn.
Traces: cloudformation__medialive.deploy-medialive-cloudwatch-alarm-template-group-and-template-round-trip-and-delete, cloudformation__medialive.deploy-medialive-cloudwatch-alarm-template-invalid-comparison-operator-rejected-and-rolled-back.

Cause 2 of 6: AWS::MediaLive::Network, AWS::MediaLive::Cluster and AWS::MediaLive::ChannelPlacementGroup are not registered resource types.
AWS: all three are supported CloudFormation resource types. A Network template reaches CREATE_COMPLETE, Ref yields the network id and GetAtt Arn its ARN, and DescribeNetwork/ListNetworks report the templated IpPools and Routes with State ACTIVE (Smithy: CreateNetwork POST /prod/networks, 201, output Id/Arn/IpPools/Routes/State). A cluster template that also declares a channel placement group provisions MediaLive CreateCluster (POST /prod/clusters, 201, output Id/Arn/ClusterType/NetworkSettings/State) and CreateChannelPlacementGroup (POST /prod/clusters/{ClusterId}/channelplacementgroups, 201, output Id/Arn/ClusterId/Nodes/State), with the group's ClusterId resolved from the cluster resource's Ref; the stack delete removes them in dependency order.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MediaLive::Network]" for the network template and "...: [AWS::MediaLive::Network, AWS::MediaLive::Cluster, AWS::MediaLive::ChannelPlacementGroup]" for the cluster template; no stack is created.
Why: no MediaLive Anywhere estate (network, cluster, placement group) can be deployed or torn down through CloudFormation, so every IaC path to MediaLive Anywhere is dead.
Fix: the CloudFormation resource-type registry plus provisioners for these three types wired to MediaLive CreateNetwork/CreateCluster/CreateChannelPlacementGroup and their deletes.
Traces: cloudformation__medialive.deploy-medialive-network-round-trip-and-delete, cloudformation__medialive.deploy-medialive-cluster-and-channel-placement-group-round-trip-and-delete.

Cause 3 of 6: AWS::MediaLive::EventBridgeRuleTemplateGroup and AWS::MediaLive::EventBridgeRuleTemplate are not registered resource types.
AWS: both are supported CloudFormation resource types; CFN provisions them with MediaLive CreateEventBridgeRuleTemplateGroup and CreateEventBridgeRuleTemplate (Smithy: CreateEventBridgeRuleTemplate POST /prod/eventbridge-rule-templates, 201; required EventType, GroupIdentifier, Name), the stack reaches CREATE_COMPLETE, Ref is the template id, GetAtt Arn the medialive:eventbridge-rule-template ARN, and GetEventBridgeRuleTemplate reads EventType/EventTargets back. An EventType outside the enum EventBridgeRuleTemplateEventType fails that resource's early property validation and rolls the stack back to ROLLBACK_COMPLETE with a PROPERTY_VALIDATION row naming /Resources/<logical id>/Properties/EventType — not a template-format refusal.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MediaLive::EventBridgeRuleTemplateGroup, AWS::MediaLive::EventBridgeRuleTemplate]"; no stack exists, so neither the round trip nor the enum rejection/rollback happens.
Why: MediaLive EventBridge rule templates cannot be deployed from a template, and the stack-level rejection shape a pipeline keys on (per-resource PROPERTY_VALIDATION + ROLLBACK_COMPLETE) is replaced by an outright CreateStack refusal.
Fix: the CloudFormation resource-type registry plus provisioners for the two types wired to the MediaLive eventbridge-rule-template create/update/delete operations.
Traces: cloudformation__medialive.deploy-medialive-eventbridge-rule-template-group-and-template-round-trip-and-delete, cloudformation__medialive.deploy-medialive-eventbridge-rule-template-invalid-event-type-rejected-and-rolled-back.

Cause 4 of 6: AWS::MediaLive::SdiSource is not a registered resource type.
AWS: a supported CloudFormation resource type. The stack reaches CREATE_COMPLETE, Ref yields the SdiSource id and GetAtt Arn its ARN, and DescribeSdiSource/ListSdiSources report the templated Mode and Type with State IDLE (Smithy: CreateSdiSource POST /prod/sdiSources, output SdiSource); deleting the stack deletes the source.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MediaLive::SdiSource]"; no stack is created.
Why: an SDI source cannot be provisioned or decommissioned through CloudFormation, so templates that model SDI inputs cannot deploy.
Fix: the CloudFormation resource-type registry plus an AWS::MediaLive::SdiSource provisioner calling MediaLive CreateSdiSource/DeleteSdiSource.
Traces: cloudformation__medialive.deploy-medialive-sdi-source-round-trip-and-delete.

Cause 5 of 6: AWS::MediaLive::SignalMap is not a registered resource type.
AWS: a supported CloudFormation resource type. The stack reaches CREATE_COMPLETE, Ref yields the signal map id and GetAtt Arn the medialive:signal-map ARN, and GetSignalMap/ListSignalMaps report the templated Name, Description and DiscoveryEntryPointArn (Smithy: CreateSignalMap POST /prod/signal-maps, 201; required DiscoveryEntryPointArn and Name); the stack delete removes the map.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MediaLive::SignalMap]"; no stack is created.
Why: signal maps — the entry point for MediaLive monitoring — cannot be deployed from a template.
Fix: the CloudFormation resource-type registry plus an AWS::MediaLive::SignalMap provisioner calling MediaLive CreateSignalMap/DeleteSignalMap.
Traces: cloudformation__medialive.deploy-medialive-signal-map-round-trip-and-delete.

Cause 6 of 6: an AWS::MediaLive::Input declared by a CREATE_COMPLETE stack does not exist in MediaLive.
AWS: CFN provisions AWS::MediaLive::Input by calling MediaLive CreateInput (Smithy: CreateInput POST /prod/inputs, 201, output Input) with the template's Name, Type and Sources, so once the stack is CREATE_COMPLETE the input is a real MediaLive input: ListInputs contains exactly one entry whose Name is the template's Name, whose Id equals the resource's Ref and whose Arn equals GetAtt Arn, and DescribeInput reports that Name, Type (e.g. MP4_FILE) and the source URLs. Deleting the stack leaves the input readable as a DELETED tombstone.
Emulator: the stack reaches CREATE_COMPLETE and resolves Outputs for Ref and GetAtt Arn of the Input resource, but MediaLive ListInputs contains no input with the template's Name — the replay asserts the single (Id, Arn) pair and gets [] ("AssertionError: MediaLive lists inputs []"). The physical id/ARN the stack reports therefore names nothing in MediaLive, and the channel, DescribeInput and tombstone assertions behind it are unreachable.
Why: a client that deploys a channel+input stack and then drives MediaLive by the stack's Outputs gets a Ref/Arn that no MediaLive read recognizes; the stack claims success while the estate is empty, which is worse than a failed create because nothing signals the gap.
Fix: the CloudFormation provisioner for AWS::MediaLive::Input — it must create the input through the MediaLive service state (CreateInput) and use the returned id/ARN as the resource's physical id and Arn attribute, rather than synthesizing them.
Traces: cloudformation__medialive.deploy-medialive-channel-round-trip-and-delete.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (service heading `### cloudformation__medialive`, 0/9 passed).

Cause 1 — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaLive::CloudWatchAlarmTemplateGroup, AWS::MediaLive::CloudWatchAlarmTemplate]
  Expected (happy path): stack CREATE_COMPLETE; GetCloudWatchAlarmTemplate(Identifier=<Ref>) reads (Name=<templated name>, ComparisonOperator=GreaterThanThreshold).
  Expected (negative): stack ROLLBACK_COMPLETE with validation row (<logical id>, PROPERTY_VALIDATION, FAILED, FAIL, "<value> is not a valid enum value. Supported values: [<enum members>]", "/Resources/<logical id>/Properties/ComparisonOperator") — fixed text is "is not a valid enum value. Supported values: [...]"; the value and member list come from the request and the model.

Cause 2 — ClientError (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::MediaLive::Network]; and: [AWS::MediaLive::Network, AWS::MediaLive::Cluster, AWS::MediaLive::ChannelPlacementGroup]
  Expected: CREATE_COMPLETE; DescribeNetwork/DescribeCluster/DescribeChannelPlacementGroup report the templated settings with State ACTIVE/ACTIVE/ASSIGNED respectively and Ref/GetAtt Arn match.

Cause 3 — ClientError (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::MediaLive::EventBridgeRuleTemplateGroup, AWS::MediaLive::EventBridgeRuleTemplate]
  Expected: CREATE_COMPLETE for the valid EventType, and ROLLBACK_COMPLETE with a PROPERTY_VALIDATION row on /Resources/<logical id>/Properties/EventType for an out-of-enum EventType.

Cause 4 — ClientError (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::MediaLive::SdiSource]
  Expected: CREATE_COMPLETE; DescribeSdiSource reports the templated Mode/Type.

Cause 5 — ClientError (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::MediaLive::SignalMap]
  Expected: CREATE_COMPLETE; GetSignalMap reports the templated Name/DiscoveryEntryPointArn.

Cause 6 — stack reached CREATE_COMPLETE and Outputs InputRef/InputArn resolved, then:
  File "check_cloudformation__medialive_deploy_medialive_channel_round_trip_and_delete.py", line 237, in run
    assert inputs == [(out["InputRef"], out["InputArn"])], f"MediaLive lists inputs {inputs!r}"
  AssertionError: MediaLive lists inputs []
  Expected: exactly one ListInputs entry whose Name is the template's Input Name, with Id == Ref and Arn == GetAtt Arn; DescribeInput then reports (Name, Type=MP4_FILE, Sources[*].Url) as templated, and after the stack delete DescribeChannel/DescribeInput both report State DELETED.

## Steps to Reproduce
1. Point boto3 at the emulator.
2. Cause 1: cfn.create_stack with a template whose Resources declare AWS::MediaLive::CloudWatchAlarmTemplateGroup {Name} and AWS::MediaLive::CloudWatchAlarmTemplate {Name, GroupIdentifier: {Ref: group}, ComparisonOperator: GreaterThanThreshold, EvaluationPeriods: 5, MetricName: 4xxErrorRate, Period: 60, Statistic: Average, TargetResourceType: CLOUDFRONT_DISTRIBUTION, Threshold: 10.0, TreatMissingData: notBreaching}. Observe the ValidationError template-format refusal instead of CREATE_COMPLETE. Repeat with ComparisonOperator="EqualToThreshold" (outside the enum) and observe that no stack exists to reach ROLLBACK_COMPLETE or expose a PROPERTY_VALIDATION row.
3. Cause 2: create_stack with AWS::MediaLive::Network {Name, IpPools, Routes}; then a template adding AWS::MediaLive::Cluster {ClusterType, NetworkSettings referencing the network} and AWS::MediaLive::ChannelPlacementGroup {ClusterId: {Ref: cluster}, Name}. Both refused as unrecognized types.
4. Cause 3: create_stack with AWS::MediaLive::EventBridgeRuleTemplateGroup and AWS::MediaLive::EventBridgeRuleTemplate {Name, GroupIdentifier: {Ref: group}, EventType: MEDIALIVE_CHANNEL_STATE_CHANGE}; then with an EventType outside the enum. Both refused as unrecognized types.
5. Cause 4: create_stack with AWS::MediaLive::SdiSource {Name, Mode, Type}. Refused as unrecognized type.
6. Cause 5: create_stack with AWS::MediaLive::SignalMap {Name, DiscoveryEntryPointArn, Description}. Refused as unrecognized type.
7. Cause 6: create_stack with AWS::S3::Bucket, AWS::IAM::Role (medialive.amazonaws.com trust), AWS::MediaLive::Input {Name, Type: MP4_FILE, Sources:[{Url}]} and AWS::MediaLive::Channel referencing both, with Outputs InputRef={Ref: Input} and InputArn={GetAtt Input.Arn}; Capabilities=[CAPABILITY_IAM]. Wait for CREATE_COMPLETE, then page medialive.list_inputs and filter by the template's input Name — the list is empty even though the stack published InputRef/InputArn.
