Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: 32a885f55a837ab4195aa343368d45f721137954

# CodeConnections CreateConnection refuses every HostArn instead of resolving one

_Filed: 2026-10-22T00:00:00Z_
_Operation: CreateConnection (service-com-amazonaws-codeconnections-codeconnections-20231201) | Class: missing_

## Description

`CreateConnection` accepts an optional `HostArn` naming the host a connection is
associated with. A host only exists once `CreateHost` minted it, and every host
operation in this crate (`CreateHost`, `GetHost`, `UpdateHost`, `DeleteHost`,
`ListHosts`) is still an unmodified scaffold answering HTTP 501, so the service holds
no host at all.

`services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/create_connection.rs`
therefore refuses *any* request carrying `HostArn` with the modelled
`ResourceUnavailableException` (400, "Resource not found. Verify the ARN for the host
resource and try again."). That is the right answer today — no host with that ARN
exists — but the refusal is keyed on the member being present rather than on a host
lookup, so it will keep refusing once hosts are real.

Consequences while this stands:

- A host-backed provider (`GitHubEnterpriseServer`, `GitLabSelfManaged`) cannot be
  given its host, so `Connection.HostArn` is never populated and
  `ListConnections`'s `HostArnFilter` always selects nothing.
- `StoredConnection.host_arn` in `src/state.rs` is always `None`.

## What the fix looks like

When the host operations are implemented (their own bootstrap slice), add the host
store to `ComAmazonawsCodeconnectionsCodeconnections20231201State` and replace the
presence check in `create_connection.rs` with a lookup:

- `HostArn` present and the host exists ⇒ store it on the connection and report it
  from `GetConnection` / `ListConnections`, and let `HostArnFilter` select by it.
- `HostArn` present and the host does not exist ⇒ keep the existing
  `ResourceUnavailableException` (400).
- `HostArn` absent ⇒ unchanged.

## Evidence

`create_connection.rs` returns `ResourceUnavailableException` for every
`input.host_arn.is_some()` (the comment at that site records why), and
`services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/create_host.rs:4`
still carries `//! SCAFFOLDED, NOT IMPLEMENTED:`. The test
`a_host_arn_that_names_no_host_is_refused` in
`tests/all/connection_lifecycle.rs` pins the current behaviour, and
`list_filters_select_from_the_stored_set` pins that `HostArnFilter` selects nothing.

## Steps to Reproduce

1. `CreateConnection` with `ConnectionName='hosted'`,
   `ProviderType='GitHubEnterpriseServer'` and a `HostArn` matching the modelled
   pattern.
2. Expected once hosts exist: 200 with the connection associated with that host, and
   `GetConnection` reporting `HostArn`.
3. Actual: 400 `ResourceUnavailableException`, for every `HostArn`.
