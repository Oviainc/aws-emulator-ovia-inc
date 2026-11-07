Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs databrew.recipe-job-filter-drops-rows-from-s3-output databrew.recipe-job-rename-and-type-coercion-land-in-output
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs|_rejected#19|AssertionError|raise AssertionError(f"{label} was ACCEPTED, but real AWS rejects it")
Issue-Key: databrew.recipe-job-filter-drops-rows-from-s3-output|run#24|AssertionError|raise AssertionError(f"output layout was {keys}, expected exactly {expected_keys}")
Issue-Key: databrew.recipe-job-rename-and-type-coercion-land-in-output|run#24|AssertionError|raise AssertionError(f"output layout was {keys}, expected exactly {expected_keys}")
Issue-Commit: 150373bafed55a708e89f80fc6461a5cd6c528b9

# DataBrew recipe jobs: CreateRecipeJob accepts a role DataBrew cannot assume, and a job run reports SUCCEEDED without writing any output

> _Note: this issue was identified against the code base at commit `150373bafed55a708e89f80fc6461a5cd6c528b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Two distinct root causes in the DataBrew recipe-job area, both grounded in replayed behavioral checks. Neither is covered by the open databrew issues ("Implement databrew CreateProject/UpdateSchedule: unimplemented scaffold stub", "DataBrew error messages reworded and UpdateSchedule state never applied", "DataBrew recipe versions are not tracked..."), which concern scaffold stubs, schedule/dataset error wording and recipe versioning respectively. BOTH causes must be closed before re-submitting.

Cause 1 of 2: CreateRecipeJob does not validate that DataBrew can assume the supplied RoleArn, so a job built on an untrusted role is created instead of refused.
AWS: Smithy: CreateRecipeJob (POST /recipeJobs) input RoleArn -> Arn @required @length 20..2048, errors [AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400]. The model states the ARN is "the IAM role to be assumed when DataBrew runs the job"; the recorded behavior is that the service resolves that role at CREATE time and refuses a role whose trust policy does not allow databrew.amazonaws.com, with HTTP 400 ValidationException whose message contains the fixed text "DataBrew is not a trusted entity for the data access role" and, as its variable part, the rejected role ARN taken from the request's RoleArn. No job is minted by the refused call (a subsequent DescribeJob on that name must answer 404 ResourceNotFoundException).
Emulator: the same CreateRecipeJob call with a role that exists but trusts lambda.amazonaws.com returns HTTP 200 with the job Name, i.e. the job is accepted and inserted into state. services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_recipe_job.rs validates only the Name pattern and the existence of DatasetName; RoleArn is copied into the JobRecord unexamined and never resolved against the IAM store.
Why: a client that relies on AWS refusing the misconfigured role at create time gets a job that looks healthy and only fails (or, in this emulator, silently does nothing) at run time. CI that asserts the 400/ValidationException for a bad trust policy passes a broken configuration through, and the job name is now claimed in state by a job AWS would never have created.
Fix: CreateRecipeJob's handler/validation in services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_recipe_job.rs — resolve RoleArn against the IAM role store and its trust policy before inserting the JobRecord, and return ValidationException when databrew.amazonaws.com is not a trusted principal. Note the same check's later assertions (the "No outputs were selected for this job." ValidationException for a CreateRecipeJob with no Outputs at all, the duplicate-name ConflictException, the MaxCapacity default of 5 reported by DescribeJob, and the ResourceNotFoundException from StartJobRun/DeleteJob/DescribeJob on an absent job) were NOT reached because the run aborted at the role assertion; they are part of the same handler and should be re-verified by replaying the check.
Traces: databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs.

Cause 2 of 2: a recipe job run is a pure state transition — it reaches SUCCEEDED but never executes the recipe and writes no output object to the job's Outputs S3 location.
AWS: Smithy: StartJobRun (POST /jobs/{Name}/startJobRun) returns RunId; DescribeJobRun (GET /jobs/{Name}/jobRun/{RunId}) reports State (JobRunState) and Outputs. Recorded behavior: once the run reaches State SUCCEEDED, the service has written the transformed dataset to the Output's S3 Location; with an Output carrying MaxOutputFiles=1 exactly one object exists under the Output Location's Key prefix, named "<prefix><job-name>_part00000.csv" — fixed text is the "_part00000" part-suffix and the ".csv" extension for a CSV Output, while the prefix and the job name are the request's Output.Location.Key and the job's Name. The object's content is the input dataset with the recipe's steps applied (recorded behavior: REMOVE_VALUES gated by a ConditionExpression drops the matching rows and leaves the header unchanged; RENAME replaces the old column name in the header with the new one; CHANGE_DATA_TYPE to int NULLs an uncoercible cell and RETAINS its row).
Emulator: StartJobRun mints a run record that transitions to SUCCEEDED on the clock alone (services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/start_job_run.rs builds a JobRunRecord with state "STARTING" and stores it; nothing reads the dataset or the recipe). After the run reports SUCCEEDED, listing the Output Location prefix in S3 returns an EMPTY key set — the replay asserts "output layout was [], expected exactly ['out/<job-name>_part00000.csv']" — so neither the layout nor any transformed bytes exist.
Why: the run claims success while producing nothing. Every consumer downstream of a DataBrew job (a crawler, an Athena table, a pipeline step that reads the output prefix) sees a SUCCEEDED run and then no data, which is indistinguishable from an empty source; and no recipe semantics can be exercised against the emulator at all, so row filtering, renames and type coercion are untestable.
Fix: the job-run execution path — StartJobRun's handler plus the run-state machine in src/state.rs: on transition to SUCCEEDED, read the dataset's S3 input through the job's role, apply the referenced recipe version's steps, and write the result to each Output's Location using AWS's part-file naming and MaxOutputFiles, so DescribeJobRun's SUCCEEDED is backed by the written objects.
Traces: databrew.recipe-job-filter-drops-rows-from-s3-output, databrew.recipe-job-rename-and-type-coercion-land-in-output.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replay against the emulator, databrew 0/7 passed.

Cause 1 — [mismatch] databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs:
  AssertionError: CreateRecipeJob with a role DataBrew cannot assume was ACCEPTED, but real AWS rejects it
  Expected: HTTP 400 ValidationException whose message contains "DataBrew is not a trusted entity for the data access role" and names the rejected role ARN; DescribeJob on that job name then answers ResourceNotFoundException.
  Actual: HTTP 200 from CreateRecipeJob (job created). The role used was created by the check with a trust policy for lambda.amazonaws.com.
  Source: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_recipe_job.rs lines 187-255 — only validate_resource_name("Name") and the datasets lookup are performed; input.role_arn is stored verbatim (line 205).

Cause 2 — [mismatch] databrew.recipe-job-filter-drops-rows-from-s3-output:
  AssertionError: output layout was [], expected exactly ['out/<job-name>_part00000.csv']
  The run had already satisfied the preceding assertion that DescribeJobRun reports State == SUCCEEDED, so the emulator reports success with zero objects under the Output prefix.
[mismatch] databrew.recipe-job-rename-and-type-coercion-land-in-output:
  AssertionError: output layout was [], expected exactly ['out/<job-name>_part00000.csv'] — same empty key set after a SUCCEEDED run.
  Source: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/start_job_run.rs lines 157-196 — the handler mints a run id and records a JobRunRecord (state "STARTING", outputs copied from the job) and returns; no dataset read, no recipe evaluation, no S3 write.

Model evidence (smithy_query on services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): CreateRecipeJob POST /recipeJobs, input {Name @required, RoleArn @required, DatasetName, RecipeReference, Outputs @length min 1, MaxCapacity default 0, Timeout default 0}, errors [AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400]; StartJobRun POST /jobs/{Name}/startJobRun -> {RunId @required}; DescribeJobRun GET /jobs/{Name}/jobRun/{RunId} -> {State, Outputs, RecipeReference, StartedOn, CompletedOn, ...}.

## Steps to Reproduce
1. Boot the emulator and point s3, iam and databrew clients at it.
2. Cause 1: create a bucket B with an object in/people.csv; create an IAM role R_bad whose trust policy allows lambda.amazonaws.com (not databrew.amazonaws.com); create_dataset(Name=D, Format=CSV, Input={S3InputDefinition:{Bucket:B, Key:'in/people.csv'}}); create_recipe(Name=RC, Steps=[...]); publish_recipe(Name=RC).
3. Call create_recipe_job(Name=J1, DatasetName=D, RoleArn=<R_bad arn>, RecipeReference={Name:RC, RecipeVersion:'1.0'}, Outputs=[{Format:'CSV', Location:{Bucket:B, Key:'out/'}, Overwrite:true, MaxOutputFiles:1}]). Expected: 400 ValidationException containing "DataBrew is not a trusted entity for the data access role" plus the role ARN, and describe_job(Name=J1) -> ResourceNotFoundException. Actual: 200, job created.
4. Cause 2: repeat step 2 with a role R_good trusting databrew.amazonaws.com, then create_recipe_job(Name=J2, ..., RoleArn=<R_good arn>, Outputs=[{Format:'CSV', Location:{Bucket:B, Key:'out/'}, Overwrite:true, MaxOutputFiles:1}]).
5. run_id = start_job_run(Name=J2)['RunId']; poll describe_job_run(Name=J2, RunId=run_id) until State is terminal. Observed: SUCCEEDED.
6. list_objects_v2(Bucket=B, Prefix='out/'). Expected: exactly one key 'out/J2_part00000.csv' whose CSV body is the input with the recipe's steps applied. Actual: no keys at all.
7. Re-replay databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs, databrew.recipe-job-filter-drops-rows-from-s3-output and databrew.recipe-job-rename-and-type-coercion-land-in-output to confirm both causes are closed.
