Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.create-experiment-definition-invalid-treatment-percentage-rejected appconfig.feature-flag-attribute-constraints-bound-deployed-values appconfig.hosted-configuration-version-json-schema-validator-rejects-invalid-content
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.create-experiment-definition-invalid-treatment-percentage-rejected|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: appconfig.feature-flag-attribute-constraints-bound-deployed-values|_error#21|AssertionError|raise AssertionError(
Issue-Key: appconfig.hosted-configuration-version-json-schema-validator-rejects-invalid-content|_error#24|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# AppConfig accepts three classes of invalid configuration/experiment input that AWS refuses (3 causes)

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedConfigurationVersion

## Description
Three distinct over-permissiveness root causes in the bootstrapped appconfig emulator: a call real AWS refuses with 400 BadRequestException is accepted with a 2xx, so the invalid state lands in the service. None is covered by an auto-filed stub issue (all three operations are implemented and answered successfully) and none is an unimplemented-operation failure. Numbered; close ALL THREE before submitting.

=== Cause 1 of 3: CreateExperimentDefinition does not require a feature-flags configuration profile ===
AWS: experiment definitions target feature flags, so the referenced ConfigurationProfileIdentifier must resolve to a profile of Type 'AWS.AppConfig.FeatureFlags'. Pointing CreateExperimentDefinition at an 'AWS.Freeform' profile is refused with BadRequestException, HTTP 400, and no definition is created (it does not appear in ListExperimentDefinitions). Smithy: CreateExperimentDefinition POST /applications/{ApplicationIdentifier}/experimentdefinitions, success code 201, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402 / InternalServerException 500; the model constrains Name, FlagKey, Treatments (1-5) and Control but says nothing about the profile type, so the profile-type precondition is recorded behavior only (the recording shows the 400 plus the message 'Configuration profile <id> cannot be used for experiments. Experiment definitions require a configuration profile of type AWS.AppConfig.FeatureFlags.').
Emulator: the same call returns HTTP 201 with a full ExperimentDefinition document (Status IDLE, the Freeform profile id echoed in ConfigurationProfileId) instead of refusing.
Why: a client that relies on the server to reject a mis-wired experiment gets an experiment definition bound to a profile that has no flags at all; the failure surfaces much later (at run/serve time, or never in the emulator), and negative tests asserting the 400 pass against AWS and fail against the emulator.
Fix: CreateExperimentDefinition's validation path — resolve the configuration profile and refuse with BadRequestException 400 when its Type is not AWS.AppConfig.FeatureFlags, before any state is written.
Traces: appconfig.create-experiment-definition-invalid-treatment-percentage-rejected.

=== Cause 2 of 3: CreateHostedConfigurationVersion does not enforce a feature flag's declared attribute constraints ===
AWS: for a profile of Type AWS.AppConfig.FeatureFlags, AppConfig validates the authored flag VALUES against the constraints declared in the same document's flag attributes (a number attribute's minimum/maximum, a string attribute's enum). A document whose value violates its own declared constraint is refused with BadRequestException, HTTP 400, no version number is consumed (the next accepted create still gets the next sequential VersionNumber), and the same value under a relaxed constraint is accepted. Smithy: CreateHostedConfigurationVersion POST /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}/hostedconfigurationversions, success code 201, errors BadRequestException 400 / ConflictException 409 / PayloadTooLargeException 413 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402; the constraint enforcement itself is recorded behavior only (the document is valid against the published AWS.AppConfig.FeatureFlags envelope schema, so running only that schema is not sufficient).
Emulator: the out-of-range document is accepted (the create SUCCEEDS instead of raising) — the emulator stores a flag value outside the constraint it was authored with.
Why: feature-flag constraints exist to gate AUTHORING; an emulator that ignores them lets a value that real AWS rejects be stored and then deployed, so a client's constraint-driven guardrails are silently absent and a deployment that would be impossible in production is reachable in the emulator.
Fix: CreateHostedConfigurationVersion's feature-flags validation path — after the envelope schema check, evaluate each flag value against that flag's declared attribute constraints (numeric range, string enum) and refuse with BadRequestException 400 without allocating a VersionNumber.
Traces: appconfig.feature-flag-attribute-constraints-bound-deployed-values.

