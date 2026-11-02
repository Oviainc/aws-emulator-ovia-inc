Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__ecs.ecsnrt-capacity-provider-delete-while-associated-blocked autoscaling__ec2__ecs.ecsnrt-capacity-provider-service-on-zero-capacity-group-parks-task-provisioning
Issue-Key: autoscaling__ec2__ecs|DeleteCapacityProvider|NotImplemented
Issue-Key: autoscaling__ec2__ecs.ecsnrt-capacity-provider-service-on-zero-capacity-group-parks-task-provisioning|_poll#18|Timeout|time.sleep(interval_s)
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# autoscaling__ec2__ecs: 2 net-new root causes — DeleteCapacityProvider answers 501 NotImplemented, and a capacityProviderStrategy service never produces a task

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCapacityProvider

## Description
TWO net-new root causes on the ECS capacity-provider plane as it is driven from an Auto Scaling group. Both checks get as far as creating the network, the Max=0 Auto Scaling group, the capacity provider and the cluster association, so the failures are in the ECS behavior that follows. A fixer must close BOTH before submitting.

Cause 1 of 2 — DeleteCapacityProvider is an unimplemented scaffold stub.
AWS: Smithy com.amazonaws.ecs#DeleteCapacityProvider takes {capacityProvider (required), cluster} and returns DeleteCapacityProviderResponse{capacityProvider: CapacityProvider}; its modeled client errors are AccessDeniedException, ClientException, ClusterNotFoundException, InvalidParameterException, UnsupportedFeatureException and UpdateInProgressException (NotImplemented is not among them), and the operation is @idempotent. Recorded behavior: deleting a provider that is still associated with a cluster does NOT raise — it answers 200 with the provider's status 'ACTIVE' and updateStatus 'DELETE_FAILED', DescribeCapacityProviders keeps reporting ('ACTIVE','DELETE_FAILED'), and the cluster keeps the provider in its capacityProviders list; after PutClusterCapacityProviders removes the association the identical delete goes through and the provider ends 'INACTIVE'.
Emulator: the call fails with HTTP 501 and ClientError (NotImplemented) 'operation DeleteCapacityProvider is scaffolded but not implemented in this emulator', so neither the blocked-delete state nor the unblocked delete can be observed at all.
Why: a client cannot retire a capacity provider, and the whole DELETE_FAILED signal — the only way AWS tells a caller that an association still holds the provider — is unreachable; teardown paths that delete providers hard-fail on an error code AWS never returns for this operation.
Fix: implement the DeleteCapacityProvider handler in the ECS service crate (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/delete_capacity_provider.rs) over the capacity-provider store, including the association check that yields status ACTIVE + updateStatus DELETE_FAILED rather than an error.
Related but not covering: open issue 20261029T175650Z-ecs-7-net-new-root-causes-in-state.md mentions a DeleteCapacityProvider NotImplemented cause under service 'ecs'; it does not carry this check id and the stub is still being served on this replay.
Traces: autoscaling__ec2__ecs.ecsnrt-capacity-provider-delete-while-associated-blocked.

Cause 2 of 2 — a service whose capacityProviderStrategy names a managed-scaling provider on a zero-capacity group never produces a task, so the check reaches no verdict.
AWS: CreateService (Smithy com.amazonaws.ecs#CreateService, input member capacityProviderStrategy: CapacityProviderStrategy) accepts a service whose strategy names a managed-scaling capacity provider backed by an Auto Scaling group pinned at MaxSize=0, and the service is scheduled anyway: recorded behavior is that ListTasks for the service reports exactly one task, DescribeTasks reports lastStatus 'PROVISIONING' with desiredStatus 'RUNNING' and capacityProviderName set to that provider, DescribeServices reports pendingCount 1 / runningCount 0, and the cluster stays at registeredContainerInstancesCount 0 with the group still at DesiredCapacity 0 and no instances. The service also reads back capacityProviderStrategy with base defaulted to 0 and carries no launchType.
Emulator: the check exhausts its budget ('Check timed out after 60.0s') with no terminal verdict — ListTasks for the service never reports the one task the strategy service should park, so the task that AWS parks in PROVISIONING is never created and none of the downstream assertions can be evaluated.
Why: a caller gets a service that silently schedules nothing; the 'no compute, task parked in PROVISIONING' state that tells an operator the capacity provider cannot scale is never reported, and nothing distinguishes it from a healthy service.
Fix: the ECS service-scheduler state must create the service's task and report it PROVISIONING/RUNNING with capacityProviderName when the service is launched through a capacityProviderStrategy whose provider cannot supply capacity (ECS crate's CreateService / ListTasks / DescribeTasks / DescribeServices state, not the serializers).
Traces: autoscaling__ec2__ecs.ecsnrt-capacity-provider-service-on-zero-capacity-group-parks-task-provisioning.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout.

