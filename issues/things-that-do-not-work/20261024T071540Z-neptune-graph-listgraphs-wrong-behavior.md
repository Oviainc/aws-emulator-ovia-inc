Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Graphs and snapshots are stored without a Region, so ListGraphs/GetGraph leak resources created in another Region

_Filed: 2026-10-24T07:15:40Z_
_Operation: ListGraphs | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

neptune-graph is a regional control plane: a graph (and a graph snapshot) exists in exactly one Region, and a request signed for a different Region must not see it. The emulator's store has no notion of the Region a record belongs to, so every graph and snapshot is visible from every Region.

Expected (AWS):
- CreateGraph signed for us-east-1, then ListGraphs signed for eu-west-1 -> `graphs: []` (and the us-east-1 graph is absent).
- GetGraph(<id of a us-east-1 graph>) signed for eu-west-1 -> 404 ResourceNotFoundException.
- CreateGraph(graphName='x') in us-east-1 and CreateGraph(graphName='x') in eu-west-1 both succeed (201): graph names are unique per account PER REGION.

Actual (emulator): one global `graphs`/`snapshots` map in `crate::state`. `ListGraphs` pages over the whole map with no Region predicate (list_graphs.rs:184-191), `GetGraph`/`GetGraphSnapshot`/`UpdateGraph`/`StartGraph`/`StopGraph`/`DeleteGraph` resolve by bare id (`settled_graph`/`settled_graph_mut`, state.rs:289-301) regardless of the signing Region, and the `graph_names` uniqueness index is global so the second CreateGraph of the same name from another Region is refused with ConflictException (409) instead of creating a second graph.

The request's Region is already available to handlers (`crate::state::request_region(ctx)`, state.rs:469) and is used to build the ARN and the endpoint hostname at create time (create_graph.rs:218, 270-275), but it is never stored on the record and never used as a lookup/listing predicate. The fix is to carry the creating Region on `state::Graph`/`state::Snapshot` and scope every lookup, listing and name-uniqueness check by the Region of the current request (many other crates in this repo already do this, e.g. services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:506).

## Evidence

Spec/semantics: the Smithy model gives this service `aws.api#service` with arnNamespace/endpointPrefix `neptune-graph` and regional endpoints (`smithy_query(query="service_info")`); every graph ARN the emulator mints embeds the request Region (`::engine::emulator_arn(ARN_NAMESPACE, &region, format!("graph/{id}"))`, create_graph.rs:269-275), and AWS control-plane resources are only visible within the Region that holds them — ListGraphs in another Region cannot return them and GetGraph on them answers ResourceNotFoundException.

Handler: services/.../src/ops/list_graphs.rs:172-200 (settle_all + range over the whole BTreeMap, no Region filter); src/state.rs:223-301 (`graphs`, `snapshots`, `graph_names`, `snapshot_names` are global maps; `struct Graph` and `struct Snapshot` carry no region member); src/ops/get_graph.rs:169-180, update_graph.rs:180-187, start_graph.rs:177-186, stop_graph.rs:176-185, delete_graph.rs:177-184, create_graph.rs:238-247 all key off the bare id/name.

Test coverage: tests/all/graph_lifecycle.rs (create/get/list/update/delete), graph_run_state.rs and graph_snapshot_lifecycle.rs all drive a single signing Region; no test asserts that a graph created in one Region is invisible in another. state.rs's own ARN tests (state.rs:979-993, 1215) assert only that a TAG ARN naming another Region matches no resource — the graph/snapshot id lookups and ListGraphs have no such scoping.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client signed for us-east-1 at it.
2. CreateGraph(graphName='region-scope-probe', provisionedMemory=16); poll GetGraph until status AVAILABLE. Note the returned arn contains ':us-east-1:'.
3. Point a second client at the same emulator with credentials signed for eu-west-1 and call ListGraphs: observe the us-east-1 graph in `graphs`; expected an empty list.
4. With the eu-west-1 client call GetGraph(graphIdentifier=<id from step 2>): observe HTTP 200 with the graph record; expected 404 ResourceNotFoundException.
5. With the eu-west-1 client call CreateGraph(graphName='region-scope-probe', provisionedMemory=16): observe 409 ConflictException ('a graph named ... already exists'); expected 201, because graph names are unique per Region.
6. Repeat steps 2-5 with CreateGraphSnapshot/GetGraphSnapshot for the snapshot store (same global maps).
