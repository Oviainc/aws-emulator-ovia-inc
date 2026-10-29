Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-service-deployment-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-service-deployment-error-paths ecs-err-service-deployment-absent-arn ecs-service-lifecycle-on-cluster ecs-err-service-not-active ecs-execute-command-cluster-and-target-error-paths ecs-container-instance-and-attribute-errors ecs-err-attributes-target-not-found ecs-task-set-external-deployment-workflow ecs-taskset-error-paths
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServiceDeployments|4xx->2xx|$.__type
Issue-Key: ecs|DescribeServiceDeployments|4xx->2xx|$.message
Issue-Key: ecs|DescribeServiceDeployments|4xx->2xx|status
Issue-Key: ecs|DescribeServiceRevisions|4xx->2xx|$.__type
Issue-Key: ecs|DescribeServiceRevisions|4xx->2xx|$.message
Issue-Key: ecs|DescribeServiceRevisions|4xx->2xx|status
Issue-Key: ecs|ContinueServiceDeployment|4xx->4xx|$.__type
Issue-Key: ecs|ContinueServiceDeployment|4xx->4xx|$.message
Issue-Key: ecs|StopServiceDeployment|4xx->4xx|$.__type
Issue-Key: ecs|StopServiceDeployment|4xx->4xx|$.message
Issue-Key: ecs|ListServicesByNamespace|4xx->4xx|$.message
Issue-Key: ecs|CreateTaskSet|4xx->4xx|$.message
Issue-Key: ecs|UpdateService|4xx->4xx|$.message
Issue-Key: ecs|ExecuteCommand|4xx->4xx|$.__type
Issue-Key: ecs|ExecuteCommand|4xx->4xx|$.message
Issue-Key: ecs|DeleteAttributes|4xx->4xx|$.message
Issue-Key: ecs|UpdateServicePrimaryTaskSet|4xx->4xx|$.message
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS service-deployment, task-set and attribute error paths accept invalid requests or answer with the wrong error class and wording

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServiceDeployments

## Description
Ten net-new root causes across the service-deployment, service-state, exec and attribute error paths: two invalid requests are accepted with 200 and eight refusals carry the wrong error class and/or emulator-authored wording. All ten must be closed before submitting.

