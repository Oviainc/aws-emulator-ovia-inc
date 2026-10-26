Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9a6c758bc8240aed8c4969459b74444d5ecf49ef

# Rotating a non-OPEN tunnel answers an unmodeled 400 ValidationException instead of the modeled ResourceNotFoundException

_Filed: 2026-10-24T13:14:39Z_
_Operation: RotateTunnelAccessToken | Class: error-response_

> _Note: this issue was identified against the code base at commit `9a6c758bc8240aed8c4969459b74444d5ecf49ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RotateTunnelAccessToken` declares exactly ONE error in this service's Smithy model: `ResourceNotFoundException` (HTTP 404). The service model defines no client-input/validation error at all (its only two error shapes are `ResourceNotFoundException` and `LimitExceededException`). Expected: when the named tunnel cannot have credentials reissued because it is no longer open (closed explicitly, or closed by the expiry sweep once `maxLifetimeTimeoutMinutes` elapsed), the refusal arrives as the operation's modeled error — wire code `ResourceNotFoundException`, HTTP 404 — so an SDK deserializes it into the generated, documented error type the operation declares.

Actual: the handler refuses with the framework's SYNTHETIC `ValidationException` (`crate::scaffolded::Scaffolded::validation`), which renders as HTTP 400 with `__type: "ValidationException"` — a shape in the `smithy.framework` namespace that is not in this service's model, so every AWS SDK sees it as an unmodeled/unknown error rather than `ResourceNotFoundException`, and code that branches on the modeled error (or on the 404 status) takes the wrong path. A client retry/error classifier therefore behaves differently against the emulator than against AWS.

The same unmodeled 400 code is used for the second refusal this operation enforces — `destinationConfig` supplied with `clientMode: SOURCE` — so both of this operation's refusal paths answer a code the model does not define. Both are symptoms of the same class: a documented failure mode answered with an error code/status AWS cannot produce for this operation.

## Evidence

Spec (smithy_query, operation=RotateTunnelAccessToken): `errors: [{shape: ResourceNotFoundException, errorKind: client, httpError: 404}]` — the only modeled error; no `ValidationException`/`InvalidRequestException` exists anywhere in `com.amazonaws.iotsecuretunneling`. The refusal is about the state of an existing RESOURCE (the tunnel is not OPEN, so no client access token can be minted for it), not about a malformed request member, and the handler's own message text (`"Tunnel {id} is close."`) is copied from AWS's wording for this refusal — i.e. AWS raises it as a resource-level error, which for this operation can only be `ResourceNotFoundException` (404).

Handler: `src/ops/rotate_tunnel_access_token.rs` lines ~198-210 — `if tunnel.status != TunnelStatus::Open { return Err(crate::scaffolded::Scaffolded::validation(format!("Tunnel {} is close.", input.tunnel_id))) }`; and lines ~178-183, the `destinationConfig` + non-destination `clientMode` pairing, also `Scaffolded::validation`. `src/scaffolded.rs` lines 136-144 and 285-291 show that variant renders as HTTP 400 with the `smithy.framework#ValidationException` shape and wire code `ValidationException`.

Tests: `tests/all/tunnel_refusal_rules.rs::rotate_is_refused_once_the_tunnel_is_no_longer_open` asserts the CURRENT behavior (`status == 400` and `__type == "ValidationException"`) across all three `clientMode` values and all three ways a tunnel stops being open, so the divergence is locked in by the test rather than caught by it; the same file's `rotate_refuses_a_destination_config_its_client_mode_does_not_cover` asserts the 400/`ValidationException` pair for the second path. No test asserts the modeled error for either path.

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` (e.g. `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}`); note the returned `tunnelId`.
2. Call `CloseTunnel` with `{"tunnelId": "<id>"}` (no `delete`), then `DescribeTunnel` to confirm the record is still readable with `status: CLOSED`.
3. Call `RotateTunnelAccessToken` with `{"tunnelId": "<id>", "clientMode": "ALL"}`.
4. Observe HTTP 400 with body `{"__type":"ValidationException","message":"Tunnel <id> is close."}` — an error code absent from this service's model; the operation's only modeled error is `ResourceNotFoundException` (HTTP 404), which is what an SDK expects to deserialize here.
5. Same code/status mismatch for the other refusal path: on an OPEN tunnel call `RotateTunnelAccessToken` with `{"tunnelId":"<id>","clientMode":"SOURCE","destinationConfig":{"services":["SSH"]}}` and observe HTTP 400 `ValidationException`.
