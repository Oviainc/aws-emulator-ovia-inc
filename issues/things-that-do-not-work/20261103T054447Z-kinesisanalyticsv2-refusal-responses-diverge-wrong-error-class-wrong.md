Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes kinesisanalyticsv2-kfs-execution-role-mistakes kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes kinesisanalyticsv2-kfs-studio-parallelism-mistakes kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes kinesisanalyticsv2-kfs-studio-update-mistakes
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->4xx|$.Message
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->2xx|$.Message
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->2xx|$.__type
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->2xx|status
Issue-Key: kinesisanalyticsv2|DeleteApplication|4xx->4xx|$.Message
Issue-Key: kinesisanalyticsv2|DeleteApplication|4xx->4xx|$.__type
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->4xx|$.__type
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->2xx|$.Message
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->2xx|$.__type
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->2xx|status
Issue-Key: kinesisanalyticsv2|UpdateApplication|4xx->4xx|$.Message
Issue-Key: kinesisanalyticsv2|UpdateApplication|4xx->4xx|status
Issue-Key: kinesisanalyticsv2|DescribeApplication|4xx->4xx|$.message
Issue-Key: kinesisanalyticsv2|UpdateApplication|4xx->4xx|$.message
Issue-Commit: ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5

# kinesisanalyticsv2 refusal responses diverge: wrong error class, wrong HTTP status, emulator-authored messages, and unenforced validations that fail open

