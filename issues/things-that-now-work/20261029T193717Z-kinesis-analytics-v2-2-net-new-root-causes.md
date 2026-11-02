Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.add-application-input-on-flink-application-rejected kinesisanalyticsv2.update-with-stale-version-id-rejected
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2.add-application-input-on-flink-application-rejected|run#21|AssertionError|assert err["Code"] == "InvalidArgumentException", (
Issue-Key: kinesisanalyticsv2.update-with-stale-version-id-rejected|_parallelism#10|KeyError|flink = kda.describe_application(ApplicationName=app)["ApplicationDetail"][
Issue-Commit: 954781cef49baf7406aef764139782df619c6255

# kinesis-analytics-v2: 2 net-new root causes — AddApplicationInput is an unimplemented 501 scaffold (never refuses SQL input on a Flink runtime), and DescribeApplication omits ApplicationConfigurationDescription for a freshly created Flink application

> _Note: this issue was identified against the code base at commit `954781cef49baf7406aef764139782df619c6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddApplicationInput

## Description
Two net-new root causes found replaying the behavioral suite against the Kinesis Analytics V2 (Managed Service for Apache Flink) emulator. The service now answers (it is bootstrapped and CreateApplication/UpdateApplication succeed), so neither cause is covered by the two open "500 AmbiguousService" issues (20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md, 20261022T145625Z-kinesisanalyticsv2-kinesis-analytics-v2-wire-lifecycle-operations-all-answer.md). 2 causes.

Cause 1 of 2: AddApplicationInput answers 501 NotImplemented instead of its modeled 400 refusal of a SQL input on a FLINK runtime application.
AWS: Smithy (com.amazonaws.kinesisanalyticsv2) binds AddApplicationInput with input members ApplicationName (@required, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$), CurrentApplicationVersionId (@required, @range 1..999999999) and Input (@required), output {ApplicationARN, ApplicationVersionId, InputDescriptions}, and models the errors CodeValidationException, ConcurrentModificationException (HTTP 409), InvalidArgumentException (HTTP 400, "The specified input parameter value is not valid"), InvalidRequestException, ResourceInUseException and ResourceNotFoundException (all HTTP 400). The operation is documented as adding a streaming source to a SQL-based application only. Recorded behavior: on an application whose RuntimeEnvironment is FLINK-1_15, AddApplicationInput is refused with HTTP 400, error code 'InvalidArgumentException' and the message "You cannot add sql configuration to a Flink application." — that message is entirely fixed text for this error class (it carries no value from the request or from state) — and the refusal is inert: DescribeApplication still reports ApplicationVersionId 1 with no SqlApplicationConfigurationDescription.InputDescriptions, and ListApplicationVersions still reports exactly [1].
Emulator: every AddApplicationInput call is answered HTTP 501 with code 'NotImplemented' and the message "operation AddApplicationInput is scaffolded but not implemented in this emulator". The handler is still the generated stub: its only statement is Err(crate::scaffolded::Scaffolded::not_implemented("AddApplicationInput")), and the file still carries the SCAFFOLDED marker. No issue in this repo names AddApplicationInput, and the suite did not mark this check as already-filed/owned.
Why: a client that adds a streaming source gets a retryable 501 server error where AWS gives a deterministic 400 client error, so SDK retry policies retry a request AWS rejects outright, and the negative contract "a Flink application cannot take SQL inputs" cannot be exercised at all. Clients that legitimately add inputs to a SQL-based application have no working path either.
Fix: implement the handler in services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/add_application_input.rs — runtime-family validation (refuse InvalidArgumentException with the fixed message when the stored RuntimeEnvironment is a FLINK family), version-token check against the stored application, and the input/version mutation plus ApplicationVersionId advance on the accepted path; then remove the SCAFFOLDED marker.
Traces: kinesis-analytics-v2.add-application-input-on-flink-application-rejected.

Cause 2 of 2: DescribeApplication's ApplicationDetail carries no ApplicationConfigurationDescription at all for an application created without an explicit ApplicationConfiguration.
AWS: Smithy DescribeApplication output member ApplicationDetail is @required, and ApplicationDetail declares ApplicationConfigurationDescription. Recorded behavior: for a FLINK-1_15 application created with only ApplicationName/RuntimeEnvironment/ServiceExecutionRole, AWS materialises the configuration with service defaults, so ApplicationConfigurationDescription is present and its FlinkApplicationConfigurationDescription.ParallelismConfigurationDescription reports ConfigurationType 'DEFAULT' (with AutoScalingEnabled true and Parallelism/ParallelismPerKPU at their service defaults); after an UpdateApplication that sets ConfigurationTypeUpdate 'CUSTOM' the same path reports 'CUSTOM'. The default values themselves are the service's, not one run's.
Emulator: DescribeApplication returns 200 with an ApplicationDetail that has no 'ApplicationConfigurationDescription' key whatsoever, so reading ApplicationDetail['ApplicationConfigurationDescription'] raises KeyError immediately after CreateApplication. The open WIRE issue 20261029T173921Z-kinesisanalyticsv2-applicationdetail-and-applicationsummary-omit-modeled-description-members-applicationmode.md describes the same omission from recorded CreateApplication/DescribeApplication envelopes, but it carries no behavioral replay key, so this check is not covered by it; fixing that issue should be verified against this check too.
Why: the configuration description is the only read-back path for a Flink application's parallelism, checkpointing and monitoring settings. A client that writes a configuration and reconciles it sees nothing to reconcile, and any caller that dereferences the required-by-model structure crashes. It also makes UpdateApplication's optimistic-concurrency contract unobservable: this check could not get as far as asserting that replaying a consumed CurrentApplicationVersionId is refused with ConcurrentModificationException and the message "The provided application version ID does not match the most recent version for the application." — that assertion is unverified here, not reported as diverging.
Fix: the application state/serializer layer for this service — CreateApplication must seed the default configuration descriptions when the request omits them, and the DescribeApplication handler must project the stored application through the ApplicationDetail serializer that emits ApplicationConfigurationDescription.
Traces: kinesis-analytics-v2.update-with-stale-version-id-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 0/5 passed; both failures below are in service token 'kinesis-analytics-v2' (bootstrapped as service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523).

Cause 1 — [mismatch] kinesis-analytics-v2.add-application-input-on-flink-application-rejected:
  AssertionError: AddApplicationInput on a FLINK-1_15 application was refused with 'NotImplemented', expected 'InvalidArgumentException'
  Expected: HTTP 400, Code 'InvalidArgumentException', Message "You cannot add sql configuration to a Flink application."; then DescribeApplication shows no SqlApplicationConfigurationDescription.InputDescriptions, ApplicationVersionId 1, ListApplicationVersions [1].
  Actual: HTTP 501, Code 'NotImplemented', Message "operation AddApplicationInput is scaffolded but not implemented in this emulator".
  Handler: services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/add_application_input.rs line 179 returns Scaffolded::not_implemented("AddApplicationInput"); SCAFFOLDED marker at line 4.

Cause 2 — [error] kinesis-analytics-v2.update-with-stale-version-id-rejected:
  KeyError: 'ApplicationConfigurationDescription' raised from
  kda.describe_application(ApplicationName=app)['ApplicationDetail']['ApplicationConfigurationDescription'] immediately after a successful CreateApplication(RuntimeEnvironment='FLINK-1_15') that returned ApplicationVersionId 1.
  Expected: ApplicationDetail.ApplicationConfigurationDescription.FlinkApplicationConfigurationDescription.ParallelismConfigurationDescription.ConfigurationType == 'DEFAULT'.
  Actual: 200 response whose ApplicationDetail has no ApplicationConfigurationDescription key.

## Steps to Reproduce
1. Clone this checkout and run the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.
2. Cause 1: create an IAM role trusted by kinesisanalytics.amazonaws.com, CreateApplication(ApplicationName=<name>, RuntimeEnvironment='FLINK-1_15', ServiceExecutionRole=<role arn>), then call AddApplicationInput(ApplicationName=<name>, CurrentApplicationVersionId=1, Input={NamePrefix:'SOURCE_SQL_STREAM', KinesisStreamsInput:{ResourceARN:<a kinesis stream arn>}, InputSchema:{RecordFormat:{RecordFormatType:'JSON', MappingParameters:{JSONMappingParameters:{RecordRowPath:'$'}}}, RecordColumns:[{Name:'COL_A', SqlType:'VARCHAR(16)', Mapping:'$.a'}]}}). Observe 501 NotImplemented instead of 400 InvalidArgumentException "You cannot add sql configuration to a Flink application." Replay check kinesis-analytics-v2.add-application-input-on-flink-application-rejected.
3. Cause 2: CreateApplication a FLINK-1_15 application with no ApplicationConfiguration, then DescribeApplication(ApplicationName=<name>) and read ApplicationDetail['ApplicationConfigurationDescription']['FlinkApplicationConfigurationDescription']['ParallelismConfigurationDescription']['ConfigurationType']; the key is absent. Replay check kinesis-analytics-v2.update-with-stale-version-id-rejected, which also then exercises UpdateApplication with a consumed CurrentApplicationVersionId.
