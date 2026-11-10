Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.asgssh-instance-from-template-carries-the-resolved-profile-arn ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected
Issue-Key: ec2__iam.asgssh-instance-from-template-carries-the-resolved-profile-arn|run#13|AssertionError|assert got == (ip["Arn"], ip["InstanceProfileId"]), (
Issue-Key: ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected|_error#10|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# ec2__iam: 2 net-new causes — DescribeInstances omits IamInstanceProfile.Id, and a launch naming a nonexistent instance profile succeeds instead of being refused

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Two root causes. Both must be closed before submitting.

Cause 1 of 2: the instance's IamInstanceProfile is returned without its Id.
AWS: Smithy com.amazonaws.ec2#IamInstanceProfile is a structure with two members, Arn ("The Amazon Resource Name (ARN) of the instance profile") and Id ("The ID of the instance profile"). For an instance launched with an instance profile, DescribeInstances returns Reservations[].Instances[].IamInstanceProfile with BOTH members populated: Arn is the profile's ARN and Id is the profile's InstanceProfileId (the AIPA... id iam:GetInstanceProfile reports).
Emulator: the member is present but carries Arn only; Id is absent, so the SDK reports it as None.
Why: Id is the stable identity of the profile — a profile can be deleted and recreated under the same name and ARN with a new Id, and clients that join the instance view to the IAM view (or that detect such a swap) key on Id. With Id missing, that join silently fails and the swap is invisible.
Fix: the serializer/state for the instance's IAM-profile association in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instances.rs (and the association state the launch records) — carry and emit the profile's InstanceProfileId alongside its ARN.
Failing check: ec2__iam.asgssh-instance-from-template-carries-the-resolved-profile-arn.

Cause 2 of 2: an unresolvable iamInstanceProfile.name is accepted instead of refused.
AWS: the instance-profile name in a launch (directly or through a launch template) is resolved at LAUNCH time, not at template creation. Naming a profile that does not exist is refused with HTTP 400, ec2Query error code InvalidParameterValue (EC2 error codes are documented, not modeled in Smithy) and the message "Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name" — the fixed text is "Value (" / ") for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name", and the profile name inside the parentheses is the value taken from the request. The refused launch must leave no instance behind.
Emulator: the call SUCCEEDS — RunInstances returns 200 for a launch template whose instance-profile name does not exist, so there is no error at all.
Why: a client gets a running instance that can never obtain credentials, and the failure surfaces much later as an opaque IMDS/credential error inside the guest instead of a synchronous, actionable InvalidParameterValue at launch. Infrastructure tooling that relies on the launch failing fast (and on no instance being created) instead leaks an unusable instance.
Fix: the instance-profile resolution/validation step of services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — resolve iamInstanceProfile.name against IAM at launch and refuse with InvalidParameterValue and AWS's message when it does not resolve, creating no instance.
Failing check: ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected.

## Evidence
Cause 1 — ec2__iam.asgssh-instance-from-template-carries-the-resolved-profile-arn:
AssertionError: the instance should report profile ('arn:aws:iam::<account>:instance-profile/sim2real-asgssh-profile-...', '<InstanceProfileId>'); got ('arn:aws:iam::<account>:instance-profile/sim2real-asgssh-profile-...', None)
The ARN matched; the second element — IamInstanceProfile.Id — came back absent. (The concrete profile name/ARN/id are per-run values.)

Cause 2 — ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected:
AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error
Expected: ('InvalidParameterValue', 'Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name') and no instance created.

## Steps to Reproduce
Cause 1: iam:CreateRole + CreateInstanceProfile + AddRoleToInstanceProfile; ec2:CreateLaunchTemplate with IamInstanceProfile={Name:<profile>}; ec2:RunInstances from that template; ec2:DescribeInstances and read Reservations[0].Instances[0].IamInstanceProfile — expect both Arn and Id, observe Id missing.
Cause 2: ec2:CreateLaunchTemplate whose LaunchTemplateData.IamInstanceProfile.Name is a profile that does not exist; ec2:RunInstances(LaunchTemplate={LaunchTemplateId:<id>, Version:'1'}, MinCount=1, MaxCount=1) — expect 400 InvalidParameterValue 'Value (<name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name' and no instance; observe a successful launch.
Then re-replay both check ids in trace_ids.
