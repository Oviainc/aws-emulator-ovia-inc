Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-detector-basic-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-detector-basic-lifecycle guardduty-detector-duplicate-create-error guardduty-detector-features-and-tags guardduty-detector-lifecycle-basic
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.cloudTrail.updatedAt
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.database
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.dnsLogs.updatedAt
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.flowLogs.updatedAt
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.kubernetes.auditLogs.updatedAt
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.malwareProtection.scanEc2InstanceWithFindings.ebsVolumes.updatedAt
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.s3Logs.updatedAt
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.malwareProtection.serviceRole
Issue-Key: guardduty|GetDetector|2xx->2xx|$.features[*]
Issue-Key: guardduty|GetDetector|2xx->2xx|$.tags
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.cloudTrail.status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.dnsLogs.status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.flowLogs.status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.kubernetes.auditLogs.status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.malwareProtection.scanEc2InstanceWithFindings.ebsVolumes.status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.s3Logs.status
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GetDetector projection drops data-source timestamps, database block and tags, and ignores applied updates

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDetector

## Description
GetDetector answers HTTP 200 but its response projection is incomplete and partly stale. Four distinct causes, all in the GetDetector response projection / detector state. Smithy (services/service-com-amazonaws-guardduty-guarddutyapiservice): GetDetector -> GetDetectorResponse {createdAt, findingPublishingFrequency, serviceRole (required), status (required), updatedAt, dataSources: DataSourceConfigurationsResult, tags: TagMap, features: DetectorFeatureConfigurationsResults}; DataSourceConfigurationsResult requires cloudTrail/dnsLogs/flowLogs/s3Logs and carries kubernetes + malwareProtection; MalwareProtectionConfigurationResult.serviceRole is "The GuardDuty Malware Protection service role"; DetectorFeatureConfigurationResult {name: DetectorFeatureResult enum, status, updatedAt, additionalConfiguration}; DetectorFeatureResult values include CLOUD_TRAIL, DNS_LOGS, FLOW_LOGS, S3_DATA_EVENTS.

Cause 1 of 4: dataSources nodes carry no updatedAt and the database block is absent. AWS: recorded behavior only — the public model's CloudTrailConfigurationResult / S3LogsConfigurationResult / KubernetesAuditLogsConfigurationResult / ScanEc2InstanceWithFindingsResult.ebsVolumes model only `status`, and DataSourceConfigurationsResult models no `database` member, but the recording shows the live wire response carrying an `updatedAt` on every data-source node and a `dataSources.database.rds.databaseEvents {status, updatedAt}` block. Emulator: every `$.dataSources.*.updatedAt` is missing and `$.dataSources.database` is absent entirely. Why: a client that renders or diffs per-data-source enablement timestamps, or that reads RDS login-event (database) enablement off GetDetector, sees nulls and concludes the data source does not exist. Fix: the GetDetector response projection (serializer) plus the detector state that must record a per-data-source update timestamp and an rds/databaseEvents entry. Traces: guardduty-detector-basic-lifecycle#task#3 (also #task#5).

Cause 2 of 4: malwareProtection.serviceRole is the wrong service-linked role, the features list is wrong, and tags is omitted. AWS: malwareProtection.serviceRole is the Malware Protection service-linked role (recorded form: arn:aws:iam::<account>:role/aws-service-role/malware-protection.guardduty.amazonaws.com/AWSServiceRoleForAmazonGuardDutyMalwareProtection), distinct from the detector-level GuardDuty role; features are returned in the service's canonical order starting at CLOUD_TRAIL, each entry carrying an `additionalConfiguration` member; a detector created without tags still reports `tags: {}` (recorded behavior; the model types tags as a TagMap). Emulator: serviceRole is the detector role arn:...:role/aws-service-role/guardduty.amazonaws.com/AWSServiceRoleForAmazonGuardDuty, features[0] is S3_DATA_EVENTS with no additionalConfiguration member, and `tags` is missing rather than an empty map. Why: a client that locates the malware-protection role, that indexes features positionally or reads additionalConfiguration, or that iterates tags without a null guard, breaks. Fix: the GetDetector response projection (feature ordering + additionalConfiguration member, empty-tag-map emission) and the malware-protection service-role derivation. Traces: guardduty-detector-duplicate-create-error#task#3.

