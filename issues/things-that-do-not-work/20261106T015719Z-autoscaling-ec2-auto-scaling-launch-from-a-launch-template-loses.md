Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgssh-asg-create-with-unresolvable-instance-profile-rejected autoscaling__ec2.asgssh-asg-launch-propagates-only-propagate-at-launch-tags autoscaling__ec2.asgssh-asg-update-of-unknown-group-rejected-while-describe-is-empty
Issue-Key: autoscaling__ec2.asgssh-asg-create-with-unresolvable-instance-profile-rejected|run#16|ParamValidationError|ec2.create_launch_template_version(LaunchTemplateId=lt_id, LaunchTemplateData=clean)
Issue-Key: autoscaling__ec2.asgssh-asg-launch-propagates-only-propagate-at-launch-tags|run#13|AssertionError|assert tags_on == want, (
Issue-Key: autoscaling__ec2.asgssh-asg-update-of-unknown-group-rejected-while-describe-is-empty|run#18|AssertionError|assert sg_ids == {est["sg"]}, (
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# Auto Scaling launch from a launch template loses the template's data: instance SGs and tags are wrong, and DescribeLaunchTemplateVersions emits a MetadataOptions.State the request shape cannot carry (3 causes)

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Three net-new root causes, grounded in three failing behavioral checks of this service. Each cause is independent and must be closed before the checks pass.

Cause 1 of 3: DescribeLaunchTemplateVersions returns LaunchTemplateData.MetadataOptions with a State member, which the request shape has no slot for, so the documented describe -> edit -> CreateLaunchTemplateVersion round trip is refused by the SDK before it leaves the client.
AWS: Smithy: the read shape com.amazonaws.ec2#LaunchTemplateInstanceMetadataOptions carries State (enum pending|applied, doc: "The state of the metadata option changes"), while the input shape com.amazonaws.ec2#LaunchTemplateInstanceMetadataOptionsRequest has exactly HttpTokens, HttpPutResponseHopLimit, HttpEndpoint, HttpProtocolIpv6, InstanceMetadataTags and NO State member. Recorded behavior: a launch template created with MetadataOptions={HttpTokens, HttpPutResponseHopLimit, HttpEndpoint} reads back from DescribeLaunchTemplateVersions WITHOUT State, which is why the recorded flow can hand that same LaunchTemplateData straight back to CreateLaunchTemplateVersion.
Emulator: DescribeLaunchTemplateVersions echoes MetadataOptions with an added State member. Feeding the returned LaunchTemplateData back into CreateLaunchTemplateVersion therefore dies in botocore's own request validation: `botocore.exceptions.ParamValidationError: Parameter validation failed: Unknown parameter in LaunchTemplateData.MetadataOptions: "State", must be one of: HttpTokens, HttpPutResponseHopLimit, HttpEndpoint, HttpProtocolIpv6, InstanceMetadataTags`. Note the check's refusal assertions earlier in the same body DID pass: CreateAutoScalingGroup correctly refuses a template naming a nonexistent instance profile with ValidationError and AWS's message.
Why: deriving a new version from an existing one (read the version, drop/alter a member, submit) is the documented and ubiquitous pattern (console, CDK, CloudFormation, any 'bump the template' script). With an unsendable member in the read payload, every such client fails client-side and can never create the next version.
Fix: the LaunchTemplateData serializer on the launch-template read paths (DescribeLaunchTemplateVersions / CreateLaunchTemplate / CreateLaunchTemplateVersion responses) must not emit MetadataOptions.State for a launch-template version. (The same check later needs CreateLaunchTemplateVersion itself, whose unimplemented-scaffold issue is already filed; that is a different work item.)
Traces: autoscaling__ec2.asgssh-asg-create-with-unresolvable-instance-profile-rejected.

Cause 2 of 3: an instance launched by an Auto Scaling group carries neither the launch template's own instance TagSpecifications nor the aws:ec2launchtemplate:* tags AWS stamps.
AWS: Smithy: com.amazonaws.autoscaling#Tag.PropagateAtLaunch - "Determines whether the tag is added to new instances as they are launched in the group". Recorded behavior: an instance launched by a group from a launch template carries (a) every group tag with PropagateAtLaunch true, (b) the instance tags declared in the template's LaunchTemplateData.TagSpecifications, and (c) the AWS-stamped reserved tags aws:autoscaling:groupName (the group name), aws:ec2launchtemplate:id (the template id) and aws:ec2launchtemplate:version (the resolved version number) - and NOT the group tags with PropagateAtLaunch false.
Emulator: the launched instance's tag set holds only the propagated group tags and aws:autoscaling:groupName. The template's own instance tag (declared in LaunchTemplateData.TagSpecifications) is absent, and both aws:ec2launchtemplate:id and aws:ec2launchtemplate:version are absent. Replay detail: expected {RIG-Version, sim2real, InstanceGroupName, aws:autoscaling:groupName, aws:ec2launchtemplate:id, aws:ec2launchtemplate:version}; got {RIG-Version, aws:autoscaling:groupName, sim2real}.
Why: tag-driven inventory, cost allocation and tag-filtered DescribeInstances miss template-declared tags, and nothing can join a running instance back to the template id/version that produced it - the standard way to tell which template version a group is actually running.
Fix: the Auto Scaling launch path that materialises instances for a group (the RunInstances call it performs and the tag merge around it) must apply LaunchTemplateData.TagSpecifications of the resolved version and stamp aws:ec2launchtemplate:id and aws:ec2launchtemplate:version alongside aws:autoscaling:groupName.
Traces: autoscaling__ec2.asgssh-asg-launch-propagates-only-propagate-at-launch-tags.

