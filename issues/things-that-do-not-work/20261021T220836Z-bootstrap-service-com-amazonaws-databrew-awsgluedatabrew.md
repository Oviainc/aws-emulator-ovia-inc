Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the AWS Glue DataBrew emulator

_Filed: 2026-10-21T22:08:36Z_
_Operation: service-com-amazonaws-databrew-awsgluedatabrew | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 44 operations in `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, holding zero state. No client workflow works at all.

Implement a FIRST SMALL SLICE: the DataBrew core loop of dataset -> recipe job -> job run, served statefully over `restJson1` (SDK id `DataBrew`, signing name `databrew`). Operations outside this slice stay 501 and are filed separately later.

### Happy path to implement, in order

1. **CreateDataset** — `POST /datasets`. Input: `Name` (required, 1..255), `Input` (required, `Input` structure, e.g. `S3InputDefinition{Bucket, Key}`), `Format` (`CSV|EXCEL|JSON|ORC|PARQUET`), `FormatOptions`, `PathOptions`, `Tags`. Output: `Name`. Must INSERT a dataset record; a second create of the same name must fail `ConflictException` (409). Missing/blank `Name` or missing `Input` => `ValidationException` (400).
2. **DescribeDataset** — `GET /datasets/{Name}`. Must return the stored record: `Name`, `Input`, `Format`, `FormatOptions`, `PathOptions`, `Tags`, plus server-derived `Source` (`S3` when the input is an S3 definition), `CreateDate`, `CreatedBy`, `LastModifiedDate`, `LastModifiedBy`, `ResourceArn` (`arn:aws:databrew:<region>:<account>:dataset/<Name>`). Unknown name => `ResourceNotFoundException` (404).
3. **ListDatasets** — `GET /datasets`. Output `Datasets` (required list of `Dataset`) built by iterating the stored map — never a canned/empty list. Honour `maxResults` (1..100) and `nextToken` (opaque token that resumes where the previous page ended; echoing back an unrelated token or ignoring it is a failure).
4. **CreateRecipeJob** — `POST /recipeJobs`. Input: `Name` (required, 1..240), `RoleArn` (required), `MaxCapacity`, `MaxRetries`, `Timeout` (defaults 0), `DatasetName`, `Outputs` (list of `Output` with `Location` = `S3Location{Bucket, Key}`), `RecipeReference` (`{RecipeName, RecipeVersion}`), `ProjectName`, `EncryptionMode`, `LogSubscription`, `Tags`. Output: `Name`. Must store a job with `Type = RECIPE`. A `DatasetName` that does not exist in state => `ResourceNotFoundException` (404); duplicate job name => `ConflictException` (409).
5. **StartJobRun** — `POST /jobs/{Name}/startJobRun`. Output: `RunId` (required). Must MINT a new run id, persist a job-run record attached to that job with `State`, `StartedOn`, `JobName`, `DatasetName`, `Outputs`, `RecipeReference` copied from the stored job, and return that same id. A random id that is not stored (so `DescribeJobRun` 404s afterwards) is a failure. Unknown job => `ResourceNotFoundException` (404).
6. **DescribeJobRun** — `GET /jobs/{Name}/jobRun/{RunId}`. Must return the persisted run: `JobName`, `RunId`, `State` (`STARTING`/`RUNNING`/`SUCCEEDED` — DataBrew runs progress and reach a terminal state; a simple deterministic progression on read-after-start is acceptable, but the value must come from the stored run, not a constant literal), `Attempt`, `ExecutionTime`, `StartedOn`, `StartedBy`, `CompletedOn` when terminal, `DatasetName`, `Outputs`, `RecipeReference`, `LogSubscription`. Unknown `Name`/`RunId` => `ResourceNotFoundException` (404). `ListJobRuns` (`GET /jobs/{Name}/jobRuns`) should fall out of the same storage.
7. **DeleteDataset** — `DELETE /datasets/{Name}`. Must REMOVE the record and return `Name`; a following `DescribeDataset` must then 404 and `ListDatasets` must no longer include it. Unknown name => `ResourceNotFoundException` (404).

### Rules

- NEVER STUBS: every one of these must compute its response from persisted state held in the single shared engine, reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). No hardcoded returns, no echoed inputs, no random id without storage.
- Names are the primary keys (DataBrew addresses datasets/jobs/runs by name), scoped per account/region.
- Timestamps and `CreatedBy`/`LastModifiedBy`/`StartedBy` are server-generated, not client-supplied.
- Errors must use the modeled shapes and statuses listed above, not generic 500s.

## State Model

In `src/state.rs`, on `ComAmazonawsDatabrewAwsgluedatabrewState`:

- `datasets: BTreeMap<String /*Name*/, Dataset>` where `Dataset { name, format: Option<InputFormat>, format_options: Option<FormatOptions>, input: Input, path_options: Option<PathOptions>, source: Option<Source>, tags: BTreeMap<String,String>, resource_arn: String, create_date: SystemTime, created_by: String, last_modified_date: SystemTime, last_modified_by: String }`
- `jobs: BTreeMap<String /*Name*/, Job>` where `Job { name, job_type: JobType /*RECIPE|PROFILE*/, dataset_name: Option<String>, project_name: Option<String>, recipe_reference: Option<RecipeReference>, outputs: Vec<Output>, role_arn, max_capacity: i32, max_retries: i32, timeout: i32, encryption_mode, encryption_key_arn, log_subscription, tags, resource_arn, create_date, created_by, last_modified_date, last_modified_by }`
- `job_runs: BTreeMap<(String /*JobName*/, String /*RunId*/), JobRun>` where `JobRun { run_id, job_name, dataset_name, state: JobRunState, attempt: i32, execution_time: i32, started_on, started_by, completed_on: Option<..>, error_message: Option<String>, outputs: Vec<Output>, recipe_reference, log_subscription, log_group_name }`
- `next_run_seq: u64` — monotonic counter used to mint unique `RunId` values stored in `job_runs`.

Later slices add recipes/recipe versions (`recipes: BTreeMap<String, Vec<RecipeVersionRecord>>`), projects + sessions, rulesets, schedules, profile jobs and tagging; the keys above are chosen so those drop in alongside.

Done = an integration script running steps 1-7 passes with correct, stateful bodies — not merely 2xx.

## Evidence

Spec: `services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json` as denormalized in the crate's OPERATIONS.md: CreateDataset `POST /datasets` (Input/Name required, errors Conflict 409 / Validation 400 / ServiceQuotaExceeded 402 / AccessDenied 403, line 33-38); DescribeDataset `GET /datasets/{Name}` returning CreateDate/CreatedBy/Source/ResourceArn, ResourceNotFound 404 (line 124-129); ListDatasets `GET /datasets` with @paginated(inputToken=NextToken, outputToken=NextToken, items=Datasets) and maxResults range 1..100 (line 173-179); CreateRecipeJob `POST /recipeJobs` with Outputs/RecipeReference/RoleArn, ResourceNotFound 404 (line 61-66); StartJobRun `POST /jobs/{Name}/startJobRun` -> required RunId (line 258-263); DescribeJobRun `GET /jobs/{Name}/jobRun/{RunId}` -> State enum FAILED|RUNNING|STARTING|STOPPED|STOPPING|SUCCEEDED|TIMEOUT, Attempt, ExecutionTime, StartedOn/CompletedOn (line 138-143); DeleteDataset `DELETE /datasets/{Name}` -> Name, ResourceNotFound 404 (line 82-87). Protocol restJson1 / signing name `databrew` (OPERATIONS.md line 8-14).

Implementation: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/` matches all 44 handler files (e.g. create_dataset.rs, describe_dataset.rs, list_datasets.rs, create_recipe_job.rs, start_job_run.rs, describe_job_run.rs, delete_dataset.rs), each returning `Scaffolded::not_implemented(..)` => HTTP 501. No handler touches `crate::state`; the only test present is the generated `tests/state_roundtrip.rs` example, so there is no behavioral coverage of any operation.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI databrew client at it.
2. `aws databrew create-dataset --name demo-ds --input '{"S3InputDefinition":{"Bucket":"my-bucket","Key":"in/data.csv"}}' --format CSV` — expect 200 with `{"Name":"demo-ds"}`; today returns HTTP 501 NotImplemented.
3. `aws databrew describe-dataset --name demo-ds` — expect the stored Input/Format back plus Source=S3, CreateDate, ResourceArn.
4. `aws databrew list-datasets --max-results 1` — expect `Datasets` to contain demo-ds, derived from state.
5. `aws databrew create-recipe-job --name demo-job --role-arn arn:aws:iam::123456789012:role/brew --dataset-name demo-ds --outputs '[{"Location":{"Bucket":"my-bucket","Key":"out/"}}]'` — expect 200 `{"Name":"demo-job"}`; with `--dataset-name missing-ds` expect ResourceNotFoundException (404).
6. `aws databrew start-job-run --name demo-job` — expect 200 with a fresh `RunId`.
7. `aws databrew describe-job-run --name demo-job --run-id <RunId>` — expect the persisted run: JobName=demo-job, DatasetName=demo-ds, a JobRunState value, StartedOn, and the Outputs copied from the job.
8. `aws databrew list-job-runs --name demo-job` — expect the run to appear.
9. `aws databrew delete-dataset --name demo-ds` then `describe-dataset --name demo-ds` — expect 200 then ResourceNotFoundException (404); `list-datasets` no longer lists it.
10. `aws databrew describe-dataset --name never-created` — expect ResourceNotFoundException (404), not 501.
