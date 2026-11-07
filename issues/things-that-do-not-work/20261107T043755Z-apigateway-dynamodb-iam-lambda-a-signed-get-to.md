Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body
Issue-Key: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body|_invoke#5|Timeout|return exc.code, {k.lower(): v for k, v in (exc.headers or {}).items()}, exc.read().decode()
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# apigateway__dynamodb__iam__lambda: a signed GET to a deployed AWS_PROXY stage never answers, so the handler's own AccessDeniedException 500 body is unobservable

> _Note: this issue was identified against the code base at commit `12bd58142e134c788a6e6e722018c3787d666c1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api deployed stage, AWS_PROXY integration)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a SigV4-signed GET of a deployed REST stage wired to an AWS_PROXY (Lambda proxy) integration produces no HTTP response at all, so neither the admitted 200 nor the 500 that follows after the function role's DynamoDB read grant is deleted is ever observed.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body.

AWS: recorded behavior only — the execute-api data plane is not described by the API Gateway control-plane Smithy model, so the model covers only the setup calls (CreateRestApi / PutMethod authorizationType=AWS_IAM / PutIntegration type=AWS_PROXY / CreateDeployment+CreateStage, lambda AddPermission for apigateway.amazonaws.com, iam DeleteRolePolicy). What the recording shows is that every request reaching a deployed stage terminates in a response: (a) a signed GET by a caller allowed execute-api:Invoke on <api-arn>/<stage>/GET/* is answered HTTP 200 with the handler's JSON body; (b) after iam DeleteRolePolicy removes the function role's table-read policy, the same signed GET is answered HTTP 500 whose JSON body carries an `error` field holding the handler's botocore message. That message's fixed text is "An error occurred (AccessDeniedException) when calling the GetItem operation: User: <principal> is not authorized to perform: dynamodb:GetItem on resource: <table-arn> because no identity-based policy allows the dynamodb:GetItem action"; its variable parts are the assumed-role principal (arn:aws:sts::<account>:assumed-role/<function-role>/<function-name>, taken from the function's execution role and function name) and the table ARN from the request — no value from any single run is part of the requirement.
Emulator: no HTTP response is produced on the proxy path at any point. The check polls the stage invoke URL until the harness budget is exhausted and the suite reports exactly 'Error: Check timed out after 60.0s'. No status line, no headers and no body are ever observed, for either the granted GET or the post-DeleteRolePolicy GET.
Why: a client of a proxy-integrated REST API hangs until its own socket timeout instead of receiving a status. It cannot tell a served request from one whose backend was denied by IAM, and the whole negative class (a backend AccessDeniedException surfacing as a 500 body naming dynamodb:GetItem and the assumed-role principal) is unverifiable; retry/alerting logic keyed on 200 vs 500 never runs.
Fix: the execute-api data-plane request path of the API Gateway service — deployed stage/route resolution, SigV4 verification at the stage, the AWS_PROXY invoke of the target function, and the mapping of the handler's proxy response (statusCode/headers/body) back to the HTTP response. Every request that reaches a deployed stage must terminate in a response.
Related open issue on the same surface that does NOT carry this check's replay key (the suite does not report this check as owned, so it is filed here and should be closed alongside): 20261103T014646Z-apigateway-dynamodb-iam-lambda-a-deployed-rest-stage.md.
Traces: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body.

## Evidence
Expected (recorded AWS): signed GET of the deployed stage path -> HTTP 200 with the seeded item as JSON; then, after iam DeleteRolePolicy drops the function role's table-read policy, the same signed GET -> HTTP 500 with a JSON body whose `error` starts with "An error occurred (AccessDeniedException) when calling the GetItem operation: User: " and ends with " is not authorized to perform: dynamodb:GetItem on resource: <table-arn> because no identity-based policy allows the dynamodb:GetItem action", the principal between them being arn:aws:sts::<account>:assumed-role/<function-role>/<function-name>.

Actual (emulator): no response at all. The suite reports 'Error: Check timed out after 60.0s'; no status, headers or body were recorded for either request.

## Steps to Reproduce
1. Create a REST API with a GET method (authorizationType=AWS_IAM) wired to an AWS_PROXY integration over a Lambda function that GetItems a DynamoDB table; AddPermission for apigateway.amazonaws.com on the stage; create the deployment and stage; seed one item.
2. Assume a role allowed execute-api:Invoke on <api-arn>/<stage>/GET/* and send a SigV4-signed GET to the stage invoke URL — no HTTP response arrives within 60 s (AWS answers 200 with the item).
3. iam DeleteRolePolicy the function role's table-read policy and repeat the signed GET — again no response arrives (AWS answers 500 with the handler's AccessDeniedException message in the body).
4. Or replay the check id in trace_ids.
