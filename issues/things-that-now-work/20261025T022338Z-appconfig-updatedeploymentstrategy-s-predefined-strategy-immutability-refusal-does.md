Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.predefined-deployment-strategy-catalogue
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.predefined-deployment-strategy-catalogue|run#17|AssertionError|assert message == f"Cannot update predefined Deployment Strategy {before['Id']}", (
Issue-Commit: a577dca75911a15e8d14eba87791fcef54f4bfc0

# AppConfig UpdateDeploymentStrategy's predefined-strategy immutability refusal does not use AWS's request-derived message template

> _Note: this issue was identified against the code base at commit `a577dca75911a15e8d14eba87791fcef54f4bfc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDeploymentStrategy

## Description
One root cause, grounded in one replayed behavioral check. The HTTP status and the error code already MATCH; the only divergence is the refusal's Message, which is an emulator-authored sentence instead of AppConfig's request-derived template.

=== Cause 1 of 1: the 'cannot update a predefined deployment strategy' refusal uses an invented sentence ===
AWS: UpdateDeploymentStrategy is PATCH /deploymentstrategies/{DeploymentStrategyId}, success code 200, output shape DeploymentStrategy; its modeled errors are BadRequestException 400, ResourceNotFoundException 404, InternalServerException 500 (Smithy: UpdateDeploymentStrategy, DeploymentStrategyId @httpLabel @required @pattern ^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$ — the second alternation branch is exactly the AWS-predefined strategy id form). The four AWS-predefined strategies (AppConfig.AllAtOnce, AppConfig.Linear50PercentEvery30Seconds, AppConfig.Canary10Percent20Minutes, AppConfig.Linear20PercentEvery6Minutes) are reference data and are immutable: a PATCH naming one is refused with BadRequestException, HTTP 400, and the template 'Cannot update predefined Deployment Strategy <DeploymentStrategyId>'. The sentence itself is recorded behavior only (captured golden corpus), but its RULE is known because it is built from the request: it is the fixed prefix 'Cannot update predefined Deployment Strategy ' followed by the submitted predefined strategy id. That template is the requirement here, never any one run's id value.
Emulator: the correct 400 BadRequestException and the correct refusal decision, but the message is a different sentence shape — an example from the replay: 'Deployment strategy AppConfig.Canary10Percent20Minutes is an AppConfig predefined deployment strategy and cannot be updated.' It names the strategy but does not follow AppConfig's template (different word order, different wording, trailing period where AWS emits none).
Why: a client that classifies or asserts on AppConfig refusal text — error classifiers, log scrapers, snapshot/contract tests, CDK and CloudFormation tooling that special-cases the predefined strategies by name — matches AppConfig's sentence and does not match the emulator's, while the status and code it also checks both look correct. The divergence is therefore invisible to code inspection and only shows up as a false negative in the client's own assertions. This is the same class of fault as open issue 20261024T131534Z-appconfig-refusal-messages-on-the-tagging-pagination-hosted.md (five request-derived refusal templates) and the resolved 20261023T164118Z-appconfig-refusal-messages-do-not-use-aws-s.md; NEITHER covers this path — the open issue names TagResource/UntagResource/ListTagsForResource, the pagination-token validators, CreateHostedConfigurationVersion, CreateExperimentDefinition and UpdateExtension, and the resolved one names CreateConfigurationProfile, GetEnvironment, UpdateEnvironment, DeleteEnvironment, GetDeploymentStrategy, GetExtension and StartDeployment. UpdateDeploymentStrategy's predefined guard appears in none of them, so this is net-new. It is NOT a reopen of the resolved issue either: the call is refused exactly as AWS refuses it, with AWS's status and code.
Fix: the predefined-strategy guard branch in UpdateDeploymentStrategy's handler (the appconfig deployment-strategy ops module) must render AWS's template from the submitted DeploymentStrategyId rather than its own sentence; it belongs with the shared appconfig refusal-message builder the sibling refusal-message issues establish, not duplicated per handler. Open issue 20261023T145538Z-appconfig-updatedeploymentstrategy-does-not-persist-the-updated-fields.md covers the SUCCESS path of this same operation (dropped mutations) and is a separate cause from this refusal message.
Traces: appconfig.predefined-deployment-strategy-catalogue.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1), check appconfig.predefined-deployment-strategy-catalogue [mismatch].

Every earlier leg of the check PASSED, which scopes the defect precisely to the message: ListDeploymentStrategies returned all four AWS-predefined strategies with no duplicate ids across pages, GetDeploymentStrategy resolved each by its literal id, every predefined parameter value (Description, DeploymentDurationInMinutes, FinalBakeTimeInMinutes, GrowthFactor, GrowthType, ReplicateTo) matched, GrowthFactor was a float, and the positive control (UpdateDeploymentStrategy on a caller-created strategy, GrowthFactor 5.0 -> 25.0) succeeded and persisted.

The failing assertion:
  AssertionError: expected the exact BadRequestException message AWS returns, got 'Deployment strategy AppConfig.Canary10Percent20Minutes is an AppConfig predefined deployment strategy and cannot be updated.'

Expected per AWS's template, for the id submitted in the request:
  Cannot update predefined Deployment Strategy AppConfig.Canary10Percent20Minutes

The error code assertion (BadRequestException) passed, so the status 400 and code are already correct. The id 'AppConfig.Canary10Percent20Minutes' is the id this run submitted and is an EXAMPLE; the requirement is the template with whatever predefined id the request names. The 'effect absent' leg after the refusal was never reached, but nothing in the replay suggests the refused PATCH mutated the strategy.

## Steps to Reproduce
Boot the emulator and point an appconfig client at its data listener.
1. get_deployment_strategy(DeploymentStrategyId='AppConfig.Canary10Percent20Minutes') — succeeds, establishing the predefined strategy exists.
2. update_deployment_strategy(DeploymentStrategyId='AppConfig.Canary10Percent20Minutes', DeploymentDurationInMinutes=5, GrowthType='LINEAR').
Expected: HTTP 400 BadRequestException with Message 'Cannot update predefined Deployment Strategy AppConfig.Canary10Percent20Minutes'.
Actual: HTTP 400 BadRequestException (correct) with Message 'Deployment strategy AppConfig.Canary10Percent20Minutes is an AppConfig predefined deployment strategy and cannot be updated.'
3. Repeat with each of the other predefined ids (AppConfig.AllAtOnce, AppConfig.Linear50PercentEvery30Seconds, AppConfig.Linear20PercentEvery6Minutes) to confirm the template is rendered from the submitted id, not hardcoded for one.
4. Confirm the regression guard still holds: update_deployment_strategy against a caller-created strategy must still succeed (the guard must not widen into a blanket refusal).
Re-replay appconfig.predefined-deployment-strategy-catalogue to confirm the fix.
