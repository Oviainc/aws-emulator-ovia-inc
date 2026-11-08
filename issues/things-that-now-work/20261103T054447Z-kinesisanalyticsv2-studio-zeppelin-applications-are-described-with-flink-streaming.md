Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes kinesisanalyticsv2-kfs-execution-role-mistakes kinesisanalyticsv2-kfs-mixed-case-glue-database-name kinesisanalyticsv2-kfs-reference-path-glue-db-then-studio-notebook kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes kinesisanalyticsv2-kfs-studio-parallelism-mistakes kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes kinesisanalyticsv2-kfs-studio-update-mistakes
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotsEnabled
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled
Issue-Key: kinesisanalyticsv2|UpdateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotsEnabled
Issue-Key: kinesisanalyticsv2|UpdateApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled
Issue-Related: 20261029T173921Z-kinesisanalyticsv2-deleteapplication-skips-createtimestamp-deserialization-and-updateapplication-never-advances.md
Issue-Commit: ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5

# Studio/Zeppelin applications are described with Flink streaming snapshot and checkpointing defaults on create, describe and update

> _Note: this issue was identified against the code base at commit `ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Shared fault: the emulator keeps ONE default configuration table for all runtimes, so an application created without snapshot/checkpoint configuration always reports the FLINK streaming defaults. Every response that serializes ApplicationConfigurationDescription for such an application is wrong in the same two members, on all three operations that return it. 3 causes.

Cause 1 of 3: CreateApplication's response reports SnapshotsEnabled=true and CheckpointingEnabled=true for an application AWS defaults to false.
AWS: Smithy models ApplicationSnapshotConfiguration.SnapshotsEnabled (@required boolean) and CheckpointConfiguration.CheckpointingEnabled (boolean, documented as true when ConfigurationType is DEFAULT for a Managed Service for Apache Flink application) but states no per-runtime default; the recorded CreateApplication responses for these applications return ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotsEnabled = false and FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled = false when the create request omits those configurations - recorded behavior only for the per-runtime split. The two booleans are structural response values, not per-run values.
Emulator: returns HTTP 200 with both members set to true.
Why: a client reading the create response to decide whether snapshots exist, whether it must pass SnapshotsEnabled on a later update, or whether checkpointing is active gets the opposite answer for every such application, and snapshot-dependent workflows are reported as available on applications that do not support them.
Fix: the default-configuration construction in application state must be keyed on RuntimeEnvironment/ApplicationMode (one default table per runtime family) instead of a single shared table, and CreateApplication's serializer must report it.
Traces: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes#task#9 (cluster also covers kinesisanalyticsv2-kfs-execution-role-mistakes#task#7, kinesisanalyticsv2-kfs-mixed-case-glue-database-name#task#5, kinesisanalyticsv2-kfs-reference-path-glue-db-then-studio-notebook#task#4, kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#9, kinesisanalyticsv2-kfs-studio-parallelism-mistakes#task#6).

Cause 2 of 3: DescribeApplication reports the same two members as true.
AWS: DescribeApplication returns the same ApplicationDetail shape and therefore the same stored configuration - SnapshotsEnabled false and CheckpointingEnabled false for these applications.
Emulator: returns HTTP 200 with both true, consistent with its create-time state, confirming the defect is in the stored defaults rather than in one serializer.
Why: DescribeApplication is the authoritative read a client uses before an update or a snapshot call; it reports a capability state AWS never reports, so the client takes the snapshot/checkpoint branch that AWS's own response would have skipped.
Fix: same default table as Cause 1; DescribeApplication's serializer must emit the stored per-runtime values.
Traces: kinesisanalyticsv2-kfs-mixed-case-glue-database-name#task#6 (cluster also covers kinesisanalyticsv2-kfs-reference-path-glue-db-then-studio-notebook#task#5, kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes#task#9, kinesisanalyticsv2-kfs-studio-update-mistakes#task#11).

Cause 3 of 3: UpdateApplication's response carries the same two wrong booleans after an update that does not touch them.
AWS: an update that changes unrelated configuration leaves SnapshotsEnabled and CheckpointingEnabled at their stored values, so the UpdateApplication response reports false for both.
Emulator: returns HTTP 200 with both true.
Why: a client that reads the update response (rather than re-describing) to confirm its change caches the wrong capability state, and an update-then-snapshot sequence diverges from AWS.
Fix: once the defaults are per-runtime, UpdateApplication must carry the stored values through unchanged; the update path must not re-derive them from the shared Flink table.
Traces: kinesisanalyticsv2-kfs-studio-update-mistakes#task#8 (cluster also covers task#10 of the same fixture).

Relation to open issues: open issue 20261103T022644Z-kinesisanalyticsv2-studio-flink-runtime-rules-unenforced-createapplication-fails-open.md describes the same per-runtime default defect from the BEHAVIORAL corpus (its Cause 7) and carries no wire replay keys, so none of the steps above are exercised by it; this issue is the wire-side evidence on the create, describe and update serializers.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. All three clusters diverge on exactly the same two JSON paths, expected (recorded AWS) vs actual (emulator):

$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotsEnabled: expected False, got True
$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled: expected False, got True

Cause 1 - CreateApplication, HTTP 200 vs 200, representative kinesisanalyticsv2-kfs-catalog-database-arn-mistakes#task#9; 6 steps in the cluster (kinesisanalyticsv2-kfs-execution-role-mistakes#task#7, kinesisanalyticsv2-kfs-mixed-case-glue-database-name#task#5, kinesisanalyticsv2-kfs-reference-path-glue-db-then-studio-notebook#task#4, kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#9, kinesisanalyticsv2-kfs-studio-parallelism-mistakes#task#6).
Cause 2 - DescribeApplication, HTTP 200 vs 200, representative kinesisanalyticsv2-kfs-mixed-case-glue-database-name#task#6; 4 steps in the cluster.
Cause 3 - UpdateApplication, HTTP 200 vs 200, representative kinesisanalyticsv2-kfs-studio-update-mistakes#task#8; 2 steps in the cluster.

Smithy citations: ApplicationSnapshotConfiguration.SnapshotsEnabled -> BooleanObject @required; CheckpointConfiguration.CheckpointingEnabled -> BooleanObject with ConfigurationType @required (DEFAULT documented as CheckpointingEnabled true / CheckpointInterval 60000 / MinPauseBetweenCheckpoints 5000 for a Managed Service for Apache Flink application); no per-runtime default is modeled, so the false/false expectation is recorded behavior only.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay trace set simparity-wire-aws-v1 (service kinesisanalyticsv2).
2. Cause 1: replay kinesisanalyticsv2-kfs-catalog-database-arn-mistakes to task#9 - CreateApplication with no ApplicationSnapshotConfiguration and no CheckpointConfiguration in the request; read ApplicationDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotsEnabled and .FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled in the 200 response - both are true where the recording has false.
3. Cause 2: replay kinesisanalyticsv2-kfs-mixed-case-glue-database-name to task#6 - DescribeApplication that application and read the same two members.
4. Cause 3: replay kinesisanalyticsv2-kfs-studio-update-mistakes to task#8 - UpdateApplication changing unrelated configuration and read the same two members in the update response.
