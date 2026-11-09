Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec s3.directory-bucket-multipart-complete-etag-is-not-an-md5-composite s3.bucket-replication-destination-validation-rejected s3.create-bucket-reserved-prefix-or-suffix-rejected s3.notification-fifo-destination-rejected s3.notification-sqs-test-event-format s3.s3-lcp-rejects-inverted-or-equal-object-size-range s3.s3x-s3express-session-and-listing-expired-token-denied
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec|_error#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: s3.directory-bucket-multipart-complete-etag-is-not-an-md5-composite|run#11|AssertionError|assert dir_etag != want_twin_etag, (
Issue-Key: s3.bucket-replication-destination-validation-rejected|_error#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: s3.create-bucket-reserved-prefix-or-suffix-rejected|_assert_reserved_prefix#10|AssertionError|assert code == "AccessDenied", f"{reason}: expected AccessDenied, got {code!r} ({message!r})"
Issue-Key: s3.notification-fifo-destination-rejected|run#13|AssertionError|assert message == "FIFO SQS queues are not supported.", message
Issue-Key: s3.notification-sqs-test-event-format|run#11|AssertionError|assert _TIME_RE.match(body["Time"]), f"expected an ISO-8601 millisecond UTC Time, got {body['Time']!r}"
Issue-Key: s3.s3-lcp-rejects-inverted-or-equal-object-size-range|run#10|AssertionError|assert message == expected_message, f"inverted size range message {message!r} != {expected_message!r}"
Issue-Key: s3.s3x-s3express-session-and-listing-expired-token-denied|run#16|AssertionError|assert refused == ("AccessDenied", 403, "Access Denied"), (
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# s3: 7 more net-new causes — directory-bucket restrictions unenforced and its MPU ETag is an MD5 composite, PutBucketReplication accepts an invalid destination, reserved bucket prefixes get the wrong error, and four refusal messages/codes diverge

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketReplication

## Description
Seven causes.

Cause 1 of 7: S3 Express One Zone (directory bucket) restrictions are not enforced and its multipart ETag is computed like a general-purpose bucket's.
AWS: directory buckets do not support object tagging, versioning, bucket notifications or replication — PutObjectTagging/PutBucketVersioning/PutBucketNotificationConfiguration/PutBucketReplication against a directory bucket are refused (NotImplemented/InvalidRequest class refusals), a CopyObject into a directory bucket with a source tagging directive or a destination SSE-C header is refused, and a directory bucket's CompleteMultipartUpload ETag is NOT the md5(md5(part1)||md5(part2))-N composite that general-purpose buckets return (Smithy: s3 CompleteMultipartUpload output ETag; CopyObject models TaggingDirective and the SSECustomer* headers).
Emulator: all of those calls succeed — the unsupported configuration calls return 200/204, the CopyObject returns 200 with ServerSideEncryption AES256, and the directory bucket's completed ETag equals the general-purpose MD5 composite ('"4c3969d0421775aa0e47fb78391fbaa9-2"' for both buckets over identical bytes).
Why: code written against S3 Express appears to work in the emulator and then fails in AWS; conversely any client that keys on the ETag's form to tell a directory bucket from a general-purpose one is misled.
Fix: the directory-bucket branch of the bucket/object handlers must refuse the unsupported operations and the MPU completion must use the directory-bucket ETag scheme rather than the MD5 composite.
Traces: s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication, s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec, s3.directory-bucket-multipart-complete-etag-is-not-an-md5-composite.

Cause 2 of 7: PutBucketReplication accepts a replication configuration whose destination is invalid.
AWS: PutBucketReplication validates the configuration — a destination bucket that does not exist / is not versioned / is otherwise invalid is refused (InvalidRequest-class client error naming the destination problem). Smithy: s3 PutBucketReplication requires ReplicationConfiguration (Role + Rules[].Destination).
Emulator: the call succeeds and returns None (HTTP 200) for the configuration the check expects to be refused.
Why: a replication rule that can never replicate is stored as valid, so the operator believes cross-region replication is configured when nothing will ever be copied.
Fix: add destination validation to the PutBucketReplication handler before the configuration is stored.
Traces: s3.bucket-replication-destination-validation-rejected.

Cause 3 of 7: a bucket name using a reserved prefix is refused with the wrong error code and message.
AWS: CreateBucket for a name using a reserved prefix such as 'sthree-' is refused 403 AccessDenied (the name is reserved to AWS, not syntactically invalid). Recorded behavior: the check asserts the code AccessDenied for each reserved prefix/suffix.
Emulator: the refusal is 'InvalidBucketName' with message 'The specified bucket is not valid.'.
Why: a caller distinguishing 'name is malformed, fix it' from 'name is reserved, you may not have it' branches wrongly, and retry logic keyed on AccessDenied vs InvalidBucketName takes the wrong path.
Fix: the CreateBucket name validation must classify reserved prefixes/suffixes as AccessDenied rather than folding them into the invalid-name check.
Traces: s3.create-bucket-reserved-prefix-or-suffix-rejected.

Cause 4 of 7: the PutBucketNotificationConfiguration refusal for a FIFO queue destination carries the wrong message.
AWS: a notification configuration whose QueueArn names a FIFO queue is refused InvalidArgument with the message 'FIFO SQS queues are not supported.' (fixed text). Smithy: s3 PutBucketNotificationConfiguration takes NotificationConfiguration with QueueConfigurations[].QueueArn and SkipDestinationValidation.
Emulator: the message is 'Unable to validate the following destination configurations' — the generic destination-validation text, which never names the FIFO restriction.
Why: the caller is told validation failed but not that FIFO queues are categorically unsupported, so the fix (use a standard queue) is not discoverable from the error.
Fix: the notification destination validation must emit the FIFO-specific InvalidArgument message for a FIFO QueueArn.
Traces: s3.notification-fifo-destination-rejected.

