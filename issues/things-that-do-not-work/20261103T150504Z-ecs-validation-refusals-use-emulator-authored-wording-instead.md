Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecscm-traj-create-service-launch-mistakes
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecscm-traj-create-service-launch-mistakes ecs-ecsdep-traj-create-deployment-configuration-refusals ecs-escc-traj-mirror-custom-service-create-refusals ecs-ntraf-service-load-balancer-block-mistakes ecs-ecsnrt-traj-task-listing-and-describe-mistakes ecs-escc-traj-list-services-filter-and-paging-mistakes ecs-ecsdep-traj-service-deployment-read-mistakes ecs-escc-traj-describe-services-name-list-mistakes ecs-escc-traj-deleted-service-stale-reads
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateService|4xx->4xx|$.message
Issue-Key: ecs|ListTasks|4xx->4xx|$.message
Issue-Key: ecs|ListServices|4xx->4xx|$.message
Issue-Key: ecs|ListServiceDeployments|4xx->4xx|$.message
Issue-Key: ecs|ListServiceDeployments|4xx->4xx|$.__type
Issue-Key: ecs|DescribeServiceDeployments|4xx->4xx|$.message
Issue-Key: ecs|DescribeServiceRevisions|4xx->4xx|$.message
Issue-Key: ecs|DescribeTasks|4xx->4xx|$.message
Issue-Key: ecs|DescribeServices|4xx->4xx|$.message
Issue-Key: ecs|DeleteService|4xx->4xx|$.message
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS validation refusals use emulator-authored wording instead of AWS's message text

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Ten net-new causes, all one fault: ECS refuses the request with the right HTTP status but writes its own prose (or its own member names/limit phrasing) where AWS has a fixed message. The error message is part of the contract, so each of these is a wire divergence even though the status matches. This is the same family as open issue 20261103T041326Z-ecs-refusal-messages-are-emulator-authored-prose (duplicate service name, negative desiredCount, ...) and 20261029T220901Z-ecs-service-deployment-task-set-and-attribute-error, but none of the ten below is carried by either issue's replay key.

Cause 1 of 10: CreateService launch-type enum refusal text. AWS: 400 InvalidParameterException, fixed text 'launch type should be one of [EC2,FARGATE,EXTERNAL]' — recorded behavior only for the member list and its order; Smithy models LaunchType as [EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES], and AWS's CreateService refusal does NOT list MANAGED_INSTANCES. No variable part. Emulator: 400 with 'launch type should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]' — it renders the modeled enum, alphabetically sorted, for every operation. Why: a client matching on the refusal text (or a human reading it) is told MANAGED_INSTANCES is acceptable on CreateService when AWS rejects it, and the ordering differs from every real response. Fix: CreateService's validation — the launch-type refusal must carry a per-operation literal list, not a sort of the modeled enum. Traces: ecs-ecscm-traj-create-service-launch-mistakes#task#7 (cluster of 12 steps across ecs-ecscm-traj-create-service-launch-mistakes, ecs-ecsdep-traj-create-deployment-configuration-refusals, ecs-escc-traj-mirror-custom-service-create-refusals, ecs-ntraf-service-load-balancer-block-mistakes).

Cause 2 of 10: ListTasks launch-type enum refusal text. AWS: 400 InvalidParameterException, fixed text 'launch type should be one of [EC2,FARGATE,EXTERNAL,FMI,MANAGED_INSTANCES]' (recorded behavior only — ListTasks's list is wider than CreateService's and includes FMI). Emulator: the same shared 'launch type should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]'. Why: the filter operations and the create operations do not accept the same set, and the emulator erases that distinction. Fix: ListTasks's validation. Traces: ecs-ecsnrt-traj-task-listing-and-describe-mistakes#task#2.

Cause 3 of 10: ListServices launch-type enum refusal text. AWS: 400 InvalidParameterException 'launch type should be one of [EC2,FARGATE,EXTERNAL,FMI,MANAGED_INSTANCES]' (recorded behavior only). Emulator: 'launch type should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]'. Fix: ListServices's validation. Traces: ecs-escc-traj-list-services-filter-and-paging-mistakes#task#10 (cluster of 3 steps).

Cause 4 of 10: ListServiceDeployments with no usable service identifier. AWS: 400 with fixed text 'The Service ARN provided is invalid.' — Smithy: ListServiceDeployments.service is @required, and InvalidParameterException/ClientException are modeled on the operation. No variable part. Emulator: 400 with its generic constraint-framework sentence "1 validation error detected: Value null at 'service' failed to satisfy constraint: Member must not be null". Why: the emulator leaks a framework message shape AWS never emits for this operation, so error-text assertions and user-facing surfacing both differ. Fix: ListServiceDeployments's validation — reject a missing/unparseable service with AWS's sentence before the generic required-member check runs. Traces: ecs-ecsdep-traj-service-deployment-read-mistakes#task#6 (cluster of 3 steps).

