Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__databrew.deploy-databrew-dataset-and-ruleset-round-trip-and-delete cloudformation__databrew.deploy-databrew-recipe-and-schedule-round-trip-and-delete cloudformation__databrew.deploy-databrew-recipe-invalid-operation-rejected-and-rolled-back
Issue-Key: cloudformation__databrew.deploy-databrew-dataset-and-ruleset-round-trip-and-delete|run#13|AssertionError|assert listed == [out["DatasetRef"]], f"ListDatasets named {dataset_name!r}: {listed!r}"
Issue-Key: cloudformation__databrew|ListRecipes|NotImplemented
Issue-Key: cloudformation__databrew|DescribeRecipe|ResourceNotFoundException
Issue-Commit: 2e3afc6c699aa7e5b0dfd114974fd7ed6d393583

# cloudformation__databrew: 2 net-new causes — AWS::DataBrew::* resources are stub-provisioned (stack reaches CREATE_COMPLETE but nothing exists in DataBrew), and databrew ListRecipes is an unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e3afc6c699aa7e5b0dfd114974fd7ed6d393583`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes, both reached through CloudFormation stacks that declare AWS::DataBrew::* resources. Neither is covered by an existing issue (the three open `databrew` issues are about recipe versioning, ListRecipeVersions descriptions and DescribeJobRun output config; no open issue mentions cloudformation__databrew or ListRecipes).

=== Cause 1 of 2: AWS::DataBrew::Dataset / ::Ruleset / ::Recipe / ::Schedule are stub-provisioned — the stack reports CREATE_COMPLETE and resolves Ref, but no resource is ever created in DataBrew state ===
Failing check ids: cloudformation__databrew.deploy-databrew-dataset-and-ruleset-round-trip-and-delete, cloudformation__databrew.deploy-databrew-recipe-invalid-operation-rejected-and-rolled-back

AWS: a CloudFormation resource type's create handler calls the owning service's control-plane API, so once the stack reports CREATE_COMPLETE the resource is a first-class DataBrew resource and is visible through DataBrew's own read APIs. Smithy (com-amazonaws-databrew-awsgluedatabrew): ListDatasets -> GET /datasets, output member `Datasets` is @required (a list of every dataset in the account/region); DescribeDataset -> GET /datasets/{Name}, 200 with @required Name and Input, errors ResourceNotFoundException (404) / ValidationException (400); ListRulesets -> GET /rulesets; DescribeRecipe -> GET /recipes/{Name} with `recipeVersion` query, 200 with @required Name plus Steps/Tags, errors ResourceNotFoundException (404) / ValidationException (400). So for a stack whose AWS::DataBrew::Dataset named N reached CREATE_COMPLETE, AWS answers ListDatasets with an entry whose Name is N (and equal to the resource's Ref), and DescribeRecipe(Name=N, RecipeVersion=LATEST_WORKING) answers 200 with that recipe's Steps — ResourceNotFoundException is reserved for a name that does not exist.

Emulator: the DataBrew resource types ARE registered (CreateStack is accepted, the stack settles CREATE_COMPLETE, and the stack Outputs carry a resolved DatasetRef/BucketRef/RulesetRef), but nothing is written into DataBrew's state. Replay evidence: after CREATE_COMPLETE, `ListDatasets` paged in full returns no entry at all for the stack's dataset name — AssertionError: ListDatasets named 'sim2real-dbdataset-<suffix>': [] (expected exactly [<the DatasetRef output>]). In the second check the control stack that declares a single AWS::DataBrew::Recipe with a valid step also reaches CREATE_COMPLETE, and the immediately following DescribeRecipe refuses: ResourceNotFoundException — "The recipe <name> for version LATEST_WORKING wasn't found." (fixed text: "The recipe " + " for version " + " wasn't found."; variable parts: the recipe Name and the RecipeVersion taken from the request — here the name the template declared). The refusal itself is correctly worded; the defect is that it is served for a resource the stack claims to have created.

Why: a client that deploys DataBrew estate through CloudFormation gets a green stack and a usable Ref, then every DataBrew call against that identifier fails or returns nothing. Datasets/rulesets/recipes/schedules cannot be listed, described, attached to jobs or profiled; teardown verification (the ResourceNotFound-after-delete contract) is indistinguishable from the create having silently done nothing; and infrastructure-as-code pipelines that assert on post-deploy readback break while CloudFormation reports success.

