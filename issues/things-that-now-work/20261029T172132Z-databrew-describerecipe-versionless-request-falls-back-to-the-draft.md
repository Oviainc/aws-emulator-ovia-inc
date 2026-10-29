Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.recipe-version-publish-and-latest-working-split
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.recipe-version-publish-and-latest-working-split|_rejected#7|AssertionError|raise AssertionError(f"{label} was ACCEPTED, but real AWS rejects it")
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# DescribeRecipe: versionless request falls back to the draft instead of refusing LATEST_PUBLISHED, and the version-miss message is misworded

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRecipe

## Description
Two distinct causes in DataBrew's recipe-version resolution, both reached through DescribeRecipe on a recipe that has never been published. Both must be closed before re-submitting.

Cause 1 of 2: a versionless DescribeRecipe resolves the WORKING draft when nothing is published, instead of refusing.
AWS: Smithy model, DescribeRecipeRequest$RecipeVersion (RecipeVersion, @length 1..16, httpQuery `recipeVersion`, optional) documents "The recipe version identifier. If this parameter isn't specified, then the latest published version is returned." So omitting RecipeVersion resolves the LATEST_PUBLISHED alias and nothing else; a recipe whose only version is its draft (0.1) holds no latest-published version, and DescribeRecipe is modeled with ResourceNotFoundException (HTTP 404). Recorded behavior for this sequence: CreateRecipe, then DescribeRecipe with no RecipeVersion is refused HTTP 404 `ResourceNotFoundException` with message `The recipe <Name> for version LATEST_PUBLISHED wasn't found.` — fixed text `The recipe `, ` for version `, ` wasn't found.`, with `<Name>` taken from the request's Name and `LATEST_PUBLISHED` being the alias the omitted parameter defaults to (not a value from the request).
Emulator: the call is ACCEPTED with HTTP 200 and serves the draft body (RecipeVersion `0.1`). The omitted identifier is resolved with a fallback to the working version rather than strictly to the latest published one, so an unpublished recipe describes successfully.
Why: a client cannot distinguish "published body" from "unsaved draft". Callers that read DescribeRecipe without a version to fetch the frozen, published recipe (job definitions, promotion/CI gates, anything that must not run an unpublished draft) silently receive draft steps, and the 404 that is supposed to tell them "nothing has been published yet" never arrives — the publish lifecycle becomes invisible and un-gateable.
Fix: the DescribeRecipe handler's version resolution — the recipe record's version lookup used for an absent RecipeVersion must behave exactly like the LATEST_PUBLISHED alias (no draft fallback) and return the modeled ResourceNotFoundException when no version has been published.
Traces: databrew.recipe-version-publish-and-latest-working-split (fails at the first rejection: "a versionless describe before the first publish was ACCEPTED, but real AWS rejects it").

Cause 2 of 2: the ResourceNotFoundException message for a recipe-VERSION miss uses the wrong template.
AWS: recorded behavior only (the model documents only the error class, "One or more resources can't be found."). For a miss that addresses a (recipe, version) pair, the message is `The recipe <Name> for version <RecipeVersion> wasn't found.` when the version is requested by alias/omitted default (e.g. LATEST_PUBLISHED, LATEST_WORKING), and `The recipe <Name> version <Version> wasn't found.` in BatchDeleteRecipeVersion's per-item ErrorMessage for a numeric version that does not exist. Fixed text in both: a leading `The recipe `, a trailing ` wasn't found.`, and the lower-case resource word `recipe`; the variable parts are the request's recipe Name and the version identifier that was looked up.
Emulator: builds the message from the generic not-found helper as `Recipe <Name> version <RecipeVersion> wasn't found.` — capitalised `Recipe`, no leading `The`, and no `for ` before the version — so the string differs from AWS for every recipe-version miss (both the versionless/alias case above and an explicitly named missing version).
Why: error messages are part of the contract clients match on; tests, retry classifiers and operator tooling that key on AWS's sentence do not match, and the message no longer reads as AWS's recipe-version wording.
Fix: the DescribeRecipe handler's not-found construction (and the shared not-found message helper it calls) must emit DataBrew's recipe-version template rather than the generic `<Kind> <Id> wasn't found.` one.
Traces: databrew.recipe-version-publish-and-latest-working-split (the same check asserts the exact message for the LATEST_PUBLISHED miss, and later, after a full delete, for the LATEST_WORKING miss).

## Evidence
Check databrew.recipe-version-publish-and-latest-working-split, step "a versionless describe before the first publish":

Expected (real AWS): DescribeRecipe(Name=<recipe>) with no RecipeVersion -> HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `The recipe <recipe> for version LATEST_PUBLISHED wasn't found.`

Actual (emulator): HTTP 200 with a DescribeRecipeResponse body; no error raised. Replay assertion:
  AssertionError: a versionless describe before the first publish was ACCEPTED, but real AWS rejects it
  (check_databrew_recipe_version_publish_and_latest_working_split.py line 60 -> _rejected line 184)

Source of the accept: services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs, Recipe::resolve_version — `None => self.published.last().or(Some(&self.working))`. The `.or(Some(&self.working))` arm is the draft fallback; `Some(LATEST_PUBLISHED) => self.published.last()` (no fallback) is the behavior the omitted identifier must share.

Source of the misworded message: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/describe_recipe.rs lines 194-205 call crate::state::not_found_message("Recipe", &format!("{} version {requested}", input.name)), and state.rs not_found_message formats `{kind} {identifier} wasn't found.` -> `Recipe <name> version LATEST_PUBLISHED wasn't found.` instead of `The recipe <name> for version LATEST_PUBLISHED wasn't found.`

Model evidence (smithy_query, com-amazonaws-databrew-awsgluedatabrew.json): DescribeRecipe input members {Name: RecipeName @required @httpLabel, RecipeVersion: RecipeVersion @httpQuery recipeVersion, len 1..16}; errors [ResourceNotFoundException httpError 404, ValidationException httpError 400]; doc on RecipeVersion: "If this parameter isn't specified, then the latest published version is returned."

## Steps to Reproduce
1. Boot the emulator and point a botocore databrew client at it.
2. db.create_recipe(Name=R, Description="draft", Steps=[{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}])
3. db.describe_recipe(Name=R, RecipeVersion="LATEST_WORKING") -> RecipeVersion "0.1" (this part is correct).
4. db.describe_recipe(Name=R)  # no RecipeVersion, nothing published yet
   Expected: ClientError ResourceNotFoundException, HTTP 404, message `The recipe R for version LATEST_PUBLISHED wasn't found.`
   Actual: HTTP 200 returning the draft (RecipeVersion "0.1").
5. For cause 2 in isolation: db.describe_recipe(Name=R, RecipeVersion="LATEST_PUBLISHED") -> the 404 is raised but with message `Recipe R version LATEST_PUBLISHED wasn't found.` instead of `The recipe R for version LATEST_PUBLISHED wasn't found.`
6. Re-replay databrew.recipe-version-publish-and-latest-working-split to confirm; the check continues past this point into the publish/update/publish version-label and BatchDeleteRecipeVersion assertions, which were never exercised while this refusal was missing.
