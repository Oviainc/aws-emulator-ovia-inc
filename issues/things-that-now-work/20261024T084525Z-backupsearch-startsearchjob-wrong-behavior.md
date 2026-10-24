Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# ClientToken idempotency never expires, so a token reused after its documented 8-hour window is not treated as a new request

_Filed: 2026-10-24T08:45:25Z_
_Operation: StartSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents ClientToken's validity window on StartSearchJob: "A client token is valid for 8 hours after the first request that uses it is completed. After this time, any request with the same token is treated as a new request."

The emulator's idempotency index is keyed off the stored job records and carries no expiry: `state.job_by_client_token(token)` (src/state.rs:545-550) matches any job ever created with that token, for the lifetime of the process. So (a) a repeat of the same body under the same token hours later keeps returning the FIRST job's SearchJobIdentifier/CreationTime instead of minting a new job, and (b) a different body under that token keeps answering 409 ConflictException forever, where AWS treats the token as expired and accepts the call.

Expected: a search job's ClientToken stops participating in idempotency 8 hours after the call that minted it completed; a later request carrying that token mints a new search job (and a different body under it is no longer a conflict). The record already stores creation_time and the crate already materialises clock-driven transitions in `settle`, so the window is computable from the same clock.

## Evidence

Spec: smithy_query doc for StartSearchJobInput member ClientToken — "A client token is valid for 8 hours after the first request that uses it is completed. After this time, any request with the same token is treated as a new request." Handler: src/ops/start_search_job.rs:200-221 consults job_by_client_token with no age check; src/state.rs:545-550 scans every stored job's client_token with no expiry; SearchJob.client_token is never cleared and `settle` touches only status fields. Test coverage: the client-token tests (tests/all/export_jobs.rs::a_client_token_is_idempotent_for_the_same_body_and_conflicts_for_a_different_one and the search-job suite) assert only the within-window behaviour; nothing asserts expiry.

## Steps to Reproduce

1. StartSearchJob(ClientToken="t1", SearchScope={"BackupResourceTypes":["S3"]}) and note SearchJobIdentifier=A.
2. Advance the emulator's clock (or wait) past 8 hours after that call.
3. StartSearchJob(ClientToken="t1", <same body>): the emulator again returns identifier A and creates nothing; AWS treats the token as expired and mints a new search job.
4. StartSearchJob(ClientToken="t1", <different Name/SearchScope>): the emulator answers 409 ConflictException; AWS accepts the request and mints a new job.
