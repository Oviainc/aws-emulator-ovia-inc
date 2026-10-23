Issue-Source: trace-sweep-divergence
Issue-Trace: appconfigdata-poll-interval-enforcement
Issue-Step: setup#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfigdata-poll-interval-enforcement appconfigdata-session-and-latest-configuration
Issue-Shape-Id: com.amazonaws.appconfigdata#AppConfigData
Issue-Key: appconfig|CreateConfigurationProfile|2xx->2xx|$.Type
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# AppConfig CreateConfigurationProfile omits the Type member from the created profile document

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationProfile

## Description
Cause 1 of 1: CreateConfigurationProfile does not echo the profile's Type.

AWS: CreateConfigurationProfile (Smithy: POST /applications/{ApplicationId}/configurationprofiles, success code 201, output shape ConfigurationProfile) returns a Type member on the created profile - input Type targets ConfigurationProfileType (@pattern ^[a-zA-Z0-9\\.\\-]+$) and Type is likewise a member of the output. When the request does not set it, AWS defaults the profile to the freeform type and the created document reports Type='AWS.Freeform' (recorded behavior: the fixture's 201 response carries $.Type = 'AWS.Freeform').
Emulator: the 201 status and the rest of the document are right, but $.Type is missing entirely from the response (expected 'AWS.Freeform', got None).
Why: Type is what selects the profile's semantics - a freeform profile versus 'AWS.AppConfig.FeatureFlags' (which triggers feature-flag content validation) versus a vended-type profile. A client that reads Type from the create response to decide how to write or interpret hosted configuration versions gets None, and code that branches on it (or asserts the default) breaks; a later Get/ListConfigurationProfiles that re-serializes the same stored record will be missing it too, so the profile's kind is unobservable.
Fix: the CreateConfigurationProfile handler and the ConfigurationProfile serializer in the appconfig crate - persist the request's Type, defaulting it to the freeform type when absent, and include it in every ConfigurationProfile document (create, get and list).
Traces: appconfigdata-poll-interval-enforcement#setup#3 (cluster also contains appconfigdata-session-and-latest-configuration#setup#3).

No open issue owns this replay key (the replay reports owned_by=[] and stub_path=''). The nearest open issue, 20261023T032202Z-appconfig-control-plane-fidelity-settings-route-missing-environmentstate.md, covers CreateConfigurationProfile's over-permissive LocationUri/RetrievalRoleArn validation and feature-flag content validation, but says nothing about the Type member being omitted from the response, so it does not cover this.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true; 5 fixtures, 36 steps, 12 passed, 22 mismatched, state_reset_failures=0 - this step did not run against a stale fixture's state).

appconfigdata-poll-interval-enforcement#setup#3, service appconfig, operation CreateConfigurationProfile, verdict mismatch, expected 201 / actual 201:
  $.Type: missing (expected 'AWS.Freeform', got None)

Affected cluster members: appconfigdata-poll-interval-enforcement#setup#3, appconfigdata-session-and-latest-configuration#setup#3 (affected_count 2).
Everything else in the response matched, so this is purely an omitted output member and not a broader create failure.

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
1. create_application(Name='<any>') -> note ApplicationId.
2. create_configuration_profile(ApplicationId=<that id>, Name='<any>', LocationUri='hosted') with no Type in the request.
3. Read Type from the 201 response: AWS reports 'AWS.Freeform'; the emulator omits the member. Also check get_configuration_profile / list_configuration_profiles for the same record.
Replay appconfigdata-poll-interval-enforcement#setup#3 to confirm.