=== Cause 3 of 3: StartDeployment does not run the configuration profile's JSON_SCHEMA validator ===
AWS: when a configuration profile carries a Validators entry of Type JSON_SCHEMA, StartDeployment validates the referenced hosted configuration version against that schema and refuses a violating version with BadRequestException, HTTP 400; no deployment is recorded (ListDeployments still holds only the earlier deployments) and the environment stays in its ready state. A schema-conformant version on the same profile deploys to COMPLETE. Smithy: StartDeployment POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success code 201, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500; the validator evaluation at deployment time is recorded behavior only (the recording's message is of the form 'Could not build JSON schema from invalid input: ... Original error message: [message: "instance type (string) does not match any allowed primitive type (allowed: [\"integer\"])" - parent node: /limit]', an example, not a requirement).
Emulator: StartDeployment of the schema-violating version returns HTTP 201 with State 'COMPLETE', PercentageComplete 100 and an EventLog of DEPLOYMENT_STARTED/DEPLOYMENT_COMPLETED — the attached JSON_SCHEMA validator is never evaluated (the profile does round-trip Validators correctly via GetConfigurationProfile, so only the deployment-time evaluation is missing).
Why: validators are the only server-side gate between a stored version and clients; ignoring them means content real AWS would never ship is served to the data plane, and a client's 'bad config cannot deploy' invariant does not hold.
Fix: StartDeployment's pre-deployment validation step — load the profile's Validators and evaluate JSON_SCHEMA ones against the version's content, refusing with BadRequestException 400 and recording no deployment.
Traces: appconfig.hosted-configuration-version-json-schema-validator-rejects-invalid-content.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1) against the booted emulator, 24/97 passed, bootstrap_signal=false.

Cause 1 — appconfig.create-experiment-definition-invalid-treatment-percentage-rejected [mismatch]: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Id': '5w5gjbo', 'Status': 'IDLE', 'ConfigurationProfileId': '<freeform profile id>', 'FlagKey': 'pctflag', 'Treatments': [{'Key': 'T1', 'Weight': 50.0, ...}], 'Control': {...}}. Expected: BadRequestException, HTTP 400. The ids and timestamps in that body are the run's values and are examples.

Cause 2 — appconfig.feature-flag-attribute-constraints-bound-deployed-values [mismatch]: AssertionError raised by the check's _error helper — 'the call under test SUCCEEDED where real AWS refuses it — this is an over-permissiveness failure, not an error-code mismatch' — on create_hosted_configuration_version of a feature-flag document whose 'percent' value exceeds the maximum declared in the same document's attribute constraints (the control document, identical except for that value, was accepted as VersionNumber 1). Expected: BadRequestException, HTTP 400, no version consumed.

Cause 3 — appconfig.hosted-configuration-version-json-schema-validator-rejects-invalid-content [mismatch]: AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'DeploymentNumber': 2, 'ConfigurationVersion': '2', 'State': 'COMPLETE', 'PercentageComplete': 100.0, 'EventLog': [{'EventType': 'DEPLOYMENT_COMPLETED', ...}, {'EventType': 'DEPLOYMENT_STARTED', ...}]} for StartDeployment of content b'{"limit":"not-an-integer"}' on a profile whose single JSON_SCHEMA validator requires {"limit": integer}. The positive control (b'{"limit":1}') deployed to COMPLETE on the same profile, so the path works and only validation is missing. Expected: BadRequestException, HTTP 400, no deployment recorded.

## Steps to Reproduce
Point an appconfig client at the emulator's data listener (region/credentials irrelevant).

Cause 1: create_application; create_environment; create_configuration_profile(LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags') for the flag 'pctflag'; create_configuration_profile(LocationUri='hosted', Type='AWS.Freeform'); then create_experiment_definition(ApplicationIdentifier=<app>, Name=<new>, ConfigurationProfileIdentifier=<the FREEFORM profile>, EnvironmentIdentifier=<env>, FlagKey='pctflag', AudienceRule='(eq 1 1)', Control={Weight:50,...}, Treatments=[{Weight:50,...}]). Emulator: 201. AWS: 400 BadRequestException and the name absent from list_experiment_definitions.

Cause 2: create a profile with Type='AWS.AppConfig.FeatureFlags'; create_hosted_configuration_version with a flag document declaring a number attribute with minimum/maximum and a value inside the range (accepted, VersionNumber 1); then re-send the identical document with ONLY that value moved above the declared maximum. Emulator: 201 (and a version number is consumed). AWS: 400 BadRequestException and no version consumed. Repeat with a string attribute value outside its declared enum.

Cause 3: create_configuration_profile(LocationUri='hosted', Validators=[{Type:'JSON_SCHEMA', Content:<schema requiring limit:integer>}]); create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE'); create_hosted_configuration_version(Content=b'{"limit":1}') and start_deployment on it (completes); wait for the environment to be ready; create_hosted_configuration_version(Content=b'{"limit":"not-an-integer"}') and start_deployment on that version. Emulator: 201 State=COMPLETE. AWS: 400 BadRequestException, no new deployment in list_deployments, environment still ReadyForDeployment.

Re-replay the three checks in trace_ids to confirm.
