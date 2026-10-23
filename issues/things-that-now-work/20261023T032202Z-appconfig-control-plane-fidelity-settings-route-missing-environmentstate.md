Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.account-settings-deletion-protection-blocks-delete appconfig.account-settings-deletion-protection-round-trip appconfig.account-settings-get-default-and-update appconfig.account-settings-negative-deletion-protection-window-rejected appconfig.account-settings-update-and-get-roundtrip appconfig.deletion-protection-control-surface appconfig.update-account-settings-invalid-value-rejected appconfig.environment-initial-state-ready-for-deployment appconfig.environment-update-and-list appconfig.update-environment-state-and-monitors-persist appconfig__appconfigdata.appconfig-hierarchy-round-trips-and-omits-validators appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected appconfig.op-contract-getApplication appconfig.create-configuration-profile-requires-valid-application appconfig.configuration-profile-cross-application-isolation appconfig.op-contract-getHostedConfigurationVersion appconfig.list-experiment-definitions-invalid-application-id-rejected appconfig.hosted-configuration-version-numeric-version-label-rejected appconfig.create-experiment-definition-invalid-treatment-rejected appconfig.create-experiment-definition-invalid-treatment-weights-rejected appconfig.hosted-configuration-version-payload-too-large-rejected appconfig.op-contract-createHostedConfigurationVersion appconfig.configuration-profile-invalid-location-uri-rejected appconfig.feature-flags-profile-enforces-flag-schema
Issue-Key: appconfig|GetAccountSettings|NotImplemented
Issue-Key: appconfig.environment-initial-state-ready-for-deployment|run#6|AssertionError|assert created.get("State") in valid_states, f"unknown environment state: {created.get('State')!r}"
Issue-Key: appconfig.environment-update-and-list|_poll_until#7|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: appconfig.update-environment-state-and-monitors-persist|_poll_until#7|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={result!r})")
Issue-Key: appconfig__appconfigdata.appconfig-hierarchy-round-trips-and-omits-validators|run#13|AssertionError|assert env["State"] == "ReadyForDeployment", (
Issue-Key: appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected|run#23|AssertionError|assert broken["State"] == "ReadyForDeployment", (
Issue-Key: appconfig.op-contract-getApplication|run#14|AssertionError|assert code_msg == (
Issue-Key: appconfig.create-configuration-profile-requires-valid-application|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.configuration-profile-cross-application-isolation|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.op-contract-getHostedConfigurationVersion|run#17|AssertionError|assert code_msg == (
Issue-Key: appconfig.list-experiment-definitions-invalid-application-id-rejected|run#5|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException, got {err['Code']}"
Issue-Key: appconfig.hosted-configuration-version-numeric-version-label-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appconfig.create-experiment-definition-invalid-treatment-rejected|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for 6 treatments (max 5), got {code!r}"
Issue-Key: appconfig.create-experiment-definition-invalid-treatment-weights-rejected|run#6|AssertionError|assert code == "BadRequestException", (
Issue-Key: appconfig.hosted-configuration-version-payload-too-large-rejected|run#9|AssertionError|assert code_msg == "Content size limit exceeded.", f"unexpected message: {code_msg!r}"
Issue-Key: appconfig.op-contract-createHostedConfigurationVersion|run#17|AssertionError|assert message == "Content size limit exceeded.", (
Issue-Key: appconfig.configuration-profile-invalid-location-uri-rejected|_error#9|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: appconfig.feature-flags-profile-enforces-flag-schema|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# AppConfig control-plane fidelity: /settings route missing, EnvironmentState serialized as the enum member name, refusal class and messages diverge, and two inputs AWS refuses are accepted (7 causes)

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSettings

## Description
Seven distinct root causes in the bootstrapped appconfig emulator, each grounded in replayed behavioral checks. Numbered; close ALL SEVEN before submitting.

