Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-not-found-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-not-found-error-paths appconfig-environment-lifecycle-and-monitors appconfig-experiment-run-conflict-and-notfound-errors appconfig-deployment-strategy-crud appconfig-deployment-strategy-update-and-list-lifecycle appconfig-extension-and-association-lifecycle appconfig-deployment-stop-rollback
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateConfigurationProfile|4xx->4xx|$.Message
Issue-Key: appconfig|GetEnvironment|4xx->4xx|$.Message
Issue-Key: appconfig|UpdateEnvironment|4xx->4xx|$.Message
Issue-Key: appconfig|DeleteEnvironment|4xx->4xx|$.Message
Issue-Key: appconfig|GetDeploymentStrategy|4xx->4xx|$.Message
Issue-Key: appconfig|GetExtension|4xx->4xx|$.Message
Issue-Key: appconfig|StartDeployment|4xx->4xx|$.Message
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# AppConfig refusal messages do not use AWS's request-derived templates

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationProfile

## Description
Seven distinct wire causes in the bootstrapped appconfig emulator, all of the SAME fault: the refusal body's Message is an emulator-invented sentence instead of AppConfig's request-derived template. In every one of the seven the HTTP status and the error class already MATCH (404 ResourceNotFoundException, 409 ConflictException) and the only diverging key is $.Message, so each cause is a message-shape defect in one handler's not-found/conflict path. These messages are rule-determined, not incidental recorded text: the same template recurs across independent fixtures with different ids (e.g. the environment form appears with 'algcbw8' in cause 2/3 and with 'zzzzzzz' in cause 4, the application form with 'ho65pb1' and with 'zzzzzzz'), which shows the text is built from the request's identifiers plus the caller's account. The requirement below is therefore the TEMPLATE, never the recorded id values.

Related but NOT covering: the open behavioral issue '20261023T032202Z-appconfig-control-plane-fidelity-settings-route-missing-environmentstate.md' describes the same message fault (its Cause 3) for Application, ConfigurationProfile and HostedConfigurationVersion lookups, and proposes a shared not-found message builder. It carries no wire replay key for any of the steps below, and it does not mention environments, deployment strategies, extensions or the StartDeployment conflict at all. Fixing all seven here should reuse/extend that single shared builder. Numbered; close ALL SEVEN before submitting.

=== Cause 1 of 7: CreateConfigurationProfile's unknown-application refusal ===
AWS: a profile create against an application id that does not exist is ResourceNotFoundException, HTTP 404, with the request-derived text 'Application with Id <ApplicationId> could not be found for account <accountId>.' (Smithy: CreateConfigurationProfile POST /applications/{ApplicationId}/configurationprofiles, code 201, errors BadRequestException 400 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402 / InternalServerException 500; ResourceNotFoundException httpError 404. The template itself is recorded behavior only, confirmed across fixtures with two different ids).
Emulator: status 404 is right, but Message is 'No application with id <id> exists.' — a different sentence shape that names no account.
Why: a client or contract test that parses or asserts the refusal text sees a contract AppConfig never produces, and the account the lookup was scoped to is not reported, so a cross-account mislookup is indistinguishable from a missing resource.
Fix: CreateConfigurationProfile's application-resolution path must emit the shared not-found message builder's output (resource kind + request identifiers + caller account) rather than its own literal.
Traces: appconfig-not-found-error-paths#task#1.

=== Cause 2 of 7: GetEnvironment's unknown-environment refusal ===
AWS: ResourceNotFoundException, HTTP 404, 'Environment with Application Id <ApplicationId> and Environment Id <EnvironmentId> could not be found for account <accountId>.' (Smithy: GetEnvironment GET /applications/{ApplicationId}/environments/{EnvironmentId}, errors include ResourceNotFoundException 404; template is recorded behavior only, seen with two different environment ids).
Emulator: 404 with 'No environment with id <envId> exists in application <appId>.'
Why: same as cause 1 — the text is rule-determined from both path labels and the account, and the emulator's form omits the account entirely.
Fix: the GetEnvironment handler's not-found branch (the hard-coded formats around services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/get_environment.rs) must call the shared builder.
Traces: appconfig-environment-lifecycle-and-monitors#teardown#8.

