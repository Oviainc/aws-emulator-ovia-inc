Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-service-lifecycle-on-cluster
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-service-lifecycle-on-cluster ecs-task-set-external-deployment-workflow ecs-taskset-basic-lifecycle ecs-taskset-errors ecs-taskset-lifecycle-ec2 ecs-err-delete-cluster-contains-services ecs-err-service-deployment-absent-arn ecs-err-service-not-active
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateService|2xx->2xx|$.service.createdBy
Issue-Key: ecs|CreateService|2xx->2xx|$.service.currentServiceRevisions
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deploymentConfiguration.bakeTimeInMinutes
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deploymentConfiguration.deploymentCircuitBreaker
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deploymentConfiguration.strategy
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deploymentController
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].failedLaunchTaskCount
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].healthyTaskCount
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].replacedTaskCount
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].rolloutState
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].scale
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].stabilityStatus
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].totalFailedTasks
Issue-Key: ecs|CreateService|2xx->2xx|$.service.extendDeploymentTimePeriod
Issue-Key: ecs|CreateService|2xx->2xx|$.service.healthCheckGracePeriodSeconds
Issue-Key: ecs|CreateService|2xx->2xx|$.service.version
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*]
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.createdBy
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.currentServiceRevisions
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deploymentConfiguration.bakeTimeInMinutes
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deploymentConfiguration.deploymentCircuitBreaker
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deploymentConfiguration.strategy
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deploymentController
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].failedLaunchTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].healthyTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].replacedTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].scale
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].stabilityStatus
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].totalFailedTasks
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.events[*]
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.extendDeploymentTimePeriod
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.healthCheckGracePeriodSeconds
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.runStatus
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.version
Issue-Key: ecs|DescribeServices|2xx->2xx|$
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].createdBy
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].currentServiceRevisions
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.bakeTimeInMinutes
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.deploymentCircuitBreaker
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.strategy
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentController
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].failedLaunchTaskCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].healthyTaskCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].replacedTaskCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].scale
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].stabilityStatus
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].totalFailedTasks
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].extendDeploymentTimePeriod
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].healthCheckGracePeriodSeconds
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].runStatus
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].status
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].version
Issue-Key: ecs|CreateService|2xx->2xx|$.service.roleArn
Issue-Key: ecs|UpdateService|4xx->2xx|$.__type
Issue-Key: ecs|UpdateService|4xx->2xx|$.message
Issue-Key: ecs|UpdateService|4xx->2xx|status
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS service projection drops deployment defaults and services never reach INACTIVE

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
The emulator's Service projection omits most of the members AWS materialises with defaults (createdBy, version, roleArn, deploymentController, the deploymentConfiguration defaults, the deployment's stability/scale/failure counters, currentServiceRevisions, events, healthCheckGracePeriodSeconds, extendDeploymentTimePeriod), and the service's own lifecycle never advances: deployments report COMPLETED/STEADY_STATE immediately, no service events are recorded, a deleted service disappears instead of becoming INACTIVE, and UpdateService on a non-ACTIVE service is accepted instead of refused.

THIS BUNDLE CONTAINS 8 CAUSES.

Cause 1 of 8 — CreateService response omits the defaulted service members and reports the deployment already complete.
AWS: Smithy: CreateService -> CreateServiceResponse.service : Service; Service.createdBy ("The principal that created the service"), Service.deploymentController, Service.deploymentConfiguration, Service.currentServiceRevisions, Service.healthCheckGracePeriodSeconds, Service.deployments : Deployments. Recording: createdBy set, deploymentController={type: ECS}, deploymentConfiguration carries bakeTimeInMinutes=0, strategy=ROLLING and a full deploymentCircuitBreaker, deployments[0] carries failedLaunchTaskCount/healthyTaskCount/replacedTaskCount/totalFailedTasks=0, scale={unit: PERCENT, value: 0.0}, stabilityStatus=STABILIZING and rolloutState=IN_PROGRESS, plus version=0 and extendDeploymentTimePeriod=0.
Emulator: all of those members are absent and deployments[0].rolloutState is 'COMPLETED' at creation time. Status 200 both sides.
Why: a client that waits on rolloutState/stabilityStatus to observe a rollout sees a deployment that is finished before it began, and one that reads deploymentConfiguration or deploymentController to learn the circuit-breaker/controller settings gets nothing.
Fix: the service projection and deployment bookkeeping in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs and the service record in services/.../src/state.rs.
Traces: ecs-service-lifecycle-on-cluster#task#3.

Cause 2 of 8 — DescribeServices returns the same impoverished service object.
AWS: Smithy: DescribeServices -> services : Services. Recording: the service carries createdBy, currentServiceDeployment, currentServiceRevisions, the full deploymentConfiguration/deploymentController, a populated events list and version.
Emulator: those members are absent from the described service (status 200).
Why: service state is unreadable, so no client-side reconciliation loop can work.
Fix: services/.../src/ops/describe_services.rs projection.
Traces: ecs-service-lifecycle-on-cluster#task#4 (+1 member, ecs-service-lifecycle-on-cluster#task#6).

Cause 3 of 8 — UpdateService response omits the same members and records no service events.
AWS: Smithy: Service.events : ServiceEvents ("The event stream for your service"); Service.runStatus. Recording: events carries the steady-state and deployment-completed entries, runStatus='SteadyState', stabilityStatus='STABILIZING', scale={unit: PERCENT, value: 100.0}, version=0.
Emulator: events absent, runStatus absent, and the deployment/configuration members above absent.
Why: a client that tails service events to diagnose a deployment sees no events at all.
Fix: services/.../src/ops/update_service.rs plus the event recording in the service state.
Traces: ecs-service-lifecycle-on-cluster#task#5.

