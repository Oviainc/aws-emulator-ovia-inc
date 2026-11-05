Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-lambda-invoke-returns-200-for-unrunnable-job
Issue-Key: batch__ecr__lambda__logs.refarch-batch-lambda-invoke-returns-200-for-unrunnable-job|run#15|AssertionError|assert 'FunctionError' not in response, (
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# batch__ecr__lambda__logs: lambda Invoke of a python3.12 wrapper that submits a Batch job answers 200 with FunctionError set, so the handler's jobId document is never produced

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, grounded in the single surviving failing check of this service (the other failures of this service are OWNED by already-filed issues and are not repeated here).

Cause 1 of 1: a synchronous Invoke of a managed python3.12 function whose handler builds a boto3 Batch client and calls SubmitJob is answered HTTP 200 but with the FunctionError header set ('Unhandled'), i.e. the emulator reports the invocation as an unhandled handler failure instead of returning the handler's result document.

AWS: Smithy (com.amazonaws.lambda) models Invoke as POST /2015-03-31/functions/{FunctionName}/invocations with output InvocationResponse{StatusCode (@httpResponseCode), FunctionError (@httpHeader X-Amz-Function-Error), LogResult, Payload (@httpPayload), ExecutedVersion}. FunctionError is NOT a modeled error shape: it is only set in-band, on an otherwise successful 200, when the function's own code raises. A successful RequestResponse invocation therefore carries StatusCode 200, NO X-Amz-Function-Error header, and Payload equal to whatever the handler returned. AWS's managed Python runtimes (python3.12 among them) additionally ship the AWS SDK for Python (boto3/botocore) on the runtime import path, so a deployment package that bundles only index.py can still `import boto3` and build a client. For this handler the Payload AWS returns is the handler's own document {"statusCode": 200, "body": "{\"jobId\": \"<id>\"}"} — the fixed part of that contract is the two keys and the jobId key inside body; the jobId value itself is state minted by Batch SubmitJob in that run and is not part of the requirement.

Emulator: Invoke answers StatusCode 200 (that assertion passes) but sets FunctionError='Unhandled', so the check fails at `assert 'FunctionError' not in response`. The handler never returns its document, so the jobId it is supposed to carry — and every downstream read of that job (DescribeJobs on the reported id, the queue it was placed on, its terminal status) — is unreachable. Hypothesis (neither the model nor this check's captured output states the payload, which the suite does not print for this check): the cause is the same bare Python runtime already filed for another service as open issue 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md — a python3.12 handler whose first statement is `import boto3` dies at module import with errorType 'Runtime.ImportModuleError' and errorMessage "Unable to import module '<handler module>': No module named 'boto3'" (fixed text; the module name comes from the function's Handler and the imported module name from the handler source). That issue carries only the seven lambda__scheduler replay keys and does not cover this check, so this check's path (a boto3 Batch client calling SubmitJob from inside an invoked function) is unverified by it; a second candidate is that the in-process runtime does import the SDK but cannot reach the emulator's own batch endpoint with the function's execution-role credentials, which would surface identically as FunctionError='Unhandled'.

Why: invoking a small wrapper function that calls another AWS service is the most common Lambda pattern, and the Batch reference architecture this check replays depends on it — a client POSTs to the function and expects a 200 whose body names the submitted jobId. Against the emulator the client gets a 200 whose FunctionError header says the handler blew up and whose payload is a runtime error document instead of JSON with a jobId, so the caller cannot parse a jobId, cannot poll DescribeJobs, and cannot distinguish 'submission accepted' from 'handler crashed' at all. Worse, the operational property the scenario exists to prove — that a 200 with a jobId is compatible with a job that later FAILS — can never be demonstrated, because no successful 200 is ever produced.

