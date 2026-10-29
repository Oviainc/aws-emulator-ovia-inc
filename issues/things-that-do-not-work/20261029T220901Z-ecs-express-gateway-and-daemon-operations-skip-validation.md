Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-express-gateway-error-invalid-role
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-express-gateway-error-invalid-role ecs-express-gateway-error-missing-execution-role ecs-express-gateway-error-role-trust ecs-express-gateway-invalid-role-error ecs-express-gateway-service-error-paths ecs-express-gateway-service-invalid-role ecs-express-gateway-service-create-error ecs-express-gateway-cluster-not-found-error ecs-express-gateway-invalid-cluster-error ecs-express-gateway-delete-nonexistent-error ecs-express-gateway-error-delete-not-found ecs-express-gateway-error-describe-not-found ecs-daemon-and-express-gateway-error-paths ecs-daemon-task-definition-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateExpressGatewayService|4xx->2xx|$.__type
Issue-Key: ecs|CreateExpressGatewayService|4xx->2xx|$.message
Issue-Key: ecs|CreateExpressGatewayService|4xx->2xx|status
Issue-Key: ecs|CreateExpressGatewayService|4xx->4xx|$.message
Issue-Key: ecs|CreateExpressGatewayService|4xx->4xx|$.__type
Issue-Key: ecs|DeleteExpressGatewayService|4xx->4xx|$.__type
Issue-Key: ecs|DeleteExpressGatewayService|4xx->4xx|$.message
Issue-Key: ecs|DescribeExpressGatewayService|4xx->4xx|$.__type
Issue-Key: ecs|DescribeExpressGatewayService|4xx->4xx|$.message
Issue-Key: ecs|CreateDaemon|4xx->4xx|$.__type
Issue-Key: ecs|CreateDaemon|4xx->4xx|$.message
Issue-Key: ecs|ListDaemons|2xx->4xx|$.daemonSummariesList
Issue-Key: ecs|ListDaemons|2xx->4xx|status
Issue-Key: ecs|DescribeDaemonTaskDefinition|2xx->2xx|$.daemonTaskDefinition.containerDefinitions[*].environment
Issue-Key: ecs|DescribeDaemonTaskDefinition|2xx->2xx|$.daemonTaskDefinition.containerDefinitions[*].mountPoints
Issue-Key: ecs|DescribeDaemonTaskDefinition|2xx->2xx|$.daemonTaskDefinition.containerDefinitions[*].secrets
Issue-Key: ecs|DescribeDaemonTaskDefinition|2xx->2xx|$.daemonTaskDefinition.containerDefinitions[*].systemControls
Issue-Key: ecs|DescribeDaemonTaskDefinition|2xx->2xx|$.daemonTaskDefinition.containerDefinitions[*].ulimits
Issue-Key: ecs|DescribeDaemonTaskDefinition|4xx->4xx|$.message
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS express-gateway and daemon operations skip validation and misclassify not-found errors

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExpressGatewayService

## Description
Nine net-new root causes on the express-gateway and daemon operations: a missing required-role check, an inverted validation order, three wrong error classes, a rejected read that AWS answers 200, omitted empty lists and one emulator-authored refusal message. All nine must be closed before submitting.

