Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Amazon Neptune Graph (neptune-graph) emulator

_Filed: 2026-10-21T22:07:57Z_
_Operation: service-com-amazonaws-neptunegraph-amazonneptunegraph | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 34 handlers in `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything against this service. Implement a FIRST, SMALL slice: the graph-resource lifecycle, statefully, over the `restJson1` protocol (sigv4 signing name `neptune-graph`).

Happy path to implement, in order:

1. **CreateGraph** — `POST /graphs -> 201`. Required input: `graphName` (1..63, pattern `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`), `provisionedMemory` (16..24576). Optional: `tags`, `publicConnectivity`, `kmsKeyIdentifier`, `vectorSearchConfiguration`, `replicaCount` (0..2), `deletionProtection`. Must mint a fresh `id` matching `^g-[a-z0-9]{10}$`, build `arn` (`arn:aws:neptune-graph:<region>:<account>:graph/<id>`), set `createTime`, synthesize an `endpoint`, store every supplied field, and return them in the response. Duplicate `graphName` -> `ConflictException` (409). Values outside the modeled range/pattern -> `ValidationException` (400).
2. **GetGraph** — `GET /graphs/{graphIdentifier} -> 200`. Must look the graph up by the id minted in step 1 and return the PERSISTED record (`id`, `name`, `arn`, `status`, `createTime`, `provisionedMemory`, `endpoint`, `publicConnectivity`, `replicaCount`, `kmsKeyIdentifier`, `deletionProtection`, `vectorSearchConfiguration`). Unknown id -> `ResourceNotFoundException` (404).
3. **ListGraphs** — `GET /graphs -> 200`. Must return one `GraphSummary` per stored graph in `graphs` (required member), honoring `maxResults` (1..100) and round-tripping `nextToken` (1..8192) per the `@paginated(inputToken=nextToken, outputToken=nextToken, items=graphs)` trait: `nextToken` present only when more graphs remain, and resuming from it must return the remainder without repeats.
4. **UpdateGraph** — `PATCH /graphs/{graphIdentifier} -> 200`. Must mutate only the supplied members (`publicConnectivity`, `provisionedMemory`, `deletionProtection`) on the stored record and return the full updated graph; a later GetGraph must show the new values. Unknown id -> `ResourceNotFoundException` (404).
5. **DeleteGraph** — `DELETE /graphs/{graphIdentifier} -> 200`, required query param `skipSnapshot`. Must remove the graph from the store and return the final graph record (with `status` reflecting deletion). A graph with `deletionProtection=true` -> `ConflictException` (409). A second delete of the same id -> `ResourceNotFoundException` (404), and GetGraph/ListGraphs must no longer show it.

Everything else (snapshots, import/export tasks, private graph endpoints, ExecuteQuery, Start/Stop/ResetGraph, tagging) stays 501 for now and will be filed as individual gap issues.

NEVER STUBS: each of these five handlers must compute its response from the shared in-memory store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). No canned bodies, no echoing the input back without storing it, no minting an id that is never persisted.

## State Model

In `src/state.rs` (`ComAmazonawsNeptunegraphAmazonneptunegraphState`):

- `graphs: BTreeMap<String /* graph id, "g-xxxxxxxxxx" */, Graph>` — BTreeMap so ListGraphs/pagination has a stable total order.
- `graph_names: HashMap<String /* graphName */, String /* graph id */>` — uniqueness index backing CreateGraph's ConflictException.
- `struct Graph { id: String, name: String, arn: String, status: GraphStatus /* AVAILABLE after create */, status_reason: Option<String>, create_time: SystemTime, provisioned_memory: i32, endpoint: String, public_connectivity: bool, replica_count: i32, kms_key_identifier: Option<String>, vector_search_configuration: Option<VectorSearchConfiguration>, deletion_protection: bool, source_snapshot_id: Option<String>, build_number: Option<String>, tags: BTreeMap<String, String> }`
- `id_counter: u64` — feeds the deterministic `g-[a-z0-9]{10}` id generator so ids are unique per store.

Done = a realistic integration script (create -> get -> list -> update -> get -> delete -> get) passes with correct, stateful bodies, not merely 2xx.

## Evidence

OPERATIONS.md (crate root) lists all 34 operations with their routes, members and errors: CreateGraph `POST /graphs -> 201` (required `graphName`, `provisionedMemory`; errors ConflictException 409, ValidationException 400, ServiceQuotaExceededException 402); GetGraph `GET /graphs/{graphIdentifier} -> 200` @readonly (ResourceNotFoundException 404); ListGraphs `GET /graphs -> 200` @paginated(inputToken=nextToken, outputToken=nextToken, items=graphs) with `maxResults` range 1..100; UpdateGraph `PATCH /graphs/{graphIdentifier} -> 200`; DeleteGraph `DELETE /graphs/{graphIdentifier} -> 200` with required `skipSnapshot` query param and ConflictException 409 / ResourceNotFoundException 404. The id patterns `^g-[a-z0-9]{10}$` and name pattern `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$` are modeled constraints. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/` matches all 34 handler files, including create_graph.rs, get_graph.rs, list_graphs.rs, update_graph.rs and delete_graph.rs — no behavior exists and no test asserts any of it.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI neptune-graph client at it.
2. `aws neptune-graph create-graph --graph-name my-graph --provisioned-memory 16 --no-deletion-protection` — expect 201 with a fresh `id` matching ^g-[a-z0-9]{10}$, an `arn`, `createTime` and the echoed-but-persisted `provisionedMemory`; today returns HTTP 501 NotImplemented.
3. `aws neptune-graph get-graph --graph-identifier <id>` — expect 200 with exactly the graph created in the previous step (name, arn, provisionedMemory, endpoint); today 501.
4. `aws neptune-graph list-graphs --max-results 1` with two graphs created — expect one GraphSummary plus a `nextToken`, and resuming with that token returns the second graph only; today 501.
5. `aws neptune-graph update-graph --graph-identifier <id> --provisioned-memory 32` then get-graph — expect provisionedMemory 32 persisted; today 501.
6. `aws neptune-graph delete-graph --graph-identifier <id> --skip-snapshot` — expect 200 with the final graph record, then get-graph on the same id returns ResourceNotFoundException (404) and list-graphs omits it; today 501.
