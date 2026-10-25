Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# An invalid tag key is refused with the unmodeled error code ValidationException instead of InvalidRequestException

_Filed: 2026-10-24T13:53:13Z_
_Operation: CreatePortal | Class: error-response_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a bad request to `CreatePortal` — here a `tags` key that breaks the `TagKey` rules (over 128 characters, empty, or the reserved `aws:` prefix) — is refused with one of the errors the operation models. IoT SiteWise refuses malformed input with `InvalidRequestException` (HTTP 400) and models no `ValidationException` anywhere in the service, so a client matching on the wire error code must see `InvalidRequestException`.

Actual: `create_portal.rs` returns `crate::scaffolded::Scaffolded::validation(message)` for a bad tag key. That renders HTTP 400 with the synthetic `smithy.framework#ValidationException` shape and the wire error code `ValidationException` (`scaffolded.rs:265-290`) — a code this service never emits, carrying only `message` and no modeled shape. It is also internally inconsistent: `TagResource` (`tag_resource.rs:171-175`) and `CreatePipeline` (`create_pipeline.rs:264-266`) refuse the identical bad key with the modeled `InvalidRequestException`, so the same violation produces two different error codes depending on which operation the caller used. CreatePortal is the only handler in the crate that takes the synthetic path.

## Evidence

Smithy model: `CreatePortal` declares `InvalidRequestException` (400), `InternalFailureException`, `LimitExceededException`, `ResourceNotFoundException`, `ThrottlingException` — and the service models no `ValidationException` at all. Handler: `src/ops/create_portal.rs:221-227` (`Scaffolded::validation`) versus `src/ops/tag_resource.rs:171-175` and `src/ops/create_pipeline.rs:264-266` (both `InvalidRequestException` via `crate::state::check_tag_key`). `src/scaffolded.rs:207-223` itself states the modeled client-input error should be preferred when the service declares one. Tests: `tests/all/portal_lifecycle.rs` does not create a portal with an invalid tag key, so nothing asserts the error code.

## Steps to Reproduce

1. POST /portals with a valid portalName/portalContactEmail/roleArn and `tags: {"aws:reserved": "v"}` (or a 129-character key).
2. Observe HTTP 400 whose body `__type` is `ValidationException`.
3. Send the same tag key to TagResource on an existing portal ARN -> HTTP 400 `InvalidRequestException`. The two paths report the same violation under two different error codes, and only the second is a code IoT SiteWise can return.
