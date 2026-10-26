Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Export ClientToken idempotency never expires past its documented 8-hour window

_Filed: 2026-10-24T08:45:25Z_
_Operation: StartSearchResultExportJob | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSearchResultExportJob's ClientToken carries the same documented window as StartSearchJob's: "A client token is valid for 8 hours after the first request that uses it is completed. After this time, any request with the same token is treated as a new request."

The emulator matches the token against every export job it has ever stored, with no age check (src/ops/start_search_result_export_job.rs:231-254 via state.export_job_by_client_token, src/state.rs:598-603). A repeat of the same body under that token therefore keeps replaying the first export job's ExportJobIdentifier/ExportJobArn indefinitely, and a different body under that token keeps being refused 409 ConflictException, where AWS — once the window has passed — treats the call as a new request and mints a second export job.

Expected: the token stops participating in idempotency 8 hours after the minting call completed; a later call with that token creates a new export job and a differing body is no longer a conflict.

## Evidence

Spec: smithy_query doc for StartSearchResultExportJobInput member ClientToken (8-hour validity sentence, identical to StartSearchJob's). Handler: src/ops/start_search_result_export_job.rs:231-254; index in src/state.rs:598-603 ("Read off the records themselves") with no expiry and ExportJob.client_token never cleared; ExportJob::settle (src/state.rs:381-397) only moves the status. Test coverage: tests/all/export_jobs.rs::a_client_token_is_idempotent_for_the_same_body_and_conflicts_for_a_different_one asserts only the within-window behaviour.

## Steps to Reproduce

1. Drive a search job to STOPPED, then StartSearchResultExportJob(ClientToken="e1", SearchJobIdentifier=<id>, ExportSpecification={"s3ExportSpecification":{"DestinationBucket":"b"}}) and note ExportJobIdentifier=E.
2. Advance the emulator's clock past 8 hours after that call.
3. Repeat the identical request with ClientToken="e1": the emulator replays identifier E and creates nothing; AWS mints a new export job.
4. Send a different body (e.g. DestinationPrefix added) with ClientToken="e1": the emulator answers 409 ConflictException; AWS accepts it.