Cause 3 of 3: an instance launched by an Auto Scaling group ignores the launch template's network-interface security groups and comes up in a different (default) security group.
AWS: Smithy: RunInstances input member NetworkInterfaces (InstanceNetworkInterfaceSpecificationList) whose Groups member names the security groups for the interface; Auto Scaling launches through RunInstances with the resolved LaunchTemplateData, so the instance's SecurityGroups is exactly the template's NetworkInterfaces[0].Groups and its SubnetId the template's SubnetId. Recorded behavior: the launched instance carries only the group's own security group.
Emulator: the instance launched after UpdateAutoScalingGroup raises DesiredCapacity reports a security group that is NOT the one named in the template's NetworkInterfaces[0].Groups (the replay shows the template group sg-...041 expected and an unrelated group sg-...02c attached - i.e. the VPC default group, not the declared one); the subnet and the absence of a public IP are correct.
Why: every network-posture conclusion a client draws is wrong - instances inherit default-group rules instead of the declared ones, so ingress/egress checks, SSH-reachability derivations and isolation tests all disagree with what the template declares.
Fix: the Auto Scaling launch path's resolution of LaunchTemplateData.NetworkInterfaces (Groups, and the per-interface fields alongside them) into the EC2 instance it creates, so the declared groups - not a default - land on the instance.
Traces: autoscaling__ec2.asgssh-asg-update-of-unknown-group-rejected-while-describe-is-empty (the group's not-found refusal and the empty-describe assertions in that check already pass; the failure is the positive control's launched instance).

## Evidence
Check [error] autoscaling__ec2.asgssh-asg-create-with-unresolvable-instance-profile-rejected
  Expected (AWS): DescribeLaunchTemplateVersions(LaunchTemplateId=<lt>, Versions=["1"]) returns LaunchTemplateData whose MetadataOptions holds only the request-shape members, so the same LaunchTemplateData (minus IamInstanceProfile) is accepted by CreateLaunchTemplateVersion.
  Actual (emulator): the returned MetadataOptions contains State, and the follow-up call never reaches the wire: `botocore.exceptions.ParamValidationError: Parameter validation failed: Unknown parameter in LaunchTemplateData.MetadataOptions: "State", must be one of: HttpTokens, HttpPutResponseHopLimit, HttpEndpoint, HttpProtocolIpv6, InstanceMetadataTags`.

Check [mismatch] autoscaling__ec2.asgssh-asg-launch-propagates-only-propagate-at-launch-tags
  Expected (AWS): the InService instance's tags = {<group tag with PropagateAtLaunch true>, <group tag sim2real>, <template instance tag>, aws:autoscaling:groupName=<group name>, aws:ec2launchtemplate:id=<lt id>, aws:ec2launchtemplate:version=<resolved version>}.
  Actual (emulator): only {<group tag with PropagateAtLaunch true>, <group tag sim2real>, aws:autoscaling:groupName=<group name>} - template TagSpecifications tag and both aws:ec2launchtemplate:* tags missing.

Check [mismatch] autoscaling__ec2.asgssh-asg-update-of-unknown-group-rejected-while-describe-is-empty
  Expected (AWS): after update_auto_scaling_group(MinSize=1, DesiredCapacity=1), the launched instance's SecurityGroups == {the security group named in the launch template's NetworkInterfaces[0].Groups}.
  Actual (emulator): AssertionError "the launched instance should carry only the group's own security group 'sg-...041'; got ['sg-...02c']" - a different group is attached. (The ids are per-run values; the requirement is set equality with the template's declared groups.)

## Steps to Reproduce
1. Create a VPC, one private subnet and one security group SG of your own.
2. ec2 CreateLaunchTemplate with LaunchTemplateData = {InstanceType: t3.micro, ImageId: <AL2023 ami>, MetadataOptions: {HttpTokens: required, HttpPutResponseHopLimit: 2, HttpEndpoint: enabled}, NetworkInterfaces: [{DeviceIndex: 0, SubnetId: <subnet>, Groups: [SG], DeleteOnTermination: true}], TagSpecifications: [{ResourceType: instance, Tags: [{Key: InstanceGroupName, Value: <value>}]}]}.
3. ec2 DescribeLaunchTemplateVersions(LaunchTemplateId=<lt>, Versions=["1"]) and hand the returned LaunchTemplateData straight back to ec2 CreateLaunchTemplateVersion.
   Observed: botocore refuses the request locally - Unknown parameter in LaunchTemplateData.MetadataOptions: "State" (Cause 1). AWS's read payload has no State, so the call is accepted.
4. autoscaling CreateAutoScalingGroup(MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, LaunchTemplate={LaunchTemplateId: <lt>, Version: "1"}, Tags=[{Key: RIG-Version, Value: "2", PropagateAtLaunch: true}, {Key: GroupOnly, Value: stays, PropagateAtLaunch: false}]); then SetDesiredCapacity/UpdateAutoScalingGroup to 1 and wait for the instance to be InService.
5. ec2 DescribeInstances on the launched instance and read Tags.
   Observed: the template's instance tag and aws:ec2launchtemplate:id / aws:ec2launchtemplate:version are missing (Cause 2).
6. Read the same instance's SecurityGroups.
   Observed: a security group other than SG (the VPC default) is attached instead of the template's declared group (Cause 3).
