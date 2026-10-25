Issue-Source: trace-sweep-divergence
Issue-Trace: snow-device-management-create-task-target-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: snow-device-management-create-task-target-not-found snow-device-management-device-describe-and-resources-not-found snow-device-management-list-devices-and-tasks-baseline snow-device-management-task-describe-cancel-not-found
Issue-Shape-Id: com.amazonaws.snowdevicemanagement#SnowDeviceManagement
Issue-Key: snow-device-management|CreateTask|4xx->4xx|$.message
Issue-Key: snow-device-management|CreateTask|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|CreateTask|4xx->4xx|status
Issue-Key: snow-device-management|DescribeDevice|4xx->4xx|$.message
Issue-Key: snow-device-management|DescribeDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|DescribeDevice|4xx->4xx|status
Issue-Key: snow-device-management|DescribeDeviceEc2Instances|4xx->4xx|$.message
Issue-Key: snow-device-management|DescribeDeviceEc2Instances|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|DescribeDeviceEc2Instances|4xx->4xx|status
Issue-Key: snow-device-management|ListDeviceResources|4xx->4xx|$.message
Issue-Key: snow-device-management|ListDeviceResources|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|ListDeviceResources|4xx->4xx|status
Issue-Key: snow-device-management|ListDevices|4xx->2xx|$.message
Issue-Key: snow-device-management|ListDevices|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|ListDevices|4xx->2xx|status
Issue-Key: snow-device-management|ListTasks|4xx->2xx|$.message
Issue-Key: snow-device-management|ListTasks|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|ListTasks|4xx->2xx|status
Issue-Key: snow-device-management|DescribeTask|4xx->4xx|$.message
Issue-Key: snow-device-management|DescribeTask|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|DescribeTask|4xx->4xx|status
Issue-Key: snow-device-management|CancelTask|4xx->4xx|$.message
Issue-Key: snow-device-management|CancelTask|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|CancelTask|4xx->4xx|status
Issue-Key: snow-device-management|ListExecutions|4xx->4xx|$.message
Issue-Key: snow-device-management|ListExecutions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|ListExecutions|4xx->4xx|status
Issue-Key: snow-device-management|DescribeExecution|4xx->4xx|$.message
Issue-Key: snow-device-management|DescribeExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: snow-device-management|DescribeExecution|4xx->4xx|status
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# snow-device-management ignores the account-level no-devices-in-region gate: every operation answers 200 or 404 instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTask, DescribeDevice, DescribeDeviceEc2Instances, ListDeviceResources, ListDevices, ListTasks, DescribeTask, CancelTask, ListExecutions, DescribeExecution

