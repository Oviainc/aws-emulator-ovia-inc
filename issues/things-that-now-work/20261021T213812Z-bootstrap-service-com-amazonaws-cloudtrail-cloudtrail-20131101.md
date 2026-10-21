Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CloudTrail emulator

_Filed: 2026-10-21T21:38:12Z_
_Operation: service-com-amazonaws-cloudtrail-cloudtrail-20131101 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state (60 operations). Implement a FIRST, SMALL stateful slice: the trail lifecycle, the service's primary resource. Protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: CloudTrail_20131101.<OperationName>` (see README/OPERATIONS.md).

Happy path to implement, in order:

1. **CreateTrail** — input `Name` (required), `S3BucketName` (required), `S3KeyPrefix`, `SnsTopicName`, `IncludeGlobalServiceEvents`, `IsMultiRegionTrail`, `EnableLogFileValidation`, `CloudWatchLogsLogGroupArn`, `CloudWatchLogsRoleArn`, `KmsKeyId`, `IsOrganizationTrail`, `TagsList`. Store a new trail keyed by name; mint `TrailARN` = `arn:aws:cloudtrail:<region>:<account>:trail/<Name>` and, when `SnsTopicName` is given, `SnsTopicARN`. Echo back the stored values (note `EnableLogFileValidation` input maps to the `LogFileValidationEnabled` output member). A second CreateTrail with the same `Name` must fail with `TrailAlreadyExistsException` (HTTP 400), not overwrite.
2. **GetTrail** — input `Name` (required, accept both bare name and trail ARN); output `Trail` built from the persisted record. Unknown name ⇒ `TrailNotFoundException` (HTTP 404).
3. **StartLogging** / **StopLogging** — input `Name` (required), empty output. Flip the persisted `is_logging` flag and record `StartLoggingTime` / `StopLoggingTime`. Unknown name ⇒ `TrailNotFoundException` (404).
4. **GetTrailStatus** — input `Name` (required); output `IsLogging`, `StartLoggingTime`, `StopLoggingTime`, `TimeLoggingStarted`, `TimeLoggingStopped` all derived from the persisted trail. A freshly created trail must report `IsLogging: false`; after StartLogging it must report `true`, and after StopLogging `false` again.
5. **DescribeTrails** — input `trailNameList` (list of names/ARNs, empty or absent = all trails), `includeShadowTrails`; output `trailList` of `Trail` structures assembled from persisted state. Filtering by `trailNameList` must actually filter.
6. **DeleteTrail** — input `Name` (required), empty output. Remove the trail from state; a following GetTrail/GetTrailStatus must return `TrailNotFoundException` (404) and DescribeTrails must no longer list it. Deleting an unknown trail ⇒ `TrailNotFoundException` (404).

Optionally round the slice out with **ListTrails** (output `Trails` of `TrailInfo` {TrailARN, Name, HomeRegion}, `NextToken`) since it reads the same map.

NEVER STUBS: each of these must compute its response from the shared engine store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input without persisting, no minted ARN that is never stored. A create followed by a describe must return exactly what was created, across requests.

## State Model

In `src/state.rs` (`ComAmazonawsCloudtrailCloudtrail20131101State`):

- `trails: BTreeMap<String /* trail name */, Trail>` — insertion/sorted order used for DescribeTrails/ListTrails output.
- `Trail { name: String, trail_arn: String, s3_bucket_name: String, s3_key_prefix: Option<String>, sns_topic_name: Option<String>, sns_topic_arn: Option<String>, include_global_service_events: bool, is_multi_region_trail: bool, log_file_validation_enabled: bool, cloud_watch_logs_log_group_arn: Option<String>, cloud_watch_logs_role_arn: Option<String>, kms_key_id: Option<String>, is_organization_trail: bool, home_region: String, has_custom_event_selectors: bool, has_insight_selectors: bool, tags: Vec<Tag>, status: TrailStatus }`
- `TrailStatus { is_logging: bool, start_logging_time: Option<SystemTime>, stop_logging_time: Option<SystemTime> }`
- Helper: resolve a caller-supplied identifier (bare name or `arn:aws:cloudtrail:...:trail/<name>`) to a map key, so every op above accepts both forms.

Done = a client script can CreateTrail → GetTrail → StartLogging → GetTrailStatus (IsLogging true) → DescribeTrails → StopLogging → DeleteTrail → GetTrail (404 TrailNotFoundException) with correct, state-derived bodies at every step. All other CloudTrail operations (event data stores, channels, dashboards, Lake queries, imports, insights, tags, resource policies, LookupEvents) stay 501 and are filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate: CreateTrail (line 60) input/output members and `TrailAlreadyExistsException (HTTP 400)`; DescribeTrails (line 114) `trailNameList`/`includeShadowTrails` → `trailList`; GetTrail (line 202) → `Trail`, `TrailNotFoundException (HTTP 404)`; GetTrailStatus (line 209) → `IsLogging`, `StartLoggingTime`, `StopLoggingTime`; StartLogging (line 374) and StopLogging; DeleteTrail (line 93) with `TrailNotFoundException (HTTP 404)`; ListTrails (line 286) → `Trails` (TrailInfo) + NextToken. Wire protocol awsJson1_1 with `X-Amz-Target: CloudTrail_20131101.<Op>` per OPERATIONS.md lines 10-11. All 60 handler files under src/ops/ carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (verified in src/ops/create_trail.rs line 4) and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501, so no trail state exists anywhere today.

## Steps to Reproduce

1. POST / with X-Amz-Target: CloudTrail_20131101.CreateTrail and body {"Name":"demo-trail","S3BucketName":"my-bucket","IsMultiRegionTrail":true,"EnableLogFileValidation":true} — expect 200 with Name, S3BucketName, TrailARN, IsMultiRegionTrail:true, LogFileValidationEnabled:true; currently 501 NotImplemented
2. POST CloudTrail_20131101.GetTrail {"Name":"demo-trail"} — expect 200 with Trail reflecting the created configuration
3. POST CloudTrail_20131101.GetTrailStatus {"Name":"demo-trail"} — expect IsLogging:false
4. POST CloudTrail_20131101.StartLogging {"Name":"demo-trail"} — expect 200 empty body; then GetTrailStatus must report IsLogging:true with StartLoggingTime set
5. POST CloudTrail_20131101.DescribeTrails {} — expect trailList containing demo-trail; repeat with {"trailNameList":["other"]} and expect it filtered out
6. POST CloudTrail_20131101.StopLogging {"Name":"demo-trail"} then GetTrailStatus — expect IsLogging:false with StopLoggingTime set
7. POST CloudTrail_20131101.CreateTrail again with Name demo-trail — expect 400 TrailAlreadyExistsException
8. POST CloudTrail_20131101.DeleteTrail {"Name":"demo-trail"} — expect 200; then GetTrail and GetTrailStatus must return 404 TrailNotFoundException and DescribeTrails must return an empty trailList
