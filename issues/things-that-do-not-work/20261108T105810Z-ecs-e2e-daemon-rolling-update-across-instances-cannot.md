Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.e2e-daemon-rolling-update-across-instances
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.e2e-daemon-rolling-update-across-instances|_poll_until#12|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: 5326d37abbcb9c44bc1d6bac564fe7e72c9c6fa2

# ecs: e2e-daemon-rolling-update-across-instances cannot provision a Managed Instances capacity provider — CreateCapacityProvider still refuses a valid managedInstancesProvider (reopened refusal, unowned replay key)

> _Note: this issue was identified against the code base at commit `5326d37abbcb9c44bc1d6bac564fe7e72c9c6fa2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
REOPENS 20261024T015148Z-ecs-createcapacityprovider-is-an-unimplemented-stub-and-createservice.md: its fix left the call refused. That (fixed) issue implemented CreateCapacityProvider, which previously answered NotImplemented; the implemented handler now refuses a well-formed `managedInstancesProvider` request with a 400, so the operation is still unusable for ECS Managed Instances. This is not a new defect — it is the same refusal surfacing under a replay key no open issue carries. The open issue 20261107T220512Z-ecs-createcapacityprovider-refuses-a-valid-managed-instances-provider.md describes exactly this refusal, but its Issue-Trace-Ids/Issue-Keys list nine other ecs checks and not this one, so this check is not owned by it; closing that issue without replaying this check would leave this failure unrun.

Cause 1 of 1: CreateCapacityProvider refuses a valid managedInstancesProvider, so the daemon rolling-update scenario never gets past provisioning.
  AWS: CreateCapacityProvider accepts a managedInstancesProvider and returns HTTP 200 with the created capacityProvider, which then converges to status ACTIVE. Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): CreateCapacityProvider input {name (required), cluster, autoScalingGroupProvider, managedInstancesProvider: CreateManagedInstancesProviderConfiguration, tags}; CreateManagedInstancesProviderConfiguration requires infrastructureRoleArn + instanceLaunchTemplate, whose `monitoring` member targets the enum ManagedInstancesMonitoringOptions ([BASIC, DETAILED]) and is OPTIONAL. Recorded behavior: AWS accepts the request this check sends — instanceLaunchTemplate = {ec2InstanceProfileArn, networkConfiguration {subnets, securityGroups}, monitoring: "basic"}. Where a value really is outside the modeled enum, AWS's refusal is the modeled InvalidParameterException (errorKind client, httpError 400) whose message names the offending member and the allowed values, not an emulator-authored sentence.
  Emulator: the CreateCapacityProvider call is refused with a client-class error (the check's helper swallows exactly `ClientException` and `InvalidParameterException` as retryable IAM-propagation noise and returns None), so the provider is never created. The check retries the call for its full 120s budget and then fails with `AssertionError: condition not met within 120s (last=None)` — the refusal never clears because it is not propagation-related, it is the handler rejecting a request AWS accepts.
  Why: a client cannot create an ECS Managed Instances capacity provider, so the entire daemon data-plane behaviour this check exercises is unreachable: CreateDaemon on a MANAGED_INSTANCES provider, the rolling update that starts a new-revision daemon task on the container instance and stops the old-revision one, and the rule that Daemon services accept only MANAGED_INSTANCES providers. The client sees a 400 validation error for a correct request and has no way to reach any daemon behaviour.
  Fix: the managedInstancesProvider / instanceLaunchTemplate validation in the ECS CreateCapacityProvider handler — accept the modeled ManagedInstancesMonitoringOptions values as AWS does (case-insensitively), treat an omitted `monitoring` as defaulted rather than invalid, and reserve InvalidParameterException for values that really are outside the enum, with a message that names the offending member and the allowed values instead of a fixed emulator sentence.
  Traces: ecs.e2e-daemon-rolling-update-across-instances (listed by the suite under `### ecs`).

## Evidence
Expected (AWS, recorded): CreateCapacityProvider --name <n> --cluster <c> --managed-instances-provider '{"infrastructureRoleArn":"<infra role arn>","instanceLaunchTemplate":{"ec2InstanceProfileArn":"<profile arn>","networkConfiguration":{"subnets":["<subnet>"],"securityGroups":["<sg>"]},"monitoring":"basic"}}' -> HTTP 200 with capacityProvider.capacityProviderArn, which then reaches status ACTIVE (not PROVISIONING/INACTIVE).

Actual (emulator): the call is refused with a 400 client-class error on every attempt; the check's `_try_create_managed_instances_capacity_provider` returns None for `ClientException`/`InvalidParameterException`, so the poll exhausts its budget:
  File "check_ecs_e2e_daemon_rolling_update_across_instances.py", line 44, in run
    cp_arn = _poll_until(
  File "check_ecs_e2e_daemon_rolling_update_across_instances.py", line 276, in _poll_until
    raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
  AssertionError: condition not met within 120s (last=None)

The same refusal is recorded verbatim on nine sibling ecs checks in the open issue named above as HTTP 400 `InvalidParameterException`: `monitoring should be one of [BASIC,DETAILED]` — a sentence that is entirely fixed emulator text and contains no value from the request. The IAM role/instance-profile ARNs, subnet and security-group ids, cluster, provider and daemon names in each run are per-run values and are not part of the requirement; the status code, error code and message wording are.

## Steps to Reproduce
1. Create an IAM role trusted by ECS carrying AmazonECSInfrastructureRolePolicyForManagedInstances, and an EC2 instance profile carrying AmazonECSInstanceRolePolicyForManagedInstances.
2. ecs create-cluster --cluster-name <c>.
3. ecs create-capacity-provider --name <n> --cluster <c> --managed-instances-provider '{"infrastructureRoleArn":"<infra role arn>","instanceLaunchTemplate":{"ec2InstanceProfileArn":"<profile arn>","networkConfiguration":{"subnets":["<subnet>"],"securityGroups":["<sg>"]},"monitoring":"basic"}}'
   -> emulator: HTTP 400 client-class refusal, no provider created (retrying for 120s never succeeds). AWS: HTTP 200 with the capacityProvider.
4. Repeat with `monitoring` omitted and with "BASIC"/"DETAILED" to confirm whether the comparison is case-sensitive and whether the optional member is treated as required.
5. With the provider created, continue the scenario: put-cluster-capacity-providers, register-daemon-task-definition, create-daemon, then update the daemon's task definition and observe a new-revision task start and the old-revision task stop on the Managed Instances container instance.
Replay: ecs.e2e-daemon-rolling-update-across-instances.
