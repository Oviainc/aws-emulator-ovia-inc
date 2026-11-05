Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative lambda__sesv2.cli-lambda-sesv2-describe-function-config
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative|_invoke_once#9|Timeout|return lam.invoke(FunctionName=function_name, Payload=json.dumps(payload).encode())
Issue-Key: lambda__sesv2.cli-lambda-sesv2-describe-function-config|_run_shell#8|Timeout|return result
Issue-Commit: 5368d81c71b1007c24003ba7dd128ee31ad4f292

# lambda__sesv2: Invoke of a python3.12 ZipFile handler never returns the handler's own payload, so checks that retry invoke until FunctionError clears never converge (2 checks exhaust their budget)

> _Note: this issue was identified against the code base at commit `5368d81c71b1007c24003ba7dd128ee31ad4f292`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the emulator's Lambda Invoke data plane never produces a successful (FunctionError-free, handler-authored) invocation result for a python3.x ZipFile function, so a client that polls Invoke until the function stops reporting FunctionError loops forever. Failing checks: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative, lambda__sesv2.cli-lambda-sesv2-describe-function-config.

AWS: Smithy lambda `Invoke` — POST /2015-03-31/functions/{FunctionName}/invocations, success httpResponseCode 200; output members StatusCode (httpResponseCode), Payload (httpPayload, the handler's own return value) and FunctionError (header X-Amz-Function-Error). Nothing in the model describes a managed runtime's contents; documented AWS behavior / recorded behavior only: the python3.x managed runtimes execute the deployment package's handler and return its JSON in Payload with NO X-Amz-Function-Error header, X-Amz-Function-Error being set only when the handler itself raised. Both checks rely on that: each creates a role, a python3.12 function whose handler calls one sesv2 API and returns a small JSON object, waits for the function to be Active, then invokes it repeatedly until the invocation comes back without FunctionError, and finally asserts on the handler's body (errorCode == "AccessDeniedException" vs "MessageRejected" for the role-policy negative; sendingEnabled equal to a direct `aws sesv2 get-account` for the CLI config check).

Emulator: Invoke answers 200, but every invocation of such a function reports FunctionError and a runtime-authored payload rather than the handler's response, so the convergence predicate (StatusCode == 200 and FunctionError absent/not starting with "Unhandled") is never satisfied. The two checks therefore never advance past their invoke loop and are reported as `Error: Check timed out after 60.0s` with no further diagnosis — the budget is consumed by retries, not by one slow call. The CFN-deployed variant additionally never reaches the point of comparing the denied function (role without ses:SendEmail) against the allowed one, and the CLI variant never reaches its get-function-configuration/get-account comparison.

Why: a client cannot observe anything a Lambda function does. The IAM-denial contract these checks exist to prove (a role lacking ses:SendEmail must yield AccessDeniedException and no MessageId, while the same code under a granting role must reach SES and get MessageRejected for an unverified sender) is unobservable, and so is the equality between a value read inside a function (SendingEnabled) and the same value read directly. Code that waits for a function to become invocable hangs instead of failing fast or succeeding.

Fix: the Lambda Invoke data-plane runtime — the python execution environment the function-runtime path builds for a ZipFile deployment package, and the FunctionError/Payload serialization of InvocationResponse. A handler that only imports the AWS SDK and calls one AWS API must run to completion and have its own return value serialized into Payload with no X-Amz-Function-Error; FunctionError must be reserved for exceptions the handler actually raises.

Related, and NOT treated as covering these two checks: open issue 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md reports the same surface (python runtime without boto3 -> Runtime.ImportModuleError / FunctionError=Unhandled) and its trace list overlaps these ids, but the suite did not mark either of these checks OWNED by it (their replay keys bind to a check-level timeout, not to the per-frame keys that issue carries), so they are filed here to keep them replayable. A fixer closing that issue should re-replay the two ids below to confirm these timeouts clear.

Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative (60 s budget spent inside _invoke_json -> _poll_until on lam.invoke), lambda__sesv2.cli-lambda-sesv2-describe-function-config (60 s budget spent inside _invoke_until_ok on `aws lambda invoke`).

## Evidence
Expected (AWS): Invoke returns HTTP 200, no X-Amz-Function-Error header, and Payload = the handler's JSON, e.g. {"ok": false, "errorCode": "AccessDeniedException", "message": "<SES's own message>"} for the role with no ses:SendEmail, {"ok": false, "errorCode": "MessageRejected"} for the granting role with an unverified sender, and {"ok": true, "sendingEnabled": <same boolean a direct sesv2 GetAccount returns>} for the CLI check. The checks' retry loops converge on the first or second attempt.

Actual (replay): neither check ever gets an invocation without FunctionError, so both end as:
- lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative — "Error: Check timed out after 60.0s" (the check's only invoke path is _invoke_json, which calls _poll_until(..., lambda r: r.get("StatusCode") == 200 and not str(r.get("FunctionError","")).startswith("Unhandled"), timeout_s=60.0); the predicate never becomes true).
- lambda__sesv2.cli-lambda-sesv2-describe-function-config — "Error: Check timed out after 60.0s" in _invoke_until_ok around `aws lambda invoke --function-name <fn> out.json`.
The function names, role names, stack names and account ids in these runs are per-run values and are not part of the requirement; the requirement being violated is that a successful Invoke of a python3.12 ZipFile function returns the handler's own payload with no FunctionError.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com with an inline policy that grants only logs:* (no ses:SendEmail) and a second role identical except that it also allows ses:SendEmail — directly via iam CreateRole/PutRolePolicy, or as AWS::IAM::Role resources in a CloudFormation stack.
2. Create two python3.12 functions (CreateFunction with Code.ZipFile, Handler=index.handler, or AWS::Lambda::Function in the same stack), one per role, whose handler builds a boto3 sesv2 client, calls send_email(...) (or get_account()), and returns {"ok": True, "messageId": ...} on success and {"ok": False, "errorCode": e.response["Error"]["Code"]} on ClientError.
3. Wait for the functions to report State=Active, then call Invoke in a loop, stopping when the response has StatusCode 200 and no FunctionError (this is what both checks do, with a 60 s budget).
4. Observe that the loop never terminates: every Invoke comes back with X-Amz-Function-Error set and a runtime-authored payload instead of the handler's JSON, so the caller spends its whole budget retrying and never gets to compare AccessDeniedException vs MessageRejected, or the in-function SendingEnabled against a direct `aws sesv2 get-account`.
