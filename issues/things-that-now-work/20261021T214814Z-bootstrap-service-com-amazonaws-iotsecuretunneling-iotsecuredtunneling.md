Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the IoTSecureTunneling emulator

_Filed: 2026-10-21T21:48:14Z_
_Operation: service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 8 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL stateful slice covering the tunnel lifecycle — the service's primary (and only) resource.

### Happy path to implement (in order)

1. **OpenTunnel** (`POST /tunnels`) — input `description`, `tags` (list<Tag> key/value), `destinationConfig` (`DestinationConfig{thingName, services (required, len>=1)}`), `timeoutConfig` (`TimeoutConfig{maxLifetimeTimeoutMinutes}`, default 720 when absent). Must MINT and PERSIST a new tunnel: generate a `tunnelId` matching `^[a-zA-Z0-9_\-+=:]{1,128}$`, build `tunnelArn` as `arn:aws:iot:<region>:<account>:tunnel/<tunnelId>`, and generate two distinct opaque tokens returned as `sourceAccessToken` and `destinationAccessToken`. Store status `OPEN`, `createdAt`/`lastUpdatedAt` = now, plus the supplied description/destinationConfig/timeoutConfig/tags. Output: `tunnelId`, `tunnelArn`, `sourceAccessToken`, `destinationAccessToken`.

2. **DescribeTunnel** (`GET /tunnels/{tunnelId}`) — look the tunnel up by id in the store and return the full `Tunnel` structure built from what was persisted: `tunnelId`, `tunnelArn`, `status` (OPEN/CLOSED), `sourceConnectionState`/`destinationConnectionState` (`ConnectionState{status: DISCONNECTED, lastUpdatedAt}` until a proxy connects), `description`, `destinationConfig`, `timeoutConfig`, `tags`, `createdAt`, `lastUpdatedAt`. Unknown id -> `ResourceNotFoundException` (HTTP 404). Access tokens must NOT be echoed here (the real API does not return them from DescribeTunnel).

3. **ListTunnels** (`GET /tunnels`) — return a `TunnelSummary` per stored tunnel (`tunnelId`, `tunnelArn`, `status`, `description`, `createdAt`, `lastUpdatedAt`), computed from state, not canned. Honour `thingName` as a filter against the stored `destinationConfig.thingName`, honour `maxResults` (1..100) as a page size, and emit/accept an opaque `nextToken` (`^[a-zA-Z0-9_=-]{1,4096}$`) that resumes exactly where the previous page stopped; omit `nextToken` on the last page.

4. **RotateTunnelAccessToken** (`POST /tunnel/{tunnelId}/rotate`) — required `tunnelId` + `clientMode` (`ALL|SOURCE|DESTINATION`), optional `destinationConfig`. Must regenerate ONLY the tokens selected by `clientMode` (invalidating the old ones in the store), apply `destinationConfig` if supplied, bump `lastUpdatedAt`, and return `tunnelArn` plus the newly minted `sourceAccessToken` and/or `destinationAccessToken`. Unknown id -> `ResourceNotFoundException` (404).

5. **CloseTunnel** (`DELETE /tunnels/{tunnelId}?delete=`) — unknown id -> `ResourceNotFoundException` (404). With `delete=false` (the modeled default) the tunnel must transition to status `CLOSED` and remain visible to DescribeTunnel/ListTunnels with updated `lastUpdatedAt`; with `delete=true` the tunnel record must be REMOVED so a subsequent DescribeTunnel returns 404. Empty output body.

6. **TagResource / ListTagsForResource / UntagResource** (`POST /tags`, `GET /tags?resourceArn=`, `POST /untag`) — tags keyed by the tunnel's `tunnelArn`: TagResource merges/overwrites by tag key, ListTagsForResource returns exactly the stored tags for that ARN, UntagResource removes the named `tagKeys`. An ARN not naming a stored tunnel -> `ResourceNotFoundException` (404). Tags set at OpenTunnel time must be visible via ListTagsForResource, and tags added via TagResource must appear in DescribeTunnel's `tunnel.tags`.

Every one of these MUST compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, and no generating a tunnelId/token that is never stored. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker from each file as it is implemented.

## State Model

In `src/state.rs`, on the service state struct:

- `tunnels: BTreeMap<String /* tunnelId */, Tunnel>` — insertion/creation ordered so ListTunnels paging is stable.
  - `Tunnel { tunnel_id: String, tunnel_arn: String, status: TunnelStatus /* OPEN | CLOSED */, description: Option<String>, destination_config: Option<DestinationConfig { thing_name: Option<String>, services: Vec<String> }>, timeout_config: TimeoutConfig { max_lifetime_timeout_minutes: i32 /* default 720 */ }, source_connection_state: ConnectionState { status: ConnectionStatus /* CONNECTED | DISCONNECTED */, last_updated_at: Timestamp }, destination_connection_state: ConnectionState, source_access_token: String, destination_access_token: String, created_at: Timestamp, last_updated_at: Timestamp }`
