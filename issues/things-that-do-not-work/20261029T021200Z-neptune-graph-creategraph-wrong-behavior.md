Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f3551fb51efeca87d6f65db01e533e63f3785794

# A graph created without kmsKeyIdentifier reports the non-ARN sentinel "AWS_OWNED_KEY" through output members the model patterns as a KMS key ARN

_Filed: 2026-10-29T02:12:00Z_
_Operation: CreateGraph | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f3551fb51efeca87d6f65db01e533e63f3785794`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation that mints a graph routes an absent `kmsKeyIdentifier` through `crate::state::graph_encryption_key`, which stores the literal sentinel `"AWS_OWNED_KEY"` on the graph record. That stored string is then reported verbatim through `kmsKeyIdentifier` on every projection of the record.

The constant's own doc-comment justifies the sentinel by asserting that the model permits a non-ARN there, and it cites exactly one shape to prove it: `GraphSummary$kmsKeyIdentifier`, which does target the prelude `String`. But `GraphSummary` is only the `ListGraphs` listing projection. The ten OTHER shapes the same stored value is reported through — `CreateGraphOutput`, `GetGraphOutput`, `UpdateGraphOutput`, `DeleteGraphOutput`, `ResetGraphOutput`, `StopGraphOutput`, `StartGraphOutput`, `RestoreGraphFromSnapshotOutput`, and (for a snapshot taken of such a graph) `CreateGraphSnapshotOutput`/`GetGraphSnapshotOutput`/`DeleteGraphSnapshotOutput`/`GraphSnapshotSummary` — all target `KmsKeyArn`, which the model constrains with `@length(1..1024)` and `@pattern("^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$")`. `"AWS_OWNED_KEY"` matches neither clause of that pattern.

So the comment's stated justification holds for exactly one of the eleven members the value travels through, and the ten remaining ones answer 200 carrying a value the model's own constraint on that member forbids. The sentinel is also exported publicly and copied onto snapshots (`CreateGraphSnapshot` sets `kms_key_identifier: graph.kms_key_identifier.clone()`, and `mint_final_snapshot` does the same), so a single unencrypted-by-customer `CreateGraph` propagates it across the snapshot surface too. The one projection the comment relies on is also the one place that would NOT have had to change.

## Evidence

The crate's justification, which names only the listing projection — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:93-103`:
```
/// The key a graph (and a snapshot of one) reports when its data is encrypted with a
/// key the service owns rather than one the caller named.
///
/// Every graph of this service is encrypted, so the reported key is never genuinely
/// unknown: a create request that names no `kmsKeyIdentifier` gets a service-owned key,
/// which has no ARN a caller may name, so the record reports this sentinel instead of
/// omitting the member. The model itself says the reported value need not be an ARN:
/// `GraphSummary$kmsKeyIdentifier` targets a plain `String`
/// (`smithy_query(query="shape",shape="GraphSummary")`) where every create INPUT member
/// targets the ARN-patterned `KmsKeyArn`.
pub const SERVICE_OWNED_KMS_KEY: &str = "AWS_OWNED_KEY";
```
and the helper every minting handler reaches — `src/state.rs:110-112`:
```
pub fn graph_encryption_key(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| SERVICE_OWNED_KMS_KEY.to_string())
}
```
called at `src/ops/create_graph.rs:309` (`kms_key_identifier: crate::state::graph_encryption_key(input.kms_key_identifier),`) and `src/ops/create_graph_using_import_task.rs:427`.

The model contradicts the quoted justification for the OUTPUT members the value is actually reported through. `smithy_query(query="shape",shape="GetGraphOutput")` ->
```
"kmsKeyIdentifier": {"target": "KmsKeyArn", "length": {"min": 1, "max": 1024}, "pattern": "^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$", "doc": "<p>The ID of the KMS key used to encrypt and decrypt graph data.</p>"}
```
and `smithy_query(query="shape",shape="KmsKeyArn")` ->
```
{"type": "string", "length": {"min": 1, "max": 1024}, "pattern": "^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$"}
```
The same `KmsKeyArn` target (with the same pattern) is what `smithy_query(query="operation",operation="UpdateGraph,ResetGraph,StartGraph,StopGraph,CreateGraphSnapshot,DeleteGraphSnapshot")` reports for each of their output `kmsKeyIdentifier` members, and `smithy_query(query="shape",shape="GraphSnapshotSummary")` reports it for the `ListGraphSnapshots` entry. Only `smithy_query(query="shape",shape="GraphSummary")` gives `"kmsKeyIdentifier": {"target": "String"}` — the single member the comment cites.

The value reaches all of them through the shared projections: `src/state.rs:3259` inside `graph_record_output!` (`kms_key_identifier: Some(graph.kms_key_identifier.clone()),`), applied at `src/state.rs:3269-3278` to `CreateGraphOutput, GetGraphOutput, UpdateGraphOutput, DeleteGraphOutput, ResetGraphOutput, StopGraphOutput, StartGraphOutput, RestoreGraphFromSnapshotOutput`; and `src/state.rs:3318` inside `snapshot_record_output!`, applied at `src/state.rs:3325-3330` to `CreateGraphSnapshotOutput, GetGraphSnapshotOutput, DeleteGraphSnapshotOutput, GraphSnapshotSummary`.

Test coverage: `tests/all/graph_record_encryption.rs:201-206` pins the sentinel for the no-key row (`expected: SERVICE_OWNED_KMS_KEY`) and asserts it on `CreateGraph`, `GetGraph`, `UpdateGraph`, `StopGraph`, `StartGraph`, `DeleteGraph` and the `ListGraphs` summary (lines 232-316). The file's stated reason is "the sentinel that names the service-owned key (which has no ARN a caller could name)" (lines 9-12) — a reason that does not survive the quoted `@pattern` on the nine non-`GraphSummary` members it asserts against. The same file's `KEY_DEFAULT_PARTITION`/`KEY_OTHER_PARTITION`/`KEY_ID_WITHOUT_HYPHENS` rows (lines 33-43) are explicitly chosen to satisfy that very pattern, so the suite already knows the constraint it exempts one row from.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph with graphName='no-key-graph' and provisionedMemory=16, supplying NO kmsKeyIdentifier.
3. Observe the 201 response carries "kmsKeyIdentifier": "AWS_OWNED_KEY" — a value that does not match the `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$` pattern the model puts on `CreateGraphOutput$kmsKeyIdentifier` (target `KmsKeyArn`).
4. GET /graphs/{id} (GetGraph) — same non-ARN value on `GetGraphOutput$kmsKeyIdentifier`, likewise targeted at `KmsKeyArn`.
5. Wait out PROVISIONING_SPAN, then POST /snapshots (CreateGraphSnapshot) with snapshotName='no-key-snap' for that graph — the snapshot's `kmsKeyIdentifier` is the same sentinel, now on `CreateGraphSnapshotOutput$kmsKeyIdentifier` and on the `GraphSnapshotSummary` of GET /snapshots, both targeted at `KmsKeyArn`.
6. Contrast with GET /graphs (ListGraphs), whose `GraphSummary$kmsKeyIdentifier` targets the prelude `String` — the one projection the comment at src/state.rs:99-102 cites as proof the value need not be an ARN.
