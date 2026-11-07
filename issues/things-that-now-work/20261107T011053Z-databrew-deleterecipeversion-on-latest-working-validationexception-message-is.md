Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.recipe-version-publish-and-latest-working-split
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.recipe-version-publish-and-latest-working-split|run#30|AssertionError|raise AssertionError(f"unexpected rejection for deleting the draft: {message!r}")
Issue-Commit: 71f467b753f0ae2b97ec4af97aae505f78f5a227

# DataBrew DeleteRecipeVersion on LATEST_WORKING: ValidationException message is reworded into the BatchDeleteRecipeVersion ordering sentence instead of AWS's "Recipe version LATEST_WORKING is not allowed to be deleted"

> _Note: this issue was identified against the code base at commit `71f467b753f0ae2b97ec4af97aae505f78f5a227`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecipeVersion

## Description
One net-new cause in DataBrew's recipe-version delete path.

Cause 1 of 1: DeleteRecipeVersion refusing the working draft answers HTTP 400 ValidationException with the wrong message text.

AWS: Smithy (services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): DeleteRecipeVersion DELETE /recipes/{Name}/recipeVersion/{RecipeVersion}, input {Name @required @httpLabel, RecipeVersion @required @httpLabel @length 1..16 — documented "You can specify a numeric versions (X.Y) or LATEST_WORKING. LATEST_PUBLISHED is not supported."}, errors [ConflictException 409, ResourceNotFoundException 404, ValidationException 400]. The model does not state the message text, so for the wording this is recorded behavior only: addressing the working draft through DeleteRecipeVersion is refused with HTTP 400, error code ValidationException and the message "Recipe version LATEST_WORKING is not allowed to be deleted". That message is entirely FIXED text — it has no variable part at all: it names neither the recipe, nor the recipe's other versions, nor the number the draft currently carries (the alias LATEST_WORKING in it is the literal identifier this operation refuses, not a value echoed from the request). The ordering rule ("the draft goes only once every other version is gone") is carried by BatchDeleteRecipeVersion's per-item 409 instead, which the same recording reports as "Recipe <RecipeName> version LATEST_WORKING can only be deleted if all other versions got deleted." — there the recipe name and the version ARE request values. Hypothesis: because AWS's DeleteRecipeVersion message states no condition, that operation refuses LATEST_WORKING categorically rather than only while other versions survive; what the recording establishes for certain is the status, the code and the message for a draft of a recipe that still holds published versions.

Emulator: the call is refused with the right status and code (400 / ValidationException) but with the batch operation's ordering sentence, "The LATEST_WORKING version will only be deleted if the recipe has no other versions." — different fixed text, and a trailing period AWS's sentence does not carry. The refusal is produced by the `holds_other_versions` branch of the DeleteRecipeVersion handler, which reuses the shared state helper that renders the batch wording.

Why: a client that matches on the refusal text — error-mapping tables, retry/cleanup logic that distinguishes "this version is undeletable by this API" from "delete the published versions first and retry", and test suites asserting the documented sentence — does not recognise the refusal and either retries a call that can never succeed or surfaces an unexpected-error path. The two delete APIs deliberately say different things about the same version in AWS; collapsing them onto one sentence erases that distinction.

Fix: the DeleteRecipeVersion handler's working-version guard and the message helper it calls — give this operation its own fixed refusal text rather than sharing the BatchDeleteRecipeVersion per-item ordering message. Note for the fixer: the handler is implemented (the stub issue 20261106T170634Z-databrew-deleterecipeversion-stub.md is resolved) and the REFUSAL ITSELF IS CORRECT AWS BEHAVIOR — do not make the call succeed; only the message text diverges. Related but distinct open issue 20261106T170634Z-databrew-batchdeleterecipeversion-the-per-item-conflict-message-for.md covers the batch operation's per-item project-conflict message and does not carry this check's key.

Traces: behavioral check databrew.recipe-version-publish-and-latest-working-split; emulator source services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_recipe_version.rs lines 242-251 (the guard) and crate::state::working_version_not_last_message().

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check databrew.recipe-version-publish-and-latest-working-split — verdict mismatch:

  File "check_databrew_recipe_version_publish_and_latest_working_split.py", line 93, in run
    raise AssertionError(f"unexpected rejection for deleting the draft: {message!r}")
  AssertionError: unexpected rejection for deleting the draft: 'The LATEST_WORKING version will only be deleted if the recipe has no other versions.'

Everything the check asserts before that point PASSED against the emulator: the draft is 0.1, a versionless DescribeRecipe before the first publish is refused 404 with "The recipe <Name> for version LATEST_PUBLISHED wasn't found.", publishes mint 1.0 and 2.0, published 1.0 stays frozen at one step while the draft advances to 1.1 with two steps, and ListRecipeVersions returns exactly ['1.0', '2.0']. So this is a message-wording divergence only.

DeleteRecipeVersion(Name=<recipe>, RecipeVersion="LATEST_WORKING") on a recipe holding published 1.0 and 2.0:
  expected: HTTP 400, Code "ValidationException", Message "Recipe version LATEST_WORKING is not allowed to be deleted"  (all fixed text, no request/state values)
  actual:   HTTP 400, Code "ValidationException", Message "The LATEST_WORKING version will only be deleted if the recipe has no other versions."

Status and error code match; only the message text diverges. The emulator's text is the sentence AWS reserves for BatchDeleteRecipeVersion's per-item 409 ("Recipe <Name> version LATEST_WORKING can only be deleted if all other versions got deleted."), rendered without the recipe name and with different fixed wording.

## Steps to Reproduce
1. Boot the emulator and point a databrew client at it.
2. create_recipe(Name=R, Description="draft", Steps=[{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]).
3. publish_recipe(Name=R, Description="first").
4. update_recipe(Name=R, Description="draft 2", Steps=[<the UPPER_CASE step>, {"Action": {"Operation": "RENAME", "Parameters": {"sourceColumn": "dept", "targetColumn": "DEPARTMENT"}}}]), then publish_recipe(Name=R, Description="second") so that published 1.0 and 2.0 both exist.
5. delete_recipe_version(Name=R, RecipeVersion="LATEST_WORKING") and read the ClientError. Expected: Code "ValidationException", Message exactly "Recipe version LATEST_WORKING is not allowed to be deleted". Actual: Code "ValidationException", Message "The LATEST_WORKING version will only be deleted if the recipe has no other versions.".
6. Re-replay behavioral check databrew.recipe-version-publish-and-latest-working-split to confirm the cause is closed (the check continues past line 93 into the BatchDeleteRecipeVersion per-item 409/404 assertions, which this issue does not claim).