=== Cause 1 of 7: the account-settings surface is not routed at all ===
AWS: GetAccountSettings is `GET /settings` and UpdateAccountSettings is `PATCH /settings` (Smithy: GetAccountSettings http {method:GET, uri:/settings, code:200}, input Unit, output AccountSettings{DeletionProtection,VendedMetrics}; UpdateAccountSettings http {method:PATCH, uri:/settings, code:200}; both model BadRequestException (400) and InternalServerException (500). DeletionProtectionSettings.ProtectionPeriodInMinutes has @range min 15 max 1440, default interval 60).
Emulator: every call answers HTTP 501 `NotImplemented` with 'operation GET /settings is not implemented in this emulator'. This is a ROUTER gap, not a scaffold stub: the message is the unrouted-path form, and there is no src/ops/get_account_settings.rs in the crate at all (read refused: 'not a file under the checkout'), so no auto-filed stub issue covers it.
Why: a client cannot read or set account-level deletion protection, so the whole deletion-protection control surface (including DeletionProtectionCheck=APPLY/BYPASS enforcement on DeleteConfigurationProfile/DeleteEnvironment) is unobservable and un-exercisable; code that reads settings before a delete gets 501 instead of a 200 settings document.
Fix: add the two operations to the appconfig service's operation set/router and implement them over appconfig state (account-level settings record), then enforce DeletionProtection on the profile/environment delete paths.
Traces: appconfig.account-settings-get-default-and-update, appconfig.account-settings-update-and-get-roundtrip, appconfig.account-settings-deletion-protection-round-trip, appconfig.account-settings-deletion-protection-blocks-delete, appconfig.account-settings-negative-deletion-protection-window-rejected, appconfig.deletion-protection-control-surface, appconfig.update-account-settings-invalid-value-rejected.

