Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-container-instance-and-attribute-errors
Issue-Step: task#11
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-container-instance-and-attribute-errors ecs-container-instance-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DeregisterContainerInstance|4xx->4xx|$.message
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECS DeregisterContainerInstance error message does not name the referenced container instance

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterContainerInstance

## Description
Cause 1 of 1: the InvalidParameterException raised for an unknown container instance carries a generic message instead of one built from the request.
AWS: Smithy DeregisterContainerInstance declares containerInstance as a required input member and InvalidParameterException (httpError 400) among its errors. The recorded 400 response's message is built from the request — it quotes the containerInstance identifier that was passed: 'Referenced container instance <containerInstance> not found.' The requirement is that rule (the message identifies the member value from the request), not the recorded text; the status 400 and the error type are unchanged by this issue.
Emulator: returns 400 with the fixed string 'The referenced container instance was not found', which never names the identifier.
Why: a client deregistering instances in a batch (or logging the failure for an operator) cannot tell WHICH identifier was rejected, so it cannot retry or report the specific instance; request-derived identifiers in ECS validation messages are the only place that information appears.
Fix: the DeregisterContainerInstance validation/error construction in the ecs service crate — interpolate the requested containerInstance into the InvalidParameterException message.
Traces: ecs-container-instance-and-attribute-errors#task#11, ecs-container-instance-lifecycle#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.
divergence_key service=ecs|DeregisterContainerInstance|status=400->400|keys=$.message; expected_status 400, actual_status 400 (status and __type agree).
detail: "$.message: value_mismatch (expected 'Referenced container instance aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee not found.', got 'The referenced container instance was not found')" — the expected message contains the exact containerInstance value the request sent, the actual contains no identifier.
affected_count 2: ecs-container-instance-and-attribute-errors#task#11, ecs-container-instance-lifecycle#task#7.
state_reset_failures = 0 for this run.

## Steps to Reproduce
1. Build the emulator from mainline and boot it.
2. Replay fixture ecs-container-instance-and-attribute-errors and compare step task#11: DeregisterContainerInstance for a container instance id that was never registered.
3. The 400 InvalidParameterException message must name the containerInstance value taken from the request (also exercised by ecs-container-instance-lifecycle#task#7).