- `tags: BTreeMap<String /* tunnelArn */, BTreeMap<String /* key */, String /* value */>>` — authoritative tag store shared by OpenTunnel, TagResource, ListTagsForResource, UntagResource and DescribeTunnel's `tunnel.tags`.
- `arn_index: BTreeMap<String /* tunnelArn */, String /* tunnelId */>` — so the tag operations can resolve an ARN to a tunnel and 404 on an unknown one.

Access tokens are stored on the tunnel (not returned by DescribeTunnel) so RotateTunnelAccessToken can be observed to actually replace them.

Operations outside this slice stay 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/OPERATIONS.md` lists all 8 bound operations with their routes, input/output members and errors (OpenTunnel `POST /tunnels` -> tunnelId/tunnelArn/sourceAccessToken/destinationAccessToken; DescribeTunnel `GET /tunnels/{tunnelId}` -> `tunnel` with ResourceNotFoundException 404; ListTunnels `GET /tunnels` @paginated(inputToken=nextToken, outputToken=nextToken) with thingName/maxResults 1..100; RotateTunnelAccessToken `POST /tunnel/{tunnelId}/rotate` with required clientMode ALL|DESTINATION|SOURCE; CloseTunnel `DELETE /tunnels/{tunnelId}` with `delete` query defaulting to false; TagResource/ListTagsForResource/UntagResource over `resourceArn`). `smithy_query shape=Tunnel,TunnelSummary,DestinationConfig,TimeoutConfig,ConnectionState` gives the field list used for the state model, including TunnelStatus ("Valid values are: Open and Closed"), ConnectionStatus (CONNECTED/DISCONNECTED) and TimeoutConfig.maxLifetimeTimeoutMinutes (range 1..720, "If not specified, maxLifetimeTimeoutMinutes defaults to 720 minutes"). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 8 handler files (open_tunnel.rs, describe_tunnel.rs, list_tunnels.rs, rotate_tunnel_access_token.rs, close_tunnel.rs, tag_resource.rs, list_tags_for_resource.rs, untag_resource.rs), each returning `Scaffolded::not_implemented(..)` => HTTP 501. No tests assert any behavior beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `iotsecuretunneling` at it (awsJson1_1-era REST routes, endpoint prefix `api.tunneling.iot`).
2. `OpenTunnel` with description="demo", destinationConfig={thingName:"thing-1", services:["SSH"]}, timeoutConfig={maxLifetimeTimeoutMinutes:60}, tags=[{key:"env",value:"test"}] — expect 200 with a non-empty tunnelId, tunnelArn and two DISTINCT access tokens (today: 501).
3. `DescribeTunnel` with that tunnelId — expect 200 and tunnel.status=OPEN, tunnel.description="demo", tunnel.destinationConfig.services=["SSH"], tunnel.timeoutConfig.maxLifetimeTimeoutMinutes=60, tunnel.tags containing env=test, and createdAt set.
4. `ListTunnels` with thingName="thing-1" — expect the new tunnel's summary; `ListTunnels` with thingName="other" — expect no summaries. Open 3 tunnels and call ListTunnels with maxResults=2, then re-call with the returned nextToken — expect the remaining tunnel and no nextToken, with no duplicates across pages.
5. `ListTagsForResource` with resourceArn=<tunnelArn> — expect env=test; `TagResource` adding owner=alice, then `ListTagsForResource` — expect both tags; `UntagResource` with tagKeys=["env"], then `ListTagsForResource` — expect only owner=alice.
6. `RotateTunnelAccessToken` with clientMode=SOURCE — expect 200 with a sourceAccessToken DIFFERENT from the OpenTunnel one, and lastUpdatedAt bumped on a follow-up DescribeTunnel.
7. `CloseTunnel` with delete omitted/false — expect 200, then DescribeTunnel returns 200 with tunnel.status=CLOSED.
8. `CloseTunnel` with delete=true — expect 200, then DescribeTunnel for that tunnelId returns ResourceNotFoundException (HTTP 404), and the tunnel is absent from ListTunnels.
9. `DescribeTunnel`/`CloseTunnel`/`RotateTunnelAccessToken`/`TagResource` with an unknown tunnelId or ARN — expect ResourceNotFoundException (HTTP 404), not 501 and not 200.
