Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email
Issue-Key: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email|_run_shell#10|Timeout|raise AssertionError(
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# lambda__sesv2: the CLI-deployed python3.12 function's in-handler SESv2 SendEmail never returns ok=True, so the cli-roundtrip deploy-and-send check never reaches a verdict

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Note on existing work: open issue 20261106T194015Z-lambda-sesv2-the-cli-deployed-python3-12-function.md already describes this same failing check, but the replay report does NOT mark this check as OWNED by it (that issue's replay key no longer matches the check's current failure shape), so it is re-filed here with the current grounding. A fixer should de-duplicate against that file rather than treat this as a second defect. Related, but owning different checks: 20261106T151148Z-lambda-sesv2-boto3-client-sesv2-and-boto3-client.md and 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md.

One net-new (unowned) root cause behind 1 failing check.

Cause 1 of 1: a Lambda function provisioned entirely through the AWS CLI (iam create-role / iam put-role-policy with an inline ses:SendEmail allow, lambda create-function --runtime python3.12 --zip-file, lambda wait function-active, lambda invoke) never produces a successful in-handler SESv2 SendEmail, so the check's invoke-until-ok poll never observes a payload with ok=true and the check is cut off by the harness cap with no pass and no AWS error. Failing check: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email.

AWS: a python3.12 function whose execution role allows ses:SendEmail can call SESv2 SendEmail from its own handler and receives the modelled success. Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2): SendEmail POST /v2/email/outbound-emails, success code 200, output member MessageId (OutboundMessageId); its modelled refusals are AccountSuspendedException, BadRequestException, LimitExceededException, MailFromDomainNotVerifiedException, MessageRejected, SendingPausedException (400), NotFoundException (404) and TooManyRequestsException (429). So an unsuccessful send always arrives at the handler as one of those AWS error codes with that error's message; AWS never answers the handler with a client-side SDK construction failure. For lambda Invoke, a healthy function answers HTTP 200 with no X-Amz-Function-Error header and the handler's JSON payload as the body. The AWS end state for this scenario is: FunctionError absent, handler payload {"ok": true, "messageId": "<non-empty string>"} where messageId is whatever OutboundMessageId SendEmail minted for that request (the value itself is per-run and is not part of the requirement).

Emulator: the check never reaches an assertion — the replay records only `Error: Check timed out after 60.0s`. The check invokes the function via the CLI every 3 s for up to 90 s, returning only when FunctionError is absent and the handler payload reports ok=true; a handler that keeps reporting ok=false (or an invoke that never settles) therefore consumes the whole budget and the check is killed. Hypothesis (stated by neither the model nor this check's own recording, but by sibling checks of this service): the handler's boto3.client("sesv2") cannot be constructed inside the emulator's in-process python runtime — the identical handler shape in the owned checks yields UnknownServiceError with the message `Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb` — so ok is never true here. Either way this CLI-driven path returns no AWS error code and no terminal answer.

Why: a client that provisions a function with the AWS CLI and then invokes it gets no answer at all — neither the 200 with a MessageId nor any modelled SESv2 error code. An operator cannot tell whether create-function, the function-active waiter, the role's propagation or the in-handler SES call is at fault, and automation that waits on its own handler's ok flag hangs instead of failing fast on an AWS error code.

Fix: the in-process python Lambda runtime's guest AWS SDK (its service-model catalog must cover every service the emulator serves, sesv2 included) and the Invoke error path, which must surface the invoked handler's failure to the invoker as a terminal response (FunctionError set with the handler's error payload) rather than letting a caller poll indefinitely; plus whichever step of the CLI deploy path (create-function, the function-active waiter, or Invoke) fails to settle.

Traces: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email (the report lists it under `### lambda__sesv2` with no OWNED/ALREADY-FILED marker). Note the suite heading `lambda__sesv2` is a composite of two services, so it joins no single bootstrap token by exact equality; both constituents (lambda, sesv2) are bootstrapped.

## Evidence
Expected (AWS): `aws lambda invoke` of the CLI-deployed python3.12 function returns HTTP 200 with no FunctionError, and the output file contains the handler's payload {"ok": true, "messageId": "<non-empty string>"} — MessageId being SendEmail's modelled 200 output member (OutboundMessageId). Had the role lacked ses:SendEmail, the handler would instead report an AWS error code (AccessDeniedException); an unverified sender would report MessageRejected. Fixed text of such a refusal is the error class's own message; its variable parts (the identity/address, the configuration set name) come from the request and the emulator's state.

Actual (emulator): lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email — `Error: Check timed out after 60.0s`. No assertion is ever reached: the check's `_invoke_until_ok(..., timeout_s=90.0, interval_s=3.0)` loop never observes `result["meta"].get("FunctionError") is None and body.get("ok") is True`, so the run is terminated by the harness cap. The same handler shape in this service's owned checks returns `{'ok': False, 'errorCode': 'UnknownServiceError'}` with `Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb` — i.e. the in-handler SESv2 client cannot be built, so ok is never true, and no SESv2 error code or status is ever delivered to the invoker either.

## Steps to Reproduce
1. Point the aws CLI at the emulator. `aws iam create-role` a role trusted by lambda.amazonaws.com, then `aws iam put-role-policy` it an inline policy allowing `ses:SendEmail` on `*` (plus logs:CreateLogGroup/CreateLogStream/PutLogEvents).
2. Zip a single `index.py` whose `handler` calls `boto3.client("sesv2").send_email(FromEmailAddress=<address under a verified sending domain>, Destination={"ToAddresses": ["success@simulator.amazonses.com"]}, Content={"Simple": {"Subject": {"Data": ...}, "Body": {"Text": {"Data": ...}}}})` and returns `{"ok": True, "messageId": resp["MessageId"]}`.
3. `aws lambda create-function --function-name <fn> --runtime python3.12 --handler index.handler --role <role arn> --timeout 30 --zip-file fileb://<zip>`, then `aws lambda wait function-active --function-name <fn>`.
4. `aws lambda invoke --function-name <fn> --cli-binary-format raw-in-base64-out --payload '{"from":"<verified sender>","to":"success@simulator.amazonses.com","subject":"...","body":"..."}' out.json`.
5. Expected: HTTP 200, no FunctionError, `out.json` = `{"ok": true, "messageId": "<non-empty>"}`. Observed: the invoke never yields ok=true and never yields a modelled SESv2 error either, so a poll waiting on it never terminates and the check is killed at the 60 s harness cap with no verdict.
