Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodeConnections emulator

_Filed: 2026-10-21T21:38:05Z_
_Operation: service-com-amazonaws-codeconnections-codeconnections-20231201 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 27 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL slice: the stateful lifecycle of the primary resource, a *connection*, so a realistic client script works end to end. Protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: CodeConnections_20231201.<OperationName>` (see README/OPERATIONS.md).

Happy path to implement, in order:

1. **CreateConnection** — input `ConnectionName` (required, len 1..32), `ProviderType` (one of AzureDevOps|Bitbucket|GitHub|GitHubEnterpriseServer|GitLab|GitLabSelfManaged), optional `Tags` (list<Tag{Key,Value}>), optional `HostArn`. Mint and PERSIST a connection: generate `ConnectionArn` of the form `arn:aws:codeconnections:<region>:<account-id>:connection/<uuid>` (must satisfy the modeled pattern `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`), set `ConnectionStatus = PENDING` (the modeled enum is PENDING|AVAILABLE|ERROR, and a connection created via API starts PENDING), set `OwnerAccountId` to the caller's 12-digit account. Return `ConnectionArn` (required) and the `Tags` as stored.
2. **GetConnection** — input `ConnectionArn` (required). Look the ARN up in state and return the `Connection` structure populated from what was stored: `ConnectionName`, `ConnectionArn`, `ProviderType`, `OwnerAccountId`, `ConnectionStatus`, `HostArn`. Unknown/deleted ARN must return the modeled `ResourceNotFoundException` (HTTP 404), not a 501 and not an empty 200.
3. **ListConnections** — returns `Connections` (list<Connection>) built by iterating stored connections, honouring `MaxResults` (range 0..100) and the `NextToken` pagination contract (`@paginated(inputToken=NextToken, outputToken=NextToken)`), plus the `ProviderTypeFilter` and `HostArnFilter` filters. Emit `NextToken` only when more items remain, and resuming with that token must continue where the page ended without duplicating or dropping entries.
4. **ListTagsForResource** — input `ResourceArn` (required). Return the `Tags` that were supplied at CreateConnection for that ARN (from state); unknown ARN → `ResourceNotFoundException` (404).
5. **DeleteConnection** — input `ConnectionArn` (required). Remove the connection from state and return an empty success body. A subsequent GetConnection for that ARN must return `ResourceNotFoundException` (404) and ListConnections must no longer include it. Deleting an unknown ARN → `ResourceNotFoundException`.

Every one of these must compute its response from persisted state held in the single shared engine (reached via `crate::state::service_state(&*self.engine)`). No canned responses, no echoing the input back, no minting an ARN that is never stored. Required output members must be populated from the stored record, and a create followed by a get/list must return exactly what was created.

## State Model

In `src/state.rs`, on the service state struct:

- `connections: BTreeMap<String /* ConnectionArn */, Connection>` — insertion/sort order is what ListConnections paginates over deterministically.
- `Connection { connection_name: String, connection_arn: String, provider_type: Option<ProviderType>, owner_account_id: String /* 12 digits */, connection_status: ConnectionStatus /* PENDING on create */, host_arn: Option<String>, tags: Vec<Tag{key,value}> }`
- A secondary `connection_names: BTreeSet<String>` (or a scan) to enforce that connection names are unique within the account, as the model documents for `Connection.ConnectionName`.
- Pagination tokens are derived from a position in the `connections` key order (e.g. the last returned ARN, opaquely encoded), so a token issued by one ListConnections call resumes correctly on the next.

Out of scope for this first slice (stay 501, filed separately later): hosts (CreateHost/GetHost/UpdateHost/DeleteHost/ListHosts), repository links, sync configurations, sync status and sync blockers, and TagResource/UntagResource.

Done = an integration script that creates a connection with tags, gets it back (name/provider/status/owner match, status PENDING), lists it (paging with MaxResults=1), lists its tags, deletes it, and then observes ResourceNotFoundException on get and absence from list — all with correct bodies, not just 2xx.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/` matches all 27 operation files (create_connection.rs, get_connection.rs, list_connections.rs, list_tags_for_resource.rs, delete_connection.rs, …), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, and the crate holds no state. Contracts cited from the crate's OPERATIONS.md: CreateConnection (input ConnectionName required len 1..32, ProviderType enum, Tags, HostArn; output ConnectionArn required, Tags; errors LimitExceededException 429 / ResourceNotFoundException 404 / ResourceUnavailableException 400), GetConnection (input ConnectionArn required; output Connection; ResourceNotFoundException 404), ListConnections (@paginated NextToken/NextToken, MaxResults range 0..100, ProviderTypeFilter, HostArnFilter; output Connections + NextToken), ListTagsForResource (ResourceArn required; output Tags; ResourceNotFoundException), DeleteConnection (ConnectionArn required; empty output; ResourceNotFoundException). The `Connection` shape members (ConnectionName, ConnectionArn, ProviderType, OwnerAccountId, ConnectionStatus, HostArn) and `ConnectionStatus` values (PENDING|AVAILABLE|ERROR, with the shape doc stating an API/CLI/SDK-created connection is PENDING by default) come from smithy_query on the model at smithy-model/com-amazonaws-codeconnections-codeconnections-20231201.json. No tests assert any of this behaviour today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `codeconnections` at it.
2. CreateConnection with ConnectionName='my-conn', ProviderType='GitHub', Tags=[{Key:'env',Value:'test'}] — expect 200 with a ConnectionArn matching ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$; actual: HTTP 501 NotImplemented.
3. GetConnection with that ConnectionArn — expect Connection{ConnectionName:'my-conn', ProviderType:'GitHub', ConnectionStatus:'PENDING', OwnerAccountId:<12 digits>, ConnectionArn:<same arn>}; actual: 501.
4. ListConnections with MaxResults=1 — expect the connection in Connections and a NextToken only if more exist; re-call with the token to confirm paging continues; actual: 501.
5. ListTagsForResource with ResourceArn=<ConnectionArn> — expect Tags=[{Key:'env',Value:'test'}]; actual: 501.
6. DeleteConnection with that ConnectionArn — expect empty 200; then GetConnection on the same ARN must return ResourceNotFoundException (404) and ListConnections must not include it; actual: 501 throughout.
