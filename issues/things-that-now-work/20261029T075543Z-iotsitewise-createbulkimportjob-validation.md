Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 97cefcb07ab7801eeb37fff053a06a3892425e6e

# CreateBulkImportJob accepts a CURATED or EXTERNAL dataset as its ingest destination, though the member it reads documents the destination as the SESSION dataset

_Filed: 2026-10-29T07:55:43Z_
_Operation: CreateBulkImportJob | Class: validation_

> _Note: this issue was identified against the code base at commit `97cefcb07ab7801eeb37fff053a06a3892425e6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateBulkImportJob's `datasetId`/`workspaceName` pair names "the session dataset to ingest data into". The handler resolves that pair to a stored dataset and checks exactly two things: that the workspace exists, and that the dataset belongs to that workspace. It never looks at the dataset's `datasetType`, so a CURATED dataset (or an EXTERNAL one) created in that same workspace is accepted as a bulk-import destination and a job is stored against it — a destination whose data, by this crate's own rule, belongs to the source session datasets it was selected from and not to that dataset at all.

The three sibling handlers in this crate that address a dataset's data segments all hold the dataset to the kind their own doc names, with the same vocabulary and the same `Option::filter` shape (a dataset the store holds no type for is deliberately NOT refused, because `CreateDataset` keeps `datasetType` absent rather than guessing one). CreateBulkImportJob is the one data-path entry point that skips the check, although the member it reads is the one that says "session dataset".

## Evidence

The model sentence the handler is implementing, smithy_query(query="doc", shape="CreateBulkImportJobRequest", member="datasetId"):
  "The ID of the session dataset to ingest data into. Specify this field, together with workspaceName, to ingest data into a session dataset in a workspace."

The handler quotes that very sentence and then checks only existence and workspace membership — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_bulk_import_job.rs:209-249:
    "// A job ingests either into a session dataset of a workspace or into time
     // series outside one, and the two members that name the dataset form ONE
     // destination: `datasetId` is \"the ID of the session dataset to ingest data
     // into. Specify this field, together with workspaceName\" ..."
  ...
    "                if store
                    .datasets
                    .get(dataset_id)
                    .is_none_or(|dataset| dataset.workspace_name.as_deref() != Some(workspace))
                {
                    return Err(not_found(crate::state::RESOURCE_DATASET, dataset_id));
                }"
  — and nothing in the file mentions `dataset_type` at all (grep for `dataset_type` in src/ops/create_bulk_import_job.rs: no matches).

The siblings of this crate that DO enforce the documented dataset kind:
  src/ops/batch_delete_dataset_data_segments.rs:265-279 — "// This operation \"deletes a batch of data segments from a SESSION dataset\", and deleting one \"deletes the underlying time series data for the segment's time range\" ... A dataset the store holds no type for states which side of that rule it falls on, so it is not refused here — `CreateDataset` keeps `datasetType` absent rather than guessing one." followed by "if let Some(dataset_type) = dataset_type.filter(|held| held != DATASET_TYPE_SESSION) { return Err(invalid_request(format!(\"Data segments can be deleted only from a {DATASET_TYPE_SESSION} dataset, but the dataset {} is a {dataset_type} dataset.\", input.dataset_id))); }"
  src/ops/batch_associate_data_segments_to_dataset.rs:367-382 — "// \"Data segments are time-bounded slices of time series data selected from source SESSION datasets\", so a dataset of any other type holds nothing an association can select." followed by "if let Some(source_type) = source_type.filter(|held| held != DATASET_TYPE_SESSION) { ... }"
  src/ops/batch_associate_data_segments_to_dataset.rs:303 and src/ops/batch_disassociate_data_segments_from_dataset.rs:296 — the same arm holding the HOLDING dataset to `DATASET_TYPE_CURATED`.

Test coverage: tests/all/bulk_import_jobs.rs:707-765 is the refusal table for this create and covers only absences — "a workspace with no dataset beside it", "a dataset with no workspace beside it", "a workspace this store does not hold", "a dataset this store does not hold", "a dataset another workspace contains", "a dataset no workspace contains", "one file past the documented quota". No row names a dataset of the wrong KIND. The datasets those tests create omit `datasetType` altogether (tests/all/bulk_import_jobs.rs:149-165), so they carry no stored type and the sibling-shaped check would leave every existing row passing.

## Steps to Reproduce

1. CreateWorkspace W.
2. CreateDataset in W with {"datasetName": "curated", "datasetType": "CURATED", "workspaceName": W, "datasetSource": {...}} -> 202, dataset D.
3. PutStorageConfiguration is not needed: the dataset branch is taken.
4. POST /jobs with {"jobName": ..., "jobRoleArn": ..., "files": [...], "errorReportLocation": {...}, "jobConfiguration": {...}, "workspaceName": W, "datasetId": D}.
5. Observed: 202 and a stored bulk import job whose destination is a CURATED dataset; DescribeBulkImportJob reports that datasetId back.
6. Expected, per the member's own documentation ("the ID of the session dataset to ingest data into") and the rule the three sibling data-segment handlers apply to the same resource class (src/ops/batch_delete_dataset_data_segments.rs:273-279): a dataset whose stored datasetType is present and is not SESSION is refused with InvalidRequestException, while a dataset with no stored type stays accepted.
