Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-negative-envelopes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-negative-envelopes
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->5xx|$.__type
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->5xx|$.message
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->5xx|status
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->5xx|$.Message
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->5xx|$.Message
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->5xx|$.__type
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->5xx|status
Issue-Key: kinesisanalyticsv2|CreateApplicationSnapshot|4xx->5xx|$.Message
Issue-Key: kinesisanalyticsv2|CreateApplicationSnapshot|4xx->5xx|$.__type
Issue-Key: kinesisanalyticsv2|CreateApplicationSnapshot|4xx->5xx|status
Issue-Key: kinesisanalyticsv2|StartApplication|4xx->5xx|$.Message
Issue-Key: kinesisanalyticsv2|StartApplication|4xx->5xx|$.__type
Issue-Key: kinesisanalyticsv2|StartApplication|4xx->5xx|status
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# Kinesis Analytics V2 negative envelopes answer 500 AmbiguousService instead of modeled 400 client errors

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
This bundle carries 5 causes. All five are error-path envelopes where the emulator must answer a MODELED 400 client error and instead answers HTTP 500 with the unmodeled error code 'AmbiguousService' (recorded message, example only: "2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one"). The shared trigger is the emulator's service-resolution layer refusing every awsJson1_1 request signed for 'kinesisanalytics' because the 2015-08-14 and 2018-05-23 services both declare endpointPrefix/sigv4Name 'kinesisanalytics'; AWS serves both behind that one name and disambiguates on the X-Amz-Target service-shape prefix (KinesisAnalytics_20180523.<Operation>), so the collision is not an error condition and 'AmbiguousService' does not exist in AWS. Routing is only the gate, though: each cause below is a distinct validation/state rule that must then produce its own error code at 400, and each has its own replay key that must pass. Because the request status is 500 rather than 4xx, SDKs treat these client errors as retryable server faults.

Companion issues: the open behavioral issue 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md reports the same routing root cause but carries no wire fixture/step keys, so it gates none of the steps below; the lifecycle success envelopes are filed separately as 'Kinesis Analytics V2 wire lifecycle operations all answer 500 AmbiguousService...'. state_reset_failures=0. Note these negative steps were evaluated after the fixture's setup CreateApplication also failed, so once routing is fixed the error codes below must be re-confirmed against a properly created application.

Cause 1 of 5: CreateApplication with an out-of-range RuntimeEnvironment must be a ValidationException at 400.
AWS: Smithy CreateApplication.RuntimeEnvironment targets the enum RuntimeEnvironment (values SQL-1_0, FLINK-1_6, FLINK-1_8, ZEPPELIN-FLINK-1_0, FLINK-1_11, FLINK-1_13, ZEPPELIN-FLINK-2_0, FLINK-1_15, ZEPPELIN-FLINK-3_0, FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-2_2, FLINK-2_3). A value outside the enum is rejected by the service's generic request validator: recorded response is HTTP 400 with __type 'ValidationException' and a message built from the request, of the form "1 validation error detected: Value '<value>' at 'runtimeEnvironment' failed to satisfy constraint: Member must satisfy enum value set: [...]" (recorded example used 'FLINK-99_9'; the requirement is the status, the code, and that the message names the offending member and its enum constraint — the enum list in the recording is the set that account/region advertised and is not itself a requirement).
Emulator: HTTP 500 with __type 'AmbiguousService' and the router's collision message.
Why: a client validating input against the service sees a retryable 5xx instead of a terminal ValidationException, so it retries a request that can never succeed and never surfaces the field-level reason.
Fix: the router/service-resolution layer first, then CreateApplication input validation (enum membership check on RuntimeEnvironment, emitting ValidationException at 400).
Traces: kinesisanalyticsv2-application-negative-envelopes#task#3.

Cause 2 of 5: CreateApplication for a SQL runtime must be UnsupportedOperationException at 400.
AWS: Smithy CreateApplication lists UnsupportedOperationException among its errors (errorKind client, HTTP 400). Recorded behavior: creating an application with the SQL runtime answers 400 __type 'UnsupportedOperationException' with Message (capital M, per this service's error serialization) — recorded example "CreateApplication is not supported for SQL applications in this Region. You can use KDA Studio to build streaming applications with Flink SQL." The requirement is the 400 status and the UnsupportedOperationException code plus a present Message member; the exact prose is recorded behavior only.
Emulator: HTTP 500, __type 'AmbiguousService', $.Message absent altogether.
Why: clients that branch on UnsupportedOperationException to fall back to a Flink runtime instead see an unknown 5xx, and the absent Message member breaks error deserialization.
Fix: router first, then the CreateApplication handler's runtime gating and the error serializer (Message member on this service's exceptions).
Traces: kinesisanalyticsv2-application-negative-envelopes#task#4 (cluster also covers #task#5).

Cause 3 of 5: DescribeApplication on an absent application must be ResourceNotFoundException at 400.
AWS: Smithy DescribeApplication errors are InvalidArgumentException/InvalidRequestException/ResourceNotFoundException, all errorKind client at HTTP 400 (note 400, not 404). Recorded response: __type 'ResourceNotFoundException' with Message of the form "Application '<name>' does not exist." (example).
Emulator: HTTP 500, __type 'AmbiguousService', $.Message missing.
Why: existence checks are the most common client pattern against this API; a 5xx makes a definitive 'not found' indistinguishable from an outage and is retried.
Fix: router first, then the DescribeApplication handler's lookup-miss path emitting ResourceNotFoundException at 400 with a Message naming the application.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#6.

