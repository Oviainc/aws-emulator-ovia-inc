Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgdx-group-on-latest-keeps-symbolic-version-and-follows-new-sg autoscaling__ec2.asgssh-asg-pinned-version-ignores-new-template-version-while-latest-follows
Issue-Key: autoscaling__ec2.asgdx-group-on-latest-keeps-symbolic-version-and-follows-new-sg|run#10|AssertionError|assert ref == {"LaunchTemplateId": lt, "LaunchTemplateName": tok, "Version": "$Latest"}, (
Issue-Key: autoscaling__ec2.asgssh-asg-pinned-version-ignores-new-template-version-while-latest-follows|run#16|AssertionError|assert seen == [("1", None), ("2", "v2")], (
Issue-Commit: bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c

# Auto Scaling group reads omit the launch template's name, and ASG-launched instances carry no launch-template identity tags (2 causes)

> _Note: this issue was identified against the code base at commit `bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Two net-new root causes in this service, each grounded in its own failing behavioral check. Both must be closed before the checks pass.

Cause 1 of 2: DescribeAutoScalingGroups returns a LaunchTemplate reference that omits LaunchTemplateName when the group was created naming the template by id alone.
AWS: Smithy: com.amazonaws.autoscaling#LaunchTemplateSpecification has exactly three members - LaunchTemplateId, LaunchTemplateName and Version - and the input is documented as "Conditional: You must specify either a LaunchTemplateId or a LaunchTemplateName." Recorded behavior: a group created with LaunchTemplate={LaunchTemplateId: <lt>} reads back with ALL THREE members populated - the id, the template's name resolved from EC2, and the Version selector (here the symbolic "$Latest"). The value of LaunchTemplateName is the name the EC2 launch template was created with; it is not supplied by the request.
Emulator: the group's LaunchTemplate reads back as {LaunchTemplateId: <lt>, Version: "$Latest"} - the LaunchTemplateName key is absent entirely. Replay detail: AssertionError "the group's template reference is {'LaunchTemplateId': 'lt-...', 'Version': '$Latest'}" where the expected shape also carries LaunchTemplateName.
Why: every client that renders or diffs a group's template binding by name (console, CDK/CloudFormation drift detection, `aws autoscaling describe-auto-scaling-groups --query ...LaunchTemplateName`, any script that resolves the group back to a named template) reads a missing key and either crashes on the lookup or concludes the group has no template name. The group cannot be joined back to its template by name at all.
Fix: the DescribeAutoScalingGroups projection of LaunchTemplateSpecification (the serializer/state that materialises the group's template reference) must resolve and emit the companion identifier - name when the request gave an id, id when the request gave a name - not only the one the caller supplied.
Traces: autoscaling__ec2.asgdx-group-on-latest-keeps-symbolic-version-and-follows-new-sg.

Note: the open issue 20261102T104402Z-autoscaling-ec2-5-net-new-root-causes-described.md describes a related "described groups lose the launch-template name" symptom but its replay key does not carry this check, so this check is not covered by it.

Cause 2 of 2: an instance launched by an Auto Scaling group from a launch template carries none of the launch-template-derived instance tags, so nothing records which template version actually produced it.
AWS: Smithy: com.amazonaws.autoscaling#LaunchTemplateSpecification.Version - "The version number, $Latest, or $Default. ... If the value is $Latest, Amazon EC2 Auto Scaling selects the latest version of the launch template when launching instances." Recorded behavior: an instance launched by a group carries (a) the instance tags declared in the resolved version's LaunchTemplateData.TagSpecifications, and (b) the AWS-stamped reserved tags aws:ec2launchtemplate:id (the template id) and aws:ec2launchtemplate:version (the RESOLVED version number as a decimal string - "1" for a group pinned to "1", "2" for a group on "$Latest" once a second version exists). The values are taken from the group's selector and the template's state, never from the request.
Emulator: the launched instances carry neither tag. Replay detail: for a pinned group and a $Latest group launched after a second template version was created, the observed (aws:ec2launchtemplate:version, template-declared tag) pairs are [(None, None), (None, None)] where AWS gives [("1", None), ("2", "v2")]. The group's own version selectors do read back correctly ("1" and "$Latest"), so the defect is in the launch path, not the group representation.
Why: a client cannot tell which template version a running instance came from - the standard way to verify that a pinned group stayed on its version while a $Latest group moved. Tag-driven inventory, cost allocation and tag-filtered DescribeInstances also miss every tag the template declares.
Fix: the Auto Scaling launch path that materialises instances for a group (the RunInstances call it performs and the tag merge around it) must apply the resolved version's LaunchTemplateData.TagSpecifications and stamp aws:ec2launchtemplate:id and aws:ec2launchtemplate:version.
Traces: autoscaling__ec2.asgssh-asg-pinned-version-ignores-new-template-version-while-latest-follows.

Note: cause 2 of the open issue 20261106T015719Z-autoscaling-ec2-auto-scaling-launch-from-a-launch-template-loses.md describes the same missing-tag defect, but its replay key names three other checks and not this one, so this check is not covered by it.

## Evidence
Check [mismatch] autoscaling__ec2.asgdx-group-on-latest-keeps-symbolic-version-and-follows-new-sg
  Expected (AWS): DescribeAutoScalingGroups -> group["LaunchTemplate"] == {"LaunchTemplateId": <the launch template's id>, "LaunchTemplateName": <that template's name>, "Version": "$Latest"}.
  Actual (emulator): {"LaunchTemplateId": <id>, "Version": "$Latest"} - LaunchTemplateName absent. (The id and name are per-run values; the requirement is that all three members are present and the name is the created template's name.)

Check [mismatch] autoscaling__ec2.asgssh-asg-pinned-version-ignores-new-template-version-while-latest-follows
  Expected (AWS): the InService instance of the group pinned to "1" carries aws:ec2launchtemplate:version == "1" and does NOT carry the tag only version 2 declares; the InService instance of the "$Latest" group carries aws:ec2launchtemplate:version == "2" and the instance tag version 2's TagSpecifications declares. Observed pairs: [("1", None), ("2", "v2")].
  Actual (emulator): [(None, None), (None, None)] - neither instance carries aws:ec2launchtemplate:version, and neither carries the template-declared instance tag. The preceding assertion on the two groups' Version selectors ("1", "$Latest") passed.

## Steps to Reproduce
1. ec2 CreateLaunchTemplate(LaunchTemplateName=NAME, LaunchTemplateData={ImageId: <AL2023 ami>, InstanceType: t3.micro, SecurityGroupIds: [SG1], TagSpecifications: [{ResourceType: instance, Tags: [{Key: K, Value: V1}]}]}) -> LT_ID.
2. autoscaling CreateAutoScalingGroup(LaunchTemplate={LaunchTemplateId: LT_ID}, VPCZoneIdentifier=<subnet>, MinSize=0, MaxSize=2, DesiredCapacity=0).
3. autoscaling DescribeAutoScalingGroups and read the group's LaunchTemplate.
   Observed: LaunchTemplateName is missing; AWS returns id, name and Version together (Cause 1).
4. ec2 CreateLaunchTemplateVersion(LaunchTemplateId=LT_ID, SourceVersion="1", LaunchTemplateData={TagSpecifications: [{ResourceType: instance, Tags: [{Key: K, Value: V1}, {Key: TemplateRevision, Value: v2}]}]}).
5. Create a second group on the same template with Version="$Latest" and a first group pinned with Version="1"; scale both to DesiredCapacity 1 and wait for an InService instance in each.
6. ec2 DescribeInstances on each launched instance and read Tags.
   Observed: no aws:ec2launchtemplate:id, no aws:ec2launchtemplate:version and no template-declared instance tag on either instance; AWS stamps version "1" on the pinned group's instance and "2" plus the v2 tag on the $Latest group's instance (Cause 2).
