Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d204f113841562be393bb0d029d4ee5d3cf48852

# A clientToken replay is refused 400 once the command is deprecated, although the handler states a replay returns the first execution's id

_Filed: 2026-10-28T23:16:40Z_
_Operation: StartCommandExecution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d204f113841562be393bb0d029d4ee5d3cf48852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCommandExecution`'s handler states one unconditional idempotency rule for `clientToken` — the same token with the same parameters returns the same `executionId` and creates nothing — and the crate's own concurrency suite relies on that rule short-circuiting a refusal that would otherwise apply (a replay at the per-device concurrency limit is answered 200 from the index instead of 402, because the index lookup runs BEFORE `try_admit_command_execution`).

The two command-registry refusals are ordered the other way round: the `facts()` lookup (command does not exist -> 400) and the `command.deprecated` branch (400) both run BEFORE the idempotency lookup. The shared command index is written by the IoT control plane at any time (`iot:UpdateCommand` -> `registry.lock().set_deprecated(...)`), so between a client's first call and its retry the command can become deprecated. The identical retry — same token, same `targetArn`/`commandArn`/`parameters`/`executionTimeoutSeconds`, hence the same fingerprint that would match the stored `StartedCommand` — is then answered `400 ValidationException` "The command <name> has been deprecated." instead of completing with the `executionId` the first call minted and still holds in `state.idempotency`.

So the idempotency guarantee the handler documents holds against one refusal (the quota) and not against the two that depend on a store a different service mutates: a client retrying a request that already succeeded is told its request was invalid, and has no way to learn the execution it started. Nothing in the crate states a reason for the split ordering, and no test covers a replay after a deprecation.

## Evidence

The rule the handler states for itself — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs:21-25:
```
//! returns its id. `clientToken` makes the call idempotent: the same token with the
//! same parameters returns the same `executionId` and creates nothing, the same
//! token with different parameters is a 409 `ConflictException` (the token member's
//! own documented rule).
```
and the member doc it cites, quoted in the same file at start_command_execution.rs:70-76:
```
//! * `clientToken` (ClientRequestTokenV2, len 1..64, pattern ^[\x21-\x7E]+$,
//!   @idempotencyToken) — The client token is used to implement idempotency. It ensures that
//!   the request completes no more than one time. If you retry a request with the same token
//!   and the same parameters, the request will complete successfully. However, if you retry
//!   the request using the same token but different parameters, an HTTP 409 conflict occurs.
```

The code puts the registry refusals first — start_command_execution.rs:457-466:
```
        let command = commands.lock().facts(&input.command_arn);
        let Some(command) = command else {
            return Err(invalid_request(command_not_found_message(command_name)));
        };
        // A DEPRECATED command still exists and is still readable, but it is not one an
        // execution may be started from any more: the request is refused with this
        // operation's modelled client error, and nothing is stored.
        if command.deprecated {
            return Err(invalid_request(command_deprecated_message(command_name)));
        }
```
and only then consults the index — start_command_execution.rs:500-513:
```
        let mut state = state.lock();

        // Idempotency, as `clientToken` documents it: the same token with the same
        // parameters completes with the execution the first call created; ...
        if let Some(token) = input.client_token.as_deref() {
            if let Some(started) = state.idempotency.get(token) {
                if started.request_fingerprint == fingerprint {
                    return Ok(ResponseDocument::new(
                        StartCommandExecutionOutput {
                            execution_id: Some(started.execution_id.clone()),
```
while the OTHER refusal of this same handler is deliberately placed after that lookup — start_command_execution.rs:538-540 (`try_admit_command_execution`), the ordering the crate's own test states as the rule — tests/all/command_execution_concurrency.rs:407-411:
```
/// RULE: `clientToken` idempotency is answered from the index, so a retry of an
/// already-admitted token completes with the execution the first call created even
/// while the device is at its concurrency limit — it creates no second execution
/// and consumes no slot. A retry under a FRESH token is a new request and is
/// refused like any other start past the limit.
```

That the deprecation flag can flip between the two calls — services/service-com-amazonaws-iot-awsiotservice/src/ops/update_command.rs:231:
```
        registry.lock().set_deprecated(&command_arn, deprecated);
```
over the same shared index this handler reads (services/common/src/iot_commands.rs:141-149, `set_deprecated`).

Test coverage: the deprecation refusal is pinned for a FIRST start only (tests/all/command_execution_validation.rs:680 `a_deprecated_command_is_read_back_in_the_refusal`, no `clientToken` in its requests), and the replay rules are pinned only against the quota (tests/all/command_execution_concurrency.rs:413) and against a live command (tests/all/job_execution_lifecycle.rs:325-331). No test replays a token after the command's facts changed.

## Steps to Reproduce

1. Register command C in the shared IoT command index with `CommandFacts::default()` (what `iot:CreateCommand` does; the suites seed it via `command_registry(&*engine).lock().register(C, CommandFacts::default())`).
2. POST /command-executions with `{"targetArn": "arn:aws:iot:us-east-1:111122223333:thing/laser-1", "commandArn": C, "clientToken": "t1"}` and observe 200 with `executionId` E.
3. Deprecate the command through the control plane: `iot:UpdateCommand(commandId=C, deprecated=true)` (services/service-com-amazonaws-iot-awsiotservice/src/ops/update_command.rs:231), or equivalently `command_registry(&*engine).lock().set_deprecated(C, true)`.
4. Retry the byte-identical POST from step 2 (same `clientToken` "t1", same parameters).
5. Observe 400 ValidationException "The command <C name> has been deprecated." instead of 200 with `executionId` E — although `state.idempotency` still holds the token with a matching fingerprint and the execution still exists (GET /things/laser-1/jobs lists it), and although the same handler answers a replay at the concurrency limit 200 from that index.
