Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.continue-deployment-already-completed-rejected codedeploy.skip-wait-time-triggers-immediate-termination
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.continue-deployment-already-completed-rejected|run#12|AssertionError|assert code == "UnsupportedActionForDeploymentTypeException", f"unexpected error code: {code!r}"
Issue-Key: codedeploy|CreateDeployment|DeploymentConfigDoesNotExistException
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# REOPENS: ContinueDeployment refuses with InvalidDeploymentWaitTypeException, and the predefined Lambda canary/linear deployment configurations do not exist (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueDeployment

## Description
REOPENS 20261023T062751Z-codedeploy-wrong-batch-alarm-quota-thresholds-two-unimplemented.md: its fix left the call refused.

That issue's Cause 3 (ContinueDeployment answering 501 NotImplemented) and Cause 2 (CreateDeployment's validation) were closed, but both operations still refuse calls the recording resolves differently: ContinueDeployment is now implemented yet reports the wrong modeled error, and CreateDeployment refuses outright for a deployment configuration AWS provides out of the box. Two independent causes.

Cause 1 of 2: ContinueDeployment on a non-blue/green (IN_PLACE) deployment answers InvalidDeploymentWaitTypeException instead of UnsupportedActionForDeploymentTypeException.
AWS: Smithy ContinueDeployment takes deploymentId and the optional deploymentWaitType (enum DeploymentWaitType) and models eight client errors, all HTTP 400: DeploymentAlreadyCompletedException, DeploymentDoesNotExistException, DeploymentIdRequiredException, DeploymentIsNotInReadyStateException, InvalidDeploymentIdException, InvalidDeploymentStatusException, InvalidDeploymentWaitTypeException and UnsupportedActionForDeploymentTypeException. Recorded behavior: for a completed IN_PLACE deployment AWS answers HTTP 400 / UnsupportedActionForDeploymentTypeException — the deployment TYPE is what makes the action unsupported, and that discrimination happens before (and independently of) any deploymentWaitType validation. The error class is the requirement; the recorded message text ('Operation is not supported for the deployment <id> of type IN_PLACE.') is an example.
Emulator: HTTP 400 with error code 'InvalidDeploymentWaitTypeException'.
Why: both codes are modeled, so a client cannot distinguish them by status; one says 'this deployment can never be continued, stop retrying' and the other says 'your deploymentWaitType member is wrong, resend it'. A caller branching on the modeled UnsupportedActionForDeploymentTypeException (botocore exposes it on the client's exception factory) never matches, and blue/green gating logic retries a request that can never succeed.
Fix: ContinueDeployment's validation/precondition ordering in the codedeploy op handler — classify on the deployment's deploymentStyle/type before validating the wait-type member.
Traces: codedeploy.continue-deployment-already-completed-rejected.

Cause 2 of 2: CreateDeployment refuses the AWS-provided predefined deployment configuration CodeDeployDefault.LambdaCanary10Percent5Minutes with DeploymentConfigDoesNotExistException.
AWS: Smithy CreateDeployment declares the optional member deploymentConfigName (DeploymentConfigName) and models DeploymentConfigDoesNotExistException (HTTP 400). AWS ships predefined, account-independent deployment configurations for the Lambda compute platform — CodeDeployDefault.LambdaAllAtOnce, CodeDeployDefault.LambdaCanary10Percent5/10/15/30Minutes and CodeDeployDefault.LambdaLinear10PercentEvery1/2/3/10Minutes — which resolve without ever being created; the recording passes CodeDeployDefault.LambdaCanary10Percent5Minutes and CreateDeployment succeeds with a deploymentId (HTTP 200).
Emulator: HTTP 400 with error code 'DeploymentConfigDoesNotExistException' and message example 'No deployment configuration found for name: CodeDeployDefault.LambdaCanary10Percent5Minutes', so no deployment is created at all. CodeDeployDefault.LambdaAllAtOnce does resolve in the same emulator (other checks deploy with it), so the predefined set is incomplete rather than absent.
Why: every canary and linear Lambda deployment — the whole class of time-based traffic-shift deployments, and the only way to exercise ContinueDeployment, skip-wait-time and bake-window behavior — is unreachable without first creating a private config, which no real client does. The call is refused where AWS accepts it, so client code written against AWS cannot run at all.
Fix: seed the predefined CodeDeployDefault.* deployment configurations (Lambda canary/linear variants, and the Server/ECS equivalents) in the codedeploy service state so GetDeploymentConfig/ListDeploymentConfigs/CreateDeployment resolve them, rather than only the AllAtOnce entry.
Traces: codedeploy.skip-wait-time-triggers-immediate-termination.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout (50/91 passed, bootstrap_signal false — the emulator was serving). Expected = behavior measured on real AWS by the check; Actual = emulator.

Cause 1 — codedeploy.continue-deployment-already-completed-rejected, check line 44:
  assert code == "UnsupportedActionForDeploymentTypeException", f"unexpected error code: {code!r}"
  AssertionError: unexpected error code: 'InvalidDeploymentWaitTypeException'
This is the same assertion recorded as Issue-Key `codedeploy.continue-deployment-already-completed-rejected|run#12|AssertionError|assert code == "UnsupportedActionForDeploymentTypeException"` in the fixed issue 20261023T062751Z-codedeploy-wrong-batch-alarm-quota-thresholds-two-unimplemented.md (its Cause 3, where the observed code was 'NotImplemented'). The operation is now implemented but still refuses with a different modeled code.

Cause 2 — codedeploy.skip-wait-time-triggers-immediate-termination:
  botocore.errorfactory.DeploymentConfigDoesNotExistException: An error occurred (DeploymentConfigDoesNotExistException) when calling the CreateDeployment operation: No deployment configuration found for name: CodeDeployDefault.LambdaCanary10Percent5Minutes
The check never reaches its own assertions — the deployment it needs is refused at creation.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): ContinueDeployment errors = DeploymentAlreadyCompletedException, DeploymentDoesNotExistException, DeploymentIdRequiredException, DeploymentIsNotInReadyStateException, InvalidDeploymentIdException, InvalidDeploymentStatusException, InvalidDeploymentWaitTypeException, UnsupportedActionForDeploymentTypeException (all 400); CreateDeployment models DeploymentConfigDoesNotExistException (400) and declares deploymentConfigName optional. The model documents no message text for these exceptions, so the quoted messages are examples; the requirements are the status and the error code (Cause 1) and the acceptance of the predefined configuration name (Cause 2). Generated deployment ids and names above are environment-specific.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite); both checks reproduce.
2. Cause 1: create an IAM service role, CreateApplication(computePlatform='Server'), CreateDeploymentGroup, CreateDeployment (in-place / IN_PLACE), drive it to a terminal status, then ContinueDeployment(deploymentId=<it>) -> expect HTTP 400 UnsupportedActionForDeploymentTypeException; observe HTTP 400 InvalidDeploymentWaitTypeException.
3. Cause 2: CreateApplication(computePlatform='Lambda'), CreateDeploymentGroup(deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce'), then CreateDeployment(applicationName=app, deploymentGroupName=dg, deploymentConfigName='CodeDeployDefault.LambdaCanary10Percent5Minutes', revision=<AppSpecContent>) -> expect HTTP 200 with a deploymentId; observe HTTP 400 DeploymentConfigDoesNotExistException.
4. Also GetDeploymentConfig(deploymentConfigName='CodeDeployDefault.LambdaCanary10Percent5Minutes') and ListDeploymentConfigs() -> expect the predefined CodeDeployDefault.Lambda{AllAtOnce,Canary*,Linear*} entries to be present.
