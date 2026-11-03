Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.flink-streaming-runtime-in-interactive-mode-rejected kinesisanalyticsv2.zeppelin-runtime-in-streaming-or-unset-mode-rejected kinesisanalyticsv2.studio-code-content-that-is-not-a-zeppelin-note-rejected kinesisanalyticsv2.studio-notebook-with-a-role-that-does-not-exist-rejected kinesisanalyticsv2.studio-notebook-with-a-role-trusting-another-service-rejected kinesisanalyticsv2.studio-delete-with-a-stale-create-timestamp-rejected kinesisanalyticsv2.studio-notebook-defaults-disable-checkpoints-snapshots-and-autoscaling-unlike-a-flink-app kinesisanalyticsv2.studio-notebook-snapshot-and-rollback-rejected-as-unsupported
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2.flink-streaming-runtime-in-interactive-mode-rejected|_error#10|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it: {result!r}"[:600])
Issue-Key: kinesisanalyticsv2.zeppelin-runtime-in-streaming-or-unset-mode-rejected|_error#9|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it: {result!r}"[:600])
Issue-Key: kinesisanalyticsv2.studio-code-content-that-is-not-a-zeppelin-note-rejected|_error_detail#9|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it: {result!r}"[:600])
Issue-Key: kinesisanalyticsv2.studio-notebook-with-a-role-that-does-not-exist-rejected|_error#9|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it: {result!r}"[:600])
Issue-Key: kinesisanalyticsv2.studio-notebook-with-a-role-trusting-another-service-rejected|_error#12|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it: {result!r}"[:600])
Issue-Key: kinesisanalyticsv2.studio-delete-with-a-stale-create-timestamp-rejected|run#8|AssertionError|assert error["Code"] == "ConcurrentModificationException", (
Issue-Key: kinesisanalyticsv2.studio-notebook-defaults-disable-checkpoints-snapshots-and-autoscaling-unlike-a-flink-app|run#8|AssertionError|assert _defaults(s) == _STUDIO, f"the Studio notebook defaults to {_defaults(s)!r}, expected {_STUDIO!r}"
Issue-Key: kinesisanalyticsv2.studio-notebook-snapshot-and-rollback-rejected-as-unsupported|run#6|AssertionError|assert err["Code"] == "UnsupportedOperationException", (
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# Studio/Flink runtime rules unenforced: CreateApplication fails open on 5 refusal classes, DeleteApplication/snapshot/rollback use the wrong error class, and ZEPPELIN-FLINK gets Flink's defaults

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Shared theme: the emulator's kinesisanalyticsv2 application path has no RuntimeEnvironment/ApplicationMode compatibility validation, no execution-role assumability validation, no Zeppelin code validation, one single default table for every runtime, and it picks the wrong modeled error class on three refusal paths. 8 causes.

Cause 1 of 8: CreateApplication accepts ApplicationMode INTERACTIVE on a plain FLINK runtime.
AWS: Smithy CreateApplication input pairs RuntimeEnvironment (@required enum) with ApplicationMode (enum ApplicationMode = STREAMING|INTERACTIVE); INTERACTIVE belongs only to a ZEPPELIN-FLINK Studio runtime, and the pairing is refused with the modeled error InvalidArgumentException ("The specified input parameter value is not valid.", client error, HTTP 400) and message "Invalid configuration provided. ApplicationMode '<mode>' is not applicable to runtime environment : <runtime>" — the fixed text is everything but <mode> and <runtime>, which are the ApplicationMode and RuntimeEnvironment values from the request (here INTERACTIVE and FLINK-1_18). Nothing is created, so a following DescribeApplication of that name is ResourceNotFoundException "Application '<name>' does not exist."
Emulator: returns HTTP 200 with a full ApplicationDetail (RuntimeEnvironment FLINK-1_18, ApplicationStatus READY, ApplicationVersionId 1) and the application exists afterwards.
Why: a client that sends an incompatible mode (a copy-paste from a Studio template onto a streaming app) gets a success and a resource that cannot behave as either runtime; clients whose error path handles InvalidArgumentException never see it, and cleanup logic that assumes nothing was created leaks the application.
Fix: CreateApplication's input validation (runtime/mode compatibility check) in the CreateApplication handler, before any state is written.
Traces: kinesisanalyticsv2.flink-streaming-runtime-in-interactive-mode-rejected.

