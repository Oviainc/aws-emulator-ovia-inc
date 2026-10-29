Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# A page cursor is a bare offset with nothing identifying the listing that minted it, although GetUsageStatistics' own page() doc says a token from another listing is refused

_Filed: 2026-10-29T04:01:38Z_
_Operation: GetUsageStatistics | Class: pagination_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated report on this service pages through the one shared helper `crate::state::paginate` / `paginate_window`, whose cursor is the number of entries already returned rendered as a decimal string. Nothing in the cursor identifies WHICH ordered list it is an offset into, and `paginate_window` refuses a cursor on exactly two grounds: it does not parse as a `usize`, or it is `>= len` of the list being paged. A cursor that another listing minted, and that happens to be in range for the list now being paged, is therefore accepted silently: the caller gets a page that starts in the middle of a list it never paged, with the skipped entries missing and — once the page reaches the end — no `nextToken` to say anything was left out. The caller cannot tell an incomplete first page from a complete one.

`GetUsageStatistics`'s own `page()` documentation states the opposite rule for the cursor it hands out and accepts: "a token from another listing, or one past the end, is refused rather than silently answered with the first page". Only the second half of that sentence is implemented. The contradiction is reachable INSIDE this one operation, because each `usageStatisticsType` aggregates over a different key list built in `aggregated()` (accounts for `SUM_BY_ACCOUNT`, features for `SUM_BY_FEATURES`, data-source names for `SUM_BY_DATA_SOURCE`, resource ARNs for `SUM_BY_RESOURCE`/`TOP_RESOURCES`) and all of them are paged through that same `page()`: a `nextToken` a `SUM_BY_FEATURES` page issued is accepted by a `SUM_BY_DATA_SOURCE` request and silently drops the leading rows of a completely different aggregation. The same holds across operations, since `ListFindings`, `ListIPSets`, `ListMembers`, `ListFilters`, `ListCoverage`, `ListInvestigations`, `DescribeMalwareScans` and the rest all mint and accept the same unscoped decimal offsets.

This is not an unattempted feature: the crate already states the rule it wants, already refuses out-of-range and non-numeric cursors in one shared place, and already has a test asserting "A nextToken this service did not issue is refused" — that test simply never exercises a cursor the service DID issue for a different list.

## Evidence

THE RULE THE CODE BREAKS — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_usage_statistics.rs:740-744:
```
/// The page of ordered keys `maxResults` / `nextToken` asks for.
///
/// The token is the one this service's other listings issue — an offset into the ordered
/// keys — so a token from another listing, or one past the end, is refused rather than
/// silently answered with the first page.
```
and the body under that doc, get_usage_statistics.rs:745-757, which hands the token straight to the shared helper with no listing identity at all:
```
fn page(
    totals: &BTreeMap<String, i64>,
    order: Order,
    max_results: Option<i32>,
    token: Option<&str>,
) -> Result<(Vec<String>, Option<String>), GetUsageStatisticsError> {
    let keys = ordered(totals, order);
    crate::state::paginate(&keys, max_results, token, TOP_ROWS).map_err(|_| {
```

WHAT THE HELPER ACTUALLY REFUSES — src/state.rs:5465-5476 (the only two rejections; neither is "a token from another listing"):
```
        let start = match token.filter(|t| !t.is_empty()) {
            Some(t) => {
                let start = t.parse::<usize>().map_err(|_| t.to_string())?;
                // A token this service issued always names an entry that exists: it is
                // emitted only while `end < len`.
                if start >= len {
                    return Err(t.to_string());
                }
                start
            }
            None => 0,
        };
```
and what it mints, state.rs:5482-5487 — a bare offset:
```
        let end = start.saturating_add(limit).min(len);
        let next = if end < len {
            Some(end.to_string())
        } else {
            None
        };
```
Its own doc restates the same claim it cannot keep, state.rs:5436-5440:
```
/// `token` is the number of entries already returned, as a decimal string — the
/// only thing a caller may do with it is hand it back. `Err(token)` means the token
/// was not one this service issued: either it is not a decimal offset at all, or it
/// points past the end of the list, which this service never hands out (it answers
/// the last page with no `NextToken`).
```

THE DIFFERENT KEY LISTS ONE OPERATION PAGES THROUGH IT — get_usage_statistics.rs:788 (`totals_by(usage, |record| Some(record.account_id.clone()))`), :806-808 (`data_source_for(record.feature)`), :826 (`Some(record.feature.to_string())`), :844 (`record.resource.clone()`), each followed by the same `page(&totals, …, next_token)?` at :789, :809, :827, :852 and :884.

OTHER LISTINGS MINTING AND ACCEPTING THE SAME OFFSETS: src/ops/list_findings.rs:199-209, src/ops/list_ip_sets.rs:177-193, src/ops/list_members.rs:193-205, src/ops/list_filters.rs:170-186, src/ops/list_coverage.rs:209-221, src/ops/list_investigations.rs:207-219, src/ops/describe_malware_scans.rs:223-233 — all call `crate::state::paginate` / `paginate_window` with `input.next_token.as_deref()` and nothing else.

TEST COVERAGE, which does not pin the current behaviour — tests/all/usage_statistics.rs:772-786:
```
/// A `nextToken` this service did not issue is refused rather than answered with the
/// first page. The rows span the ways a token can fail to resolve.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    …
    for token in ["not-a-token", "-1", "9999", ""] {
```
The four rows are a non-numeric value, a negative value, an out-of-range offset and the empty string. No test hands one listing's (or one `usageStatisticsType`'s) issued cursor to another, so nothing asserts that it is accepted today.

## Steps to Reproduce

1. Create a detector: `POST /detector` with `{"enable": true}`; note the `detectorId`.
2. Generate findings whose usage spans at least three features: `POST /detector/{detectorId}/findings/create` (`CreateSampleFindings`) with no `findingTypes`, which generates the whole catalogue — e.g. flow-log, CloudTrail and S3-data-event findings.
3. Page one aggregation: `POST /detector/{detectorId}/usage/statistics` with `{"usageStatisticsType":"SUM_BY_FEATURES","usageCriteria":{},"maxResults":1}`. The answer carries `nextToken` = "1" (get_usage_statistics.rs:827 → state.rs:5483-5487).
4. Hand that cursor to a DIFFERENT aggregation of the same operation: `POST /detector/{detectorId}/usage/statistics` with `{"usageStatisticsType":"SUM_BY_DATA_SOURCE","usageCriteria":{},"nextToken":"1"}`. Expected per get_usage_statistics.rs:742-744: the token is refused. Actual: HTTP 200 with `sumByDataSource` starting at the SECOND data-source key, the first one silently missing and no `nextToken` to indicate a partial answer, because state.rs:5467-5473 only checks that `1 < len`.
5. Repeat across operations: create three IPSets (`POST /detector/{detectorId}/ipset` three times), then `POST /detector/{detectorId}/findings` (`ListFindings`) with `{"maxResults":1}` to obtain `nextToken` = "1", and send `GET /detector/{detectorId}/ipset?nextToken=1`. It answers 200 with only the last two IPSets instead of refusing a cursor no IPSet listing issued.
6. Contrast with the cursor values the existing test uses (tests/all/usage_statistics.rs:779 — "not-a-token", "-1", "9999"): those are refused, so the refusal exists and only the listing-identity half of the stated rule is missing.
