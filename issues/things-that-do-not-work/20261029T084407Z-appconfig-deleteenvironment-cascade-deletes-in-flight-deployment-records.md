Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.in-flight-deployment-does-not-block-leaf-deletion
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StopDeployment|ResourceNotFoundException
Issue-Commit: 503006d3ba8cd79659b55d355ff4fa6b9fbb777f

# AppConfig DeleteEnvironment cascade-deletes in-flight deployment records, so StopDeployment on the orphaned deployment 404s instead of returning it ROLLED_BACK

> _Note: this issue was identified against the code base at commit `503006d3ba8cd79659b55d355ff4fa6b9fbb777f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Cause 1 of 1: a deployment record does not survive the deletion of its environment, so StopDeployment against the orphaned deployment is refused with ResourceNotFoundException instead of succeeding.

Failing check: appconfig.in-flight-deployment-does-not-block-leaf-deletion

AWS: AppConfig models deletion of an environment and the lifetime of a deployment record asymmetrically. DeleteEnvironment (Smithy: DELETE /applications/{ApplicationId}/environments/{EnvironmentId}, success 204, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404) is ACCEPTED while the environment is in the Deploying state and the environment is gone immediately. After that, the read paths for its deployments 404 — GetDeployment (Smithy: GET .../deployments/{DeploymentNumber}, ResourceNotFoundException 404) reports the failure against the DEPLOYMENT, with the message template "Deployment with Application Id <applicationId>, Environment Id <environmentId>, and deploymentNumber <deploymentNumber> could not be found for account <accountId>.", and ListDeployments (Smithy: GET .../deployments, ResourceNotFoundException 404) raises rather than returning an empty page, reporting against the ENVIRONMENT ("Environment with Application Id ..."). But StopDeployment (Smithy: DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, success code 202, output shape Deployment, errors BadRequestException 400 / ResourceNotFoundException 404) STILL SUCCEEDS on that same orphaned deployment: it returns 202 with the full Deployment shape, State = ROLLED_BACK, the same DeploymentNumber that was started, the ConfigurationProfileId it was deploying (even when that profile has itself been deleted), and an EventLog containing an entry whose EventType is ROLLBACK_COMPLETED. The emulator's own earlier legs of this check confirm the recorded AWS behavior up to that point: DeleteEnvironment on a Deploying environment is accepted, GetDeployment then 404s naming the deploymentNumber, ListDeployments then 404s naming the environment.

Emulator: DeleteEnvironment appears to cascade — the deployment record is destroyed along with its environment. The subsequent StopDeployment is refused with HTTP 404 and error code ResourceNotFoundException, message "Deployment with Application Id <applicationId>, Environment Id <environmentId>, and deploymentNumber <deploymentNumber> could not be found for account <accountId>." (in the replay: application id, environment id and deploymentNumber 1 for account 000000000000 — those values are per-run state and only the fixed text "Deployment with Application Id ", ", Environment Id ", ", and deploymentNumber ", " could not be found for account " is the requirement; the ids, number and account are taken from the request and the emulator's state). The check raises an unhandled ResourceNotFoundException at the StopDeployment call, so no ROLLED_BACK state, no surviving ConfigurationProfileId and no ROLLBACK_COMPLETED EventLog entry are ever produced.

Why: a client that tears an environment down while a deployment is in flight and then calls StopDeployment to settle that deployment's record gets a hard 404 from the emulator where AWS returns 202 and a ROLLED_BACK Deployment. Cleanup and teardown code that relies on StopDeployment being idempotent-ish and still addressable after the environment is removed breaks, and nothing can observe the rollback that AWS records in the deployment's EventLog. More broadly, the emulator models environment deletion as a cascade over deployments, which is a representation AWS does not have, so no client can distinguish "deployment record exists but is unreadable through Get/List" from "deployment record is gone".

Fix: the deployment record's lifetime in the appconfig service state, and the StopDeployment handler's lookup. Deleting an environment must remove the environment (and make GetDeployment/ListDeployments for it 404) without removing the deployment records keyed under it; StopDeployment must resolve a deployment by (ApplicationId, EnvironmentId, DeploymentNumber) even when the environment no longer exists, transition it to ROLLED_BACK, append a ROLLBACK_COMPLETED event to its EventLog, and return 202 with the Deployment shape still naming its (possibly deleted) ConfigurationProfileId.

Traces: appconfig.in-flight-deployment-does-not-block-leaf-deletion — the check's later legs; files in services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/ (DeleteEnvironment, StopDeployment) and src/state.rs.

## Evidence
Expected (real AWS, asserted by the check): after DeleteEnvironment on an environment in Deploying, stop_deployment(ApplicationId=app, EnvironmentId=env, DeploymentNumber=n) returns HTTP 202 with State == "ROLLED_BACK", DeploymentNumber == n, ConfigurationProfileId == the (now deleted) profile id, and "ROLLBACK_COMPLETED" present in [e["EventType"] for e in EventLog].

Actual (emulator): botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the StopDeployment operation: Deployment with Application Id ho65pb1, Environment Id algcbw8, and deploymentNumber 1 could not be found for account 000000000000.

HTTP status 404, error code ResourceNotFoundException. Fixed text of that message: "Deployment with Application Id " / ", Environment Id " / ", and deploymentNumber " / " could not be found for account "; variable parts: the application id (ho65pb1), environment id (algcbw8), deployment number (1) and account id (000000000000) — all per-run request/state values, not requirements. The divergence is that this refusal is emitted at all: AWS answers 202 with a Deployment here.

All preceding legs passed, which localises the cause: DeleteEnvironment on a Deploying environment was accepted, GetDeployment for the deployment then correctly returned ResourceNotFoundException naming deploymentNumber, and ListDeployments for the deleted environment correctly raised ResourceNotFoundException naming the environment. Only the StopDeployment survival leg diverges.

## Steps to Reproduce
1. appconfig.CreateApplication, then CreateEnvironment and CreateConfigurationProfile (LocationUri=hosted) under it, plus a second spare profile.
2. CreateDeploymentStrategy with DeploymentDurationInMinutes=15, GrowthFactor=10, GrowthType=LINEAR, FinalBakeTimeInMinutes=10, ReplicateTo=NONE (long enough to stay in flight).
3. CreateHostedConfigurationVersion on the first profile.
4. StartDeployment(ApplicationId, EnvironmentId, DeploymentStrategyId, ConfigurationProfileId, ConfigurationVersion) and poll GetEnvironment until State == "Deploying"; note the returned DeploymentNumber.
5. DeleteHostedConfigurationVersion, then DeleteConfigurationProfile for the profile being deployed (both succeed on AWS mid-deployment).
6. DeleteEnvironment(ApplicationId, EnvironmentId) while the environment is Deploying — accepted.
7. Call StopDeployment(ApplicationId, EnvironmentId, DeploymentNumber from step 4).
   Expected: HTTP 202, State == "ROLLED_BACK", EventLog contains EventType "ROLLBACK_COMPLETED".
   Observed: HTTP 404 ResourceNotFoundException "Deployment with Application Id ..., Environment Id ..., and deploymentNumber ... could not be found for account ...".

Replay the check appconfig.in-flight-deployment-does-not-block-leaf-deletion to confirm.
