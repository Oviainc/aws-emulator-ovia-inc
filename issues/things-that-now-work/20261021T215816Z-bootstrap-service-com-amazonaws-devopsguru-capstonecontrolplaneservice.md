Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the DevOps Guru (CapstoneControlPlaneService) emulator

_Filed: 2026-10-21T21:58:16Z_
_Operation: service-com-amazonaws-devopsguru-capstonecontrolplaneservice | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in `services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), so the service holds no state at all.

Implement a FIRST SMALL SLICE: the client-driven configuration lifecycle of DevOps Guru, which is the only part of the service a caller can create and observe (insights/anomalies are produced by AWS analysis, not by the client, so they stay stubbed for later gap issues).

Happy path, in order (protocol: restJson1, sigv4 name `devops-guru`):

1. **UpdateResourceCollection** — `PUT /resource-collections` — input `Action` (ADD | REMOVE, required) and `ResourceCollection` (UpdateResourceCollectionFilter, required: `CloudFormation.StackNames` list, `Tags[].{AppBoundaryKey, TagValues}`). Empty output. ADD must merge the given stack names / tag key-values into persisted coverage; REMOVE must subtract them. Must be idempotent for repeated ADD of the same stack name (set semantics).
2. **GetResourceCollection** — `GET /resource-collections/{ResourceCollectionType}` (AWS_CLOUD_FORMATION | AWS_SERVICE | AWS_TAGS, @httpLabel; `NextToken` @httpQuery) — output `ResourceCollection` (ResourceCollectionFilter: `CloudFormation.StackNames`, `Tags[].{AppBoundaryKey, TagValues}`) plus `NextToken`. Must return exactly what step 1 persisted, filtered by the requested collection type (AWS_CLOUD_FORMATION returns stack names, AWS_TAGS returns tag collections) — never a canned body.
3. **AddNotificationChannel** — `PUT /channels` — input `Config` (NotificationChannelConfig, required: `Sns.TopicArn` required, optional `Filters.{Severities,MessageTypes}`) — output `Id`, required, which MUST be a freshly generated lowercase UUID matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` and MUST be stored against the config. Adding the same `Sns.TopicArn` twice is a `ConflictException` (409).
4. **ListNotificationChannels** — `POST /channels` — input `NextToken`; output `Channels` (list<NotificationChannel> = `{Id, Config}`) and `NextToken`. Must list every channel added in step 3 with the same `Id` and the same `Config.Sns.TopicArn`/`Filters` that were submitted; an account with no channels returns an empty/absent list, not an error.
5. **RemoveNotificationChannel** — `DELETE /channels/{Id}` (Id must match the UUID pattern, @httpLabel) — empty output. Must delete the stored channel; a second delete of the same Id, or an unknown Id, must return `ResourceNotFoundException` (404). A subsequent ListNotificationChannels must no longer show it.
6. **DescribeServiceIntegration** / **UpdateServiceIntegration** (optional sixth step if cheap) — `GET`/`PUT /service-integrations`; Update takes `ServiceIntegration` (UpdateServiceIntegrationConfig, required) and Describe must return the persisted `ServiceIntegration` (ServiceIntegrationConfig) reflecting the last update, defaulting to a sane disabled config before any update.

NEVER STUBS: every one of these must compute its response from the persisted store. Hardcoded bodies, echoing the request back without storing it, or generating a channel Id without recording it are failures. Reads must be pure: GetResourceCollection / ListNotificationChannels / DescribeServiceIntegration must not mutate state.

## State Model

In `crate::state` (reached via `crate::state::service_state(&*self.engine)` and its mutex guard), a single per-account/per-region struct:

