Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.execute-command-without-enable-flag-rejected ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider ecs.delete-cluster-with-capacity-provider-rejected ecs.e2e-service-deployment-rollback-on-failed-taskdef
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RunTask|InvalidParameterException
Issue-Key: ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider|_poll_until#13|Timeout|last = probe()
Issue-Key: ecs.delete-cluster-with-capacity-provider-rejected|_create_once#8|Timeout|time.sleep(2)
Issue-Key: ecs.e2e-service-deployment-rollback-on-failed-taskdef|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# ECS: RunTask ignores overrides.taskRoleArn when gating an exec-enabled launch, and three capacity-provider / deployment-lifecycle scenarios never reach a verdict (4 causes)

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Four net-new root causes in the ECS emulator, all grounded in failing behavioural checks that the replay report did NOT mark ALREADY FILED or OWNED. Related open issues exist but none carries these replay keys: 20261104T062603Z-ecs-exec-the-executecommandagent-managed-agent-never-reaches.md (ECS Exec agent), 20261104T062603Z-ecs-runtask-clienttoken-is-ignored-capacity-provider-base.md (capacity-provider base/weight), 20261104T062603Z-ecs-rolling-deployment-controls-are-not-enforced-the.md (rolling controls), 20261029T175650Z-ecs-7-net-new-root-causes-in-state.md (capacity-provider state). Each cause below must be closed before submitting.

Cause 1 of 4: RunTask refuses an exec-enabled Fargate launch whose task role is supplied by overrides.taskRoleArn.
AWS: Smithy (com-amazonaws-ecs-amazonec2containerservicev20141113) models RunTask.overrides -> TaskOverride, whose member taskRoleArn is the IAM role the launched task assumes; RunTask's modelled client errors include InvalidParameterException (HTTP 400). AWS resolves a task's IAM role from overrides.taskRoleArn when that member is present and falls back to the task definition's taskRoleArn only when it is absent, so a RunTask carrying enableExecuteCommand=true plus overrides={"taskRoleArn": <role trusted by ecs-tasks.amazonaws.com with the ssmmessages grants>} is ACCEPTED: HTTP 200 with tasks[0] populated and failures empty. The refusal message the emulator emits is AWS's message for the case where NO task role is resolvable at all.
Emulator: the call is refused with HTTP 400 InvalidParameterException and the message "The task couldn't be launched because a valid taskRoleArn is not being used. Specify a valid task role in your task definition and try again." The whole two-sentence text is fixed AWS wording with no variable parts, so the divergence is not the wording but that it is emitted at all: the emulator looks only at the task definition's taskRoleArn and never consults overrides.taskRoleArn. In the failing check the role was created, trusted to ecs-tasks.amazonaws.com and passed only via overrides.
Why: every ECS Exec scenario begins with an exec-enabled positive-control RunTask that supplies the task role through overrides; a client that follows AWS's documented override precedence cannot launch any task at all, and the refusal points it at its task definition, where nothing is wrong.
Fix: the RunTask handler's task-role resolution and the exec-enabled task-role validation that precedes placement (ops/run_task.rs in the ecs service crate) must prefer overrides.taskRoleArn over the task definition's value and only refuse when neither resolves.
Traces: ecs.execute-command-without-enable-flag-rejected

Cause 2 of 4: a task that names neither launchType nor capacityProviderStrategy is never placed by the cluster's default strategy and never reaches a terminal state.
AWS: recorded behavior only for the placement outcome. RunTask with no launchType and no capacityProviderStrategy is placed using the cluster's defaultCapacityProviderStrategy (set via CreateCluster/PutClusterCapacityProviders), and the resulting Task reports capacityProviderName equal to that provider (here FARGATE_SPOT) while an explicit capacityProviderStrategy on the same cluster overrides it (FARGATE). DescribeClusters echoes defaultCapacityProviderStrategy exactly as set, and each task's container really runs and the task then settles to lastStatus STOPPED.
Emulator: the scenario never reaches a verdict. The check is killed by the harness at 60s while polling DescribeTasks for lastStatus STOPPED, so neither the default-strategy attribution nor the explicit-strategy override is ever observable; the tasks do not progress to a terminal lifecycle.
Why: a client cannot tell which capacity provider placed a task, and a caller that waits for a RunTask task to finish waits forever.
Fix: the task lifecycle progression in the ECS state/scheduler that advances a placed task to STOPPED, and the placement code that stamps Task.capacityProviderName from the cluster's default strategy (the ecs service crate's run_task handler and task-state machine).
Traces: ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider

Cause 3 of 4: DeleteCluster on a cluster with a cluster-scoped (Managed Instances) capacity provider attached never produces the modelled refusal.
AWS: Smithy models DeleteCluster with errors ClientException, ClusterContainsCapacityProviderException, ClusterContainsContainerInstancesException, ClusterContainsServicesException, ClusterContainsTasksException, UpdateInProgressException (all HTTP 400). For an attached cluster-scoped capacity provider AWS answers HTTP 400 ClientException with the fixed message "The Cluster cannot be deleted while Cluster Scoped Capacity Providers are attached to the cluster." (the text has no variable parts), and the refusal has no effect: DescribeClusters still shows the cluster not INACTIVE and still lists the provider in capacityProviders. A plain cluster with no association deletes cleanly and settles to INACTIVE.
Emulator: the scenario never reaches the refusal. The check is killed at 60s inside the setup/settle path for a managedInstancesProvider capacity provider (CreateCapacityProvider -> DescribeCapacityProviders polling for a terminal status, then PutClusterCapacityProviders, then DeleteCluster), so the ClientException contract above is never exercised and a client gets no refusal it can rely on.
Why: a client that deletes a cluster still carrying a cluster-scoped provider sees no AWS-shaped refusal, and tooling that distinguishes UpdateInProgressException from the capacity-provider refusal cannot do so.
Fix: CreateCapacityProvider/DescribeCapacityProviders for managedInstancesProvider must settle a created provider to a terminal status, and the DeleteCluster handler must refuse with ClientException and the exact message above while leaving cluster state and its capacityProviders list untouched (delete_cluster and create_capacity_provider handlers plus the capacity-provider state in the ecs service crate).
Traces: ecs.delete-cluster-with-capacity-provider-rejected

