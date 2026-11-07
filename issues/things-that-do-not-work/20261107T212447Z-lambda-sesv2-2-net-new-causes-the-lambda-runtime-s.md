Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email lambda__sesv2.cli-lambda-sesv2-describe-function-config lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution|_poll_until#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive|_poll_until#8|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email|_invoke_until_ok#10|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cli-lambda-sesv2-describe-function-config|_invoke_until_ok#8|AssertionError|raise AssertionError(f"invocation never reported ok=True within {timeout_s}s; last body: {last_body}")
Issue-Key: lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative|_poll_until#14|Timeout|last = probe()
Issue-Key: lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative|_poll_until#17|Timeout|last = probe()
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 2 net-new causes: the Lambda runtime's bundled botocore knows no 'sesv2' client, and SESv2 events never reach EventBridge

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Two net-new root causes. Open issues 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md and 20261106T151148Z-lambda-sesv2-boto3-client-sesv2-and-boto3-client.md cover other checks in this service; none of the checks below carry their replay keys.

Cause 1 of 8... (single-cause note: this issue carries two causes only — numbering follows.)

Cause 1 of 2 — `boto3.client("sesv2")` inside a managed Python Lambda raises UnknownServiceError, so no SESv2 call can be made from a function and every SESv2 leg of these scenarios is unreachable.
AWS: recorded behavior only. The managed python3.x runtimes ship a complete boto3/botocore, so `boto3.client("sesv2").send_email(...)` / `create_email_identity(...)` / `get_account()` resolve the service model and are then either served or refused by IAM with AccessDeniedException. Several of these recordings specifically assert the refusal class ("expected AccessDeniedException ... got 'UnknownServiceError'"), so the error code matters, not just success.
Emulator: the client cannot be constructed at all. Every leg returns FunctionError 'Unhandled' with `errorType: 'UnknownServiceError'` and `errorMessage: "Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager"`, raised from `client.py` `_raise_refusal`. Representative details: `AssertionError: successful invoke of <fn> not met within 60.0s. Last result: {... 'x-amz-function-error': 'Unhandled' ...}`, `AssertionError: invocation never reported ok=True within 90.0s; last body: {'ok': False, 'errorCode': 'UnknownServiceError', 'errorMessage': "Unknown service: 'sesv2'..."}`.
Why: no application can send mail, manage identities, configuration sets, contact lists or suppression from a function. The failure arrives as an SDK-construction error, so a client cannot distinguish a missing IAM grant (AccessDeniedException) from a runtime without SESv2, and every negative authorization test in this area passes for the wrong reason.
Fix: the in-process Lambda Python runtime's bundled botocore service catalogue — the `sesv2` service model must be resolvable so calls reach the emulator's SESv2 handlers and are authorized there.
Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution, lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative, lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive, lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected, lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative, lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive, lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email, lambda__sesv2.cli-lambda-sesv2-describe-function-config.

Cause 2 of 2 — SESv2 sending events never reach an EventBridge rule, so the event-driven legs never reach a verdict.
AWS: recorded behavior only. A configuration set with an event destination publishes SESv2 sending events (Send, Delivery, Bounce, ...) so an EventBridge rule matching them invokes its target; a rule whose pattern does not match, or whose target grants EventBridge no invoke permission, produces the corresponding negative outcome. The recordings poll for the rule's target to run (or not).
Emulator: neither the positive nor the negative outcome is ever observed — both checks fail with `Error: Check timed out after 180.0s`.
Why: a client that reacts to bounces and complaints through EventBridge gets no events at all. Suppression-list maintenance, reputation handling and delivery dashboards never fire, and the absence is silent: no rule failure, no dead-letter, nothing.
Fix: the SESv2 configuration-set event-destination publisher — emission of sending events to EventBridge, with the rule's event pattern evaluated and the target's resource policy checked on dispatch.
Traces: lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative, lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative.

## Evidence
Cause 1: expected SESv2 calls from a python3.12 handler to be served or refused with AccessDeniedException; actual errorType 'UnknownServiceError', errorMessage "Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager".
Cause 2: expected a SESv2 sending event to reach a matching EventBridge rule's target (and the mismatched-pattern / missing-permission legs to produce their refusals); actual `Error: Check timed out after 180.0s` on both.

## Steps to Reproduce
Cause 1: CreateFunction (python3.12) whose handler calls boto3.client("sesv2").send_email(...) and Invoke — the payload carries errorType UnknownServiceError naming 'sesv2'.
Cause 2: CreateConfigurationSet with an EventBridge event destination; create an EventBridge rule on source aws.ses with a Lambda target (once with a matching pattern and an invoke permission, once without); SendEmail through the configuration set and poll the target — nothing ever runs.