Cause 1 — autoscaling__ec2__ecs.ecsnrt-capacity-provider-delete-while-associated-blocked [error]: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteCapacityProvider operation: operation DeleteCapacityProvider is scaffolded but not implemented in this emulator. Expected a 200 whose capacityProvider reports (status, updateStatus) == ('ACTIVE', 'DELETE_FAILED') while the cluster still lists the provider, and, after PutClusterCapacityProviders(capacityProviders=[]), a delete that is not DELETE_FAILED and leaves the provider INACTIVE. Modeled errors for the operation are AccessDeniedException/ClientException/ClusterNotFoundException/InvalidParameterException/UnsupportedFeatureException/UpdateInProgressException/ServerException — NotImplemented/501 is not a response AWS can give here.

Cause 2 — autoscaling__ec2__ecs.ecsnrt-capacity-provider-service-on-zero-capacity-group-parks-task-provisioning [mismatch]: 'Check timed out after 60.0s'. The check's first poll after CreateService is ListTasks(cluster, serviceName) waiting for len(taskArns) == 1; no task ever appeared, so the subsequent assertions — DescribeTasks (lastStatus 'PROVISIONING', desiredStatus 'RUNNING', capacityProviderName == the provider), DescribeServices (pendingCount 1, runningCount 0), DescribeClusters (registeredContainerInstancesCount 0) and DescribeAutoScalingGroups (DesiredCapacity 0, Instances []) — were never reached and the check produced no verdict. Reported here as the emulator's: the setup calls all succeeded and the missing task is state the emulator owns.

Smithy grounding (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json): DeleteCapacityProvider{capacityProvider required, cluster} -> DeleteCapacityProviderResponse{capacityProvider}, @idempotent; CreateService input capacityProviderStrategy (CapacityProviderStrategy list) with launchType as the alternative. The DELETE_FAILED/PROVISIONING behaviors are recorded behavior only.

## Steps to Reproduce
1) Build and boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1, checks autoscaling__ec2__ecs.ecsnrt-capacity-provider-delete-while-associated-blocked and autoscaling__ec2__ecs.ecsnrt-capacity-provider-service-on-zero-capacity-group-parks-task-provisioning.
2) Cause 1: create an Auto Scaling group, ecs CreateCapacityProvider over it (managedScaling DISABLED), CreateCluster, PutClusterCapacityProviders associating the provider, then DeleteCapacityProvider(capacityProvider=<name>) -> expect HTTP 200 with (status, updateStatus) == ('ACTIVE', 'DELETE_FAILED'), DescribeCapacityProviders unchanged and the cluster still listing it; then PutClusterCapacityProviders(capacityProviders=[], defaultCapacityProviderStrategy=[]) and repeat the identical delete -> expect updateStatus not DELETE_FAILED and the provider to reach INACTIVE.
3) Cause 2: create an Auto Scaling group pinned at MaxSize=0, CreateCapacityProvider over it with managedScaling ENABLED, CreateCluster + PutClusterCapacityProviders, RegisterTaskDefinition (requiresCompatibilities EC2), then CreateService(desiredCount=1, capacityProviderStrategy=[{capacityProvider: <name>, weight: 1}]) -> ListTasks for the service must report one task; DescribeTasks must report ('PROVISIONING', 'RUNNING', <provider>); DescribeServices must report pendingCount 1 / runningCount 0; the cluster must stay at 0 container instances and the group at DesiredCapacity 0 with no instances.
