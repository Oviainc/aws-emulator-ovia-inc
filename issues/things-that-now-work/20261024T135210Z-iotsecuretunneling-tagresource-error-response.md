Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Per-resource tag-limit refusal answers an unmodeled 400 ValidationException instead of the modeled LimitExceededException (403)

_Filed: 2026-10-24T13:52:10Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a request that would take a tunnel past the per-resource tag limit is refused with the limit error AWS uses for this service — `LimitExceededException`, HTTP 403, the only limit error `com.amazonaws.iotsecuretunneling` models ("Thrown when a tunnel limit is exceeded") — so an SDK client resolves the refusal to a modeled error shape it can branch on.

Actual: both tag entry points refuse with `crate::scaffolded::Scaffolded::validation(..)`, i.e. HTTP 400 with the wire error code `ValidationException`, a shape that exists nowhere in this service's model (it is the framework's synthetic `smithy.framework#ValidationException`). `src/ops/tag_resource.rs` returns it when `merged_tag_count(..) > MAX_TAGS_PER_TUNNEL` (50), and `src/ops/open_tunnel.rs` returns it for the same limit when the tags arrive at open time. A client that matches on the error code or status therefore sees a code the service never emits, and no modeled error at all; the same root cause (one shared constant, two entry points) covers both operations.

This is the same divergence pattern already accepted for this service in `20261024T131439Z-iotsecuretunneling-rotatetunnelaccesstoken-error-response.md` (a resource/limit-level refusal must use a modeled error, not the synthetic `ValidationException`), applied to the tag paths rather than to the rotate path. If, on the other hand, the service accepts up to the modeled `TagList` bound (`length(min: 1, max: 200)`) per request and enforces no 50-key accumulation cap, then the refusal itself — not just its code — is the divergence; either way the current 400 `ValidationException` is not a response AWS can produce.

## Evidence

Spec: `com.amazonaws.iotsecuretunneling#LimitExceededException` (errorKind client, httpError 403, doc "Thrown when a tunnel limit is exceeded") is the only limit error in the model and is bound to `OpenTunnel`; `TagResource` models only `ResourceNotFoundException` (404). No `ValidationException`/`InvalidRequestException` shape exists anywhere in the model, so the code the emulator emits is not in this service's error vocabulary. Well-established behavior: a tunnel is an AWS IoT resource (`arn:aws:iot:<region>:<account>:tunnel/<id>`) and AWS IoT's documented quota is 50 tags per resource, reported as a limit-exceeded error rather than as an input-validation error.

Handler: `src/ops/tag_resource.rs` lines ~153-159 (`if merged > crate::state::MAX_TAGS_PER_TUNNEL { return Err(crate::scaffolded::Scaffolded::validation(..)) }`) and `src/ops/open_tunnel.rs` lines ~193-204 (same refusal for the `tags` member); the constant and its rationale are in `src/state.rs` (`MAX_TAGS_PER_TUNNEL = 50`), which notes the model publishes no such bound.

Tests: `tests/all/tunnel_lifecycle.rs::a_tunnel_carries_at_most_its_limit_of_tags` asserts the CURRENT behavior (`status == 400` and `__type == "ValidationException"`) for both the `OpenTunnel` and the `TagResource` over-limit rows, so it locks in the unmodeled code; nothing asserts the modeled 403 `LimitExceededException`.

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` with `destinationConfig: {thingName: "thing-tags", services: ["SSH"]}` and 50 tags (`k0..k49`); keep the returned `tunnelArn`.
2. Call `TagResource` with that `resourceArn` and one new tag (`{"key": "one-too-many", "value": "v"}`).
3. Observe HTTP 400 with body `__type: "ValidationException"` — an error code this service's model does not define — instead of HTTP 403 `LimitExceededException`.
4. Call `OpenTunnel` directly with 51 tags and observe the same 400 `ValidationException`, even though `OpenTunnel` declares `LimitExceededException` (403) as its refusal error.
5. Confirm with `ListTagsForResource` that nothing was stored in either case (the state handling is correct; only the error code/status diverges).
