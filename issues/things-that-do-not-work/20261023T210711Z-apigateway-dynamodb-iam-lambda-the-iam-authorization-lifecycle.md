Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
Issue-Key: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle|_await#7|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__dynamodb__iam__lambda: the IAM-authorization lifecycle check hangs to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a SigV4-signed request to a deployed REST API stage with AWS_IAM authorization never returns.
AWS: a method with authorizationType=AWS_IAM answers at the stage invoke URL — 200 for a signed request from a principal allowed execute-api:Invoke, 403 for a principal without it, and 403 with code MissingAuthenticationToken for an unsigned request. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives; the check exhausts the harness budget and is reported as "Check timed out after 60.0s".
Why: the IAM authorization lifecycle at the stage (grant, invoke, revoke, re-invoke) cannot be observed at all, and the hang stalls the caller instead of refusing it.
Fix: the execute-api data-plane request path — SigV4 verification and authorization at the stage must always produce a response.
Traces: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle.

## Evidence
Detail is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Deploy a REST API with a method whose authorizationType is AWS_IAM, and an IAM role allowed execute-api:Invoke on it.
2. Send a SigV4-signed request to the stage invoke URL with that role's credentials and observe that no response arrives within 60 s.
3. Or replay apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle.
