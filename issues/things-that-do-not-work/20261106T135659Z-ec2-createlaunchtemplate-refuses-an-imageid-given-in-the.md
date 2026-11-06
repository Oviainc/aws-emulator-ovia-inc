Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Step: setup#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ec2|CreateLaunchTemplate|2xx->4xx|status
Issue-Commit: dabd605678ea57cce68275edff8c08147ffd283a

# EC2 CreateLaunchTemplate refuses an ImageId given in the documented resolve:ssm: alias form

> _Note: this issue was identified against the code base at commit `dabd605678ea57cce68275edff8c08147ffd283a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplate

## Description
Cause 1 of 1: EC2 CreateLaunchTemplate refuses a LaunchTemplateData.ImageId supplied as a Systems Manager parameter alias instead of accepting it and deferring resolution to launch time. Fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, step setup#4.

AWS: Smithy (com.amazonaws.ec2) CreateLaunchTemplate, input LaunchTemplateData of shape RequestLaunchTemplateData, member ImageId. Its modelled documentation states: "The ID of the AMI in the format ami-0ac394d6a3example. Alternatively, you can specify a Systems Manager parameter, using one of the following formats. The Systems Manager parameter will resolve to an AMI ID on launch", and lists the accepted alias forms — resolve:ssm:<public-parameter>, resolve:ssm:<parameter-name>[:version-number|:label], and resolve:ssm:<parameter-ARN>[:version-number|:label]. An ImageId in any of those forms is a valid value: AWS answers HTTP 200 with the LaunchTemplate element (LaunchTemplateId, LaunchTemplateName, DefaultVersionNumber, LatestVersionNumber) and stores the alias string verbatim in the version's ImageId; the AMI id is resolved only when an instance is launched from the template. There is no refusal for this input, and in particular none keyed on whether the parameter exists at create time.

Emulator: the recorded call is refused — replay reports status expected 200, got 400, with no launch template created. The emulator evidently resolves the alias eagerly at CreateLaunchTemplate time against its own parameter store. The refusal's error code and message are not asserted by this fixture step; the same call is recorded on the behavioral side as HTTP 400 with code SsmInvalidParameter and message "The following aliases are invalid: <alias path>" (fixed text "The following aliases are invalid: ", variable part the alias path echoed from the request's ImageId minus the resolve:ssm: prefix) — see open issue 20261106T125205Z-autoscaling-ec2-ecs-createlaunchtemplate-refuses-a-resolve-ssm.md, which is a behavioral finding and carries no replay key for this wire fixture step, so it does not cover it.

Why: a client that follows the documented launch-template idiom — pin an AMI by SSM alias rather than hard-coding an ami- id — cannot create a launch template at all, and cannot distinguish a genuinely malformed alias from an emulator whose parameter store lacks the referenced namespace. Because the launch template is the first resource in the EC2-capacity bootstrap chain (launch template -> Auto Scaling group -> ECS capacity provider -> cluster association), the refusal lands before anything downstream exists, so the whole capacity-provider scenario in this fixture proceeds against resources that were never created.

Fix: the EC2 CreateLaunchTemplate handler and its ImageId validation in the ec2 service crate — accept every documented resolve:ssm: form, store the alias verbatim in the launch-template version rather than resolving it during create, and resolve it only on the instance-launch path (where an unresolvable alias may legitimately be refused).

Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#setup#4.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Divergence cluster service=ec2|CreateLaunchTemplate|status=200->400, fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, step setup#4:
  status: expected 200, got 400

Expected (AWS): HTTP 200 with the LaunchTemplate element, the version holding ImageId exactly as submitted (the resolve:ssm: alias string), resolution deferred to launch.
Actual (emulator): HTTP 400, no launch template created. The behavioral corpus records the same call as (SsmInvalidParameter) "The following aliases are invalid: <alias path>" — fixed text "The following aliases are invalid: ", variable part the alias path taken from the request's ImageId.
The launch-template name and id in this recording are per-run values and are not part of the requirement; the requirement is that the call succeeds with 200 and the alias is stored unresolved.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Call EC2 CreateLaunchTemplate with LaunchTemplateName=<any unused name> and LaunchTemplateData={"ImageId": "resolve:ssm:<an AWS-published public parameter path, e.g. /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id>", "InstanceType": "t3.micro"}.
3. Observe HTTP 400 and no launch template; real AWS answers HTTP 200 with the LaunchTemplate element and stores the alias string unresolved in the version's ImageId.
4. Re-replay fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair step setup#4 to confirm the status expectation (200) is met.
