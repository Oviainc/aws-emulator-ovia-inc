Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-lifecycle kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected kinesisanalyticsv2-application-version-history
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationEncryptionConfigurationDescription
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationSystemRollbackConfigurationDescription
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationMaintenanceConfigurationDescription
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationMode
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ConditionalToken
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.LastUpdateTimestamp
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationEncryptionConfigurationDescription
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationSystemRollbackConfigurationDescription
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationMaintenanceConfigurationDescription
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationMode
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ConditionalToken
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.LastUpdateTimestamp
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.MonitoringConfigurationDescription
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.ParallelismConfigurationDescription
Issue-Key: kinesisanalyticsv2|ListApplications|2xx->2xx|$.ApplicationSummaries[*]
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# ApplicationDetail and ApplicationSummary omit modeled description members (ApplicationMode, Flink/encryption/rollback/maintenance configuration)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApplication

## Description
Shared fault: the kinesisanalyticsv2 application serializer/state builds only a minimal ApplicationDetail/ApplicationSummary. The modeled description sub-structures that AWS always materialises with service defaults, plus ApplicationMode and LastUpdateTimestamp, are absent from every response that returns an application, and the ConditionalToken it does return is a UUID that never rotates. 5 causes.

Cause 1 of 5: CreateApplication response is missing ApplicationDetail description members.
AWS: Smithy (com.amazonaws.kinesisanalyticsv2) CreateApplication output member ApplicationDetail -> ApplicationDetail is @required, and ApplicationDetail declares ApplicationConfigurationDescription, ApplicationMaintenanceConfigurationDescription, ApplicationMode (enum ApplicationMode: STREAMING|INTERACTIVE), LastUpdateTimestamp and ConditionalToken (@length 1..512, @pattern ^[a-zA-Z0-9-_+/=]+$). Recorded behavior: for a FLINK runtime application created with no explicit Flink configuration, AWS returns ApplicationConfigurationDescription populated with service defaults - ApplicationEncryptionConfigurationDescription (KeyType AWS_OWNED_KEY), ApplicationSystemRollbackConfigurationDescription (RollbackEnabled false) and FlinkApplicationConfigurationDescription containing CheckpointConfigurationDescription (ConfigurationType DEFAULT, CheckpointingEnabled true, CheckpointInterval 60000, MinPauseBetweenCheckpoints 5000), MonitoringConfigurationDescription (ConfigurationType DEFAULT, LogLevel INFO, MetricsLevel APPLICATION) and ParallelismConfigurationDescription (ConfigurationType DEFAULT, AutoScalingEnabled true, Parallelism 1, ParallelismPerKPU 1, CurrentParallelism 1) - plus ApplicationMaintenanceConfigurationDescription with the account's maintenance window, ApplicationMode STREAMING for a non-notebook Flink application, and a LastUpdateTimestamp. The concrete window times and timestamp values are per-environment; the presence and default-valued sub-structures are the requirement.
Emulator: returns ApplicationDetail with ApplicationEncryptionConfigurationDescription, ApplicationSystemRollbackConfigurationDescription, FlinkApplicationConfigurationDescription, ApplicationMaintenanceConfigurationDescription, ApplicationMode and LastUpdateTimestamp all absent (null), and emits ConditionalToken as a hyphenated 36-character UUID that the fixture's shape assertion rejects (Hypothesis: AWS issues an opaque base64-style token rather than a UUID; the pattern itself does permit hyphens, so only the shape family differs).
Why: an SDK client that reads response.applicationDetail().applicationMode() or the Flink checkpoint/parallelism/monitoring descriptions to decide whether defaults are in force gets null and either NPEs or concludes the feature is off. Tooling that round-trips a described configuration back into UpdateApplication drops the whole Flink configuration.
Fix: the CreateApplication handler/state must seed the default configuration descriptions when the request omits them, and the ApplicationDetail serializer must emit all of them.
Traces: kinesisanalyticsv2-application-lifecycle#task#2.

Cause 2 of 5: DescribeApplication right after create is missing the same members.
AWS: DescribeApplication output ApplicationDetail is @required and carries the same fields described in Cause 1.
Emulator: the same seven paths are missing/divergent ($.ApplicationDetail.ApplicationConfigurationDescription.ApplicationEncryptionConfigurationDescription, .ApplicationSystemRollbackConfigurationDescription, .FlinkApplicationConfigurationDescription, $.ApplicationDetail.ApplicationMaintenanceConfigurationDescription, $.ApplicationDetail.ApplicationMode, $.ApplicationDetail.LastUpdateTimestamp, $.ApplicationDetail.ConditionalToken), status 200 in both.
Why: describe is the read path every poller uses, so the gap is observed on every subsequent reconcile, not just at create time.
Fix: the DescribeApplication handler must project the stored application through the same ApplicationDetail serializer as Cause 1.
Traces: kinesisanalyticsv2-application-lifecycle#task#3.

