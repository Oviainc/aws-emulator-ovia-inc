Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected
Issue-Key: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected|run#19|AssertionError|assert vpc_cfg.get("securityGroups") == [], f"the omitted security groups read back {vpc_cfg.get('securityGroups')!r}"
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# ec2__ecs: CreateService's awsvpcConfiguration read-back omits securityGroups instead of returning an empty list

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One net-new root cause.

Cause 1 of 1: a service created with an awsvpcConfiguration that supplies only `subnets` reads back without the `securityGroups` member at all, where AWS materializes it as an empty list.
AWS: Smithy: NetworkConfiguration.awsvpcConfiguration -> AwsVpcConfiguration, whose members are subnets (StringList, required), securityGroups (StringList, optional — "If you don't specify a security group, the default security group for the VPC is used") and assignPublicIp (enum AssignPublicIp, default DISABLED for create-service/update-service). Recorded behavior: CreateService with awsvpcConfiguration={"subnets": [<subnet>]} answers a Service whose networkConfiguration.awsvpcConfiguration carries the subnet list, securityGroups as [] and assignPublicIp "DISABLED" — the server materializes the omitted members in the response.
Emulator: the CreateService response echoes only what the caller sent: awsvpcConfiguration has `subnets` and no `securityGroups` key (it reads back None).
Why: a client cannot distinguish "no security groups were requested, so the VPC default applies" from "this member is unknown/unsupported"; code that iterates the returned list (or diffs desired vs actual network configuration, as CDK/Terraform-style reconcilers do) either crashes on the missing key or reports permanent drift, and an idempotent re-apply of the read-back value changes meaning.
Fix: CreateService network-configuration default materialization plus the Service/NetworkConfiguration serializer (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs and the Service serializer) — store/emit the normalized AwsVpcConfiguration rather than the raw request echo.
Traces: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected. Note the rejection legs of that same check PASS — CreateService and RunTask on an awsvpc task definition with no networkConfiguration are both correctly refused with InvalidParameterException — so this is purely the accepted-path read-back normalization.

Related open issue, not a duplicate: 20261024T081305Z-ecs-service-and-task-definition-read-models-omit.md Cause 4 covers the sibling assignPublicIp default under the `ecs` service token; the `securityGroups` member is not in that bundle and is reached through this ec2__ecs check.

## Evidence
Replay output, verdict `mismatch`:
- ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected — after a VPC + one subnet are created in EC2 and a task definition with networkMode=awsvpc is registered, `ecs.create_service(cluster=..., serviceName=..., taskDefinition=<awsvpc td>, desiredCount=0, launchType="EC2", networkConfiguration={"awsvpcConfiguration": {"subnets": [<subnet-id>]}})` succeeds, and `service["networkConfiguration"]["awsvpcConfiguration"]["subnets"]` is correct, but the next assertion fails: AssertionError: "the omitted security groups read back None" (expected []).
Expected: awsvpcConfiguration = {subnets: [<subnet-id>], securityGroups: [], assignPublicIp: "DISABLED"}. Actual: {subnets: [<subnet-id>]}.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the check named in trace_ids.
3. By hand: ec2 CreateVpc + CreateSubnet; ecs CreateCluster; ecs RegisterTaskDefinition with networkMode="awsvpc", requiresCompatibilities=["EC2"], one container; ecs CreateService with networkConfiguration={"awsvpcConfiguration": {"subnets": [<subnet>]}} and no securityGroups / assignPublicIp. Inspect the returned service (and DescribeServices) — `securityGroups` is absent where AWS returns [].
