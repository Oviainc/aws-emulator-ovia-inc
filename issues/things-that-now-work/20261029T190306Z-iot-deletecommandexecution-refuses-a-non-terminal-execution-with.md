Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.command-execution-delete-non-terminal-rejected iot.delete-command-execution-non-terminal-rejected iot.command-execution-state-transitions
Issue-Key: iot.command-execution-delete-non-terminal-rejected|run#16|AssertionError|assert code == ("ValidationException", "Failed to delete command execution as it hasn't become terminal."), f"expected ValidationException, got {code!r}"
Issue-Key: iot.delete-command-execution-non-terminal-rejected|run#16|AssertionError|assert err == (
Issue-Key: iot.command-execution-state-transitions|_error_code#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iot: DeleteCommandExecution refuses a non-terminal execution with ConflictException instead of ValidationException, and deletes a TIMED_OUT execution AWS refuses

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCommandExecution

## Description
Two net-new root causes in the DeleteCommandExecution deletability gate (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_command_execution.rs). Both are about which executions that gate refuses and how it words the refusal; a fixer must close BOTH before submitting.

Cause 1 of 2: a non-terminal (CREATED/IN_PROGRESS) execution is refused with the wrong error class, HTTP status and message.
AWS: Smithy DeleteCommandExecution documents "Only command executions that enter a terminal state can be deleted from your account" and models ConflictException (409), InternalServerException (500), ThrottlingException (400) and ValidationException (400). Recorded behavior: AWS refuses a delete of an execution that has not reached a terminal status with HTTP 400, error code `ValidationException`, message `Failed to delete command execution as it hasn't become terminal.` — that message is entirely FIXED text for this error class (it interpolates no request value: no executionId, no status, no targetArn), and the status code is 400.
Emulator: answers HTTP 409 with code `ConflictException` and the message `Command execution <executionId> is in status <STATUS> and only command executions in a terminal status can be deleted` — i.e. a different error class, a different status code, and a message template whose variable parts (`<executionId>` from the request, `<STATUS>` from the stored record) AWS does not emit at all. The handler's `is_pending` branch (delete_command_execution.rs, the `if is_pending` arm) constructs ConflictException by design, and the module doc asserts 409 ConflictException as the contract.
Why: a client that classifies this refusal — botocore `except iot.exceptions.ValidationException`, or any retry/branch policy keyed on the 400-vs-409 distinction — does not match. 409 is also commonly treated as a transient conflict worth retrying, so a client can retry forever against a condition AWS reports as a permanent request-validation failure, and any client that surfaces or parses the message text sees wording AWS never produces.
Fix: the deletability refusal in the DeleteCommandExecution handler — emit the modelled ValidationException (400) with AWS's fixed message text instead of ConflictException, and leave the execution untouched (the emulator already leaves it intact, which is correct).
Traces: iot.command-execution-delete-non-terminal-rejected, iot.delete-command-execution-non-terminal-rejected.

Cause 2 of 2: a TIMED_OUT execution is accepted and deleted, where AWS refuses it.
AWS: recorded behavior only (the model's prose says only "a terminal state", and the service guide's list of statuses DeleteCommandExecution accepts is SUCCEEDED, FAILED, REJECTED — not TIMED_OUT). Recorded behavior: a delete of an execution that reached TIMED_OUT (the cloud-side timeout, no device response) is refused exactly like cause 1 — HTTP 400, `ValidationException`, `Failed to delete command execution as it hasn't become terminal.` — and the execution remains retrievable by GetCommandExecution with status TIMED_OUT.
Emulator: the handler classifies TIMED_OUT as terminal (module doc: "an execution whose armed timeout is due is TIMED_OUT, hence terminal, hence deletable", and `time_out_due_executions` is run before the status is read), so the delete SUCCEEDS with HTTP 200 and an empty body and the execution is removed from the store.
Why: the emulator accepts a call AWS rejects, so it destroys state AWS keeps. A client (or test) relying on AWS's refusal to keep a timed-out execution readable for diagnosis instead loses the record, and code written against the emulator's acceptance will break with a 400 against real AWS.
Fix: the terminal/deletable status predicate the DeleteCommandExecution handler consults (the `is_pending` projection it reads off the stored record) — TIMED_OUT must fall on the refused side of that gate, refused with the same ValidationException as cause 1, and the record must be left in place. The pre-delete `time_out_due_executions` materialization itself is fine; only its deletability consequence is wrong.
Traces: iot.command-execution-state-transitions.

## Evidence
Cause 1 — check iot.delete-command-execution-non-terminal-rejected and iot.command-execution-delete-non-terminal-rejected:
  expected (AWS): ("ValidationException", "Failed to delete command execution as it hasn't become terminal.") with HTTP 400
  actual (emulator): ('ConflictException', 'Command execution 3d0faa35ffac955ca49fbbf38303e5d8 is in status CREATED and only command executions in a terminal status can be deleted') with HTTP 409
  (the hex id and the CREATED status in the actual message are per-run/request values; the divergence is the error code, the status code, and the fact that AWS's message carries no interpolated values at all)
Source: services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_command_execution.rs lines 211-228 (the `if is_pending` arm builds ConflictException) and lines 10-15 (module doc states 409 ConflictException as the intended contract).

Cause 2 — check iot.command-execution-state-transitions:
  the check polled GetCommandExecution until status == TIMED_OUT (reached), then called DeleteCommandExecution
  expected (AWS): ClientError ValidationException 400 "Failed to delete command execution as it hasn't become terminal.", execution still retrievable at TIMED_OUT
  actual (emulator): AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (empty body, content-length 2) and the execution is removed
Source: same file, lines 184-230 (`time_out_due_executions` then `is_pending`-only refusal, then `remove_execution`).

Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json): DeleteCommandExecution DELETE /command-executions/{executionId}?targetArn=..., success 200, errors ConflictException(409), InternalServerException(500), ThrottlingException(400), ValidationException(400); doc "Only command executions that enter a terminal state can be deleted from your account".

## Steps to Reproduce
Cause 1:
1. iot.create_command(commandId=<unique>, namespace="AWS-IoT", payload={content: b'{"action":"noop"}', contentType: "application/json"}) -> commandArn
2. iot.create_thing(thingName=<unique>) -> thingArn
3. iot.describe_endpoint(endpointType="iot:Data-ATS"), then on that endpoint: iot-jobs-data.start_command_execution(commandArn, targetArn=thingArn, executionTimeoutSeconds=600) -> executionId
4. iot.get_command_execution(executionId, targetArn) -> status CREATED (non-terminal; no device acks it)
5. iot.delete_command_execution(executionId, targetArn)
   -> emulator: 409 ConflictException 'Command execution <id> is in status CREATED and only command executions in a terminal status can be deleted'
   -> AWS: 400 ValidationException "Failed to delete command execution as it hasn't become terminal."

Cause 2:
1-3. as above but with executionTimeoutSeconds=6
4. poll iot.get_command_execution(executionId, targetArn) until status == TIMED_OUT
5. iot.delete_command_execution(executionId, targetArn)
   -> emulator: HTTP 200, empty body, execution removed
   -> AWS: 400 ValidationException "Failed to delete command execution as it hasn't become terminal.", execution still readable at TIMED_OUT