Cause 3 of 4: the same projection gaps persist after feature and tag updates. AWS: as in causes 1 and 2 — updatedAt per data source, the database block, the malware-protection role and CLOUD_TRAIL-first feature ordering are all present on a detector that has had features and tags updated. Emulator: identical omissions after UpdateDetector/TagResource, so the gap is in the projection rather than in the create path. Why: a client reading back a detector it just reconfigured cannot confirm the configuration. Fix: same GetDetector projection. Traces: guardduty-detector-features-and-tags#task#2 (also #task#4 and guardduty-detector-lifecycle-basic#task#2).

Cause 4 of 4: data-source enablement requested at create/update is never applied to detector state. AWS: a detector whose data sources were enabled reports status ENABLED on cloudTrail, dnsLogs, flowLogs, s3Logs, kubernetes.auditLogs, malwareProtection.scanEc2InstanceWithFindings.ebsVolumes and database.rds.databaseEvents. Emulator: all of those report DISABLED on the read-back while features[0] reports S3_DATA_EVENTS ENABLED, so the feature state and the dataSources view disagree. Why: a client that enables data sources and polls GetDetector never observes them enabled and will retry or fail a readiness gate forever. Fix: the detector state write path for CreateDetector/UpdateDetector dataSources+features, and the projection that derives the deprecated dataSources view from feature state. Traces: guardduty-detector-lifecycle-basic#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (86 fixtures, 555 steps).

Cause 1 — guardduty-detector-basic-lifecycle#task#3, GetDetector, expected 200 / actual 200:
  $.dataSources.cloudTrail.updatedAt: missing (expected '<shaped-not-asserted>', got None)
  $.dataSources.dnsLogs.updatedAt / flowLogs.updatedAt / s3Logs.updatedAt / kubernetes.auditLogs.updatedAt / malwareProtection.scanEc2InstanceWithFindings.ebsVolumes.updatedAt: missing
  $.dataSources.database: missing (expected {'rds': {'databaseEvents': {'status': 'DISABLED', 'updatedAt': '<shaped-not-asserted>'}}}, got None)
  cluster also covers guardduty-detector-basic-lifecycle#task#5

Cause 2 — guardduty-detector-duplicate-create-error#task#3, GetDetector, expected 200 / actual 200:
  $.dataSources.malwareProtection.serviceRole: value_mismatch (expected 'arn:aws:iam::000000000000:role/aws-service-role/malware-protection.guardduty.amazonaws.com/AWSServiceRoleForAmazonGuardDutyMalwareProtection', got 'arn:aws:iam::000000000000:role/aws-service-role/guardduty.amazonaws.com/AWSServiceRoleForAmazonGuardDuty')
  $.features[0]: value_mismatch (expected {'name': 'CLOUD_TRAIL', 'status': 'ENABLED', 'updatedAt': <epoch>, 'additionalConfiguration': None}, got {'name': 'S3_DATA_EVENTS', 'status': 'ENABLED', 'updatedAt': <epoch>})
  $.tags: missing (expected {}, got None)
  plus the cause-1 updatedAt/database omissions

Cause 3 — guardduty-detector-features-and-tags#task#2, GetDetector, 200/200: same updatedAt + database + serviceRole + features[0] divergences; cluster also covers guardduty-detector-features-and-tags#task#4 and guardduty-detector-lifecycle-basic#task#2.

Cause 4 — guardduty-detector-lifecycle-basic#task#5, GetDetector, 200/200:
  $.dataSources.cloudTrail.status: value_mismatch (expected 'ENABLED', got 'DISABLED')
  likewise dnsLogs.status, flowLogs.status, s3Logs.status, kubernetes.auditLogs.status, malwareProtection.scanEc2InstanceWithFindings.ebsVolumes.status
  $.dataSources.database: missing (expected {'rds': {'databaseEvents': {'status': 'ENABLED', ...}}}, got None)

Timestamps, account ids and generated detector ids in the recordings are examples; the requirement is the presence of the members, the role's service principal/role name, the feature ordering and the ENABLED statuses.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from mainline.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Inspect these steps:
   - guardduty-detector-basic-lifecycle#task#3 (GetDetector after CreateDetector): response has no dataSources.*.updatedAt and no dataSources.database.
   - guardduty-detector-duplicate-create-error#task#3: malwareProtection.serviceRole is the detector role, features[0] is S3_DATA_EVENTS with no additionalConfiguration, tags absent.
   - guardduty-detector-features-and-tags#task#2: same omissions after feature/tag updates.
   - guardduty-detector-lifecycle-basic#task#5 (GetDetector after enabling data sources): every dataSources status is still DISABLED.
4. Equivalently: CreateDetector with enable=true and data sources enabled, then GET /detector/{detectorId} and compare the dataSources/features/tags projection against the recorded response.
