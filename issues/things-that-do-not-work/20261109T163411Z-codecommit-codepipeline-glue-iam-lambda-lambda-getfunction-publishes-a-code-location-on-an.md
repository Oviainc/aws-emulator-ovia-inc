Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__codepipeline__glue__iam__lambda.e2e-diagnose-glue-pipeline-not-running-lifecycle
Issue-Key: codecommit__codepipeline__glue__iam__lambda.e2e-diagnose-glue-pipeline-not-running-lifecycle|phase#6|AssertionError|raise AssertionError(f'[phase {index}/{len(self.phases)} {name}] {exc}') from exc
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# Lambda GetFunction publishes a Code.Location on an amazonaws.com host the emulator does not serve, so the code bytes cannot be fetched

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
Cause 1 of 1: GetFunction's Code.Location is serialized as a real-AWS presigned S3 URL host instead of an address on one of the emulator's own listeners.

AWS: GetFunction returns Code.Location, a presigned URL from which the caller can download the function's deployment package; the URL is resolvable and a GET against it returns the zip bytes. Smithy: lambda#GetFunction returns FunctionCodeLocation{RepositoryType, Location}, and Location is documented as a presigned URL valid for the caller to download the package — recorded behavior only for which host serves it; the requirement is that the announced URL is fetchable from the caller.

Emulator: Code.Location is announced as `https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<account>/<function>-<uuid>` — a host that is not one of the emulator's own hosts. A customer of the emulator needs `http(s)://<emulator host>:<port>/<resource path>`. The caller therefore cannot fetch the package at all; the announced address is for a service the emulator is not reached on.

Why: any client that downloads a function's code to inspect, diff or re-deploy it (the exact diagnosis this trace performs: reading back the deployed launcher bytes) is sent to an address outside the emulator. It either fails to connect or, worse, reaches real AWS. The returned value is unusable even though the operation reported success.

Fix: the GetFunction serializer's Code.Location construction, which must mint a URL on the emulator's own host/port (or the per-resource listener) rather than the upstream S3 host template.

Traces: codecommit__codepipeline__glue__iam__lambda.e2e-diagnose-glue-pipeline-not-running-lifecycle.

## Evidence
Expected: GetFunction.Code.Location is an address served by the emulator, e.g. http(s)://<emulator host>:<port>/<path>.
Actual: AssertionError: [phase 2/4 solve] endpoint_url was given https://awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com/snapshots/000000000000/sim2real-refarch-glue-stack-ddac8-EtlLaunchFunction-…: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:33145/<resource path>

## Steps to Reproduce
1. Deploy a Lambda function (e.g. through a CloudFormation stack with Code.ZipFile).
2. Call GetFunction on it.
3. Read Code.Location and attempt to GET it; observe the host is an amazonaws.com S3 host the emulator does not serve.