```text
DevOpsGuruState {
    resource_collection: ResourceCoverage {
        cloudformation_stack_names: BTreeSet<String>,            // from UpdateCloudFormationCollectionFilter.StackNames
        tag_collections: BTreeMap<String /*AppBoundaryKey*/, BTreeSet<String> /*TagValues*/>,
    },
    notification_channels: BTreeMap<String /*Id = uuid v4 lowercase*/, NotificationChannel {
        id: String,
        sns_topic_arn: String,                                   // Config.Sns.TopicArn
        severities: Vec<InsightSeverity>,                        // Config.Filters.Severities
        message_types: Vec<NotificationMessageType>,             // Config.Filters.MessageTypes
    }>,
    service_integration: ServiceIntegrationConfig {              // last UpdateServiceIntegration wins
        ops_center_enabled: bool,
        logs_anomaly_detection_enabled: bool,
        kms_server_side_encryption: Option<KMSServerSideEncryptionIntegration>,
    },
}
```

Pagination tokens for the two list/read ops must be opaque UUID-shaped strings (the model constrains `NextToken` to the 36-char UUID pattern) resolved against this store; with small collections returning no token is acceptable, but `MaxResults` where modeled must be honoured.

Done = a client script can run steps 1→5 end-to-end and observe its own writes: the stack name it added appears in GetResourceCollection, the channel Id it received appears in ListNotificationChannels with the submitted topic ARN, and after RemoveNotificationChannel it is gone and a repeat delete is a 404.

All other operations (DescribeInsight, ListInsights, SearchInsights, ListAnomaliesForInsight, ListEvents, ListRecommendations, PutFeedback, cost-estimation and the organization/* family, …) stay at 501 and are tracked as individual gap issues later.

## Evidence

`services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/OPERATIONS.md` lists all 31 bound operations with routes, input/output members and errors — e.g. AddNotificationChannel `PUT /channels` -> required `Id` (36-char UUID pattern) with ConflictException 409; RemoveNotificationChannel `DELETE /channels/{Id}` with ResourceNotFoundException 404; UpdateResourceCollection `PUT /resource-collections` with required `Action` (ADD|REMOVE) and `ResourceCollection`; GetResourceCollection `GET /resource-collections/{ResourceCollectionType}` returning `ResourceCollection`; ListNotificationChannels `POST /channels` returning `Channels`. Shape members cited (`NotificationChannelConfig.Sns.TopicArn` required, `NotificationChannel{Id,Config}`, `UpdateCloudFormationCollectionFilter.StackNames`, `UpdateTagCollectionFilter{AppBoundaryKey,TagValues}`, `ResourceCollectionFilter{CloudFormation,Tags}`) come from smithy_query on the model at `smithy-model/com-amazonaws-devopsguru-capstonecontrolplaneservice.json`. Proof of all-stubs: `src/ops/add_notification_channel.rs` lines 4 and 179 show the `SCAFFOLDED, NOT IMPLEMENTED` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("AddNotificationChannel"))`; the same pattern is present in every file under `src/ops/`. No tests assert any behaviour for this service.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `devops-guru` client at it.
2. Call UpdateResourceCollection with Action=ADD and ResourceCollection={CloudFormation:{StackNames:["my-stack"]}} — currently HTTP 501, expected 200 with the coverage persisted.
3. Call GetResourceCollection with ResourceCollectionType=AWS_CLOUD_FORMATION — expected 200 with ResourceCollection.CloudFormation.StackNames == ["my-stack"]; currently 501.
4. Call AddNotificationChannel with Config={Sns:{TopicArn:"arn:aws:sns:us-east-1:123456789012:guru-alerts"}} — expected 200 with a freshly generated lowercase-UUID Id; currently 501.
5. Call ListNotificationChannels — expected 200 with one Channel whose Id equals the Id returned above and whose Config.Sns.TopicArn is the submitted ARN; currently 501.
6. Call AddNotificationChannel again with the same TopicArn — expected ConflictException (409).
7. Call RemoveNotificationChannel with that Id — expected 200, then ListNotificationChannels returns no channels, and a repeat RemoveNotificationChannel with the same Id returns ResourceNotFoundException (404); currently 501 for all of these.
8. Call UpdateResourceCollection with Action=REMOVE for "my-stack" and verify GetResourceCollection no longer lists it.
