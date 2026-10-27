Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected
Issue-Key: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected|_error_code#41|AssertionError|raise AssertionError(
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# AppConfig StartDeployment never runs a configuration profile's LAMBDA Validators, so content the validator rejects deploys successfully

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause.

Cause 1 of 1: a configuration profile carrying Validators=[{Type: LAMBDA, Content: <function ARN>}] is deployed without the validator ever being consulted.

AWS: a profile's validators are the deployment-time safety gate (Smithy: Validator{Type @required -> ValidatorType enum [JSON_SCHEMA, LAMBDA], Content @required} — "AppConfig supports validators of type JSON_SCHEMA and LAMBDA"; "a validator provides a syntactic or semantic check to ensure the configuration that you want to deploy functions as intended"). CreateHostedConfigurationVersion is NOT gated by a LAMBDA validator — the non-compliant bytes are stored verbatim — but StartDeployment invokes the named Lambda function with the candidate configuration and, when the function reports the content non-compliant, refuses the deployment with BadRequestException, HTTP 400 (Smithy: StartDeployment errors BadRequestException httpError 400). Recorded behavior: the refusal creates no deployment record (the environment's deployment history is unchanged and the per-environment deployment number is not consumed), the data plane keeps serving the previously deployed version's bytes, and a later version the same validator accepts deploys and is served normally. The relayed message carries the Lambda invocation's runtime request id, so only the status and error code are requirements here.

Emulator: StartDeployment ignores the profile's Validators entirely. The deployment of the version the validator rejects is ACCEPTED (HTTP 201 with a DeploymentNumber) instead of being refused, so the check's _error_code helper raises "the call under test SUCCEEDED where real AWS refuses it — this is an over-permissiveness failure, not an error-code mismatch". The profile's Validators do round-trip on GetConfigurationProfile, so the gate is recorded and then never enforced.

Why: this is AppConfig's only customer-code safety gate. A client that relies on a LAMBDA validator to stop a bad configuration from reaching production sees the bad configuration deploy and be served; a negative test asserting BadRequestException passes against AWS and fails here, and the downstream invariants (no deployment record, no consumed deployment number, previous bytes still served) are never exercised. A deployment pipeline validated only against this emulator ships configuration AWS would have blocked.

Fix: the StartDeployment handler in services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/start_deployment.rs, which today resolves the application/environment/profile/strategy and records the deployment without reading profile.validators. Before any deployment record is written, evaluate each validator of the profile for the candidate configuration version — for Type=LAMBDA, invoke the function named by Content through the Lambda service and treat a non-compliant verdict as a refusal with BadRequestException/400, leaving the deployment counter and the environment's history untouched.

Traces: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected.

Related, not the same cause: 20261025T034405Z-appconfig-lambda-backed-extension-action-points-never-fire.md covers EXTENSION action points (PRE_START_DEPLOYMENT / AT_DEPLOYMENT_TICK / PRE_CREATE_HOSTED_CONFIGURATION_VERSION), a different mechanism from a configuration profile's Validators; 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers lambda Invoke answering NotImplemented, which this fix will depend on.

## Evidence
appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected [mismatch]:
  File "check_appconfig__lambda_validator_lambda_rejects_noncompliant_content_rejected.py", line 399, in run -> code = _error_code(lambda: ac.start_deployment(...ConfigurationVersion=str(bad_version)...))
  AssertionError: the call under test SUCCEEDED where real AWS refuses it — this is an over-permissiveness failure, not an error-code mismatch

The check reached that call only after its barrier succeeded: the profile was created with Validators=[{"Type": "LAMBDA", "Content": fn_arn}], GetConfigurationProfile returned those Validators unchanged, the compliant version 1 deployed to COMPLETE and was served byte-exactly, and the non-compliant version 2 was stored verbatim (asserted, and correct). The expected answer for the deployment of version 2 is BadRequestException / HTTP 400.

Smithy consulted: services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json — Validator (Type @required -> ValidatorType [JSON_SCHEMA, LAMBDA], Content @required, sensitive, len 0..32768), BadRequestException (errorKind client, httpError 400, members Message/Reason/Details), StartDeployment errors {BadRequestException, ConflictException, InternalServerException, ResourceNotFoundException}.

Confirmed in the checkout: services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/start_deployment.rs handle() resolves the application, environment, configuration profile and strategy, checks tags, LatestDeploymentNumber and the environment's active deployment, then records the deployment — it never reads the profile's validators and has no validator-evaluation path.

## Steps to Reproduce
1. lambda: create a function that inspects its event's configuration content and reports compliant/non-compliant, wait for it to be Active, and add_permission(Principal="appconfig.amazonaws.com", Action="lambda:InvokeFunction").
2. appconfig: create_application -> create_environment -> create_configuration_profile(LocationUri="hosted", Validators=[{"Type": "LAMBDA", "Content": <function ARN>}]) -> create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo="NONE").
3. create_hosted_configuration_version with COMPLIANT content (version 1), start_deployment it and poll to COMPLETE; confirm the data plane serves those bytes.
4. create_hosted_configuration_version with NON-COMPLIANT content (version 2) — this must be accepted and stored verbatim.
5. start_deployment(ConfigurationVersion="2"). Expected: BadRequestException, HTTP 400 (message relays the Lambda payload and is only an example). Actual: HTTP 201 with a new DeploymentNumber.
6. Then assert the absent effects AWS guarantees: list_deployments still shows only deployment 1, the data plane still serves version 1's bytes, and a later compliant version 3 deploys with DeploymentNumber == 1 + the first deployment's number.
7. Re-replay appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected.
