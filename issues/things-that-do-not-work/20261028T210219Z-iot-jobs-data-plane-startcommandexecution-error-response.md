Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# A commandArn naming no existing command is answered 400 ValidationException instead of the modelled ResourceNotFoundException (404)

_Filed: 2026-10-28T21:02:19Z_
_Operation: StartCommandExecution | Class: error-response_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCommandExecution` resolves `commandArn` against the shared command index and, when the index holds no such command, refuses the call with the modelled `ValidationException` (HTTP 400) — using a message the crate itself words as a NOT-FOUND ("The command {command_name} was not found."). The operation models a dedicated error for exactly that condition, `ResourceNotFoundException` (HTTP 404), whose model documentation is "The specified resource does not exist."; that variant is declared in the handler's error enum and is never constructed anywhere in the crate, while `ValidationException`, documented as "A validation error occurred when performing the API request.", is used for a request whose values are all well formed and whose only fault is that the referenced resource does not exist.

The consequence a client sees is that a missing command is a permanent, caller-side 400 rather than the 404 the operation models for a non-existent resource, so a client branching on the modelled error set can never observe `ResourceNotFoundException` from this operation.

## Evidence

Model — the operation's own error set declares a not-found error with its trigger (smithy_query query="shape" shape="ResourceNotFoundException"): `"httpError": 404` with doc `"<p>The specified resource does not exist.</p>"`; the error actually returned is `ValidationException`, `"httpError": 400`, doc `"<p>A validation error occurred when performing the API request.</p>"`. Both are bound to this operation — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/OPERATIONS.md:45:
```
- **Errors:** `ConflictException (HTTP 409)`, `InternalServerException (HTTP 500)`, `ResourceNotFoundException (HTTP 404)`, `ServiceQuotaExceededException (HTTP 402)`, `ThrottlingException (HTTP 429)`, `ValidationException (HTTP 400)`
```

The handler names the condition "not found" and answers with the validation error — src/ops/start_command_execution.rs:254-259:
```
/// The 400 message the service builds when `commandArn` names a command that does
/// not exist. ...
fn command_not_found_message(command_name: &str) -> String {
    format!("The command {command_name} was not found.")
}
```
returned through src/ops/start_command_execution.rs:186-191:
```
fn invalid_request(message: String) -> StartCommandExecutionError {
    StartCommandExecutionErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
    .into()
}
```
at src/ops/start_command_execution.rs:457-460:
```
        let command = commands.lock().facts(&input.command_arn);
        let Some(command) = command else {
            return Err(invalid_request(command_not_found_message(command_name)));
        };
```
The modelled 404 variant is declared and never constructed — `grep ResourceNotFoundException src/ops/start_command_execution.rs` returns only line 100 (the doc header) and line 371 (`ResourceNotFoundException(ResourceNotFoundException),` in `StartCommandExecutionErrorModeled`).

Test coverage: tests/all/command_execution_validation.rs:144-253 (`an_execution_starts_only_for_a_command_that_exists`) pins the current code — "//! 1. an execution starts only for a command that EXISTS — an unregistered `commandArn` is refused 400 `ValidationException` naming the command, and nothing is persisted;" (tests/all/command_execution_validation.rs:7-9) and asserts `error_code(&body) == "ValidationException"` (line 245) with message `format!("The command {command_name} was not found.")` (line 251). The test's stated rule is only that such a start must be REFUSED and persist nothing; it gives no reason for choosing the validation error over the operation's modelled not-found error.

## Steps to Reproduce

1. Register one command in the shared index (`command_registry(...).register("arn:aws:iot:us-east-1:111122223333:command/reboot", CommandFacts::default())`), as tests/all/command_execution_validation.rs does.
2. POST /command-executions with body {"targetArn": "arn:aws:iot:us-east-1:111122223333:thing/laser-1", "commandArn": "arn:aws:iot:us-east-1:111122223333:command/absent"}.
3. Observe HTTP 400 with `__type` ValidationException and message "The command absent was not found.".
4. Expected per the model: the operation's `ResourceNotFoundException` (HTTP 404), documented as "The specified resource does not exist." — the error the handler declares but never constructs.
