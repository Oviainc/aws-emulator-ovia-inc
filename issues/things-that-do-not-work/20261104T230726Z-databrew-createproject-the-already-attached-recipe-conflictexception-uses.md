Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.delete-dataset-referenced-by-project-rejected
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.delete-dataset-referenced-by-project-rejected|run#28|AssertionError|raise AssertionError(f"unexpected refusal for reusing an attached recipe: {message!r}")
Issue-Commit: 93a6fab98750aee235e39bc6e7f643f7e213bd09

# DataBrew CreateProject: the already-attached-recipe ConflictException uses the generic "in use by the following projects" sentence instead of AWS's "already associated with project" message

> _Note: this issue was identified against the code base at commit `93a6fab98750aee235e39bc6e7f643f7e213bd09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProject

## Description
One net-new root cause in the DataBrew project/recipe conflict area. The HTTP status and the error class are already correct, so only the message template diverges.

Cause 1 of 1: CreateProject naming a recipe that is already attached to another project is refused with the right status and the right error shape, but with a reworded message built by the generic resource-in-use helper.

AWS: Smithy (services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): CreateProject POST /projects, input {Name @required ProjectName len 1..255, DatasetName @required len 1..255, RecipeName @required len 1..255, RoleArn @required len 20..2048, Sample, Tags}, errors [ConflictException httpError 409 "Updating or deleting a resource can cause an inconsistent state.", InternalServerException 500, ServiceQuotaExceededException 402, ValidationException 400]. A DataBrew recipe belongs to at most one project (Recipe$ProjectName is a single name), so creating a second project on an already-attached recipe is refused. Recorded behavior: HTTP 409, error code ConflictException, Message "The recipe <RecipeName> is already associated with project <ProjectName of the owning project>. Choose a recipe that isn't already part of a project." The fixed text is "The recipe ", " is already associated with project ", ". Choose a recipe that isn't already part of a project." — a single sentence pair, terminating period included; the two variable parts are the request's RecipeName and the name of the project that already holds that recipe, read from state. (The names in the replay — recipe sim2real-behav-databrew-rc-0375e7bca359, project sim2real-behav-databrew-prj-9a47afe47c1f — are that run's generated values filling the template, not part of it.)

Emulator: same 409 and same ConflictException shape (the check's code assertion passes), but the Message is "Recipe <RecipeName> is currently in use by the following projects: <project>." — AWS's wording, word order, subject ("The recipe" vs "Recipe"), the list framing ("the following projects:" plus a comma-joined list vs a single named project) and the trailing remediation sentence ("Choose a recipe that isn't already part of a project.") are all absent. The text comes from crate::state::in_use_message("Recipe", name, "projects", &[owner]), a shared plural-list helper that is not DataBrew's CreateProject sentence.

Why: an error message is the only human- and machine-readable detail a ConflictException carries for this refusal — there is no per-member or per-resource field in the shape. A client, SDK integration test, console surface or alert matcher keyed on AWS's sentence does not match, and the emulator's text also loses the remediation clause that tells the caller what to do (pick an unattached recipe), so a user reading the emulator's error is told the recipe is "in use" without being told a recipe may belong to only one project.

Fix: CreateProject's already-attached-recipe conflict construction in the operation's handler — build DataBrew's CreateProject-specific sentence there (single owning project named, remediation clause included) instead of calling the shared plural resource-in-use message helper. Do not retarget the shared helper itself: other DataBrew refusals (e.g. the BatchDeleteRecipeVersion per-item "version ... are used by project ..." text and the DeleteDataset "The dataset X is used in project Y. To delete this dataset, first delete the associated project." text, which the same check asserts and which already match) use different templates, so these must stay per-site. Related but distinct: the open issue "DataBrew error messages reworded and UpdateSchedule state never applied" (20261029T193300Z-databrew-error-messages-reworded-and-updateschedule-state-never.md) covers reworded CreateSchedule/DescribeSchedule/CreateDataset messages; it does NOT cover CreateProject's recipe conflict, which is this issue.

Traces: behavioral check databrew.delete-dataset-referenced-by-project-rejected (it reaches this call after its DeleteDataset-conflict assertions pass).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), databrew 0/7; this check verdict = mismatch.

[mismatch] databrew.delete-dataset-referenced-by-project-rejected
  check_databrew_delete_dataset_referenced_by_project_rejected.py line 100:
  AssertionError: unexpected refusal for reusing an attached recipe: 'Recipe sim2real-behav-databrew-rc-0375e7bca359 is currently in use by the following projects: sim2real-behav-databrew-prj-9a47afe47c1f.'

The check had already accepted the status/code: the preceding assertion `if code != "ConflictException"` passed, so the emulator returns HTTP 409 / ConflictException correctly and ONLY the Message text diverges.

Expected (the check's `expected`, AWS's template):
  "The recipe {recipe} is already associated with project {project}. Choose a recipe that isn't already part of a project."
Actual (emulator):
  "Recipe {recipe} is currently in use by the following projects: {project}."

Earlier assertions in the same check that PASS and show the per-site templates that must not be unified:
  DeleteDataset on a project-pinned dataset -> 409 ConflictException, "The dataset {dataset} is used in project {project}. To delete this dataset, first delete the associated project."

Source evidence: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_project.rs lines 225-239 — the already-attached-recipe branch constructs ConflictException with `crate::state::in_use_message("Recipe", &record.recipe_name, "projects", &[owner.as_str()])`.

Model evidence (smithy_query, query="operation", operation="CreateProject", model_path="services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json"): http POST /projects code 200; errors ConflictException(409), InternalServerException(500), ServiceQuotaExceededException(402), ValidationException(400).

## Steps to Reproduce
1. Boot the emulator and point a databrew client (and s3/iam for setup) at it.
2. Create an S3 input object and an assumable DataBrew role ARN.
3. db.create_dataset(Name=D, Format="CSV", FormatOptions={"Csv": {"Delimiter": ",", "HeaderRow": True}}, Input={"S3InputDefinition": {"Bucket": B, "Key": K}}).
4. db.create_recipe(Name=R, Steps=[{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]).
5. db.create_project(Name=P1, DatasetName=D, RecipeName=R, RoleArn=role, Sample={"Size": 500, "Type": "FIRST_N"}) — succeeds, attaching R to P1.
6. db.create_project(Name=P2, DatasetName=D, RecipeName=R, RoleArn=role, Sample={"Size": 10, "Type": "FIRST_N"}).
   Expected: HTTP 409, ConflictException, Message "The recipe R is already associated with project P1. Choose a recipe that isn't already part of a project."
   Actual:   HTTP 409, ConflictException (correct), Message "Recipe R is currently in use by the following projects: P1."
7. Re-replay behavioral check databrew.delete-dataset-referenced-by-project-rejected to confirm the cause is closed.
