Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-negative-envelopes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-negative-envelopes kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->4xx|$.Message
Issue-Key: kinesisanalyticsv2|CreateApplicationSnapshot|4xx->4xx|$.Message
Issue-Key: kinesisanalyticsv2|CreateApplicationSnapshot|4xx->4xx|$.__type
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->4xx|$.message
Issue-Key: kinesisanalyticsv2|DescribeApplicationSnapshot|4xx->4xx|$.Message
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# Kinesis Analytics V2 error envelopes use wrong exception type and non-AWS message wording

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Shared fault: the kinesisanalyticsv2 error paths return the right HTTP status but hand-written envelopes - one with the wrong modeled exception type, and three whose Message text does not match how AWS words that error class (quoting, punctuation and value ordering). 4 causes.

Cause 1 of 4: CreateApplication duplicate-name message wording.
AWS: Smithy CreateApplication declares ResourceInUseException (client error, HTTP 400). For a name that already exists AWS answers 400 ResourceInUseException with Message "Application '<ApplicationName>' already exists." - fixed text "Application ", " already exists." with the trailing period, and the application name single-quoted, taken from the request's ApplicationName.
Emulator: same status and __type but Message "Application <ApplicationName> already exists" - name unquoted and no trailing period.
Why: clients and test suites that match on the documented message text (or surface it to users) see a different string for the same error class, so duplicate-name handling that keys off the message fails.
Fix: the CreateApplication handler's duplicate-name error construction (message template).
Traces: kinesisanalyticsv2-application-negative-envelopes#task#5.

Cause 2 of 4: CreateApplicationSnapshot against a non-RUNNING application returns the wrong exception type.
AWS: Smithy CreateApplicationSnapshot declares both InvalidRequestException and ResourceInUseException (both client, HTTP 400). When the application is not RUNNING, AWS answers 400 with __type InvalidRequestException and Message "Application <ApplicationName> is in <ApplicationStatus> status. Snapshot creation is only allowed when application is in RUNNING status" - fixed text around the two variable parts (the request's ApplicationName and the application's current status from state), no trailing period.
Emulator: 400 with __type ResourceInUseException and Message "Application <ApplicationName> cannot be snapshotted while it is in the <ApplicationStatus> state".
Why: __type is what SDKs dispatch on, so a client catching InvalidRequestException does not catch this and the error surfaces as an unmodeled/unexpected ResourceInUseException; retry logic that treats ResourceInUseException as transient will retry an error AWS never retries.
Fix: the CreateApplicationSnapshot handler's state-gate must raise InvalidRequestException with the AWS message template.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#8.

Cause 3 of 4: CreateApplication enum validation message lists the RuntimeEnvironment value set in the wrong order.
AWS: Smithy RuntimeEnvironment is an enum; an unmodeled value yields 400 with the standard constraint message "1 validation error detected: Value '<supplied>' at 'runtimeEnvironment' failed to satisfy constraint: Member must satisfy enum value set: [<values>]". The fixed text is that sentence; the variable parts are the supplied value, the member name and the enum value set, which AWS renders sorted lexicographically (recording: [FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_6, FLINK-1_8, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0]).
Emulator: emits the set in model-declaration order ([SQL-1_0, FLINK-1_6, FLINK-1_8, ZEPPELIN-FLINK-1_0, FLINK-1_11, ...]). Which runtimes are members depends on the model revision in use (this checkout's model also has FLINK-1_18/1_19/1_20/2_2/2_3, newer than the recording), so the requirement here is the lexicographic ordering of whatever the service's value set is, not the recorded membership.
Why: the enum-set rendering is the only machine-readable hint of the accepted values; clients and docs scrapers that parse it get a differently ordered list than AWS produces for the same request.
Fix: the shared enum-constraint validation message builder must sort the value set lexicographically before rendering.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#3.

Cause 4 of 4: DescribeApplicationSnapshot not-found message over-quotes the application name.
AWS: Smithy DescribeApplicationSnapshot declares ResourceNotFoundException (client, HTTP 400). For a missing snapshot AWS answers 400 with Message "Snapshot '<SnapshotName>' does not exist for application <ApplicationName>." - the snapshot name is single-quoted, the application name is NOT, and the sentence ends with a period; both names come from the request.
Emulator: 400 with Message "Snapshot '<SnapshotName>' does not exist for application '<ApplicationName>'." - the application name is additionally single-quoted.
Why: any assertion or log parser keyed on the AWS wording mismatches; the status and __type are right, so the defect is silent until a client compares text.
Fix: the DescribeApplicationSnapshot handler's not-found message template.
Traces: kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected#task#14.

Note: the two open 500-AmbiguousService issues for this service do not cover these - the operations now reach their handlers and diverge only in the error envelope.

## Evidence
Replay of simparity-wire-aws-v1, all clusters kinesisanalyticsv2, every one status-matched at 400 except as noted.
Cause 1 - kinesisanalyticsv2-application-negative-envelopes#task#5 CreateApplication, 400 vs 400: $.Message value_mismatch - expected "Application 'sim2real-probe-kdav2-neg-4b8ad2' already exists.", got "Application sim2real-probe-kdav2-neg-4b8ad2 already exists" (names are per-run values; the quoting and trailing period are the template).
Cause 2 - kinesisanalyticsv2-application-negative-envelopes#task#8 CreateApplicationSnapshot, 400 vs 400: $.__type value_mismatch - expected 'InvalidRequestException', got 'ResourceInUseException'; $.Message value_mismatch - expected "Application <name> is in READY status. Snapshot creation is only allowed when application is in RUNNING status", got "Application <name> cannot be snapshotted while it is in the READY state".
Cause 3 - kinesisanalyticsv2-application-negative-envelopes#task#3 CreateApplication, 400 vs 400: $.message value_mismatch - expected enum set "[FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_6, FLINK-1_8, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0]", got "[SQL-1_0, FLINK-1_6, FLINK-1_8, ZEPPELIN-FLINK-1_0, FLINK-1_11, FLINK-1_13, ZEPPELIN-FLINK-2_0, FLINK-1_15, ZEPPELIN-FLINK-3_0, FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-2_2, FLINK-2_3]" - declaration order instead of lexicographic.
Cause 4 - kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected#task#14 DescribeApplicationSnapshot, 400 vs 400: $.Message value_mismatch - expected "Snapshot '<snap>' does not exist for application <app>.", got "Snapshot '<snap>' does not exist for application '<app>'.".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay kinesisanalyticsv2-application-negative-envelopes to task#5 (CreateApplication re-using an existing ApplicationName) and compare $.Message.
3. Cause 2: continue to task#8 (CreateApplicationSnapshot while the application is READY) and compare $.__type and $.Message.
4. Cause 3: replay the same fixture's task#3 (CreateApplication with RuntimeEnvironment 'FLINK-99_9') and compare the enum value set ordering in $.message.
5. Cause 4: replay kinesisanalyticsv2-application-maintenance-and-config-deletes-rejected to task#14 (DescribeApplicationSnapshot for a snapshot that was never created) and compare $.Message quoting.
