Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative|_poll_until#34|AssertionError|raise AssertionError(
Issue-Commit: c1544a9590ded4504bea161b6ee6981cbcc184a7

# cloudformation__iam__lambda__s3: a corrected bucket→Lambda notification still never runs the function, so the object the handler writes never appears (1 cause)

> _Note: this issue was identified against the code base at commit `c1544a9590ded4504bea161b6ee6981cbcc184a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: an S3 bucket notification whose Lambda destination IS validated and persisted never actually runs the function, so the side effect the handler performs is never observable.

This is a net-new failure point for this check. Its earlier failure (PutBucketNotificationConfiguration accepting a wiring AWS refuses) no longer occurs: the check's first phase now passes — the put with an invoke permission scoped to a FOREIGN bucket ARN is refused with InvalidArgument and nothing is persisted, and after the permission is corrected to name this bucket the identical put is accepted and GetBucketNotificationConfiguration reports exactly one LambdaFunctionConfigurations entry. The check now fails one step later, on the delivery itself. Two open issues describe the same symptom in other groups but do NOT carry this check's replay key, so they do not cover it: 20261029T192356Z-cloudformation-iam-lambda-s3-8-net-new-root.md (its Cause 6 is keyed to s3-notification-triggers-lambda-role-tags-object-positive and s3-notification-lambda-role-missing-put-tagging-negative, and its Cause 7 is keyed to this check's now-passing refusal assertion) and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md. Neither is a resolved issue, so nothing here is a reopen.

AWS: once a bucket's NotificationConfiguration names a Lambda function for s3:ObjectCreated:* under a key-name prefix Filter, and the function's resource policy grants lambda:InvokeFunction to s3.amazonaws.com for that bucket, a PutObject of a key matching the filter asynchronously invokes that function with the Records envelope for the event. Smithy (lambda) Invoke$InvocationType, value `Event`: "Invoke the function asynchronously. Send events that fail multiple times to the function's dead-letter queue (if one is configured). The API response only includes a status code." The function's own code therefore runs, and any AWS call it makes (here an s3:PutObject of a marker object under marker/) takes effect within seconds. The marker key and body are the handler's, not the service's: the requirement is that the handler ran at all.

Emulator: nothing runs. The check polls GetObject for the marker key for the whole ~53 s budget and every poll answers 404 NoSuchKey (the probe maps that to None), so the harness fails with `AssertionError: invocation marker after the permission was corrected not met within 52.6589324869999s. Last result: None`. The accepted-and-persisted notification configuration is therefore inert: the only way a client can tell a delivered-but-failed invocation from a never-delivered one is by an effect that never comes. (Recorded behavior only for the delivery itself — no response diverges; the divergence is the missing asynchronous effect.)

Why: every S3-driven pipeline built on a Lambda notification target is dead while reporting healthy — the put succeeds, the configuration reads back correctly, and no event ever reaches the function. A client that waits on the handler's side effect (the usual S3→Lambda idiom) hangs until its own timeout, and a NEGATIVE control that intends to observe a *denied* invocation cannot distinguish 'the handler was refused' from 'the handler was never invoked', so the permission defect it is probing is unobservable in either direction.

Fix: the Lambda Invoke handler's execution path — an InvocationType=Event invocation must queue and run the function's deployment package instead of refusing it, so that S3's notification dispatch (which already resolves a LambdaFunction destination and calls Invoke with InvocationType=Event) produces the handler's effects. If the dispatch itself is dropping the delivery, the S3 notification-delivery path owns it instead; the reported hypothesis below distinguishes the two.

Hypothesis: S3's dispatch is present and reaches Lambda — the emulator's S3 crate resolves a LambdaFunction destination and invokes it with InvocationType=Event, logging but swallowing a delivery error (best-effort delivery) — while Lambda's Invoke refuses every real invocation with NotImplemented, so the swallowed error is what makes the configuration inert. Nothing in the replay output states which of the two it is; only the missing effect is recorded.

Traces: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative. Code citations for the dispatch path: services/service-com-amazonaws-s3-amazons3/src/ops/put_object.rs lines 883-900 and 955-956 (the pending notifications a put owes, published after the store is released); services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs lines 501-505 (a LambdaFunction destination becomes a Function delivery) and 836-851 (the delivery calls Invoke with InvocationType=Event and only reports a failure).

## Evidence
Check: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative — verdict: mismatch.

Passing prefix (so these are NOT the divergence): PutBucketNotificationConfiguration with an invoke permission scoped to a foreign bucket ARN was refused with Error.Code 'InvalidArgument' and Error.Message 'Unable to validate the following destination configurations' and persisted no LambdaFunctionConfigurations; after RemovePermission/AddPermission re-scoped SourceArn to this bucket, the identical put was accepted and GetBucketNotificationConfiguration returned exactly 1 LambdaFunctionConfiguration.

Diverging step — expected: after `PutObject(Bucket=<bucket>, Key='trigger/hello.txt')`, the function invoked by the s3:ObjectCreated:* notification writes an object at the key it derives ('marker/hello.txt.done') whose body is 'invoked', so GetObject on that key answers 200 within the poll budget.
Actual: GetObject on that key answers 404 NoSuchKey for the full budget; the harness raises
  File "check_cloudformation__iam__lambda__s3_s3_notification_cross_account_bucket_lambda_role_negative.py", line 188, in _wait_for_effect / line 178, in _poll_until
  AssertionError: invocation marker after the permission was corrected not met within 52.6589324869999s. Last result: None
The bucket name, function name, trigger key and marker key are per-run values from the check's own request; the requirement is that the configured function runs at all and its writes take effect.

## Steps to Reproduce
1. Create a bucket and an IAM role trusted by lambda.amazonaws.com with s3:PutObject on <bucket>/*.
2. Create a python3.12 function with that role whose handler reads event['Records'][*]['s3'] and calls s3:PutObject to write 'marker/<basename>.done' with body b'invoked'; wait for State=Active.
3. lambda:AddPermission(StatementId='s3invoke-foreign', Action='lambda:InvokeFunction', Principal='s3.amazonaws.com', SourceArn=arn:aws:s3:::<some-other-bucket>, SourceAccount=<account>).
4. s3:PutBucketNotificationConfiguration with one LambdaFunctionConfigurations entry: that function's ARN, Events=['s3:ObjectCreated:*'], Filter prefix 'trigger/'. Expect InvalidArgument 'Unable to validate the following destination configurations' and no persisted configuration (this currently holds).
5. lambda:RemovePermission the foreign statement and AddPermission(StatementId='s3invoke-correct', ...) with SourceArn=arn:aws:s3:::<bucket>. Retry the identical put until accepted; GetBucketNotificationConfiguration must report 1 entry (this currently holds).
6. s3:PutObject(Key='trigger/hello.txt'). Poll s3:GetObject for 'marker/hello.txt.done'. AWS: it appears with body 'invoked'. Emulator: NoSuchKey until the poll budget expires.
