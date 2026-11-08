Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected
Issue-Key: ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected|_refusal#12|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# ECS CreateService accepts an awsvpcConfiguration whose subnets span two VPCs instead of refusing InvalidParameterException

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One net-new root cause.

Cause 1 of 1: an awsvpc service whose subnets belong to different VPCs is created instead of refused.
AWS: Smithy — ecs CreateService takes networkConfiguration.awsvpcConfiguration.subnets and models InvalidParameterException (client, HTTP 400). Recorded: a FARGATE CreateService whose subnets list names one subnet from the ECS VPC and one from a second VPC (here the cache subnet group's subnet) is refused 400 InvalidParameterException with the message "All input subnets must be from the same VPC." (entirely fixed text), and the service name stays MISSING — DescribeServices returns no services and a failure with reason MISSING.
Emulator: the call SUCCEEDS; no refusal is raised, so a service is created over subnets in two different VPCs (the check fails at its refusal assertion with "the call was expected to be refused, but it succeeded"). The positive control — the identical request with the ECS subnet alone — is the barrier that shows the request only differs by the foreign subnet.
Why: awsvpc networking cannot span VPCs, so a client relying on AWS to reject the request gets a service the emulator reports as ACTIVE but whose network configuration is impossible; the misconfiguration surfaces later (or never) instead of at CreateService, and cross-VPC diagnostics that expect the refusal see none.
Fix: CreateService validation in the ecs handler — resolve every subnet in awsvpcConfiguration.subnets and refuse when they do not share one VpcId, before any service state is written.
Traces: ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected.

## Evidence
From the behavioral replay, ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected [mismatch]:

Expected: ecs:CreateService(cluster=<cluster>, serviceName=<name>, launchType=FARGATE, desiredCount=0, networkConfiguration={awsvpcConfiguration:{subnets:[<ecs-subnet>, <cache-vpc-subnet>], securityGroups:[<ecs-sg>], assignPublicIp:DISABLED}}) is refused HTTP 400 InvalidParameterException with message "All input subnets must be from the same VPC.", and DescribeServices for that name returns failures=[{reason: MISSING}] with no services.

Actual:
File "check_ec2__ecs__elasticache_tgwc_service_subnets_spanning_ecs_and_cache_vpcs_rejected.py", line 51, in run -> _refusal(...) -> line 187
AssertionError: the call was expected to be refused, but it succeeded

(the check first proves, via ec2:DescribeSubnets, that the two subnet ids belong to two different VPCs, and the control CreateService with the ECS subnet alone is ACTIVE).

## Steps to Reproduce
1. Boot the emulator. Create two VPCs, one private subnet in each (same AZ), and a security group in the first VPC.
2. Create an ECS cluster and register a Fargate-compatible task definition.
3. Call ecs:CreateService with launchType=FARGATE, desiredCount=0 and networkConfiguration.awsvpcConfiguration.subnets = [<subnet in VPC A>, <subnet in VPC B>], securityGroups=[<sg in VPC A>].
4. Observe the call returns 200 and creates the service; AWS answers 400 InvalidParameterException "All input subnets must be from the same VPC." and leaves the name MISSING.
5. Control: the identical call with only the VPC-A subnet must succeed and read back that one subnet and group.
6. Re-replay ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected to confirm the fix.
