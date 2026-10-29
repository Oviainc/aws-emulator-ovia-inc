Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-container-instance-and-attribute-errors
Issue-Step: teardown#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-container-instance-and-attribute-errors ecs-run-task-and-task-protection ecs-service-lifecycle-on-cluster ecs-start-task-error-paths ecs-task-definition-lifecycle ecs-task-definition-list-sort-and-status-partition ecs-task-definition-revision-lifecycle ecs-task-set-external-deployment-workflow ecs-taskdef-tagging-lifecycle ecs-taskset-basic-lifecycle ecs-taskset-errors ecs-taskset-lifecycle-ec2 ecs-task-definition-list-next-token-pagination ecs-cluster-error-taxonomy ecs-cluster-describe-and-list-lifecycle ecs-fargate-awsvpc-task-definition-rendered-defaults
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DeregisterTaskDefinition|2xx->2xx|$.taskDefinition.previousStatus
Issue-Key: ecs|DeleteTaskDefinitions|2xx->2xx|$.taskDefinitions[*].previousStatus
Issue-Key: ecs|DeleteTaskDefinitions|2xx->2xx|$.failures[*].reason
Issue-Key: ecs|DeregisterTaskDefinition|4xx->4xx|$.__type
Issue-Key: ecs|DeregisterTaskDefinition|4xx->4xx|$.message
Issue-Key: ecs|ListTaskDefinitionFamilies|4xx->4xx|$.message
Issue-Key: ecs|ListClusters|4xx->4xx|$.message
Issue-Key: ecs|ListClusters|4xx->4xx|$.__type
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.compatibilities[*]
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.compatibilities[*]
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS task-definition and list operations drop previousStatus and MANAGED_INSTANCES compatibility and use emulator refusal wording

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterTaskDefinition

## Description
Nine net-new root causes in the task-definition lifecycle and in list-parameter validation. All nine must be closed before submitting.