Fix: the lambda crate's Invoke execution path — the component that builds the runtime sandbox for a `python3.*` function and runs its handler. The runtime it starts must make boto3/botocore importable as runtime-provided dependencies (not only what the deployment ZIP contains), and must configure the standard runtime environment (endpoint and execution-role credentials) so that a client built inside the function reaches the emulator's own batch endpoint; a handler that then returns normally must be answered with 200, no X-Amz-Function-Error header, and the handler's return value as Payload.

Traces: batch__ecr__lambda__logs.refarch-batch-lambda-invoke-returns-200-for-unrunnable-job.

## Evidence
Suite run (201/347 passed): service batch__ecr__lambda__logs scored 7/15 (47%), 8 failed; seven of those eight are OWNED by already-filed issues, this is the eighth.

[mismatch] batch__ecr__lambda__logs.refarch-batch-lambda-invoke-returns-200-for-unrunnable-job
  File "check_batch__ecr__lambda__logs_refarch_batch_lambda_invoke_returns_200_for_unrunnable_job.py", line 56, in run
    assert 'FunctionError' not in response, (
  AssertionError: Invoke reported FunctionError='Unhandled'; the wrapper never inspects the job, so an unrunnable job cannot surface as a handler error — and if it does, the operator would have known from the 200 alone

Expected (AWS): Invoke (InvocationType RequestResponse) on the python3.12 function -> HTTP 200, NO X-Amz-Function-Error header, Payload = {"statusCode": 200, "body": "{\"jobId\": \"<batch job id>\"}"}; DescribeJobs on that id returns exactly one job placed on the queue named by the function's JOB_QUEUE environment variable, which later reaches status FAILED because its image cannot be pulled.

Actual (emulator): Invoke -> HTTP 200 with X-Amz-Function-Error = 'Unhandled'. The handler's return document is never produced, so no jobId is observable and none of the subsequent DescribeJobs assertions is reached. Only the StatusCode assertion (200) passed. The function itself was created and settled State=Active with exactly the environment {JOB_QUEUE, JOB_DEFINITION} as sent, so the divergence is in execution, not in CreateFunction. The literal 'Unhandled' is the AWS-defined FunctionError value for an unhandled handler/init failure; the function name, role ARN, queue ARN and job-definition ARN in any one run are values minted by that run and are not part of the requirement.

## Steps to Reproduce
1. batch:CreateComputeEnvironment (FARGATE) plus batch:CreateJobQueue, enabled, and wait for VALID.
2. ecr:CreateRepository (leave it empty) and batch:RegisterJobDefinition type=container, platformCapabilities=[FARGATE], containerProperties.image = the repository URI with no tag, an executionRoleArn, resourceRequirements VCPU 0.25 / MEMORY 512.
3. iam:CreateRole trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole and an inline policy granting batch:SubmitJob on the queue and definition ARNs.
4. lambda:CreateFunction with Runtime='python3.12', Handler='index.handler', Timeout=60, Code.ZipFile containing only index.py whose body is `import boto3`, `import json`, `import os` and a handler that does boto3.client("batch").submit_job(jobName=..., jobQueue=os.environ['JOB_QUEUE'], jobDefinition=os.environ['JOB_DEFINITION'], parameters={...}) and returns {"statusCode": 200, "body": json.dumps({"jobId": response["jobId"]})}. Set Environment.Variables JOB_QUEUE and JOB_DEFINITION. The archive bundles no boto3, exactly as on AWS.
5. Poll GetFunctionConfiguration until State=Active (it does reach Active).
6. lambda:Invoke the function with InvocationType='RequestResponse'.
   -> AWS: 200, no X-Amz-Function-Error, Payload is the handler's {"statusCode":200,"body":"{\"jobId\":...}"}.
   -> Emulator: 200 with X-Amz-Function-Error='Unhandled' and no handler document, so the jobId is never returned.
7. (Unreachable on the emulator) batch:DescribeJobs on the returned jobId and assert one job on the JOB_QUEUE queue that reaches status FAILED.
