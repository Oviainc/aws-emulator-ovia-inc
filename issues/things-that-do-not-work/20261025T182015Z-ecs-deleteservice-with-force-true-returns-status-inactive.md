Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.op-contract-deleteService
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.op-contract-deleteService|run#19|AssertionError|assert resp["service"]["status"] == "DRAINING", f"expected DRAINING immediately after forced delete, got {resp['service']['status']}"
Issue-Commit: 8872a34b239da6f35a39fa56ec21084188bccf39

# ECS DeleteService with force=true returns status INACTIVE immediately instead of DRAINING (1 cause)

> _Note: this issue was identified against the code base at commit `8872a34b239da6f35a39fa56ec21084188bccf39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteService

## Description
One net-new behavioral root cause on the ECS DeleteService response projection.

Related open issue: 20261023T153036Z-ecs-deleteservice-returns-status-inactive-instead-of-draining.md describes the same INACTIVE-vs-DRAINING divergence, but it is a WIRE issue whose Issue-Key/Issue-Trace-Ids set is bound to recorded fixture steps (ecs-service-lifecycle-on-cluster#task#11 etc.) and does NOT carry this behavioral check's replay key — the suite therefore did not mark this check as owned by it. This issue files the behavioral grounding so the fix is gated on the forced-delete path exercised by ecs.op-contract-deleteService; a fixer closing that issue should re-replay this check too.

Cause 1 of 1: DeleteService(force=true) on a service that is still scaled above zero reports the service as already INACTIVE, skipping the DRAINING state entirely.

AWS:      Smithy DeleteService -> DeleteServiceResponse{service: Service}. Service.status is documented as "The status of the service. The valid values are ACTIVE, DRAINING, or INACTIVE." The DeleteService operation documentation states the rule normatively: "When you delete a service, if there are still running tasks that require cleanup, the service status moves from ACTIVE to DRAINING ... After all tasks have transitioned to either STOPPING or STOPPED status, the service status moves from DRAINING to INACTIVE." So a forced delete of a service whose desiredCount is still above 0 (tasks requiring cleanup) MUST return a 200 whose $.service.status is DRAINING; INACTIVE is only reachable after the tasks have drained, observed via a subsequent DescribeServices. The preceding behavior in the same check is already correct in the emulator and is not part of this cause: an unforced DeleteService on a service scaled above 0 is refused with InvalidParameterException and the fixed message 'The service cannot be stopped while it is scaled above 0.', and DescribeServices still reports status ACTIVE with desiredCount unchanged.

Emulator: the forced DeleteService returns 200 with $.service.status = 'INACTIVE'. The only variable parts here are the generated cluster/service/task-definition names the check mints; the requirement is the status token, which must be the fixed value 'DRAINING' on this response and must only become 'INACTIVE' after the drain completes.

Why:      a client that deletes a service and then polls DescribeServices for the DRAINING -> INACTIVE transition to learn when cleanup finished sees the terminal state in the delete response itself. It therefore proceeds immediately to tear down everything the service still depends on — the task definition revision, the load balancer / target group, the cluster — while the emulator's own model still has tasks attached to the service. Deployment tooling (CodeDeploy/EXTERNAL rollouts, CDK/CloudFormation stack deletes) uses exactly this transition as its ordering barrier, so collapsing it turns an ordered teardown into a race. It also makes the opposite bug untestable: a client can no longer distinguish "drain finished" from "drain never started".

Fix:      the DeleteService handler's state transition and the Service projection it serializes — on a forced delete with remaining work the service must be moved to DRAINING and serialized as DRAINING, with the move to INACTIVE happening in the service-lifecycle/scheduler state once the tasks reach STOPPING/STOPPED, so a later DescribeServices observes INACTIVE.

Traces:   behavioral check ecs.op-contract-deleteService. Failure detail: AssertionError: expected DRAINING immediately after forced delete, got INACTIVE (check_ecs_op_contract_deleteService.py line 61, asserting resp["service"]["status"]).

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from mainline.

Failing check: [mismatch] ecs.op-contract-deleteService
Detail (tail): File "check_ecs_op_contract_deleteService.py", line 61, in run
    assert resp["service"]["status"] == "DRAINING", f"expected DRAINING immediately after forced delete, got {resp['service']['status']}"
AssertionError: expected DRAINING immediately after forced delete, got INACTIVE

Expected: DeleteService(cluster=<cluster>, service=<service>, force=True) -> 200 with $.service.serviceName == the requested service name and $.service.status == "DRAINING".
Actual:   200 with the correct $.service.serviceName but $.service.status == "INACTIVE".

The check reached line 61, so the assertions before it passed against the emulator and are NOT part of this finding: the unforced DeleteService was correctly refused with error code InvalidParameterException and the exact message 'The service cannot be stopped while it is scaled above 0.' (fixed text, no variable parts), and the post-refusal DescribeServices correctly reported status ACTIVE with desiredCount 1.

Model citation: Smithy ecs#DeleteService output DeleteServiceResponse.service -> Service; Service.status doc 'The valid values are ACTIVE, DRAINING, or INACTIVE'; DeleteService doc 'the service status moves from ACTIVE to DRAINING ... then from DRAINING to INACTIVE'.

## Steps to Reproduce
1. Build the emulator from mainline and boot it.
2. CreateCluster with a fresh cluster name.
3. RegisterTaskDefinition with a single essential container (cpu 128, memory 128).
4. CreateService in that cluster with desiredCount=1 and launchType=EC2.
5. Call DeleteService WITHOUT force and confirm it is refused with InvalidParameterException and the message 'The service cannot be stopped while it is scaled above 0.' (this part already passes).
6. Call DeleteService with force=true and read $.service.status of the 200 response.
   Expected: "DRAINING". Observed: "INACTIVE".
7. Then poll DescribeServices until the service reports "INACTIVE" — that terminal value must only appear here, after the drain, not in the step-6 response.
8. Re-replay behavioral check ecs.op-contract-deleteService to confirm the fix.
