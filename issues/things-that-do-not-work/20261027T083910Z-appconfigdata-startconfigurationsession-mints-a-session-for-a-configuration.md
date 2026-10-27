Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfigdata.appconfigdata-empty-configuration-profile-first-poll-returns-empty
Issue-Shape-Id: com.amazonaws.appconfigdata#AppConfigData
Issue-Key: appconfigdata.appconfigdata-empty-configuration-profile-first-poll-returns-empty|_client_error#14|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Commit: d566282976cd3ecbd638dcfb7eabea66136f946a

# AppConfigData StartConfigurationSession mints a session for a configuration profile that has never been deployed instead of refusing ResourceNotFoundException 'Deployment not found'

> _Note: this issue was identified against the code base at commit `d566282976cd3ecbd638dcfb7eabea66136f946a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartConfigurationSession

## Description
One net-new root cause in the bootstrapped appconfigdata data plane, grounded in a replayed behavioral check.

Cause 1 of 1: StartConfigurationSession succeeds (201 + InitialConfigurationToken) for an application/environment/configuration-profile triple that exists but has NO deployment, where AWS refuses the session with ResourceNotFoundException / 'Deployment not found'.

AWS: StartConfigurationSession (Smithy sdkId AppConfigData, restJson1, POST /configurationsessions, success 201) resolves ApplicationIdentifier, EnvironmentIdentifier and ConfigurationProfileIdentifier (all @required, Identifier @length 1..128, each accepting an id OR a name) against appconfig state AND requires that the configuration profile have been deployed to that environment. There is no 'empty first poll' for a never-deployed profile: the session itself is refused with the modeled ResourceNotFoundException (httpError 404; members Message, ResourceType -> ResourceType enum {Application, ConfigurationProfile, Deployment, Environment, Configuration}, ReferencedBy -> StringMap). For this class the resource that is missing is the deployment, so ResourceType is 'Deployment' and the Message is the fixed text "Deployment not found" — that message has NO variable part; it is the whole message, and nothing from the request is interpolated into it (ReferencedBy, not the message, carries the request members that referenced the missing resource). Once a hosted configuration version IS deployed to that environment and the deployment reaches COMPLETE, the identical call returns 201 with a token and GetLatestConfiguration serves the deployed bytes.

Emulator: the call is ACCEPTED. The replayed check sent StartConfigurationSession for a profile with no deployment of any kind (verified in the same check via appconfig ListDeployments, which recorded no deployment for that profile) and the emulator answered HTTP 201 with a body of the form {"InitialConfigurationToken": "<64-hex>"} (one recorded run: '4acf1f73...'; the token value is per-run and is not part of the requirement). No 404, no ResourceNotFoundException, no ResourceType='Deployment', no 'Deployment not found' message. A deployed sibling profile in the same application/environment did open a session in the same run, so session minting is not simply unconditional-failing — it is unconditional-succeeding: the handler never consults deployment state when deciding whether a session may be opened.

Why: a client relying on AWS's behavior uses the refusal at session start as the signal that there is nothing deployed yet (bootstrapping clients commonly retry StartConfigurationSession until the first deployment lands, and treat ResourceNotFoundException as 'not deployed yet' rather than as a fatal error). Against the emulator it instead receives a usable-looking token for a profile that has no configuration at all, so the 'not deployed yet' branch is never taken, the retry loop exits immediately, and the failure surfaces later and less legibly at poll time (or is masked entirely). Negative tests asserting the refusal pass against AWS and fail here.

Note on overlap: open issue 20261024T112615Z-appconfigdata-data-plane-deployed-configuration-never-served-sessions.md lists this check id in its trace_ids, but it binds the check only to its Cause 5 (appconfig ListDeployments was an unimplemented scaffold stub, which blocked the check before it reached this call) and its Cause 2 covers only identifiers that do not RESOLVE (ResourceType Application/Environment/ConfigurationProfile). ListDeployments now answers and the check proceeds past it, exposing this distinct cause: the identifiers all resolve, it is the DEPLOYMENT that is missing, and the refusal AWS gives is ResourceType 'Deployment' with the message 'Deployment not found'. Neither of those causes describes or gates it.

Fix: the StartConfigurationSession handler and the appconfig deployment state it must read — after resolving the three identifiers, look up the latest deployment for that (application, environment, configuration profile) triple and, when none exists, refuse with the modeled ResourceNotFoundException at HTTP 404 with Message 'Deployment not found' and ResourceType 'Deployment' (populating ReferencedBy with the request members that named the triple) and mint no token; keep minting the session once such a deployment exists, so the deployed-sibling path and the post-restore path continue to return 201.

## Evidence
Behavioral replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator (13 checks, 8 passed, 3 mismatched, 2 errored, 0 skipped; the emulator answered every request, so this is not a bootstrap failure). The other four appconfigdata failures in this run are marked OWNED by open issues and are skipped; this check is not owned by any open issue's replay key.

[mismatch] appconfigdata.appconfigdata-empty-configuration-profile-first-poll-returns-empty:
  AssertionError: the call was expected to be refused but succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '96', ...}, 'RetryAttempts': 0}, 'InitialConfigurationToken': '4acf1f737fe75a8431d08dc68d723e7bad42c5a29273ce2004e02813e79b4b2b'}
  raised at check line 41 from the helper _client_error, i.e. the assertion never got as far as comparing a code or a message because no error was raised at all.

Expected by the check (and by AWS):
  code    == 'ResourceNotFoundException'   (HTTP 404)
  message == 'Deployment not found'        (fixed text, no interpolated values)
Observed: HTTP 201, no error, body {'InitialConfigurationToken': <64-hex, per-run>}.

Positive controls inside the same check that DID behave: a sibling configuration profile in the same application/environment that HAD been deployed returned a session token ('positive control' assertion passed), and appconfig ListDeployments returned no deployment for the profile under test (so the precondition 'never deployed' was established from the emulator's own state, not assumed).

Smithy consulted: services/service-com-amazonaws-appconfigdata-appconfigdata/smithy-model/com-amazonaws-appconfigdata-appconfigdata.json — StartConfigurationSessionRequest (ApplicationIdentifier / EnvironmentIdentifier / ConfigurationProfileIdentifier all required, Identifier @length 1..128, 'application ID or the application name'), ResourceNotFoundException (httpError 404; Message, ResourceType, ReferencedBy StringMap 'indicating which parameters in the request reference the resource that was not found'), ResourceType enum including the member 'Deployment'. The pairing of ResourceType='Deployment' with the fixed message text 'Deployment not found' is recorded behavior; the model supplies the error, the status and the ResourceType vocabulary.

## Steps to Reproduce
Boot the emulator and point an appconfig client and an appconfigdata client at its data listener (region and credentials are irrelevant here).

1. appconfig.create_application -> app; create_environment(app) -> env; create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE') -> strategy.
2. create_configuration_profile(app, LocationUri='hosted') TWICE: 'control' and 'target'.
3. Deploy only the control profile: create_hosted_configuration_version(app, control, Content=b'{"control":true}') -> start_deployment -> poll get_deployment until State == 'COMPLETE'.
4. Positive control: appconfigdata.start_configuration_session(ApplicationIdentifier=app, EnvironmentIdentifier=env, ConfigurationProfileIdentifier=control) must return 201 with an InitialConfigurationToken (it does).
5. Confirm the target profile has no deployment: appconfig.list_deployments(ApplicationId=app, EnvironmentId=env) must contain no item whose ConfigurationProfileId is the target profile.
6. THE DIVERGENCE: appconfigdata.start_configuration_session(ApplicationIdentifier=app, EnvironmentIdentifier=env, ConfigurationProfileIdentifier=target). Expected: ClientError with Code 'ResourceNotFoundException', HTTP status 404, Message exactly 'Deployment not found', ResourceType 'Deployment', and NO InitialConfigurationToken. Observed: HTTP 201 and a token.
7. Regression guard for the fix: now create_hosted_configuration_version(app, target, Content=b'{"restored":true}') -> start_deployment -> wait for COMPLETE, then start_configuration_session on the target must again return 201 and get_latest_configuration with that token must return the deployed bytes — the refusal must apply only while no deployment exists.

Re-replay appconfigdata.appconfigdata-empty-configuration-profile-first-poll-returns-empty to confirm.
