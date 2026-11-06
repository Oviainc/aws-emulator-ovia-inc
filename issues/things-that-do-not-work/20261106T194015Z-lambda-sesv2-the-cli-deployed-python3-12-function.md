Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email
Issue-Key: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email|_run_shell#10|Timeout|return result
Issue-Commit: e8d34fd8b87a640bf972b7f1f05015aa932d31e1

# lambda__sesv2: the CLI-deployed python3.12 function's sesv2 SendEmail never returns ok, so the cli-roundtrip deploy-and-send check never reaches a verdict (hangs to the harness cap)

> _Note: this issue was identified against the code base at commit `e8d34fd8b87a640bf972b7f1f05015aa932d31e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: a Lambda function deployed through the AWS CLI (iam create-role / put-role-policy, lambda create-function --runtime python3.12 --zip-file, lambda wait function-active, lambda invoke) never produces a successful in-handler SESv2 send, so the check's invoke-until-ok poll never observes `ok=True` and the whole check is killed by the harness cap instead of producing a pass or a fail. Failing check: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email.

AWS: a python3.12 function whose role grants `ses:SendEmail` can call SESv2 SendEmail from its handler and gets the modelled 200 output. Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2): SendEmail POST /v2/email/outbound-emails, success code 200, output member `MessageId` (OutboundMessageId) — a non-empty string; the modelled refusals are MessageRejected / MailFromDomainNotVerifiedException / BadRequestException / NotFoundException / SendingPausedException / AccountSuspendedException / LimitExceededException (400/404) and TooManyRequestsException (429). A refusal therefore always arrives as one of those AWS error codes with that error's message; AWS never answers the handler with a client-side SDK construction error, and `lambda invoke` of a healthy function returns an HTTP 200 with no `FunctionError` and the handler's JSON payload in the output file. In this scenario AWS's expected end state is: `FunctionError` absent and the handler payload `{"ok": true, "messageId": "<non-empty string>"}`.

Emulator: the check never completes — the replay records only `Check timed out after 60.0s`, with no assertion ever reached. The check polls `lambda invoke` every 3 s for up to 90 s waiting for a handler payload with `ok=True`, so a handler that keeps reporting `ok=False` (or an invoke that never settles) consumes the entire run; nothing in the suite's 60 s budget is left and the check is cut off. Hypothesis (not stated by the model or by this check's recording, but stated by the sibling checks of this same service): the handler's `boto3.client("sesv2")` cannot be built in the emulator's in-process python runtime — open issue 20261106T151148Z-lambda-sesv2-boto3-client-sesv2-and-boto3-client.md records `UnknownServiceError`, `Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb` for exactly this handler shape — so `ok` is never true and the poll runs out. That issue does not carry this check's replay key, and the CLI-driven deploy path (create-role/put-role-policy/create-function/wait function-active/invoke driven entirely by the aws CLI) is exercised by no other check it owns, so this check is unowned work: whatever the fix there turns out to be, this path must be re-replayed and must terminate.

Why: a client that provisions a function with the AWS CLI and then invokes it gets no answer at all — not a success, not an AWS error code. An operator cannot tell whether the deployment, the role propagation, the function's activation or the in-handler SES call is at fault, and automation that waits on `ok` from its own handler hangs rather than failing fast with an AWS error.

Fix: the in-process python Lambda runtime's guest AWS SDK (its service-model catalog must include every service the emulator serves, sesv2 among them) and the Invoke error path that must return the service's own error code/message to the invoker; plus whichever step of the CLI deploy path (create-function, the function-active waiter, or Invoke) fails to settle, so that an unsuccessful run reports a terminal AWS error instead of making the caller poll forever.

Traces: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email (the only example; the report lists it under `### lambda__sesv2` with no owner).

## Evidence
Expected (AWS): `aws lambda invoke` of the CLI-deployed python3.12 function returns HTTP 200 with no `FunctionError`, and the handler's payload is `{"ok": true, "messageId": "<non-empty string>"}` — MessageId is SendEmail's modelled 200 output member. If the role lacked ses:SendEmail the handler would instead report an AWS error code (AccessDeniedException), and an unverified sender would report MessageRejected.

Actual (emulator): lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email — `Error: Check timed out after 60.0s`. No assertion is reached: the check's `_invoke_until_ok(..., timeout_s=90.0, interval_s=3.0)` loop never sees `FunctionError is None and body["ok"] is True`, so the run is cut off by the harness cap. The identical handler shape in the owned checks of this service returns `{'ok': False, 'error': 'UnknownServiceError', 'code': 'UnknownServiceError'}` / `Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb` (see 20261106T151148Z-lambda-sesv2-boto3-client-sesv2-and-boto3-client.md), which is the behaviour that makes `ok` never true here.

## Steps to Reproduce
1. With the aws CLI pointed at the emulator, `aws iam create-role` a role trusted by lambda.amazonaws.com and `aws iam put-role-policy` it an inline policy allowing `ses:SendEmail` on `*` (plus basic logs permissions).
2. Zip a single `index.py` whose `handler` does `boto3.client("sesv2").send_email(FromEmailAddress=<address under a verified sending domain>, Destination={"ToAddresses": ["success@simulator.amazonses.com"]}, Content={"Simple": {...}})` and returns `{"ok": True, "messageId": resp["MessageId"]}`.
3. `aws lambda create-function --runtime python3.12 --handler index.handler --role <role arn> --zip-file fileb://<zip>` then `aws lambda wait function-active --function-name <fn>`.
4. `aws lambda invoke --function-name <fn> --payload '{"from":...,"to":"success@simulator.amazonses.com","subject":...,"body":...}' out.json`.
5. Expected: HTTP 200, no FunctionError, `out.json` = `{"ok": true, "messageId": "<non-empty>"}`. Observed: the invoke never yields `ok=true`, so a retry loop waiting on it never terminates and the check is killed at the 60 s harness cap with no verdict.
