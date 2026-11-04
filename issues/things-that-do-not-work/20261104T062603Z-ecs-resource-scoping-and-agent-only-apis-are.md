Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.deploy-ecs-runtask-unassociated-capacity-provider-rejected ecs.put-cluster-capacity-providers-unassociated-provider-used-in-strategy-rejected ecs.e2e-disassociate-capacity-provider-in-use-rejected ecs.describe-services-resolves-a-name-only-within-its-cluster ecs.fargate-service-on-a-deleted-subnet-rejected ecs.deployment-read-ops-on-missing-cluster-not-found ecs.update-task-protection-cluster-mismatch-rejected ecs.submit-task-state-change-invalid-status-rejected ecs.task-state-change-ordering-enforced-rejected ecs.task-state-change-server-enforced ecs.task-state-changes-are-monotonic-terminal ecs.list-services-by-namespace-filters-correctly
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.deploy-ecs-runtask-unassociated-capacity-provider-rejected|_error#10|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.put-cluster-capacity-providers-unassociated-provider-used-in-strategy-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.e2e-disassociate-capacity-provider-in-use-rejected|_error#12|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.deployment-read-ops-on-missing-cluster-not-found|_error#22|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.update-task-protection-cluster-mismatch-rejected|_error#10|AssertionError|raise AssertionError(f"expected UpdateTaskProtection to reject a cross-cluster task ARN, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.submit-task-state-change-invalid-status-rejected|run#13|AssertionError|assert control_error == "AccessDeniedException", (
Issue-Key: ecs.task-state-change-ordering-enforced-rejected|_error_code#12|AssertionError|raise AssertionError("expected SubmitTaskStateChange to raise for a non-agent caller, it succeeded")
Issue-Key: ecs.task-state-change-server-enforced|_error#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ecs.task-state-changes-are-monotonic-terminal|run#30|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: ecs|ListServicesByNamespace|InvalidParameterException
Issue-Related: 20261103T041326Z-ecs-createservice-runtask-createcluster-describeservices-accept-seven-request.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS resource-scoping and agent-only APIs are unguarded: capacity-provider association, cross-cluster name resolution, subnet validity and SubmitTaskStateChange all accept what AWS refuses (8 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitTaskStateChange

## Description
Eight independent authorization/scoping validations are missing.

Cause 1 of 8: RunTask accepts a capacityProviderStrategy naming a provider that is not associated with the cluster. AWS refuses with ClientException/InvalidParameterException; a provider must be associated via PutClusterCapacityProviders first. Emulator: 200, the task is launched with capacityProviderName FARGATE_SPOT on a cluster whose capacityProviders list is empty. Fix: RunTask capacity-provider resolution. Traces: ecs.deploy-ecs-runtask-unassociated-capacity-provider-rejected, ecs.put-cluster-capacity-providers-unassociated-provider-used-in-strategy-rejected.

Cause 2 of 8: a capacity provider that is in use can be disassociated. AWS refuses the PutClusterCapacityProviders/UpdateCluster that drops a provider still backing tasks or a service. Emulator: the call succeeds and the subsequent RunTask also succeeds. Fix: cluster capacity-provider state transitions. Trace: ecs.e2e-disassociate-capacity-provider-in-use-rejected.

Cause 3 of 8: DescribeServices resolves a bare service name against a cluster that does not hold it. AWS scopes a name to the named cluster and returns the service in failures (or refuses), never another cluster's service. Emulator: the call succeeds and returns the foreign service. Fix: the service lookup in the DescribeServices handler (cluster-scoped index). Trace: ecs.describe-services-resolves-a-name-only-within-its-cluster.

Cause 4 of 8: CreateService accepts an awsvpcConfiguration subnet that has been deleted. AWS refuses with InvalidParameterException. Emulator: 200. Fix: CreateService network-configuration validation against the EC2 subnet state. Trace: ecs.fargate-service-on-a-deleted-subnet-rejected.

Cause 5 of 8: DescribeServiceDeployments against a cluster that does not exist answers 200 with failures=[{reason:'MISSING'}] instead of ClusterNotFoundException. AWS raises ClusterNotFoundException for an unknown cluster on the deployment read operations. Emulator: 200 with a MISSING failure entry. Fix: the cluster-resolution preamble shared by the service-deployment read handlers. Trace: ecs.deployment-read-ops-on-missing-cluster-not-found.

Cause 6 of 8: UpdateTaskProtection accepts a task ARN belonging to another cluster, answering 200 with failures=[{arn:<task>, reason:'MISSING'}] instead of refusing the cross-cluster reference with InvalidParameterException. Fix: UpdateTaskProtection ARN/cluster consistency check. Trace: ecs.update-task-protection-cluster-mismatch-rejected.

Cause 7 of 8: SubmitTaskStateChange is reachable from a non-agent caller and accepts arbitrary/illegal transitions. AWS restricts the Submit*StateChange family to the container agent (a normal caller is refused with AccessDeniedException) and enforces monotonic, non-terminal-reversing transitions. Emulator: the calls succeed, returning {'acknowledgment': 'ACK'} with HTTP 200, including for a Fargate task and for an out-of-order/terminal-reversing status. Fix: the Submit*StateChange handlers' caller gate and transition validation. Traces: ecs.submit-task-state-change-invalid-status-rejected, ecs.task-state-change-ordering-enforced-rejected, ecs.task-state-change-server-enforced, ecs.task-state-changes-are-monotonic-terminal.

Cause 8 of 8: ListServicesByNamespace validates a namespace ARN against the namespace-NAME pattern and so refuses a legitimate ARN. AWS accepts either the Cloud Map namespace name or its ARN. Emulator: `InvalidParameterException: Namespace (arn:aws:servicediscovery:...:namespace/ns-...) must match ^[!-~&&[^<>/"]]{1,1024}$.` — note the Java-style character-class '&&' leaking into the wire message. Fix: the ListServicesByNamespace input validation/resolution. Trace: ecs.list-services-by-namespace-filters-correctly.

Why (all eight): a client that depends on AWS refusing a cross-scope or privileged request gets silent success, and state the real service would never permit — a task on an unassociated provider, a service read from the wrong cluster, task state driven by a non-agent caller.

## Evidence
Replayed details (one per cause):
1. `AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'tasks': [{... 'capacityProviderName': 'FARGATE_SPOT', 'clusterArn': ... }], 'failures': []}` (HTTP 200) and `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'capacityProviderName': 'FARGATE_SPOT' ...}`.
2. `AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'tasks': [...]}`.
3. `AssertionError: the call succeeded; real AWS refuses it`.
4. `AssertionError: the call succeeded; real AWS refuses it`.
5. `AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'serviceDeployments': [], 'failures': [{'arn': 'arn:aws:ecs:...:service-deployment/no-such-cluster-.../...', 'reason': 'MISSING'}]}` — expected ClusterNotFoundException.
6. `AssertionError: expected UpdateTaskProtection to reject a cross-cluster task ARN, but it SUCCEEDED and returned {'protectedTasks': [], 'failures': [{'arn': 'arn:aws:ecs:...:task/<other-cluster>/<id>', 'reason': 'MISSING'}]}`.
7. `AssertionError: expected the well-formed control status 'PENDING' to be refused with AccessDeniedException (the Fargate-agent-only gate), got None`; `AssertionError: expected SubmitTaskStateChange to raise for a non-agent caller, it succeeded`; `AssertionError: expected the call to be rejected, but it succeeded: {'acknowledgment': 'ACK', ... 'HTTPStatusCode': 200}`.
8. `botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the ListServicesByNamespace operation: Namespace (arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-s4x6olxhyfdnyiry) must match ^[!-~&&[^<>/"]]{1,1024}$.`
Ids/ARNs quoted are per-run values.

## Steps to Reproduce
1. CreateCluster with no capacityProviders; RunTask with capacityProviderStrategy=[{capacityProvider:'FARGATE_SPOT',weight:1}] -> 200.
2. Associate a provider, run a task on it, then PutClusterCapacityProviders dropping it -> accepted.
3. Create service S in cluster A; DescribeServices(cluster=B, services=['S']) -> returns A's service.
4. Delete a subnet, then CreateService with that subnet in awsvpcConfiguration -> 200.
5. DescribeServiceDeployments with a service-deployment ARN under a nonexistent cluster -> 200 + MISSING.
6. UpdateTaskProtection(cluster=A, tasks=[<task ARN in cluster B>]) -> 200 + MISSING.
7. Call SubmitTaskStateChange as an ordinary signed caller -> {'acknowledgment':'ACK'}; repeat with a terminal-reversing status -> accepted.
8. ListServicesByNamespace(namespace=<full Cloud Map namespace ARN>) -> InvalidParameterException.
