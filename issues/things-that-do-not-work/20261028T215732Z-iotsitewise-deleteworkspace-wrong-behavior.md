Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# A bulk import job keyed to a workspace is neither a content that refuses the delete nor swept with the workspace, so it survives reporting a workspace and dataset the store no longer holds

_Filed: 2026-10-28T21:57:32Z_
_Operation: DeleteWorkspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate splits everything keyed by a workspace into exactly two kinds, and states the split in two doc comments: a "content" (a resource the caller creates inside the workspace and addresses on its own) must be gone before DeleteWorkspace is accepted, and everything else keyed off the workspace is swept away by `forget_workspace` so "a workspace this service no longer holds can leave none of them behind for a later read".

A bulk import job falls through both nets. `workspace_contents` counts only datasets, pipelines, tasks and applications, so a workspace holding a bulk import job is deleted; `forget_workspace` sweeps searches, queries, enrichment jobs, dataset export jobs, capture sources and logging options, but not `bulk_import_jobs`. The job record keeps the `workspaceName` (and `datasetId`) that CreateBulkImportJob validated against the store at create time, and `DescribeBulkImportJob` reports both members straight off the record whenever the request carries no `workspaceName` filter — so a 200 answer names a workspace nothing holds, while the same job addressed with `?workspaceName=<deleted>` is answered 404 for the missing workspace. `ListBulkImportJobs` with no filter lists the orphan too.

## Evidence

The stated split, both halves:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:5585-5594 —
    "/// These are the kinds a caller creates INSIDE a workspace and addresses as
     /// resources of their own — each with its own create and its own delete — so
     /// they are what a delete of the container has to find empty. The records
     /// this store keys by a workspace but a caller cannot CREATE one at a time
     /// (a search, a query, an enrichment job, a dataset export job, a capture
     /// source) are NOT contents in this sense: they are what running the
     /// workspace produced, and they go with it in [`Self::forget_workspace`]."
  and what it actually counts, src/state.rs:5616-5621 —
    "        [
            (RESOURCE_DATASET, datasets),
            (RESOURCE_PIPELINE, pipelines),
            (RESOURCE_TASK, tasks),
            (RESOURCE_APPLICATION, applications),
        ]"
  src/state.rs:5632-5634 — "/// Each of those is addressable ONLY inside the workspace, so a workspace
  /// this service no longer holds can leave none of them behind for a later
  /// read"
  and what it actually sweeps, src/state.rs:5645-5656 — "self.searches.retain(...); self.queries.retain(...); self.enrichment_jobs.retain(...); self.dataset_export_jobs.retain(...); self.capture_sources.remove(workspace_name); self.capture_source_aliases.remove(workspace_name); self.logging_options.remove(workspace_name);" — `bulk_import_jobs` appears in neither list.

The handler that relies on them: src/ops/delete_workspace.rs:217 "let contents = store.workspace_contents(&input.workspace_name);" and :235 "store.forget_workspace(&input.workspace_name);".

The reference the job keeps, resolved against the store at create time:
  src/ops/create_bulk_import_job.rs:235-237 — "if !store.workspaces.contains_key(workspace) { return Err(not_found(crate::state::RESOURCE_WORKSPACE, workspace)); }"
  src/ops/create_bulk_import_job.rs:309-310 — "dataset_id: input.dataset_id.clone(), workspace_name: input.workspace_name.clone(),"

The read that then reports the dangling reference:
  src/ops/describe_bulk_import_job.rs:204-208 — the workspace check runs only "if let Some(workspace) = input.workspace_name.as_deref()"
  src/ops/describe_bulk_import_job.rs:242-243 — "dataset_id: record.dataset_id.clone(), workspace_name: record.workspace_name.clone(),"

Test coverage: tests/all/workspace_delete.rs:9-17 pins the rule for "a dataset, a pipeline, a task or an application" only; a grep for `bulk_import`/`/jobs` over tests/all finds only bulk_import_jobs.rs and list_bulk_import_jobs.rs, neither of which deletes a workspace.

## Steps to Reproduce

1. CreateWorkspace W.
2. CreateDataset in W with datasetType SESSION -> D.
3. CreateBulkImportJob with workspaceName=W, datasetId=D and one file -> jobId J (both references are validated against the store at create time).
4. DeleteDataset(D) -> accepted (D holds no data segments).
5. DeleteWorkspace(W) -> accepted: workspace_contents(W) is empty because bulk import jobs are not counted.
6. GET /jobs/{J} (no workspaceName) -> 200 reporting workspaceName=W and datasetId=D, neither of which the store holds any more.
7. GET /jobs/{J}?workspaceName=W -> 404 ResourceNotFoundException for the workspace, so one job is simultaneously readable and unreachable depending on the filter.
8. GET /jobs (no filter) -> the orphaned job J is still listed.
9. Expected per state.rs:5632-5634 ("a workspace this service no longer holds can leave none of them behind for a later read") and state.rs:5585-5590 (a content must be found empty): either DeleteWorkspace is refused while a bulk import job names the workspace, or the job goes with the workspace.
