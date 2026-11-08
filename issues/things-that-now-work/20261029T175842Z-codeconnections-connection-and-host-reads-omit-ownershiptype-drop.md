Issue-Source: trace-sweep-divergence
Issue-Trace: codeconnections-connection-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeconnections-connection-lifecycle codeconnections-list-filters-and-errors codeconnections-host-delete-blocked-by-connection codeconnections-host-lifecycle
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections|GetConnection|2xx->2xx|$.Connection.OwnershipType
Issue-Key: codeconnections|ListConnections|2xx->2xx|$.Connections[*]
Issue-Key: codeconnections|GetConnection|2xx->2xx|$.Connection.ProviderType
Issue-Key: codeconnections|GetHost|2xx->2xx|$.ProviderEndpoint
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# codeconnections connection and host reads omit OwnershipType, drop the host-derived ProviderType and serve a stale endpoint

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
SHARED FAULT (4 causes): the connection and host read paths report less than the resource actually is — a member the service always emits is never serialized, a member that must be derived from the connection's host is dropped, and a host's updated endpoint is never reflected. All four are 200 responses whose payload diverges, so a client sees a successful read of wrong resource state.

Cause 1 of 4 — GetConnection omits Connection.OwnershipType (fixture codeconnections-connection-lifecycle, step task#2).
AWS: 200 with Connection.OwnershipType present; for a connection created in the caller's own account the value is "Owned" (recorded behavior only — the in-repo model version 2023-12-01 declares Connection with ConnectionName/ConnectionArn/ProviderType/OwnerAccountId/ConnectionStatus/HostArn and no OwnershipType member, so the serializer has to emit it from the record rather than from the modeled shape).
Emulator: 200 with OwnershipType absent from the Connection structure (every other member matches).
Why: a client that reads OwnershipType to tell an owned connection from one shared into the account gets no value at all, so "is this connection mine?" cannot be answered and consumers that treat the field as required fail to deserialize.
Fix: the Connection projection used by the connection reads — the record-to-modeled conversion in services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/state.rs (`to_modeled`, called from src/ops/get_connection.rs) must carry an ownership type, defaulted to Owned for a connection created by the caller's account.
Traces: codeconnections-connection-lifecycle#task#2.

Cause 2 of 4 — ListConnections entries omit the same OwnershipType (fixture codeconnections-connection-lifecycle, step task#3).
AWS: 200, each Connections[i] carries OwnershipType ("Owned" for an account-owned connection) alongside ConnectionName, ConnectionArn, ProviderType, OwnerAccountId and ConnectionStatus (recorded behavior only, as above).
Emulator: 200, Connections[0] has every other member and no OwnershipType, so the listed element does not equal the recorded element.
Why: listing-driven clients (console-style inventories, reconcilers) cannot classify ownership for any connection they enumerate, and element-by-element comparisons against AWS fail.
Fix: same Connection projection, shared by the listing — fixing cause 1 in the shared conversion fixes this read too, but it must be re-replayed separately because the listing path also re-serializes the element.
Traces: codeconnections-connection-lifecycle#task#3 (also codeconnections-list-filters-and-errors#task#3).

Cause 3 of 4 — GetConnection on a host-based connection drops BOTH OwnershipType and ProviderType (fixture codeconnections-host-delete-blocked-by-connection, step task#3).
AWS: 200 with OwnershipType "Owned" and ProviderType resolved from the host the connection was created on — a connection created with HostArn only (no ProviderType in the request) still reports the host's provider type, e.g. GitHubEnterpriseServer (Smithy: CreateConnection takes ProviderType OR HostArn, Connection.ProviderType is "the name of the external provider where your third-party code repository is configured"; the derivation is recorded behavior).
Emulator: 200 with both members absent — ProviderType is null because the create request did not state one and the host's provider type was never copied onto the connection record.
Why: a client cannot tell what provider a host-based connection talks to, so provider-specific branching (and CodePipeline-style source configuration) has nothing to key on even though the read succeeded.
Fix: the CreateConnection handler/state must derive and persist the connection's ProviderType from the referenced host when the request names only HostArn, and the Connection projection must emit it (src/ops/create_connection.rs + src/state.rs).
Traces: codeconnections-host-delete-blocked-by-connection#task#3.

Cause 4 of 4 — GetHost reports the host's pre-update ProviderEndpoint after UpdateHost (fixture codeconnections-host-lifecycle, step task#7).
AWS: 200 with ProviderEndpoint equal to the endpoint most recently applied by UpdateHost (Smithy: UpdateHost input ProviderEndpoint (Url, len 1..512) updates the host; GetHost output ProviderEndpoint reports it).
Emulator: 200 with the endpoint the host was created with — the updated value never appears, because UpdateHost is still an unimplemented scaffold (src/ops/update_host.rs returns `Scaffolded::not_implemented("UpdateHost")`), so no host mutation is ever persisted. This cause is the read-side evidence of that gap and is filed because the replay reported it as its own divergence cluster with no owning issue.
Why: a client that updates a host's endpoint and then reads the host back sees the stale endpoint and concludes the update was lost (or, worse, keeps routing to the old provider endpoint).
Fix: implement the host update so ProviderEndpoint (and VpcConfiguration) are written to the stored host record, which GetHost then renders — src/ops/update_host.rs plus the host record in src/state.rs.
Traces: codeconnections-host-lifecycle#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (all four are HTTP 200 vs 200 payload divergences):
- GetConnection (codeconnections-connection-lifecycle#task#2): $.Connection.OwnershipType missing — expected 'Owned', got None.
- ListConnections (codeconnections-connection-lifecycle#task#3; cluster also covers codeconnections-list-filters-and-errors#task#3): $.Connections[0] differs from the recorded element only by the absent OwnershipType ('Owned'); ConnectionName, ConnectionArn, ProviderType, OwnerAccountId and ConnectionStatus all match.
- GetConnection (codeconnections-host-delete-blocked-by-connection#task#3): $.Connection.OwnershipType missing (expected 'Owned', got None) and $.Connection.ProviderType missing (expected 'GitHubEnterpriseServer', got None) for a connection created with HostArn only.
- GetHost (codeconnections-host-lifecycle#task#7): $.ProviderEndpoint expected the updated endpoint, got the endpoint the host was created with; src/ops/update_host.rs:162 returns not_implemented, so no host update is persisted.
Generated ARNs/uuids and host names in these fixtures are per-run values; the requirement is the presence and derivation of the members, not the recorded literals.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. codeconnections-connection-lifecycle: CreateConnection, then GetConnection (step task#2) and ListConnections (step task#3) — the returned Connection/Connections[0] have no OwnershipType.
3. codeconnections-host-delete-blocked-by-connection: CreateHost (GitHubEnterpriseServer), CreateConnection with only that HostArn, then GetConnection (step task#3) — ProviderType and OwnershipType are both absent.
4. codeconnections-host-lifecycle: CreateHost, UpdateHost with a new ProviderEndpoint, then GetHost (step task#7) — the endpoint reported is the original one.
