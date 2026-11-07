Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-recipe-version-lifecycle
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-recipe-version-lifecycle
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|ListRecipeVersions|2xx->2xx|$.Recipes[*].Description
Issue-Commit: b5a9f3dd62ee9fc35f8e9b6468c76fff808fb23b

# DataBrew ListRecipeVersions reports the publish-request description instead of the version's own snapshot

> _Note: this issue was identified against the code base at commit `b5a9f3dd62ee9fc35f8e9b6468c76fff808fb23b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecipeVersions

## Description
One cause, in DataBrew's recipe-version state as seen through ListRecipeVersions.

Cause 1 of 1: a listed recipe version carries the Description from the most recent PublishRecipe request rather than the Description that version was frozen with.
AWS: Smithy: ListRecipeVersions GET /recipeVersions, input {Name @required @httpQuery "name" @length 1..255, MaxResults @httpQuery maxResults @range 1..100, NextToken @httpQuery nextToken}, output {Recipes @required -> RecipeList of Recipe, NextToken}, errors [ValidationException httpError 400]; Recipe.Description -> RecipeDescription @length 0..1024, documented "The description of the recipe." Each element of Recipes is one version of the recipe, so its Description is the description that version holds. PublishRecipe.Description is documented "A description of the recipe to be published, for this version of the recipe." Recorded behavior: after CreateRecipe with Description "wire draft" and a subsequent PublishRecipe whose request Description was "wire published 1", the recorded HTTP 200 ListRecipeVersions response reports the listed version's Description as the recipe's own description ("wire draft") — the publish request's text does not become the description reported for the listed version. The compared value is the version's description string, not these particular recording values.
Emulator: the call answers HTTP 200 but $.Recipes[0].Description is the Description supplied on the preceding PublishRecipe call ("wire published 1" in the fixture) instead of the version's own description ("wire draft"). Hypothesis: the emulator keeps a single mutable recipe record with one Description field, PublishRecipe overwrites that field with its request Description, and ListRecipeVersions serializes that one record as the version list — so there is no per-version snapshot to report.
Why: a client that lists recipe versions to show, diff or choose a version (recipe-version pickers, promotion/rollback tooling, audits of what a job ran) reads a description that belongs to the last publish call rather than to the version it is looking at. Every listed version ends up describing the same thing, so versions become indistinguishable and a pinned version is labelled with text it was never created with.
Fix: the recipe state's version records plus the ListRecipeVersions response serializer — each published version must own an immutable snapshot of its Description (and Steps) taken when it was frozen, and ListRecipeVersions must serve those snapshots instead of the single mutable recipe record; PublishRecipe's Description must not retroactively mutate the description reported for existing versions.
Related: the open issue "DataBrew recipe versions are not tracked: DescribeRecipe serves the draft for every version and omits lifecycle metadata" (20261029T193300Z-databrew-recipe-versions-are-not-tracked-describerecipe-serves.md) covers the DescribeRecipe view of the same missing version state and owns fixture steps task#3/task#4/task#6/task#8/task#9; it does not cover ListRecipeVersions and carries no replay key for task#10, so this divergence is filed here. Closing both together is expected.
Traces: databrew-recipe-version-lifecycle#task#10.

## Evidence
Replay of fixture databrew-recipe-version-lifecycle against the emulator (trace set simparity-wire-aws-v1), step task#10, operation ListRecipeVersions, verdict mismatch, expected status 200 / actual status 200:

  $.Recipes[0].Description: value_mismatch (expected 'wire draft', got 'wire published 1')

'wire draft' is the Description the recipe was created with and which the recorded response reports for the listed version; 'wire published 1' is the Description sent in the PublishRecipe request earlier in the same fixture. Status, headers and all other members of the response matched; only the version's description value diverges.

Model evidence (smithy_query on services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): ListRecipeVersions GET /recipeVersions, output {Recipes @required -> RecipeList of Recipe, NextToken @length 1..2000}, paginated {inputToken NextToken, outputToken NextToken, items Recipes, pageSize MaxResults}; Recipe {Name @required, Description @length 0..1024 "The description of the recipe.", RecipeVersion @length 1..16 (numeric X.Y, LATEST_WORKING or LATEST_PUBLISHED), Steps, CreatedBy, CreateDate, LastModifiedBy, LastModifiedDate, PublishedBy, PublishedDate, ResourceArn, Tags}; PublishRecipe POST /recipes/{Name}/publishRecipe, input Description "A description of the recipe to be published, for this version of the recipe."

## Steps to Reproduce
1. Boot the emulator and point a databrew client at it.
2. create_recipe(Name=R, Description="wire draft", Steps=[{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]).
3. publish_recipe(Name=R, Description="wire published 1").
4. list_recipe_versions(Name=R) -> HTTP 200. Expected: the listed version's Description is the recipe's own description, "wire draft". Actual: Recipes[0].Description is "wire published 1", the Description passed to publish_recipe.
5. Re-replay fixture databrew-recipe-version-lifecycle step task#10 to confirm the cause is closed.
