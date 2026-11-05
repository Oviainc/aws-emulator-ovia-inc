Issue-Source: trace-sweep-divergence
Issue-Trace: appintegrations-application-full-config
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appintegrations-application-full-config appintegrations-application-lifecycle appintegrations-data-integration-get-roundtrip
Issue-Shape-Id: com.amazonaws.appintegrations#AmazonAppIntegrationService
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.Publications
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.Subscriptions
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.Tags
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.ApplicationType
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.IframeConfig
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.Permissions
Issue-Key: app-integrations|GetDataIntegration|2xx->2xx|$.IntegrationType
Issue-Key: app-integrations|GetDataIntegration|2xx->2xx|$.Tags
Issue-Commit: e7e2240280b5af8861aa38841849f2941d902f6c

# appintegrations read paths omit server-defaulted members on GetApplication and GetDataIntegration

> _Note: this issue was identified against the code base at commit `e7e2240280b5af8861aa38841849f2941d902f6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
THREE net-new causes, one root behaviour: the AppIntegrations emulator stores every create-input member verbatim and its read paths echo the stored Option straight back, so a member the caller did not send is ABSENT from the response body. AWS fills in a server-side default instead and always serializes the member (empty list, empty map, enum default). There is no open appintegrations issue covering any of this (the only prior appintegrations issues — the bootstrap issue, the BadRequestException/ResourceNotFoundException wording issue and the CreateApplication/CreateDataIntegration stub issues — are all resolved).

Cause 1 of 3 — GetApplication drops Publications, Subscriptions and Tags.
AWS: Smithy GetApplicationResponse.Publications (PublicationList, @length 0..50), .Subscriptions (SubscriptionList, @length 0..50), .Tags (TagMap). Recorded behavior: a GetApplication on an application created without publications, subscriptions or tags answers HTTP 200 with those members PRESENT and empty — Publications: [], Subscriptions: [], Tags: {}. (The model constrains TagMap to @length min 1, yet the recording shows the service emits an empty map for an untagged resource; the recording is the requirement here.)
Emulator: HTTP 200, but the three members are missing from the JSON body entirely.
Why: a client that reads response.Publications/.Subscriptions/.Tags without a null guard sees null/undefined instead of an empty collection — SDKs that model these as collections hand back an absent optional, so iteration code written against AWS throws or silently skips, and 'has no tags' cannot be told apart from 'tags not reported'.
Fix: the GetApplication serializer/state pair — ApplicationRecord must default these to empty collections at create time, or the handler must project None to the empty list/map (crate::state::modeled_tags currently converts an empty map to None on purpose).
Traces: appintegrations-application-full-config#task#2 (expected Publications [], Subscriptions [], Tags {}; got None for all three). Code: services/service-com-amazonaws-appintegrations-amazonappintegrationservice/src/state.rs:168 (modeled_tags returns None for an empty map), src/ops/get_application.rs:198-219, src/ops/create_application.rs:254-274.

Cause 2 of 3 — GetApplication drops ApplicationType, IframeConfig, Permissions, Publications and Subscriptions.
AWS: Smithy GetApplicationResponse.ApplicationType targets the ApplicationType enum (STANDARD | SERVICE | MCP_SERVER | A2A_SERVER); .IframeConfig targets IframeConfig; .Permissions targets PermissionList (@length 0..150). Recorded behavior: an application created without those members reads back with ApplicationType: "STANDARD" (the server-assigned default for an application that is not a service), IframeConfig: {}, Permissions: [], Publications: [], Subscriptions: [] — every member present.
Emulator: HTTP 200 with ApplicationType, IframeConfig, Permissions, Publications and Subscriptions all absent.
Why: ApplicationType is how a client decides how to render/launch an application; absent instead of STANDARD means type-dispatch code on the client takes its unknown/error branch for an ordinary application. IframeConfig: {} vs absent has the same empty-vs-null problem as cause 1.
Fix: CreateApplication state construction must assign the defaults (ApplicationType STANDARD when unset, empty IframeConfig/Permissions/Publications/Subscriptions), and GetApplication must serialize them.
Traces: appintegrations-application-lifecycle#task#2.

Cause 3 of 3 — GetDataIntegration drops IntegrationType and Tags.
AWS: recorded behavior only for IntegrationType — the checked-in Smithy model's GetDataIntegrationResponse declares Arn, Id, Name, Description, KmsKey, SourceURI, ScheduleConfiguration, Tags, FileConfiguration and ObjectConfiguration and no IntegrationType member, so the model is behind the live API. The recorded 200 response for the DataIntegration this fixture creates carries IntegrationType: "IMPORT" (the type the service assigns the integration) and Tags: {} for an untagged integration. Smithy: GetDataIntegrationResponse.Tags (TagMap).
Emulator: HTTP 200 with IntegrationType and Tags both absent.
Why: a client cannot tell what kind of data integration it read back, and again cannot distinguish an untagged integration from an unreported tag set.
Fix: the GetDataIntegration handler/serializer plus DataIntegrationRecord — persist and report the integration type, and report an empty tag map rather than omitting the member.
Traces: appintegrations-data-integration-get-roundtrip#task#4. Code: src/state.rs:287-301 (DataIntegrationRecord), src/state.rs:168.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, 53 steps, 27 passed).

Cause 1 — fixture appintegrations-application-full-config, step task#2, GetApplication, status 200 vs 200:
  $.Publications: missing (expected [], got None)
  $.Subscriptions: missing (expected [], got None)
  $.Tags: missing (expected {}, got None)

Cause 2 — fixture appintegrations-application-lifecycle, step task#2, GetApplication, status 200 vs 200:
  $.ApplicationType: missing (expected 'STANDARD', got None)
  $.IframeConfig: missing (expected {}, got None)
  $.Permissions: missing (expected [], got None)
  $.Publications: missing (expected [], got None)
  $.Subscriptions: missing (expected [], got None)

Cause 3 — fixture appintegrations-data-integration-get-roundtrip, step task#4, GetDataIntegration, status 200 vs 200:
  $.IntegrationType: missing (expected 'IMPORT', got None)
  $.Tags: missing (expected {}, got None)

Model evidence: smithy_query(operation="GetApplication,GetDataIntegration") on services/service-com-amazonaws-appintegrations-amazonappintegrationservice/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json; smithy_query(shape="ApplicationType") -> values STANDARD|SERVICE|MCP_SERVER|A2A_SERVER.

## Steps to Reproduce
1. Replay fixture appintegrations-application-full-config and compare step task#2: CreateApplication with Name/Namespace/ApplicationSourceConfig only, then GetApplication on the returned Arn — the response body has no Publications, Subscriptions or Tags members.
2. Replay fixture appintegrations-application-lifecycle step task#2: CreateApplication without ApplicationType/IframeConfig/Permissions, then GetApplication — no ApplicationType ('STANDARD' expected), IframeConfig, Permissions, Publications or Subscriptions members.
3. Replay fixture appintegrations-data-integration-get-roundtrip step task#4: CreateDataIntegration, then GetDataIntegration on the identifier it returns — no IntegrationType ('IMPORT' expected) and no Tags ({} expected).
4. Re-run replay_wire_corpus() after the fix; all three steps must pass.
