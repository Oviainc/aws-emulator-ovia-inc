Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 806af0622baab44e6ea67461017632177fdce3d4

# The owed-status settle rule is implemented twice in state.rs, and the two copies have already drifted on CompletionTime

_Filed: 2026-10-28T19:14:33Z_
_Operation: GetSearchJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `806af0622baab44e6ea67461017632177fdce3d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsBackupsearchCryobackupsearchserviceState::settle` (reached by every handler that reports or moves a status, e.g. `GetSearchJob`) drives TWO independent implementations of one rule: "expire the record's ClientToken, then — if the clock has made the owed transition due — write the new status, the StatusMessage that status comes with, and CompletionTime". `SearchJob::settle` implements it for search jobs and `ExportJob::settle` implements it again for export jobs, down to a reworded copy of the same explanatory comment about status and message being written together.

The two copies have ALREADY drifted on one branch of that rule: the search-job copy states and enforces "`CompletionTime` is when the job COMPLETED, so only a status the job never leaves sets it: passing through a transient status completes nothing" and guards the write with `TERMINAL_STATES.contains(&next)`; the export-job copy carries no such guard and stamps `completion_time` on EVERY owed transition it materialises. That divergence is invisible today only because `StartSearchResultExportJob` happens to owe an export job nothing but a terminal status (`COMPLETED`/`FAILED`) — i.e. the second copy is correct by accident of its only caller, not by the rule it is supposed to implement. The model itself documents a non-terminal export status this service could later owe (`ExportJobSummary$Status`: "The status of the export job is one of the following: CREATED; RUNNING; FAILED; or COMPLETED"), at which point the unguarded copy would report a CompletionTime for an export that has not completed.

The defect is the duplication: one rule, two places to change, and one of the two already missing a branch the other spells out. Folding the shared steps (token expiry, due-instant comparison, status+message+CompletionTime write with the terminal-state guard) into one helper that both records call leaves every response and every existing test unchanged — the search-job copy additionally moves `scan_matches` into `results` on `COMPLETED`, which stays record-specific.

## Evidence

Copy 1 — services/.../src/state.rs:952-971 (`SearchJob::settle`):
```
    fn settle(&mut self, now: SystemTime) {
        expire_client_token(&mut self.client_token, now);
        while let Some(&(due, _, _)) = self.owed_statuses.front() {
            if now < due {
                return;
            }
            ...
            self.status = next.to_string();
            // The status and the message that status comes with are written together, so
            // a record can never explain a failure it has not reached, nor reach one
            // silently.
            self.status_message = message;
            // `CompletionTime` is when the job COMPLETED, so only a status the job never
            // leaves sets it: passing through a transient status completes nothing.
            if TERMINAL_STATES.contains(&next) {
                self.completion_time = Some(timestamp(due));
            }
```

Copy 2 — services/.../src/state.rs:1093-1107 (`ExportJob::settle`), the same rule with the CompletionTime guard missing:
```
    fn settle(&mut self, now: SystemTime) {
        expire_client_token(&mut self.client_token, now);
        let Some((due, next, message)) = self.next_status_due.clone() else {
            return;
        };
        if now < due {
            return;
        }
        self.status = next.to_string();
        self.next_status_due = None;
        self.completion_time = Some(timestamp(due));
        // The status and the message it comes with are written together, so a record can
        // never report a failure's text while it is still RUNNING.
        self.status_message = message;
    }
```

Both copies are driven from one place — services/.../src/state.rs:1211-1218:
```
    pub fn settle(&mut self, now: SystemTime) {
        for job in self.search_jobs.values_mut() {
            job.settle(now);
        }
        for job in self.export_jobs.values_mut() {
            job.settle(now);
        }
    }
```
and that is what `GetSearchJob` calls at services/.../src/ops/get_search_job.rs:192 (`state.settle(SystemTime::now());`).

Model text showing the branch the second copy drops is not hypothetical: `smithy_query(query="shape", shape="ExportJobSummary")` documents `Status` as "The status of the export job is one of the following: CREATED; RUNNING; FAILED; or COMPLETED."

Test coverage that must stay green and unchanged after the fold: tests/all/owed_status_due_instants.rs (`a_started_search_job_owes_each_phase_at_its_span_after_the_call`, `a_stopped_search_job_owes_its_end_at_the_transition_span_after_the_stop`, `a_started_export_job_owes_its_end_at_the_transition_span_after_the_call`, `a_token_window_that_cannot_be_represented_is_a_window_already_closed`), tests/all/search_job_status_phases.rs, tests/all/export_jobs.rs (`a_started_export_job_is_persisted_and_every_member_is_read_back_out_of_the_store`, `an_export_that_would_have_to_transmit_items_ends_in_the_modelled_failed_status`), tests/all/client_token_window.rs and tests/all/export_client_token_expiry.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs and read `SearchJob::settle` (lines 952-992) and `ExportJob::settle` (lines 1093-1107): both expire the record's ClientToken, both compare `now` against the stored due instant, and both write status + StatusMessage + CompletionTime together.
2. Compare the CompletionTime branch: the search-job copy guards it with `if TERMINAL_STATES.contains(&next)` and documents why; the export-job copy writes `self.completion_time = Some(timestamp(due));` unconditionally — one rule, two spellings, already divergent.
3. Confirm both copies are reached through the single `ComAmazonawsBackupsearchCryobackupsearchserviceState::settle` (state.rs:1211) that `GetSearchJob` calls at ops/get_search_job.rs:192, so any future change to the owed-transition rule has to be made in two places to stay consistent.
4. Fold the shared steps into one helper used by both records (keeping the terminal-state guard and leaving the search-job-only `scan_matches` -> `results` move where it is) and re-run the crate's tests: every response and every listed test must be unchanged.
