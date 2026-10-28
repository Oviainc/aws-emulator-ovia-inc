Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# GetJobsQueryResults reports a nextToken for a query that has not completed, contradicting its own "reports no page at all" rule

_Filed: 2026-10-27T02:23:20Z_
_Operation: GetJobsQueryResults | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetJobsQueryResults gates the `Jobs` half of the stored page on the query having reached `COMPLETE`, but copies `NextToken` out of the record unconditionally. A caller polling a query that is still `SUBMITTED` therefore gets `status: SUBMITTED`, no `jobs`, and a `nextToken` that points past a batch it has never been shown — a token it can immediately hand to StartJobsQuery to start the SECOND batch of a query whose first batch is still unreadable. The handler's own comment three lines above states the opposite rule.

## Evidence

services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/get_jobs_query_results.rs:184-203:
```
        // The matched page is the one the query stored when it was started, so the jobs a
        // caller polls do not shift under them. A query that has not completed yet
        // reports no page at all, only its status.
        let complete = query.status == crate::state::JOBS_QUERY_STATUS_COMPLETE;
        let status = Some(query.status.clone());
        let next_token = query.result_next_token.clone();
        let matched = query.matched.clone();
        let jobs = complete.then(|| { ... });

        Ok(GetJobsQueryResultsOutput {
            jobs,
            next_token,
            status,
        })
```
`jobs` honours `complete`; `next_token` does not, even though both are halves of the one stored page (src/state.rs:2987-2991: "The ids of the jobs on the page this query matched ... Reported once the query is JOBS_QUERY_STATUS_COMPLETE" / "The cursor for the batch after this page").

Test coverage: tests/all/jobs_query.rs:394-418 `a_query_reports_submitted_until_its_completion_comes_due` asserts only `pending.at("/jobs").is_null()` for the held query; no assertion is made about `nextToken` while the query is SUBMITTED, so the current behaviour is unpinned.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. Create more than `maxResults` jobs (e.g. 3 jobs) so the query's matched page is not the last batch.
3. start_jobs_query(MaxResults=1) and immediately get_jobs_query_results(Id=<id>) before the completion comes due.
4. Observe the response: Status 'SUBMITTED', no Jobs member, but a NextToken is present — a cursor past a page the caller was not shown.
5. Compare with the handler's stated rule: a query that has not completed "reports no page at all, only its status".
