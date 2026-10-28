Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 1faf41e59ff01533a48e04d029136d23581e5c75

# The page-context sentinel "ALL" collides with a real filter value, so a NextToken resumes a differently filtered listing instead of being refused

_Filed: 2026-10-28T21:25:45Z_
_Operation: ListSearchResultExportJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `1faf41e59ff01533a48e04d029136d23581e5c75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSearchResultExportJobs` builds the sequence name that goes inside every `NextToken` by substituting the literal string `ALL` for an ABSENT filter (`crate::state::export_jobs_page_context`). `SearchJobIdentifier` on this operation is an unconstrained `GenericId` filter that the crate deliberately does not syntax-check, so `ALL` is a value a client can legally send — and when it does, the context string is byte-identical to the one the UNFILTERED listing issues.

The consequence is exactly the case the context was introduced to prevent, and the one `decode_next_token` is written to refuse: a token this service issued for the unfiltered sequence (`export-jobs:ALL:ALL`) is accepted under `?SearchJobIdentifier=ALL`, and a token issued under that filter is accepted for the unfiltered listing. Instead of the 400 `ValidationException` the module's own rule calls for, the caller gets a 200 whose page was produced by a different filter than the token was handed out for — an error turned into a success. The sibling listing's helper has the same shape (`jobs_page_context` substitutes `ALL` for an absent `ByStatus`), so the defect is in the sentinel scheme, not in one call site: the fix is a context encoding in which "no filter" cannot be spelled by a filter value (e.g. distinguishing present from absent rather than reusing an in-band word).

## Evidence

The sentinel, services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:2034-2044 — the doc states the invariant and the code spells "no filter" with an in-band value:
```
/// The sequence a `ListSearchResultExportJobs` page indexes: the export jobs left by
/// both of that request's filters. Part of the page's `NextToken` for the same reason
/// [`jobs_page_context`] is: an offset alone does not say which filtered sequence it
/// indexes.
pub fn export_jobs_page_context(by_status: Option<&str>, by_search_job: Option<&str>) -> String {
    format!(
        "export-jobs:{}:{}",
        by_status.unwrap_or("ALL"),
        by_search_job.unwrap_or("ALL")
    )
}
```
and the same pattern at state.rs:2017-2022: "Part of the page's `NextToken`, so a token cannot resume a differently filtered listing." / `format!("jobs:{}", by_status.unwrap_or("ALL"))`.

The guard the collision defeats, state.rs:1947-1952:
```
    let (position, token_context) = token.split_once('@').ok_or_else(malformed)?;
    if token_context != context {
        return Err(format!(
            "NextToken '{token}' was issued for a different request and cannot resume this one"
        ));
    }
```

The handler feeds the raw filter value straight into that name, src/ops/list_search_result_export_jobs.rs:207-210:
```
        let context = crate::state::export_jobs_page_context(
            input.status.as_deref(),
            input.search_job_identifier.as_deref(),
        );
```

Why `ALL` is a value that reaches it: the crate states that `GenericId` carries no pattern — state.rs:541-544: "The model puts no `@pattern` on `GenericId`, so the boundary validator cannot refuse a string outside that shape and the service does ([`check_job_identifier`])" — and that this member in particular is exempt from that guard, state.rs:574-577: "A member that is a FILTER rather than a resource to resolve (`ListSearchResultExportJobs`'s `SearchJobIdentifier`) is not an identifier this rule covers: it narrows a listing and is never looked up, so it answers an empty page." The model agrees: `smithy_query(query="operation", operation="ListSearchResultExportJobs")` reports `"SearchJobIdentifier": {"target": "GenericId", "httpQuery": "SearchJobIdentifier", "notProperty": true}` — no constraint at all.

The rule the crate's own test pins for the sibling listing, tests/all/search_jobs.rs:763-781:
```
    // ...and nothing else. Each row is a request the token was NOT issued for.
    let others: Vec<(&str, String)> = vec![
        ("a status filter", format!("/search-jobs?Status=RUNNING&MaxResults=1&NextToken={token}")),
        ...
    for (case, uri) in others {
        let answer = call(&emulator, "us-west-2", "GET", &uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token issued for another sequence must be refused, got {} ({:?})",
```
No test sends `SearchJobIdentifier=ALL` (or any filter value equal to the sentinel): tests/all/export_listing.rs:733-757 and tests/all/export_listing_order.rs:109-110 only cross tokens between `?Status=RUNNING` and a real search-job identifier, so the collision is unpinned.

## Steps to Reproduce

1. Start two search jobs and let them settle to COMPLETED, then start two export jobs (PUT /export-search-jobs) so the store holds more than one export job.
2. Call GET /export-search-jobs?MaxResults=1 with no filters. The page carries NextToken "1@export-jobs:ALL:ALL" (state.rs:1850 `format!("{position}@{context}")` with the context from state.rs:2038-2044).
3. Resume that token under a DIFFERENT filter: GET /export-search-jobs?MaxResults=1&SearchJobIdentifier=ALL&NextToken=1@export-jobs:ALL:ALL. The handler recomputes the context as "export-jobs:ALL:ALL" (the sentinel for "no SearchJobIdentifier filter"), the equality check at state.rs:1948 passes, and the call answers 200 with a page produced under a filter the token was never issued for.
4. Compare with the rule the crate enforces for the sibling listing: GET /search-jobs?Status=RUNNING&MaxResults=1&NextToken=<token from the unfiltered /search-jobs> is refused 400 ValidationException (tests/all/search_jobs.rs:775-785).
5. The reverse direction too: take the NextToken from GET /export-search-jobs?MaxResults=1&SearchJobIdentifier=ALL (an empty-but-tokenless page unless an export job's search job is literally named ALL; use MaxResults=1 with a store where the filter admits a record) and replay it with no filter at all — the same context string is accepted.
