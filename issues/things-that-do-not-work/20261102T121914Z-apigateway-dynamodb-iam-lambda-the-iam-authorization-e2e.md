Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
Issue-Key: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle|_await#7|Timeout|last = probe()
Issue-Commit: 1bdb62554babe5a57dd4c3017ee8c08adb16fcb5

# apigateway__dynamodb__iam__lambda: the IAM-authorization e2e lifecycle reaches no verdict — a signed AWS_IAM stage GET never produces an HTTP response, so the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `1bdb62554babe5a57dd4c3017ee8c08adb16fcb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
1 cause (Cause 1 of 1).

Cause 1 of 1: a signed, AWS_IAM-authorized GET against a deployed REST stage whose method has an AWS_PROXY Lambda integration never produces any HTTP response, so the end-to-end IAM-authorization lifecycle check never reaches a verdict and is cut off by the harness budget.

AWS: recorded behavior only — the execute-api data plane is not described by the apigateway Smithy model, so the requirement is the recorded contract the check asserts. Against real AWS, with a REST method `GET /data-agreements/{agreementId}` whose `authorizationType` is `AWS_IAM` and whose integration is `AWS_PROXY` to a Lambda function, two signed SigV4 (`execute-api`) requests to the deployed stage URL are answered promptly and durably:
  (a) a session of a role whose only identity grant is `execute-api:Invoke` on `arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/POST/*` is answered HTTP 403 with response header `x-amzn-errortype: AccessDeniedException` and a JSON body whose `Message` ends with `:<api-id>/<stage>/GET<resource-path> because no identity-based policy allows the execute-api:Invoke action`. Fixed text of that message class: `User: <principal-arn> is not authorized to perform: execute-api:Invoke on resource: <execute-api-resource-arn> because no identity-based policy allows the execute-api:Invoke action`; the variable parts are the caller's assumed-role session ARN and the execute-api resource ARN (region, account, api id, stage, verb, resource path) built from the request and state — never the literal ids of one run.
  (b) a session of a role granted `execute-api:Invoke` on `.../<stage>/GET/*` is answered HTTP 200 with the Lambda proxy response body, i.e. the DynamoDB item the handler read, verbatim.
The status, the error code `AccessDeniedException` and that message sentence are the requirement.

Emulator: the check never gets either answer. The replay verdict is `Error: Check timed out after 60.0s` and no phase lines were emitted, so the output attributes no single call; the check's own polling helper `_await` re-issues the signed request every 3 s for up to 240 s, which can only exhaust the harness cap if the emulator never serves the 403/AccessDeniedException decision (and then the 200) for the deployed stage. Hypothesis: this is the same non-answering execute-api data plane that the 18 sibling checks of this service already fail on — open issue 20261102T091623Z-apigateway-18-further-requests-to-a-deployed-rest.md (`apigateway: 18 further requests to a deployed REST API stage never produce any HTTP response (60 s timeout)`) names that class, but it does not carry this check's replay key, so this check is filed separately and should be re-replayed with it.

Why: a client cannot observe API Gateway's IAM authorization at all. Nothing is refused, nothing is served, and no status or error code ever arrives: an SDK or raw HTTP caller blocks until its own socket/retry budget expires instead of receiving 403 AccessDeniedException (or 200 with the integration's body). Any workflow that reproduces a 403, derives the policy fix from live API Gateway/IAM state and then proves the fix by invoking the same URL cannot be run against the emulator.

Fix: the execute-api data-plane request path for deployed REST stages — the stage/route resolver, its AWS_IAM authorizer (SigV4 verification plus identity-policy evaluation of `execute-api:Invoke` against the method's `<api-id>/<stage>/<VERB>/<path>` resource ARN) and the AWS_PROXY integration dispatch to the Lambda backend, including the 403 `AccessDeniedException` error serializer for that plane.

Traces: failing check apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle (verdict `mismatch`, detail `Error: Check timed out after 60.0s`).

## Evidence
Behavioral suite replay, service heading `### apigateway__dynamodb__iam__lambda`:

  #### [mismatch] apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
  Error: Check timed out after 60.0s

Expected (recorded contract asserted by the check):
  - restricted role (grant on `.../beta/POST/*`) GET to the deployed stage URL -> HTTP 403, header `x-amzn-errortype: AccessDeniedException`, body `Message` ending `:<api-id>/beta/GET/data-agreements/<id> because no identity-based policy allows the execute-api:Invoke action`.
  - verifier role (grant on `.../beta/GET/*`) GET to the same URL -> HTTP 200 with the seeded DynamoDB item as the Lambda proxy body.

Actual: neither response is ever produced. The check's `_await(probe, pred, ...)` loop (3 s poll, 240 s budget) never satisfies either predicate and the whole check is terminated by the harness after 60 s with no phase output, so the deploy/setup/solve phases produce no verdict at all.

Note: the 18 other checks under this and the `apigateway__iam` headings fail on the same non-answering stage and are owned by 20261102T091623Z-apigateway-18-further-requests-to-a-deployed-rest.md; this check carries no owner in the report.

## Steps to Reproduce
1. Create a DynamoDB table with hash key `agreementId` (S) and put one item.
2. Create a Lambda execution role (trust `lambda.amazonaws.com`, AWSLambdaBasicExecutionRole + dynamodb read on the table) and a python3.12 function whose handler returns `{statusCode:200, body: json(item)}` for `event.pathParameters.agreementId`.
3. Create a REST API (REGIONAL), resources `/data-agreements/{agreementId}`, `put_method(httpMethod=GET, authorizationType="AWS_IAM")`, `put_integration(type="AWS_PROXY", integrationHttpMethod="POST", uri=<lambda invocations arn>)`, `lambda add_permission` for `apigateway.amazonaws.com`, then `create_deployment(stageName="beta")`.
4. Create role A with an inline grant `execute-api:Invoke` on `arn:aws:execute-api:<region>:<account>:<api-id>/beta/POST/*` and role B with the same action on `.../beta/GET/*`; both trusted by the account root.
5. `sts assume_role` role A, SigV4-sign (service `execute-api`) a GET to `https://<api-id>.execute-api.<region>.amazonaws.com/beta/data-agreements/<id>` and send it.
   Expected: 403 with `x-amzn-errortype: AccessDeniedException` and the `...because no identity-based policy allows the execute-api:Invoke action` message. Observed: no HTTP response is ever returned.
6. Repeat step 5 with role B. Expected: 200 with the seeded item as the body. Observed: no response.
7. Replay `apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle` to confirm.
