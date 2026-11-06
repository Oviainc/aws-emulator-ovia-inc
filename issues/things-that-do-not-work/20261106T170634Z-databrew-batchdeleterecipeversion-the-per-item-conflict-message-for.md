Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.delete-dataset-referenced-by-project-rejected
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.delete-dataset-referenced-by-project-rejected|run#35|AssertionError|raise AssertionError(f"the per-item message does not name the project: {errors[0]!r}")
Issue-Commit: abded3a6ac12b18365060f4d039abb5421f29c54

# DataBrew BatchDeleteRecipeVersion: the per-item conflict message for a version a project holds omits the recipe name, pluralizes "version" and drops the sentence's period

> _Note: this issue was identified against the code base at commit `abded3a6ac12b18365060f4d039abb5421f29c54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteRecipeVersion

## Description
Cause 1 of 1: the `ErrorMessage` of the per-item `Errors` entry BatchDeleteRecipeVersion reports for a recipe version that a project is developing is worded wrongly. Failing check: databrew.delete-dataset-referenced-by-project-rejected.

AWS: BatchDeleteRecipeVersion returns HTTP 200 and reports a version it could not delete as an item of `Errors` (Smithy: BatchDeleteRecipeVersion -> BatchDeleteRecipeVersionResponse$Errors of RecipeVersionErrorDetail{ErrorCode pattern ^[1-5][0-9][0-9]$, ErrorMessage, RecipeVersion}; the operation doc states "The request will complete successfully, but with partial failures, if: ... You specify LATEST_WORKING, but it's being used by a project."). The model documents the member only as "The text of the error message", so the message text itself is recorded behavior only. The recording shows AWS words this item as the sentence:

  Recipe <recipe-name> version <version-identifier> are used by project <project-name>.

Its FIXED text is `Recipe `, ` version `, ` are used by project ` and the terminating `.`; its VARIABLE parts are the recipe name from the request's `Name`, the version identifier from the requested `RecipeVersions` entry (e.g. the literal LATEST_WORKING the caller sent), and the name of the project that holds that recipe, read from emulator state. `ErrorCode` is "409" and `RecipeVersion` echoes the requested identifier — both of those the emulator already gets right.

Emulator: for the same call it returns the item

  {'ErrorCode': '409', 'ErrorMessage': 'Recipe versions LATEST_WORKING are used by project <project-name>', 'RecipeVersion': 'LATEST_WORKING'}

i.e. the message is `Recipe versions <version> are used by project <project>` — it (a) never interpolates the recipe name, (b) writes the plural noun `versions` where AWS writes the singular `version` followed by the recipe name/version pair, and (c) omits the sentence-terminating period. The builder is crate::state::recipe_version_used_by_project_message(version, project), which takes no recipe name at all, so the name cannot appear.

Why: clients and operators read this per-item text to find out WHICH recipe and version is still held and by what, because the call is a 200 and carries no top-level error. With the recipe name missing, a batch issued against a recipe cannot be told apart from one against any other recipe in log scrapes or in support/automation that parses the sentence; the plural/period differences break exact-match assertions and message-equality tests written against real AWS.

Note on the same root cause: the sibling builder crate::state::recipe_version_used_by_job_message has the identical shape (`Recipe versions {version} are used by job {job}`) and is the same helper family and the same defect; no behavioral check in this suite reaches the job variant, so it is reported here as the same code path rather than as a separate cause.

Also note: open issue 20261029T193300Z-databrew-recipe-versions-are-not-tracked-describerecipe-serves.md concerns DescribeRecipe not tracking versions; it does not cover this per-item message wording.

Fix: the message construction for this refusal class — the per-item conflict message builder in the DataBrew state/message helpers, plus the BatchDeleteRecipeVersion handler's call into it, which must pass the recipe name the request addressed so the sentence can name it.

## Evidence
Check databrew.delete-dataset-referenced-by-project-rejected, after creating dataset+recipe+project and then calling BatchDeleteRecipeVersion(Name=<recipe>, RecipeVersions=["LATEST_WORKING"]):

Expected (real AWS, template): Errors == [one item] with ErrorCode "409", RecipeVersion "LATEST_WORKING", ErrorMessage == "Recipe <recipe-name> version LATEST_WORKING are used by project <project-name>."

Actual (emulator): {'ErrorCode': '409', 'ErrorMessage': 'Recipe versions LATEST_WORKING are used by project sim2real-behav-databrew-prj-c6b8db50d881', 'RecipeVersion': 'LATEST_WORKING'}

Assertion raised: AssertionError: the per-item message does not name the project: {'ErrorCode': '409', 'ErrorMessage': 'Recipe versions LATEST_WORKING are used by project sim2real-behav-databrew-prj-c6b8db50d881', 'RecipeVersion': 'LATEST_WORKING'}

The status (200) and the per-item ErrorCode ("409") and RecipeVersion match; only the ErrorMessage text diverges. The generated project/recipe names above are per-run values filling the template, not part of it.

Traces/citations: services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs lines 119-121 (recipe_version_used_by_project_message, no recipe-name parameter, plural "versions", no period) and lines 129-131 (the identical job variant); services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/batch_delete_recipe_version.rs lines 364-372 (the call site, which passes only version and project).

## Steps to Reproduce
1. CreateDataset(Name=<ds>, Format=CSV, Input.S3InputDefinition=<bucket/key>).
2. CreateRecipe(Name=<rc>, Steps=[{Action:{Operation:UPPER_CASE, Parameters:{sourceColumn:dept}}}]).
3. CreateProject(Name=<prj>, DatasetName=<ds>, RecipeName=<rc>, RoleArn=<databrew role>, Sample={Size:500,Type:FIRST_N}) — the project now develops the recipe's LATEST_WORKING version.
4. BatchDeleteRecipeVersion(Name=<rc>, RecipeVersions=["LATEST_WORKING"]).
5. Inspect Errors[0].ErrorMessage. Observed: "Recipe versions LATEST_WORKING are used by project <prj>". Expected: "Recipe <rc> version LATEST_WORKING are used by project <prj>." (recipe name present, singular "version", terminating period).
