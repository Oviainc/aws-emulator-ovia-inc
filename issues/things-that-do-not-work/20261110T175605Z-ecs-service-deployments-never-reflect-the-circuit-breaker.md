Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop ecs-service-deployment-error-paths
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].taskDefinition
Issue-Key: ecs|ListServiceDeployments|2xx->2xx|$.serviceDeployments[*].status
Issue-Key: ecs|ListServiceDeployments|2xx->2xx|$.serviceDeployments[*].statusReason
Issue-Key: ecs|DescribeServiceDeployments|2xx->2xx|$.serviceDeployments[*].deploymentCircuitBreaker.status
Issue-Key: ecs|DescribeServiceDeployments|2xx->2xx|$.serviceDeployments[*].rollback
Issue-Key: ecs|DescribeServiceDeployments|2xx->2xx|$.serviceDeployments[*].status
Issue-Key: ecs|DescribeServiceDeployments|2xx->2xx|$.serviceDeployments[*].statusReason
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].rolloutState
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].rolloutStateReason
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].runningCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].runningCount
Issue-Key: ecs|ListTasks|2xx->2xx|$
Issue-Key: ecs|ListTasks|2xx->2xx|$.taskArns
Issue-Key: ecs|DescribeServiceDeployments|4xx->4xx|$.__type
Issue-Related: 20261103T150504Z-ecs-listings-and-describes-never-reflect-deletion-or.md
Issue-Related: 20261103T150504Z-ecs-service-responses-drop-deployment-configuration-task-sets.md
Issue-Related: 20261103T150504Z-ecs-validation-refusals-use-emulator-authored-wording-instead.md
Issue-Commit: 54b3fe10738e925f02de63999e233b2f22231166

# ECS service deployments never reflect the circuit breaker: failed deployments report SUCCESSFUL/COMPLETED, no rollback happens, and a malformed deployment ARN is misclassified

> _Note: this issue was identified against the code base at commit `54b3fe10738e925f02de63999e233b2f22231166`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Nine net-new ECS wire causes, bundled because they share one fault: the emulator's service-deployment state machine never evaluates the deployment circuit breaker. Every deployment it runs is declared successful, so neither the rollback outcome nor the stop outcome AWS produces is ever observable, and the service-deployment resource it reports back (ARN form, status, statusReason, rollback, deploymentCircuitBreaker) and its ARN validation diverge with it. All nine causes must be closed before this issue is submitted.

NOTE ON NEIGHBOURING OPEN ISSUES: 20261105T015737Z-ecs-responses-diverge-service-connect-endpoint-failure-arn.md (cause 1) and 20261106T135659Z-ecs-fargate-tasks-and-deployments-never-advance-task.md (cause 3) both describe this fixture's step task#15 as a poll that NEVER answered (fixture budget spent). The emulator now answers every step of the fixture, so the replay keys below are different and are not owned by either issue: the deployment now runs to completion and reports the WRONG terminal outcome. 20261107T170944Z-ecs-circuit-breaker-deployment-never-reaches-a-terminal.md and 20261106T115645Z-ecs-3-net-new-causes-a-cluster-scoped.md touch the same area and carry none of these replay keys.

Cause 1 of 9: a circuit-breaker rollback never happens, so the service and its PRIMARY deployment still point at the failing revision (operation DescribeServices; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15).
AWS: Smithy com.amazonaws.ecs DescribeServices -> Service.deployments (Deployment) with deploymentConfiguration.deploymentCircuitBreaker {enable, rollback}; ServiceDeploymentCircuitBreaker is documented as "Information about the circuit breaker used to determine when a service deployment has failed" and "has an option that will automatically roll back a failed deployment", with a calculated threshold (minimum 3, maximum 200, formula 0.5 * desired task count). With enable=true and rollback=true, once the launch failures reach the threshold AWS fails the new deployment and starts a replacement deployment onto the last known-good task-definition revision; after that the service's taskDefinition and its PRIMARY deployment's taskDefinition both name the PREVIOUS (good) revision, rolloutState COMPLETED, rolloutStateReason "ECS deployment <deployment-id> completed." and runningCount equal to the desired count.
Emulator: the bad deployment is reported COMPLETED and never rolled back. The service's taskDefinition and deployments[0].taskDefinition both still name the revision whose tasks cannot start, and the PRIMARY deployment is the bad deployment (the emulator's third deployment id) rather than a rollback deployment onto the good revision.
Why: a client that deploys a broken revision with rollback armed is told the rollout succeeded and is left serving the broken revision. Every supervisor that reads Service.taskDefinition/deployments to confirm which revision is live concludes the opposite of the truth, and the rollback the feature exists to provide never occurs.
Fix: the ECS service scheduler / service-deployment state machine in the ecs service crate's state — evaluate the circuit breaker against the deployment's task launch failures, fail the deployment at the threshold, and create the rollback deployment onto the previous service revision when deploymentCircuitBreaker.rollback is true; DescribeServices then serializes it.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15.