Cause 4 of 8 — a deleted service never settles into its recorded terminal state.
AWS: recording: DescribeServices after DeleteService returns the service with runStatus='TOMBSTONE' and the full member set; the replay's poll expects that steady state.
Emulator: the poll was capped at 60s because the service never reports those members/status.
Why: teardown logic that waits for the service to settle after deletion hangs.
Fix: DeleteService's state transition plus the projection cited in Cause 2.
Traces: ecs-service-lifecycle-on-cluster#task#12.

Cause 5 of 8 — CreateService omits createdBy, roleArn, version and the deployment-time defaults for an EXTERNAL deployment-controller service.
AWS: Smithy: Service.roleArn ("The ARN of the IAM role that's associated with the service"). Recording: roleArn='arn:aws:iam::<account>:role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS', createdBy set, deploymentConfiguration.bakeTimeInMinutes=0, extendDeploymentTimePeriod=0, healthCheckGracePeriodSeconds=0, version=0.
Emulator: all absent.
Why: a client that reads roleArn to confirm the service-linked role was attached cannot.
Fix: as Cause 1.
Traces: ecs-task-set-external-deployment-workflow#setup#4 (+3 members, e.g. ecs-taskset-basic-lifecycle#setup#4).

Cause 6 of 8 — DescribeServices never reaches the recorded post-delete service state in the contains-services fixture.
AWS: recording: the service is returned with status/runStatus settled and the full deployment member set.
Emulator: poll capped at 60s; createdBy, deploymentController, deploymentConfiguration defaults, stabilityStatus, version and runStatus never appear.
Fix: as Causes 2 and 4.
Traces: ecs-err-delete-cluster-contains-services#task#7.

Cause 7 of 8 — after DeleteService, DescribeServices reports a MISSING failure instead of the INACTIVE service.
AWS: Smithy: Service.status documents ACTIVE, DRAINING and INACTIVE. Recording: the deleted service is still described with status INACTIVE.
Emulator: services=[] and failures=[{arn: <service arn>, reason: 'MISSING'}]; the status poll never saw INACTIVE.
Why: a client cannot distinguish a deleted service from one that never existed, and delete-confirmation waits never succeed.
Fix: DeleteService must retain the record with status INACTIVE in services/.../src/state.rs.
Traces: ecs-err-service-deployment-absent-arn#teardown#11 (+1 member, ecs-err-service-not-active#teardown#10).

Cause 8 of 8 — UpdateService on a non-ACTIVE service is accepted instead of refused.
AWS: Smithy: UpdateService errors include ServiceNotActiveException (client, HTTP 400). Recording: status 400, __type 'ServiceNotActiveException', message 'Service was not ACTIVE.'
Emulator: answers 200 and applies the update; no __type, no message.
Why: a client relying on the refusal to detect that it is mutating a draining/deleted service silently corrupts its own state model.
Fix: the service-state precondition in services/.../src/ops/update_service.rs.
Traces: ecs-err-service-not-active#task#8.

## Evidence
Cause 1 (ecs-service-lifecycle-on-cluster#task#3, CreateService, 200/200): $.service.createdBy, $.service.currentServiceRevisions, $.service.deploymentConfiguration.bakeTimeInMinutes, $.service.deploymentConfiguration.deploymentCircuitBreaker, $.service.deploymentConfiguration.strategy, $.service.deploymentController, $.service.deployments[0].failedLaunchTaskCount/healthyTaskCount/replacedTaskCount/scale/stabilityStatus/totalFailedTasks, $.service.extendDeploymentTimePeriod, $.service.healthCheckGracePeriodSeconds, $.service.version all 'missing'; $.service.deployments[0].rolloutState: value_mismatch (expected 'IN_PROGRESS', got 'COMPLETED').
Cause 3 (ecs-service-lifecycle-on-cluster#task#5, UpdateService): additionally $.service.events[0] and $.service.events[1] missing (expected steady-state and deployment-completed events), $.service.runStatus missing (expected 'SteadyState').
Cause 5 (ecs-task-set-external-deployment-workflow#setup#4): $.service.roleArn missing (expected 'arn:aws:iam::000000000000:role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS'), $.service.createdBy/version/healthCheckGracePeriodSeconds/extendDeploymentTimePeriod missing.
Cause 7 (ecs-err-service-deployment-absent-arn#teardown#11, DescribeServices): $: poll capped at 60s (got {'services': [], 'failures': [{'reason': 'MISSING'}]}); $.services[0].status: poll: not in ['INACTIVE'].
Cause 8 (ecs-err-service-not-active#task#8, UpdateService): status expected 400, got 200; $.__type missing (expected 'ServiceNotActiveException'); $.message missing (expected 'Service was not ACTIVE.').

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixture ecs-service-lifecycle-on-cluster steps task#3, task#4, task#5, task#12; ecs-err-service-not-active step task#8; ecs-err-service-deployment-absent-arn step teardown#11.
3. By hand: CreateCluster; RegisterTaskDefinition; CreateService -> the response has no createdBy/deploymentController/deploymentConfiguration defaults and the deployment is already COMPLETED. DeleteService then DescribeServices -> MISSING failure instead of status INACTIVE. UpdateService on that service -> 200 instead of 400 ServiceNotActiveException.
