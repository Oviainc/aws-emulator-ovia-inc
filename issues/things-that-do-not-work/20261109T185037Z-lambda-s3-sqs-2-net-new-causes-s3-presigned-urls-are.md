Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.lambda-sdk-s3-generate-presigned-url-positive lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative lambda__s3__sqs.dotnet-s3-notification-records-reach-handler-via-onsuccess-destination lambda__s3__sqs.java-s3-notification-record-fields-reach-the-handler lambda__s3__sqs.ruby-s3-object-created-notification-record-reaches-the-handler
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-generate-presigned-url-positive|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(unauthorized_url)), p
Issue-Key: lambda__s3__sqs.dotnet-s3-notification-records-reach-handler-via-onsuccess-destination|_await_messages#13|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Key: lambda__s3__sqs.java-s3-notification-record-fields-reach-the-handler|run#19|TimeoutError|raise TimeoutError(
Issue-Key: lambda__s3__sqs.ruby-s3-object-created-notification-record-reaches-the-handler|_poll#14|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 2 net-new causes: S3 presigned URLs are published on the real s3.<region>.amazonaws.com host, and dotnet/java/ruby handlers never run

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Two distinct root causes in the lambda__s3__sqs suite. Each is listed with its own failing check ids; both must be closed.

Cause 1 of 2 — a presigned S3 URL is generated against the real AWS host, so it is unreachable from the client that generated it. AWS: a presigned URL is a signed request against the endpoint the SDK is configured with; it must resolve to the service that holds the object, and GETting it returns the object (or 403 when the signing principal lacks permission). Emulator: the published URL is https://s3.us-west-2.amazonaws.com/<bucket>/<key>?x-id=GetObject&X-Amz-Algorithm=AWS4-HMAC-SHA256&... — 'its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>'. An emulator that announces an address other than the one it is reached on breaks the URL for every consumer. Why: presigned URLs are handed to third parties (browsers, partners); none of them can fetch the object, and the negative case (missing permission) cannot be observed at all. Fix: endpoint/host resolution used when signing presigned URLs — announce the emulator's own host:port (the same defect shape as the CloudFormation custom-resource ResponseURL). Traces: lambda__s3__sqs.lambda-sdk-s3-generate-presigned-url-positive, lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative.

Cause 2 of 2 — functions on the .NET, Java and Ruby runtimes are accepted but never executed, so S3 notification records never reach the handler. AWS: Lambda runs every managed runtime it accepts; an S3 ObjectCreated notification reaches a dotnet8/java21/ruby3.x handler with the full Records entry (bucket, key, size, eTag, sequencer) and the handler's OnSuccess destination record follows. Emulator: all three checks end with 'the run budget ended first; not graded' while the emulator states elsewhere in this run that it 'runs the Python and node runtimes'; CreateFunction accepts the runtime regardless. Why: the function reports Active and the notification is configured, yet nothing runs — no invocation, no error, no log. An unsupported runtime must be refused at CreateFunction with Lambda's InvalidParameterValueException instead of accepted and ignored. Fix: the Lambda runtime-execution component. Traces: lambda__s3__sqs.dotnet-s3-notification-records-reach-handler-via-onsuccess-destination, lambda__s3__sqs.java-s3-notification-record-fields-reach-the-handler, lambda__s3__sqs.ruby-s3-object-created-notification-record-reaches-the-handler.

## Evidence
Cause 1: AssertionError: the emulator published https://s3.us-west-2.amazonaws.com/sim2real-behav-presign-.../presigned/object.bin?x-id=GetObject&X-Amz-Algorithm=AWS4-HMAC-SHA256&...: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:45889/<resource path> (same for the missing-permission check)
Cause 2: TimeoutError: the S3-triggered invoke's OnSuccess destination record: the run budget ended first; not graded; TimeoutError: waiting for a PutObject to reach the Java handler: the run budget ended first; not graded; TimeoutError: the handler relays the Records entry it received: the run budget ended first; not graded

## Steps to Reproduce
Cause 1: from a Lambda (or any client), generate_presigned_url('get_object', ...) against the emulator and GET the result — the host is s3.<region>.amazonaws.com.
Cause 2: CreateFunction with Runtime dotnet8 / java21 / ruby3.x, configure an S3 ObjectCreated notification to it, PutObject, and wait for the handler's effect.
