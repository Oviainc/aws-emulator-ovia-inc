Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5c8ac701ad3e8d191ad2ab71e1d77d8b6db767d3

# The ACTIVE-data-store guard runs before the clientToken ledger, so the idempotent REPLAY of a job already submitted is answered 409 instead of reporting that job

_Filed: 2026-10-29T06:25:25Z_
_Operation: StartDICOMImportJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `5c8ac701ad3e8d191ad2ab71e1d77d8b6db767d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartDICOMImportJob` is `@idempotent` and its `clientToken` is a real `@idempotencyToken`: the handler keeps a ledger (`crate::state::client_token_key` / `remember_client_token`) and its own comment states that a token already in the ledger makes the repeat call REPORT the job it named. But the handler evaluates the data-store precondition (`!datastore_is_active()` -> 409 `ConflictException`) BEFORE it ever looks in the ledger, so the comment's rule does not hold whenever the store has left `ACTIVE` since the first submit.

Concretely: a job is submitted against an `ACTIVE` store (ledger records clientToken -> jobId); `DeleteDatastore` is then accepted, because it only blocks on image sets the store still HOLDS, and the store enters `DELETING`; a client retrying the submit with the SAME clientToken — the exact retry the token exists for — is answered 409 `ConflictException` with `DATASTORE_NOT_ACTIVE_MESSAGE` instead of 200 with the jobId it already owns. The retry creates nothing, so the justification the handler gives for the guard ("a store the clock has not yet settled ... cannot TAKE an import") does not apply to it: there is no import to take.

The two sibling `@idempotent` writers in this same crate order these two steps the other way round. `CreateDatastore` resolves the ledger FIRST and only then applies its refusals, so a replay can never be turned into an error by a condition the first call already passed. `DeleteDatastore` carves out exactly this case explicitly, admitting the repeat of a delete that is already in flight rather than refusing it mid-transition. `StartDICOMImportJob` is the one `@idempotent` operation whose replay a status change can turn into a failure.

## Evidence

The guard runs first — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/start_dicom_import_job.rs:191-200:
```
        // "Start importing bulk data into an ACTIVE data store": a store the clock has
        // not yet settled — still `CREATING`, or already `DELETING` — cannot take an
        // import, which is what the modelled `ConflictException` (HTTP 409) reports
        // ([`crate::state::Datastore::is_active`]).
        if !datastore_is_active {
            return Err(StartDICOMImportJobErrorModeled::ConflictException(ConflictException {
                message: crate::state::DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
            })
            .into());
        }
```
and the ledger — whose own comment states the rule the code above it has already broken — only afterwards, src/ops/start_dicom_import_job.rs:202-222:
```
        // `@idempotencyToken`, scoped to the data store the job runs against: a token
        // already in the ledger names the job it submitted, so the repeat call reports
        // that job's CURRENT status rather than submitting a second one.
        let token_key = crate::state::client_token_key(...);
        if let Some(existing_id) = store.client_tokens.get(&token_key).cloned() {
            if let Some(existing) = store
                .datastore_in(&region, &input.datastore_id)
                .and_then(|datastore| datastore.import_jobs.get(&existing_id))
            {
                return Ok(StartDICOMImportJobOutput { ... });
```

The sibling that resolves the ledger BEFORE its refusals — src/ops/create_datastore.rs:196-208 (the quota refusals follow at :216 and :231):
```
        // `@idempotencyToken`: a token already in the ledger names the store it minted,
        // so the repeat call reports that store's CURRENT status rather than creating
        // a second one.
        let token_key = ...;
        if let Some(existing_id) = store.client_tokens.get(&token_key).cloned() {
            if let Some(existing) = store.datastore_in(&region, &existing_id) {
                return Ok(CreateDatastoreOutput { ... });
```

The sibling that explicitly admits the idempotent repeat mid-transition — src/ops/delete_datastore.rs:196-202:
```
        // `@idempotent`: a repeat delete reports the same `DELETING` without pushing the
        // deletion the clock already owes further out.
        if datastore.datastore_status != DatastoreStatus::Deleting {
```

That `DeleteDatastore` admits the delete at all while the store still owns import jobs is what makes the window reachable: the delete blocks only on image sets — src/ops/delete_datastore.rs:185:
```
        if datastore.held_image_sets().next().is_some() {
```

Test coverage: no test drives a REPLAY against a non-ACTIVE store. tests/all/datastore_status_guard.rs:228-233 drives "import while deleting" with a per-case FRESH token (`import_job_body` at :182-189 builds `case_token("guard-job", case)`), i.e. a genuine new submission; and tests/all/datastore_status_guard.rs:595-696 (`an_import_racing_the_activation_is_refused_and_its_token_survives`) covers only the CREATING race, where the token has not been spent yet — its closing assertions at :684-689 pin the opposite half of this contract ("The ledger entry the admitted submit wrote is LIVE, not dangling: repeating the same token names the same job rather than minting a second one").

## Steps to Reproduce

1. POST /datastore with clientToken "s1"; wait out DATASTORE_ACTIVATION_DELAY so GetDatastore reports ACTIVE.
2. POST /startDICOMImportJob/datastore/{datastoreId} with clientToken "t1" and valid dataAccessRoleArn / inputS3Uri / outputS3Uri. Observe 200 with jobId J and jobStatus SUBMITTED.
3. DELETE /datastore/{datastoreId}. It is accepted (the store holds no image sets) and reports datastoreStatus DELETING.
4. Within DATASTORE_DELETION_DELAY, re-send the identical POST /startDICOMImportJob/datastore/{datastoreId} with clientToken "t1" — the retry a client doing at-least-once delivery sends.
5. Observe HTTP 409 ConflictException ("...not ACTIVE...") instead of HTTP 200 reporting jobId J and its current jobStatus, which is what the handler's own comment at start_dicom_import_job.rs:202-204 says a token already in the ledger produces.
6. Compare with CreateDatastore: replay clientToken "s1" at any point and the ledger answers first, so the replay is never converted into an error.
