Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip
Issue-Key: dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip|run#10|PublishedUrlRejected|code_url, route_headers = ctx.endpoint_route(location)
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# dynamodb__lambda__rekognition__s3: Lambda GetFunction publishes Code.Location on a real AWS S3 host (awslambda-<region>-tasks.s3.<region>.amazonaws.com) the emulator does not serve, so the deployment package can never be downloaded

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
Cause 1 of 1: GetFunction's presigned code-download URL is announced on an address other than the one the emulator is reached on.
  AWS: GetFunction returns Code (FunctionCodeLocation) whose Location member is documented in the model as "A presigned URL that you can use to download the deployment package" (Smithy: lambda GetFunction output member Code -> FunctionCodeLocation.Location, with RepositoryType). On AWS that URL is served by the account's region-local Lambda task bucket and an unauthenticated HTTPS GET of it returns exactly the bytes that were uploaded as the function's zip — the uploaded package's bytes and its SHA-256 are byte-identical to what the URL serves. The requirement is that the published URL is fetchable by the caller; the host is whatever the serving endpoint is.
  Emulator: GetFunction answers 200, but Code.Location is a URL on the real-AWS host `awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com` (path /snapshots/<account>/<function-name>-<uuid>). That is not one of the emulator's own hosts, so no client of the emulator can resolve/reach it; the harness refuses the value with `simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com/snapshots/000000000000/… (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The specific host, port and object path are per-run values; the defect is that the announced host is a real-AWS endpoint instead of the emulator's serving endpoint.
  Why: every client workflow that downloads a function's code through the documented path breaks — `aws lambda get-function` followed by a fetch of Code.Location, SAM/CDK and CI jobs that re-download a deployed package to diff or re-publish it, and any tool that verifies CodeSha256 against the served bytes. The call appears to succeed, so the failure surfaces later as a DNS/connection error (or, for a caller with real AWS credentials and network, a request sent to real AWS), which is worse than an explicit refusal.
  Fix: the GetFunction handler's serializer for FunctionCodeLocation — the presigned code URL must be minted against the emulator's own serving host/port (the same endpoint the GetFunction request arrived on, or a per-resource listener the emulator owns) and that path must serve the stored deployment-package bytes unchanged. Same treatment as the other "published address is a real-AWS host" reports (e.g. 20261109T120740Z-iot-data-…, 20261109T120625Z-docdb-cluster-instance-endpoints-are-published-as-real), none of which covers Lambda's Code.Location.
  Traces: dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip.

## Evidence
Check: dynamodb__lambda__rekognition__s3.get-function-code-location-serves-the-exact-uploaded-zip (GROUP environment-fidelity; the check uploads a zip, then GETs GetFunction's Code.Location and byte-compares the response against the uploaded bytes).
Expected (AWS): GetFunction -> Code.Location is a presigned URL the caller can fetch; an HTTPS GET of it returns the uploaded zip byte-for-byte (matching CodeSha256). For an emulator, that URL must be on an address the emulator serves, e.g. http(s)://<emulator host>:<port>/<path>.
Actual (emulator): Code.Location = https://awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com/snapshots/000000000000/sim2real-scn-rekfn-<suffix>-<uuid> — a real-AWS S3 hostname the emulator does not listen on. Replay output:
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(f"{message}: {problem[1]}", problem[0], value_class(value))
  simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com/snapshots/000000000000/sim2real-scn-rekfn-5fe0f9c9c01c-910015de-8ce5-4425-a8e7-863bfd12cc18 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36917/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The hostname pattern (awslambda-<region>-tasks.s3.<region>.amazonaws.com) is the divergence; the account id, function name, uuid and the harness's own 127.0.0.1:36917 are per-run values.

## Steps to Reproduce
1. lambda.create_function(FunctionName=<fn>, Code={'ZipFile': <zip bytes>}, …) (or CreateFunction from an S3 object), and keep the uploaded zip bytes.
2. lambda.get_function(FunctionName=<fn>) and read Code.Location.
3. Observe the URL's host is awslambda-<region>-tasks.s3.<region>.amazonaws.com — not the host:port the GetFunction request was made against.
4. HTTP GET that URL from the same client that reached the emulator: it is unreachable (name resolution / connection failure), so the uploaded bytes can never be compared. Expected: the URL is on the emulator's own serving host and returns the uploaded zip byte-for-byte.
