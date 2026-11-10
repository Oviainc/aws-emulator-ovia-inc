Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-import-from-s3-then-export-round-trips-asset
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-import-from-s3-then-export-round-trips-asset
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|GetJob|2xx->2xx|$.State
Issue-Key: dataexchange|ListRevisionAssets|2xx->2xx|$.Assets[*]
Issue-Commit: d9137e240d6d86ce2d0eba5736851daf32b7e9eb

# DataExchange IMPORT_ASSETS_FROM_S3 job fails instead of completing and never materializes the imported asset

> _Note: this issue was identified against the code base at commit `d9137e240d6d86ce2d0eba5736851daf32b7e9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
Two causes, both in the DataExchange import-job pipeline: a started IMPORT_ASSETS_FROM_S3 job terminates in the ERROR state rather than COMPLETED, and because the job never performs its work the target revision stays empty, so the imported asset is never visible through ListRevisionAssets. One shared fault — the import job executor does not actually import — surfaces in both the job-state projection and the revision's asset collection.

Cause 1 of 2: GetJob reports State=ERROR for a valid, started IMPORT_ASSETS_FROM_S3 job.
AWS: Smithy: GetJob -> GET /v1/jobs/{JobId}, 200, output member State targets com.amazonaws.dataexchange#State with values WAITING | IN_PROGRESS | ERROR | COMPLETED | CANCELLED | TIMED_OUT; StartJob (PATCH /v1/jobs/{JobId}, 202) moves a WAITING job into IN_PROGRESS. On AWS, an import job whose Details name an existing S3 source object and an existing data set/revision runs to State=COMPLETED, and GetJob's `Errors` (ListOfJobError) stays absent; ERROR is reserved for a job that genuinely failed, and AWS then populates Errors[] with a JobError carrying the required Code and Message members explaining the failure. Recorded behavior: the fixture polls GetJob until a terminal state and the recording's terminal state is COMPLETED.
Emulator: GetJob answers HTTP 200 but with $.State = "ERROR". The replay poller gives up because ERROR is a fail_on state, after 2 attempts (expected 'COMPLETED', got 'ERROR'). The emulator supplies no explanation for the failure either — nothing in the response identifies a cause the way AWS's Errors[].Code / Errors[].Message would.
Why: a client that creates a job, starts it and polls GetJob for COMPLETED either loops forever or aborts with a spurious failure, and every step downstream of the import (asset listing, export of the imported asset) is unreachable. Code that branches on State=="ERROR" to report an import failure reports a failure that did not happen, and gets no Errors[] entry to report with it.
Fix: the DataExchange job state machine / StartJob-driven job executor and the state it writes — the job must transition WAITING -> IN_PROGRESS -> COMPLETED when its import Details resolve, and only reach ERROR with a populated Errors[] JobError (Code, Message) when they do not.
Traces: dataexchange-import-from-s3-then-export-round-trips-asset#task#7.

Cause 2 of 2: ListRevisionAssets returns an empty Assets list after an import job, so the imported asset is never created.
AWS: Smithy: ListRevisionAssets -> GET /v1/data-sets/{DataSetId}/revisions/{RevisionId}/assets, 200, output Assets targets ListOfAssetEntry. AssetEntry requires Id, Arn, Name, AssetType, AssetDetails, DataSetId, RevisionId, CreatedAt and UpdatedAt; its Name doc states "When importing from Amazon S3, the Amazon S3 object key is used as the asset name". So a completed S3 import adds one AssetEntry per imported object, with Name = the source S3 object key, AssetType = S3_SNAPSHOT, DataSetId/RevisionId = the import target, and AssetDetails.S3SnapshotAsset.Size = the object's byte length (Id/Arn/CreatedAt/UpdatedAt are generated per environment and are not requirements).
Emulator: ListRevisionAssets answers HTTP 200 but $.Assets[0] is missing (got None) — the list is empty, so no asset entry was ever created for the imported object.
Why: a client that imports an object into a revision and then enumerates the revision sees nothing, so it cannot find the asset's Id to finalize the revision, export it, or reference it. Any workflow that round-trips data through DataExchange silently loses the data instead of failing loudly.
Fix: the import-job executor's state mutation — a completing IMPORT_ASSETS_FROM_S3 job must insert an AssetEntry into the target revision's asset collection (keyed by DataSetId/RevisionId) with Name taken from the request's S3 object key, AssetType S3_SNAPSHOT and AssetDetails.S3SnapshotAsset.Size from the source object — and ListRevisionAssets' handler must read that collection.
Traces: dataexchange-import-from-s3-then-export-round-trips-asset#task#8.

Neither cause is covered by an existing open issue: the open DataExchange issues cover DeleteAsset/GetAsset/UpdateAsset stubs (20261031T181820Z-*), CreateDataGrant validation (20261104T113707Z-*, 20261109T135251Z-*) and CancelJob's missing Content-Length (20261108T180836Z-dataexchange-canceljob-204-response-omits-content-length-0.md). The s3 GetObject failure later in the same fixture (task#12) is already owned by 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md and is not filed here.

## Evidence
Replay of fixture dataexchange-import-from-s3-then-export-round-trips-asset (trace set simparity-wire-aws-v1):

Cause 1 — step task#7, GetJob, divergence_key service=dataexchange|GetJob|status=200->200|keys=$.State
  expected_status 200, actual_status 200
  detail: "$.State: value_mismatch (expected 'COMPLETED', got 'ERROR')"
           "$.State: poll: 'ERROR' is in fail_on after 2 attempt(s) (expected 'COMPLETED', got 'ERROR')"
  Expected: State is one of the Smithy State enum values and reaches COMPLETED for a successful import; Errors[] absent.
  Actual:   State = ERROR, with no Errors[] JobError (Code/Message) accompanying it.

Cause 2 — step task#8, ListRevisionAssets, divergence_key service=dataexchange|ListRevisionAssets|status=200->200|keys=$.Assets[0]
  expected_status 200, actual_status 200
  detail: "$.Assets[0]: missing (expected {'Id': '<shaped-not-asserted>', 'Arn': '<shaped-not-asserted>', 'Name': 'source/orders.csv', 'CreatedAt': '<shaped-not-asserted>', 'UpdatedAt': '<shaped-not-asserted>', 'DataSetId': '<data set id>', 'RevisionId': '<revision id>', 'AssetType': 'S3_SNAPSHOT', 'AssetDetails': {'S3SnapshotAsset': {'Size': 53}}}, got None)"
  Expected: exactly one AssetEntry for the imported object — Name = the request's S3 object key, AssetType = S3_SNAPSHOT, DataSetId/RevisionId = the import target, AssetDetails.S3SnapshotAsset.Size = the source object's byte length. Id/Arn/CreatedAt/UpdatedAt are environment-generated and only shape-asserted; the ids and the size 53 in the recording are that run's values, not requirements.
  Actual:   Assets is empty — $.Assets[0] absent.

Model references used: GetJob/ListRevisionAssets/StartJob/CreateJob operation signatures and the State, AssetEntry and JobError shapes from services/service-com-amazonaws-dataexchange-dataexchange/smithy-model/com-amazonaws-dataexchange-dataexchange.json.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay fixture dataexchange-import-from-s3-then-export-round-trips-asset from trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: at step task#7 the fixture polls GET /v1/jobs/{JobId} (GetJob) for the IMPORT_ASSETS_FROM_S3 job created by CreateJob and started by StartJob, waiting for State=COMPLETED. Observe the response is 200 with $.State="ERROR" (a fail_on state), so the poll aborts after 2 attempts, with no Errors[] entry explaining the failure.
4. Cause 2: at step task#8 the fixture calls GET /v1/data-sets/{DataSetId}/revisions/{RevisionId}/assets (ListRevisionAssets) for the import's target revision. Observe 200 with an empty Assets list: no AssetEntry named after the imported S3 object key was created.
5. After a fix, re-replay both steps: task#7 must reach State=COMPLETED and task#8 must return one AssetEntry whose Name is the imported object key, AssetType S3_SNAPSHOT and AssetDetails.S3SnapshotAsset.Size the source object's length.
