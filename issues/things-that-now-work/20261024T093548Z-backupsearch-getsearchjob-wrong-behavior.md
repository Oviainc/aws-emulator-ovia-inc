Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# A just-started search job is reported RUNNING, never the CREATING status AWS assigns it first

_Filed: 2026-10-24T09:35:48Z_
_Operation: GetSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSearchJob mints the job with `status: RUNNING` and owes it a terminal status after a fixed span, so the first status any caller can observe through GetSearchJob/ListSearchJobs is RUNNING. On AWS a freshly started search job first sits in CREATING and only then becomes RUNNING: the recorded refusal template for an export started back-to-back with StartSearchJob is "Export is only supported for search job with COMPLETED,STOPPED statuses. SearchJob has status <STATUS>. Please try again later", and it was recorded with <STATUS>=CREATING as well as <STATUS>=RUNNING (quoted in the open issue 20261023T110504Z-backupsearch-export-stop-refusals-..., cause 2) — that status can only have come from a search job the service had put in CREATING.

Expected: a search job observed immediately after StartSearchJob reports Status=CREATING, then RUNNING, then its terminal status; the status is a non-terminal transient either way, so the owed-transition machinery in src/state.rs already fits (one more due instant: CREATING -> RUNNING -> terminal).
Actual: the first observable status is always RUNNING; CREATING never appears in any response, and a caller that waits for RUNNING as "the scan has started" cannot distinguish the two phases.

Note: the modeled SearchJobState enum lists only COMPLETED|FAILED|RUNNING|STOPPED|STOPPING, so CREATING is an unmodeled value the service nonetheless returns (restJson1 enums are open on the wire); the recording, not the enum, is the authority here. This is deliberately filed LOW: the terminal-status rule (already filed and fixed) is what most clients branch on.

## Evidence

Spec/grounding: recorded AWS behavior — the status-parameterized export refusal template recorded with <STATUS>=CREATING for a job just created by StartSearchJob (open issue 20261023T110504Z-backupsearch-export-stop-refusals-non-iam-rolearn-misclassified.md, cause 2, trace backupsearch.start-search-result-export-job-immediately-after-start-rejected). Handler/state: src/ops/start_search_job.rs:292-323 sets `status: crate::state::RUNNING` and `next_status_due = (now + TRANSITION_DELAY, owed_status, owed_message)`; src/state.rs declares RUNNING/STOPPING as the only transient statuses and has no CREATING at all. Test coverage: tests/all/search_jobs.rs asserts the first observed status is RUNNING within WITHIN_THE_DUE_INSTANT (so that assertion moves with the fix); no test mentions CREATING.

## Steps to Reproduce

1. Boot the emulator and point a botocore backupsearch client at it.
2. start_search_job(Name="x", SearchScope={"BackupResourceTypes":["S3"]}).
3. Immediately call get_search_job(SearchJobIdentifier=<id>) (and list_search_jobs) and read Status: the emulator reports RUNNING from the very first read; AWS reports CREATING first and only then RUNNING.
4. Cross-check through the export gate: immediately call start_search_result_export_job for that job — AWS's refusal reports the job's status as CREATING, the emulator's can only ever report RUNNING.