> _Note: this issue was identified against the code base at commit `ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Shared fault: on the kinesisanalyticsv2 application path, every REFUSAL is produced by the emulator's own rules rather than the service's. Some refusals never happen at all (the request fails open, which then poisons the rest of the fixture), some pick a different modeled error shape than the service does, some serialize a modeled error with the wrong HTTP status, and several render an emulator-authored message where AWS has a fixed message template. 9 causes.

Cause 1 of 9: CreateApplication does not validate the region of a Glue DatabaseARN, so a cross-region catalog ARN is reported as a missing database.
AWS: Smithy GlueDataCatalogConfiguration.DatabaseARN -> DatabaseARN is @required, length 1..2048, pattern ^arn:, and CreateApplication declares InvalidArgumentException ("The specified input parameter value is not valid.", client error, HTTP 400). A DatabaseARN whose region segment is not the region the request was sent to is refused on the ARN itself, before any catalog lookup, with the message "Incorrect region in DatabaseArn, expecting '<endpoint-region>' but found '<arn-region>'." The fixed text is everything outside the two quoted values; <endpoint-region> is the region of the endpoint serving the request and <arn-region> is the region segment parsed out of the request's DatabaseARN.
Emulator: answers HTTP 400 with the same error code but the message "Cannot find database with name '<db-name>' in Glue Data Catalog" - i.e. it skips the ARN region check and falls through to its catalog-existence check.
Why: the two messages point a client at opposite fixes. The real defect is a region mismatch in the ARN the client passed, but the emulator tells the client the database does not exist, so the client creates (or looks for) a database that already exists in the other region and never corrects the ARN.
Fix: the DatabaseARN validation in the CreateApplication handler must parse the ARN and compare its region to the request's region before any Glue catalog lookup, emitting the region-mismatch message above.
Traces: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes#task#6.

Cause 2 of 9: CreateApplication accepts a ServiceExecutionRole it cannot assume - the refusal never happens.
AWS: Smithy CreateApplication.ServiceExecutionRole -> RoleARN is @required (length 1..2048, pattern ^arn:). The service assumes the role while validating the request, so a role ARN that does not resolve to an assumable role is refused with HTTP 400, __type InvalidArgumentException and message "Kinesis Analytics service doesn't have sufficient privileges to assume the role: <role-arn>. Please check the role provided." The fixed text is everything but <role-arn>, which is echoed verbatim from the request's ServiceExecutionRole. Nothing is created.
Emulator: answers HTTP 200 with a full ApplicationDetail and stores the application; __type and Message are absent.
Why: the most common real failure of this API (a role typo, a role in another account, a role that trusts another service) is reported as success, and the client is handed an application that references an identity which cannot run it. Worse, the created-but-should-not-exist application changes the outcome of every later step on that name (see Causes 4 and 5).
Fix: a role-assumability check in CreateApplication's validation (and the matching ServiceExecutionRoleUpdate check in UpdateApplication), run before any application state is written.
Traces: kinesisanalyticsv2-kfs-execution-role-mistakes#task#6 (cluster also covers kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#5 and kinesisanalyticsv2-kfs-studio-parallelism-mistakes#task#5).

Cause 3 of 9: DeleteApplication with a stale CreateTimestamp answers the wrong error class and an emulator-authored message.
AWS: Smithy DeleteApplication input is ApplicationName + CreateTimestamp -> Timestamp (both @required), and the operation declares ConcurrentModificationException ("Exception thrown as a result of concurrent modifications to an application..."). A well-formed CreateTimestamp that does not equal the application's stored CreateTimestamp is refused with __type ConcurrentModificationException and message "The application with name: '<name>' has a different create timestamp than the one you provided. Please call DescribeApplication to fetch valid create timestamp." - fixed text apart from <name>, the ApplicationName from the request. The recorded response carries this with HTTP 400 (the model's httpError for the shape is 409; the recording is the wire truth for this service, see Cause 7).
Emulator: answers __type InvalidArgumentException with the message "CreateTimestamp <epoch-seconds> does not match the create timestamp of application <name>" - a different error class and a message of its own invention.
Why: ConcurrentModificationException is this API's optimistic-concurrency signal; a client retry loop keyed on it (re-DescribeApplication, then re-delete with the fresh timestamp) never fires. InvalidArgumentException reads as a permanently malformed request, so the client aborts a delete that would have succeeded on retry.
Fix: the CreateTimestamp comparison in the DeleteApplication handler must raise ConcurrentModificationException with the message template above.
Traces: kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes#task#8.

Cause 4 of 9: CreateApplication does not enforce ApplicationMode/RuntimeEnvironment compatibility, so the refusal is replaced by a name-collision error.
AWS: CreateApplication pairs RuntimeEnvironment (@required enum) with ApplicationMode (enum STREAMING|INTERACTIVE). A ZEPPELIN-FLINK runtime is only valid with INTERACTIVE, so ApplicationMode STREAMING on a ZEPPELIN-FLINK runtime is refused with HTTP 400, __type InvalidArgumentException and message "Invalid configuration provided. ApplicationMode '<mode>' is not applicable to runtime environment : <runtime>" - fixed text apart from <mode> and <runtime>, taken from the request's ApplicationMode and RuntimeEnvironment. The refusal creates nothing, so repeating it returns the same error rather than a conflict.
Emulator: answers HTTP 400 with __type ResourceInUseException and message "Application '<name>' already exists." - because the same incompatible create earlier in the fixture was accepted (Cause 2's fail-open), the name is now taken and the emulator reports a collision instead of the configuration refusal.
Why: the client is told its name is in use when the actual defect is an incompatible runtime/mode pair; retrying with a fresh name reproduces the problem forever, and error handling keyed on InvalidArgumentException never matches.
Fix: a runtime/mode compatibility check in CreateApplication's input validation, ahead of the name-uniqueness check in application state.
Traces: kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#6 (cluster also covers task#7 of the same fixture).

Cause 5 of 9: DescribeApplication returns 200 for an application AWS never created.
AWS: DescribeApplication declares ResourceNotFoundException ("Specified application can't be found.", client error, HTTP 400). After the refused creates of Cause 4, the name does not exist, so DescribeApplication is refused with HTTP 400, __type ResourceNotFoundException and message "Application '<name>' does not exist." - fixed text apart from <name>, the ApplicationName from the request.
Emulator: answers HTTP 200 with an ApplicationDetail body and no __type/Message, because its fail-open create persisted the application.
Why: a client probing for existence (or cleaning up after a failed create) concludes the resource exists and proceeds against a resource real AWS does not have; the not-found branch of client code is never exercised.
Fix: once Causes 2 and 4 refuse before writing state, DescribeApplication's lookup must miss and raise ResourceNotFoundException with that message; the DescribeApplication handler's not-found path must be the one that answers.
Traces: kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#8.

Cause 6 of 9: UpdateApplication's missing-version-id refusal uses a generic emulator message instead of the runtime-specific one.
AWS: Smithy UpdateApplication input has CurrentApplicationVersionId (ApplicationVersionId, range 1..999999999) and ConditionalToken (length 1..512, pattern ^[a-zA-Z0-9-_+/=]+$), both optional in the model; for an application on a ZEPPELIN-FLINK runtime the version id is mandatory, and omitting it is refused with HTTP 400, __type InvalidArgumentException and message "CurrentApplicationVersionId parameter should be provided for application with RuntimeEnvironment: <runtime>" - fixed text apart from <runtime>, the target application's RuntimeEnvironment.
Emulator: answers the right status and code but the message "You must provide the CurrentApplicationVersionId or the ConditionalToken.", which states a different rule (either-or) than the service enforces for this runtime.
Why: the emulator's wording tells the client a ConditionalToken would satisfy the call on a Studio application; it does not, so the client retries with a token and is refused again, and message-matching assertions in tests diverge.
Fix: the version-id/conditional-token precondition check in the UpdateApplication handler must branch on the application's RuntimeEnvironment and emit the runtime-specific message.
Traces: kinesisanalyticsv2-kfs-studio-update-mistakes#task#6.

Cause 7 of 9: a modeled kinesisanalyticsv2 error is serialized with HTTP 409 where the service answers HTTP 400.
AWS: the recorded UpdateApplication refusal carries HTTP 400 with the correct error code and message (only the status diverges). Smithy marks ConcurrentModificationException with httpError 409, but this service's wire responses for that shape are HTTP 400 - recorded behavior only, and the recording is the requirement.
Emulator: answers HTTP 409 for the same refusal.
Why: clients and SDK retry/error classification keyed on the status (4xx bucketing, 409-specific conflict handling, proxies and gateways that treat 409 differently) take a different path than against AWS, and status assertions fail even though the payload is right.
Fix: the error serializer for the kinesisanalyticsv2 protocol must map this error shape to HTTP 400 rather than taking the model's httpError 409.
Traces: kinesisanalyticsv2-kfs-studio-update-mistakes#task#9.

Cause 8 of 9: constraint-violation messages print the raw anchored regex instead of AWS's unanchored pattern text.
AWS: ApplicationName carries pattern ^[a-zA-Z0-9_.-]+$ (length 1..128) on every application operation, and a violating name is refused with HTTP 400 and the standard validation envelope in the lowercase member `message`: "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+". The fixed text is the envelope; <value> is the rejected request value, <member> the input member's wire path, and the pattern clause is rendered WITHOUT the ^ and $ anchors.
Emulator: emits the same envelope but prints "^[a-zA-Z0-9_.-]+$", i.e. the model trait's literal value including anchors.
Why: clients and test suites that compare the validation message, and tooling that re-compiles the quoted pattern from the message, see a string AWS never emits; every anchored pattern in this service is affected, not just ApplicationName.
Fix: the constraint-validation message formatter (shared validation/serializer layer for this service) must strip leading ^ and trailing $ when rendering a pattern trait into the message.
Traces: kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes#task#5.

Cause 9 of 9: enum constraint messages list the allowed values in alphabetical order instead of the service's order.
AWS: LogLevelUpdate targets the LogLevel enum, and a lowercase value is refused with HTTP 400 and `message` = "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [WARN, ERROR, INFO, DEBUG]". The fixed text is the envelope plus that value list in that order; <value> and <member> come from the request. The order is neither alphabetical nor the model's declaration order (Smithy LogLevel declares INFO, WARN, ERROR, DEBUG) - recorded behavior only, and the recording is the requirement.
Emulator: emits the same envelope with the set sorted alphabetically, "[DEBUG, ERROR, INFO, WARN]".
Why: the message is part of the contract clients assert on; a sorted list silently differs from AWS for every enum in the service. This is the same formatter defect already filed for RuntimeEnvironment in open issue 20261030T053820Z-kinesisanalyticsv2-createapplication-enum-validation-message-lists-the-wrong-runtimeenvironment.md (that issue owns the CreateApplication RuntimeEnvironment value-set contents); this cause is the UpdateApplication LogLevel set and its ORDER, which that issue's replay key does not cover.
Fix: the enum-constraint message formatter must emit the service's recorded value order rather than sorting, in the shared validation message builder.
Traces: kinesisanalyticsv2-kfs-studio-update-mistakes#task#7.

Relation to open issues: 20261103T022644Z-kinesisanalyticsv2-studio-flink-runtime-rules-unenforced-createapplication-fails-open.md describes the same families of root cause from the BEHAVIORAL corpus and carries no wire replay keys, so none of the steps above are exercised by it; 20261029T173921Z (non-deserializable CreateTimestamp), 20261031T065833Z and 20261031T192811Z (ApplicationMode omission / ConditionalToken) and 20261102T114851Z (StopApplication message) cover different steps and do not cover any cause here.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 62.6%, 58 mismatched steps). Per cause, expected (recorded AWS) vs actual (emulator):

Cause 1 - kinesisanalyticsv2-kfs-catalog-database-arn-mistakes#task#6, CreateApplication, 400 vs 400: $.Message expected "Incorrect region in DatabaseArn, expecting 'us-west-2' but found 'us-east-1'.", got "Cannot find database with name 'sim2real-probe-kfs-cat-db-7c73b3ee' in Glue Data Catalog" (the quoted region and database values are this recording's values filling the templates).

Cause 2 - kinesisanalyticsv2-kfs-execution-role-mistakes#task#6, CreateApplication: status expected 400, got 200; $.__type expected 'InvalidArgumentException', got None; $.Message expected "Kinesis Analytics service doesn't have sufficient privileges to assume the role: <role-arn>. Please check the role provided." (recording's role ARN: arn:aws:iam::000000000000:role/sim2real-probe-kfs-role-role-61e1e0e4-typo), got None. Same cluster: kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#5, kinesisanalyticsv2-kfs-studio-parallelism-mistakes#task#5.

Cause 3 - kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes#task#8, DeleteApplication, 400 vs 400: $.__type expected 'ConcurrentModificationException', got 'InvalidArgumentException'; $.Message expected "The application with name: '<name>' has a different create timestamp than the one you provided. Please call DescribeApplication to fetch valid create timestamp.", got 'CreateTimestamp <epoch> does not match the create timestamp of application <name>' (recording: name sim2real-probe-kfs-del-ba910498, epoch 1600000000).

Cause 4 - kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#6 (and task#7), CreateApplication, 400 vs 400: $.__type expected 'InvalidArgumentException', got 'ResourceInUseException'; $.Message expected "Invalid configuration provided. ApplicationMode 'STREAMING' is not applicable to runtime environment : ZEPPELIN-FLINK-3_0", got "Application 'sim2real-probe-kfs-mode-bad-app-d6228632' already exists.".

Cause 5 - kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes#task#8, DescribeApplication: status expected 400, got 200; $.__type expected 'ResourceNotFoundException', got None; $.Message expected "Application '<name>' does not exist." (recording: sim2real-probe-kfs-mode-bad-app-d6228632), got None.

Cause 6 - kinesisanalyticsv2-kfs-studio-update-mistakes#task#6, UpdateApplication, 400 vs 400: $.Message expected 'CurrentApplicationVersionId parameter should be provided for application with RuntimeEnvironment: ZEPPELIN-FLINK-3_0', got 'You must provide the CurrentApplicationVersionId or the ConditionalToken.'.

Cause 7 - kinesisanalyticsv2-kfs-studio-update-mistakes#task#9, UpdateApplication: status expected 400, got 409; no other key diverged, so the code and message matched and only the HTTP status is wrong.

Cause 8 - kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes#task#5, DescribeApplication, 400 vs 400: $.message expected "1 validation error detected: Value 'flink studio realtime' at 'applicationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+", got the same text ending "...pattern: ^[a-zA-Z0-9_.-]+$".

Cause 9 - kinesisanalyticsv2-kfs-studio-update-mistakes#task#7, UpdateApplication, 400 vs 400: $.message expected "...Value 'debug' at 'applicationConfigurationUpdate.zeppelinApplicationConfigurationUpdate.monitoringConfigurationUpdate.logLevelUpdate' failed to satisfy constraint: Member must satisfy enum value set: [WARN, ERROR, INFO, DEBUG]", got the same text with "[DEBUG, ERROR, INFO, WARN]".

Smithy citations: CreateApplication input (ApplicationName @required length 1..128 pattern ^[a-zA-Z0-9_.-]+$, RuntimeEnvironment @required enum, ServiceExecutionRole -> RoleARN @required pattern ^arn:, ApplicationMode enum) with errors InvalidArgumentException (400), ResourceInUseException (400), ConcurrentModificationException (409); DeleteApplication input (ApplicationName, CreateTimestamp -> Timestamp, both @required) with ConcurrentModificationException; DescribeApplication errors InvalidArgumentException/InvalidRequestException/ResourceNotFoundException (400); UpdateApplication input (CurrentApplicationVersionId range 1..999999999, ConditionalToken pattern ^[a-zA-Z0-9-_+/=]+$, RuntimeEnvironmentUpdate); GlueDataCatalogConfiguration.DatabaseARN @required pattern ^arn:; enum LogLevel values INFO, WARN, ERROR, DEBUG.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay trace set simparity-wire-aws-v1 (service kinesisanalyticsv2).
2. Cause 1: replay kinesisanalyticsv2-kfs-catalog-database-arn-mistakes up to task#6 - CreateApplication for a Flink SQL application whose GlueDataCatalogConfiguration.DatabaseARN names a region other than the endpoint's; observe the catalog-not-found message instead of the region-mismatch message.
3. Cause 2: replay kinesisanalyticsv2-kfs-execution-role-mistakes to task#6 - CreateApplication with a ServiceExecutionRole ARN that resolves to no assumable role; observe HTTP 200 and an ApplicationDetail instead of 400 InvalidArgumentException.
4. Cause 3: replay kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes to task#8 - DeleteApplication with a CreateTimestamp that does not match the stored one; observe InvalidArgumentException with the emulator's message instead of ConcurrentModificationException with the stale-timestamp message.
5. Causes 4 and 5: replay kinesisanalyticsv2-kfs-studio-mode-and-runtime-mistakes to task#6, task#7 and task#8 - CreateApplication(RuntimeEnvironment=ZEPPELIN-FLINK-3_0, ApplicationMode=STREAMING) returns ResourceInUseException/already-exists instead of InvalidArgumentException, and the following DescribeApplication of that name returns 200 instead of 400 ResourceNotFoundException.
6. Cause 6: replay kinesisanalyticsv2-kfs-studio-update-mistakes to task#6 - UpdateApplication on a ZEPPELIN-FLINK application with neither CurrentApplicationVersionId nor ConditionalToken; compare the message.
7. Cause 7: same fixture, task#9 - observe HTTP 409 where the recording is HTTP 400 with the identical body.
8. Cause 8: replay kinesisanalyticsv2-kfs-studio-describe-and-delete-mistakes to task#5 - DescribeApplication with ApplicationName 'flink studio realtime'; compare the pattern clause of the validation message.
9. Cause 9: replay kinesisanalyticsv2-kfs-studio-update-mistakes to task#7 - UpdateApplication with zeppelinApplicationConfigurationUpdate.monitoringConfigurationUpdate.logLevelUpdate='debug'; compare the enum value-set order in the validation message.
