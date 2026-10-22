Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the ApiGatewayManagementApi emulator

_Filed: 2026-10-21T21:18:07Z_
_Operation: service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: all three handlers under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). The service is a 3-operation API (restJson1, signing name `execute-api`) whose entire surface is the connection lifecycle, so the bootstrap slice is all three operations plus the connection registry they share.

### Happy path to implement (in order)

1. **PostToConnection** — `POST /@connections/{ConnectionId}`, input `ConnectionId` (@httpLabel, required) and `Data` (blob, @httpPayload, required, length 0..131072); no output members, success status 200. Must look `ConnectionId` up in the connection registry; on hit, append `Data` to that connection's delivered-message queue and bump `LastActiveAt`. On an unknown/deleted connection return the modeled `GoneException` (HTTP 410), NOT 200 and not 501. A `Data` payload above 131072 bytes must surface `PayloadTooLargeException` (HTTP 413).
2. **GetConnection** — `GET /@connections/{ConnectionId}`, input `ConnectionId` (@httpLabel, required); output `ConnectedAt` (ISO8601 timestamp), `LastActiveAt` (ISO8601 timestamp), `Identity` (`Identity` shape: `SourceIp`, `UserAgent`). Every field must be read back from the stored connection record — `ConnectedAt` is the time the connection was registered, `LastActiveAt` is the time of the most recent PostToConnection/GetConnection-observable activity, and `Identity` is whatever was recorded for the connection. No fabricated or `now()`-echoed timestamps. Unknown/deleted connection → `GoneException` (410).
3. **DeleteConnection** — `DELETE /@connections/{ConnectionId}`, input `ConnectionId` (@httpLabel, required); no output members, success status 204. Must remove the connection record from the registry so subsequent GetConnection/PostToConnection/DeleteConnection on the same id all answer `GoneException` (410).
4. **Connection registration seam** — this API has no modeled operation that creates a connection (in real AWS the connection is created by a WebSocket API client handshake). The bootstrap must therefore provide a way for a test/client to make a connection exist in the registry: either an emulator-internal seeding hook on the shared engine state, or auto-registration of a connection id on first reference, with the chosen rule documented in the crate. Without this seam the 410-only behaviour is unverifiable and the service can never return a successful GetConnection.

Every response must be computed from persisted engine state reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the request back, no generating timestamps at response time, no accepting a PostToConnection for a connection that was never registered.

## State Model

Add to `ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState`:

- `connections: HashMap<String /* ConnectionId */, Connection>`

```text
Connection {
    connection_id: String,          // the @httpLabel value / registry key
    connected_at: SystemTime,       // set once at registration; serialized as ConnectedAt (ISO8601)
    last_active_at: SystemTime,     // updated on each successful PostToConnection (and activity)
    identity: Identity {            // mirrors the modeled Identity shape
        source_ip: String,
        user_agent: String,
    },
    messages: Vec<Vec<u8>>,         // Data blobs delivered by PostToConnection, in send order
}
```

Deletion removes the map entry outright; a missing key is the single source of truth for `GoneException`. Nothing else in the service is in scope for this first slice — there are no other modeled operations.

## Evidence

`services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/OPERATIONS.md` lists exactly 3 operations with their routes, members and errors: DeleteConnection (`DELETE /@connections/{ConnectionId} -> 204`), GetConnection (`GET /@connections/{ConnectionId} -> 200`, output `ConnectedAt`/`Identity`/`LastActiveAt`), PostToConnection (`POST /@connections/{ConnectionId} -> 200`, input `Data` blob len 0..131072 @httpPayload). Each declares `ForbiddenException (403)`, `GoneException (410)`, `LimitExceededException (429)`; PostToConnection adds `PayloadTooLargeException (413)`. All three handler files (`src/ops/get_connection.rs:4`, `src/ops/post_to_connection.rs`, `src/ops/delete_connection.rs`) carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and end in `Err(crate::scaffolded::Scaffolded::not_implemented(..))` — e.g. `get_connection.rs:158`. `src/state.rs:8` is an empty struct with a TODO, so no state exists to compute any response from. GoneException's own model doc ("The connection with the provided id no longer exists.") grounds the delete-then-410 requirement.

## Steps to Reproduce

1. Register/seed a connection id (e.g. `abc123`) in the emulator's connection registry via the bootstrap registration seam, with a SourceIp and UserAgent.
2. Call `GetConnection(ConnectionId="abc123")` → expect HTTP 200 with `ConnectedAt`, `LastActiveAt` and `Identity.SourceIp`/`Identity.UserAgent` matching what was registered. Today: HTTP 501 NotImplemented.
3. Call `PostToConnection(ConnectionId="abc123", Data=b"hello")` → expect HTTP 200 with empty body, the blob recorded against the connection, and `LastActiveAt` advanced. Today: HTTP 501.
4. Call `GetConnection(ConnectionId="abc123")` again → expect `LastActiveAt` to be >= the time of the PostToConnection and `ConnectedAt` unchanged (proving state, not a fresh `now()`).
5. Call `PostToConnection(ConnectionId="abc123", Data=<131073 bytes>)` → expect `PayloadTooLargeException` (HTTP 413).
6. Call `DeleteConnection(ConnectionId="abc123")` → expect HTTP 204 with no body. Today: HTTP 501.
7. Call `GetConnection(ConnectionId="abc123")` and `PostToConnection(ConnectionId="abc123", Data=b"hi")` → both must now return `GoneException` (HTTP 410).
8. Call `GetConnection(ConnectionId="never-existed")` → expect `GoneException` (HTTP 410), not 200 and not 501.