Cause 5 of 7: the s3:TestEvent notification's Time is not an ISO-8601 millisecond UTC timestamp.
AWS: the test event S3 delivers when a notification configuration is created carries Time as an ISO-8601 UTC timestamp with exactly three fractional-second digits (e.g. '2026-11-09T21:17:05.007Z'). The instant is per-run; the format is the requirement.
Emulator: Time is '2026-11-09T21:17:05.00719Z' — five fractional digits, so it does not match the documented millisecond form.
Why: consumers that parse the envelope with a strict millisecond format (the Java/Go SDK event models, log pipelines) fail to parse the event at all.
Fix: the S3 notification event serializer must format Time with millisecond precision.
Traces: s3.notification-sqs-test-event-format.

Cause 6 of 7: the lifecycle inverted object-size-range refusal carries the wrong message.
AWS: a lifecycle rule whose filter has ObjectSizeGreaterThan >= ObjectSizeLessThan is refused InvalidArgument with the message "'ObjectSizeLessThan' has to be a value greater than 'ObjectSizeGreaterThan'." (fixed text).
Emulator: the message is 'Rule 1: ObjectSizeGreaterThan 1000 is not below ObjectSizeLessThan 100, so no object size matches the filter' — the emulator's own prose, with the rule index and the two sizes taken from the request.
Why: a caller matching AWS's documented message (or a user searching for it) does not recognise the refusal; the error text is part of the API contract.
Fix: the PutBucketLifecycleConfiguration validation must emit AWS's exact InvalidArgument message for an inverted or equal size range.
Traces: s3.s3-lcp-rejects-inverted-or-equal-object-size-range.

Cause 7 of 7: a GetObject with an expired S3 Express session token is refused ExpiredToken/400 instead of AccessDenied/403.
AWS: an S3 Express request presenting an expired session token (x-amz-s3session-token) is refused HTTP 403 AccessDenied with message 'Access Denied'.
Emulator: the refusal is HTTP 400 ExpiredToken with 'The provided token has expired.'.
Why: SDK retry logic differentiates a 403 AccessDenied (re-create the session via CreateSession) from a 400 ExpiredToken (a credential-level error); the wrong status and code send clients down the wrong recovery path.
Fix: the S3 Express session-token verifier must map an expired session token to the AccessDenied/403 refusal S3 returns.
Traces: s3.s3x-s3express-session-and-listing-expired-token-denied.

## Evidence
Cause 1: AssertionError: expected the call to be refused, but it succeeded: {... 'HTTPStatusCode': 200 ...} (directory-bucket tagging/versioning/notification/replication); expected the call to be refused, but it succeeded: {... 200, 'ServerSideEncryption': 'AES256', 'CopyObjectResult': {...}} (CopyObject with source tags + destination SSE-C); the directory bucket's multipart ETag '"4c3969d0421775aa0e47fb78391fbaa9-2"' equals md5(md5(p1)||md5(p2))-2 for the identical bytes; want it to differ.
Cause 2: AssertionError: expected the call to be rejected, but it succeeded and returned None (PutBucketReplication).
Cause 3: AssertionError: an sthree- prefix: expected AccessDenied, got 'InvalidBucketName' ('The specified bucket is not valid.').
Cause 4: AssertionError: expected message 'FIFO SQS queues are not supported.', got 'Unable to validate the following destination configurations'.
Cause 5: AssertionError: expected an ISO-8601 millisecond UTC Time, got '2026-11-09T21:17:05.00719Z'.
Cause 6: AssertionError: inverted size range message 'Rule 1: ObjectSizeGreaterThan 1000 is not below ObjectSizeLessThan 100, so no object size matches the filter' != "'ObjectSizeLessThan' has to be a value greater than 'ObjectSizeGreaterThan'."
Cause 7: AssertionError: GetObject with the expired session token returned ('ExpiredToken', 400, 'The provided token has expired.'), expected ('AccessDenied', 403, 'Access Denied').

## Steps to Reproduce
Cause 1: create a directory bucket; call put_object_tagging / put_bucket_versioning / put_bucket_notification_configuration / put_bucket_replication on it, and copy_object into it with TaggingDirective='COPY' plus destination SSE-C headers -> all succeed; complete a 2-part MPU in a directory bucket and compare the ETag with the md5-composite of a general-purpose twin.
Cause 2: put_bucket_replication with a Rules[].Destination that AWS rejects -> succeeds.
Cause 3: create_bucket(Bucket='sthree-...') -> InvalidBucketName instead of AccessDenied.
Cause 4: put_bucket_notification_configuration with a QueueConfiguration whose QueueArn is a FIFO queue -> wrong message.
Cause 5: create a notification configuration and read the delivered s3:TestEvent's Time field.
Cause 6: put_bucket_lifecycle_configuration with a filter where ObjectSizeGreaterThan=1000 and ObjectSizeLessThan=100 -> wrong InvalidArgument message.
Cause 7: create_session on a directory bucket, let the token expire, then get_object with it -> ExpiredToken/400 instead of AccessDenied/403.
