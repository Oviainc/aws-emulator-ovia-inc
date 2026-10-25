Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.feature-flag-profile-serves-resolved-flag-document
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.feature-flag-profile-serves-resolved-flag-document|_serve_until#17|Timeout|resp = acd.get_latest_configuration(ConfigurationToken=token)
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# AppConfig serves the stored authoring document for an AWS.AppConfig.FeatureFlags profile instead of the resolved flag document

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLatestConfiguration

## Description
One net-new root cause.

Cause 1 of 1: a configuration profile whose Type is AWS.AppConfig.FeatureFlags is served to the data plane as the bytes that were STORED, with no flag resolution at all.

AWS: a profile created with Type=AWS.AppConfig.FeatureFlags (Smithy: CreateConfigurationProfileRequest$Type doc — "When calling this API, enter one of the following values for Type: AWS.AppConfig.FeatureFlags, AWS.Freeform") holds an AUTHORING document with the flags/values/version envelope, and AppConfig's feature-flags helper stamps every flags/values entry with _createdAt/_updatedAt when it stores it. What the data plane serves is a DIFFERENT document: the values half projected to the top level, the flags schema, the version string and all _createdAt/_updatedAt bookkeeping pruned, and a disabled flag reduced to exactly {"enabled": false} with none of its attributes. Served over appconfigdata as HTTP 200 (Smithy appconfigdata: GetLatestConfigurationResponse$Configuration @httpPayload blob, ContentType @httpHeader Content-Type; http 200). Recorded behavior (the check's MEASURED note, us-west-2): authoring {"version":"1","flags":{"alpha":{...attributes.limit...},"beta":{...}},"values":{"alpha":{"enabled":true,"limit":7},"beta":{"enabled":false}}} is stored with the stamps injected and served as exactly {"alpha":{"enabled":true,"limit":7},"beta":{"enabled":false}}; after deploying a second version that flips both flags, the served document follows and the now-disabled flag loses its attributes. The transformation is recomputed per deployed version, not frozen at first serve.

Emulator: the storage half is right (the feature-flags helper validates the document and stamps _createdAt/_updatedAt — the check's assertions on the STORED document pass), but nothing resolves the document on the way out. The deployed hosted version's bytes are handed back verbatim, so a client polling the profile receives the authoring envelope (flags/values/version plus the stamps) rather than the projected flag document. The check therefore never observes its own resolved-document predicate ("alpha" enabled at the top level), polls fresh sessions for its full 240 s budget and is killed at the harness 60 s cap: reported as "Check timed out after 60.0s".

Why: a feature-flag client reads flags off the top level of the served document (flagKey -> {enabled, <attributes>}). Against this emulator every such lookup misses, because the top level carries only "flags", "values" and "version"; worse, the internal authoring schema and the bookkeeping stamps are leaked to every consumer, and a disabled flag still ships the attribute values that resolution is supposed to drop. Any AppConfig Agent / SDK feature-flag integration is unusable, and a test that asserts a flag document is served cannot pass or even terminate.

Fix: the configuration the data plane serves — appconfigdata's served_configuration in services/service-com-amazonaws-appconfigdata-appconfigdata/src/state.rs, which today copies the hosted version's content byte-for-byte, together with the feature-flags module it should delegate to (services/service-com-amazonaws-appconfig-amazonappconfig/src/state/feature_flags.rs, which currently only validates and stamps). When the in-force deployment's configuration profile has Type=AWS.AppConfig.FeatureFlags, resolve the stored authoring document before it is served: project `values` to the top level, prune `flags`, `version` and every _createdAt/_updatedAt, and for a flag whose `enabled` is false emit exactly {"enabled": false}. The stored bytes GetHostedConfigurationVersion returns must not change.

Traces: appconfig.feature-flag-profile-serves-resolved-flag-document.

Related, not the same cause: 20261024T112615Z-appconfigdata-data-plane-deployed-configuration-never-served-sessions.md covers the data plane not serving a deployed configuration at all; serving now works, and this issue is about the TRANSFORMATION the feature-flags profile type owes on the way out, which that issue does not describe.

## Evidence
Behavioral replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator (97 checks, 31 passed; the emulator answered requests, so this is not a bootstrap failure).

appconfig.feature-flag-profile-serves-resolved-flag-document [mismatch]: "Error: Check timed out after 60.0s". The check's assertions on the stored document (set(stored) == {flags, values, version} and "_createdAt" in stored["values"]["alpha"]) are reached first and do not fail, so the version is stored and stamped correctly; the check then deploys and calls _serve_until(..., predicate=lambda b: json.loads(b).get("alpha", {}).get("enabled") is True) on FRESH sessions with a 240 s budget. That predicate is never satisfied, which is why the harness cap is what reports the failure.

Confirmed in the checkout: services/service-com-amazonaws-appconfigdata-appconfigdata/src/state.rs served_configuration() returns ServedConfiguration { content: version.content.clone(), .. } — the stored hosted-version bytes, with no branch on the configuration profile's Type; services/service-com-amazonaws-appconfig-amazonappconfig/src/state/feature_flags.rs exposes only is_valid_feature_flags_content() and stamped_feature_flags_content() (validation + _createdAt/_updatedAt stamping) and no resolution function; FEATURE_FLAGS_PROFILE_TYPE = "AWS.AppConfig.FeatureFlags" is read nowhere on the serve path.

Smithy consulted: appconfig CreateConfigurationProfileRequest$Type (values AWS.AppConfig.FeatureFlags / AWS.Freeform); appconfigdata GetLatestConfiguration (GET /configuration, 200) with Configuration @httpPayload blob and ContentType @httpHeader.

## Steps to Reproduce
1. appconfig: create_application -> create_environment -> create_configuration_profile(LocationUri="hosted", Type="AWS.AppConfig.FeatureFlags") -> create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo="NONE").
2. create_hosted_configuration_version(Content=b'{"version":"1","flags":{"alpha":{"name":"alpha","attributes":{"limit":{"constraints":{"type":"number"}}}},"beta":{"name":"beta"}},"values":{"alpha":{"enabled":true,"limit":7},"beta":{"enabled":false}}}', ContentType="application/json"). get_hosted_configuration_version must still return that authoring document with _createdAt/_updatedAt stamps (it does).
3. start_deployment of that version and poll get_deployment until State == "COMPLETE".
4. appconfigdata: start_configuration_session on (application, environment, profile) and get_latest_configuration(ConfigurationToken=token).
   Expected payload: exactly {"alpha": {"enabled": true, "limit": 7}, "beta": {"enabled": false}} with ContentType application/json.
   Actual: the stored authoring document (keys flags/values/version, stamps present), so no top-level flag key exists.
5. Author a second version flipping the flags (values.alpha = {"enabled": false, "limit": 9}, values.beta = {"enabled": true}), deploy it, and re-poll: expected {"alpha": {"enabled": false}, "beta": {"enabled": true}} — the disabled flag must drop limit.
6. Re-replay appconfig.feature-flag-profile-serves-resolved-flag-document.
