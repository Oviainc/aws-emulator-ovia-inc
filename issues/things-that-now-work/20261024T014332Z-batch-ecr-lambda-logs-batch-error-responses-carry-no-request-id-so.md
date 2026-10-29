Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-without-execution-role-rejected
Issue-Key: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-without-execution-role-rejected|run#8|AssertionError|assert (code, message) == ('ClientException', expected), (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Batch error responses carry no request id, so a ClientException message cannot be correlated with the response that produced it

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterJobDefinition

## Description
One cause.

Cause 1 of 1: an AWS Batch error response does not expose a request id to the client, although the error message text embeds one.

AWS: recorded behavior only (the Smithy model does not describe message text). AWS Batch rejects a FARGATE RegisterJobDefinition with no containerProperties.executionRoleArn with HTTP 400 and error code `ClientException`, and the ClientException message ends with `RequestId: <id>` where `<id>` is the request id of that same response — the value the SDK surfaces as `ResponseMetadata.RequestId` (the `x-amzn-RequestId` response header). The two are the same string by construction, which is why a client can quote the id from a failed call when opening a support case.

Emulator: the error code is right (`ClientException`) and the sentence is right, and the message embeds a UUID (recorded example: `Error executing request, Exception : executionRoleArn is required for Fargate jobs., RequestId: 154cbf1a-ac3a-46c9-b383-87cb94868ca7`), but the same response's `ResponseMetadata.RequestId` reads as absent/None, so the id in the message corresponds to nothing the client can see. The replayed check reads the id from the response metadata and reconstructs the expected message, which therefore renders as `... RequestId: None`.

Why: a client that logs `ResponseMetadata.RequestId` on failure records nothing, and a client that parses the id out of the message gets a value it cannot match to any response metadata it holds. Any correlation or retry/diagnostic logic keyed on the request id of a failed call is broken, and the message and metadata of one response disagree with each other.

Fix: the error-serialization path for this service — whatever emits the request-id response header / metadata for error responses, plus the RegisterJobDefinition handler's validation that composes the ClientException message, so both use the one id generated for the request.

Traces: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-without-execution-role-rejected (the rest of the check passes: the rejection code is correct and no revision is stored).

## Evidence
Expected (check, built from the response's own metadata): code `ClientException`, message `Error executing request, Exception : executionRoleArn is required for Fargate jobs., RequestId: <ResponseMetadata.RequestId>`.
Actual: code `ClientException`, message `... RequestId: 154cbf1a-ac3a-46c9-b383-87cb94868ca7` while `exc.response['ResponseMetadata'].get('RequestId')` is None, so the expected string renders as `... RequestId: None`.
AssertionError: registering a Fargate job definition with no executionRoleArn raised 'ClientException': '... RequestId: 154cbf1a-...', expected ClientException '... RequestId: None'.

## Steps to Reproduce
1. batch.register_job_definition(jobDefinitionName=<name>, type='container', platformCapabilities=['FARGATE'], containerProperties={'image': ..., 'resourceRequirements': [VCPU/MEMORY]}) — i.e. omit executionRoleArn.
2. Catch botocore ClientError and read exc.response['Error']['Message'] and exc.response['ResponseMetadata'].get('RequestId').
3. Observe the message embeds a request id while ResponseMetadata.RequestId is missing.