=== Cause 2 of 7: EnvironmentState is serialized as the enum MEMBER NAME, not its wire value ===
AWS: recorded behavior only — an environment's State is served in the CamelCase form `ReadyForDeployment` (and `Deploying` / `RollingBack` / `RolledBack` / `Reverted` for the other states); five independent replayed checks, measured against real AWS, assert that form, and their teardown helpers treat ('Deploying','RollingBack') as the transitional set. The model's EnvironmentState enum member names are the SCREAMING_SNAKE spellings.
Emulator: GetEnvironment/CreateEnvironment/UpdateEnvironment/ListEnvironments report State='READY_FOR_DEPLOYMENT'. services/service-com-amazonaws-appconfig-amazonappconfig/src/state.rs defines ENVIRONMENT_READY_FOR_DEPLOYMENT = "READY_FOR_DEPLOYMENT" and ENVIRONMENT_DEPLOYING = "DEPLOYING", i.e. the member names are stored and echoed as the wire value.
Why: every client that compares State to AWS's documented value mismatches: a wait-for-ready loop never terminates (two checks time out after 30s on State='READY_FOR_DEPLOYMENT'), and a teardown that waits out ('Deploying','RollingBack') cannot see an in-flight deployment.
Fix: the state module's EnvironmentState constants and the environment serializer must carry the model's enumValue (CamelCase), not the member name; same review for any other enum whose wire value differs from its member name.
Traces: appconfig.environment-initial-state-ready-for-deployment, appconfig.environment-update-and-list, appconfig.update-environment-state-and-monitors-persist, appconfig__appconfigdata.appconfig-hierarchy-round-trips-and-omits-validators, appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected (the last two are multi-service check ids; the diverging response is appconfig's GetEnvironment).

=== Cause 3 of 7: ResourceNotFoundException messages use an emulator-invented form ===
AWS: the refusal text is built from the request plus the caller's account, in a fixed per-resource form (recorded, asserted by four checks): 'Application with Id <id> could not be found for account <account>.'; 'Configuration Profile with Application Id <appId> and Configuration Profile Id <cpId> could not be found for account <account>.'; 'Hosted Configuration Version with Application Id <appId>, Configuration Profile Id <cpId>, and Version <n> could not be found for account <account>' (no trailing period on this one). Code/status are correct today: ResourceNotFoundException, HTTP 404 (Smithy: ResourceNotFoundException httpError 404, members Message+ResourceName).
Emulator: 'No application with id z9z9z9z9 exists.', 'No configuration profile with id tn20ygr exists in application algcbw8.', 'No hosted configuration version 42 exists in configuration profile algcbw8.' — a different sentence shape that names no account and, for the hosted version, drops the application entirely. See the formats hard-coded in src/ops/get_environment.rs lines 165-183 for the same pattern on environments.
Why: the message is rule-determined (request members + account id), so clients and tooling that parse or assert on it — and operators diffing emulator output against AWS — see a different contract; the missing account id also hides which account the lookup was scoped to.
Fix: a single shared not-found message builder in the appconfig crate, parameterized by resource kind, the request's identifiers and the caller's account, used by every Get*/Create*/Delete* handler.
Traces: appconfig.op-contract-getApplication, appconfig.create-configuration-profile-requires-valid-application, appconfig.configuration-profile-cross-application-isolation, appconfig.op-contract-getHostedConfigurationVersion.

=== Cause 4 of 7: modeled-constraint violations are refused as an unmodeled ValidationException instead of BadRequestException ===
AWS: a constraint violation on an appconfig input is BadRequestException, HTTP 400, with the standard validation-error text, e.g. ListExperimentDefinitions with a 2049-char ApplicationIdentifier -> "1 validation error detected: Value '<value>' at 'applicationIdentifier' failed to satisfy constraint: Member must have length less than or equal to 2048" (Smithy: ListExperimentDefinitions.ApplicationIdentifier target Identifier @length 1..2048; errors BadRequestException 400 / ResourceNotFoundException 404 / InternalServerException 500 — the model declares NO ValidationException anywhere in this service). Same for CreateHostedConfigurationVersion.VersionLabel (@pattern [^0-9], so an all-digit label is refused), CreateExperimentDefinition.Treatments (@length 1..5, so 6 treatments are refused) and TreatmentInput.Weight (@range min 0, so a negative weight is refused).
Emulator: all four are refused with code 'ValidationException' — a class this service does not model — emitted by the framework's pre-handler constraint check.
Why: a client catching the modeled BadRequestException (or switching on it) does not catch ValidationException at all, so an input AWS refuses with a 400 the SDK can classify becomes an unrecognized error class; the request-derived message naming the member and the constraint is also lost.
Fix: map pre-handler constraint failures in this service to the modeled BadRequestException (400) with AWS's 'N validation error detected: Value <v> at '<member>' failed to satisfy constraint: ...' text — i.e. the service's validation/error-mapping layer, not the individual handlers.
Traces: appconfig.list-experiment-definitions-invalid-application-id-rejected, appconfig.hosted-configuration-version-numeric-version-label-rejected, appconfig.create-experiment-definition-invalid-treatment-rejected, appconfig.create-experiment-definition-invalid-treatment-weights-rejected.

=== Cause 5 of 7: the oversize-content refusal message is the emulator's own text, and the limit it enforces is the emulator's ===
AWS: recorded behavior only — an oversize CreateHostedConfigurationVersion is refused with PayloadTooLargeException and the fixed message 'Content size limit exceeded.' (two checks assert that exact constant; the code and HTTP 413 already match — Smithy: PayloadTooLargeException httpError 413 with Message/Measure/Limit/Size).
Emulator: message 'The configuration content is larger than the maximum of 2097152 bytes this emulator stores for a hosted configuration version.' — it both replaces the service-owned text and announces the emulator as the authority. Hypothesis: src/state.rs MAX_HOSTED_CONFIGURATION_BYTES = 2*1024*1024 is the emulator's own bound, while AppConfig's hosted-store limit is 1 MB, so content between 1 MB and 2 MiB is stored here and refused by AWS (not directly observed — the checks used a 5 MiB payload).
Why: a client that surfaces or matches the service message shows emulator-internal prose, and any payload in the 1 MB..2 MiB band is accepted here and rejected in production.
Fix: the CreateHostedConfigurationVersion handler's size check — emit the service-owned message and apply AppConfig's documented hosted-configuration size limit (keep populating Measure/Limit/Size).
Traces: appconfig.op-contract-createHostedConfigurationVersion, appconfig.hosted-configuration-version-payload-too-large-rejected.

=== Cause 6 of 7: CreateConfigurationProfile accepts a LocationUri that requires a RetrievalRoleArn ===
AWS: a LocationUri that is not the hosted store requires RetrievalRoleArn, and the request is refused with BadRequestException (HTTP 400) and the cross-field validation text "1 validation error detected: Value null at 'retrievalRoleArn' failed to satisfy constraint: Member must not be null if the request has the following uri: <uri>" (Smithy: CreateConfigurationProfile.LocationUri required, RetrievalRoleArn optional in the model, so this is a server-side cross-field rule; errors include BadRequestException 400).
Emulator: the call SUCCEEDS with HTTP 201 and persists the profile — observed response {'HTTPStatusCode': 201, 'Id': 'tn20ygr', 'LocationUri': 'ftp://not-a-supported-scheme/config'} with no RetrievalRoleArn. The positive control (LocationUri='hosted') also succeeds, so this is over-permissiveness, not a blanket refusal gap.
Why: a misconfigured profile is created in the emulator and fails only in production, and the resource exists where AWS left nothing — a later list/delete sees state AWS would never have.
Fix: CreateConfigurationProfile's (and UpdateConfigurationProfile's) validation: require RetrievalRoleArn whenever LocationUri is not the hosted store, refusing with the modeled BadRequestException.
Traces: appconfig.configuration-profile-invalid-location-uri-rejected.

