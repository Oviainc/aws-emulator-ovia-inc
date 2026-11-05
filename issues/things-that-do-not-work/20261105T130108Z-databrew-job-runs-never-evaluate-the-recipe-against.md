Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing|run#29|AssertionError|raise AssertionError(
Issue-Commit: fa425f252fef6a08c1668b343fb18c9fbceb6346

# DataBrew job runs never evaluate the recipe against the dataset's schema: a step naming a column the dataset lacks ends SUCCEEDED with no ErrorMessage instead of FAILED with a RecipeStepError

> _Note: this issue was identified against the code base at commit `fa425f252fef6a08c1668b343fb18c9fbceb6346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One net-new root cause, grounded in one replayed behavioral check.

Cause 1 of 1: a DataBrew recipe-job run is a clock-driven state transition that never reads the dataset or evaluates the referenced recipe version, so a run whose recipe step references a column the dataset does not have reports terminal SUCCEEDED with no ErrorMessage, where AWS reports terminal FAILED carrying a RecipeStepError diagnosis.
Failing check: databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing.

AWS: Smithy (services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): StartJobRun POST /jobs/{Name}/startJobRun -> {RunId @required}, errors [ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400]; DescribeJobRun GET /jobs/{Name}/jobRun/{RunId} -> {State: JobRunState enum [STARTING, RUNNING, STOPPING, STOPPED, SUCCEEDED, FAILED, TIMEOUT], ErrorMessage: JobRunErrorMessage, Outputs, ...}. The model does not state the execution semantics, so for those it is recorded behavior only: a schema mismatch is NOT a rejection at the door — StartJobRun is accepted with HTTP 200 and a RunId, the run is admitted and appears in ListJobRuns exactly once, and the run then executes and ends in State FAILED. DescribeJobRun's ErrorMessage for that class of failure is a RecipeStepError diagnosis: its fixed parts are the leading token "RecipeStepError", the phrase "Failed at step <n>" naming the 1-based index of the recipe step that could not be applied, and the phrase "Column '<name>' does not exist"; its variable parts are the step index, the column name taken from the recipe step's Parameters/ConditionExpressions (here sourceColumn/TargetColumn), and the enumeration of the column names the dataset actually has, taken from the dataset's input (here the header of the CSV, sku and qty). A FAILED run writes zero objects under each Output's Location Key prefix.

Emulator: the same sequence returns a run that reaches terminal State SUCCEEDED with ErrorMessage absent (replay: "a recipe job over a mismatched schema ended SUCCEEDED, expected FAILED (ErrorMessage None)"). StartJobRun mints a run record and the run-state machine advances it to SUCCEEDED on the clock alone; nothing resolves the job's DatasetName to its S3 input, reads its header, or applies the RecipeReference's steps, so no step can fail and no RecipeStepError can ever be produced. Because the check aborted on this assertion, its later assertions in the same path were not reached and must be re-verified by replaying it: that ErrorMessage starts with "RecipeStepError" and names the step, the missing column and the dataset's own columns; that a FAILED run leaves the Output prefix empty; and that ListJobRuns reports exactly the one admitted run.

Why: a client relying on AWS's behavior treats SUCCEEDED as "the recipe was applied to this data" and FAILED+ErrorMessage as the signal to repair the recipe or the dataset. Against the emulator a job whose recipe does not match its dataset at all is indistinguishable from a healthy one: pipelines gated on run state proceed, alarms on FAILED never fire, and the operator gets no diagnosis of which step or which column is wrong. Error-path tests for recipe/dataset schema drift cannot be written against the emulator at all.

Fix: the job-run execution path — StartJobRun's handler (services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/start_job_run.rs) plus the run-state machine in the service's state module: when advancing a run out of RUNNING, resolve the job's dataset input and the referenced recipe version, apply the steps, and when a step references a column that is not present, terminate the run in FAILED and store the RecipeStepError ErrorMessage described above (step index, missing column, available columns) for DescribeJobRun/ListJobRuns to serve, writing nothing to the Outputs.

Related but not covering: open issue 20261102T051711Z-databrew-recipe-jobs-createrecipejob-accepts-a-role-databrew.md (Cause 2) covers the SUCCESS path producing no output objects, for the checks databrew.recipe-job-filter-drops-rows-from-s3-output and databrew.recipe-job-rename-and-type-coercion-land-in-output; it does not carry this check's replay key and says nothing about the FAILED path or the RecipeStepError diagnosis, which is what this issue reports. The other open databrew issues concern scaffold stubs (UpdateRecipe, DeleteSchedule), schedule/dataset error wording and recipe versioning.

Traces: databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replay against the emulator: databrew 0/7 passed.

[mismatch] databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing
  AssertionError: a recipe job over a mismatched schema ended SUCCEEDED, expected FAILED (ErrorMessage None)

Setup the check performed: a bucket holding in/people.csv (header id,employee_name,dept,salary) and in/parts.csv (header sku,qty); an IAM role DataBrew can assume; two CSV datasets, one over each key; a recipe with a single step {Action: {Operation: REMOVE_VALUES, Parameters: {sourceColumn: dept}}, ConditionExpressions: [{Condition: IS, Value: '["sales"]', TargetColumn: dept}]}, published to a version. A positive control (CreateProject/DescribeProject over the MATCHING dataset with the same role and recipe) passed, so the role, bucket, recipe and dataset wiring are all sound. A recipe job was then created over the MISMATCHED dataset (the one whose columns are sku,qty — it has no 'dept') with Outputs=[{Format: CSV, Location: {Bucket: B, Key: 'out-mismatch/'}, Overwrite: true, MaxOutputFiles: 1}], Timeout=30, MaxRetries=0, and started.

Expected (AWS, recorded): DescribeJobRun reaches State "FAILED" with ErrorMessage beginning "RecipeStepError", containing "Failed at step 1" and "Column 'dept' does not exist", and enumerating the dataset's real columns sku and qty; zero objects under the 'out-mismatch/' prefix; ListJobRuns reports exactly the one RunId returned by StartJobRun.
Actual (emulator): DescribeJobRun reaches terminal State "SUCCEEDED" and ErrorMessage is absent (None). The step index, the missing column name and the dataset's column list in the expected message are variable parts taken from the recipe step and the dataset's header, not literals of this run; the job/dataset/recipe names and the run id in the recording are per-run values and are not requirements.

## Steps to Reproduce
1. Boot the emulator and point s3, iam and databrew clients at it.
2. Create bucket B; put in/people.csv with header 'id,employee_name,dept,salary' and in/parts.csv with header 'sku,qty'.
3. Create an IAM role R whose trust policy allows databrew.amazonaws.com and that can read B.
4. create_dataset(Name=D_ok, Format='CSV', FormatOptions={'Csv': {'Delimiter': ',', 'HeaderRow': True}}, Input={'S3InputDefinition': {'Bucket': B, 'Key': 'in/people.csv'}}) and the same for D_bad over 'in/parts.csv'.
5. create_recipe(Name=RC, Steps=[{'Action': {'Operation': 'REMOVE_VALUES', 'Parameters': {'sourceColumn': 'dept'}}, 'ConditionExpressions': [{'Condition': 'IS', 'Value': '["sales"]', 'TargetColumn': 'dept'}]}]); publish_recipe(Name=RC); version = describe_recipe(Name=RC)['RecipeVersion'].
6. create_recipe_job(Name=J, DatasetName=D_bad, RoleArn=<R arn>, LogSubscription='DISABLE', Timeout=30, MaxRetries=0, RecipeReference={'Name': RC, 'RecipeVersion': version}, Outputs=[{'Format': 'CSV', 'Location': {'Bucket': B, 'Key': 'out-mismatch/'}, 'Overwrite': True, 'MaxOutputFiles': 1}]).
7. run_id = start_job_run(Name=J)['RunId']; poll describe_job_run(Name=J, RunId=run_id) until State is terminal.
   Expected: State 'FAILED' with ErrorMessage starting 'RecipeStepError', containing 'Failed at step 1', "Column 'dept' does not exist" and the dataset's own columns sku and qty. Actual: State 'SUCCEEDED', ErrorMessage absent.
8. list_objects_v2(Bucket=B, Prefix='out-mismatch/') must return no keys, and list_job_runs(Name=J) must report exactly [run_id].
9. Re-replay databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing to confirm the cause is closed.
