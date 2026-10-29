Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# The store-cap quota refusal is written out twice, once per Start operation, while every other rule the two share is single-homed in state.rs

_Filed: 2026-10-29T04:14:12Z_
_Operation: StartSearchJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both `Start` operations of this crate implement the SAME rule in full, independently: "when the store already holds its maximum number of records, refuse with ServiceQuotaExceededException whose five members are the cap sentence, the emulator account id, the record's resource type, the request's signing name and a `<Kind>JobsPerAccount` quota code". The predicate is duplicated too (`is_full` and `export_jobs_full` are two copies of `self.<map>.len() >= MAX_*` in state.rs).

This is the only rule the two `Start` operations share that is NOT single-homed. state.rs centralises every other one explicitly and says so: the tag admission rule (`check_tag_map`), the ClientToken replay rule (`client_token_replay`), the owed-status settle rule (`settle_owed_statuses`), the identifier mint (`fresh_identifier`) and the ARN-to-tag-set resolution (`taggable_record`) each exist once and are parameterised per record type. The quota refusal is the copy left over, so the two can drift: a change to which member carries the resource id, to the `quota_code` convention, or to the order of the cap check relative to the tag check has to be made twice and nothing fails if only one copy is edited.

The fix is the shape the crate already uses elsewhere — one state.rs helper parameterised by (cap, resource type, quota code, the job-kind word), with each handler lifting the returned `ServiceQuotaExceededException` into its own error channel the way `check_tag_map(..)?` already does. Every byte of both responses, both cap constants and both existing tests stay exactly as they are.

## Evidence

Copy 1 — src/ops/start_search_job.rs:257-276:
```
        // This store holds its jobs for the life of the process and only this operation
        // adds one, so it is capped: past the cap the service's own quota error is the
        // honest answer (`crate::state::MAX_SEARCH_JOBS`).
        if state.is_full() {
            return Err(StartSearchJobErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this account already holds the maximum of {} search jobs",
                        crate::state::MAX_SEARCH_JOBS
                    ),
                    resource_id: ::engine::emulator_account_id().to_string(),
                    resource_type: SEARCH_JOB_RESOURCE.to_string(),
                    service_code: ctx.identity().signing_name().to_string(),
                    // AWS publishes no quota code for this limit, which it documents as
                    // adjustable; this names the quota the emulator enforces.
                    quota_code: "SearchJobsPerAccount".to_string(),
```

Copy 2 — src/ops/start_search_result_export_job.rs:351-372:
```
        // This store holds its export jobs for the life of the process and only this
        // operation adds one, so it is capped: past the cap the service's own quota error
        // is the honest answer (`crate::state::MAX_EXPORT_JOBS`).
        if state.export_jobs_full() {
            return Err(
                StartSearchResultExportJobErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "this account already holds the maximum of {} export jobs",
                            crate::state::MAX_EXPORT_JOBS
                        ),
                        resource_id: ::engine::emulator_account_id().to_string(),
                        resource_type: EXPORT_JOB_RESOURCE.to_string(),
                        service_code: ctx.identity().signing_name().to_string(),
                        // AWS publishes no quota code for this limit; this names the
                        // quota the emulator enforces.
                        quota_code: "ExportJobsPerAccount".to_string(),
```

The duplicated predicate — src/state.rs:1458-1460 and src/state.rs:1520-1522:
```
    pub fn is_full(&self) -> bool {
        self.search_jobs.len() >= MAX_SEARCH_JOBS
    }
```
```
    pub fn export_jobs_full(&self) -> bool {
        self.export_jobs.len() >= MAX_EXPORT_JOBS
    }
```

The crate's own single-home principle for the rules these two operations share — src/state.rs:2315-2318 (`check_tag_map`):
```
/// This is the one admission rule every operation that writes a tag applies — the two
/// `Start` operations minting a record's tag set and `TagResource` merging into a stored
/// one — so the per-entry check, the count that is checked, the order of the two checks
/// and the refusal channel are decided here once instead of per handler.
```
and src/state.rs:471-476 (`client_token_replay`):
```
/// One rule behind both `Start` operations of this service — the model states the same
/// window sentence on both `ClientToken` members — so which step runs first, what a
/// non-matching fingerprint answers and whether a closed window falls through are decided
/// here once instead of per handler.
```

Test coverage of both copies (both must keep passing unchanged):
- tests/all/search_jobs.rs:1399-1454 `starting_a_job_past_the_store_cap_is_refused_with_the_quota_error` — asserts `past_the_cap.status == 402` and `error_code == Some("ServiceQuotaExceededException")`.
- tests/all/export_jobs.rs:822-889 `starting_an_export_past_the_store_cap_is_refused_with_the_quota_error` — the same two assertions for the export store.

## Steps to Reproduce

1. Open services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/start_search_job.rs and read lines 257-276: the `state.is_full()` branch builds a ServiceQuotaExceededException from the cap sentence, the emulator account id, SEARCH_JOB_RESOURCE, the signing name and the quota code.
2. Open src/ops/start_search_result_export_job.rs and read lines 351-372: the `state.export_jobs_full()` branch builds the same five members the same way, differing only in the cap constant, the job-kind word, EXPORT_JOB_RESOURCE and the quota code — i.e. in its parameters, not in its rule.
3. Open src/state.rs:1458-1460 and 1520-1522: `is_full` and `export_jobs_full` are two copies of `len() >= cap`.
4. Compare with src/state.rs:2315-2318 and 471-476, where the crate states that every other rule shared by the two `Start` operations is "decided here once instead of per handler" (check_tag_map, client_token_replay) — and with settle_owed_statuses (src/state.rs:398-400) and taggable_record (src/state.rs:1341-1348), which say the same of the settle and ARN-resolution rules.
5. Confirm both copies are pinned: run tests/all/search_jobs.rs `starting_a_job_past_the_store_cap_is_refused_with_the_quota_error` and tests/all/export_jobs.rs `starting_an_export_past_the_store_cap_is_refused_with_the_quota_error` — a single parameterised helper must leave both green and both response bodies byte-identical.
