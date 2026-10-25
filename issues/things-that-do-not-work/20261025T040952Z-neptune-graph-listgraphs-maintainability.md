Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# Hoist the triplicated pagination-token codec into crate::state beside the id-space predicates it already hosts

_Filed: 2026-10-25T04:09:52Z_
_Operation: ListGraphs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination-token codec of this crate is written three times, once per implemented listing handler, instead of once. `encode_page_token` is byte-identical in all three copies (`hex::encode(x)`), and `decode_page_token` is identical in all three except for which `crate::state` id-space predicate it calls (`is_graph_id` / `is_snapshot_id` / `is_vpc_id`). The VARYING half of the rule is already centralised in `crate::state` — the three predicates live there, and `is_graph_id`'s own doc says it is "Shared by ... `ListGraphs`, which refuses a pagination token that does not name a position in the graph-id space" — while the INVARIANT half (hex-encode, hex-decode, UTF-8 decode, reject-on-predicate-failure, resume-from-a-position-not-a-row) is copied per handler. Each copy also carries its own identical `page_tokens_round_trip_and_reject_foreign_tokens` unit test.

Because the token WIRE FORMAT is one rule shared across the service's listings (the copies' own doc comments say so), three independent implementations can drift: a change to the encoding, to the UTF-8 handling, or to the "membership in the store is deliberately NOT required" decision applied to one copy silently leaves the other two on the old rule, and a client that pages one listing then another would see two different token grammars from one service. The two still-scaffolded listings (`ListExportTasks`, `ListImportTasks`) would add two more copies when implemented.

The fix is a single `crate::state` helper pair — encode(last_returned_key) and decode(token, id_space_predicate) — called by all three handlers. Every response byte and every existing test stays unchanged: the encoding, the refusal wording, the default page size and the resume semantics are all preserved; only the place the code lives changes.

## Evidence

Copy 1 — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_graphs.rs:207-222:
```
fn encode_page_token(last_returned_id: &str) -> String {
    hex::encode(last_returned_id)
}
...
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    crate::state::is_graph_id(&decoded).then_some(decoded)
}
```

Copy 2 — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_graph_snapshots.rs:239-254:
```
fn encode_page_token(last_returned_id: &str) -> String {
    hex::encode(last_returned_id)
}
...
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    crate::state::is_snapshot_id(&decoded).then_some(decoded)
}
```

Copy 3 — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_private_graph_endpoints.rs:248-263:
```
fn encode_page_token(last_returned_vpc_id: &str) -> String {
    hex::encode(last_returned_vpc_id)
}
...
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    crate::state::is_vpc_id(&decoded).then_some(decoded)
}
```

The crate's own comments state the codec is ONE shared rule:
- list_graph_snapshots.rs:236-238: "A pagination token naming the last snapshot id of the page just returned. Hex keeps the token opaque to a client and within `PaginationToken`'s 1..8192 length, the same encoding `ListGraphs` pages its own id space with."
- list_private_graph_endpoints.rs:244-247: "Hex keeps the token opaque to a client and within `PaginationToken`'s 1..8192 length, the same encoding the other listings of this service page their own id spaces with."

And the varying half is ALREADY hosted centrally, for exactly this caller — src/state.rs:2027-2052:
```
/// Whether `value` matches the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
///
/// Shared by the id generator's own test and by `ListGraphs`, which refuses a
/// pagination token that does not name a position in the graph-id space.
pub fn is_graph_id(value: &str) -> bool { is_resource_id(value, "g-") }

pub fn is_snapshot_id(value: &str) -> bool { is_resource_id(value, "gs-") }
...
pub fn is_vpc_id(value: &str) -> bool { ... }
```

Test coverage that must stay green and unchanged:
- unit: `page_tokens_round_trip_and_reject_foreign_tokens` in list_graphs.rs:231, list_graph_snapshots.rs:263, list_private_graph_endpoints.rs:272 (three near-identical copies of one test).
- integration: tests/all/graph_lifecycle.rs:489-594 (ListGraphs paging, foreign-token refusal at :559, unissued-position token at :574); tests/all/graph_snapshot_listing.rs:216-550; tests/all/private_graph_endpoint_listing.rs:239-580; tests/all/region_scoping.rs:392-404 (paging across Regions).

## Steps to Reproduce

1. Open services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_graphs.rs and read lines 203-222 (DEFAULT_PAGE_SIZE, encode_page_token, decode_page_token).
2. Open src/ops/list_graph_snapshots.rs lines 234-254 and src/ops/list_private_graph_endpoints.rs lines 242-263 and compare: `encode_page_token` is byte-identical in all three; `decode_page_token` differs only in the `crate::state::is_*_id` predicate it calls.
3. Confirm no shared helper exists: `grep -n 'page_token' services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs` returns nothing, while `grep -n 'is_graph_id\|is_snapshot_id\|is_vpc_id' src/state.rs` shows the varying half is already centralised there (state.rs:2031, 2036, 2046).
4. Note the three identical `#[cfg(test)] mod tests` blocks (list_graphs.rs:224-257, list_graph_snapshots.rs:256-..., list_private_graph_endpoints.rs:265-...) which also triplicate one test.
5. Change the encoding in one copy only (e.g. swap hex for another encoding in list_graphs.rs:208) and observe the other two listings keep issuing the old token grammar — the drift this duplication permits.