Fix: the CloudFormation resource provisioners for the AWS::DataBrew::* resource types (the resource-type registry's create/update/delete handlers in the cloudformation service crate) must drive the real DataBrew control-plane operations — CreateDataset/CreateRuleset/CreateRecipe/CreateSchedule on create, the matching Update* on update, Delete* on delete — writing into the databrew service crate's state store (services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs) rather than synthesizing a physical id and returning. Tag propagation belongs in the same place: the checks additionally require the stack-level tag plus the template's own resource Tags to read back through DescribeDataset/DescribeRuleset/DescribeRecipe/DescribeSchedule, and the stack-driven delete to leave DataBrew answering the exact ResourceNotFoundException for that name.

Traces: cloudformation__databrew.deploy-databrew-dataset-and-ruleset-round-trip-and-delete (ListDatasets after CREATE_COMPLETE returns []), cloudformation__databrew.deploy-databrew-recipe-invalid-operation-rejected-and-rolled-back (DescribeRecipe on the CREATE_COMPLETE control stack's recipe answers ResourceNotFoundException).

=== Cause 2 of 2: databrew ListRecipes answers NotImplemented ===
Failing check id: cloudformation__databrew.deploy-databrew-recipe-and-schedule-round-trip-and-delete

AWS: Smithy models ListRecipes as a normal paginated read — GET /recipes, HTTP 200, output `Recipes` @required (RecipeList) plus optional NextToken, paginated {inputToken: NextToken, outputToken: NextToken, items: Recipes, pageSize: MaxResults}, input RecipeVersion as the `recipeVersion` query parameter. Its ONLY modeled error is ValidationException (400). There is no modeled failure mode in which ListRecipes refuses a well-formed request, so AWS answers 200 with the recipe list (empty list when the account has none).

Emulator: ListRecipes is a scaffold stub and refuses outright — ClientError (NotImplemented) when calling the ListRecipes operation: "operation ListRecipes is scaffolded but not implemented in this emulator". NotImplemented is not a member of the operation's error union at all, and the message is emulator prose rather than any AWS error text.

Why: every client that enumerates recipes breaks, including paginator-driven ones that cannot distinguish "no recipes" from "service refuses". It also masks Cause 1 for recipe/schedule stacks: the round-trip check dies on ListRecipes before it can even reach the DescribeRecipe/DescribeSchedule readback, so recipe and schedule provisioning fidelity is untestable until this stub is filled in.

Fix: implement the ListRecipes handler in the databrew service crate (move it out of services/service-com-amazonaws-databrew-awsgluedatabrew/src/scaffolded.rs into src/ops/), reading recipes from the crate's state store, honouring the recipeVersion/maxResults/nextToken query parameters and always emitting the required `Recipes` member.

Traces: cloudformation__databrew.deploy-databrew-recipe-and-schedule-round-trip-and-delete.

## Evidence
Suite: 7/10 passed. Per-service: cloudformation__databrew 0/3 (0%), databrew 7/7 ✓.

Cause 1, check `deploy-databrew-dataset-and-ruleset-round-trip-and-delete` [mismatch]:
  CreateStack(template with AWS::S3::Bucket + AWS::DataBrew::Dataset + AWS::DataBrew::Ruleset) -> stack settles CREATE_COMPLETE, Outputs resolve BucketRef/DatasetRef/RulesetRef.
  Expected (AWS): ListDatasets (all pages) contains exactly one entry whose Name == the DatasetRef output.
  Actual (emulator): no entry — AssertionError: ListDatasets named 'sim2real-dbdataset-886d46bfec1d': []

Cause 1, check `deploy-databrew-recipe-invalid-operation-rejected-and-rolled-back` [error]:
  CreateStack(template with one AWS::DataBrew::Recipe, Steps=[{Action:{Operation:UPPER_CASE, Parameters:{sourceColumn:col1}}}]) -> CREATE_COMPLETE.
  Expected (AWS): DescribeRecipe(Name=<recipe>, RecipeVersion="LATEST_WORKING") -> 200, Steps == [{Action:{Operation:UPPER_CASE,Parameters:{sourceColumn:col1}}, ConditionExpressions:[]}].
  Actual (emulator): botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeRecipe operation: The recipe sim2real-dbrecipeok-e8c0bb56b19e for version LATEST_WORKING wasn't found.
  (fixed text "The recipe … for version … wasn't found."; the name and version are request values — the per-run suffix in the name is not part of the requirement.)

Cause 2, check `deploy-databrew-recipe-and-schedule-round-trip-and-delete` [error]:
  CreateStack(AWS::DataBrew::Recipe + AWS::DataBrew::Schedule) -> CREATE_COMPLETE, then ListRecipes(RecipeVersion="LATEST_WORKING") via paginator.
  Expected (AWS): HTTP 200 with the required `Recipes` member (Smithy: ListRecipes GET /recipes, 200, errors = {ValidationException}).
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListRecipes operation: operation ListRecipes is scaffolded but not implemented in this emulator

## Steps to Reproduce
Cause 1:
1. cloudformation.CreateStack with a template declaring {"Bucket": AWS::S3::Bucket, "Dataset": AWS::DataBrew::Dataset {Name: <n>, Input.S3InputDefinition {Bucket: {Ref: Bucket}, Key: "data.csv"}, Tags:[{Key: sim2real-ds, Value: <n>}]}, "Ruleset": AWS::DataBrew::Ruleset {Name: <r>, TargetArn: Fn::Sub dataset ARN, Rules:[{Name: rule1, CheckExpression: "AGG(DUPLICATE_ROWS_COUNT) < :val", SubstitutionMap:[{ValueReference: ":val", Value: "100"}]}]}} and Outputs Ref-ing each.
2. Poll DescribeStacks until CREATE_COMPLETE (it reaches it) and read Outputs.
3. databrew.ListDatasets (all pages) and filter on Name == <n> -> empty; DescribeDataset(Name=<n>) / DescribeRuleset(Name=<r>) likewise do not resolve.
4. Same with a single-resource AWS::DataBrew::Recipe template: after CREATE_COMPLETE, databrew.DescribeRecipe(Name=<recipe>, RecipeVersion="LATEST_WORKING") answers ResourceNotFoundException instead of 200.

Cause 2:
1. Call databrew.ListRecipes (e.g. list_recipes(RecipeVersion="LATEST_WORKING"), or the list_recipes paginator) against the emulator with no other setup.
2. Observe HTTP 501 / error code NotImplemented with the message "operation ListRecipes is scaffolded but not implemented in this emulator" instead of a 200 carrying `Recipes`.
