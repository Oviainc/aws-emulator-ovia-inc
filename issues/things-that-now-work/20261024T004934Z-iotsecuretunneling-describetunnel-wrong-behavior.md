Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# Tunnel store is not partitioned by region, so a tunnel is visible through every region's endpoint

_Filed: 2026-10-24T00:49:34Z_
_Operation: DescribeTunnel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A tunnel is an AWS IoT Secure Tunneling regional resource: it lives in the region of the endpoint (`api.tunneling.iot.<region>.amazonaws.com`) it was opened against, and its ARN embeds that region (`arn:aws:iot:<region>:<account>:tunnel/<id>`). Expected: a tunnel opened with credentials scoped to `us-east-1` is NOT visible through the `eu-west-1` endpoint — `DescribeTunnel` there answers `ResourceNotFoundException` (HTTP 404), and `ListTunnels` there does not list it.

Actual: the emulator keys every tunnel only by `tunnelId` in one global map (`state.rs`: `tunnels: BTreeMap<String, StoredTunnel>`), and no handler looks at the request's region at all except `OpenTunnel`, which only uses it to render the ARN (`crate::state::request_region(ctx)` -> `tunnel_arn(&region, &tunnel_id)`). So `DescribeTunnel` signed for any region returns HTTP 200 for a tunnel created in another region, and the returned `tunnel.tunnelArn` names a region different from the one the caller is talking to — a response AWS can never produce. The same unpartitioned store makes `ListTunnels` report other regions' tunnels (its own documentation scopes the listing to "all tunnels for an Amazon Web Services account" in the region being called), and lets `CloseTunnel`/`RotateTunnelAccessToken` mutate a tunnel through the wrong region's endpoint. One fix (keying the store, and the ARN index, by the request's region/account alongside the id) covers all four operations; they are all symptoms of the same root cause.

Note the tag operations already behave correctly by accident, because `arn_index` is keyed by the full ARN, so a same-account/other-region ARN does not resolve.

## Evidence

Spec/semantics: the Smithy model gives the service `endpointPrefix: api.tunneling.iot` with an endpoint rule set (per-region endpoints), models `TunnelArn` on `Tunnel`/`OpenTunnelResponse`, and `DescribeTunnel` declares `ResourceNotFoundException` (HTTP 404) as the answer for a tunnel this endpoint does not hold. AWS resources addressed by a regional endpoint are never readable through another region's endpoint — well-established service behavior reflected in the region segment of the ARN the emulator itself builds.

Handler: `src/ops/describe_tunnel.rs` looks the tunnel up with `store.tunnels.get(&input.tunnel_id)` and never reads the request's credential scope (`let _ = ctx;`). `src/state.rs` lines ~152-170 define a single global `tunnels` map; `request_region` exists but is called only from `src/ops/open_tunnel.rs`.

Tests: `tests/all/tunnel_lifecycle.rs` and `tests/all/tunnel_refusal_rules.rs` sign every request for the one region `eu-west-1` (`const REGION: &str = "eu-west-1";`), so no test asserts cross-region isolation.

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` with an Authorization header whose SigV4 credential scope names `us-east-1`; note the returned `tunnelId` and that `tunnelArn` is `arn:aws:iot:us-east-1:000000000000:tunnel/<id>`.
2. Call `DescribeTunnel` for that `tunnelId` with an Authorization header whose credential scope names `eu-west-1`.
3. Observe HTTP 200 with the full tunnel (its `tunnelArn` still naming `us-east-1`); AWS answers `ResourceNotFoundException` (404) because the `eu-west-1` endpoint holds no such tunnel.
4. Call `ListTunnels` scoped to `eu-west-1` and observe the `us-east-1` tunnel in `tunnelSummaries`; likewise `CloseTunnel` scoped to `eu-west-1` closes it.
