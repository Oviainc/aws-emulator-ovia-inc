Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# A different CreateDatasetExportJob reusing a spent clientToken is answered 202 with the FIRST job's id and starts no export

_Filed: 2026-10-29T00:26:49Z_
_Operation: CreateDatasetExportJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateDatasetExportJob treats any request carrying a token already seen in the same workspace as a retry: the scan compares `workspace_name` and `client_token` and nothing else. `destinationS3Uri`, `input` (the processing input) and `errorReportLocation` are never compared, so a request reusing the token with a different destination or a different dataset is answered 202 with the first job's `jobId`, and the export it asked for never exists. The handler's own quoted doc — "If you retry a request that completed successfully using the same client token, the server returns the cached result from the original successful request" — covers a retry of THAT request, not a different one; the crate's generic idempotency helper classifies a different request under a spent token as `Spent`, which the siblings turn into the modelled `ConflictingOperationException` this operation also models (409).

## Evidence

Handler — services/.../src/ops/create_dataset_export_job.rs:196-212:
```
        // "If you retry a request that completed successfully using the same client
        // token, the server returns the cached result from the original successful
        // request without performing the operation again": the retry writes nothing
        // and reports the job the ORIGINAL call started. ...
        if let Some(token) = input.client_token.as_deref() {
            if let Some(existing) = store.dataset_export_jobs.values().find(|job| {
                job.workspace_name == input.workspace_name
                    && job.client_token.as_deref() == Some(token)
            }) {
                return Ok(CreateDatasetExportJobOutput {
                    job_id: existing.job_id.clone(),
                    workspace_name: existing.workspace_name.clone(),
                });
            }
        }
```
Contrast the sibling that hand-rolls the same lookup correctly — create_application.rs:229-232: "The retry has to be the SAME request, so the members that identify the application are compared too; a token already spent on a different request collides with the application it made, rather than answering from it." and the generic rule in state.rs:9338-9342 (`Replay` only when `call.request == request`, else `Spent`), restated in create_task.rs:193-198.
The operation models the error the collision needs — smithy_query CreateDatasetExportJob errors include `{"shape": "ConflictingOperationException", "httpError": 409}`.

## Steps to Reproduce

1. CreateWorkspace W and two datasets D1, D2.
2. CreateDatasetExportJob in W with input = D1, destinationS3Uri = s3://bucket/one/ and clientToken T; note jobId J1.
3. CreateDatasetExportJob in W with input = D2, destinationS3Uri = s3://bucket/two/ and the same clientToken T.
4. Observe 202 carrying jobId J1; ListDatasetExportJobs in W shows only the D1 job, so the second export was silently never created, instead of the modelled ConflictingOperationException.
