Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard cloudwatch__dynamodb__lambda.reserved-concurrency-admission-control-and-metric-agreement-rejected
Issue-Key: lambda|ListEventSourceMappings|ResourceNotFoundException
Issue-Key: cloudwatch__dynamodb__lambda.reserved-concurrency-admission-control-and-metric-agreement-rejected|run#34|Timeout|time.sleep(10)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Lambda: ListEventSourceMappings answers ResourceNotFoundException 'Function not found' for a function that exists, and reserved-concurrency admission control is never observable (2 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEventSourceMappings

## Description
Two net-new root causes.

Cause 1 of 2: ListEventSourceMappings refuses a live function ARN as not found.
AWS: ListEventSourceMappings(FunctionName=<arn of an existing function>) returns 200 with the mappings for that function — an empty EventSourceMappings list when there are none. ResourceNotFoundException is reserved for a function that genuinely does not exist. Recorded behavior: a 200 listing.
Emulator: answers ResourceNotFoundException "Function not found: arn:aws:lambda:<region>:<account>:function:<name>" for a function the same run just created, so the check aborts before it can inspect the DynamoDB-stream event source mapping or the dashboard built over it.
Why: the standard idiom for reconciling event source mappings — list by function, then create/update what is missing — fails outright, and a caller cannot distinguish "function absent" from "function present with no mappings". Any deploy or drift-detection loop over event source mappings is broken.
Fix: function resolution in the Lambda ListEventSourceMappings handler (it must resolve the same function identity CreateFunction registered, by ARN as well as by bare name).
Traces: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard.

Cause 2 of 2: reserved-concurrency admission control and its metric agreement are never observable.
AWS: PutFunctionConcurrency sets a reserved concurrency ceiling that the invoke path enforces — invocations beyond the ceiling are refused/throttled — and the AWS/Lambda ConcurrentExecutions and Throttles series agree with the admission decisions. Recorded behavior: the refusal and the agreeing metrics are both observed.
Emulator: the check polls for that agreement and is killed at the harness 60 s cap, so neither the admission refusal nor the metric agreement is ever observable. (The invoke leg is additionally blocked by the already-filed Lambda Invoke stub, 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md; that issue does not carry this replay key.)
Why: a client cannot verify a concurrency ceiling is enforced, so a function that should be throttled is not, and the Throttles/ConcurrentExecutions metrics used to alarm on it never appear.
Fix: reserved-concurrency admission control in the Lambda invoke path and the AWS/Lambda metric publication that must agree with it.
Traces: cloudwatch__dynamodb__lambda.reserved-concurrency-admission-control-and-metric-agreement-rejected.

## Evidence
Cause 1: "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListEventSourceMappings operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-273b4dacf94f".
Cause 2: "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. CreateFunction(FunctionName=f); ListEventSourceMappings(FunctionName=<f's ARN>) -> ResourceNotFoundException 'Function not found: <arn>' instead of 200 with an EventSourceMappings list.
2. CreateFunction, PutFunctionConcurrency(ReservedConcurrentExecutions=n), drive more than n concurrent invocations, then poll AWS/Lambda Throttles / ConcurrentExecutions for agreement with the admission decisions -> never resolves.
