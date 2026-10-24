Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Graph records never report kmsKeyIdentifier for an AWS-owned key, and buildNumber is hardcoded to absent

_Filed: 2026-10-24T07:15:40Z_
_Operation: GetGraph | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every graph-record response (CreateGraph, GetGraph, UpdateGraph, StartGraph, StopGraph, DeleteGraph, and the GraphSummary in ListGraphs) is rendered from one shared projection in `crate::state`, and two modeled output members can never be populated by any code path:

1. `kmsKeyIdentifier` — the emulator stores exactly what the caller passed to CreateGraph and reports `None` when nothing was passed, so a graph created without a customer KMS key answers with the member omitted. AWS always reports the encryption key for the graph: when no customer managed key is supplied, the graph is encrypted with an AWS-owned key and the member is reported as the sentinel value `AWS_OWNED_KEY` (note that the model types `GraphSummary.kmsKeyIdentifier` as a plain `String` with doc 'The ID of the KMS key used to encrypt and decrypt graph data', not as the ARN-patterned `KmsKeyArn`, precisely because of that sentinel). A client that reads `kmsKeyIdentifier` to decide whether a graph is CMK-encrypted sees a missing field instead of the sentinel.

2. `buildNumber` — hardcoded to `None` at the create site (create_graph.rs:295-299: `build_number: None`) and never set anywhere else, so no response in the service ever carries the member. AWS reports the graph software build number for a provisioned graph.

Expected: a GetGraph of a graph created with no `kmsKeyIdentifier` returns `kmsKeyIdentifier: "AWS_OWNED_KEY"` and a non-empty `buildNumber`. Actual: both members are absent from every graph response the emulator produces.

## Evidence

Spec: `smithy_query(query="operation",operation="GetGraph")` declares `kmsKeyIdentifier` and `buildNumber` on GetGraphOutput (and on CreateGraph/UpdateGraph/Start/Stop/DeleteGraph outputs); `GraphSummary.kmsKeyIdentifier` is target `String` — deliberately unconstrained, unlike the `KmsKeyArn` used for the CreateGraph INPUT member — documented as 'The ID of the KMS key used to encrypt and decrypt graph data', and `buildNumber` is documented as 'The build number of the graph software'. Neptune Analytics encrypts every graph (customer key or AWS-owned key), so the key member is never genuinely unknown.

Handler: src/state.rs:682-735 — the shared `graph_record_output!` macro and `From<&Graph> for GraphSummary` copy `kms_key_identifier: graph.kms_key_identifier.clone()` and `build_number: graph.build_number.clone()`; src/ops/create_graph.rs:293-299 stores `kms_key_identifier: input.kms_key_identifier` (so `None` whenever the caller omitted it) and `build_number: None` with the comment 'buildNumber is the version of the graph software the real service runs, which this emulator does not have'. No other write site exists (grep for `build_number` in the crate yields only these).

Test coverage: tests/all/graph_lifecycle.rs:126-284 ('create_graph_persists_every_supplied_member_and_defaults_the_rest') asserts the SUPPLIED members round-trip; no test asserts an AWS-owned-key sentinel or a build number, so neither field is covered.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName='kms-default-probe', provisionedMemory=16) with no kmsKeyIdentifier; poll GetGraph until status AVAILABLE.
3. Inspect the GetGraph response body: observe no `kmsKeyIdentifier` member and no `buildNumber` member; expected `kmsKeyIdentifier` = 'AWS_OWNED_KEY' and a populated `buildNumber`.
4. Call ListGraphs and inspect the GraphSummary for the same graph: observe the same two members missing.