=== Cause 7 of 7: feature-flag content is not validated on a AWS.AppConfig.FeatureFlags profile ===
AWS: CreateHostedConfigurationVersion against a configuration profile of Type 'AWS.AppConfig.FeatureFlags' runs the feature-flags helper extension over the content and refuses invalid content with BadRequestException, message 'Error invoking extension AppConfig Feature Flags Helper: Invalid 'Content' data' (recorded; the model carries the profile Type but no content schema).
Emulator: the call SUCCEEDS — observed HTTP 201 with VersionNumber 2 stored (headers application-id/configuration-profile-id/version-number echoed), i.e. invalid flag documents are persisted as a servable version.
Why: a deployment of that version would serve a document AWS would have rejected at write time, so the emulator cannot be used to test feature-flag schema failures and hides a real misconfiguration.
Fix: the CreateHostedConfigurationVersion handler must branch on the owning profile's Type and validate AWS.AppConfig.FeatureFlags content (flags/values/version structure) before storing, refusing with the modeled BadRequestException.
Traces: appconfig.feature-flags-profile-enforces-flag-schema.

## Evidence
Replay report (simparity-behavioral-aws-v1, appconfig 2/220 passed):

Cause 1 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccountSettings operation: operation GET /settings is not implemented in this emulator. Expected: HTTP 200 AccountSettings document; PATCH /settings round-trips DeletionProtection{Enabled, ProtectionPeriodInMinutes}.

Cause 2 — AssertionError: unknown environment state: 'READY_FOR_DEPLOYMENT' (expected 'ReadyForDeployment'); AssertionError: a fresh environment with no deployment is ReadyForDeployment, got 'READY_FOR_DEPLOYMENT'; AssertionError: environment ready state not observed within 30.0s; last saw 'READY_FOR_DEPLOYMENT'; AssertionError: condition not met within 30.0s (last='READY_FOR_DEPLOYMENT'); AssertionError: the environment with the unassumable monitor role must still report 'ReadyForDeployment', got 'READY_FOR_DEPLOYMENT'.

