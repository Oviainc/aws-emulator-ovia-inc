Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.create-task-set-scale-out-of-range-rejected ecs.escc-ec2-service-keeps-desired-count-without-capacity
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.create-task-set-scale-out-of-range-rejected|run#9|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: ecs.escc-ec2-service-keeps-desired-count-without-capacity|_poll#7|Timeout|time.sleep(interval_s)
Issue-Commit: ef659389d11311dc27a7ee58cb593d22d3f663cf

# ecs: 2 net-new behavioral root causes — CreateTaskSet refuses every call (no Scale range validation) and the service scheduler never records a placement-failure event

> _Note: this issue was identified against the code base at commit `ef659389d11311dc27a7ee58cb593d22d3f663cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTaskSet

## Description
Two net-new behavioral root causes in the ecs emulator, surviving after the suite's ALREADY FILED / OWNED markers were honoured. A fixer must close BOTH causes below before submitting.

Cause 1 of 2: CreateTaskSet answers NotImplemented for every request, so the documented Scale.value range check (0-100) never happens and no task set is ever created.
AWS: Smithy CreateTaskSet (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113) models input member `scale` -> `Scale`, whose `value` member documents "The value, specified as a percent total of a service's desiredCount, to scale the task set. Accepted values are numbers between 0 and 100." CreateTaskSet models InvalidParameterException (client error, HTTP 400) among its errors; AWS rejects scale={"value":150,"unit":"PERCENT"} with error code InvalidParameterException and creates no task set (the recorded message, cited as an EXAMPLE of the rule "percent scale may not exceed 100", is 'TaskSet Scale value for percent can not be greater than 100.0'). An in-range value (50/PERCENT) is accepted and reads back on the created task set via DescribeTaskSets.
Emulator: the call is refused with error code `NotImplemented` ('operation CreateTaskSet is scaffolded but not implemented in this emulator') — both for the out-of-range request and for the in-range positive control, so neither the rejection nor the creation path exists. The open stub issue 20261024T081305Z-ecs-createtaskset-stub.md records the bare unimplemented operation; it does NOT state this validation requirement, so the range check and the error classification recorded here must be satisfied too, not just a 200.
Why: a client driving an EXTERNAL / blue-green deployment cannot create a task set at all, and once the operation is stood up a client that relies on AWS rejecting an out-of-range scale would instead get a task set created with a >100% scale and no error to branch on — it would also lose the InvalidParameterException it catches to report a bad input.
Fix: the CreateTaskSet handler and its input validation (Scale.value range) plus the task-set state/serializer in the ecs service crate (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_task_set.rs), including the DescribeTaskSets read-back of scale.value/scale.unit.
Traces: ecs.create-task-set-scale-out-of-range-rejected.

Cause 2 of 2: an EC2-launch-type service that cannot be placed never gets a scheduler event, so Service.events stays without the placement-failure record.
AWS: Smithy Service.events -> ServiceEvents/ServiceEvent {id, createdAt, message} — "The event stream for your service. A maximum of 100 of the latest events are displayed." Recorded behavior: a REPLICA service created with launchType EC2 and desiredCount 2 on a cluster with no registered container instances stays status ACTIVE with desiredCount 2 / runningCount 0, and the scheduler durably appends an event whose message says it was "unable to place a task" and names the missing capacity ("no container instance" met the requirements). The failed placement never lowers desiredCount.
Emulator: no such event ever appears. The check polls DescribeServices for an event message containing 'unable to place a task' and the condition is never satisfied — the check is killed by the harness at 60s with 'Check timed out after 60.0s' having only ever seen the emulator's own steady-state events. (The emulator does emit steady-state service events, per open issue 20261023T153036Z-ecs-service-read-and-write-responses-omit-recorded.md, which concerns OMITTED Service/Deployment members and event-id shape, not the missing scheduler-failure event; this cause is not covered there.)
Why: a client or operator that diagnoses a stuck deployment by reading Service.events — the documented place ECS explains why runningCount < desiredCount — gets no explanation, so "service will not start" is indistinguishable from "service is still starting", and any wait loop keyed on the placement-failure event never terminates.
Fix: the ECS service scheduler's placement path and the service-event stream it writes (the state behind DescribeServices' Service.events projection) in the ecs service crate.
Traces: ecs.escc-ec2-service-keeps-desired-count-without-capacity.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from mainline (75/356 checks passed).

Cause 1 — check ecs.create-task-set-scale-out-of-range-rejected [mismatch]:
  AssertionError: expected InvalidParameterException for scale.value=150, got 'NotImplemented'
  (raised at check_ecs_create_task_set_scale_out_of_range_rejected.py line 69). The emulator's error body is 'operation CreateTaskSet is scaffolded but not implemented in this emulator'. Expected: error code InvalidParameterException, HTTP 400, no task set created; then the in-range control scale={'value':50,'unit':'PERCENT'} accepted with scale.value 50 / scale.unit PERCENT read back by DescribeTaskSets.

Cause 2 — check ecs.escc-ec2-service-keeps-desired-count-without-capacity [mismatch]:
  Error: Check timed out after 60.0s. The check creates a cluster, registers a bridge/EC2 task definition, creates an EC2-launch-type service with desiredCount 2 on a cluster with zero container instances, then polls DescribeServices every 10s for an event message containing 'unable to place a task' (and, once found, requires 'no container instance' in the event text, and desiredCount/runningCount still (2,0)). The predicate is never satisfied against the emulator, so the poll runs until the harness kills the check. Expected: the scheduler's placement-failure event is present in Service.events shortly after creation and persists.

## Steps to Reproduce
1. Build the emulator from mainline and boot it (the behavioral suite does this).
2. Replay check ecs.create-task-set-scale-out-of-range-rejected: CreateCluster -> RegisterTaskDefinition (EC2/bridge) -> CreateService with deploymentController {type: EXTERNAL} and desiredCount 0 -> CreateTaskSet with scale {value: 150, unit: PERCENT}. Observe error code NotImplemented instead of InvalidParameterException (HTTP 400); then CreateTaskSet with scale {value: 50, unit: PERCENT} and observe it is refused too instead of creating a task set whose scale reads back as 50/PERCENT via DescribeTaskSets.
3. Replay check ecs.escc-ec2-service-keeps-desired-count-without-capacity: CreateCluster (no container instances registered) -> RegisterTaskDefinition (networkMode bridge, requiresCompatibilities [EC2]) -> CreateService launchType EC2, desiredCount 2 -> poll DescribeServices. Observe that no event with message 'unable to place a task ... no container instance' is ever appended to services[0].events, so the check times out.
4. Both checks must pass before this issue is closed.
