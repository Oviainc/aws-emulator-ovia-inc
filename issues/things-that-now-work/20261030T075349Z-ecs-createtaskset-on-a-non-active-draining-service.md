Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.create-task-set-on-draining-service-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.create-task-set-on-draining-service-rejected|run#22|AssertionError|assert message == "Service was not ACTIVE.", f"unexpected message: {message!r}"
Issue-Commit: 99a6bf4e8a9dd64f73b61f622317c845b92ac2b7

# ECS CreateTaskSet on a non-ACTIVE (DRAINING) service refuses with emulator-authored wording instead of AWS's 'Service was not ACTIVE.'

> _Note: this issue was identified against the code base at commit `99a6bf4e8a9dd64f73b61f622317c845b92ac2b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTaskSet

## Description
One net-new root cause in the ECS service-state guard for CreateTaskSet. It must be closed before submitting.

Cause 1 of 1: CreateTaskSet against a service whose status is not ACTIVE (behavioral check ecs.create-task-set-on-draining-service-rejected).

AWS: CreateTaskSet on a service that is not ACTIVE is refused with HTTP 400 and error code ServiceNotActiveException, message 'Service was not ACTIVE.' — the message is entirely fixed text with NO variable parts: it does not name the service, does not echo the cluster, and does not disclose which non-ACTIVE state (DRAINING / INACTIVE) the service is in. Smithy (ECS 2014-11-13, com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): CreateTaskSet models ServiceNotActiveException among its errors, errorKind=client, httpError 400 (defaulted); cluster/service/taskDefinition are required members of CreateTaskSetRequest.

Emulator: the refusal carries the correct error code ServiceNotActiveException at HTTP 400, but an emulator-authored message of the form 'service "<serviceName>" is DRAINING and cannot be updated' — i.e. a template that interpolates the service name from the request and the internal lifecycle state, and uses the verb 'cannot be updated' (a service-update phrasing) for a task-set creation call. In the replayed check the observed message was 'service "sim2real-cts-svc-450729f86559" is DRAINING and cannot be updated'; the generated service name in that string is a per-run value, the defect is the template, not the name.

Why: a client or negative test that matches AWS's documented refusal text 'Service was not ACTIVE.' does not recognize the condition, so an EXTERNAL-deployment-controller workflow that is supposed to detect "the service went away under me, stop creating task sets" falls through to its generic error branch and retries or aborts incorrectly. Conversely the emulator leaks internal state (the concrete DRAINING value) that AWS deliberately does not expose for this class, so a client written against the emulator can grow a dependency on text real AWS never emits.

Fix: the service-state guard in CreateTaskSet's handler — it should emit the fixed ServiceNotActiveException message rather than formatting the service name and lifecycle state into a service-update message. The guard appears to be shared with the service-update path, so the fix belongs in whatever helper produces this refusal, with CreateTaskSet's message made the fixed AWS text.

Related open issue, which does NOT carry this check's replay key: '20261029T220901Z-ecs-service-deployment-task-set-and-attribute-error.md' (ECS service-deployment, task-set and attribute error paths...), whose Cause 6 of 10 describes the same guard from the wire corpus (fixture ecs-err-service-not-active, step task#7). This behavioral check is the control-plane-sequence evidence for it: fixing that cause should also make this check pass, and this check should be re-replayed to confirm.

Traces: ecs.create-task-set-on-draining-service-rejected.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1), check ecs.create-task-set-on-draining-service-rejected [mismatch]:

  File "check_ecs_create_task_set_on_draining_service_rejected.py", line 72, in run
    assert message == "Service was not ACTIVE.", f"unexpected message: {message!r}"
  AssertionError: unexpected message: 'service "sim2real-cts-svc-450729f86559" is DRAINING and cannot be updated'

What matched: the positive control (CreateTaskSet succeeds on the ACTIVE EXTERNAL service, DescribeTaskSets reports PRIMARY/ACTIVE), the post-DeleteService status assertion (service reports DRAINING/INACTIVE), the error code assertion (code == 'ServiceNotActiveException'), and the forbidden-effect assertion (no task set was created).

What diverged — the error message only:
  expected (AWS, fixed text, no variable parts): 'Service was not ACTIVE.'
  actual   (emulator template): 'service "<serviceName>" is DRAINING and cannot be updated'
           fixed text in the emulator's string: 'service "', '" is ', ' and cannot be updated'
           values taken from the request/state: <serviceName> from CreateTaskSetRequest.service, 'DRAINING' from the emulator's service record.
HTTP status (400) and error code (ServiceNotActiveException) agree with AWS; the message does not.

Smithy: CreateTaskSet errors include ServiceNotActiveException (errorKind client, httpError 400, defaulted).

## Steps to Reproduce
1. CreateCluster, then RegisterTaskDefinition with networkMode=bridge / requiresCompatibilities=[EC2] and one container definition.
2. CreateService in that cluster with desiredCount=0 and deploymentController={"type":"EXTERNAL"}.
3. Positive control: CreateTaskSet(cluster, service, taskDefinition, launchType=EC2, scale={value:100,unit:PERCENT}) — succeeds; DeleteTaskSet(force=true) and poll DescribeServices until the task set clears from the service's taskSets list.
4. DeleteService(cluster, service) and confirm DescribeServices reports the service as DRAINING (or INACTIVE).
5. Call CreateTaskSet again with the same arguments.
6. Observe: HTTP 400 / ServiceNotActiveException (correct) but Error.Message is 'service "<serviceName>" is DRAINING and cannot be updated' instead of AWS's fixed 'Service was not ACTIVE.'
7. Re-replay behavioral check ecs.create-task-set-on-draining-service-rejected to confirm the fix.
