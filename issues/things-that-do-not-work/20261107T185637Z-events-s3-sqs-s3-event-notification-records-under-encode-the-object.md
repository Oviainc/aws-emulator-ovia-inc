Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers
Issue-Key: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers|run#31|AssertionError|assert native.native_for(_KEY_NATIVE), (
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# S3 event-notification records under-encode the object key: '~' is left literal instead of percent-encoded

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261029T193114Z-s3-sqs-4-net-new-causes-object-notifications.md owns other checks of this service and does not carry this replay key.

Cause 1 of 1: the s3.object.key field of a native S3 -> SQS notification record is not fully URL-encoded.
AWS: in an S3 event notification record, s3.object.key is the object key URL-encoded — space becomes '+', and every character outside the unreserved set is percent-encoded, including the tilde, which S3 emits as %7E. For a key containing '+', '=', a multi-byte character and '~', the record's key is the fully encoded form (e.g. the '~' appears as %7E, not as a literal '~'). The encoding rule is the requirement; the particular key in the recording only instantiates it.
Emulator: the record delivers the key with '~' left literal — the check saw 'dual+path/report+2026%2Bq3%3D%C3%A9~.txt' where the required form is 'dual+path/report+2026%2Bq3%3D%C3%A9%7E.txt'. Every other character in that key was encoded correctly, so the encoder's unreserved-character set wrongly includes '~'. The check polled 120 s for the correctly encoded record and reported "no native record for '<encoded key>' reached queue N within 120s".
Why: a consumer that URL-decodes s3.object.key (what AWS documents and every SDK example does) gets a different byte sequence than the key S3 stored, so GetObject on the decoded key fails for exactly the keys that contain a tilde — a silent, data-dependent breakage of every notification consumer.
Fix: the S3 notification record serializer's key encoder (the percent-encoding character set used for s3.object.key).
Traces: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers.

## Evidence
Expected s3.object.key 'dual+path/report+2026%2Bq3%3D%C3%A9%7E.txt'; the delivered record carried 'dual+path/report+2026%2Bq3%3D%C3%A9~.txt' — the '~' was not percent-encoded as %7E. ('AssertionError: no native record for ... reached queue N within 120s; seen: {"records": ["dual+path/report+2026%2Bq3%3D%C3%A9~.txt", ...]}')

## Steps to Reproduce
1) PutBucketNotificationConfiguration with a QueueConfiguration for s3:ObjectCreated:* on a queue.
2) PutObject with a key containing '+', '=', a non-ASCII character and '~'.
3) ReceiveMessage and compare Records[0].s3.object.key against the fully percent-encoded key.
