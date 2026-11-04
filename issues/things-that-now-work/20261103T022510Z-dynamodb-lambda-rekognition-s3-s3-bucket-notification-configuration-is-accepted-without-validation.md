Issue-Severity: HIGH
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# S3 bucket notification configuration is accepted without validation or name normalisation, notifications are never delivered, GetFunction's code-location URL 404s, and the Rekognition stack rolls back (7 causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Seven net-new root causes. The open issue 20261028T214304Z-dynamodb-lambda-rekognition-s3-the-rekognition-s3-trigger.md describes the same stack rollback symptom as Cause 7 but the replay reports that check as unowned, so it is carried here; 20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md covers a neighbouring set of S3-notification checks under a different service heading and carries none of the keys below.

=== Cause 1 of 7: a notification target whose Lambda permission names another account is accepted ===
AWS: PutBucketNotificationConfiguration validates that the bucket can actually invoke the named LambdaFunctionArn; if the function's resource policy grants s3.amazonaws.com under a SourceAccount other than the bucket owner's, the call is refused with HTTP 400 and error code InvalidArgument (recorded behavior: the refusal carries the InvalidArgument code and names the unverifiable destination).
Emulator: the call succeeds, so a bucket is wired to a target it can never invoke.
Why: the misconfiguration is silent — the client believes the notification is wired and only discovers nothing is delivered in production.
Fix: destination-permission validation in the PutBucketNotificationConfiguration handler.
Traces: notification-to-a-lambda-whose-permission-names-another-account-rejected-invalid-argument.

=== Cause 2 of 7: two rules carrying the same suffix filter for one event type are accepted ===
AWS: overlapping notification configurations for the same event and the same filter are ambiguous and refused with HTTP 400 InvalidArgument.
Emulator: the call succeeds, leaving two rules that both match the same object key for the same event.
Why: delivery becomes nondeterministic in the emulator and the configuration is rejected when the same template is applied to real AWS.
Fix: the overlap/ambiguity check in the PutBucketNotificationConfiguration handler's validation.
Traces: two-rules-with-the-same-suffix-for-one-event-rejected-as-ambiguous.

=== Cause 3 of 7: FilterRule Name is not normalised to its canonical capitalised form ===
AWS: S3 accepts a FilterRule Name in any case and normalises it on read to the canonical capitalised token — "Suffix" or "Prefix" — so GetBucketNotificationConfiguration always reports the capitalised form whatever the caller sent.
Emulator: the value is echoed verbatim, so a configuration written with 'suffix' reads back as 'suffix'.
Why: a client comparing the live configuration with its desired state (every IaC provider) sees permanent drift and rewrites the configuration on every run.
Fix: normalise FilterRule.Name when storing, or in the GetBucketNotificationConfiguration serializer, in the S3 notification-configuration state.
Traces: three-suffix-filtered-lambda-notification-configs-round-trip.

=== Cause 4 of 7: object-created notifications are never delivered to the Lambda target ===
AWS: a PutObject matching a configured s3:ObjectCreated:* rule invokes the configured Lambda once per matching event; overwriting the same key delivers a second event; and the suffix filter is case-sensitive so an uppercase key is NOT delivered.
Emulator: nothing is delivered, so both the second-delivery assertion and the case-sensitivity negative control hang to the 60 s harness cap.
Why: the whole S3-event-driven pattern is unobservable; neither the positive nor the negative behaviour can be developed or tested.
Fix: the S3 notification dispatch path that turns a stored object event into a Lambda invocation.
Traces: same-key-overwrite-invokes-the-notification-target-a-second-time, suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered.