Cause 4 of 5: CreateApplicationSnapshot on an absent application must be InvalidArgumentException at 400.
AWS: Smithy CreateApplicationSnapshot models InvalidArgumentException (client, 400) alongside ResourceNotFoundException/InvalidApplicationConfigurationException/InvalidRequestException/LimitExceededException/ResourceInUseException/UnsupportedOperationException. Recorded behavior for a missing application on this path is __type 'InvalidArgumentException' at 400 with Message of the form "Application <name> does not exist under accountId <account>" (example) — i.e. this operation reports the miss as InvalidArgumentException, not ResourceNotFoundException, so the code is operation-specific and must not be normalised.
Emulator: HTTP 500, __type 'AmbiguousService', $.Message missing.
Why: snapshot orchestration that distinguishes a bad argument from a transient fault retries forever on a 5xx; a client matching InvalidArgumentException never matches.
Fix: router first, then the CreateApplicationSnapshot handler's missing-application path and the error serializer.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#7 (cluster also covers #task#8).

Cause 5 of 5: StartApplication with no valid code content must be InvalidApplicationConfigurationException at 400.
AWS: Smithy StartApplication models InvalidApplicationConfigurationException (client, 400) plus InvalidArgumentException/InvalidRequestException/ResourceInUseException/ResourceNotFoundException. Recorded response: 400 with __type 'InvalidApplicationConfigurationException' and Message of the form "Application '<name>' under account '<account>' does not have a valid configuration to start. Application is not configured with a valid code content. ..." (example).
Emulator: HTTP 500, __type 'AmbiguousService', $.Message missing.
Why: the start-readiness contract is unobservable — a caller cannot tell a mis-configured application from a server failure, so it cannot stop retrying or report the configuration defect.
Fix: router first, then the StartApplication handler's configuration-readiness check against stored application configuration, emitting InvalidApplicationConfigurationException at 400.
Traces: kinesisanalyticsv2-application-negative-envelopes#task#9.

## Evidence
Replay of trace_set simparity-wire-aws-v1 against the emulator built from mainline: 38 steps, 6 passed, 28 mismatched, 4 blocked, score 15.8%, state_reset_failures=0, emulator_up=true.

Expected vs actual, per divergence cluster (status and error code are the requirements; recorded Message text is an example except where the rule is stated in the cause):
- kinesisanalyticsv2-application-negative-envelopes#task#3 CreateApplication (bad RuntimeEnvironment): expected 400 __type 'ValidationException' with message "1 validation error detected: Value 'FLINK-99_9' at 'runtimeEnvironment' failed to satisfy constraint: Member must satisfy enum value set: [FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_6, FLINK-1_8, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0]"; actual 500 __type 'AmbiguousService' with "2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one".
- kinesisanalyticsv2-application-negative-envelopes#task#4 CreateApplication (SQL runtime): expected 400 __type 'UnsupportedOperationException' with $.Message 'CreateApplication is not supported for SQL applications in this Region. You can use KDA Studio to build streaming applications with Flink SQL.'; actual 500 __type 'AmbiguousService', $.Message missing (2 affected steps, also #task#5).
- kinesisanalyticsv2-application-negative-envelopes#task#6 DescribeApplication (absent app): expected 400 __type 'ResourceNotFoundException' with $.Message "Application 'sim2real-probe-kdav2-absent-4b8ad2' does not exist."; actual 500 __type 'AmbiguousService', $.Message missing.
- kinesisanalyticsv2-application-negative-envelopes#task#7 CreateApplicationSnapshot (absent app): expected 400 __type 'InvalidArgumentException' with $.Message 'Application sim2real-probe-kdav2-absent-4b8ad2 does not exist under accountId 000000000000'; actual 500 __type 'AmbiguousService', $.Message missing (2 affected steps, also #task#8).
- kinesisanalyticsv2-application-negative-envelopes#task#9 StartApplication (no code content): expected 400 __type 'InvalidApplicationConfigurationException' with $.Message "Application 'sim2real-probe-kdav2-neg-4b8ad2' under account '000000000000' does not have a valid configuration to start. Application is not configured with a valid code content. Please update the application with a valid code content."; actual 500 __type 'AmbiguousService', $.Message missing.

Model evidence: services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json — RuntimeEnvironment enum values; CreateApplication errors include UnsupportedOperationException (client, 400); DescribeApplication errors include ResourceNotFoundException (client, 400); CreateApplicationSnapshot errors include InvalidArgumentException (client, 400); StartApplication errors include InvalidApplicationConfigurationException (client, 400). The colliding signing name comes from the v1 model services/service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814/..., which declares the same endpointPrefix/sigv4Name 'kinesisanalytics'.

## Steps to Reproduce
1. Clone mainline and boot the emulator built from it.
2. client = boto3.client('kinesisanalyticsv2', endpoint_url=<emulator>).
3. Call create_application(ApplicationName='probe', RuntimeEnvironment='FLINK-99_9', ServiceExecutionRole='arn:aws:iam::000000000000:role/probe-role', ApplicationConfiguration={...}) — observe HTTP 500 __type AmbiguousService instead of 400 ValidationException.
4. Call create_application with RuntimeEnvironment='SQL-1_0' — expect 400 UnsupportedOperationException with a Message member; observe 500 AmbiguousService with no Message.
5. Call describe_application(ApplicationName='does-not-exist') — expect 400 ResourceNotFoundException; observe 500 AmbiguousService.
6. Call create_application_snapshot(ApplicationName='does-not-exist', SnapshotName='s1') — expect 400 InvalidArgumentException; observe 500 AmbiguousService.
7. Create an application with no code content, then call start_application on it — expect 400 InvalidApplicationConfigurationException; observe 500 AmbiguousService.
8. To confirm a fix, re-replay ALL five keys: kinesisanalyticsv2-application-negative-envelopes#task#3 (representative), plus #task#4, #task#6, #task#7 and #task#9. Fixing routing alone will not close them — each must return its own modeled 400 error code with a Message member.
