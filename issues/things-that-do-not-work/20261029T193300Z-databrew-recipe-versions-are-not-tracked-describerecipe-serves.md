Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-recipe-version-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-recipe-version-lifecycle
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeRecipe|4xx->2xx|$.Message
Issue-Key: databrew|DescribeRecipe|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: databrew|DescribeRecipe|4xx->2xx|status
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.Tags
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.Description
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.PublishedBy
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.PublishedDate
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.RecipeVersion
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.LastModifiedBy
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.LastModifiedDate
Issue-Key: databrew|DescribeRecipe|2xx->2xx|$.Steps[*]
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.CreateDate
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.CreatedBy
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.Description
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.Name
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.PublishedBy
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.PublishedDate
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.RecipeVersion
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.ResourceArn
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.Steps
Issue-Key: databrew|DescribeRecipe|2xx->4xx|$.Tags
Issue-Key: databrew|DescribeRecipe|2xx->4xx|status
Issue-Commit: 497ea4470f6a57da4d057b03de07c81ba40956c1

# DataBrew recipe versions are not tracked: DescribeRecipe serves the draft for every version and omits lifecycle metadata

> _Note: this issue was identified against the code base at commit `497ea4470f6a57da4d057b03de07c81ba40956c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRecipe

## Description
Five distinct causes in DataBrew's recipe-version state, all reached through DescribeRecipe over a create -> publish -> update recipe lifecycle. The emulator keeps a single mutable recipe record: there is no published-version list, no working-version numbering and no audit metadata, so every DescribeRecipe (whatever version is addressed, and after any mutation) answers with the original draft body. All five must be closed before re-submitting.

Cause 1 of 5: DescribeRecipe omits the Tags member entirely.
AWS: Smithy: DescribeRecipe output member Tags -> TagMap (map, @length 1..200); recorded behavior shows an untagged recipe answered HTTP 200 with Tags present as an empty map ({}). The member is always serialized, empty when the recipe carries no tags.
Emulator: the 200 body has no Tags member at all (absent/None) on every DescribeRecipe response in the fixture.
Why: a client that reads resp["Tags"] to enumerate or diff tags (tag-compliance sweeps, IaC drift detection, copy-a-recipe tooling) raises KeyError/NPE instead of seeing an empty map, so untagged recipes fault rather than reporting "no tags".
Fix: the DescribeRecipe response serializer - emit Tags unconditionally, defaulting to an empty map when the recipe record holds none.
Traces: databrew-recipe-version-lifecycle#task#3 (and the same omission recurs in task#6, task#8, task#9).

Cause 2 of 5: a versionless DescribeRecipe resolves the WORKING draft instead of refusing with the LATEST_PUBLISHED miss.
AWS: Smithy: DescribeRecipe input member RecipeVersion -> RecipeVersion (@length 1..16, @httpQuery "recipeVersion", optional), documented "The recipe version identifier. If this parameter isn't specified, then the latest published version is returned."; errors include ResourceNotFoundException (httpError 404). So an omitted RecipeVersion resolves the LATEST_PUBLISHED alias strictly, with no draft fallback, and a recipe that has never been published has no such version. Recorded response: HTTP 404 with the x-amzn-errortype error header present and Message "The recipe <Name> for version LATEST_PUBLISHED wasn't found." - fixed text "The recipe ", " for version ", " wasn't found."; <Name> is the request's Name and LATEST_PUBLISHED is the alias the omitted parameter defaults to (not a request value).
Emulator: the call is accepted with HTTP 200 and the draft body (RecipeVersion "0.1"); no Message and no x-amzn-errortype header are produced.
Why: callers cannot distinguish a frozen published recipe from an unsaved draft. Promotion gates, job definitions and CI checks that read DescribeRecipe without a version to get the published recipe silently receive draft steps, and the 404 that signals "nothing published yet" never arrives.
Fix: the DescribeRecipe handler's version resolution - an absent RecipeVersion must behave exactly like LATEST_PUBLISHED (no working-version fallback) and raise the modeled ResourceNotFoundException.
Note: the open behavioral issue "DescribeRecipe: versionless request falls back to the draft instead of refusing LATEST_PUBLISHED, and the version-miss message is misworded" (20261029T172132Z-databrew-describerecipe-versionless-request-falls-back-to-the-draft.md) describes the same fault from the behavioral corpus but carries no wire replay key, so this wire step is filed here; closing both together is expected.
Traces: databrew-recipe-version-lifecycle#task#4.

Cause 3 of 5: after PublishRecipe, DescribeRecipe for the published version still serves the draft and has no publish metadata.
AWS: Smithy: PublishRecipe (POST /recipes/{Name}/publishRecipe, input Description + Name) freezes the current working version as the next published version; DescribeRecipe output then carries RecipeVersion, PublishedBy and PublishedDate (Date timestamp) alongside Description/Steps. Recorded behavior: the first publish produces version "1.0" whose Description is the publish request's Description, with PublishedBy set to the calling identity's ARN and PublishedDate set to the publish time.
Emulator: the response is HTTP 200 but reports the draft - Description still the create-time draft description, RecipeVersion "0.1" instead of the published version label, and PublishedBy / PublishedDate / Tags absent.
Why: a client that publishes and then reads back the published recipe sees the draft text and a draft version number, so it cannot verify what it published, cannot record who/when published, and will pin jobs to a version label the service never reports.
Fix: the recipe state record plus the DescribeRecipe handler - maintain a published-version list with its own Description/Steps snapshot and PublishedBy/PublishedDate, and serve the addressed published version rather than the working copy.
Note: PublishRecipe is itself an unimplemented scaffold stub (open issue "Implement databrew PublishRecipe: unimplemented scaffold stub"); that issue owns the publish-side handler, this cause owns the version record and the DescribeRecipe view of it.
Traces: databrew-recipe-version-lifecycle#task#6.

Cause 4 of 5: after UpdateRecipe, DescribeRecipe of the working version serves the pre-update body and no modification metadata.
AWS: Smithy: UpdateRecipe (PUT /recipes/{Name}, input Description + Steps) replaces the working version's Description and Steps; DescribeRecipe output then carries the new Description, the full new Steps list, LastModifiedBy (caller ARN) and LastModifiedDate, and the working version label advances past the last published one (recorded: "1.1" once version 1.0 exists).
Emulator: the response still reports the original draft Description, only the original single step (the step added by the update is missing from Steps), RecipeVersion "0.1", and LastModifiedBy / LastModifiedDate / Tags absent - i.e. the update is not reflected in state at all.
Why: a recipe edit is silently dropped. Anything that updates steps and then reads them back (interactive editors, migrations, a job that runs the recipe) operates on stale transformation logic, and the version label never advances so callers cannot tell a modified draft from the published version.
Fix: the UpdateRecipe handler and the recipe state record - apply Description/Steps to the working version, stamp LastModifiedBy/LastModifiedDate, and derive the working version label from the highest published version; DescribeRecipe must serve that record.
Traces: databrew-recipe-version-lifecycle#task#8.

Cause 5 of 5: DescribeRecipe for an explicitly named published version is refused 404 although that version exists.
AWS: Smithy: DescribeRecipe RecipeVersion is a version identifier (numeric label or LATEST_PUBLISHED/LATEST_WORKING alias); a published version remains addressable by its label and returns HTTP 200 with CreatedBy, CreateDate, Description, Name, PublishedBy, PublishedDate, RecipeVersion, ResourceArn (arn:aws:databrew:<region>:<account-id>:recipe/<Name>), Steps and Tags.
Emulator: the request is refused HTTP 404 and the whole body is absent, because only the single draft record exists and the numeric published label resolves to nothing.
Why: the published version is unreachable, so every consumer that addresses a pinned recipe version - DataBrew jobs, rollbacks, audits of what a run used - gets ResourceNotFoundException for a version the service reported as created.
Fix: the recipe state's version lookup (shared with Cause 3) - register each published version under its label and resolve numeric labels and both aliases against it in the DescribeRecipe handler.
Traces: databrew-recipe-version-lifecycle#task#9.

## Evidence
Replay of fixture databrew-recipe-version-lifecycle against the emulator (trace set simparity-wire-aws-v1); all five steps are DescribeRecipe, verdict mismatch:

task#3 - expected 200 / actual 200: $.Tags missing (expected {}, got None).

task#4 - expected 404, actual 200: $.Message missing (expected "The recipe <Name> for version LATEST_PUBLISHED wasn't found.", got None); $headers.x-amzn-errortype missing (expected an error-type header, got none). The emulator returned a DescribeRecipeResponse body instead of ResourceNotFoundException.

task#6 - expected 200 / actual 200: $.Description value_mismatch (expected the published description, got the draft description 'wire draft'); $.RecipeVersion value_mismatch (expected '1.0', got '0.1'); $.PublishedBy missing (expected the caller identity ARN, got None); $.PublishedDate missing (expected a publish timestamp, got None); $.Tags missing.

task#8 - expected 200 / actual 200: $.Description value_mismatch (expected the updated description, got the pre-update 'wire draft'); $.RecipeVersion value_mismatch (expected '1.1', got '0.1'); $.Steps[1] missing (expected the second step added by UpdateRecipe - Action.Operation RENAME with sourceColumn/targetColumn parameters - got None); $.LastModifiedBy missing (expected the caller identity ARN, got None); $.LastModifiedDate missing; $.Tags missing.

task#9 - expected 200, actual 404: the entire published-version body is absent - $.Name, $.Description, $.RecipeVersion ('1.0'), $.Steps, $.CreatedBy, $.CreateDate, $.PublishedBy, $.PublishedDate, $.ResourceArn (arn:aws:databrew:<region>:<account-id>:recipe/<Name>) and $.Tags all missing.

Model evidence (smithy_query on services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): DescribeRecipe GET /recipes/{Name}, input {Name @required @httpLabel, RecipeVersion @httpQuery recipeVersion @length 1..16 - "If this parameter isn't specified, then the latest published version is returned."}, output {CreatedBy, CreateDate, LastModifiedBy, LastModifiedDate, PublishedBy, PublishedDate, Description, Name @required, Steps, Tags, ResourceArn, RecipeVersion}, errors [ResourceNotFoundException httpError 404, ValidationException httpError 400]. PublishRecipe POST /recipes/{Name}/publishRecipe {Description, Name}; UpdateRecipe PUT /recipes/{Name} {Description, Name, Steps}.

## Steps to Reproduce
1. Boot the emulator and point a databrew client at it.
2. create_recipe(Name=R, Description="wire draft", Steps=[{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]).
3. describe_recipe(Name=R, RecipeVersion="LATEST_WORKING") -> 200. Expected: Tags present as {}. Actual: no Tags member (Cause 1).
4. describe_recipe(Name=R) with no RecipeVersion. Expected: 404 ResourceNotFoundException, x-amzn-errortype header set, Message "The recipe R for version LATEST_PUBLISHED wasn't found.". Actual: 200 with the draft body, RecipeVersion "0.1" (Cause 2).
5. publish_recipe(Name=R, Description="wire published 1"), then describe_recipe(Name=R, RecipeVersion="LATEST_PUBLISHED"). Expected: the published Description, RecipeVersion "1.0", PublishedBy/PublishedDate set. Actual: draft Description, RecipeVersion "0.1", PublishedBy/PublishedDate absent (Cause 3).
6. update_recipe(Name=R, Description="wire draft 2", Steps=[<the UPPER_CASE step>, {"Action": {"Operation": "RENAME", "Parameters": {"sourceColumn": "dept", "targetColumn": "DEPARTMENT"}}}]), then describe_recipe(Name=R, RecipeVersion="LATEST_WORKING"). Expected: new Description, both steps, RecipeVersion "1.1", LastModifiedBy/LastModifiedDate set. Actual: pre-update Description, one step, "0.1", no LastModified* (Cause 4).
7. describe_recipe(Name=R, RecipeVersion="1.0"). Expected: 200 with the full published body incl. ResourceArn. Actual: 404 (Cause 5).
8. Re-replay fixture databrew-recipe-version-lifecycle steps task#3, task#4, task#6, task#8 and task#9 to confirm all five causes are closed.
