Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam.asgeni-asg-template-with-absent-instance-profile-rejected
Issue-Key: autoscaling__ec2__iam.asgeni-asg-template-with-absent-instance-profile-rejected|_error#16|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CreateAutoScalingGroup accepts a launch template whose IamInstanceProfile ARN resolves to no instance profile instead of refusing ValidationError

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Cause 1 of 1: CreateAutoScalingGroup does not resolve the launch template's IamInstanceProfile.Arn against IAM, so a group whose template names an absent instance profile is created instead of refused.

AWS: EC2 CreateLaunchTemplate stores any well-formed IamInstanceProfile.Arn verbatim (no IAM resolution), and the reference is resolved only when Auto Scaling consumes the template. Auto Scaling's CreateAutoScalingGroup resolves that profile ARN against IAM and, when it resolves to nothing, refuses the request with HTTP 400 and error code ValidationError, creating no group. Smithy: CreateAutoScalingGroup (com-amazonaws-autoscaling-autoscaling-2011-01-01) models only AlreadyExistsFault, LimitExceededFault, ResourceContentionFault and ServiceLinkedRoleFailure, so ValidationError is the protocol's generic request-validation error and is recorded behavior only. The refusal message is built from the request and names the rejected ARN, so its RULE is the requirement: 'You must use a valid fully-formed launch template. Value (<iamInstanceProfile.arn as supplied>) for parameter iamInstanceProfile.arn is invalid. Invalid IAM Instance Profile ARN' (the concrete ARN text in the recording is only an example). The same error is also what a not-yet-visible, freshly created profile gets, which is why the check retries by message on its positive control.

Emulator: the call SUCCEEDS. With a launch template carrying IamInstanceProfile={'Arn': 'arn:aws:iam::<account>:instance-profile/<name-that-was-never-created>'}, CreateAutoScalingGroup (MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, LaunchTemplate={'LaunchTemplateId': <bad template>}) returns 200 and the group is created, so the check fails at its _error() helper with 'the call succeeded; real AWS rejects it, so there is no error code'. The positive control through a real instance profile is accepted and describes correctly, so the handler is reachable — it simply performs no cross-service reference validation. Because no refusal happens, the companion requirement that a refused create leave no group behind is also unobservable.

Why: a client relying on AWS's behavior treats CreateAutoScalingGroup as the gate that validates the launch template's IAM instance profile. Against the emulator a misconfigured group (bad or deleted instance profile, or a typo'd profile name) is accepted silently; infrastructure tooling and tests that assert the ValidationError (CDK/CloudFormation rollback paths, guardrail tests, retry-until-profile-visible loops) see a success, and the broken configuration surfaces only much later, at instance launch, instead of at create time.

Fix: in the autoscaling service's CreateAutoScalingGroup handler validation step — resolve LaunchTemplate (and MixedInstancesPolicy launch-template) IamInstanceProfile.Arn/Name against IAM instance-profile state and reject unresolvable references with a 400 ValidationError built from the supplied value, leaving no group in autoscaling state. EC2's CreateLaunchTemplate must keep storing the ARN verbatim (that part already matches AWS).

Traces: autoscaling__ec2__iam.asgeni-asg-template-with-absent-instance-profile-rejected.

## Evidence
Check autoscaling__ec2__iam.asgeni-asg-template-with-absent-instance-profile-rejected [mismatch].
Expected (AWS): botocore ClientError with Error.Code == 'ValidationError' (HTTP 400) and message naming the rejected iamInstanceProfile.arn, e.g. 'You must use a valid fully-formed launch template. Value (arn:aws:iam::<account>:instance-profile/sim2real-asgeni-none-<suffix>) for parameter iamInstanceProfile.arn is invalid. Invalid IAM Instance Profile ARN'; DescribeAutoScalingGroups for that name then returns [].
Actual (emulator): create_auto_scaling_group returned successfully (no error raised). Replay trace:
  File "check_autoscaling__ec2__iam_asgeni_asg_template_with_absent_instance_profile_rejected.py", line 56, in run
    code, message = _error(lambda: asg.create_auto_scaling_group(AutoScalingGroupName=name, MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=sub, LaunchTemplate={"LaunchTemplateId": bad["id"]}))
  File "...", line 91, in _error
    raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
Preceding asserts passed: EC2 stored the unresolvable profile ARN verbatim in the launch template version, and the positive control group built on a REAL instance profile was created and described with the expected LaunchTemplateId.

## Steps to Reproduce
1. ec2:CreateVpc (10.93.0.0/16) + ec2:CreateSubnet in one AZ.
2. iam:CreateRole (ec2.amazonaws.com trust) + iam:CreateInstanceProfile + iam:AddRoleToInstanceProfile -> real profile ARN.
3. ec2:CreateLaunchTemplate 'good' with LaunchTemplateData={'ImageId': 'resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64', 'InstanceType': 't3.micro', 'IamInstanceProfile': {'Arn': <real profile arn>}}.
4. ec2:CreateLaunchTemplate 'bad' with the same data but IamInstanceProfile={'Arn': 'arn:aws:iam::<account>:instance-profile/<name never created>'}; confirm DescribeLaunchTemplateVersions echoes that ARN verbatim.
5. autoscaling:CreateAutoScalingGroup with LaunchTemplate={'LaunchTemplateId': <good>}, MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet> — expect success (positive control).
6. autoscaling:CreateAutoScalingGroup with LaunchTemplate={'LaunchTemplateId': <bad>} and the same sizes — expect HTTP 400 ValidationError naming iamInstanceProfile.arn; the emulator returns 200 and creates the group.
7. autoscaling:DescribeAutoScalingGroups(AutoScalingGroupNames=[<name from step 6>]) — expect AutoScalingGroups == []; the emulator reports the wrongly created group.
Replay: autoscaling__ec2__iam.asgeni-asg-template-with-absent-instance-profile-rejected