Cause 3 of 5: DescribeApplication omits ApplicationConfigurationDescription entirely after maintenance-configuration updates.
AWS: DescribeApplication returns ApplicationConfigurationDescription (including ApplicationSnapshotConfigurationDescription.SnapshotsEnabled) and ApplicationMaintenanceConfigurationDescription reflecting the window set by UpdateApplicationMaintenanceConfiguration, plus ApplicationMode and LastUpdateTimestamp.
Emulator: ApplicationConfigurationDescription, ApplicationMaintenanceConfigurationDescription, ApplicationMode and LastUpdateTimestamp are all null; status 200 in both.
Why: a client cannot read back the maintenance window it just set, so maintenance-window management is unverifiable against the emulator.
Fix: the application state must persist the maintenance window and configuration descriptions and the describe serializer must emit them.
Traces: kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected#task#7.

Cause 4 of 5: same omissions on the version-history fixture's describe.
AWS: as Cause 2, with the Flink MonitoringConfigurationDescription and ParallelismConfigurationDescription present even when only a checkpoint configuration was supplied at create time.
Emulator: FlinkApplicationConfigurationDescription.MonitoringConfigurationDescription and .ParallelismConfigurationDescription are null alongside the encryption/rollback/maintenance/ApplicationMode/LastUpdateTimestamp omissions; ConditionalToken is again a UUID.
Why: a partially supplied FlinkApplicationConfiguration must still be completed with defaults; otherwise clients see an application with no monitoring or parallelism settings at all.
Fix: configuration merge in the CreateApplication/UpdateApplication state layer must fill unsupplied Flink sub-configurations with their DEFAULT descriptions.
Traces: kinesisanalyticsv2-application-version-history#task#3.

Cause 5 of 5: ListApplications summaries omit ApplicationMode.
AWS: Smithy ApplicationSummary declares ApplicationMode (enum ApplicationMode); for a Managed Service for Apache Flink application it is STREAMING (INTERACTIVE for a Studio notebook), and the recording shows it present in every summary.
Emulator: $.ApplicationSummaries[0] carries ApplicationName, ApplicationARN, ApplicationStatus, ApplicationVersionId and RuntimeEnvironment but no ApplicationMode; status 200 in both.
Why: callers that list applications and filter notebooks out by ApplicationMode see null for every entry and cannot distinguish STREAMING from INTERACTIVE without an extra describe.
Fix: the ListApplications serializer must emit ApplicationMode from the stored application.
Traces: kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected#task#15.

Note: the two open issues about Kinesis Analytics V2 answering 500 AmbiguousService do not cover these - the service now answers 200 and diverges in the body.

## Evidence
Replay of simparity-wire-aws-v1 (score 32.7%, 13 divergence clusters, all kinesisanalyticsv2).
Cause 1 - kinesisanalyticsv2-application-lifecycle#task#2 CreateApplication, 200 vs 200: $.ApplicationDetail.ApplicationConfigurationDescription.ApplicationEncryptionConfigurationDescription missing (expected {'KeyType': 'AWS_OWNED_KEY'}); .ApplicationSystemRollbackConfigurationDescription missing (expected {'RollbackEnabled': False}); .FlinkApplicationConfigurationDescription missing (expected CheckpointConfigurationDescription {ConfigurationType DEFAULT, CheckpointingEnabled true, CheckpointInterval 60000, MinPauseBetweenCheckpoints 5000}, MonitoringConfigurationDescription {DEFAULT, INFO, APPLICATION}, ParallelismConfigurationDescription {AutoScalingEnabled true, DEFAULT, CurrentParallelism 1, Parallelism 1, ParallelismPerKPU 1}); $.ApplicationDetail.ApplicationMaintenanceConfigurationDescription missing; $.ApplicationDetail.ApplicationMode missing (expected 'STREAMING'); $.ApplicationDetail.LastUpdateTimestamp missing; $.ApplicationDetail.ConditionalToken value_mismatch (shape-asserted, got '2e0865dd-c827-13b9-59fb-0bef4793e78d').
Cause 2 - kinesisanalyticsv2-application-lifecycle#task#3 DescribeApplication, 200 vs 200: identical seven paths.
Cause 3 - kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected#task#7 DescribeApplication, 200 vs 200: $.ApplicationDetail.ApplicationConfigurationDescription missing (whole structure, incl. ApplicationSnapshotConfigurationDescription {SnapshotsEnabled true}); ApplicationMaintenanceConfigurationDescription missing; ApplicationMode missing; LastUpdateTimestamp missing.
Cause 4 - kinesisanalyticsv2-application-version-history#task#3 DescribeApplication, 200 vs 200: FlinkApplicationConfigurationDescription.MonitoringConfigurationDescription and .ParallelismConfigurationDescription missing, plus encryption/rollback/maintenance/ApplicationMode/LastUpdateTimestamp missing and ConditionalToken '17817d2a-d93c-ba8f-9460-f93b051327f2'.
Cause 5 - kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected#task#15 ListApplications, 200 vs 200: $.ApplicationSummaries[0] expected to include 'ApplicationMode': 'STREAMING'; actual summary has no ApplicationMode key.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1/2: replay kinesisanalyticsv2-application-lifecycle steps task#2 (CreateApplication of a FLINK-1_15 STREAMING application with no Flink configuration) and task#3 (DescribeApplication of it) and diff ApplicationDetail against the recorded response.
3. Cause 3: replay kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected through task#7 (DescribeApplication after UpdateApplicationMaintenanceConfiguration).
4. Cause 4: replay kinesisanalyticsv2-application-version-history step task#3.
5. Cause 5: replay kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected step task#15 (ListApplications) and inspect ApplicationSummaries[0] for ApplicationMode.