Cause 2 of 9: ListServiceDeployments reports a rolled-back deployment as SUCCESSFUL with emulator-authored statusReason, and the serviceDeploymentArn's last segment is a sequential counter instead of an opaque deployment id (operation ListServiceDeployments; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#16).
AWS: Smithy ServiceDeploymentBrief.status of shape ServiceDeploymentStatus, whose value set is [PENDING, SUCCESSFUL, STOPPED, STOP_REQUESTED, IN_PROGRESS, ROLLBACK_REQUESTED, ROLLBACK_IN_PROGRESS, ROLLBACK_SUCCESSFUL, ROLLBACK_FAILED]; statusReason is "Information about why the service deployment is in the current status. For example, the circuit breaker detected a deployment failure." A deployment the circuit breaker rolled back ends at status ROLLBACK_SUCCESSFUL with statusReason "Service deployment rolled back because the circuit breaker threshold was exceeded." (that whole sentence is fixed text — it carries no request or state values). serviceDeploymentArn is of the form arn:aws:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<deployment-id>, where <deployment-id> is an opaque 21-character id (recorded instances RdNbXTA5n85xYxxM3OybC, pAvBEewqs-m4ALRCjLEwz — those particular ids are per-run values; the opaque form is the requirement).
Emulator: status 'SUCCESSFUL' and statusReason 'ECS deployment ecs-svc/<counter> completed.' — a sentence AWS does not emit for this status at all — and the ARN's last segment is a 19-digit zero-padded counter (e.g. 0000000000000000003) rather than an opaque id.
Why: a client filtering ListServiceDeployments by status (the modelled `status` filter) can never find rolled-back deployments, and one that keys on the opaque deployment id form mis-parses the ARN. The statusReason is the only text telling an operator why the deployment ended as it did.
Fix: the ListServiceDeployments handler's ServiceDeploymentBrief projection plus the service-deployment record in state — set status/statusReason from the deployment's real terminal outcome, and mint the deployment id in the opaque form.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#16.

