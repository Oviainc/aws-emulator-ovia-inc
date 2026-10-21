Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodeStar Connections emulator

_Filed: 2026-10-21T21:38:03Z_
_Operation: service-com-amazonaws-codestarconnections-codestar-connections-20191201 | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 27 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST minimal slice: the connection lifecycle, which is the service's primary resource and the one CodePipeline/CloudFormation users touch.

Happy path to implement, in order (wire protocol: awsJson1_0, `POST /` with `X-Amz-Target: CodeStar_connections_20191201.<OperationName>`):

1. **CreateConnection** — input `ConnectionName` (required, 1..32), `ProviderType` (Bitbucket|GitHub|GitHubEnterpriseServer|GitLab|GitLabSelfManaged), optional `HostArn`, optional `Tags`. Must mint and persist a real connection: generate a UUID connection id and return `ConnectionArn` of the form `arn:aws:codestar-connections:<region>:<account>:connection/<uuid>` (must satisfy the modeled pattern `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`), echo back `Tags` as stored. Per the modeled docs, a connection created through the CLI/SDK/CloudFormation starts in `PENDING` status — store `ConnectionStatus = PENDING`, not `AVAILABLE`. Record `OwnerAccountId` as the caller's 12-digit account id.
2. **GetConnection** — input `ConnectionArn` (required). Must look the stored connection up by ARN and return the full `Connection` structure reconstructed from state: `ConnectionName`, `ConnectionArn`, `ProviderType`, `OwnerAccountId`, `ConnectionStatus`, `HostArn` (only when one was supplied at create). Unknown ARN -> `ResourceNotFoundException` (HTTP 404).
3. **ListConnections** — input `MaxResults` (0..100), `NextToken`, optional `ProviderTypeFilter` and `HostArnFilter`. Must return every connection currently in state as `Connections` (list<Connection>), applying both filters, honouring `MaxResults` as a real page size and emitting/consuming an opaque `NextToken` so paging through N connections yields each exactly once.
4. **TagResource** / **ListTagsForResource** — `TagResource` takes `ResourceArn` + `Tags` (list of `Tag{Key,Value}`) and must merge them onto the stored connection's tag map (same key overwrites); `ListTagsForResource` must return exactly the tags held in state for that ARN. Unknown ARN -> `ResourceNotFoundException` (404) for both.
5. **DeleteConnection** — input `ConnectionArn` (required). Must remove the connection (and its tags) from state and return an empty body. A subsequent `GetConnection` on that ARN must then fail with `ResourceNotFoundException` (404), and `ListConnections` must no longer include it.

Every one of these must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)` per README.md). No canned responses, no echoing the input back without storing it, no minting an ARN that is never recorded — a create followed by a get/list must return what was created.

## State Model

In `src/state.rs`, on `ComAmazonawsCodestarconnectionsCodestarConnections20191201State`:

- `connections: BTreeMap<String /* ConnectionArn */, Connection>` where `Connection { connection_id: String (uuid), connection_name: String, connection_arn: String, provider_type: ProviderType, owner_account_id: String (12 digits), connection_status: ConnectionStatus /* PENDING|AVAILABLE|ERROR */, host_arn: Option<String>, created_at: SystemTime }`. Keyed by ARN for O(1) Get/Delete; `created_at` gives ListConnections a stable deterministic order.
- `tags: BTreeMap<String /* ResourceArn */, BTreeMap<String, String>>` — the tag map shared by TagResource / UntagResource / ListTagsForResource and populated from `CreateConnection.Tags`.
- `account_id: String` and `region: String` (from the request context) used to build ARNs consistently so a client-constructed ARN matches a stored one.

Out of scope for this first slice (these stay 501 and are filed separately): hosts (CreateHost/GetHost/UpdateHost/DeleteHost/ListHosts), repository links (CreateRepositoryLink/GetRepositoryLink/UpdateRepositoryLink/DeleteRepositoryLink/ListRepositoryLinks), and all sync-configuration / sync-blocker / sync-status operations.

## Evidence

`services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/OPERATIONS.md` lists all 27 operations with their input/output members and errors: CreateConnection (ConnectionName required, output ConnectionArn required + Tags), GetConnection (ConnectionArn required -> Connection, ResourceNotFoundException 404), ListConnections (@paginated inputToken=NextToken/outputToken=NextToken, MaxResults range 0..100), TagResource/ListTagsForResource/UntagResource (ResourceArn + Tags, ResourceNotFoundException 404), DeleteConnection (ConnectionArn required, no output members, ResourceNotFoundException 404). The `Connection` and `Tag` shapes from the Smithy model give the exact fields (ConnectionName, ConnectionArn, ProviderType, OwnerAccountId, ConnectionStatus, HostArn; Tag{Key,Value} both required), and the `Connection` shape doc states a connection created through CloudFormation/CLI/SDK is in PENDING status by default. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 27 handler files (create_connection.rs, get_connection.rs, list_connections.rs, tag_resource.rs, list_tags_for_resource.rs, delete_connection.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with endpoint override and signing name `codestar-connections`.
2. CreateConnection with ConnectionName="my-github", ProviderType="GitHub", Tags=[{Key:"env",Value:"dev"}] — expect 200 with a ConnectionArn matching `arn:aws:codestar-connections:<region>:<account>:connection/<uuid>`; today returns HTTP 501 NotImplemented.
3. GetConnection with that ConnectionArn — expect 200 with Connection.ConnectionName="my-github", ProviderType="GitHub", ConnectionStatus="PENDING", OwnerAccountId = the 12-digit account id.
4. ListConnections with MaxResults=10 — expect the new connection present in Connections; repeat with ProviderTypeFilter="Bitbucket" and expect it absent.
5. ListTagsForResource with that ARN — expect Tags=[{Key:"env",Value:"dev"}]; then TagResource with [{Key:"team",Value:"infra"}] and re-list — expect both tags.
6. DeleteConnection with that ARN — expect 200 and an empty body.
7. GetConnection with the deleted ARN — expect HTTP 404 ResourceNotFoundException; ListConnections must no longer return it.
