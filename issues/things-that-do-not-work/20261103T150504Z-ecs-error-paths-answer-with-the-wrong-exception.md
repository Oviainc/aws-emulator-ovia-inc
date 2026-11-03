Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-escc-traj-describe-services-cluster-addressing-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-escc-traj-describe-services-cluster-addressing-mistakes ecs-ecsdep-traj-update-service-revision-mistakes ecs-ecsnrt-traj-task-listing-and-describe-mistakes ecs-escc-traj-mirror-custom-service-create-refusals ecs-ntraf-service-load-balancer-block-mistakes ecs-escc-traj-task-definition-platform-route ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes ecs-ecsnrt-traj-run-task-launch-selection-mistakes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|4xx->4xx|$.__type
Issue-Key: ecs|UpdateService|4xx->4xx|$.__type
Issue-Key: ecs|ListTasks|4xx->4xx|$.__type
Issue-Key: ecs|CreateService|4xx->4xx|$.__type
Issue-Key: ecs|DescribeTaskDefinition|4xx->4xx|$.message
Issue-Key: ecs|DescribeTaskDefinition|4xx->4xx|$.__type
Issue-Key: ecs|RunTask|4xx->4xx|$.message
Issue-Related: 20261029T220901Z-ecs-service-deployment-task-set-and-attribute-error.md
Issue-Related: 20261103T150504Z-ecs-validation-refusals-use-emulator-authored-wording-instead.md
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS error paths answer with the wrong exception class or a message about a different fault

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Eight net-new causes, one fault: the emulator refuses the request, but it diagnoses a DIFFERENT fault than AWS does — it checks its own conditions in the wrong order, so the error code (__type) and/or the message describe a problem the client does not have. Status is 400 on both sides throughout. Related open issues (20261029T220901Z-ecs-express-gateway-and-daemon-operations-skip-validation, 20261103T041326Z-ecs-refusal-messages-are-emulator-authored-prose) cover other operations and do not carry any of these replay keys.

Cause 1 of 8: DescribeServices for a service addressed through the wrong cluster. AWS: 400 InvalidParameterException with fixed text 'Service is not within the provided Cluster.' — AWS resolves the cluster, finds it, and then reports the service/cluster mismatch. Emulator: 400 ClusterNotFoundException 'Cluster not found.' Why: a client that handles ClusterNotFoundException by creating the cluster takes a repair action for a cluster that already exists, and never learns the service is in a different cluster. Fix: DescribeServices's handler — resolve the cluster first and report the mismatch, instead of failing cluster lookup. Traces: ecs-escc-traj-describe-services-cluster-addressing-mistakes#task#6 (cluster of 2 steps).

Cause 2 of 8: UpdateService refusal uses the wrong error class. AWS: 400 with __type ClientException for this refusal (message matches on both sides). Smithy: UpdateService models both ClientException and InvalidParameterException. Emulator: 400 with __type InvalidParameterException. Why: ClientException and InvalidParameterException are distinct modeled shapes; SDK error-specific catch blocks never fire. Fix: UpdateService's validation — raise the modeled class AWS raises for this condition. Traces: ecs-ecsdep-traj-update-service-revision-mistakes#task#11.

Cause 3 of 8: UpdateService with an empty task-definition family. AWS: 400 InvalidParameterException, fixed text 'Family should not be null or empty.' Emulator: 400 ClientException 'Family contains invalid characters.' Why: the emulator treats 'empty' as 'invalid characters', so both the class and the diagnosis are wrong and the client cannot tell a missing value from a malformed one. Fix: UpdateService's task-definition resolution — test for null/empty before the character check. Traces: ecs-ecsdep-traj-update-service-revision-mistakes#task#13 (cluster of 2 steps).

Cause 4 of 8: ListTasks with serviceName plus another mutually exclusive filter. AWS: 400 InvalidParameterException, fixed text 'cannot specify serviceName with other arguments' — the argument-combination check runs BEFORE the service is looked up. Emulator: 400 ServiceNotFoundException 'Service not found.' Why: the client is told the service does not exist when the real fault is its filter combination; a client that then creates or waits on the service loops forever. Fix: ListTasks's validation — enforce filter exclusivity ahead of the service lookup. Traces: ecs-ecsnrt-traj-task-listing-and-describe-mistakes#task#5.