Cause 3 of 9: DescribeServiceDeployments omits the rollback member and reports the circuit breaker as MONITORING_COMPLETE for a deployment the breaker tripped (operation DescribeServiceDeployments; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#17).
AWS: Smithy ServiceDeployment.rollback of shape Rollback {reason, startedAt, serviceRevisionArn} — "Information about the service deployment rollback", reason being "The reason the rollback happened. For example, the circuit breaker initiated the rollback operation." and serviceRevisionArn "The ARN of the service revision deployed as part of the rollback"; and ServiceDeployment.deploymentCircuitBreaker.status of shape ServiceDeploymentRollbackMonitorsStatus with values [TRIGGERED, MONITORING, MONITORING_COMPLETE, DISABLED]. For a deployment the breaker rolled back, AWS reports status ROLLBACK_SUCCESSFUL, statusReason "Service deployment rolled back because the circuit breaker threshold was exceeded.", deploymentCircuitBreaker.status TRIGGERED, and a populated rollback whose reason is that same sentence, whose serviceRevisionArn names the service revision rolled back to and whose startedAt is the rollback time (the ARN and timestamp are per-run values).
Emulator: rollback is absent (None), deploymentCircuitBreaker.status is 'MONITORING_COMPLETE' (the breaker watched and declared success), status is 'SUCCESSFUL' and statusReason is 'ECS deployment ecs-svc/<counter> completed.'.
Why: MONITORING_COMPLETE with no rollback member is the wire signature of a healthy deployment, so a client cannot detect that the breaker fired or learn which revision it rolled back to — the audit trail of the rollback is simply missing.
Fix: the DescribeServiceDeployments handler's ServiceDeployment serializer plus the deployment record in state — persist the breaker outcome (TRIGGERED) and the rollback {reason, startedAt, serviceRevisionArn} when the breaker rolls a deployment back.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#17.

Cause 4 of 9: with rollback disabled, the failing deployment is reported COMPLETED instead of FAILED, so a DescribeServices poll for the breaker verdict never converges (operation DescribeServices; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#18).
AWS: Smithy Deployment.rolloutState of shape DeploymentRolloutState with values [COMPLETED, FAILED, IN_PROGRESS], plus rolloutStateReason. With deploymentCircuitBreaker {enable:true, rollback:false}, AWS trips the breaker and leaves the primary deployment at rolloutState FAILED with rolloutStateReason "ECS deployment circuit breaker: tasks failed to start." (fixed text), the deployment's runningCount 0 and the service's runningCount 0 — the tasks that could not start are not counted as running.
Emulator: the deployment reaches rolloutState 'COMPLETED' with rolloutStateReason 'ECS deployment ecs-svc/<counter> completed.', deployment runningCount 1 and service runningCount 1, so the poll waiting for a FAILED rolloutState exhausts its 180s cap. The emulator also reports a task as running for a revision whose tasks cannot start.
Why: a rollout gate that waits for FAILED to stop a release never sees it and either hangs or is told a broken deployment succeeded; the fixture's whole point — distinguishing breaker-with-rollback from breaker-without-rollback — is unobservable.
Fix: the ECS deployment/rollout state machine in the ecs service crate's state — count only tasks that actually started, trip the breaker at its threshold, and set rolloutState FAILED with the documented rolloutStateReason when rollback is not enabled.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#18.

Cause 5 of 9: ListServiceDeployments reports a breaker-stopped deployment as SUCCESSFUL (operation ListServiceDeployments; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#19).
AWS: Smithy ServiceDeploymentStatus includes STOPPED. With the circuit breaker enabled and rollback disabled, the failed deployment ends at status STOPPED with statusReason "Service deployment stopped because the circuit breaker threshold was exceeded." (fixed text), under a serviceDeploymentArn whose last segment is the opaque deployment id.
Emulator: status 'SUCCESSFUL' with statusReason 'ECS deployment ecs-svc/<counter> completed.', and the counter-form ARN segment of cause 2.
Why: the stopped-vs-rolled-back distinction is the only way a client can tell what the breaker did; reporting both as SUCCESSFUL hides a failed release entirely.
Fix: the ListServiceDeployments projection over the deployment's terminal outcome in state (same component as cause 2).
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#19.

Cause 6 of 9: DescribeServiceDeployments reports the breaker-stopped deployment as SUCCESSFUL / MONITORING_COMPLETE (operation DescribeServiceDeployments; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#21).
AWS: as in causes 3 and 5 — status STOPPED, statusReason "Service deployment stopped because the circuit breaker threshold was exceeded.", deploymentCircuitBreaker.status TRIGGERED (ServiceDeploymentRollbackMonitorsStatus).
Emulator: status 'SUCCESSFUL', statusReason 'ECS deployment ecs-svc/<counter> completed.', deploymentCircuitBreaker.status 'MONITORING_COMPLETE'.
Why: the detailed read of the deployment agrees with the list view that nothing went wrong, so no client can observe that the breaker stopped the rollout.
Fix: the DescribeServiceDeployments serializer plus the stored breaker outcome (same component as cause 3).
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#21.

Cause 7 of 9: the tasks of a breaker-stopped deployment stay listed as running, so ListTasks never drains (operation ListTasks; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#23).
AWS: Smithy ListTasks -> taskArns, with the default desiredStatus RUNNING. Tasks launched by a deployment the circuit breaker stopped fail to start and are moved to STOPPED, so a ListTasks for the cluster/service returns an empty taskArns list once the breaker has fired.
Emulator: taskArns keeps one task ARN for the service indefinitely — the poll reports "1 item(s), not 0 after 19 attempt(s)" and is capped at 180s — because the task that could never start is held in a running state.
Why: a client that drains or reconciles a failed deployment by listing its running tasks sees a task that does not exist on AWS, so cleanup, scale-to-zero and teardown logic all block.
Fix: the ECS task lifecycle in the ecs service crate's state — a task of a deployment whose launches fail must transition to STOPPED (and leave the RUNNING-desiredStatus listing) rather than remain running.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#23.

Cause 8 of 9: DescribeServices keeps reporting the stopped deployment as COMPLETED with a running task after the breaker fired (operation DescribeServices; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#24).
AWS: as in cause 4 — the service's runningCount is 0, and its primary deployment carries rolloutState FAILED with rolloutStateReason "ECS deployment circuit breaker: tasks failed to start." and runningCount 0.
Emulator: service runningCount 1, deployments[0].rolloutState 'COMPLETED', rolloutStateReason 'ECS deployment ecs-svc/<counter> completed.', deployments[0].runningCount 1 — this is the settled, no-longer-polling read, so the wrong values are terminal and not a timing artefact.
Why: the service's steady-state description permanently misreports a failed rollout as healthy, including its capacity, so monitoring and remediation never trigger.
Fix: the deployment/rollout state machine and the Service serializer behind DescribeServices (same component as cause 4).
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#24.

Cause 9 of 9: DescribeServiceDeployments misclassifies a malformed serviceDeploymentArn as a missing cluster (operation DescribeServiceDeployments; fixture ecs-service-deployment-error-paths, step task#3).
AWS: Smithy DescribeServiceDeployments takes the required serviceDeploymentArns (StringList) and models InvalidParameterException, ClusterNotFoundException, ServiceNotFoundException, ClientException, AccessDeniedException, ServerException and UnsupportedFeatureException. An entry that is not a well-formed service-deployment ARN is rejected before any cluster lookup: HTTP 400 with __type InvalidParameterException and message "The ServiceDeployment ARN is invalid." — the whole message is fixed text, with no request or state values in it.
Emulator: HTTP 400 but __type 'ClusterNotFoundException' with message 'Cluster not found.' — it parses the malformed ARN far enough to extract a cluster segment and then reports that cluster as missing.
Why: the error code is the branch clients switch on. A caller that passed a bad ARN is told its CLUSTER does not exist, so it retries against, or re-creates, a cluster that is fine, and never corrects the actual bad argument.
Fix: the DescribeServiceDeployments handler's input validation — validate each serviceDeploymentArns entry's ARN form first and raise InvalidParameterException with the documented message before resolving the cluster.
Traces: ecs-service-deployment-error-paths#task#3.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 118 fixtures, 1380 steps, 1206 passed, score 87.4%, 89 divergence clusters. The clusters below are neither blocked by an unimplemented-operation stub nor owned by an open issue's replay key.

Cause 1 — ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15, DescribeServices, expected 200 / actual 200:
  $.services[0].taskDefinition: value_mismatch (expected '...task-definition/<good-revision>:1', got '...task-definition/<bad-revision>:1')
  $.services[0].deployments[0]: expected {'id': 'ecs-svc/<rollback-deployment>', 'status': 'PRIMARY', 'rolloutState': 'COMPLETED', 'rolloutStateReason': 'ECS deployment <id> completed.', 'taskDefinition': '<good-revision>:1', 'runningCount': 1}, got the bad-revision deployment ('taskDefinition': '<bad-revision>:1').

Cause 2 — ...#task#16, ListServiceDeployments, 200/200:
  $.serviceDeployments[0].status: value_mismatch (expected 'ROLLBACK_SUCCESSFUL', got 'SUCCESSFUL')
  $.serviceDeployments[0].statusReason: value_mismatch (expected 'Service deployment rolled back because the circuit breaker threshold was exceeded.', got 'ECS deployment ecs-svc/0000000000000000003 completed.')
  $.serviceDeployments[0].serviceDeploymentArn / [1].serviceDeploymentArn: last segment expected an opaque 21-char id (recorded: RdNbXTA5n85xYxxM3OybC, pAvBEewqs-m4ALRCjLEwz), got 0000000000000000003 / 0000000000000000001.

Cause 3 — ...#task#17, DescribeServiceDeployments, 200/200:
  $.serviceDeployments[0].status: expected 'ROLLBACK_SUCCESSFUL', got 'SUCCESSFUL'
  $.serviceDeployments[0].statusReason: expected 'Service deployment rolled back because the circuit breaker threshold was exceeded.', got 'ECS deployment ecs-svc/0000000000000000003 completed.'
  $.serviceDeployments[0].rollback: missing (expected {'reason': 'Service deployment rolled back because the circuit breaker threshold was exceeded.', 'serviceRevisionArn': '<service-revision>', 'startedAt': <timestamp>}, got None)
  $.serviceDeployments[0].deploymentCircuitBreaker.status: expected 'TRIGGERED', got 'MONITORING_COMPLETE'

Cause 4 — ...#task#18, DescribeServices (poll), 200/200, poll capped at 180s:
  $.services[0].deployments[0].rolloutState: expected 'FAILED', got 'COMPLETED'
  $.services[0].deployments[0].rolloutStateReason: expected 'ECS deployment circuit breaker: tasks failed to start.', got 'ECS deployment ecs-svc/0000000000000000002 completed.'
  $.services[0].runningCount and deployments[0].runningCount: expected 0, got 1

Cause 5 — ...#task#19, ListServiceDeployments, 200/200:
  $.serviceDeployments[0].status: expected 'STOPPED', got 'SUCCESSFUL'
  $.serviceDeployments[0].statusReason: expected 'Service deployment stopped because the circuit breaker threshold was exceeded.', got 'ECS deployment ecs-svc/0000000000000000002 completed.'
  $.serviceDeployments[0].serviceDeploymentArn: opaque id expected (recorded DmZNL9uudhPP6-rrV3k6M), got 0000000000000000002

Cause 6 — ...#task#21, DescribeServiceDeployments, 200/200:
  $.serviceDeployments[0].status: expected 'STOPPED', got 'SUCCESSFUL'
  $.serviceDeployments[0].statusReason: expected 'Service deployment stopped because the circuit breaker threshold was exceeded.', got 'ECS deployment ecs-svc/0000000000000000002 completed.'
  $.serviceDeployments[0].deploymentCircuitBreaker.status: expected 'TRIGGERED', got 'MONITORING_COMPLETE'

Cause 7 — ...#task#23, ListTasks, 200/200, poll capped at 180s:
  $.taskArns: poll: 1 item(s), not 0 after 19 attempt(s) (expected [], got ['arn:aws:ecs:<region>:<account>:task/<cluster>/<task-id>'])

Cause 8 — ...#task#24, DescribeServices, 200/200:
  $.services[0].runningCount: expected 0, got 1
  $.services[0].deployments[0].rolloutState: expected 'FAILED', got 'COMPLETED'
  $.services[0].deployments[0].rolloutStateReason: expected 'ECS deployment circuit breaker: tasks failed to start.', got 'ECS deployment ecs-svc/0000000000000000002 completed.'
  $.services[0].deployments[0].runningCount: expected 0, got 1

Cause 9 — ecs-service-deployment-error-paths#task#3, DescribeServiceDeployments, expected 400 / actual 400:
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'ClusterNotFoundException')
  $.message: value_mismatch (expected 'The ServiceDeployment ARN is invalid.', got 'Cluster not found.')

The cluster/service/task-definition names, task ids, deployment ids and timestamps shown are per-run probe values; the requirements are the statuses, the status/rolloutState reason texts (which are fixed sentences), the presence of the rollback member, the opaque deployment-id form, the zero running counts and the error code/message of cause 9.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay trace set simparity-wire-aws-v1.
2. Causes 1-3: replay fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop through step task#17 — create a cluster, register a good and a bad (tasks cannot start) task-definition revision, CreateService on the good revision with deploymentConfiguration.deploymentCircuitBreaker {enable:true, rollback:true}, UpdateService onto the bad revision, then DescribeServices (task#15), ListServiceDeployments (task#16) and DescribeServiceDeployments (task#17) for the service. The service stays on the bad revision, the deployment is reported SUCCESSFUL/COMPLETED, statusReason is the emulator's 'ECS deployment ecs-svc/<counter> completed.', rollback is absent and deploymentCircuitBreaker.status is MONITORING_COMPLETE; AWS reports the rollback deployment onto the good revision, ROLLBACK_SUCCESSFUL, 'Service deployment rolled back because the circuit breaker threshold was exceeded.' and TRIGGERED.
3. Causes 4-8: continue the same fixture through steps task#18, task#19, task#21, task#23 and task#24 — a second service with deploymentCircuitBreaker {enable:true, rollback:false} deployed onto the bad revision. DescribeServices polls for rolloutState FAILED and never sees it (COMPLETED, runningCount 1), ListServiceDeployments/DescribeServiceDeployments report SUCCESSFUL/MONITORING_COMPLETE instead of STOPPED/TRIGGERED, and ListTasks never drains to an empty taskArns list.
4. Cause 9: replay fixture ecs-service-deployment-error-paths to step task#3 — call DescribeServiceDeployments with a serviceDeploymentArns entry that is not a well-formed service-deployment ARN. The emulator answers 400 ClusterNotFoundException 'Cluster not found.'; AWS answers 400 InvalidParameterException 'The ServiceDeployment ARN is invalid.'.