Cause 1 of 10: DescribeServiceDeployments (fixture ecs-service-deployment-error-paths, step task#3).
AWS: a malformed service-deployment ARN is refused — HTTP 400, error code InvalidParameterException, message 'The ServiceDeployment ARN is invalid.' (fixed text).
Emulator: HTTP 200 with a body.
Why: a client that relies on the refusal to detect a mis-assembled ARN proceeds on a success response that describes nothing, and negative tests pass wrongly.
Fix: DescribeServiceDeployments' validation stage — validate the ARN before resolving it.

Cause 2 of 10: DescribeServiceRevisions (fixture ecs-service-deployment-error-paths, step task#4).
AWS: when the cluster implied by the request does not exist the refusal is HTTP 400, error code ClusterNotFoundException, message 'Cluster not found.' (fixed text).
Emulator: HTTP 200.
Why: as cause 1 — missing-cluster detection silently succeeds.
Fix: cluster resolution in DescribeServiceRevisions.

Cause 3 of 10: ContinueServiceDeployment (fixture ecs-err-service-deployment-absent-arn, step task#8).
AWS: a well-formed service-deployment ARN that names no existing deployment is refused with HTTP 400, error code ClientException, message 'The service deployment does not exist.' (fixed text).
Emulator: 400 with __type 'InvalidParameterException' and message 'The ServiceDeployment ARN is invalid.' — it reports a syntax problem for a syntactically valid ARN.
Why: a caller cannot distinguish "I built the ARN wrong" from "the deployment is gone", so retry/backfill logic takes the wrong branch.
Fix: ContinueServiceDeployment — separate ARN validation from existence checking.

Cause 4 of 10: StopServiceDeployment (fixture ecs-err-service-deployment-absent-arn, step task#9).
AWS: the same absent deployment yields HTTP 400, error code ClientException, message 'Service deployment: <service-deployment-arn>, not found' — fixed text 'Service deployment: ' + the ARN from the request + ', not found'.
Emulator: 400 with __type 'ServiceDeploymentNotFoundException' and an emulator-authored message ("The service deployment ARN <arn> that you specified doesn't exist. You can use ListServiceDeployments to retrieve the service deployment ARNs."), inventing a remediation sentence AWS does not emit for this class.
Why: both the error code and the text differ, so code and message matching both fail.
Fix: StopServiceDeployment's error classification and message template.

Cause 5 of 10: ListServicesByNamespace (fixture ecs-service-lifecycle-on-cluster, step task#8).
AWS: a namespace that cannot be resolved yields a 400 whose message is 'Failed to retrieve namespace for <namespace>' — fixed text 'Failed to retrieve namespace for ' plus the namespace identifier from the request.
Emulator: 400 with "The specified namespace wasn't found." — emulator wording that drops the identifier.
Why: a client matching AWS's text fails, and the operator loses the echoed namespace.
Fix: ListServicesByNamespace's error message template.

Cause 6 of 10: CreateTaskSet (fixture ecs-err-service-not-active, step task#7).
AWS: creating a task set on a service that is not ACTIVE is refused with 400 and message 'Service was not ACTIVE.' (fixed text, no variable parts; Smithy models ServiceNotActiveException for CreateTaskSet).
Emulator: 400 with message 'service "<service-name>" is DRAINING and cannot be updated' — emulator wording that embeds the service name and the internal state.
Why: a client matching AWS's refusal text does not recognize the condition.
Fix: CreateTaskSet's service-state guard message.

Cause 7 of 10: UpdateService (fixture ecs-err-service-not-active, step task#8).
AWS: updating a non-ACTIVE service is refused with 400 and message 'Service was not ACTIVE.'
Emulator: 'service "<service-name>" is DRAINING and cannot be updated'.
Why: as cause 6; the same guard is worded by the emulator in both operations.
Fix: the shared service-state guard message used by UpdateService.

Cause 8 of 10: ExecuteCommand (fixture ecs-execute-command-cluster-and-target-error-paths, step task#2).
AWS: when the task ARN's cluster segment disagrees with the cluster parameter the refusal is HTTP 400, error code InvalidParameterException, message 'Invalid identifier: cluster identifiers mismatch' (fixed text).
Emulator: 400 with __type 'ClusterNotFoundException' and message 'Cluster not found.'
Why: the caller is told the cluster does not exist when in fact two identifiers conflict, so it creates or re-resolves a cluster that is already there.
Fix: ExecuteCommand's target validation — compare the task ARN's cluster with the cluster parameter and report the mismatch.

Cause 9 of 10: DeleteAttributes (fixture ecs-container-instance-and-attribute-errors, step task#7).
AWS: when targetId is a short id rather than a full ARN and targetType is absent, the 400 refusal message is 'TargetType is required when targetId is not a fully qualified ARN.' (fixed text).
Emulator: 400 with 'attributes.targetType is required when targetId is the short form id of a resource' — emulator wording.
Why: a client matching AWS's text does not recognize the failure.
Fix: DeleteAttributes' validation message.

Cause 10 of 10: UpdateServicePrimaryTaskSet (fixture ecs-task-set-external-deployment-workflow, step task#14).
AWS: promoting a task set that does not exist yields a 400 whose message is 'Unable to find task set with id <task-set-id> on service <service-name>.' — fixed text with the task-set id and service name taken from the request (Smithy models TaskSetNotFoundException for this operation).
Emulator: 400 with 'The specified task set was not found.' — emulator wording that drops both identifiers.
Why: a client matching AWS's text fails, and an operator debugging a blue/green switch cannot see which task set and service were meant.
Fix: UpdateServicePrimaryTaskSet's not-found message template.

Related open issues, neither of which covers these causes: "ecs: 7 net-new root causes in not-found / error-classification semantics" (20261029T175650Z-ecs-7-net-new-root-causes-in-not.md) and "ecs: 7 net-new root causes in request validation" (20261029T175650Z-ecs-7-net-new-root-causes-in-request.md).

## Evidence
Replay of simparity-wire-aws-v1:
- DescribeServiceDeployments: status expected 400, got 200; $.__type expected 'InvalidParameterException', got absent; $.message expected 'The ServiceDeployment ARN is invalid.', got absent.
- DescribeServiceRevisions: status expected 400, got 200; $.__type expected 'ClusterNotFoundException', got absent; $.message expected 'Cluster not found.', got absent.
- ContinueServiceDeployment 400/400: $.__type expected 'ClientException', got 'InvalidParameterException'; $.message expected 'The service deployment does not exist.', got 'The ServiceDeployment ARN is invalid.'
- StopServiceDeployment 400/400: $.__type expected 'ClientException', got 'ServiceDeploymentNotFoundException'; $.message expected 'Service deployment: <arn>, not found', got "The service deployment ARN <arn> that you specified doesn't exist. You can use ListServiceDeployments to retrieve the service deployment ARNs." (2 affected steps).
- ListServicesByNamespace 400/400: $.message expected 'Failed to retrieve namespace for <namespace>', got "The specified namespace wasn't found."
- CreateTaskSet and UpdateService 400/400: $.message expected 'Service was not ACTIVE.', got 'service "<service>" is DRAINING and cannot be updated'.
- ExecuteCommand 400/400: $.__type expected 'InvalidParameterException', got 'ClusterNotFoundException'; $.message expected 'Invalid identifier: cluster identifiers mismatch', got 'Cluster not found.'
- DeleteAttributes 400/400: $.message expected 'TargetType is required when targetId is not a fully qualified ARN.', got 'attributes.targetType is required when targetId is the short form id of a resource' (2 affected steps).
- UpdateServicePrimaryTaskSet 400/400: $.message expected 'Unable to find task set with id <task-set-id> on service <service>.', got 'The specified task set was not found.' (2 affected steps).
Smithy (ECS 2014-11-13): CreateTaskSet errors include ServiceNotActiveException, ClusterNotFoundException, InvalidParameterException, ClientException; UpdateServicePrimaryTaskSet errors include TaskSetNotFoundException, ServiceNotActiveException, InvalidParameterException.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and read the ecs service-deployment / service-state / exec / attribute clusters.
2. DescribeServiceDeployments with a malformed service-deployment ARN -> emulator 200 instead of 400 InvalidParameterException 'The ServiceDeployment ARN is invalid.'; DescribeServiceRevisions against a nonexistent cluster -> emulator 200 instead of 400 ClusterNotFoundException 'Cluster not found.'
3. ContinueServiceDeployment / StopServiceDeployment with a well-formed but absent service-deployment ARN -> wrong error codes and messages.
4. ListServicesByNamespace with an unresolvable namespace -> emulator wording without the namespace.
5. Put a service into DRAINING, then CreateTaskSet and UpdateService on it -> emulator wording instead of 'Service was not ACTIVE.'
6. ExecuteCommand with a task ARN whose cluster differs from the cluster parameter -> ClusterNotFoundException instead of InvalidParameterException 'Invalid identifier: cluster identifiers mismatch'.
7. DeleteAttributes with a short-form targetId and no targetType -> emulator wording.
8. UpdateServicePrimaryTaskSet with a task-set id that does not exist -> message drops the task-set id and service name.
