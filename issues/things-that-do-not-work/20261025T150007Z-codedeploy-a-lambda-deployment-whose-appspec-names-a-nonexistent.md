Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: d2e421bd781628705310382509b1712cb5427402

# A Lambda deployment whose AppSpec names a nonexistent alias never reaches a terminal Failed/INVALID_LAMBDA_FUNCTION outcome

> _Note: this issue was identified against the code base at commit `d2e421bd781628705310382509b1712cb5427402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
One cause.

Cause 1 of 1: the emulator never resolves an AppSpec's Lambda alias reference against the lambda service, so a deployment naming an alias that does not exist does not terminate as a Failed deployment carrying errorInformation.code INVALID_LAMBDA_FUNCTION, and the check that observes that outcome (plus the two effect-absent assertions around it) cannot complete.

AWS: Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) models DeploymentInfo.status with DeploymentStatus = Created, Queued, InProgress, Baking, Succeeded, Failed, Stopped, Ready, and DeploymentInfo.errorInformation as ErrorInformation{code: ErrorCode, message: ErrorMessage}; ErrorCode includes INVALID_LAMBDA_FUNCTION (alongside INVALID_LAMBDA_CONFIGURATION, INVALID_REVISION, IAM_ROLE_PERMISSIONS). Recorded behavior (measured us-west-2 by the check): a well-formed AppSpec whose Alias property names an alias that was never created is NOT refused by CreateDeployment — the call returns HTTP 200 with a deploymentId — and the verdict appears later as that deployment's own terminal state: status 'Failed' with errorInformation.code 'INVALID_LAMBDA_FUNCTION' and errorInformation.message of the fixed form "The deployment failed because either the target Lambda function <FunctionName> does not exist or the specified function version or alias cannot be found", where <FunctionName> is the function name taken from the request's AppSpec (the rest of that sentence is fixed text; no recorded id, version or timestamp is part of it). Two effects must be ABSENT: the named alias must still not exist afterwards (the AppSpec's Alias property is a reference, never an upsert — lambda GetAlias on it must still answer ResourceNotFoundException with the message "Cannot find alias arn: <functionArn>:<aliasName>", both values taken from the request/state), and the unrelated real alias must stay at its current version with no residual AdditionalVersionWeights. The same AppSpec naming the real alias succeeds and shifts that alias, so alias existence — i.e. account state, not schema validity — is the only thing deciding the outcome.

Emulator: the check never completes — "Check timed out after 60.0s" — even though all three of its deployments are CodeDeployDefault.LambdaAllAtOnce and measured about 5 s each on real AWS, and the check's own polls allow 420 s for a deployment and 120 s for an alias. No deployment is ever observed in a terminal state with errorInformation.code INVALID_LAMBDA_FUNCTION, and no AppSpec alias reference is resolved against the lambda service state. Hypothesis: the AppSpec is stored as an opaque document and the deployment's status is flipped without parsing it or querying Lambda, so both the positive control (poll the real alias until it reaches the AppSpec's TargetVersion) and the negative leg (poll for Failed + INVALID_LAMBDA_FUNCTION) poll for a state that can never arrive.

Why: a client gets no signal that it deployed against a referent that does not exist. The deployment either reports success for an AppSpec pointing at nothing, or hangs in a non-terminal state — in both cases a CI/CD gate that treats the terminal status as 'the new code is live' is wrong, and the modeled INVALID_LAMBDA_FUNCTION error code is unreachable. The accompanying over-permissive risk is an implementation that treats Alias as an upsert and creates the alias it was merely asked to read.

Fix: the deployment execution path of the codedeploy service — parse the AppSpec, resolve its function/alias/CurrentVersion/TargetVersion against the lambda service state, and when the alias cannot be resolved terminate the deployment with status 'Failed' and errorInformation {code: INVALID_LAMBDA_FUNCTION, message: as worded above} without creating the alias and without touching any other alias (services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/create_deployment.rs and src/state.rs).

Related open issue that does NOT cover this cause: 20261024T114701Z-codedeploy-deployments-report-success-without-executing-the-lambda.md — its eight causes cover the alias not moving on a SUCCESSFUL deployment, time-based shifting, deployment targets, the service role, S3 revision resolution, the on-premises terminal status, per-group concurrency and on-premises deregistration. None of them covers resolving an AppSpec alias REFERENCE to a Failed/INVALID_LAMBDA_FUNCTION outcome or the must-not-create-the-alias effect-absence. 20261022T171541Z-codedeploy-createdeploymentgroup-and-createdeployment-accept-input-real-aws.md and 20261022T213118Z-codedeploy-accepts-invalid-input-and-misclassifies-rejections.md cover create-time request validation only, and this behavior is explicitly NOT a refusal of CreateDeployment.

Traces: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 56/91 passed, bootstrap_signal false (the emulator was serving); codedeploy 56/86.

Failing check (verdict: mismatch): codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created — "Error: Check timed out after 60.0s".

Expected (measured on real AWS, us-west-2, per the check's own recorded constants and assertions):
- CreateDeployment with a well-formed AppSpecContent naming a nonexistent alias is ACCEPTED (returns a deploymentId; it is not refused).
- GetDeployment(deploymentId).deploymentInfo.status == 'Failed'.
- deploymentInfo.errorInformation.code == 'INVALID_LAMBDA_FUNCTION'.
- deploymentInfo.errorInformation.message == "The deployment failed because either the target Lambda function <FunctionName> does not exist or the specified function version or alias cannot be found" (fixed text; <FunctionName> comes from the request).
- lambda GetAlias(FunctionName=<fn>, Name=<ghost>) still raises ResourceNotFoundException with message "Cannot find alias arn: <functionArn>:<ghost>" (fixed text "Cannot find alias arn: ", then the function ARN from state and the alias name from the request).
- lambda GetAlias on the untouched real alias still reports its prior FunctionVersion and no RoutingConfig.AdditionalVersionWeights.
- The identical AppSpec naming the REAL alias ends 'Succeeded' and moves that alias to the AppSpec's TargetVersion.

Actual (emulator): the check does not finish inside the suite's 60 s per-check cap. The check's deployments are LambdaAllAtOnce and measured ~5 s each on real AWS, and its internal waits are 420 s (deployment terminal) / 120 s (alias version), so the cap is reached while polling for a state the emulator never produces: no Failed deployment carrying errorInformation.code INVALID_LAMBDA_FUNCTION is ever observed, and the AppSpec's alias reference is never resolved against the lambda service.

Smithy backing: ErrorCode enum includes INVALID_LAMBDA_FUNCTION; ErrorInformation = {code: ErrorCode, message: ErrorMessage}; DeploymentStatus includes Failed. Function names, alias names, ARNs, version numbers and deployment ids in the check are environment-specific values that fill the message templates; the requirements are the terminal status, the error code, the message wording and the two absent effects.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite); check codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created reproduces.
2. Create a Lambda execution role, create a 128MB Lambda function, publish versions 1, 2 and 3, and create an alias (e.g. 'live') pointing at version 1.
3. Pick an alias name that does not exist (e.g. 'ghost<suffix>') and confirm lambda GetAlias(FunctionName=<fn>, Name=<ghost>) raises ResourceNotFoundException with message "Cannot find alias arn: <functionArn>:<ghost>".
4. CreateApplication(computePlatform='Lambda') and CreateDeploymentGroup(deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce') with a CodeDeploy service role.
5. Positive control first: CreateDeployment with an AppSpecContent revision naming the function, the REAL alias, CurrentVersion='1', TargetVersion='2'; poll GetDeployment to terminal -> expect 'Succeeded', then lambda GetAlias(Name='live') -> expect FunctionVersion '2'.
6. Change exactly one thing — the alias name inside the AppSpec — to the nonexistent <ghost> (CurrentVersion='2', TargetVersion='3') and CreateDeployment. Expect HTTP 200 with a deploymentId (NOT a refusal).
7. Poll GetDeployment(deploymentId) to a terminal state -> expect status 'Failed' with errorInformation.code 'INVALID_LAMBDA_FUNCTION' and errorInformation.message "The deployment failed because either the target Lambda function <fn> does not exist or the specified function version or alias cannot be found". Observe instead that no such terminal state arrives (the check's poll is cut off at the suite's 60 s cap).
8. Effect absent 1: lambda GetAlias(FunctionName=<fn>, Name=<ghost>) -> expect ResourceNotFoundException "Cannot find alias arn: <functionArn>:<ghost>"; the alias must NOT have been created by the deployment that merely named it.
9. Effect absent 2: lambda GetAlias(FunctionName=<fn>, Name='live') -> expect FunctionVersion still '2' and no RoutingConfig.AdditionalVersionWeights.
10. Second positive control: CreateDeployment for the same 2 -> 3 shift naming the REAL alias -> expect 'Succeeded' and the alias at '3'; then ListDeployments (paginated) for the group -> expect all three deployment ids.