Cause 3 — AssertionError: unexpected message: 'No application with id z9z9z9z9 exists.' (expected "Application with Id z9z9z9z9 could not be found for account <account>."); AssertionError: expected message 'Application with Id bogusba could not be found for account 000000000000.', got 'No application with id bogusba exists.'; AssertionError: expected message 'Configuration Profile with Application Id algcbw8 and Configuration Profile Id tn20ygr could not be found for account 000000000000.', got 'No configuration profile with id tn20ygr exists in application algcbw8.'; AssertionError: unexpected message: 'No hosted configuration version 42 exists in configuration profile algcbw8.' (expected "Hosted Configuration Version with Application Id <app>, Configuration Profile Id <cp>, and Version 42 could not be found for account <account>").

Cause 4 — AssertionError: expected BadRequestException, got ValidationException (2049-char applicationIdentifier on ListExperimentDefinitions; expected message "1 validation error detected: Value '<x*2049>' at 'applicationIdentifier' failed to satisfy constraint: Member must have length less than or equal to 2048"); AssertionError: expected BadRequestException, got ValidationException (all-digit VersionLabel); AssertionError: expected BadRequestException for 6 treatments (max 5), got 'ValidationException'; AssertionError: expected a server-side BadRequestException for a negative treatment weight, got 'ValidationException'.

Cause 5 — AssertionError: expected message 'Content size limit exceeded.', got 'The configuration content is larger than the maximum of 2097152 bytes this emulator stores for a hosted configuration version.' (code PayloadTooLargeException/413 matched).

Cause 6 — AssertionError: expected rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'ApplicationId': 'ho65pb1', 'Id': 'tn20ygr', 'Name': 'sim2real-behav-cp-rejected-...', 'LocationUri': 'ftp://not-a-supported-scheme/config'}. Expected code BadRequestException with "1 validation error detected: Value null at 'retrievalRoleArn' failed to satisfy constraint: Member must not be null if the request has the following uri: ftp://not-a-supported-scheme/config".

Cause 7 — AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...'version-number': '2'...}, 'ApplicationId': 'ho65pb1', 'ConfigurationProfileId': 'algcbw8', 'VersionNumber': 2, 'ContentType': 'application/json', ...}. Expected BadRequestException, message "Error invoking extension AppConfig Feature Flags Helper: Invalid 'Content' data".

## Steps to Reproduce
Boot the emulator and point an appconfig client at it (region/credentials irrelevant).

Cause 1: appconfig.get_account_settings() -> 501 NotImplemented instead of a 200 AccountSettings document; appconfig.update_account_settings(DeletionProtection={'Enabled': True, 'ProtectionPeriodInMinutes': 15}) likewise.
Cause 2: create_application -> create_environment -> get_environment; read State. Emulator 'READY_FOR_DEPLOYMENT', AWS 'ReadyForDeployment'.
Cause 3: get_application(ApplicationId='z9z9z9z9'); create_configuration_profile(ApplicationId='<bogus 7-char>', Name=..., LocationUri='hosted'); get_configuration_profile with a profile id from another application; get_hosted_configuration_version(VersionNumber=<never created>). Compare Error.Message with the AWS forms above.
Cause 4: list_experiment_definitions(ApplicationIdentifier='x'*2049); create_hosted_configuration_version(VersionLabel='12345'); create_experiment_definition(Treatments=[6 entries]); create_experiment_definition(Treatments=[{'Weight': -1, ...}]). Each must be BadRequestException (400).
Cause 5: create_hosted_configuration_version(Content=b'a'*5*1024*1024, ContentType='text/plain') on a hosted profile; read Error.Message.
Cause 6: create_configuration_profile(ApplicationId=<real>, Name=..., LocationUri='ftp://not-a-supported-scheme/config') with no RetrievalRoleArn -> must be refused.
Cause 7: create_configuration_profile(..., LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags') then create_hosted_configuration_version with content that is not a valid feature-flag document -> must be refused.

Replay the checks listed in trace_ids to confirm each cause.
