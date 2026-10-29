Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6078ee6ad52df0e8ecffa919707cccb1e63193a4

# CompleteRollout's enum refusal echoes the whole submitted Status, bypassing the crate's bounded value echo

_Filed: 2026-10-29T01:16:26Z_
_Operation: CompleteRollout | Class: error-response_

> _Note: this issue was identified against the code base at commit `6078ee6ad52df0e8ecffa919707cccb1e63193a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CompleteRollout` is the only operation of this crate that builds a constraint-violation clause itself: `Status` targets the `CompletionStatus` enum, the framework's constraint walk does not refuse an unmodelled value (the handler is the enforcer — see `crate::ops::complete_rollout::handle`), so `enum_violation()` hand-builds an `aws_smithy_schema_server::Violation` and renders it with `envelope`.

That hand-built violation stores `value: Some(value.to_string())` — the submitted `Status` string in FULL, with no ceiling. `envelope` interpolates whatever is stored, so the 400 `ValidationException` message is sized by the request: a `CompleteRollout` body whose `Status` carries 1 MiB of text comes back with that 1 MiB quoted inside `Message`.

Every OTHER refusal this crate words goes through the shared echo-restoring step (`service.rs::refused_by` -> `service_common::validation::refusing_violations`), which exists precisely to keep that from happening: it restores a submitted value into a clause only up to `MAX_RESTORED_ECHO` (16 KiB) and otherwise leaves the framework's shortened `... [<n> bytes]` rendering standing. So the two refusal paths of the same crate disagree for the same message family, and the handler's own doc comment asserts the opposite of what the code does. The crate's own suite states the bound as a RULE and pins it only on the framework path (`trajectoryId`), with the enum rows stopping at 300 characters, so nothing catches the unbounded path.

The defect is the missing ceiling on the hand-built clause, not the wording: every value the existing tests submit (<= 600 bytes) must keep its byte-for-byte echo.

## Evidence

The hand-built clause, unbounded — `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/complete_rollout.rs:150-168`:
```
/// The refusal for a `Status` outside the modelled `CompletionStatus` enum.
///
/// The clause is built by the framework's own constraint renderer
/// (`aws_smithy_schema_server::envelope` over a `ViolationKind::EnumValue`), so an enum
/// value the model declines is worded exactly as every other constraint violation this
/// emulator refuses — ...
fn enum_violation(value: &str) -> CompleteRolloutError {
    let violation = ::aws_smithy_schema_server::Violation {
        path: "status".to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::EnumValue(...),
        value: Some(value.to_string()),
    };
    let message = ::aws_smithy_schema_server::envelope(std::slice::from_ref(&violation))
```
and it is reached from the handler for any value the model does not declare — `src/ops/complete_rollout.rs:224-230`: `Some(value) => match crate::state::CompletionStatus::from_wire(value) { ... None => return Err(enum_violation(value)) }`.

The path EVERY other refusal of this crate takes, which does bound the echo — `src/service.rs:255-257`:
```
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    service_common::validation::refusing_violations::<O>(protocol, req)
}
```
and the rule that helper states — `services/common/src/validation.rs:56-59`: "[`MAX_RESTORED_ECHO`] (16 KiB) — the largest single value restored into a clause by default. ... the fidelity this exists for — echoing a 300-character id, an over-long ARN, a rejected enum value — always lands, while one clause can never carry request-sized content." and `services/common/src/validation.rs:76-77`: "Beyond any of these the framework's elided text stands: a shortened echo is a divergence in one message, while an unbounded one is a response sized by the request." (`MAX_RESTORED_ECHO` is declared at `services/common/src/validation.rs:389`.)

Test coverage, which states the rule but exercises only the framework path — `tests/all/rejected_value_echo.rs:345-354`: "RULE (the echo's own bound): the echo is restored up to the per-clause maximum stated in `services/common/src/validation.rs` — `MAX_RESTORED_ECHO`, 16 KiB ... and past it the response stays bounded rather than request-sized"; its rows past the bound are all `TrajectoryId` on `CompleteRollout` (`tests/all/rejected_value_echo.rs:371-403`, asserting `message.len() < value.len()` and `[<n> bytes]`), while the enum rows stop at `("a 300-character status", "s".repeat(300))` and `("a multibyte status", "ré".repeat(200))` (`tests/all/rejected_value_echo.rs:284-296`), both far under 16 KiB — so the unbounded handler path is never reached by a test.

## Steps to Reproduce

1. Create a job in the shared directory so the ARN resolves (as `tests/all/harness.rs::create_job_at_arn` does), e.g. `arn:aws:sagemaker:us-east-1:123456789012:job/AgentRFT/echo-run-1`.
2. POST `/complete-rollout` with header `X-Amzn-SageMaker-Job-Arn: <that ARN>` and body `{"TrajectoryId":"traj-1","Status":"<1 MiB of 's'>","ClientToken":"tok"}`.
3. Observe the 400 `ValidationException`: `Message` is `1 validation error detected: Value '<the whole 1 MiB>' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ready, failed]` — the response is sized by the request.
4. Compare the same-size value on the member that goes through the shared path: POST `/complete-rollout` with `TrajectoryId` set to 1 MiB of 'b' and `Status: "ready"`. That refusal stays bounded and renders `... [1048576 bytes]`, as `tests/all/rejected_value_echo.rs::the_echo_is_restored_up_to_the_stated_bound_and_bounded_past_it` pins.
5. Re-run `cargo test -p service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice` after bounding the hand-built clause: every existing row (statuses of <= 600 bytes) must still be echoed byte for byte.
