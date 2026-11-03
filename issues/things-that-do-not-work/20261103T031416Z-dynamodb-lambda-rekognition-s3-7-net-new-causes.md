Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip dynamodb__lambda__rekognition__s3.notification-to-a-lambda-whose-permission-names-another-account-rejected-invalid-argument dynamodb__lambda__rekognition__s3.two-rules-with-the-same-suffix-for-one-event-rejected-as-ambiguous dynamodb__lambda__rekognition__s3.three-suffix-filtered-lambda-notification-configs-round-trip dynamodb__lambda__rekognition__s3.same-key-overwrite-invokes-the-notification-target-a-second-time dynamodb__lambda__rekognition__s3.suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered dynamodb__lambda__rekognition__s3.transact-write-items-unused-expression-attribute-name-rejected-whole-and-writes-nothing
Issue-Key: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip|run#11|HTTPError|with urllib.request.urlopen(
Issue-Key: dynamodb__lambda__rekognition__s3.notification-to-a-lambda-whose-permission-names-another-account-rejected-invalid-argument|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: dynamodb__lambda__rekognition__s3.two-rules-with-the-same-suffix-for-one-event-rejected-as-ambiguous|_error#13|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: dynamodb__lambda__rekognition__s3.three-suffix-filtered-lambda-notification-configs-round-trip|run#15|AssertionError|assert rules[0]["Name"] == "Suffix", (
Issue-Key: dynamodb__lambda__rekognition__s3.same-key-overwrite-invokes-the-notification-target-a-second-time|_within_round#14|Timeout|time.sleep(2)
Issue-Key: dynamodb__lambda__rekognition__s3.suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered|_within_round#15|Timeout|time.sleep(2)
Issue-Key: dynamodb__lambda__rekognition__s3.transact-write-items-unused-expression-attribute-name-rejected-whole-and-writes-nothing|run#10|AssertionError|assert got == ("ValidationException",
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# dynamodb__lambda__rekognition__s3: 7 net-new causes — S3 notification destination/overlap validation missing, filter-rule names not re-cased, notifications never delivered, GetFunction Code.Location 404s, the CFN stack rolls back, and a TransactWriteItems message carries an extra prefix

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Seven distinct root causes under the suite heading `dynamodb__lambda__rekognition__s3`. A fixer must close all seven before submitting. (The eighth failure in this service, `bucket-versioning-status-key-absent-until-versioning-is-enabled`, is already filed as blocked by the unimplemented `DetectLabels` stub and is deliberately not part of this issue.)

Cause 1 of 7: the scenario's CloudFormation stack never reaches CREATE_COMPLETE.
AWS: a CreateStack whose template declares the scenario's bucket/table/function resources reaches `CREATE_COMPLETE`, and each declared resource exists afterwards. Recorded behavior only for which resource fails; the recording shows the terminal status.
Emulator: the stack reaches `ROLLBACK_COMPLETE`. Replay detail: `[phase 1/6 deploy] stack <name> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE`.
Why: every later phase of the scenario is unreachable; a client deploying this template gets a rolled-back stack and no resources.
Fix: cloudformation's resource provisioners for the types this template declares (and the stack-event reason that should name the failing resource).
Traces: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle

Cause 2 of 7: GetFunction's `Code.Location` presigned URL is not served — fetching it answers 404.
AWS: Smithy: lambda GetFunction returns `Code.Location`, a presigned URL; an HTTP GET of it returns 200 and the exact bytes of the uploaded deployment package.
Emulator: the GET answers `urllib.error.HTTPError: HTTP Error 404: Not Found`.
Why: a client that downloads a function's package (the documented way to recover deployed code) gets a 404 from a URL the API just handed it.
Fix: the lambda code-store download route that `Code.Location` points at (serve the stored artifact bytes for that presigned path).
Traces: dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip

Cause 3 of 7: PutBucketNotificationConfiguration accepts a Lambda destination whose only invoke grant is scoped to a FOREIGN SourceAccount.
AWS: S3 validates every destination in the configuration before storing it. A function whose only `lambda:AddPermission` statement names `Principal=s3.amazonaws.com`, `SourceArn=arn:aws:s3:::<this-bucket>` but `SourceAccount=<another account>` does not satisfy validation, exactly as if no grant existed: HTTP 400 `InvalidArgument` with message `Unable to validate the following destination configurations` (that whole sentence is fixed text), and nothing is stored.
Emulator: the call SUCCEEDS (200) and the configuration is stored.
Why: a misconfigured grant is accepted at configure time, so the failure surfaces later as silently undelivered events instead of at the API call that AWS refuses.
Fix: s3's PutBucketNotificationConfiguration destination validation — evaluate the destination's resource policy including the SourceAccount condition.
Traces: dynamodb__lambda__rekognition__s3.notification-to-a-lambda-whose-permission-names-another-account-rejected-invalid-argument

Cause 4 of 7: PutBucketNotificationConfiguration accepts two rules sharing a suffix for one event type instead of refusing the ambiguity.
AWS: two `LambdaFunctionConfigurations` whose suffix filters overlap for the same event type are refused with HTTP 400 `InvalidArgument`, message `Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.` (fixed text, no variable parts), and the previously installed set survives untouched because the operation is a full replace.
Emulator: the call SUCCEEDS and replaces the installed set.
Why: S3 refuses ambiguity rather than choosing a rule; an emulator that accepts it lets a client install a configuration whose delivery behaviour is undefined, and silently wipes the configuration that was there.
Fix: s3's notification-configuration validator — the prefix/suffix overlap check per event type.
Traces: dynamodb__lambda__rekognition__s3.two-rules-with-the-same-suffix-for-one-event-rejected-as-ambiguous