## Description
The wire corpus simparity-wire-ga-prod-v1 records a Snow Device Management account/region that has no Snow device with Snow Device Management activated. In that state EVERY operation of the service is REFUSED by a service-level precondition gate rather than answered: the recorded response is HTTP 403 with x-amzn-errortype AccessDeniedException (Smithy: com.amazonaws.snowdevicemanagement#AccessDeniedException, errorKind client, @httpError 403, declared on CreateTask, DescribeDevice, DescribeDeviceEc2Instances, DescribeExecution, DescribeTask, ListDeviceResources, ListDevices, ListExecutions, ListTasks, CancelTask) and the recorded message is "No devices found in this region." (recorded behavior only for the text; the model supplies the error shape and its 403 status). The gate is evaluated BEFORE any per-resource id resolution and before the operation produces a result, so a caller on a gated account never learns whether a particular device/task id exists and never receives a success payload.

The emulator has no such gate. Instead each handler resolves the request against its own (now empty) regional store and answers either the per-resource lookup failure — 404 ResourceNotFoundException (Smithy: @httpError 404) — or, for the collection operations, a plain HTTP 200 with an empty list. 10 of the 13 replayed steps diverge this way; the two behaviors below are the same single missing precondition seen from the two kinds of handler, and all 10 causes must be closed before this issue is submitted.

=== Cause 1 of 10: CreateTask ===
AWS: on a gated account CreateTask is refused by the account gate ahead of target resolution — 403, x-amzn-errortype AccessDeniedException, message e.g. "No devices found in this region.". Smithy: CreateTask declares AccessDeniedException (403) and ResourceNotFoundException (404); the gate outranks the target existence check.
Emulator: 404 with x-amzn-errortype ResourceNotFoundException and a body reporting the unresolved target, e.g. "managed-device <id> not found".
Why: a client distinguishing "this account/region is not enabled for Snow Device Management" (403, not retryable by re-pointing at another id) from "that device id does not exist" (404) takes the wrong branch, and the inverted ordering leaks per-id existence that AWS does not disclose on a gated account.
Fix: the CreateTask handler — consult the shared account/region precondition in the service state module before resolving targets.
Traces: snow-device-management-create-task-target-not-found#task#1.

=== Cause 2 of 10: DescribeDevice ===
AWS: 403 AccessDeniedException from the account gate, ahead of managedDeviceId resolution (Smithy: DescribeDevice errors AccessDeniedException/403, ResourceNotFoundException/404).
Emulator: 404 ResourceNotFoundException, message of the form "managed-device <id> not found".
Why: same wrong error class and status (404 vs 403) for an account-level condition.
Fix: the DescribeDevice handler, before id resolution.
Traces: snow-device-management-device-describe-and-resources-not-found#task#1.

=== Cause 3 of 10: DescribeDeviceEc2Instances ===
AWS: 403 AccessDeniedException from the account gate (Smithy: errors AccessDeniedException/403, ResourceNotFoundException/404).
Emulator: 404 ResourceNotFoundException ("managed-device <id> not found").
Why: as above; EC2-resource reads on a gated account are refused, not reported as a missing device.
Fix: the DescribeDeviceEc2Instances handler, before id resolution.
Traces: snow-device-management-device-describe-and-resources-not-found#task#2.

=== Cause 4 of 10: ListDeviceResources ===
AWS: 403 AccessDeniedException from the account gate (Smithy: errors AccessDeniedException/403, ResourceNotFoundException/404; GET /managed-device/{managedDeviceId}/resources).
Emulator: 404 ResourceNotFoundException ("managed-device <id> not found"). Affects both replayed steps of this cluster, including the paginated/filtered form.
Fix: the ListDeviceResources handler, before id resolution.
Traces: snow-device-management-device-describe-and-resources-not-found#task#3 (and #task#4, same cluster).

=== Cause 5 of 10: ListDevices ===
AWS: the collection read is refused, not answered — 403, x-amzn-errortype AccessDeniedException (Smithy: ListDevices errors are AccessDeniedException/403 and ThrottlingException only, so a gated account has no success representation here).
Emulator: HTTP 200 with a response body carrying no error at all (the handler enumerates its empty regional store and returns a devices list).
Why: the client's negative path is never reached; code that expects to be told the account is not enabled instead reads "this account genuinely owns zero devices", which is a different fact and suppresses the enablement error entirely.
Fix: the ListDevices handler plus the service state module's account/region precondition — a gated account must answer the gate rather than an empty page.
Traces: snow-device-management-list-devices-and-tasks-baseline#task#1.

=== Cause 6 of 10: ListTasks ===
AWS: 403 AccessDeniedException (Smithy: ListTasks errors AccessDeniedException/403, ThrottlingException).
Emulator: HTTP 200, no error, empty task page.
Why: same as Cause 5 — an enablement refusal is reported as an empty inventory.
Fix: the ListTasks handler plus the shared precondition in the state module.
Traces: snow-device-management-list-devices-and-tasks-baseline#task#2.

=== Cause 7 of 10: DescribeTask ===
AWS: 403 AccessDeniedException from the account gate ahead of taskId resolution (Smithy: errors AccessDeniedException/403, ResourceNotFoundException/404).
Emulator: 404 ResourceNotFoundException, message of the form "task <id> not found".
Fix: the DescribeTask handler, before id resolution.
Traces: snow-device-management-task-describe-cancel-not-found#task#1.

=== Cause 8 of 10: CancelTask ===
AWS: 403 AccessDeniedException from the account gate (Smithy: errors AccessDeniedException/403, ResourceNotFoundException/404; POST /task/{taskId}/cancel).
Emulator: 404 ResourceNotFoundException ("task <id> not found").
Why: a cancel on a gated account must not be reported as an unknown task; retry/idempotency logic keyed on 404 loops on a condition that will never resolve.
Fix: the CancelTask handler, before id resolution.
Traces: snow-device-management-task-describe-cancel-not-found#task#2.

=== Cause 9 of 10: ListExecutions ===
AWS: 403 AccessDeniedException from the account gate (Smithy: errors AccessDeniedException/403, ResourceNotFoundException/404; GET /executions).
Emulator: 404 ResourceNotFoundException referencing the unresolved task ("task <id> not found").
Fix: the ListExecutions handler, before the taskId filter is resolved.
Traces: snow-device-management-task-describe-cancel-not-found#task#3.

=== Cause 10 of 10: DescribeExecution ===
AWS: 403 AccessDeniedException from the account gate (Smithy: errors AccessDeniedException/403, ResourceNotFoundException/404; POST /task/{taskId}/execution/{managedDeviceId}).
Emulator: 404 ResourceNotFoundException ("task <id> not found").
Fix: the DescribeExecution handler, before id resolution.
Traces: snow-device-management-task-describe-cancel-not-found#task#4.

Related already-filed work, none of which owns these replay keys (every cluster reported owned_by: [] and stub_path: ""): the resolved behavioral issue 20261022T034615Z-snow-device-management-the-account-level-no-devices described this same gate as absent, and the resolved issues 20261024T151437Z-snow-device-management-describedevice-error-response and 20261024T151437Z-snow-device-management-describedeviceec2instances-error-response asked for the OPPOSITE of the recording (ResourceNotFoundException in place of the 403), which is the behavior the replay now observes; the still-open 20261024T151437Z-snow-device-management-listdevices-wrong-behavior and 20261024T151437Z-snow-device-management-listtasks-wrong-behavior likewise ask for an empty list where the GA-prod recording shows the 403 gate. The recorded wire corpus is the authority here: the gate must be restored ahead of per-resource resolution for all ten operations above.

## Evidence
Replay of simparity-wire-ga-prod-v1 against the emulator (emulator_up: true, bootstrap_signal: false): 13 steps total, 2 passed, 11 mismatched, score 15.4%, 10 divergence clusters — all 10 in service snow-device-management.

Every cluster reports the same expected response: status 403, $headers.x-amzn-errortype = 'AccessDeniedException', $.message = 'No devices found in this region.'

Actual, id-addressed operations (status: expected 403, got 404; x-amzn-errortype value_mismatch -> 'ResourceNotFoundException'):
- CreateTask — snow-device-management-create-task-target-not-found#task#1, body message 'managed-device sim2real-probe-snow-device-management-mdevice-ddb113a1 not found'
- DescribeDevice — snow-device-management-device-describe-and-resources-not-found#task#1, 'managed-device sim2real-probe-snow-device-management-mdevice-735451fc not found'
- DescribeDeviceEc2Instances — ...-device-describe-and-resources-not-found#task#2, same message form
- ListDeviceResources — ...-device-describe-and-resources-not-found#task#3 (+#task#4 in the same cluster), same message form
- DescribeTask — snow-device-management-task-describe-cancel-not-found#task#1, 'task sim2real-probe-snow-device-management-task-67448098 not found'
- CancelTask — ...-task-describe-cancel-not-found#task#2, same message form
- ListExecutions — ...-task-describe-cancel-not-found#task#3, same message form
- DescribeExecution — ...-task-describe-cancel-not-found#task#4, same message form
(The quoted messages are the recording's/emulator's examples; the requirements asserted here are the 403 status and the AccessDeniedException error code.)

Actual, collection operations (status: expected 403, got 200; $.message missing, got None; $headers.x-amzn-errortype missing, got None):
- ListDevices — snow-device-management-list-devices-and-tasks-baseline#task#1
- ListTasks — snow-device-management-list-devices-and-tasks-baseline#task#2

Model facts used (smithy_query digest + shape on services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/smithy-model/com-amazonaws-snowdevicemanagement-snowdevicemanagement.json): protocol restJson1; AccessDeniedException errorKind client @httpError 403, declared on CreateTask, DescribeDevice, DescribeDeviceEc2Instances, DescribeExecution, DescribeTask, ListDeviceResources, ListDevices, ListExecutions, ListTasks, CancelTask; ResourceNotFoundException @httpError 404.

Code: services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/ops/list_devices.rs:152-221 — the handler goes straight from state lookup to token decoding and page assembly and returns ListDevicesOutput with no account/region precondition consulted anywhere; the id-addressed handlers under the same src/ops/ directory return their ResourceNotFoundException from the id lookup with no preceding gate.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-ga-prod-v1 wire corpus (replay_wire_corpus) against it.
2. Fixture snow-device-management-list-devices-and-tasks-baseline, step task#1: GET /managed-devices on the recorded account/region. Recorded: 403 with x-amzn-errortype AccessDeniedException and message 'No devices found in this region.'. Observed: 200 with an empty devices page and no errortype header. Step task#2 does the same for GET /tasks (ListTasks).
3. Fixture snow-device-management-create-task-target-not-found, step task#1: POST /task naming a managed device the account does not own. Recorded: 403 AccessDeniedException. Observed: 404 ResourceNotFoundException 'managed-device ... not found'.
4. Fixture snow-device-management-device-describe-and-resources-not-found, steps task#1..task#4: POST /managed-device/{id}/describe, POST /managed-device/{id}/resources/ec2/describe, GET /managed-device/{id}/resources. Recorded: 403 AccessDeniedException on each. Observed: 404 ResourceNotFoundException on each.
5. Fixture snow-device-management-task-describe-cancel-not-found, steps task#1..task#4: POST /task/{taskId}, POST /task/{taskId}/cancel, GET /executions, POST /task/{taskId}/execution/{managedDeviceId}. Recorded: 403 AccessDeniedException on each. Observed: 404 ResourceNotFoundException 'task ... not found' on each.
6. Re-replay all five fixtures after the fix; all 13 steps must pass, including the two that already pass.
