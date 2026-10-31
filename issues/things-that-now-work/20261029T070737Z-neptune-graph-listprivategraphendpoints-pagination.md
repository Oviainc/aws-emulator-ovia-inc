Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3a384b4a12755a7fa8fedde9fe2fff5778558e98

# The private-endpoint cursor is not scoped to the graph it was minted for, so one graph's nextToken silently truncates (or empties) another graph's page

_Filed: 2026-10-29T07:07:37Z_
_Operation: ListPrivateGraphEndpoints | Class: pagination_

> _Note: this issue was identified against the code base at commit `3a384b4a12755a7fa8fedde9fe2fff5778558e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListPrivateGraphEndpoints is the ONE listing of this crate whose entries are ordered inside a PER-PARENT map: `private_endpoints` is a `BTreeMap<graph_id, BTreeMap<vpc_id, PrivateGraphEndpoint>>`, and `private_endpoints_of` ranges over `self.private_endpoints[graph_id]` alone. The cursor it mints names only a VPC id, and the gate it is checked by (`crate::state::decode_page_token` with `crate::state::is_vpc_id`) asks nothing but "is this a well-formed VPC id".

So a `nextToken` this service issued while paging graph A's endpoints is accepted verbatim when the same caller (or a retry/SDK paginator aimed at a different graph) sends it to graph B: `private_endpoints_of(graph_B, Some(vpc_from_A))` resumes at `Bound::Excluded(that vpc id)` inside graph B's OWN map and silently drops every endpoint of B ordered at or before it — a truncated page, or an empty page carrying no `nextToken`, reported as a successful end-of-listing. Nothing tells the caller the cursor belonged to another graph's listing, although the crate has a refusal (`unissued_page_token`) for exactly "a value this service never issued for the page being asked for".

The other four listings are immune for a structural reason the crate states about them: `snapshots_in` / `import_tasks_in` / `export_tasks_in` / `graphs_in` all `range(...)` over ONE global `BTreeMap` and apply the `of_graph` restriction as a FILTER after the range, so a token minted under one `graphIdentifier` filter still names a valid position in the same single total order. Only the private-endpoint listing has a separate order per parent, which is what makes its token meaningful in exactly one graph and meaningless — but still honoured — in every other.

## Evidence

The gate the crate says keeps one listing's token out of another's pages — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:2371-2385`:
```
/// The position a token resumes after, or `None` when it is not a token
/// [`encode_page_token`] minted for the id space `in_id_space` accepts.
///
/// `in_id_space` is the listing's own id-space predicate, one of the `is_*_id`
/// predicates below: a decoded value naming no position in it is refused rather than
/// treated as the start of the list, which is also what stops one listing's token from
/// paging another's id space.
...
pub fn decode_page_token(token: &str, in_id_space: impl Fn(&str) -> bool) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    in_id_space(&decoded).then_some(decoded)
}
```

What this listing hands it — `src/ops/list_private_graph_endpoints.rs:231-233`:
```
fn decode_page_token(token: &str) -> Option<String> {
    crate::state::decode_page_token(token, crate::state::is_vpc_id)
}
```
and `src/state.rs:2474-2481`, which bounds only the prefix and alphabet of a value another service mints:
```
pub fn is_vpc_id(value: &str) -> bool {
    value.strip_prefix("vpc-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}
```

The accepted value is then used as a cut point inside the PER-GRAPH map — `src/ops/list_private_graph_endpoints.rs:210-216`:
```
        // The endpoints of a graph are keyed by the VPC id they are addressed by, so one
        // stable total order underlies every page and a token can name a position in it.
        let mut remaining = store
            .private_endpoints_of(&graph_id, start_after.as_deref())
            .map(PrivateGraphEndpointSummary::from);
        let (private_graph_endpoints, next_token) =
            take_page(&mut remaining, page_size, |endpoint| &endpoint.vpc_id);
```
over `src/state.rs:1704-1719`:
```
    pub fn private_endpoints_of<'store>(
        &'store self,
        graph_id: &str,
        after_vpc_id: Option<&str>,
    ) -> impl Iterator<Item = &'store PrivateGraphEndpoint> {
        let lower = match after_vpc_id {
            Some(vpc_id) => ::std::ops::Bound::Excluded(vpc_id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.private_endpoints
            .get(graph_id)
            .map(|endpoints| endpoints.range((lower, ::std::ops::Bound::Unbounded)))
```
whose backing field is keyed by graph FIRST — `src/state.rs:1307-1312`:
```
    /// Every stored private graph endpoint, keyed by the graph it reaches and then by
    /// the VPC it lives in. ...
    pub private_endpoints: BTreeMap<String, BTreeMap<String, PrivateGraphEndpoint>>,
```

The contrast the other listings rest on — `src/state.rs:1925-1942` (`snapshots_in`), which ranges over the ONE global map and filters by graph afterwards:
```
        self.snapshots
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, snapshot)| snapshot)
            .filter(move |snapshot| {
                snapshot.is_in(region)
                    && of_graph.is_none_or(|graph_id| snapshot.source_graph_id == graph_id)
            })
```

The refusal the crate already has for a cursor it cannot resolve to a position in the page being asked for — `src/state.rs:2399-2404`:
```
pub fn unissued_page_token(token: &str) -> ValidationException {
    ValidationException {
        message: format!("nextToken '{token}' is not a pagination token this service issued"),
        reason: Some("ILLEGAL_ARGUMENT".to_string()),
    }
}
```

Test coverage: nothing pins the cross-graph case. `tests/all/unissued_page_token_refusal.rs:168-175` registers this listing under one `space: "vpc"` and only feeds it tokens of the OTHER id spaces (lines 197-203), and `tests/all/listing_paging_rule.rs:320,363` populates and pages the endpoints of a single graph (`endpoint_in_service(&router, &graph_id, ...)`, `path: format!("/graphs/{graph_id}/endpoints/")`), so no existing assertion covers a token minted on one graph being sent to another.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName="endpoint-host-a", provisionedMemory=16) and CreateGraph(graphName="endpoint-host-b", provisionedMemory=16); poll GetGraph on both until AVAILABLE.
3. On graph A: CreatePrivateGraphEndpoint twice, with vpcId="vpc-0a1b2c3d4e5f00001" and vpcId="vpc-0a1b2c3d4e5f00002" (subnetIds=["subnet-0a1b2c3d4e5f00000"]); poll GetPrivateGraphEndpoint until each is AVAILABLE.
4. On graph B: CreatePrivateGraphEndpoint once, with vpcId="vpc-0a1b2c3d4e5f00001" (the same VPC id is legal on another graph); poll until AVAILABLE.
5. GET /graphs/{A}/endpoints/?maxResults=1 — note the returned nextToken (it is hex("vpc-0a1b2c3d4e5f00001")).
6. GET /graphs/{B}/endpoints/?nextToken=<that token> — observe HTTP 200 with privateGraphEndpoints == [] and no nextToken: graph B's only endpoint is silently dropped because its vpcId sorts at the cut point taken from graph A's order, and the answer reads as "graph B has no endpoints".
7. GET /graphs/{B}/endpoints/ (no token) — observe the endpoint is there, proving the previous page omitted an entry instead of refusing a cursor that names no position in THIS listing (`nextToken '<token>' is not a pagination token this service issued`).
