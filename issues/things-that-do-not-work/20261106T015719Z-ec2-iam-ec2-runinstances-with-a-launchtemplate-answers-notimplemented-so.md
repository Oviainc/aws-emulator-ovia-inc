Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected
Issue-Key: ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected|run#12|AssertionError|assert got == want, (
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# EC2 RunInstances with a LaunchTemplate answers NotImplemented, so a launch from a template never happens and its iamInstanceProfile.name refusal is unreachable

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Cause 1 of 1: RunInstances refuses any request that carries a LaunchTemplate with an emulator-authored NotImplemented fault instead of resolving the template version and launching (or refusing with the client error AWS uses).
AWS: Smithy: RunInstances input member LaunchTemplate targets com.amazonaws.ec2#LaunchTemplateSpecification (LaunchTemplateId | LaunchTemplateName, Version); the operation models no errors, so a bad request is an HTTP 400 client fault carrying an EC2 error code and message. The template's data is resolved at LAUNCH, not at template creation, so a template whose LaunchTemplateData names an instance profile that does not exist is accepted by CreateLaunchTemplate and refused by RunInstances with code `InvalidParameterValue` and the message `Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name` - fixed text `Value (`, `) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name`, with the single variable part being the profile name taken from the template's IamInstanceProfile.Name. The refused launch leaves no instance, and the identical launch with a real profile passes DryRun with DryRunOperation.
Emulator: the call is refused with code `NotImplemented` and the message `operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator` - a server-fault class and a message AWS never emits, in place of either a successful launch or the 400 InvalidParameterValue above.
Why: launching from a launch template is the normal launch path for CDK/CloudFormation, Auto Scaling and almost every modern client; none of them can start an instance. Worse, the fault class is wrong: a NotImplemented/5xx-shaped answer is retried by SDK retry policy and reported as a service outage, whereas AWS's answer here is a terminal 400 client error that tells the caller exactly which parameter is bad. Validation of the template's iamInstanceProfile.name against IAM therefore never happens either.
Fix: the RunInstances handler's LaunchTemplate branch (services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs) - resolve LaunchTemplateId/Name + Version ($Latest/$Default included) to the stored LaunchTemplateData, merge it under the request's own members, validate the resolved IamInstanceProfile name/ARN against IAM and refuse an unresolvable one with InvalidParameterValue and AWS's message, then launch as for an inline request. The open issue 20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md records the same 501 NotImplemented class for RunInstances-with-LaunchTemplate under the `ec2` service; it does not carry this check.
Traces: ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected.

## Evidence
Check [mismatch] ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected
  Expected (AWS): ec2 RunInstances(LaunchTemplate={LaunchTemplateId: <template whose data names a nonexistent instance profile>, Version: "1"}, MinCount=1, MaxCount=1) -> HTTP 400, Code `InvalidParameterValue`, Message `Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name`, and DescribeInstances filtered on the template's tag finds no instance.
  Actual (emulator): Code `NotImplemented`, Message `operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator`.
  (The profile name in the message is a per-run value supplied by the request; the requirement is the template around it.)

## Steps to Reproduce
1. Create a VPC, a private subnet and a security group.
2. ec2 CreateLaunchTemplate with LaunchTemplateData = {InstanceType: t3.micro, ImageId: <AL2023 ami>, NetworkInterfaces: [{DeviceIndex: 0, SubnetId: <subnet>, Groups: [<sg>]}], IamInstanceProfile: {Name: <a profile name that does not exist>}, TagSpecifications: [{ResourceType: instance, Tags: [{Key: Run, Value: <marker>}]}]}.
   Observed and expected: CreateLaunchTemplate accepts the unresolved profile name and DescribeLaunchTemplateVersions reads it back verbatim.
3. ec2 RunInstances(LaunchTemplate={LaunchTemplateId: <lt>, Version: "1"}, MinCount=1, MaxCount=1).
   Observed: NotImplemented / "operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator". AWS answers 400 InvalidParameterValue with `Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name`.
4. ec2 DescribeInstances(Filters=[{Name: tag:Run, Values: [<marker>]}]) -> must be empty (it is, because nothing launched).
5. Create a real IAM role + instance profile, make an otherwise identical template naming it, and call RunInstances with DryRun=true.
   Observed: the same NotImplemented instead of DryRunOperation.
