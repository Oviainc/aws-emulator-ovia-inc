Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the CleanRoomsML emulator

_Filed: 2026-10-21T21:57:56Z_
_Operation: service-com-amazonaws-cleanroomsml-awsstarkcontrolservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 59 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run. Implement a FIRST, SMALL slice: the training-dataset -> audience-model lifecycle, which is CleanRoomsML's primary resource chain (restJson1, sigv4 signing name `cleanrooms-ml`).

Happy path, in order (fields from OPERATIONS.md):

1. **CreateTrainingDataset** — `POST /training-dataset`. Input: `name` (required, 1..63), `roleArn` (required, IAM role ARN pattern), `trainingData` (required, list<Dataset>), `description`, `tags`. Must mint and PERSIST a new training dataset keyed by a generated ARN of the form `arn:aws:cleanrooms-ml:<region>:<account>:training-dataset/<name-or-id>` and return it as `trainingDatasetArn`.
2. **GetTrainingDataset** — `GET /training-dataset/{trainingDatasetArn}`. Must look the ARN up in state and return the stored `name`, `trainingData`, `roleArn`, `description`, `tags`, plus `status` = `ACTIVE` and the `createTime`/`updateTime` recorded at create. Unknown ARN -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListTrainingDatasets** — `GET /training-dataset`. Must return one `TrainingDatasetSummary` per persisted dataset (ARN, name, status, create/update times, description), honouring `maxResults` (1..100) and round-tripping `nextToken` (1..10240) so paging terminates.
4. **CreateAudienceModel** — `POST /audience-model`. Input: `name` (required), `trainingDatasetArn` (required), optional `trainingDataStartTime`/`trainingDataEndTime`, `kmsKeyArn`, `description`, `tags`. Must VERIFY the referenced training dataset exists in state (else `ResourceNotFoundException` 404), persist the audience model, and return a minted `audienceModelArn` (`...:audience-model/...`).
5. **GetAudienceModel** — `GET /audience-model/{audienceModelArn}`. Must return the persisted `name`, `trainingDatasetArn`, `trainingDataStartTime`/`EndTime`, `kmsKeyArn`, `description`, `tags`, `createTime`/`updateTime`, and `status` = `ACTIVE` (models created by the emulator settle to ACTIVE immediately). Unknown ARN -> 404.
6. **DeleteAudienceModel** then **DeleteTrainingDataset** — `DELETE /audience-model/{audienceModelArn}` and `DELETE /training-dataset/{trainingDatasetArn}`. Must remove the entry from state so the matching Get returns `ResourceNotFoundException` (404) and the List no longer includes it. Deleting a training dataset that still has a dependent audience model should fail with `ConflictException` (HTTP 409), which both ops declare.

NEVER STUBS: each of these must compute its response from persisted state. No hardcoded ARNs, no echoing the request body back without storing it, no random ARN that is not inserted into the store. A create followed by a get must return exactly what was created; a delete followed by a get must 404.

## State Model

In `src/state.rs` (`ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState`, reached via `crate::state::service_state(&*self.engine)` and `.lock()`):

- `training_datasets: BTreeMap<String /* trainingDatasetArn */, TrainingDataset>` where `TrainingDataset { arn: String, name: String, role_arn: String, training_data: Vec<Dataset>, description: Option<String>, tags: BTreeMap<String,String>, status: TrainingDatasetStatus /* ACTIVE */, create_time: DateTime, update_time: DateTime }`
- `audience_models: BTreeMap<String /* audienceModelArn */, AudienceModel>` where `AudienceModel { arn: String, name: String, training_dataset_arn: String, training_data_start_time: Option<DateTime>, training_data_end_time: Option<DateTime>, kms_key_arn: Option<String>, description: Option<String>, tags: BTreeMap<String,String>, status: AudienceModelStatus /* ACTIVE */, status_details: Option<StatusDetails>, create_time: DateTime, update_time: DateTime }`
- a small counter/uuid source used to mint the resource id segment of each ARN, and the account id/region taken from the request context so minted ARNs match the modeled patterns.

BTreeMap keying gives deterministic list ordering for pagination tokens (encode the last-seen ARN in `nextToken`).

Out of scope for this first slice and expected to keep returning 501: configured audience models, audience generation/export jobs, trained models, ML input channels, configured model algorithms (+ associations), ML configuration, collaboration-scoped reads, and the tagging APIs. Those are filed separately as per-operation gap issues.

## Evidence

`OPERATIONS.md` at the crate root documents all 59 operations with routes, members and errors: CreateTrainingDataset (`POST /training-dataset`, output `trainingDatasetArn` required), GetTrainingDataset (`GET /training-dataset/{trainingDatasetArn}`, output includes required `status` ACTIVE, `createTime`, `updateTime`, `trainingData`, `roleArn`; errors include ResourceNotFoundException 404), ListTrainingDatasets (`GET /training-dataset`, @paginated inputToken=nextToken/outputToken=nextToken/items=trainingDatasets, maxResults 1..100), CreateAudienceModel (`POST /audience-model`, required `trainingDatasetArn`, output `audienceModelArn`), GetAudienceModel (`GET /audience-model/{audienceModelArn}`, required `status` of AudienceModelStatus), DeleteAudienceModel / DeleteTrainingDataset (DELETE, errors ConflictException 409 + ResourceNotFoundException 404). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice/src/ops/` matches all 59 handler files, including create_training_dataset.rs, get_training_dataset.rs, list_training_datasets.rs, create_audience_model.rs, get_audience_model.rs, delete_audience_model.rs and delete_training_dataset.rs — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). README.md confirms the wire protocol is restJson1 and that the marker line is the stub census.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `cleanroomsml` client at it (restJson1, signing name `cleanrooms-ml`).
2. CreateTrainingDataset with name='demo-ds', roleArn='arn:aws:iam::123456789012:role/CleanRoomsMLRole', trainingData=[one Dataset entry] — expect 200 with a well-formed `trainingDatasetArn`; today: HTTP 501 NotImplemented.
3. GetTrainingDataset with the returned ARN — expect 200 echoing the stored name/roleArn/trainingData with status=ACTIVE and createTime/updateTime.
4. ListTrainingDatasets (maxResults=1) — expect the created dataset in `trainingDatasets` and a usable `nextToken` when more exist.
5. CreateAudienceModel with name='demo-model' and trainingDatasetArn from step 2 — expect 200 with a minted `audienceModelArn`; CreateAudienceModel against a bogus trainingDatasetArn must return ResourceNotFoundException (404).
6. GetAudienceModel with the returned ARN — expect 200 with trainingDatasetArn equal to step 2's ARN and status=ACTIVE.
7. DeleteAudienceModel, then DeleteTrainingDataset — expect 200 each; re-running GetAudienceModel / GetTrainingDataset must now return ResourceNotFoundException (404) and ListTrainingDatasets must no longer list the dataset.
8. Attempt DeleteTrainingDataset while an audience model still references it — expect ConflictException (409).