Cause 5 of 7: notification filter-rule `Name` is echoed in the case the caller sent instead of S3's canonical capitalisation.
AWS: S3 normalises `FilterRules[].Name` to `Prefix` / `Suffix` whatever case the caller sent, so GetBucketNotificationConfiguration always reports the capitalised form.
Emulator: a configuration sent with `{"Name": "suffix"}` reads back as `'suffix'`.
Why: every caller that matches rule names against `Suffix`/`Prefix` (including the AWS SDK examples and CloudFormation drift readers) behaves differently against the emulator than against AWS.
Fix: s3's notification-configuration state/serializer — canonicalise the filter-rule name on write and emit the capitalised form on read.
Traces: dynamodb__lambda__rekognition__s3.three-suffix-filtered-lambda-notification-configs-round-trip

Cause 6 of 7: an S3 ObjectCreated notification never invokes its Lambda target, so two delivery checks hang to the 60 s harness cap.
AWS: a PutObject matching a stored `LambdaFunctionConfigurations` rule invokes the target function; overwriting the same key invokes it a second time, and a suffix filter is case-sensitive so an uppercase key is NOT delivered (the negative is proved by the positive control delivering).
Emulator: neither check ever observes a delivery; both report `Error: Check timed out after 60.0s`, with no error code from the emulator.
Why: the whole object-driven pipeline is inert, and because the positive control never fires, a client cannot even distinguish "filtered out" from "never delivered".
Fix: s3's object-write notification dispatcher (match stored rules, then invoke the Lambda destination). Related open issue `20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md` covers the same inertness for a different service heading and does not carry these replay keys.
Traces: dynamodb__lambda__rekognition__s3.same-key-overwrite-invokes-the-notification-target-a-second-time, dynamodb__lambda__rekognition__s3.suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered

Cause 7 of 7: TransactWriteItems' unused-ExpressionAttributeNames ValidationException message carries an extra `1 validation error detected: ` prefix AWS does not send.
AWS: an `ExpressionAttributeNames` entry unused by any expression fails the whole TransactWriteItems with HTTP 400 `ValidationException`, message `Value provided in ExpressionAttributeNames unused in expressions: keys: {#unused}` — the fixed text is `Value provided in ExpressionAttributeNames unused in expressions: keys: {` + the offending names from the request + `}`; there is no `N validation error detected:` prefix on this one.
Emulator: same code, message `1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#unused}`.
Why: a client matching AWS's exact message (or stripping a fixed prefix) misclassifies the error; the prefix is applied where AWS applies none.
Fix: the dynamodb TransactWriteItems expression validator's error formatting — do not wrap this refusal in the validation-error-count envelope.
Traces: dynamodb__lambda__rekognition__s3.transact-write-items-unused-expression-attribute-name-rejected-whole-and-writes-nothing

## Evidence
Cause 1 — expected stack status CREATE_COMPLETE; actual ROLLBACK_COMPLETE.
Cause 2 — expected HTTP 200 and the exact uploaded zip bytes from `GetFunction().Code.Location`; actual HTTP 404 Not Found.
Cause 3 — expected ('InvalidArgument', 'Unable to validate the following destination configurations') and nothing stored; actual: the call succeeded (no error raised) and the configuration was stored.
Cause 4 — expected ('InvalidArgument', 'Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.') with the prior set intact; actual: the call succeeded.
Cause 5 — expected `FilterRules[0].Name == 'Suffix'`; actual `'suffix'` (the case the caller sent).
Cause 6 — expected a second invocation on same-key overwrite, and no invocation for an uppercase-suffix key with the positive control delivering; actual: `Error: Check timed out after 60.0s` on both checks — no delivery and no error.
Cause 7 — expected ('ValidationException', 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#unused}'); actual ('ValidationException', '1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#unused}').

## Steps to Reproduce
1. Replay the behavioral suite and re-run every check id in trace_ids.
2. Cause 3: AddPermission on a function with Principal=s3.amazonaws.com, SourceArn=arn:aws:s3:::<bucket>, SourceAccount=111122223333, then PutBucketNotificationConfiguration naming that function for the same bucket.
3. Cause 4: install three LambdaFunctionConfigurations with distinct suffixes (accepted), then PUT two configurations both filtering suffix '.jpg' for s3:ObjectCreated:*.
4. Cause 5: PUT a configuration with `FilterRules=[{'Name':'suffix','Value':'.jpg'}]` and read it back with GetBucketNotificationConfiguration.
5. Cause 6: with a granted configuration installed, PutObject a matching key twice and then an uppercase-suffix key; watch for invocations.
6. Cause 7: `TransactWriteItems` with two Put members where one carries `ExpressionAttributeNames={'#unused':'labels'}` not referenced by its ConditionExpression.
7. Cause 2: CreateFunction from a ZipFile, GetFunction, and HTTP GET `Code.Location`.
8. Cause 1: deploy the scenario template and read the stack status and events.
