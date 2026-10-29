Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-negative-envelopes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-negative-envelopes kinesisanalyticsv2-application-version-history
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|DeleteApplication|4xx->2xx|$
Issue-Key: kinesisanalyticsv2|DeleteApplication|4xx->2xx|status
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->4xx|$.ApplicationDetail
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->4xx|status
Issue-Key: kinesisanalyticsv2|DeleteApplication|2xx->4xx|status
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotsEnabled
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.ConfigurationType
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationVersionId
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ApplicationVersionUpdatedFrom
Issue-Related: 20261029T173921Z-kinesisanalyticsv2-applicationdetail-and-applicationsummary-omit-modeled-description-members-applicationmode.md
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# DeleteApplication skips CreateTimestamp deserialization and UpdateApplication never advances application version state

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplication

## Description
Shared fault: the application mutation path does not enforce or record the state AWS does - a malformed required timestamp is accepted instead of rejected (and the application is destroyed as a side effect), and an accepted UpdateApplication leaves the stored version and configuration unchanged. 4 causes.

Cause 1 of 4: DeleteApplication accepts a CreateTimestamp that cannot be deserialized.
AWS: Smithy DeleteApplication input member CreateTimestamp -> Timestamp is @required. A JSON value that is not convertible to an epoch timestamp fails protocol deserialization before the operation runs, so AWS answers HTTP 400 with __type SerializationException and Message "STRING_VALUE can not be converted to milliseconds since epoch" (fixed text; "STRING_VALUE" is the offending JSON value's type token). No application is deleted.
Emulator: answers HTTP 200 with an empty body and deletes the application - the malformed timestamp is ignored rather than rejected.
Why: this is a fail-open on a required input. A client that sends a wrong-typed CreateTimestamp (the classic stale-value or wrong-unit bug) gets success from the emulator and silently destroys a running application, where AWS would have refused the call; and code paths that handle SerializationException are never exercised.
Fix: the JSON protocol deserializer for DeleteApplication input (timestamp member coercion) must raise SerializationException/400 before the handler mutates state.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#10.

Cause 2 of 4: the application is gone, so the following DescribeApplication fails (direct consequence of Cause 1, separate replay step).
AWS: after the rejected delete the application still exists, so DescribeApplication returns 200 with the full ApplicationDetail (ARN, name, description, RuntimeEnvironment FLINK-1_15, ApplicationStatus READY, ApplicationVersionId 1, ServiceExecutionRole, configuration/maintenance descriptions, ApplicationMode STREAMING, timestamps).
Emulator: answers 400 with no ApplicationDetail, because the application was deleted by Cause 1.
Why: the whole post-negative-case portion of a client workflow runs against a non-existent resource; a fixer must re-replay this step to confirm the resource survives.
Fix: resolved by Cause 1 (reject before mutating state); no separate DescribeApplication change expected.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#11.

Cause 3 of 4: fixture teardown DeleteApplication now fails (same consequence, separate replay step).
AWS: the teardown delete with the correct CreateTimestamp succeeds with 200.
Emulator: 400 - the application no longer exists.
Why: same root cause; listed so the fixer's replay exercises the teardown step too.
Fix: resolved by Cause 1.
Traces: kinesisanalyticsv2-application-negative-envelopes#teardown#12.

Cause 4 of 4: UpdateApplication does not bump ApplicationVersionId, does not set ApplicationVersionUpdatedFrom, and does not apply the configuration update.
AWS: Smithy ApplicationDetail declares ApplicationVersionId (@required, long 1..999999999) with doc "Managed Service for Apache Flink updates the ApplicationVersionId each time you update the application", and ApplicationVersionUpdatedFrom ("the previous application version before the latest application update"). After two successful UpdateApplication calls the recording shows ApplicationVersionId 3 and ApplicationVersionUpdatedFrom 2, with the supplied updates reflected in the description - ApplicationSnapshotConfigurationDescription.SnapshotsEnabled true, FlinkApplicationConfigurationDescription.MonitoringConfigurationDescription {ConfigurationType CUSTOM, LogLevel DEBUG, MetricsLevel TASK}, CheckpointConfigurationDescription.ConfigurationType DEFAULT after the configuration was reverted, and ParallelismConfigurationDescription present with its defaults. Version numbers advance by one per accepted update; the specific values above are that fixture's sequence.
Emulator: DescribeApplication after the updates still reports ApplicationVersionId 1, no ApplicationVersionUpdatedFrom, SnapshotsEnabled false, CheckpointConfigurationDescription.ConfigurationType CUSTOM, and null MonitoringConfigurationDescription / ParallelismConfigurationDescription (plus the ApplicationMode / maintenance / encryption / rollback / LastUpdateTimestamp omissions filed separately); status 200 in both.
Why: optimistic concurrency on this API is driven by CurrentApplicationVersionId/ConditionalToken. With the version frozen at 1, every subsequent update is accepted with a stale version id, rollback cannot work, and a client that polls until its configuration change is visible loops forever because the describe never reflects the update.
Fix: the UpdateApplication handler and application state must apply the configuration update, increment ApplicationVersionId, record ApplicationVersionUpdatedFrom and refresh ConditionalToken/LastUpdateTimestamp.
Traces: kinesisanalyticsv2-application-version-history#task#9.

Note: the two open Kinesis Analytics V2 AmbiguousService issues do not cover these - the operations are reached and answer, but with the wrong status and the wrong stored state.

## Evidence
Replay of simparity-wire-aws-v1, all clusters kinesisanalyticsv2.
Cause 1 - kinesisanalyticsv2-application-negative-envelopes#task#10 DeleteApplication: status expected 400, got 200; body expected {'__type': 'SerializationException', 'Message': 'STRING_VALUE can not be converted to milliseconds since epoch'}, got {} (empty_body).
Cause 2 - kinesisanalyticsv2-application-negative-envelopes#task#11 DescribeApplication: status expected 200, got 400; $.ApplicationDetail missing (expected the full detail: ApplicationARN .../application/<name>, ApplicationStatus READY, ApplicationVersionId 1, RuntimeEnvironment FLINK-1_15, ApplicationMode STREAMING, ServiceExecutionRole, configuration and maintenance descriptions, shaped timestamps and ConditionalToken).
Cause 3 - kinesisanalyticsv2-application-negative-envelopes#teardown#12 DeleteApplication: status expected 200, got 400.
Cause 4 - kinesisanalyticsv2-application-version-history#task#9 DescribeApplication, 200 vs 200: $.ApplicationDetail.ApplicationVersionId value_mismatch (expected 3, got 1); $.ApplicationDetail.ApplicationVersionUpdatedFrom missing (expected 2); ApplicationSnapshotConfigurationDescription.SnapshotsEnabled value_mismatch (expected True, got False); FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.ConfigurationType value_mismatch (expected 'DEFAULT', got 'CUSTOM'); .MonitoringConfigurationDescription missing (expected {ConfigurationType CUSTOM, LogLevel DEBUG, MetricsLevel TASK}); .ParallelismConfigurationDescription missing; ConditionalToken unchanged across versions ('17817d2a-d93c-ba8f-9460-f93b051327f2' at both task#3 and task#9).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay kinesisanalyticsv2-application-negative-envelopes to task#10 - DeleteApplication with a non-timestamp JSON value for the required CreateTimestamp - and observe 200 plus the application being removed instead of 400 SerializationException.
3. Causes 2 and 3: continue the same fixture through task#11 (DescribeApplication, expected 200) and teardown#12 (DeleteApplication with the correct CreateTimestamp, expected 200); both now answer 400.
4. Cause 4: replay kinesisanalyticsv2-application-version-history through its UpdateApplication steps to task#9 (DescribeApplication) and compare ApplicationVersionId, ApplicationVersionUpdatedFrom and the configuration descriptions against the recorded response.
