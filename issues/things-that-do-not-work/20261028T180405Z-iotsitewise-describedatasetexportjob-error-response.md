Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# DescribeDatasetExportJob skips the workspace-existence check its sibling DescribeEnrichmentJob states as the rule for every workspace-scoped job read

_Filed: 2026-10-28T18:04:05Z_
_Operation: DescribeDatasetExportJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeDatasetExportJob builds the `(workspaceName, jobId)` store key and looks the record up directly, with no check that the named workspace exists. A request naming a workspace this service holds no record of is therefore reported as an absent dataset export JOB rather than as the absent workspace. Every other workspace-scoped job handler in this crate — DescribeEnrichmentJob, ListDatasetExportJobs, ListEnrichmentJobs, CancelEnrichmentJob, CreateDatasetExportJob — resolves the workspace FIRST, and their comments state that ordering as the crate-wide rule ("Resolved FIRST, as `CreateEnrichmentJob` and `ListEnrichmentJobs` resolve it, so a request that names neither a held workspace nor a stored job reports the outer resource its path names"). The read that is the direct sibling of ListDatasetExportJobs is the one that skips it.

## Evidence

The rule, stated by the same-shaped read of the sibling job kind — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_enrichment_job.rs:212-222:

    // A job is addressed WITHIN a workspace, so the workspace the request names
    // has to exist: a name this service does not hold addresses no job at all.
    // Resolved FIRST, as `CreateEnrichmentJob` and `ListEnrichmentJobs` resolve
    // it, so a request that names neither a held workspace nor a stored job
    // reports the outer resource its path names.
    if !store.workspaces.contains_key(&input.workspace_name) {
        return Err(not_found(crate::state::not_found_message(
            crate::state::RESOURCE_WORKSPACE,
            &input.workspace_name,
        )));
    }

And by this operation's own listing — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_dataset_export_jobs.rs:172-185: "The workspace registry is the one place that says which workspaces exist — the create resolves a job's workspace there too, so no job can be stored under a workspace the registry does not hold. Resolved BEFORE the filter and the paging arguments are read, as every handler in this crate resolves the resource its path names first", followed by `if !store.workspaces.contains_key(&input.workspace_name) { return Err(not_found(... RESOURCE_WORKSPACE ...`. The create agrees: create_dataset_export_job.rs:183 `if !store.workspaces.contains_key(&input.workspace_name) {`.

What the read does instead — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_dataset_export_job.rs:173-181:

    let key = crate::state::dataset_export_job_key(&input.workspace_name, &input.job_id);
    // A read, so nothing here writes: ...
    let store = state.lock();
    let record = store
        .dataset_export_jobs
        .get(&key)
        .ok_or_else(|| not_found(&input.job_id))?;

where `not_found` is documented at describe_dataset_export_job.rs:196-205 as "naming the job the request addressed" (`RESOURCE_DATASET_EXPORT_JOB`). The file contains no `workspaces.contains_key`.

Test coverage: tests/all/dataset_export_jobs.rs:446-496 (`a_job_is_addressed_within_the_workspace_that_holds_it`) asserts only `status == 404` and `error_code(&body) == "ResourceNotFoundException"` for the "a workspace no job lives in" row, never the message or the named resource, so resolving the workspace first leaves it passing.

## Steps to Reproduce

1. CreateWorkspace `floor-1`.
2. CreateDatasetExportJob in `floor-1` and keep the returned `jobId`.
3. Call DescribeDatasetExportJob with `workspaceName=never-created` and that `jobId`: the refusal names the dataset export job as the absent resource, although the workspace is what the request got wrong.
4. Call DescribeEnrichmentJob (or ListDatasetExportJobs) with `workspaceName=never-created`: the refusal names the WORKSPACE — the two workspace-scoped job reads disagree.
