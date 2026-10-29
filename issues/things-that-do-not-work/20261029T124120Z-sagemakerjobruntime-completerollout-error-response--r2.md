Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 099b8d2e90cdf0281a13a19adea252c3b1691b86

# Refusal messages exceed the modelled 1024-character maximum of the error Message member

_Filed: 2026-10-29T12:41:20Z_
_Operation: CompleteRollout | Class: error-response_

> _Note: this issue was identified against the code base at commit `099b8d2e90cdf0281a13a19adea252c3b1691b86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every error shape of this service declares exactly one prose member, `Message`, targeting `FailureReason`, and `FailureReason` declares `@length` min 0 **max 1024**. The crate's refusal wording ignores that maximum: it quotes the caller's rejected value into the message up to the shared 16 KiB per-clause echo ceiling and serialises the result straight into `ValidationException.Message`, so a single refusal can answer a `Message` of ~16.4 KB — sixteen times the maximum the model declares for the member that carries it.

Two paths reach it, both on `CompleteRollout`:

1. Over the wire: `CompleteRollout`'s `Status` targets the `CompletionStatus` enum and declares no `@length`, so the rejected value is as large as the body carrying it. `service.rs` collects the violation with the shared crate's DEFAULT echo ceiling (`refusing_violations`, 16 KiB) and puts the rendered sentence into `ValidationException { message }` unchanged. The crate's own suite drives an exactly-16-KiB `Status` and asserts the whole sentence comes back in `Message` byte for byte.
2. In the handler: `enum_violation` builds the same clause through `service_common::validation::quoted_echo`, whose ceiling is the same 16 KiB, and the handler's unit test pins a 16380-byte quoted value inside one `Message`.

The same wording path words `JobArn` refusals (`malformed_arn_message` echoes the submitted header value), so the overflow is not specific to the enum member.

Nothing in this crate acknowledges the 1024 maximum: `service.rs` cites the member as "`FailureReason`, `@required`" and omits its `@length`, and the echo suites measure the ceiling against INPUT member bounds ("every other member a refusal of this service quotes (`TrajectoryId`, `JobArn`) declares `@length` max 256"), never against the error member's own declared maximum. The shared crate already exposes the seam for a service whose model states its own bound — `refusing_violations_quoting_up_to(protocol, req, max_echo)`, documented as "with the largest single value one clause may quote stated by the SERVICE rather than taken from this module's own `MAX_RESTORED_ECHO`" — and this service calls the defaulted `refusing_violations` instead, so respecting the modelled bound needs no new machinery.

## Evidence

Smithy model (`smithy_query(query="closure",operation="Sample")`):
- `"ValidationException": {"errorKind": "client", "httpError": 400, "members": {"Message": {"target": "FailureReason", "required": true, "length": {"min": 0, "max": 1024}}}}`
- `"FailureReason": {"shape_id": "com.amazonaws.sagemakerjobruntime#FailureReason", "type": "string", "length": {"min": 0, "max": 1024}}` — every error shape of the service (`AccessDeniedException`, `ConflictException`, `InternalServiceError`, `ResourceNotFoundException`, `ServiceQuotaExceededException`, `ThrottlingException`, `ValidationException`) carries that same one member.

Code that overruns it:
- `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/service.rs:255-257`: `fn refused_by<O: Operation>(...) -> Vec<Violation> { service_common::validation::refusing_violations::<O>(protocol, req) }`
- same file `:264-270`: `let refused = crate::ops::types::shared_types::ValidationException { message: refusal_message(violations)? };` — the rendered sentence goes into `Message` with no bound of its own.
- `src/ops/complete_rollout.rs:169-179`: `value: Some(service_common::validation::quoted_echo(value).into_owned())` … `let message = ::aws_smithy_schema_server::envelope(...)` → `validation(message)`.
- `src/ops/complete_rollout.rs:160-163` states the ceiling it uses and why, naming only the request-size concern: "`Status` targets an enum and declares no `@length`, so the value a caller can put here is unbounded, and a clause that interpolated it whole would answer a request-sized message." The member's own 1024 maximum is never mentioned (the crate's only `1024` literals are test payload sizes and `16 * 1024`).

Tests that pin the overrun:
- `tests/all/enum_refusal_echo_is_bounded.rs:152-177`: row `("ASCII exactly at the ceiling", "s".repeat(MAX_RESTORED_ECHO))` (`MAX_RESTORED_ECHO: usize = 16 * 1024`, line 39) is sent as `CompleteRollout`'s `Status` and asserted `assert_eq!(actual, expected, ...)` where `expected = one_validation_error(&status, "status", VALUE_SET)` — i.e. the wire `Message` is the ~16.4 KB sentence.
- `src/ops/complete_rollout.rs:354-372`: `("exactly at the ceiling", "s".repeat(MAX_RESTORED_ECHO))` and `("multibyte characters just under the ceiling", "€".repeat(5460))` assert the submitted 16380 bytes are quoted "byte for byte" inside the `ValidationException.message`.

Seam already available in a crate this service depends on:
- `services/common/src/validation.rs:156-174`: "As [`refusing_violations`], with the largest single value one clause may quote stated by the SERVICE rather than taken from this module's own [`MAX_RESTORED_ECHO`] … so the ceiling is a MODEL constant" — `pub fn refusing_violations_quoting_up_to<O: Operation>(protocol, req, max_echo)`.
- `services/common/src/validation.rs:56-59`: the default is justified as "64x the largest `@length` maximum most emulated members declare (256)" — a statement about input members, not about the error `Message` member this service declares at 1024.

## Steps to Reproduce

1. Register a job in the shared directory (as the SageMaker control plane does), e.g. `arn:aws:sagemaker:us-east-1:123456789012:job/AgentRFT/echo-run-1`.
2. POST `/complete-rollout` with header `X-Amzn-SageMaker-Job-Arn: <that ARN>` and body `{"TrajectoryId":"traj-1","Status":"<8192 's' characters>","ClientToken":"tok"}`.
3. Observe `400 ValidationException` whose `Message` is `1 validation error detected: Value '<the 8192 characters>' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ready, failed]` — roughly 8.3 KB of prose.
4. Compare with the model: `ValidationException.Message` targets `FailureReason`, declared `@length` max 1024, so no `Message` this service answers may exceed 1024 characters.
5. Same overrun for a long `X-Amzn-SageMaker-Job-Arn` header value (a pattern violation), which `service.rs::malformed_arn_message` echoes into `Message` as `Input arn <value> is malformed`.
