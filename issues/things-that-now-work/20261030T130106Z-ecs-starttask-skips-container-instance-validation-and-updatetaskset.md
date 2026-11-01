Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-start-task-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-start-task-error-paths ecs-taskset-crud-lifecycle ecs-taskset-error-paths ecs-taskset-errors
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|StartTask|4xx->2xx|$.__type
Issue-Key: ecs|StartTask|4xx->2xx|$.message
Issue-Key: ecs|StartTask|4xx->2xx|status
Issue-Key: ecs|UpdateTaskSet|4xx->4xx|$.__type
Issue-Key: ecs|UpdateTaskSet|4xx->4xx|$.message
Issue-Commit: 232d6cb39af83c8f3e5d0960e0729eaeb270e0f7

# ECS StartTask skips container-instance validation and UpdateTaskSet misclassifies draining and absent task sets

> _Note: this issue was identified against the code base at commit `232d6cb39af83c8f3e5d0960e0729eaeb270e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTaskSet

## Description
Three task/task-set error paths diverge: one invalid request is accepted with 200, and two refusals carry the wrong error class or emulator-authored wording. This bundle contains 3 causes; all three must be closed.

Cause 1 of 3: StartTask accepts a malformed container-instance identifier with 200 instead of refusing it.
AWS: StartTask.containerInstances is a required StringList of container-instance ids or ARNs, and StartTask models InvalidParameterException (Smithy: StartTask input member containerInstances required; errors include InvalidParameterException, ClientException, ClusterNotFoundException, httpError 400). A container-instance id is a UUID, so AWS rejects an entry whose id part is neither 32 nor 36 characters before any placement happens: HTTP 400, `__type` InvalidParameterException, message `instanceId length should be one of [32,36]`. That whole message is fixed text — the two lengths are the modeled id forms (UUID without and with hyphens), not values from the request.
Emulator: HTTP 200 with a normal StartTask response body; no `__type`, no `message`, no failure entry. The validation is absent entirely.
Why: a client that passes a truncated or otherwise malformed container-instance id (a common outcome of string-slicing an ARN) is told the task started. It then polls DescribeTasks for a task that was never placed on any instance, and its error-handling branch for InvalidParameterException is dead code — the bug surfaces much later, as a hang, instead of at the call.
Fix: StartTask's input validation, ahead of placement — validate each containerInstances entry's id length and raise InvalidParameterException.
Traces: ecs-start-task-error-paths#task#5.

Cause 2 of 3: UpdateTaskSet on an existing but DRAINING task set answers TaskSetNotFoundException instead of InvalidParameterException.
AWS: UpdateTaskSet models both InvalidParameterException and TaskSetNotFoundException (Smithy: UpdateTaskSet errors include ClientException, InvalidParameterException, ServiceNotActiveException, ServiceNotFoundException, TaskSetNotFoundException, all httpError 400). A task set that is present in state but has moved to DRAINING is not absent — AWS refuses the scale update with HTTP 400, `__type` InvalidParameterException, message `Unable to update taskset with status = DRAINING`. Fixed text: `Unable to update taskset with status = `; variable part: the task set's current status read from state (DRAINING here).
Emulator: HTTP 400 with `__type` TaskSetNotFoundException and message `Unable to find task set with id <taskSetId> on service <serviceName>.` — it treats a draining task set as one that does not exist.
Why: the two error classes mean opposite things to a caller. TaskSetNotFoundException is terminal (stop, the resource is gone); InvalidParameterException on a DRAINING task set is transient (wait for the drain, or stop scaling it). A deployment controller that retires task sets will delete its own bookkeeping for a task set that is still draining, and will never retry.
Fix: the UpdateTaskSet handler's lookup/branch order — resolve the task set irrespective of its status, then branch on status before concluding not-found.
Traces: ecs-taskset-crud-lifecycle#task#11.

Cause 3 of 3: UpdateTaskSet's genuine not-found refusal uses emulator wording and the wrong error class.
AWS: for a task-set id that is absent from the named service, AWS answers HTTP 400 with message `TaskSet: <taskSetId> does not exist in Service: <serviceName>.` Fixed text: `TaskSet: `, ` does not exist in Service: ` and the trailing `.`; variable parts: the task-set id from the request's `taskSet` member and the service name from the request's `service` member.
Emulator: HTTP 400 with message `Unable to find task set with id <taskSetId> on service <serviceName>.` — same ids, entirely emulator-authored sentence.
Why: the message is part of the contract for this error class. Clients, test suites and log-scrapers that match AWS's phrasing (or surface it to an operator) see text no AWS region emits, and a human reading an emulator log cannot match it against AWS documentation.
Fix: the UpdateTaskSet handler's error construction — use AWS's message template for the not-found case.
Traces: ecs-taskset-error-paths#task#8 (also ecs-taskset-errors#task#9).

Related but NOT covering these: 20261029T220901Z-ecs-service-deployment-task-set-and-attribute-error.md owns other task-set/service-deployment error clusters, 20261030T075349Z-ecs-createtaskset-on-a-non-active-draining-service.md owns CreateTaskSet against a DRAINING SERVICE (a different operation and a different resource's status), and 20261029T175650Z-ecs-7-net-new-root-causes-in-request.md owns other over-permissive validations. None of them carries the replay key for these three steps.

## Evidence
Cause 1 — fixture ecs-start-task-error-paths step task#5, operation StartTask (divergence_key service=ecs|StartTask|status=400->200|keys=$.__type,$.message,status):
  status: expected 400, got 200
  $.__type: expected 'InvalidParameterException', got None
  $.message: expected 'instanceId length should be one of [32,36]', got None

Cause 2 — fixture ecs-taskset-crud-lifecycle step task#11, operation UpdateTaskSet (divergence_key service=ecs|UpdateTaskSet|status=400->400|keys=$.__type,$.message):
  $.__type: expected 'InvalidParameterException', got 'TaskSetNotFoundException'
  $.message: expected 'Unable to update taskset with status = DRAINING', got 'Unable to find task set with id ecs-svc/0000000000000000001 on service <service>.'

Cause 3 — fixture ecs-taskset-error-paths step task#8, operation UpdateTaskSet (divergence_key service=ecs|UpdateTaskSet|status=400->400|keys=$.message), 2 affected steps:
  $.message: expected 'TaskSet: ecs-svc/9999999999999999999 does not exist in Service: <service>.', got 'Unable to find task set with id ecs-svc/9999999999999999999 on service <service>.'

The task-set ids and service names shown are per-run fixture values filling the template; the fixed sentence structure and the error class/status are the requirement.

## Steps to Reproduce
1. Cause 1: replay fixture ecs-start-task-error-paths to step task#5 — StartTask with a containerInstances entry whose id part is neither 32 nor 36 characters long. The emulator answers 200; AWS answers 400 InvalidParameterException 'instanceId length should be one of [32,36]'.
2. Cause 2: replay fixture ecs-taskset-crud-lifecycle to step task#11 — the task set created earlier in the fixture has been moved to DRAINING; call UpdateTaskSet with a new scale on it. The emulator answers 400 TaskSetNotFoundException; AWS answers 400 InvalidParameterException 'Unable to update taskset with status = DRAINING'.
3. Cause 3: replay fixture ecs-taskset-error-paths to step task#8 — UpdateTaskSet naming a task-set id that was never created on that service. Compare the message against 'TaskSet: <id> does not exist in Service: <service>.'
4. All three steps pass only when the status, __type and message match the recorded AWS responses.