=== Cause 3 of 7: UpdateEnvironment's unknown-environment refusal ===
AWS: ResourceNotFoundException, HTTP 404, with the SAME environment template as cause 2 ('Environment with Application Id <ApplicationId> and Environment Id <EnvironmentId> could not be found for account <accountId>.') (Smithy: UpdateEnvironment PATCH /applications/{ApplicationId}/environments/{EnvironmentId}, errors include ResourceNotFoundException 404).
Emulator: 404 with 'No environment with id <envId> exists in application <appId>.'
Why: a client that deletes then updates (a common idempotency check) gets a differently shaped refusal than AWS's, and the same wrong literal is duplicated in a second handler — evidence the message is per-handler rather than centralized.
Fix: UpdateEnvironment's lookup path must use the one shared builder, not its own copy of the sentence.
Traces: appconfig-environment-lifecycle-and-monitors#teardown#9.

=== Cause 4 of 7: DeleteEnvironment's unknown-environment refusal ===
AWS: ResourceNotFoundException, HTTP 404, same environment template as causes 2-3 (Smithy: DeleteEnvironment DELETE /applications/{ApplicationId}/environments/{EnvironmentId}, code 204, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500).
Emulator: 404 with 'No environment with id <envId> exists in application <appId>.' Observed here with an id that never existed, which is why it also demonstrates the template is request-derived rather than tied to a deleted record.
Why: idempotent-delete handling that inspects the message cannot classify the refusal the way it does against AWS.
Fix: DeleteEnvironment's lookup path, via the shared builder.
Traces: appconfig-experiment-run-conflict-and-notfound-errors#task#15.

=== Cause 5 of 7: GetDeploymentStrategy's unknown-strategy refusal ===
AWS: ResourceNotFoundException, HTTP 404, 'DeploymentStrategy with Id <DeploymentStrategyId> could not be found.' — note this form carries NO account clause (Smithy: GetDeploymentStrategy GET /deploymentstrategies/{DeploymentStrategyId}, errors include ResourceNotFoundException 404; template is recorded behavior only, asserted by two fixtures).
Emulator: 404 with 'No deployment strategy with id <id> exists.'
Why: the per-resource templates are not uniform in AWS (strategies omit the account, applications and environments include it), so a single invented sentence shape cannot be right for any of them; a client normalizing refusals per resource kind mis-parses.
Fix: GetDeploymentStrategy's not-found branch must emit the strategy-specific template from the shared builder.
Traces: appconfig-deployment-strategy-crud#teardown#6.

=== Cause 6 of 7: GetExtension's unknown-extension refusal ===
AWS: ResourceNotFoundException, HTTP 404, 'Extension could not be found for account <accountId>.' — the identifier is deliberately NOT echoed, only the account (Smithy: GetExtension GET /extensions/{ExtensionIdentifier}, errors include ResourceNotFoundException 404; template is recorded behavior only).
Emulator: 404 with 'No extension matching <id> exists.' — it echoes the identifier AWS withholds and drops the account AWS reports.
Why: beyond the shape difference, the emulator reflects a caller-supplied identifier into the refusal where AWS does not, so behavior built against the emulator's echo has nothing to read in production.
Fix: GetExtension's lookup path must use the extension-specific template (account only, no identifier).
Traces: appconfig-extension-and-association-lifecycle#teardown#13.

=== Cause 7 of 7: StartDeployment's already-deploying conflict message ===
AWS: starting a second deployment while one is in flight is ConflictException, HTTP 409, with text built from the environment and its state: 'Environment <EnvironmentId> currently has a state of <STATE>. The current deployment must finish before a new deployment can be started.' (Smithy: StartDeployment POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, code 201, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500. The sentence is recorded behavior only; the rule — environment id plus the blocking DeploymentState — is visible in the recording).
Emulator: the correct 409 ConflictException, but the message is 'Deployment number <n> is still <STATE> in environment <envId>; only one deployment at a time can be in progress for an environment.' It reports the blocking DEPLOYMENT NUMBER, which AWS's refusal does not mention, and frames the conflict on the deployment rather than on the environment's state.
Why: AWS attributes this conflict to the ENVIRONMENT's state, which is what a retry loop keys on (it then polls the environment back to the ready state); the emulator's text points at a deployment number instead, so message-driven retry logic reads the wrong subject.
Fix: StartDeployment's in-flight-deployment guard must render the environment-state conflict text from the environment id and the environment's current state.
Traces: appconfig-deployment-stop-rollback#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (44 fixtures, 505 steps, 184 passed, score 36.4%, bootstrap_signal=false, state_reset_failures=0). In all seven clusters below the expected and actual HTTP status are EQUAL and $.Message is the only diverging key (the x-amzn-errortype header matched), so these are message-shape divergences, not status or error-class divergences.