Cause 1 of 9: DeregisterTaskDefinition (fixture ecs-container-instance-and-attribute-errors, step teardown#13).
AWS: recorded behavior only — the 200 response's taskDefinition carries previousStatus, the status the revision held before the call ('ACTIVE' when deregistering an active revision). The member is not in the Smithy TaskDefinition shape, so the recording is the authority.
Emulator: omits taskDefinition.previousStatus.
Why: a client that confirms a deregistration by comparing previousStatus with status cannot tell a first deregistration from a repeat of one.
Fix: DeregisterTaskDefinition's response projection for the task definition.

Cause 2 of 9: DeleteTaskDefinitions (fixture ecs-task-definition-lifecycle, step task#8).
AWS: each entry of taskDefinitions carries previousStatus — 'INACTIVE' for a revision that was deregistered before deletion.
Emulator: omits taskDefinitions[].previousStatus.
Why: as cause 1.
Fix: DeleteTaskDefinitions' response projection (same derivation as cause 1).

Cause 3 of 9: DeleteTaskDefinitions (fixture ecs-task-definition-lifecycle, step task#6).
AWS: deleting a revision that is still ACTIVE does not fail the call — 200 with a failures entry whose reason is 'The specified task definition is still in ACTIVE status. Please deregister the target and try again.' (fixed text; the arn is the requested revision).
Emulator: 200 but reason 'Task definition must be in INACTIVE status to be deleted.' — emulator wording.
Why: a client matching the AWS reason, or showing it to an operator with the deregister remediation, gets different text and loses the remediation sentence.
Fix: DeleteTaskDefinitions' failure-reason text.

Cause 4 of 9: DeregisterTaskDefinition (fixture ecs-task-definition-list-sort-and-status-partition, step task#18).
AWS: a taskDefinition value that names a family with no revision is refused with HTTP 400, error code InvalidParameterException, message 'Revision is missing' (fixed text, no variable parts).
Emulator: 400 with __type 'ClientException' and an emulator-authored message that embeds the requested family ('taskDefinition must name a revision (family:revision or a task-definition ARN); got "<family>"').
Why: the error class a client handles (InvalidParameterException) never arrives and the message does not match AWS's.
Fix: DeregisterTaskDefinition's validation — error class and message.

Cause 5 of 9: ListTaskDefinitionFamilies (fixture ecs-task-definition-list-next-token-pagination, step task#15).
AWS: a maxResults above the service limit is refused with 400 and message 'Max results can not be greater than 100' (fixed text, limit 100).
Emulator: 400 with 'MaxResults must be a positive number between 1-100.'
Why: a client matching the refusal text does not recognize it; the emulator wording also misdescribes the rule (it conflates the too-large and non-positive cases).
Fix: the shared maxResults validator's message for the list operations.

Cause 6 of 9: ListClusters (fixture ecs-cluster-error-taxonomy, step task#3).
AWS: same refusal text 'Max results can not be greater than 100' for an over-limit maxResults.
Emulator: 'MaxResults must be a positive number between 1-100.'
Why: as cause 5 — the same validator is wrong across list operations.
Fix: as cause 5.

Cause 7 of 9: ListClusters (fixture ecs-cluster-describe-and-list-lifecycle, step task#7).
AWS: an unparseable nextToken is refused with HTTP 400, error code InvalidParameterException, message 'Invalid token <token>' — fixed text 'Invalid token ' followed by the token value from the request.
Emulator: 400 with __type 'ClientException' and message 'Invalid nextToken.', which neither uses AWS's error class nor echoes the offending token.
Why: a paginating client branching on InvalidParameterException to restart pagination does not take that branch, and the operator cannot see which token was rejected.
Fix: the pagination-token validator — error class and message template.

Cause 8 of 9: RegisterTaskDefinition (fixture ecs-fargate-awsvpc-task-definition-rendered-defaults, step task#2).
AWS: compatibilities is derived from the task definition's parameters and, for an awsvpc/Fargate-compatible definition, includes MANAGED_INSTANCES alongside the other launch types. Smithy: TaskDefinition.compatibilities -> CompatibilityList of Compatibility enum [EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES].
Emulator: the derived list omits MANAGED_INSTANCES (the recorded second element is MANAGED_INSTANCES where the emulator has FARGATE).
Why: a client that checks whether a task definition can run on ECS Managed Instances reads the compatibility list and concludes it cannot.
Fix: the compatibilities derivation in RegisterTaskDefinition.

Cause 9 of 9: DescribeTaskDefinition (fixture ecs-fargate-awsvpc-task-definition-rendered-defaults, step task#3).
AWS: the same stored compatibilities list, including MANAGED_INSTANCES, is returned on describe.
Emulator: omits MANAGED_INSTANCES.
Why: as cause 8, on the read path.
Fix: shared compatibilities derivation / stored value used by DescribeTaskDefinition.

## Evidence
Replay of simparity-wire-aws-v1:
- DeregisterTaskDefinition 200/200: $.taskDefinition.previousStatus expected 'ACTIVE', got absent (14 affected steps).
- DeleteTaskDefinitions 200/200: $.taskDefinitions[0].previousStatus expected 'INACTIVE', got absent (5 affected steps).
- DeleteTaskDefinitions 200/200: $.failures[0].reason expected 'The specified task definition is still in ACTIVE status. Please deregister the target and try again.', got 'Task definition must be in INACTIVE status to be deleted.' (3 affected steps).
- DeregisterTaskDefinition 400/400: $.__type expected 'InvalidParameterException', got 'ClientException'; $.message expected 'Revision is missing', got 'taskDefinition must name a revision (family:revision or a task-definition ARN); got "<family>"'.
- ListTaskDefinitionFamilies 400/400 and ListClusters 400/400: $.message expected 'Max results can not be greater than 100', got 'MaxResults must be a positive number between 1-100.'
- ListClusters 400/400: $.__type expected 'InvalidParameterException', got 'ClientException'; $.message expected 'Invalid token <token>', got 'Invalid nextToken.'
- RegisterTaskDefinition / DescribeTaskDefinition 200/200: $.taskDefinition.compatibilities[1] expected 'MANAGED_INSTANCES', got 'FARGATE'.
Smithy (ECS 2014-11-13): Compatibility enum [EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES]; TaskDefinition has status/compatibilities but no previousStatus member (recorded behavior only); DeregisterTaskDefinition errors ClientException|InvalidParameterException.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and read the ecs task-definition / list clusters.
2. RegisterTaskDefinition then DeregisterTaskDefinition the revision -> the response's taskDefinition has no previousStatus.
3. DeleteTaskDefinitions on a still-ACTIVE revision -> failures[0].reason uses emulator wording; after deregistering, DeleteTaskDefinitions -> taskDefinitions[0].previousStatus absent.
4. DeregisterTaskDefinition with taskDefinition=<family> (no revision) -> ClientException with an emulator message instead of InvalidParameterException 'Revision is missing'.
5. ListClusters / ListTaskDefinitionFamilies with maxResults above 100 -> wrong refusal text; ListClusters with a garbage nextToken -> ClientException 'Invalid nextToken.' instead of InvalidParameterException 'Invalid token <token>'.
6. RegisterTaskDefinition an awsvpc/Fargate-compatible definition and DescribeTaskDefinition it -> compatibilities omits MANAGED_INSTANCES.
