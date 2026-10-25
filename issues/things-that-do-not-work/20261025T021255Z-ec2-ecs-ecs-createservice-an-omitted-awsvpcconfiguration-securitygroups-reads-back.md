Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected
Issue-Key: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected|run#19|AssertionError|assert vpc_cfg.get("securityGroups") == [], f"the omitted security groups read back {vpc_cfg.get('securityGroups')!r}"
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# ECS CreateService: an omitted awsvpcConfiguration.securityGroups reads back absent instead of an empty list

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Cause 1 of 1: for an awsvpc service created with a networkConfiguration that names only subnets, the service's awsvpcConfiguration comes back WITHOUT a securityGroups member, where AWS returns it as an empty list.

AWS: Smithy (com.amazonaws.ecs#AwsVpcConfiguration): subnets is required, securityGroups is an optional StringList documented as "The IDs of the security groups associated with the task or service. If you don't specify a security group, the default security group for the VPC is used", and assignPublicIp defaults to DISABLED for create-service. Recorded behavior: CreateService with networkConfiguration={"awsvpcConfiguration": {"subnets": [<subnet>]}} returns a service whose networkConfiguration.awsvpcConfiguration round-trips the subnet, carries securityGroups as an EMPTY LIST, and carries assignPublicIp "DISABLED" — the omitted list is materialized as [], not left out of the response.

Emulator: subnets and assignPublicIp are correct, but securityGroups is absent from the response (reads back as None in the SDK).

Why: a client relying on AWS's behavior reads service["networkConfiguration"]["awsvpcConfiguration"]["securityGroups"] unconditionally and gets a KeyError/None dereference against the emulator; configuration-drift and diff tooling compares an absent member against [] and reports a spurious difference, and typed SDKs that model the member as a list see null where AWS always sends a list.

Fix: the ECS CreateService path's network-configuration state/serializer (the same representation DescribeServices reads back) — materialize AwsVpcConfiguration.securityGroups as an empty list when the request omits it, as it already does for the assignPublicIp default.

Note: the refusal half of this check (CreateService and RunTask without a networkConfiguration for an awsvpc task definition -> InvalidParameterException "Network Configuration must be provided when networkMode 'awsvpc' is specified.", creating nothing) PASSED; only the representation of the accepted control diverges.

Traces: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected

## Evidence
Check ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected, verdict mismatch.

Expected (recorded AWS): created service -> networkConfiguration.awsvpcConfiguration == {"subnets": [<subnet>], "securityGroups": [], "assignPublicIp": "DISABLED"}.
Actual (emulator): AssertionError: the omitted security groups read back None

HTTP 200 in both cases; the divergence is the missing securityGroups member in the response shape.

## Steps to Reproduce
1. Create a VPC and one subnet via ec2.
2. ecs.create_cluster(<unique name>).
3. ecs.register_task_definition(family=<unique>, networkMode="awsvpc", requiresCompatibilities=["EC2"], containerDefinitions=[{"name": "app", "image": "example.invalid/service:dev", "memoryReservation": 64}]).
4. ecs.create_service(cluster=<cluster>, serviceName=<unique>, taskDefinition=<arn>, desiredCount=0, launchType="EC2", networkConfiguration={"awsvpcConfiguration": {"subnets": [<subnet>]}}).
5. Inspect response["service"]["networkConfiguration"]["awsvpcConfiguration"]: securityGroups is absent; AWS returns [].
