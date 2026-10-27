Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# NextToken offsets index the newest-first sequence, so a job created mid-walk makes a resumed page repeat entries and skip the new job

_Filed: 2026-10-27T13:42:51Z_
_Operation: ListSearchJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSearchJobs` pages by storing a numeric OFFSET in the `NextToken` (`encode_next_token(context, end)` -> "<end>@jobs:ALL"). `decode_next_token` justifies that offset-based token with the premise that the sequence it indexes is only ever APPENDED to, so a handed-out offset keeps naming the same place. That premise does not hold for the sequence the handler actually pages: the handler builds `summaries` from `state.jobs_newest_first()`, i.e. `job_order` walked BACKWARDS, so a newly started job is PREPENDED to the paged sequence (index 0) and every previously-issued offset now points one entry earlier than it did.

Consequence for a paginating client: with jobs j1,j2,j3 (newest-first [j3,j2,j1]) and MaxResults=2, page 1 returns [j3,j2] and a token with offset 2. If `StartSearchJob` mints j4 before the next call, the sequence is [j4,j3,j2,j1]; resuming at offset 2 returns [j2,j1] — j2 is reported twice in one walk, and j4 is never reported at all even though it exists for the whole of page 2. A walk that is supposed to enumerate the held jobs therefore both duplicates and loses entries, silently (HTTP 200), whenever a job is created between two pages.

The same offset-over-reversed-order construction is shared by `ListSearchResultExportJobs` through the same helper (`state::newest_first` / `export_jobs_newest_first`), so the defect is in the one paging rule rather than in one listing. The crate's own paging test only creates every job BEFORE the walk starts, so nothing pins the mid-walk case.

## Evidence

The documented premise (state.rs:1777-1781, `decode_next_token`):
```
/// `len` is the length of that held sequence and never of a filtered view of it. A held
/// sequence of this service is only ever appended to, while which of its records a filter
/// keeps can change after a token was issued, because a record's own status moves on with
/// no caller involved; so an offset this service handed out stays inside the sequence it
/// indexes for as long as the store holds those records.
```
The sequence actually indexed is the REVERSE of that appended-to order (state.rs:1253-1263):
```
/// `job_order` holds the identifiers in creation order and is only ever appended to,
/// so walking it backwards is exactly newest-first and is stable across calls.
pub fn jobs_newest_first(&self) -> impl Iterator<Item = &SearchJob> {
    newest_first(&self.job_order, &self.search_jobs)
}
```
and the handler pages exactly that reversed vector by offset (src/ops/list_search_jobs.rs:166-198):
```
let summaries: Vec<SearchJobSummary> = state
    .jobs_newest_first()
...
let offset = crate::state::decode_next_token(input.next_token.as_deref(), &context, summaries.len())
...
let (search_jobs, next_token) = crate::state::page_matching(&summaries, offset, crate::state::page_size(input.max_results), &context, ...)
```
where the offset is a raw index into that vector (state.rs:1849-1856):
```
let mut end = offset.min(items.len());
...
let next = (page.len() == max_results).then(|| encode_next_token(context, end));
```
Test coverage: `tests/all/search_jobs.rs:563-655` (`paging_returns_every_job_exactly_once_whatever_the_page_size`) creates all jobs up front and only then follows the token — "following NextToken to exhaustion yields each persisted job exactly once newest-first" — so no existing test exercises a create between two pages; same for `tests/all/export_listing.rs:234` and `tests/all/export_listing_order.rs:146`.

## Steps to Reproduce

1. PUT /search-jobs three times with body {"SearchScope":{"BackupResourceTypes":["S3"]}} to mint j1, j2, j3.
2. GET /search-jobs?MaxResults=2 — the page holds [j3, j2] and carries NextToken "2@jobs:ALL".
3. PUT /search-jobs once more to mint j4 (as any concurrent caller may).
4. GET /search-jobs?MaxResults=2&NextToken=2@jobs:ALL — the page holds [j2, j1]: j2 is returned a second time inside the same walk, and j4 is never returned by the walk at all.
5. Compare with the premise quoted in state.rs:1777-1781, which claims an issued offset keeps naming its place because the indexed sequence is only appended to.
