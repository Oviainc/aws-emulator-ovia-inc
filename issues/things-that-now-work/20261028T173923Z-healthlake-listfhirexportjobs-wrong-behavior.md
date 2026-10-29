Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 145e568e7ef9b3115be9abe08039cd724717eead

# SubmittedBefore/SubmittedAfter are read as inclusive cutoffs, although the modelled members say "before"/"after" (the model spells the inclusive reading "at or before" elsewhere in this same service)

_Filed: 2026-10-28T17:39:23Z_
_Operation: ListFHIRExportJobs | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `145e568e7ef9b3115be9abe08039cd724717eead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListFHIRExportJobs` filters on `SubmitTime` with both documented cutoffs treated as INCLUSIVE: a job whose `SubmitTime` is exactly the instant the request named is reported for `SubmittedBefore` and again for `SubmittedAfter`. The modelled members say "submitted before a user-specified date" and "submitted after a user-specified date" — a job submitted AT the named instant is neither before nor after it.

That the distinction is meant is settled inside this same model: `ListDataTransformationJobsRequest$SubmittedBefore` is documented as "jobs submitted at or before this timestamp" and `$SubmittedAfter` as "at or after". The model therefore spells the inclusive reading explicitly when it wants it, and the FHIR job listings do not get that spelling. The handler nevertheless gives the FHIR pair the inclusive reading, and its own doc comment states the reason as internal consistency with another member rather than as the member's own documentation.

Effect on a caller: a half-open window built from two adjacent calls (`SubmittedBefore: T` then `SubmittedAfter: T`) reports the job submitted exactly at `T` in BOTH pages, and a caller asking for jobs strictly before a cutoff gets one it excluded.

The same rule is copied verbatim into `ListFHIRImportJobs` (`src/ops/list_fhir_import_jobs.rs:286-295`, whose `SubmittedBefore` is likewise documented as "import jobs submitted before a user-specified date") and into `ListFHIRDatastores`' `Filter.CreatedBefore` / `CreatedAfter` (`src/ops/list_fhir_datastores.rs:238-248`, documented as "All data stores created before/after the specified date are included"), so a fix should settle the FHIR-side readings together; they are not filed separately.

## Evidence

MODEL — `smithy_query(query="doc", shape="ListFHIRExportJobsRequest", member="SubmittedBefore")`:
"Limits the response to FHIR export jobs submitted before a user- specified date."
and `member="SubmittedAfter"`:
"Limits the response to FHIR export jobs submitted after a user-specified date."

MODEL, the same service spelling the INCLUSIVE reading when it means it — `smithy_query(query="doc", shape="ListDataTransformationJobsRequest", member="SubmittedBefore")`:
"Filters the results to include only jobs submitted at or before this timestamp."

HANDLER — services/service-com-amazonaws-healthlake-healthlake/src/ops/list_fhir_export_jobs.rs:286-295 (equality is admitted on both sides):
```
    let submitted = crate::state::epoch_nanos(&job.submitted_at);
    if let Some(before) = input.submitted_before.as_ref() {
        if submitted > crate::state::epoch_nanos(before) {
            return false;
        }
    }
    if let Some(after) = input.submitted_after.as_ref() {
        if submitted < crate::state::epoch_nanos(after) {
            return false;
        }
    }
```

HANDLER DOC stating the chosen reading and its reason — src/ops/list_fhir_export_jobs.rs:263-271:
```
/// Each member narrows the listing on its own and all of them together:
/// `JobName` and `JobStatus` match exactly (the model: "limits the response to
/// the export job with the specified job name" / "with the specified job
/// status"), and `SubmittedBefore` / `SubmittedAfter` are INCLUSIVE cutoffs on
/// `SubmitTime` — the same reading `ListFHIRDatastores` gives its own
/// `CreatedBefore` / `CreatedAfter` pair, so one data store's two listings
/// cannot disagree about whether a boundary instant is in the window.
```
(The quoted reason is internal consistency with `ListFHIRDatastores`, whose own modelled docs also say "created before/after the specified date" — so neither member's documentation supports the inclusive reading.)

TEST that pins the current behaviour (no reason tied to the member's documentation) — tests/all/fhir_export_job_listing.rs:365-374:
```
        (
            "SubmittedBefore exactly on a submit instant is inclusive",
            json!({ "SubmittedBefore": BASE_SECS + 20 }),
            vec!["job-alpha", "job-bravo"],
        ),
```

SIBLING with the identical copied rule — src/ops/list_fhir_import_jobs.rs:286-295, and src/ops/list_fhir_datastores.rs:238-248.

## Steps to Reproduce

1. Create a data store and wait for it to become ACTIVE.
2. Start a FHIR export job on it and note the `SubmitTime` T reported by `DescribeFHIRExportJob`.
3. Call `ListFHIRExportJobs` with `DatastoreId` and `SubmittedBefore: T`; observe the job is reported, although the modelled member says the response is limited to jobs "submitted before a user-specified date".
4. Call `ListFHIRExportJobs` with `SubmittedAfter: T`; observe the same job reported again, so the two adjacent half-open windows both contain it.
5. Compare with `ListDataTransformationJobsRequest$SubmittedBefore`, which the same model documents as "at or before this timestamp" — the spelling the FHIR members do not carry.