Cause 1 — appconfig-not-found-error-paths#task#1, CreateConfigurationProfile, status 404 -> 404: $.Message value_mismatch (expected 'Application with Id zzzzzzz could not be found for account 000000000000.', got 'No application with id zzzzzzz exists.').

Cause 2 — appconfig-environment-lifecycle-and-monitors#teardown#8, GetEnvironment, 404 -> 404: $.Message value_mismatch (expected 'Environment with Application Id ho65pb1 and Environment Id algcbw8 could not be found for account 000000000000.', got 'No environment with id algcbw8 exists in application ho65pb1.').

Cause 3 — appconfig-environment-lifecycle-and-monitors#teardown#9, UpdateEnvironment, 404 -> 404: $.Message value_mismatch, same expected/actual pair as cause 2 (expected 'Environment with Application Id ho65pb1 and Environment Id algcbw8 could not be found for account 000000000000.', got 'No environment with id algcbw8 exists in application ho65pb1.').

Cause 4 — appconfig-experiment-run-conflict-and-notfound-errors#task#15, DeleteEnvironment, 404 -> 404: $.Message value_mismatch (expected 'Environment with Application Id ho65pb1 and Environment Id zzzzzzz could not be found for account 000000000000.', got 'No environment with id zzzzzzz exists in application ho65pb1.') — the same template with a different environment id, confirming it is request-derived.

Cause 5 — appconfig-deployment-strategy-crud#teardown#6, GetDeploymentStrategy, 404 -> 404: $.Message value_mismatch (expected 'DeploymentStrategy with Id ho65pb1 could not be found.', got 'No deployment strategy with id ho65pb1 exists.'). Cluster also covers appconfig-deployment-strategy-update-and-list-lifecycle#task#8.

Cause 6 — appconfig-extension-and-association-lifecycle#teardown#13, GetExtension, 404 -> 404: $.Message value_mismatch (expected 'Extension could not be found for account 000000000000.', got 'No extension matching algcbw8 exists.').

Cause 7 — appconfig-deployment-stop-rollback#task#7, StartDeployment, 409 -> 409: $.Message value_mismatch (expected 'Environment algcbw8 currently has a state of DEPLOYING. The current deployment must finish before a new deployment can be started.', got 'Deployment number 1 is still DEPLOYING in environment algcbw8; only one deployment at a time can be in progress for an environment.').

The ids ('ho65pb1', 'algcbw8', 'zzzzzzz') and the account ('000000000000') are the recording's values and are examples; the requirement is the template each cause states.

## Steps to Reproduce
Boot the emulator and point an appconfig client at its data listener (region/credentials irrelevant). For each cause, read the error's HTTP status, error code and Message: status and code already match, so compare only the Message against the template in the description.

Cause 1: create_configuration_profile(ApplicationId='zzzzzzz' (never created), Name=<any>, LocationUri='hosted') -> 404 ResourceNotFoundException; expect 'Application with Id zzzzzzz could not be found for account <account>.'
Cause 2: create_application -> create_environment -> delete_environment, then get_environment(ApplicationId=<app>, EnvironmentId=<deleted env>) -> 404; expect 'Environment with Application Id <app> and Environment Id <env> could not be found for account <account>.'
Cause 3: same setup, then update_environment(ApplicationId=<app>, EnvironmentId=<deleted env>, Description='x') -> 404; expect the same environment template.
Cause 4: delete_environment(ApplicationId=<real app>, EnvironmentId='zzzzzzz') -> 404; expect the environment template with that id.
Cause 5: create_deployment_strategy -> delete_deployment_strategy, then get_deployment_strategy(DeploymentStrategyId=<deleted id>) -> 404; expect 'DeploymentStrategy with Id <id> could not be found.' (no account clause).
Cause 6: create_extension -> delete_extension, then get_extension(ExtensionIdentifier=<deleted id>) -> 404; expect 'Extension could not be found for account <account>.' with no identifier echoed.
Cause 7: create app/environment/profile/hosted version, start_deployment with a multi-minute strategy (non-instant growth) so the deployment stays in flight, then call start_deployment again for the same environment -> 409 ConflictException; expect 'Environment <env> currently has a state of DEPLOYING. The current deployment must finish before a new deployment can be started.'

Re-replay appconfig-not-found-error-paths#task#1 plus every fixture#step in trace_ids to confirm all seven causes are closed.
