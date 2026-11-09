Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-lambda-wrapper-config-update-replaces-env-leaves-code-untouched
Issue-Key: batch__ecr__lambda__logs.refarch-batch-lambda-wrapper-config-update-replaces-env-leaves-code-untouched|_fetch_source#27|PublishedUrlRejected|target, route = ctx.endpoint_route(location)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# batch__ecr__lambda__logs: Lambda publishes the deployed code's location on a real AWS S3 host (awslambda-<region>-tasks.s3.<region>.amazonaws.com), so a config-only update cannot be verified byte-identical

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
Cause 1 of 1: the function code location is announced on a host the emulator does not serve.
AWS: GetFunction returns Code.Location — a presigned URL a client can GET to download the deployed package. Smithy (Lambda): FunctionCodeLocation$Location is 'a presigned URL that you can use to download the deployment package'; the URL is served by the account/region's Lambda task bucket and is reachable by the caller.
Emulator: the published location is 'https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<accountId>/<function>-<ids>' — a real-AWS hostname that is not one of the emulator's own hosts, so the announced address is refused (a customer of the emulator needs http(s)://<own host>:<port>/<path>). The GET that would compare the deployed bytes before and after an UpdateFunctionConfiguration therefore never reaches the emulator.
Why: a client that follows the address the API handed it is sent to AWS's S3 DNS name instead of the emulator; downloading the deployed package is impossible, so nothing can verify that a config-only update left the code untouched (nor can any tooling that round-trips the package work at all).
Fix: the Lambda GetFunction/FunctionCodeLocation serializer (publish Code.Location on the emulator's own listener host and port). Same root cause as the open issue 20261109T124205Z-dynamodb-lambda-rekognition-s3-lambda-getfunction-publishes-code.md, which is filed under a different service and does not carry this check's replay key.
Traces: batch__ecr__lambda__logs.refarch-batch-lambda-wrapper-config-update-replaces-env-leaves-code-untouched.

## Evidence
simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com/snapshots/000000000000/<function>-<uuid> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Expected: GetFunction's Code.Location is a URL on a host:port the emulator serves, and a GET of it returns the deployment package bytes (identical before and after an UpdateFunctionConfiguration that only replaces Environment.Variables).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay batch__ecr__lambda__logs.refarch-batch-lambda-wrapper-config-update-replaces-env-leaves-code-untouched.
3. Or: CreateFunction with a zip package, call GetFunction and GET the returned Code.Location — the host is awslambda-<region>-tasks.s3.<region>.amazonaws.com, which the emulator does not serve.