Cause 2 of 8: CreateApplication accepts a ZEPPELIN-FLINK runtime in STREAMING mode and with ApplicationMode omitted.
AWS: the mirror of Cause 1 — a ZEPPELIN-FLINK runtime is only valid with ApplicationMode INTERACTIVE. STREAMING is refused with InvalidArgumentException / "Invalid configuration provided. ApplicationMode 'STREAMING' is not applicable to runtime environment : <runtime>", and an OMITTED ApplicationMode is refused with the same code and the same template rendering the absent value as the literal 'null': "Invalid configuration provided. ApplicationMode 'null' is not applicable to runtime environment : <runtime>" (<runtime> is the request's RuntimeEnvironment, e.g. ZEPPELIN-FLINK-3_0). Neither call creates anything; DescribeApplication of either name is ResourceNotFoundException "Application '<name>' does not exist."
Emulator: both calls return HTTP 200 with an ApplicationDetail (RuntimeEnvironment ZEPPELIN-FLINK-3_0, READY, version 1) and the applications exist.
Why: the omitted-mode case is the common one (ApplicationMode is optional in the model and defaults to STREAMING for Flink), so a client building a Studio notebook without setting the mode is told it succeeded and then gets a notebook that no Studio operation accepts.
Fix: same validation point as Cause 1 — the CreateApplication handler must reject a ZEPPELIN-FLINK runtime whose ApplicationMode is STREAMING or absent, rendering an absent mode as 'null' in the message.
Traces: kinesisanalyticsv2.zeppelin-runtime-in-streaming-or-unset-mode-rejected.

Cause 3 of 8: CreateApplication accepts Studio code content that is not a Zeppelin note.
AWS: for a ZEPPELIN-FLINK notebook, ApplicationConfiguration.ApplicationCodeConfiguration.CodeContent.TextContent must be a Zeppelin note JSON object; a bare SQL/paragraph string is refused with InvalidArgumentException and message "Zeppelin application code provided in TextContent must be a valid Zeppelin note JSON object containing 'id' and 'name' fields" (entirely fixed text — no request values). Nothing is created; DescribeApplication of the name is ResourceNotFoundException "Application '<name>' does not exist."
Emulator: returns HTTP 200 with ApplicationDetail for the notebook (RuntimeEnvironment ZEPPELIN-FLINK-3_0, READY, version 1) and stores it.
Why: the emulator stores unusable code and reports success, so a client's validation-failure path is untested and the recorded notebook content is garbage.
Fix: CreateApplication's ApplicationCodeConfiguration validation for ZEPPELIN-FLINK runtimes (TextContent must parse as a note object carrying 'id' and 'name').
Traces: kinesisanalyticsv2.studio-code-content-that-is-not-a-zeppelin-note-rejected.

Cause 4 of 8: CreateApplication and UpdateApplication accept a ServiceExecutionRole that does not exist.
AWS: Smithy ServiceExecutionRole -> RoleARN is @required on CreateApplication (pattern ^arn:) and ServiceExecutionRoleUpdate is the same shape on UpdateApplication. The service assumes the role while validating the request, so a dangling role ARN is refused up front with InvalidArgumentException and message "Kinesis Analytics service doesn't have sufficient privileges to assume the role: <role-arn>. Please check the role provided." — fixed text apart from <role-arn>, which is echoed from the request. The refused create leaves nothing behind (DescribeApplication -> ResourceNotFoundException "Application '<name>' does not exist.") and the refused update leaves the existing role and ApplicationVersionId untouched.
Emulator: CreateApplication returns HTTP 200 with ApplicationDetail whose ServiceExecutionRole is the nonexistent ARN, and the application is created.
Why: the single most common real-world failure of this API (role typo, role not yet created, wrong account) is silently accepted; a client never learns the notebook cannot run and the emulator's state references an identity that does not exist.
Fix: a role-assumability check in the CreateApplication and UpdateApplication handlers' validation (resolve the ARN against IAM state before writing the application).
Traces: kinesisanalyticsv2.studio-notebook-with-a-role-that-does-not-exist-rejected.

Cause 5 of 8: CreateApplication accepts an execution role whose trust policy names another service.
AWS: the execution role must be assumable by kinesisanalytics.amazonaws.com; a role whose trust policy names a different principal (e.g. lambda.amazonaws.com) is refused on every attempt — not transiently — with the same class and message as Cause 4: InvalidArgumentException / "Kinesis Analytics service doesn't have sufficient privileges to assume the role: <role-arn>. Please check the role provided." (<role-arn> from the request). Nothing is created.
Emulator: returns HTTP 200 with ApplicationDetail (ZEPPELIN-FLINK-3_0, READY, version 1) for the lambda-trusting role and stores the application.
Why: distinct from Cause 4 — the role exists, so an existence check alone still passes it. A misconfigured trust policy is accepted, hiding the authorization defect a client would hit immediately on real AWS.
Fix: the same validation point as Cause 4 must evaluate the role's trust policy principal, not only the role's existence.
Traces: kinesisanalyticsv2.studio-notebook-with-a-role-trusting-another-service-rejected.

Cause 6 of 8: DeleteApplication with a mismatched CreateTimestamp answers the wrong error class and message.
AWS: Smithy DeleteApplication input has CreateTimestamp -> Timestamp @required, and the operation declares ConcurrentModificationException ("Exception thrown as a result of concurrent modifications to an application. This error can be the result of attempting to modify an application without using the current application ID.", client error, HTTP 409). A well-formed timestamp that does not equal the application's stored CreateTimestamp is refused with code ConcurrentModificationException and message "The application with name: '<name>' has a different create timestamp than the one you provided. Please call DescribeApplication to fetch valid create timestamp." — fixed text apart from <name>, the ApplicationName from the request. The application survives at its real status and timestamp, and a delete quoting the described timestamp then succeeds.
Emulator: answers code 'InvalidArgumentException' (HTTP 400) instead of 'ConcurrentModificationException' (HTTP 409).
Why: this is the API's optimistic-concurrency signal. A client retry loop keyed on ConcurrentModificationException / 409 (re-DescribeApplication, then re-delete) never fires, and InvalidArgumentException reads as a permanently malformed request, so the client aborts a delete that would have succeeded on retry.
Fix: the DeleteApplication handler's CreateTimestamp comparison must raise ConcurrentModificationException with that message rather than InvalidArgumentException. Distinct from open issue 20261029T173921Z-kinesisanalyticsv2-deleteapplication-skips-createtimestamp-deserialization-and-updateapplication-never-advances.md, which covers a NON-deserializable CreateTimestamp being accepted (SerializationException) — here the timestamp is valid and merely stale, and the call IS refused, only with the wrong class.
Traces: kinesisanalyticsv2.studio-delete-with-a-stale-create-timestamp-rejected.

Cause 7 of 8: a ZEPPELIN-FLINK notebook is described with a FLINK streaming app's defaults.
AWS: the omitted-configuration defaults differ by runtime. For a ZEPPELIN-FLINK (INTERACTIVE) notebook created with no checkpoint, parallelism or snapshot configuration, DescribeApplication reports FlinkApplicationConfigurationDescription.CheckpointConfigurationDescription.CheckpointingEnabled = false, ParallelismConfigurationDescription.AutoScalingEnabled = false and ApplicationSnapshotConfigurationDescription.SnapshotsEnabled = false, while the same omissions on a FLINK-1_18 STREAMING app yield all three true. (Smithy models these members — CheckpointConfiguration.CheckpointingEnabled, ParallelismConfiguration.AutoScalingEnabled, ApplicationSnapshotConfiguration.SnapshotsEnabled @required — but states no per-runtime default: recorded behavior only for the split.)
Emulator: the Studio notebook describes {checkpointing: true, autoscaling: true, snapshots: true}, i.e. the Flink defaults, where false/false/false is required.
Why: a client reading DescribeApplication to decide whether snapshots exist, whether to pass SnapshotsEnabled on update, or whether autoscaling is active gets the opposite answer for every Studio notebook; snapshot-dependent workflows are reported as available on a runtime that does not support them at all (see Cause 8).
Fix: the application state's default-configuration construction must be keyed on RuntimeEnvironment (one default table per runtime family) instead of a single shared table, and DescribeApplication's serializer must report it.
Traces: kinesisanalyticsv2.studio-notebook-defaults-disable-checkpoints-snapshots-and-autoscaling-unlike-a-flink-app.

Cause 8 of 8: CreateApplicationSnapshot and RollbackApplication on a ZEPPELIN-FLINK notebook answer the wrong error class.
AWS: both operations declare UnsupportedOperationException ("The request was rejected because a specified parameter is not supported or a specified resource is not valid for this operation.", client error, HTTP 400). These are FLINK-streaming features, so on a ZEPPELIN-FLINK application they are refused with code UnsupportedOperationException: CreateApplicationSnapshot with message "This operation is not supported for RuntimeEnvironment <runtime>" and RollbackApplication with message "RollbackApplication is not supported for application with runtime environment <runtime>." — fixed text apart from <runtime>, the application's RuntimeEnvironment (here ZEPPELIN-FLINK-3_0). Note the two messages are differently worded; neither refusal changes the application's ApplicationVersionId or ApplicationStatus.
Emulator: CreateApplicationSnapshot answers code 'InvalidRequestException' instead of 'UnsupportedOperationException' (the replay stops there, so RollbackApplication's class is asserted by the same check and must be fixed with it).
Why: InvalidRequestException tells a client its request was malformed and should be corrected; UnsupportedOperationException tells it the capability does not exist for this runtime and the branch should be skipped. A client feature-probing the runtime takes the wrong branch, and generated error-handling keyed on the modeled UnsupportedOperationException never matches.
Fix: the runtime-capability guards in the CreateApplicationSnapshot and RollbackApplication handlers must raise UnsupportedOperationException with the two messages above.
Traces: kinesisanalyticsv2.studio-notebook-snapshot-and-rollback-rejected-as-unsupported.

Note on neighbouring open issues: the two AmbiguousService routing issues, 20261030T053820Z (RuntimeEnvironment enum message), 20261031T065833Z / 20261031T192811Z (ApplicationMode omission and ConditionalToken) and 20261102T114851Z (StopApplication's ResourceInUseException message) do not cover any cause above — those operations are reached and answer; these are different validations, defaults and error classes.

## Evidence
Replay of simparity-behavioral-aws-v1, service kinesisanalyticsv2 (6/20 passed).

Cause 1 — kinesisanalyticsv2.flink-streaming-runtime-in-interactive-mode-rejected: CreateApplication(RuntimeEnvironment=FLINK-1_18, ApplicationMode=INTERACTIVE) expected InvalidArgumentException / "Invalid configuration provided. ApplicationMode 'INTERACTIVE' is not applicable to runtime environment : FLINK-1_18"; got HTTP 200 — "the call succeeded where real AWS rejects it: {'ApplicationDetail': {'RuntimeEnvironment': 'FLINK-1_18', 'ApplicationStatus': 'READY', 'ApplicationVersionId': 1, ...}}".

Cause 2 — kinesisanalyticsv2.zeppelin-runtime-in-streaming-or-unset-mode-rejected: CreateApplication(RuntimeEnvironment=ZEPPELIN-FLINK-3_0, ApplicationMode=STREAMING) expected InvalidArgumentException / "...ApplicationMode 'STREAMING' is not applicable to runtime environment : ZEPPELIN-FLINK-3_0" and, with ApplicationMode omitted, "...ApplicationMode 'null' is not applicable to runtime environment : ZEPPELIN-FLINK-3_0"; got HTTP 200 with ApplicationDetail (ZEPPELIN-FLINK-3_0, READY, version 1).

Cause 3 — kinesisanalyticsv2.studio-code-content-that-is-not-a-zeppelin-note-rejected: CreateApplication with ApplicationCodeConfiguration{CodeContentType=PLAINTEXT, TextContent="%flink.ssql\nSELECT ..."} expected ('InvalidArgumentException', "Zeppelin application code provided in TextContent must be a valid Zeppelin note JSON object containing 'id' and 'name' fields"); got HTTP 200 with ApplicationDetail (ZEPPELIN-FLINK-3_0, READY, version 1).

Cause 4 — kinesisanalyticsv2.studio-notebook-with-a-role-that-does-not-exist-rejected: CreateApplication(ServiceExecutionRole=<arn of a role never created>) expected InvalidArgumentException / "Kinesis Analytics service doesn't have sufficient privileges to assume the role: <role-arn>. Please check the role provided."; got HTTP 200 with ApplicationDetail whose ServiceExecutionRole is that nonexistent ARN.

Cause 5 — kinesisanalyticsv2.studio-notebook-with-a-role-trusting-another-service-rejected: CreateApplication with a role whose trust policy names lambda.amazonaws.com expected the same InvalidArgumentException refusal on every attempt across a 30 s window; got HTTP 200 with ApplicationDetail (ZEPPELIN-FLINK-3_0, READY, version 1) on the first attempt.

Cause 6 — kinesisanalyticsv2.studio-delete-with-a-stale-create-timestamp-rejected: "DeleteApplication with a CreateTimestamp one second off answered 'InvalidArgumentException', expected 'ConcurrentModificationException'"; the expected message is "The application with name: '<name>' has a different create timestamp than the one you provided. Please call DescribeApplication to fetch valid create timestamp."

Cause 7 — kinesisanalyticsv2.studio-notebook-defaults-disable-checkpoints-snapshots-and-autoscaling-unlike-a-flink-app: "the Studio notebook defaults to {'checkpointing': True, 'autoscaling': True, 'snapshots': True}, expected {'checkpointing': False, 'autoscaling': False, 'snapshots': False}".

Cause 8 — kinesisanalyticsv2.studio-notebook-snapshot-and-rollback-rejected-as-unsupported: "CreateApplicationSnapshot on a Studio notebook answered 'InvalidRequestException', expected 'UnsupportedOperationException'" (message expected "This operation is not supported for RuntimeEnvironment ZEPPELIN-FLINK-3_0"); the same check then asserts RollbackApplication -> UnsupportedOperationException / "RollbackApplication is not supported for application with runtime environment ZEPPELIN-FLINK-3_0."

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1, service kinesisanalyticsv2.
2. Cause 1: CreateApplication(ApplicationName=<new>, RuntimeEnvironment=FLINK-1_18, ApplicationMode=INTERACTIVE, ServiceExecutionRole=<assumable role>) — observe 200 instead of 400 InvalidArgumentException, then DescribeApplication the name and observe it exists.
3. Cause 2: the same call with RuntimeEnvironment=ZEPPELIN-FLINK-3_0 and (a) ApplicationMode=STREAMING, (b) ApplicationMode omitted — both return 200.
4. Cause 3: CreateApplication for a ZEPPELIN-FLINK-3_0 / INTERACTIVE notebook with ApplicationCodeConfiguration.CodeContent.TextContent set to a bare SQL paragraph — returns 200.
5. Cause 4: CreateApplication (and UpdateApplication with ServiceExecutionRoleUpdate) naming a role ARN that was never created — returns 200.
6. Cause 5: create an IAM role whose trust policy names lambda.amazonaws.com, then CreateApplication with it as ServiceExecutionRole — returns 200.
7. Cause 6: create a notebook, read its CreateTimestamp with DescribeApplication, then DeleteApplication with that timestamp minus one second — observe code InvalidArgumentException / 400 instead of ConcurrentModificationException / 409 with the stale-timestamp message.
8. Cause 7: create a ZEPPELIN-FLINK-3_0 / INTERACTIVE notebook with no checkpoint, parallelism or snapshot configuration and a FLINK-1_18 / STREAMING app likewise; compare CheckpointingEnabled, AutoScalingEnabled and SnapshotsEnabled in both DescribeApplication responses — the notebook reports true/true/true where false/false/false is required.
9. Cause 8: on that notebook call CreateApplicationSnapshot and RollbackApplication(CurrentApplicationVersionId=1) — observe InvalidRequestException instead of UnsupportedOperationException with the two runtime-specific messages.
