Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# A NextToken this service issued for a transient-status listing is refused 400 once the clock moves jobs out of the filter

_Filed: 2026-10-26T08:47:02Z_
_Operation: ListSearchJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListSearchJobs (and ListSearchResultExportJobs) settle the store BEFORE they build the filtered sequence the page token indexes, and the token carries only an offset plus the filter name. A `ByStatus`/`Status` filter naming a TRANSIENT status (RUNNING, STOPPING for search jobs; RUNNING for export jobs) therefore names a sequence that SHRINKS on its own: the clock materialises the owed terminal status without anybody calling, so the filtered sequence that was N entries long when page 1 was served is shorter — often empty — when page 2 arrives. `decode_next_token` then rejects the token the service itself handed out, because `offset > len`, and the caller gets HTTP 400 ValidationException ("resumes past the end of the results") in the middle of walking a listing it paged correctly.

That contradicts the rationale written over the refusal: the helper's own doc says an `Err` here is the CALLER's problem (a token the service never issued, a token for another sequence, or one past the end), yet this refusal is caused only by the service's own clock advancing a status the crate documents as moving without a reader. The caller's request is unchanged and its token is genuine. Only transient-status filters are affected: the unfiltered listing and terminal-status filters never shrink, so their offsets stay valid.

The same helper and the same shrink apply to ListSearchResultExportJobs?Status=RUNNING (list_search_result_export_jobs.rs:174-216), which is the second call site of the identical code path.

## Evidence

Both sides are in this crate.

(1) The refusal and the reason given for it — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:1384-1409:
```
/// Where a `NextToken` resumes, as an offset into the sequence `context` names.
///
/// `Err` is the caller's problem: a token this service did not issue, one issued for a
/// different sequence, or one that points past the end of what it is paging over.
...
    if offset > len {
        return Err(format!(
            "NextToken '{token}' resumes past the end of the results"
        ));
    }
```

(2) The sequence the offset indexes is rebuilt per call, AFTER settling, and `len` is the FILTERED length — src/ops/list_search_jobs.rs:157-188:
```
        let mut state = state.lock();
        // Report the statuses the clock has already made due, so a job that completed
        // while nobody was looking is listed as COMPLETED.
        state.settle(SystemTime::now());

        let summaries: Vec<SearchJobSummary> = state
            .jobs_newest_first()
            .filter(|job| {
                input
                    .by_status
                    .as_deref()
                    .is_none_or(|wanted| job.status == wanted)
            })
...
        let offset = crate::state::decode_next_token(
            input.next_token.as_deref(),
            &context,
            summaries.len(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation)?;
```

(3) The crate states that the shrink happens with no caller involved — src/state.rs:21-29: "## The clock, not the reader, moves a status ... Those owed transitions are stored as DUE INSTANTS ... and materialised by [`...::settle`], which every handler calls first"; and src/state.rs:180 `TRANSITION_DELAY: Duration = Duration::from_millis(500)` with src/state.rs:190 `CREATING_DELAY ... 250` — so a RUNNING job leaves RUNNING ~250 ms after it enters it.

(4) Test coverage pins only the FABRICATED-offset case, not this one — tests/all/search_jobs.rs:623-654 `a_next_token_this_service_did_not_issue_is_refused` asserts 400 for `"9999@jobs:ALL"` and for `"1@jobs:RUNNING"` sent WITHOUT a status filter (a context mismatch), with the stated reason "a token this service did not issue for THIS request must be refused". A token the service DID issue for a `Status=RUNNING` listing is not covered by that reason.

## Steps to Reproduce

1. Start three search jobs: PUT /search-jobs {"SearchScope":{"BackupResourceTypes":["S3"]}} three times.
2. Wait ~300 ms, i.e. past CREATING_DELAY (250 ms) but inside TRANSITION_DELAY (500 ms), so all three jobs report Status=RUNNING.
3. GET /search-jobs?Status=RUNNING&MaxResults=1 -> 200 with one SearchJobSummary and NextToken "1@jobs:RUNNING" (a token this service issued).
4. Wait another ~300 ms, so the clock makes each job's terminal status due (FAILED on an empty scope, COMPLETED with recovery points in scope).
5. GET /search-jobs?Status=RUNNING&MaxResults=1&NextToken=1@jobs:RUNNING -> HTTP 400 ValidationException "NextToken '1@jobs:RUNNING' resumes past the end of the results", because the RUNNING-filtered sequence is now empty (summaries.len() == 0 < offset 1).
6. Same path on the export listing: start an export (PUT /export-search-jobs) for a settled search job, GET /export-search-jobs?Status=RUNNING&MaxResults=1 to get a token, wait past TRANSITION_DELAY, then resume with that token -> the same 400.
