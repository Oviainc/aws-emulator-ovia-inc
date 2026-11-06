Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.delete-dataset-referenced-by-project-rejected
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.delete-dataset-referenced-by-project-rejected|run#31|AssertionError|raise AssertionError(f"unexpected not-found message for the refused project: {message!r}")
Issue-Commit: 57b88ffebde7e0555a50fc5146f1edae04c57e49

# DataBrew project not-found ResourceNotFoundException uses the generic "Project <name> wasn't found." template instead of AWS's "The project <name> wasn't found."

> _Note: this issue was identified against the code base at commit `57b88ffebde7e0555a50fc5146f1edae04c57e49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProject

## Description
One net-new cause in the projects area. The HTTP status and the error code are already correct; only the message template diverges.

Cause 1 of 1: a lookup that addresses a project which does not exist answers its ResourceNotFoundException with the shared generic not-found sentence rather than DataBrew's project sentence.

AWS: Smithy (services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): DescribeProject input {Name: ProjectName @required @length 1..255}, errors ResourceNotFoundException (errorKind client, httpError 404, doc "One or more resources can't be found.") and ValidationException (400). The sentence documented on the error SHAPE describes the error class, not the message a particular lookup answers with: DataBrew builds the message from the request so a client can tell which resource was missing and of which kind. For a project the message is "The project <Name> wasn't found." — the fixed text is "The project " and " wasn't found." (leading "The", the kind spelled in LOWER case, terminating period); the only variable part is <Name>, taken verbatim from the request's Name member. Recorded behavior confirms this status/code/message triple for a project name that resolves to no project. Note the template is per-resource in this service and must not be unified: schedules use "Schedule <Name> not found" (no "The", no period) and a recipe-version miss uses "The recipe <Name> for version <Version> wasn't found." — see fixed issue 20261029T193300Z-databrew-error-messages-reworded-and-updateschedule-state-never (Cause 4) and 20261029T172132Z-databrew-describerecipe-versionless-request-falls-back-to-the-draft, neither of which covers the project kind.

Emulator: HTTP 404 with the correct ResourceNotFoundException code, but Message "Project <Name> wasn't found." — the capitalised kind with no leading "The". The project lookup in services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/describe_project.rs calls the shared helper crate::state::not_found_message("Project", &input.name), and that helper (services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs) renders every kind not listed in SHORT_FORM_RESOURCE_KINDS as "{kind} {identifier} wasn't found.", so the project kind inherits a sentence AWS does not emit. Every project miss, from any operation that resolves a project by name, diverges the same way.

Why: the message is the only machine-readable detail a DataBrew ResourceNotFoundException carries — the shape has no per-resource or per-member field — so it is part of the contract. A client that distinguishes miss reasons by message text, an SDK integration test that asserts AWS's sentence, and log/alert matchers keyed on it all fail against the emulator even though the status and error class are right; a human reading the error sees wording the service never prints. In the failing check the divergence is reached exactly where a client verifies that a refused CreateProject left no project behind: the follow-up DescribeProject correctly refuses with ResourceNotFoundException, but the sentence it refuses with is not AWS's.

Fix: the project not-found construction in the DescribeProject handler, and the shared not-found message helper it calls in the service's state module — give the project kind its own template ("The project " + Name + " wasn't found.") the way the schedule and recipe-version kinds already have theirs, and route every other operation that resolves a project by name through the same template so one kind reads identically from every lookup. Do NOT collapse the kinds onto one string.

Traces: databrew.delete-dataset-referenced-by-project-rejected (the ConflictException refusals earlier in the same check — DeleteDataset on a pinned dataset and CreateProject on an already-attached recipe — both passed; the check stops on this message).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replay against the emulator: databrew 0/7 passed.

[mismatch] databrew.delete-dataset-referenced-by-project-rejected
  check_databrew_delete_dataset_referenced_by_project_rejected.py, line 107, in run
  AssertionError: unexpected not-found message for the refused project: "Project sim2real-behav-databrew-prj2-beb399a0156b wasn't found."

The check reached line 107, so everything before it matched: DescribeProject pinned the created dataset/recipe, DeleteDataset on the pinned dataset refused with ConflictException and AWS's "The dataset <D> is used in project <P>. To delete this dataset, first delete the associated project.", and CreateProject reusing the attached recipe refused with ConflictException and AWS's "The recipe <R> is already associated with project <P>. Choose a recipe that isn't already part of a project."

The failing assertion, on DescribeProject(Name=<project name the refused CreateProject used>):
  status:   404 (expected 404 — matches)
  code:     ResourceNotFoundException (expected ResourceNotFoundException — matches)
  expected Message: "The project <Name> wasn't found."
  actual   Message: "Project <Name> wasn't found."
Fixed text differs: AWS emits "The project " + " wasn't found."; the emulator emits "Project " + " wasn't found.". <Name> is the request's Name member and is the only variable part (the name shown above, sim2real-behav-databrew-prj2-beb399a0156b, is one run's generated value, not part of the template).

Source of the wrong text:
  services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/describe_project.rs:167-171 — store.projects.get(&input.name).ok_or_else(|| ResourceNotFoundException { message: Some(crate::state::not_found_message("Project", &input.name)) })
  services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs:53-59 — not_found_message renders non-short-form kinds as format!("{kind} {identifier} wasn't found."); SHORT_FORM_RESOURCE_KINDS (line 34) lists only "Schedule", so "Project" takes the capitalised generic form.

Model evidence (smithy_query, shape=ResourceNotFoundException): {"errorKind": "client", "httpError": 404, "members": {"Message": {"target": "Message"}}, "doc": "One or more resources can't be found."} — a single free-text Message member, confirming the message is the only detail the error carries.

## Steps to Reproduce
1. Boot the emulator and point a databrew client (plus s3 and iam, which the check uses for the dataset input and the job role) at it.
2. Create an S3 bucket with an input object, create a DataBrew role, then create_dataset(Name=D, Format="CSV", Input={"S3InputDefinition": {...}}) and create_recipe(Name=R, Steps=[...]).
3. create_project(Name=P, DatasetName=D, RecipeName=R, RoleArn=<role>, Sample={"Size": 500, "Type": "FIRST_N"}).
4. Attempt create_project(Name=P2, DatasetName=D, RecipeName=R, ...) with the recipe already attached to P. It is correctly refused with 409 ConflictException, so no project named P2 exists.
5. Call describe_project(Name=P2).
   Expected: 404 ResourceNotFoundException with Message "The project P2 wasn't found."
   Actual:   404 ResourceNotFoundException with Message "Project P2 wasn't found."
6. Confirm the same wording appears for any never-created project name, independent of how the name was produced.
7. Re-replay check databrew.delete-dataset-referenced-by-project-rejected to confirm the cause is closed; it must then continue past line 107 into the BatchDeleteRecipeVersion and DeleteProject assertions.