Cause 4 of 4: the deployment circuit breaker never fails a bad revision's deployment and never rolls the service back.
AWS: Smithy models UpdateService with deploymentConfiguration -> DeploymentConfiguration.deploymentCircuitBreaker {enable, rollback}; errors are the modelled set (InvalidParameterException, ServiceNotFoundException, ...). With enable=true and rollback=true, updating a service to a revision whose image can never be pulled makes that deployment settle to rolloutState FAILED with a rolloutStateReason attributing the failure to the circuit breaker, the running tasks return to the prior stable revision, no task of the rejected revision survives, DescribeServices settles back to a single PRIMARY deployment on the prior revision with runningCount == desiredCount, and ListServiceDeployments/DescribeServiceDeployments surface a record whose serviceArn matches the service.
Emulator: the scenario never reaches a verdict. The check is killed at 60s; the deployment bookkeeping never produces a FAILED rolloutState for the bad revision and the rollback is never observable.
Why: a client relying on the circuit breaker to protect a rollout gets no FAILED signal and no rollback, so a broken revision looks like a successful deployment.
Fix: the service deployment state machine that evaluates the circuit breaker and performs the rollback, and the DescribeServices/ListServiceDeployments/DescribeServiceDeployments projections of it (the ecs service crate's service-deployment state and update_service handler).
Traces: ecs.e2e-service-deployment-rollback-on-failed-taskdef

## Evidence
Cause 1 — expected: RunTask(enableExecuteCommand=true, overrides={taskRoleArn: <trusted role>}) => HTTP 200, tasks[0].taskArn set, failures=[]. Actual: HTTP 400 InvalidParameterException, message "The task couldn't be launched because a valid taskRoleArn is not being used. Specify a valid task role in your task definition and try again." (botocore.errorfactory.InvalidParameterException raised from client.py _make_api_call for operation RunTask).

Cause 2 — expected: DescribeClusters returns defaultCapacityProviderStrategy == [{capacityProvider: FARGATE_SPOT, weight: 1, base: 0}]; the strategy-less task reports capacityProviderName FARGATE_SPOT, the explicit one FARGATE, and both reach lastStatus STOPPED. Actual: "Check timed out after 60.0s" while polling DescribeTasks for STOPPED — no task reaches a terminal state.

Cause 3 — expected: DeleteCluster => HTTP 400, Error.Code "ClientException", Error.Message exactly "The Cluster cannot be deleted while Cluster Scoped Capacity Providers are attached to the cluster."; DescribeClusters afterwards shows status != INACTIVE and the provider still in capacityProviders. Actual: "Check timed out after 60.0s" — the Managed Instances provider never settles to a terminal status, so DeleteCluster is never reached and no refusal is produced.

Cause 4 — expected: the deployment whose taskDefinition is the unpullable revision reaches rolloutState "FAILED" with rolloutStateReason containing "circuit breaker"; running tasks settle back on the prior revision; the PRIMARY deployment references the prior revision with runningCount == desiredCount == 1; DescribeServiceDeployments[0].serviceArn equals the service's serviceArn. Actual: "Check timed out after 60.0s" — no FAILED rolloutState and no rollback is ever observed.

## Steps to Reproduce
1. Replay the behavioural suite against the emulator and re-run exactly these checks: ecs.execute-command-without-enable-flag-rejected, ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider, ecs.delete-cluster-with-capacity-provider-rejected, ecs.e2e-service-deployment-rollback-on-failed-taskdef.
2. Cause 1 directly: CreateCluster; RegisterTaskDefinition (FARGATE, awsvpc, cpu 256, memory 512, executionRoleArn set, NO taskRoleArn); create an IAM role trusted by ecs-tasks.amazonaws.com; RunTask(cluster, taskDefinition, launchType=FARGATE, count=1, enableExecuteCommand=true, overrides={"taskRoleArn": <that role ARN>}, networkConfiguration.awsvpcConfiguration) — observe the 400 InvalidParameterException instead of a 200 with a task.
3. Cause 2: CreateCluster with capacityProviders [FARGATE, FARGATE_SPOT] and defaultCapacityProviderStrategy [{capacityProvider: FARGATE_SPOT, weight: 1, base: 0}]; RunTask with no launchType and no capacityProviderStrategy, then RunTask with capacityProviderStrategy [{capacityProvider: FARGATE, weight: 1}]; poll DescribeTasks for lastStatus STOPPED and compare capacityProviderName.
4. Cause 3: CreateCluster; CreateCapacityProvider with managedInstancesProvider (infrastructureRoleArn, instanceLaunchTemplate.ec2InstanceProfileArn, networkConfiguration, monitoring=basic); poll DescribeCapacityProviders for a terminal status; PutClusterCapacityProviders to attach it; call DeleteCluster and inspect Error.Code / Error.Message, then DescribeClusters.
5. Cause 4: CreateService with deploymentConfiguration.deploymentCircuitBreaker {enable: true, rollback: true} on a good revision; wait for one RUNNING task; RegisterTaskDefinition a revision with an unpullable image; UpdateService to it; poll DescribeServices for that deployment's rolloutState/rolloutStateReason and for the rollback.
