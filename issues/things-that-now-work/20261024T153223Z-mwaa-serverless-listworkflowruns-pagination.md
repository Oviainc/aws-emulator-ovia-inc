Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# ListWorkflowRuns pages runs in hashed-RunId order, so listings are not chronological and a run started mid-pagination can be skipped

_Filed: 2026-10-24T15:32:23Z_
_Operation: ListWorkflowRuns | Class: pagination_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Runs are stored in a `BTreeMap` keyed by the minted `RunId`, and `RunId` is `run-<16 hex chars of a SHA-256 digest>` — a value with no relation to when the run started. The listing walks that map in ascending key order and uses the last `RunId` of a page as the `NextToken` cursor (`Bound::Excluded(last)`).

Two observable consequences:
1. Order is pseudorandom with respect to time. A client that starts run A, then run B, and calls ListWorkflowRuns gets them in digest order, so the first element of the first page is not the most recent (or the oldest) run. On the real service run listings are chronological (the summary's `RunDetailSummary` carries `CreatedOn`/`StartedAt` for exactly this purpose, and a client's normal way to find 'the latest run' is the head of the listing).
2. Paging is not merely approximate, it can omit a run entirely: a run started while a client is paging gets a digest-derived id that may sort BEFORE the cursor already consumed, so that run is never returned by any subsequent page even though it existed for the whole walk. With a time-ordered key a concurrently started run is appended after the cursor and is always eventually seen.

Expected: runs returned in a stable chronological order (by start/creation time), so paging has the usual newest-first/oldest-first semantics and a concurrently created run cannot fall behind the cursor.

Actual: ascending digest order over `run-<hash>` ids.

## Evidence

Minting: `crate::state::mint_run_id` (state.rs ~line 626) = `format!("run-{suffix}")` where suffix is `digest(["workflow-run", workflow_arn, nonce])` truncated to 16 chars — not time-ordered. Storage/paging: `WorkflowRun`s live in `workflow.runs: BTreeMap<String, WorkflowRun>` keyed by RunId; `src/ops/list_workflow_runs.rs` lines 255-281 page with `workflow.runs.range((Excluded(last), Unbounded))` and encode the next cursor from the last `run_id`. Model grounding: `WorkflowRunSummary.RunDetailSummary` (`smithy_query(query="shape", shape="RunDetailSummary")`) exposes `CreatedOn`/`StartedAt`/`EndedAt`, i.e. the listing is a time-ordered view of executions; AWS execution/run listings are returned chronologically. Test coverage: `tests/all/workflow_run_listing.rs` (`every_persisted_run_is_listed_exactly_once_across_pages_in_run_id_order`, line 208, and the assertions near line 490) pins the current RunId-order behavior, so it asserts the divergence rather than chronological order and must be updated with the fix.

## Steps to Reproduce

1. CreateWorkflow, then call StartWorkflowRun three times in sequence, recording the returned RunIds and StartedAt values in call order
2. Call ListWorkflowRuns with MaxResults=10 and compare the order of `WorkflowRuns[*].RunDetailSummary.StartedAt` with the order the runs were started — they do not match (the order follows the hashed RunId)
3. Repeat with MaxResults=1: fetch page 1, then StartWorkflowRun once more, then continue paging with the returned NextToken; when the new run's hashed RunId sorts before the cursor it never appears in any later page
