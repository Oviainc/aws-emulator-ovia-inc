Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.delete-dataset-referenced-by-project-rejected
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.delete-dataset-referenced-by-project-rejected|run#21|AssertionError|raise AssertionError(f"the refusal does not name the project holding the dataset: {message!r}")
Issue-Commit: 75d52ea0c257a701244c018d7c3f550d51e1d46f

# DataBrew DeleteDataset in-use ConflictException message does not use AWS's wording

> _Note: this issue was identified against the code base at commit `75d52ea0c257a701244c018d7c3f550d51e1d46f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataset

## Description
One net-new root cause in databrew, grounded in one failing behavioral check.

Cause 1 of 1: the ConflictException DeleteDataset answers when a project still pins the dataset carries an emulator-invented sentence instead of AWS's message for that error class.

AWS: Smithy: DeleteDataset (DELETE /datasets/{Name}, input {Name -> DatasetName @required @httpLabel @length 1..255}), errors [ConflictException httpError 409, ResourceNotFoundException 404, ValidationException 400]; ConflictException's shape doc is "Updating or deleting a resource can cause an inconsistent state.", which describes the error CLASS, not the message a particular refusal answers with. Recorded behavior for deleting a dataset a project was created against: HTTP 409, error code ConflictException, message template
  "The dataset <DatasetName> is used in project <ProjectName>. To delete this dataset, first delete the associated project."
Fixed text, exactly: "The dataset ", " is used in project ", ". To delete this dataset, first delete the associated project." — note AWS opens with "The dataset" (article, lower-case kind), names the project in the SINGULAR with the word "project", and closes with the remediation sentence. Variable parts: <DatasetName> is the request's Name member; <ProjectName> is the name of the project in the emulator's state whose DatasetName equals it. No recording value is part of the template.

Emulator: the status (409) and the error code (ConflictException) are correct, but the message is built by the shared `in_use_message` helper as a pluralised list sentence: "Dataset <DatasetName> is currently in use by the following projects: <ProjectName>." — i.e. fixed text "Dataset ", " is currently in use by the following projects: ", "." None of AWS's fixed text is present: there is no leading article, the phrase is "is currently in use by the following projects:" rather than "is used in project", and the remediation sentence ("To delete this dataset, first delete the associated project.") is missing entirely.

Why: the message is the only machine-readable detail a DataBrew ConflictException carries (there is no per-resource member on the error shape), so a client that distinguishes "dataset pinned by a project" from any other 409 by matching AWS's sentence, and any SDK integration test or operator alert keyed on it, does not match. A human reading the emulator's error also never sees the remediation step AWS prints, so the documented recovery path (delete the project first) is lost.

Fix: the DeleteDataset handler's conflict construction in services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_dataset.rs and the shared message helper it calls (`in_use_message` in services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs) — DataBrew's dataset-in-use refusal is a per-resource template naming ONE project, not a generic "in use by the following <kind>s" list, so do not unify it with other in-use messages. Related but distinct: the open issue 20261029T193300Z-databrew-error-messages-reworded-and-updateschedule-state-never.md covers CreateSchedule / DescribeSchedule / CreateDataset message wording; it does NOT cover DeleteDataset's in-use message, which is this cause.

Traces: behavioral check databrew.delete-dataset-referenced-by-project-rejected (failed at the first refusal assertion; later assertions in the same check about the CreateProject attached-recipe conflict and the BatchDeleteRecipeVersion per-item 409 message were never reached and are not claimed here).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) check databrew.delete-dataset-referenced-by-project-rejected — verdict mismatch:

  AssertionError: the refusal does not name the project holding the dataset: 'Dataset sim2real-behav-databrew-ds-70a1169ce428 is currently in use by the following projects: sim2real-behav-databrew-prj-3776a127c50c.'

The check creates a dataset D, a recipe R, and a project P over (D, R), confirms DescribeProject pins D and R, then calls DeleteDataset(Name=D).

Expected (AWS): HTTP 409, Code ConflictException, Message
  "The dataset <D> is used in project <P>. To delete this dataset, first delete the associated project."
Actual (emulator): HTTP 409, Code ConflictException (both correct), Message
  "Dataset <D> is currently in use by the following projects: <P>."

The dataset/project names in the recorded strings above are per-run generated names and are the template's variable parts, not requirements.

Code path in the checkout: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_dataset.rs lines 167-183 collect the pinning projects and return ConflictException with crate::state::in_use_message("Dataset", &input.name, "projects", &pinning); the helper's contract is documented at services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs lines 68-80.

Model evidence (smithy_query, services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): DeleteDataset http {method DELETE, uri /datasets/{Name}, code 200}; errors ConflictException (409), ResourceNotFoundException (404), ValidationException (400).

## Steps to Reproduce
1. Boot the emulator and point an s3, iam and databrew client at it.
2. Put a CSV object in a bucket and create a DataBrew-trusted role (as the check does).
3. db.create_dataset(Name=D, Format="CSV", FormatOptions={"Csv": {...}}, Input={"S3InputDefinition": {...}}).
4. db.create_recipe(Name=R, Steps=[{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]).
5. db.create_project(Name=P, DatasetName=D, RecipeName=R, RoleArn=<role>, Sample={"Size": 500, "Type": "FIRST_N"}).
6. db.delete_dataset(Name=D) and read Error.Code and Error.Message.
   Expected: ConflictException (HTTP 409) with Message "The dataset D is used in project P. To delete this dataset, first delete the associated project."
   Actual: ConflictException (HTTP 409) with Message "Dataset D is currently in use by the following projects: P."
7. Re-replay behavioral check databrew.delete-dataset-referenced-by-project-rejected to confirm the cause is closed.
