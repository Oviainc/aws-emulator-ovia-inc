Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.capacity-provider-ops-on-missing-cluster-not-found ecs.list-services-by-namespace ecs.e2e-capacity-provider-fargate-spot-task-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.capacity-provider-ops-on-missing-cluster-not-found|_create_once#5|Timeout|time.sleep(2)
Issue-Key: ecs.list-services-by-namespace|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: ecs|ListTasks|ClusterNotFoundException
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS reads hang or mis-resolve on clusters/providers that should answer immediately: capacity-provider and namespace reads time out and a Fargate-Spot lifecycle reports the cluster inactive (3 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServicesByNamespace

## Description
Cause 1 of 3: capacity-provider read/mutate operations against a cluster that does not exist never answer within the budget.
AWS: DescribeCapacityProviders/PutClusterCapacityProviders and friends against a missing cluster are refused immediately with ClusterNotFoundException.
Emulator: the check polls and is cut off (`Check timed out after 60.0s`) — no refusal is produced.
Fix: the cluster-resolution preamble of the capacity-provider handlers.
Trace: ecs.capacity-provider-ops-on-missing-cluster-not-found.

Cause 2 of 3: ListServicesByNamespace never converges for a namespace that does hold services.
AWS: once a service is created with a serviceConnectConfiguration/namespace, ListServicesByNamespace lists it.
Emulator: the positive listing check polls until the 60s budget expires.
Fix: the namespace index used by ListServicesByNamespace (populated on CreateService/UpdateService).
Trace: ecs.list-services-by-namespace.

Cause 3 of 3: a cluster that was just created and is ACTIVE is reported inactive by ListTasks during the Fargate-Spot lifecycle.
AWS: ListTasks against an ACTIVE cluster succeeds.
Emulator: `ClusterNotFoundException: The referenced cluster was inactive.` is raised for the cluster the check just created and used, i.e. cluster resolution in that path consults the wrong state or the wrong key.
Fix: the cluster lookup shared by ListTasks/DescribeServices.
Traces: ecs.e2e-capacity-provider-fargate-spot-task-lifecycle, ecs.escc-describe-services-on-an-unknown-cluster-is-cluster-not-found is NOT part of this (it is owned elsewhere).

## Evidence
1. `Error: Check timed out after 60.0s` (expected ClusterNotFoundException immediately).
2. `Error: Check timed out after 60.0s`.
3. `botocore.errorfactory.ClusterNotFoundException: An error occurred (ClusterNotFoundException) when calling the ListTasks operation: The referenced cluster was inactive.` raised against a cluster the same check created and that DescribeClusters reports ACTIVE.

## Steps to Reproduce
1. DescribeCapacityProviders / PutClusterCapacityProviders with cluster='no-such-cluster-<suffix>' -> no ClusterNotFoundException within 60s.
2. CreateCluster with serviceConnectDefaults, CreateService in that namespace, then poll ListServicesByNamespace -> never lists it.
3. CreateCluster, associate FARGATE_SPOT, RunTask, then ListTasks(cluster=<that cluster>) -> ClusterNotFoundException 'The referenced cluster was inactive.'