Cause 5 of 8: CreateService with an unknown platformVersion. AWS: 400 PlatformUnknownException, fixed text 'The specified platform does not exist.' — Smithy models PlatformUnknownException on CreateService ('The specified platform version doesn't exist'), and AWS validates the platform version before the duplicate-name check. Emulator: 400 InvalidParameterException with a duplicate-name message whose variable parts are the requested service name and cluster name. Why: a modeled, specifically-catchable error is replaced by a generic one describing an unrelated conflict. Fix: CreateService's validation ordering. Traces: ecs-escc-traj-mirror-custom-service-create-refusals#task#9 (cluster of 2 steps).

Cause 6 of 8: DescribeTaskDefinition for a family-qualified identifier it cannot resolve. AWS: 400 with fixed text 'Unable to describe task definition.' (error class matches). Emulator: 400 'Family contains invalid characters.' Why: the message points the client at the identifier's spelling rather than at the failed lookup. Fix: DescribeTaskDefinition's handler — only report a character fault for characters AWS rejects; otherwise report the lookup failure. Traces: ecs-escc-traj-task-definition-platform-route#task#10.

Cause 7 of 8: DescribeTaskDefinition with a blank taskDefinition. AWS: 400 InvalidParameterException, fixed text 'Task Definition can not be blank.' — Smithy: DescribeTaskDefinitionRequest.taskDefinition is @required. Emulator: 400 ClientException 'Family contains invalid characters.' Fix: DescribeTaskDefinition's validation — blank check first, with AWS's class and sentence. Traces: ecs-escc-traj-task-definition-platform-route#task#12.

Cause 8 of 8: RunTask with an awsvpc networkConfiguration naming a nonexistent subnet. AWS: 400 whose message is the subnet-resolution failure: fixed text 'Error retrieving subnet information for [' + the requested subnet id list + ']: The subnet ID ' + the quoted subnet id + ' does not exist (ErrorCode: InvalidSubnetID.NotFound)'. The subnet ids are request values; the surrounding sentence and the ErrorCode token are fixed. Emulator: 400 'No Container Instances were found in your cluster.' — it skips subnet resolution and falls through to placement. Why: the client is told to add capacity when the actual fault is an unresolvable subnet, so the repair is wrong in both cases (adding instances never fixes it). Fix: RunTask's validation — resolve networkConfiguration.awsvpcConfiguration.subnets against EC2 before attempting placement. Traces: ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes#task#6 (cluster of 5 steps across ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes and ecs-ecsnrt-traj-run-task-launch-selection-mistakes).

## Evidence
Replay of trace set simparity-wire-aws-v1; all eight clusters have empty owned_by and empty stub_path, status 400 expected and 400 actual.
Cause 1 — $.__type expected 'InvalidParameterException', got 'ClusterNotFoundException'; $.message expected 'Service is not within the provided Cluster.', got 'Cluster not found.'.
Cause 2 — $.__type expected 'ClientException', got 'InvalidParameterException'.
Cause 3 — $.__type expected 'InvalidParameterException', got 'ClientException'; $.message expected 'Family should not be null or empty.', got 'Family contains invalid characters.'.
Cause 4 — $.__type expected 'InvalidParameterException', got 'ServiceNotFoundException'; $.message expected 'cannot specify serviceName with other arguments', got 'Service not found.'.
Cause 5 — $.__type expected 'PlatformUnknownException', got 'InvalidParameterException'; $.message expected 'The specified platform does not exist.', got a 'service "..." already exists in cluster "..."' sentence.
Cause 6 — $.message expected 'Unable to describe task definition.', got 'Family contains invalid characters.'.
Cause 7 — $.__type expected 'InvalidParameterException', got 'ClientException'; $.message expected 'Task Definition can not be blank.', got 'Family contains invalid characters.'.
Cause 8 — $.message expected "Error retrieving subnet information for [<subnet-id>]: The subnet ID '<subnet-id>' does not exist (ErrorCode: InvalidSubnetID.NotFound)", got 'No Container Instances were found in your cluster.'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-escc-traj-describe-services-cluster-addressing-mistakes, step task#6 — DescribeServices naming a service that lives in another cluster; compare $.__type and $.message.
3. Replay each fixture#step in trace_ids and compare $.__type and $.message against the recorded response.