=== Cause 5 of 7: GetFunction's CodeLocation presigned URL serves HTTP 404 ===
AWS: GetFunction returns Code.Location, a presigned URL that an unauthenticated GET resolves to the exact deployment package that was uploaded (same bytes, same SHA256).
Emulator: fetching the returned location answers HTTP 404 Not Found, so the uploaded ZIP cannot be retrieved.
Why: any tool that downloads a function's code (deployment diffing, drift detection, `aws lambda get-function` followed by a fetch) fails.
Fix: the Lambda GetFunction handler's code-location URL and the object it must serve from the function's code store.
Traces: get-function-code-location-serves-the-exact-uploaded-zip.

=== Cause 6 of 7: the TransactWriteItems unused-expression-name refusal is prefixed where AWS does not prefix ===
AWS: a TransactWriteItems action carrying an ExpressionAttributeNames entry that no expression references is refused with ValidationException before any condition is evaluated, and the message is the bare sentence "Value provided in ExpressionAttributeNames unused in expressions: keys: {<name>}" with the unreferenced placeholder substituted.
Emulator: the sentence is prefixed with "1 validation error detected: ".
Why: a client matching AWS's wording fails, and the prefix wrongly implies the constraint-violation renderer rather than the expression validator produced the refusal.
Fix: the TransactWriteItems per-action expression validation in the DynamoDB transaction handler.
Traces: transact-write-items-unused-expression-attribute-name-rejected-whole-and-writes-nothing.

=== Cause 7 of 7: the Rekognition/S3-trigger CloudFormation stack ends ROLLBACK_COMPLETE ===
AWS: the template deploys to CREATE_COMPLETE, after which the rest of the scenario runs.
Emulator: the stack reaches ROLLBACK_COMPLETE, so every later phase of the lifecycle is unreachable.
Why: the whole scenario cannot run; a user following the same template gets a failed stack.
Fix: the CloudFormation resource providers the template declares (the failing resource is named in the stack's events).
Traces: e2e-diagnose-rekognition-reupload-stale-lifecycle.

## Evidence
Cause 1 — notification-to-a-lambda-whose-permission-names-another-account-rejected-invalid-argument: AssertionError: the call succeeded; real AWS rejects it, so there is no error code.
Cause 2 — two-rules-with-the-same-suffix-for-one-event-rejected-as-ambiguous: AssertionError: the call succeeded; real AWS rejects it, so there is no error code.
Cause 3 — three-suffix-filtered-lambda-notification-configs-round-trip: AssertionError: S3 normalises the filter-rule name to the capitalised 'Suffix' whatever case the caller sent; config 'sim2real-cfg-jpg' reports 'suffix'.
Cause 4 — same-key-overwrite-invokes-the-notification-target-a-second-time and suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered: Check timed out after 60.0s.
Cause 5 — get-function-code-location-serves-the-exact-uploaded-zip: urllib.error.HTTPError: HTTP Error 404: Not Found (raised fetching the Code.Location URL GetFunction returned).
Cause 6 — transact-write-items-unused-expression-attribute-name-rejected-whole-and-writes-nothing: AssertionError: an unused attribute name should fail the request before any condition is evaluated, got ('ValidationException', '1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#unused}').
Cause 7 — e2e-diagnose-rekognition-reupload-stale-lifecycle: AssertionError: [phase 1/6 deploy] stack <name> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events for the resource that failed.

## Steps to Reproduce
Replay the behavioral suite and re-run the dynamodb__lambda__rekognition__s3 checks in trace_ids. Minimal reproductions: (1) create a function, add_permission for s3.amazonaws.com with SourceAccount set to a different account, then s3.put_bucket_notification_configuration naming that function — expect HTTP 400 InvalidArgument, observe success; (2) put_bucket_notification_configuration with two LambdaFunctionConfigurations carrying the same {'Name':'suffix','Value':'.jpg'} filter for s3:ObjectCreated:* — expect InvalidArgument, observe success; (3) put_bucket_notification_configuration with FilterRule Name 'suffix', then get_bucket_notification_configuration and read the Name back; (4) put_object on a matching key twice and poll the target for two invocations; (5) lambda.get_function(FunctionName=fn) then GET Code.Location.