Cause 1 of 9: CreateExpressGatewayService (fixture ecs-express-gateway-error-invalid-role, step task#1).
AWS: an express-gateway service created without a usable execution role is refused — HTTP 400, error code InvalidParameterException, message 'ExecutionRoleArn can not be blank.' (fixed text).
Emulator: HTTP 200 and the service is created.
Why: every negative test of the role requirement passes wrongly, and the emulator holds a service AWS would never have created, so later describe/delete behavior diverges from AWS for the rest of the session.
Fix: CreateExpressGatewayService's validation stage — require a non-blank execution role before creating state.

Cause 2 of 9: CreateExpressGatewayService (fixture ecs-express-gateway-service-create-error, step task#1).
AWS: the same request is refused with 400 and message 'ExecutionRoleArn can not be blank.' — the execution-role check runs before the container/task-definition requirement.
Emulator: 400 but with message 'either primaryContainer or taskDefinitionArn must be specified'.
Why: the client is told to fix the wrong field; a caller that fixes only the named field loops.
Fix: validation order in CreateExpressGatewayService.

Cause 3 of 9: CreateExpressGatewayService (fixture ecs-express-gateway-cluster-not-found-error, step task#1).
AWS: when the named cluster does not exist (or is not a valid cluster identifier) the refusal is HTTP 400, error code ClusterNotFoundException, message 'Cluster not found.' (fixed text) — cluster resolution precedes body validation.
Emulator: 400 with __type 'InvalidParameterException' and message 'either primaryContainer or taskDefinitionArn must be specified'.
Why: a client branching on ClusterNotFoundException to create the cluster and retry never takes that branch.
Fix: validation order in CreateExpressGatewayService — resolve the cluster first.

Cause 4 of 9: DeleteExpressGatewayService (fixture ecs-express-gateway-delete-nonexistent-error, step task#1).
AWS: a malformed/foreign express-gateway ARN is rejected on ARN syntax: HTTP 400, error code InvalidParameterException, message 'Invalid Region in ARN' (fixed text).
Emulator: 400 with __type 'ServiceNotFoundException' and message 'Service not found'.
Why: a bad-ARN programming error is reported as a missing resource, so clients retry or fall through to create-if-absent logic.
Fix: ARN parsing/validation in DeleteExpressGatewayService ahead of the lookup.

Cause 5 of 9: DescribeExpressGatewayService (fixture ecs-express-gateway-error-describe-not-found, step task#1).
AWS: an ARN whose resource type is not an express-gateway service is rejected with HTTP 400, error code InvalidParameterException, message 'Invalid target type in ARN: express-gateway-service' — fixed text 'Invalid target type in ARN: ' plus the target type derived from the request's ARN.
Emulator: 400 with __type 'ResourceNotFoundException' and message 'Service not found'.
Why: as cause 4 — malformed input is indistinguishable from a deleted resource.
Fix: ARN target-type validation in DescribeExpressGatewayService.

Cause 6 of 9: CreateDaemon (fixture ecs-daemon-and-express-gateway-error-paths, step task#1).
AWS: when the referenced daemon task definition does not exist the refusal is HTTP 400, error code ClientException, message 'DaemonTaskDefinition not found.' (fixed text).
Emulator: 400 with __type 'ClusterNotFoundException' and message 'Cluster not found.'
Why: the caller is pointed at the wrong missing resource and the error class it handles never arrives.
Fix: CreateDaemon's validation order — resolve the daemon task definition and report its absence.

Cause 7 of 9: ListDaemons (fixture ecs-daemon-and-express-gateway-error-paths, step task#2).
AWS: 200 with daemonSummariesList: [] when there are no daemons to list.
Emulator: HTTP 400 and no body.
Why: a client enumerating daemons before creating one fails instead of seeing an empty list, so discovery-then-create flows abort.
Fix: ListDaemons handler — return an empty list rather than a validation error. The open issue "ecs: 7 net-new root causes in not-found / error-classification semantics" (20261029T175650Z-ecs-7-net-new-root-causes-in-not.md) mentions ListDaemons but does not carry this fixture step's replay key.

Cause 8 of 9: DescribeDaemonTaskDefinition (fixture ecs-daemon-task-definition-lifecycle, step task#3).
AWS: 200 with each container definition's list members serialized even when empty — environment: [], mountPoints: [], secrets: [], systemControls: [], ulimits: [].
Emulator: omits all five members.
Why: a client iterating containerDefinitions[].environment() gets an absent member instead of an empty list (null dereference in strongly typed SDKs, and "unknown" rather than "none" in diff/drift tooling).
Fix: the container-definition serializer used by the daemon task-definition response.

Cause 9 of 9: DescribeDaemonTaskDefinition (fixture ecs-daemon-task-definition-lifecycle, step task#5).
AWS: for an undescribable/unknown daemon task definition the 400 refusal message is 'Unable to describe task definition.' (fixed text).
Emulator: 400 with 'Unable to describe daemon task definition.' — emulator wording.
Why: a client matching AWS's refusal text does not recognize the failure.
Fix: DescribeDaemonTaskDefinition's error message.

## Evidence
Replay of simparity-wire-aws-v1:
- CreateExpressGatewayService: status expected 400, got 200; $.__type expected 'InvalidParameterException', got absent; $.message expected 'ExecutionRoleArn can not be blank.', got absent (6 affected steps).
- CreateExpressGatewayService 400/400: $.message expected 'ExecutionRoleArn can not be blank.', got 'either primaryContainer or taskDefinitionArn must be specified' (2 affected steps).
- CreateExpressGatewayService 400/400: $.__type expected 'ClusterNotFoundException', got 'InvalidParameterException'; $.message expected 'Cluster not found.', got 'either primaryContainer or taskDefinitionArn must be specified' (2 affected steps).
- DeleteExpressGatewayService 400/400: $.__type expected 'InvalidParameterException', got 'ServiceNotFoundException'; $.message expected 'Invalid Region in ARN', got 'Service not found' (2 affected steps).
- DescribeExpressGatewayService 400/400: $.__type expected 'InvalidParameterException', got 'ResourceNotFoundException'; $.message expected 'Invalid target type in ARN: express-gateway-service', got 'Service not found'.
- CreateDaemon 400/400: $.__type expected 'ClientException', got 'ClusterNotFoundException'; $.message expected 'DaemonTaskDefinition not found.', got 'Cluster not found.'
- ListDaemons: status expected 200, got 400; $.daemonSummariesList expected [], got absent.
- DescribeDaemonTaskDefinition 200/200: containerDefinitions[0].environment/mountPoints/secrets/systemControls/ulimits expected [], got absent.
- DescribeDaemonTaskDefinition 400/400: $.message expected 'Unable to describe task definition.', got 'Unable to describe daemon task definition.'
Smithy (ECS 2014-11-13): ECS client errors include ClientException (400), InvalidParameterException (400), ClusterNotFoundException (400), ServiceNotFoundException (400).

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and read the ecs express-gateway / daemon clusters.
2. CreateExpressGatewayService with a blank/absent executionRoleArn -> emulator 200 instead of 400 InvalidParameterException 'ExecutionRoleArn can not be blank.'
3. CreateExpressGatewayService naming a nonexistent cluster -> emulator reports InvalidParameterException about primaryContainer instead of ClusterNotFoundException 'Cluster not found.'
4. DeleteExpressGatewayService / DescribeExpressGatewayService with a malformed or wrong-target-type ARN -> emulator reports Service/ResourceNotFoundException instead of InvalidParameterException ARN messages.
5. CreateDaemon referencing a nonexistent daemon task definition -> emulator reports ClusterNotFoundException.
6. ListDaemons on a cluster with no daemons -> emulator 400 instead of 200 with daemonSummariesList [].
7. DescribeDaemonTaskDefinition on a registered daemon task definition -> empty container-definition lists are omitted; on an unknown one the message is emulator-worded.
