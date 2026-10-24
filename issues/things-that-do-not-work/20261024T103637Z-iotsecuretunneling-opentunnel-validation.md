Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: a8ff7f4dc53b4b49522420414dd554a0d94e7915

# destinationConfig.services is accepted with any number of entries, past the per-tunnel data-stream limit

_Filed: 2026-10-24T10:36:37Z_
_Operation: OpenTunnel | Class: validation_

> _Note: this issue was identified against the code base at commit `a8ff7f4dc53b4b49522420414dd554a0d94e7915`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS IoT Secure Tunneling multiplexes a tunnel over at most three simultaneous data streams, i.e. a tunnel's `destinationConfig.services` may name at most 3 services; a request naming more is refused with a 4xx client error and no tunnel is created. Expected: `OpenTunnel` with `destinationConfig: {thingName: "t", services: ["SSH","HTTP","DB","VNC"]}` is refused with a client error, no tunnel id/ARN is minted and no access tokens are handed out.

Actual: the handler copies whatever list the request carried straight into the stored record (`src/ops/open_tunnel.rs`: `destination_config = input.destination_config.as_ref().map(|config| StoredDestinationConfig { thing_name: ..., services: config.services.clone() })`) and answers HTTP 200 with a tunnel id, ARN and two access tokens. `DescribeTunnel` then reports a tunnel with 4+ services — a record AWS can never produce. The modeled `ServiceList` carries only `length(min: 1)` (no maximum), so the boundary validator cannot catch this; the rule has to be enforced in the handler, the same way the crate already enforces the other unmodeled rules through `crate::scaffolded::Scaffolded::validation` (see `rotate_tunnel_access_token.rs` and `list_tunnels.rs`).

The same unchecked path exists on the other writer of this field: `src/ops/rotate_tunnel_access_token.rs` replaces `tunnel.destination_config` with the request's config (for `clientMode` DESTINATION/ALL) without bounding `services` either, so a rotate can also push a stored tunnel past the limit. One check shared by both writers covers it.

## Evidence

Spec: `com.amazonaws.iotsecuretunneling#DestinationConfig$services` -> `ServiceList` (`length(min: 1)`, member `Service` length 1..128, pattern `^[a-zA-Z0-9:_-]+$`) — the model expresses no maximum, and the member doc ("A list of service names that identify the target application...") states none, so the cap is service-enforced behavior rather than a modeled constraint. Well-established behavior: secure tunneling supports multiplexing of up to three simultaneous data streams (service names) per tunnel, so a `services` list longer than that is rejected at request time.

Handler: `src/ops/open_tunnel.rs` lines ~162-169 (clone of `config.services` with no length check) and lines ~215-242 (the record is stored and 200 returned); `src/ops/rotate_tunnel_access_token.rs` lines ~220-225 writes the same field unchecked.

Tests: `tests/all/tunnel_lifecycle.rs` open cases use one and several services and `tests/all/tunnel_refusal_rules.rs::rotate_refuses_a_destination_config_its_client_mode_does_not_cover` uses a 3-service config and asserts it is ACCEPTED — nothing asserts a refusal above the per-tunnel stream limit, so this is uncovered (and consistent with a cap of 3).

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` with `{"destinationConfig": {"thingName": "thing-1", "services": ["SSH", "HTTP", "DB", "VNC"]}}`.
2. Observe HTTP 200 with `tunnelId`, `tunnelArn`, `sourceAccessToken` and `destinationAccessToken` instead of a 4xx client refusal.
3. Call `DescribeTunnel` for the returned `tunnelId` and observe `tunnel.destinationConfig.services` carrying all four service names.
4. Repeat through the other writer: open a tunnel with one service, then call `RotateTunnelAccessToken` with `clientMode=ALL` and `destinationConfig: {"services": ["SSH","HTTP","DB","VNC"]}` — also accepted, and the oversized list is persisted.