Cause 5 of 10: ListServiceDeployments rejects a bad nextToken with the wrong class and wording. AWS: 400 InvalidParameterException, message 'Invalid parameter: NextToken' — fixed text 'Invalid parameter: ' plus the member name. Emulator: 400 ClientException 'Invalid nextToken.'. Why: a client branching on __type to decide whether to restart pagination sees ClientException, which it must treat as non-retryable. Fix: ListServiceDeployments's handler/validation (page-token decode path). Traces: ecs-ecsdep-traj-service-deployment-read-mistakes#task#10.

Cause 6 of 10: DescribeServiceDeployments over-length ARN list. AWS: 400, fixed text 'Service Deployment ARN count cannot exceed 20' (no trailing period; 20 is the documented per-call cap for Smithy's DescribeServiceDeployments.serviceDeploymentArns). Emulator: 400 'service deployments can have at most 20 items.' Why: wording-sensitive clients and operators see a message no AWS region produces. Fix: DescribeServiceDeployments's validation. Traces: ecs-ecsdep-traj-service-deployment-read-mistakes#task#15.

Cause 7 of 10: DescribeServiceRevisions over-length ARN list. AWS: 400, fixed text 'Service Revision ARN count cannot exceed 20'. Emulator: 400 'service revisions can have at most 20 items.' Fix: DescribeServiceRevisions's validation. Traces: ecs-ecsdep-traj-service-deployment-read-mistakes#task#18.

Cause 8 of 10: DescribeTasks over-length task list names the wrong member. AWS: 400, fixed text 'taskIds can have at most 100 items.' — AWS words the limit against 'taskIds' even though the wire member (Smithy: DescribeTasksRequest.tasks, @required StringList) is 'tasks'. Emulator: 400 'tasks can have at most 100 items.' Why: the emulator substitutes the wire member name for the name AWS prints, so the only variable-looking part of the message is wrong. Fix: DescribeTasks's validation. Traces: ecs-ecsnrt-traj-task-listing-and-describe-mistakes#task#8.

Cause 9 of 10: DescribeServices with no services list. AWS: 400, fixed text 'Services cannot be empty' (no trailing period) — Smithy: DescribeServicesRequest.services is @required. Emulator: 400 with the generic "1 validation error detected: Value null at 'services' failed to satisfy constraint: Member must not be null". Fix: DescribeServices's validation. Traces: ecs-escc-traj-describe-services-name-list-mistakes#task#7.

Cause 10 of 10: DeleteService refusal carries an empty message member. AWS: the recorded 400 response for this refusal has NO message member at all. Emulator: 400 with message present and set to the empty string. Why: a client that tests for the presence of message, or prints it, renders a blank reason instead of falling back to the error code. Fix: DeleteService's error serializer — omit message rather than emitting an empty one. Traces: ecs-escc-traj-deleted-service-stale-reads#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (all ten clusters have empty owned_by and empty stub_path).
Cause 1 — expected $.message 'launch type should be one of [EC2,FARGATE,EXTERNAL]'; actual 'launch type should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]' (status 400 both sides).
Cause 2/3 — expected 'launch type should be one of [EC2,FARGATE,EXTERNAL,FMI,MANAGED_INSTANCES]'; actual 'launch type should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]'.
Cause 4 — expected 'The Service ARN provided is invalid.'; actual "1 validation error detected: Value null at 'service' failed to satisfy constraint: Member must not be null".
Cause 5 — expected $.__type 'InvalidParameterException' / $.message 'Invalid parameter: NextToken'; actual 'ClientException' / 'Invalid nextToken.'.
Cause 6 — expected 'Service Deployment ARN count cannot exceed 20'; actual 'service deployments can have at most 20 items.'.
Cause 7 — expected 'Service Revision ARN count cannot exceed 20'; actual 'service revisions can have at most 20 items.'.
Cause 8 — expected 'taskIds can have at most 100 items.'; actual 'tasks can have at most 100 items.'.
Cause 9 — expected 'Services cannot be empty'; actual "1 validation error detected: Value null at 'services' failed to satisfy constraint: Member must not be null".
Cause 10 — expected $.message absent; actual $.message present and equal to ''.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecscm-traj-create-service-launch-mistakes, step task#7 — CreateService with launchType outside the accepted set; compare $.message.
3. For the other nine causes replay the fixture#step tokens in trace_ids and compare $.__type and $.message against the recorded response.
