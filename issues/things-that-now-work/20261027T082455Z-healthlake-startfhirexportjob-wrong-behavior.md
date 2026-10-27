Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# StartFHIRExportJob's ClientToken replay is unreachable once the data store leaves ACTIVE, unlike every other idempotent mint in the crate

_Filed: 2026-10-27T08:24:55Z_
_Operation: StartFHIRExportJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartFHIRExportJob orders its two gates the wrong way round relative to every sibling in this crate: it resolves the data store and refuses any non-ACTIVE status BEFORE it ever looks at the recorded `ClientToken`. The handler's own comment states the replay rule unconditionally ("a repeat of the SAME call carrying a token already seen is answered out of the job that token started, so one token starts one export job"), but the code under it can only honour that rule while the data store is still ACTIVE.

Consequence: a caller that started an export job with ClientToken `t`, and then updates or deletes the data store, gets a 400 `ValidationException` ("in a transition state") or a `ResourceNotFoundException` when it repeats the IDENTICAL request under `t` — even though the job record that token minted is still held and is still reported by DescribeFHIRExportJob/ListFHIRExportJobs. The retry the token exists to make safe therefore answers an error instead of the job it started, and which answer a client gets depends purely on what happened to the data store in between.

The crate's other three idempotent mints all resolve the token FIRST and answer out of the stored record, so a repeat cannot be turned into a refusal by a state change that happened after the first call: StartDataTransformationJob replays before it even looks up the profile the job names, and both CreateFHIRDatastore and CreateDataTransformationProfile replay before their quota/content gates. StartFHIRExportJob is the one that does it the other way round.

Moving the token lookup ahead of the status gate leaves every refusal the tests pin untouched, because none of those requests ever recorded a token (a first start that names a CREATING/DELETING/DELETED/unheld data store still finds no recorded token and still falls through to the same gate).

## Evidence

The rule the handler states, then the gate that makes it unreachable — services/service-com-amazonaws-healthlake-healthlake/src/ops/start_fhir_export_job.rs:253-257:
```
        // `ClientToken` is this operation's `@idempotencyToken`: a repeat of the
        // SAME call carrying a token already seen is answered out of the job
        // that token started, so one token starts one export job. A token reused
        // with a different request is a different request, and is refused rather
        // than answered with the earlier job.
```
But the status gate runs first — start_fhir_export_job.rs:218-249:
```
            match datastore.status {
                // Only a settled, usable data store can be exported from.
                crate::state::DatastoreStatus::Active => {}
                ...
                crate::state::DatastoreStatus::Creating
                | crate::state::DatastoreStatus::Updating
                | crate::state::DatastoreStatus::Deleting => {
                    return Err(StartFHIRExportJobErrorModeled::ValidationException(
```
and only afterwards — start_fhir_export_job.rs:258-283:
```
        let fingerprint = request_fingerprint(&input);
        if let Some(started) = input
            .client_token
            .as_deref()
            .and_then(|token| store.export_job_for_client_token(token))
        {
            ...
            return Ok(StartFHIRExportJobOutput {
                job_id: started.id.clone(),
```

The sibling that does it the other way round — services/service-com-amazonaws-healthlake-healthlake/src/ops/start_data_transformation_job.rs:214-233, where the replay precedes the resource lookup:
```
        // A repeat of the SAME call under a token already seen is answered out
        // of the job that token started, so one token starts one job.
        ...
        if let Some(started) = store.transformation_job_for_client_token(&input.client_token) {
            ...
            // Answered out of the STORED record, so a repeat cannot report a
            // status the job has already left.
            return Ok(StartDataTransformationJobOutput {
```
and only then, at start_data_transformation_job.rs:239-241:
```
        let Some(profile) = store.profiles.get(&input.profile_id) else {
            return Err(not_found(crate::state::PROFILE_NOT_FOUND_MESSAGE));
        };
```
The same ordering appears in create_fhir_datastore.rs:265-290 (token replay) ahead of :295-308 (the full-store refusal) and in create_data_transformation_profile.rs:237-259 ahead of :264-282.

Test coverage, and why a fix does not disturb it: tests/all/fhir_export_job_start.rs:459-477 pins the replay only for a data store that is still ACTIVE ("a repeat of one request under one ClientToken must answer the job it started"), and tests/all/fhir_export_job_start.rs:389-445 pins the CREATING / DELETING / DELETED / unheld refusals for FIRST starts that carry no ClientToken at all (`assert_start_refused`), so no pinned case records a token before the gate is hit.

## Steps to Reproduce

1. CreateFHIRDatastore, then wait past crate::state::LIFECYCLE_TRANSITION so the data store is ACTIVE
2. StartFHIRExportJob with ClientToken "t" and some OutputDataConfig/DataAccessRoleArn — note the returned JobId J (JobStatus SUBMITTED)
3. Call UpdateFHIRDatastore on that data store (e.g. { DatastoreName: "renamed" }); the record is now UPDATING until its transition falls due
4. Immediately repeat the IDENTICAL StartFHIRExportJob request, same ClientToken "t": the response is a ValidationException carrying datastore_in_transition_message(Updating, ExportFrom) instead of the recorded job J, even though DescribeFHIRExportJob(J) still reports it
5. Variant: DeleteFHIRDatastore instead, wait past the transition, then repeat the same start under "t" — the answer is ResourceNotFoundException while the job record J is still held for the remainder of DELETED_RETENTION
6. Compare with StartDataTransformationJob: start a job under ClientToken "u", DeleteDataTransformationProfile the profile it named, then repeat the identical start — it still replays the stored job (start_data_transformation_job.rs:220-233 runs before the profile lookup at :239)
