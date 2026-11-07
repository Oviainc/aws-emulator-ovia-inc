Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative|_poll_until#9|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive|_poll_until#9|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected|_poll_until#9|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative|_poll_until#10|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive|_poll_until#8|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 677b6e35c5226b67b9bfea5e554d250e3c084a9c

# lambda__sesv2: five CloudFormation-deployed SESv2 Lambda checks never complete — the scenario is unservable within the 60 s budget

> _Note: this issue was identified against the code base at commit `677b6e35c5226b67b9bfea5e554d250e3c084a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CFN-deployed SESv2-plus-Lambda scenario never settles, so five checks are cut off by the suite's 60 s budget rather than answering.
AWS: recorded behavior only — the recording completes these flows. The checks create a CloudFormation stack containing a Lambda function, its execution role and SESv2 resources, wait for the stack to reach a terminal CREATE_COMPLETE/UPDATE_COMPLETE status, then invoke the function and assert the SESv2 outcome (a missing-policy refusal, the role trust, an unverified-sender refusal, a wrong-ARN refusal, and a permission added by a stack update). AWS answers each of those waits in seconds.
Emulator: every one of the five fails with "Error: Check timed out after 60.0s" — nothing was asserted, because some awaited condition (stack terminal status, the function's invocability, or the SESv2 effect) never arrives. Hypothesis (neither the model nor the recording states which): the stack never reaches a terminal status, or the deployed handler never produces its SESv2 side effect, which is the shape of the already-open lambda__sesv2 issues 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md and 20261105T101616Z-lambda-sesv2-the-cfn-deployed-sesv2-handler-never.md — neither of those carries the replay keys below.
Why: a client deploying a SESv2 + Lambda stack through CloudFormation hangs instead of getting a terminal stack status and a working function; a waiter blocks indefinitely rather than failing or succeeding.
Fix: the CloudFormation create/update path for this resource set and the deployed function's invoke path — whichever of the two never reaches its terminal condition; the five checks above are the examples.
Traces: the five check ids in trace_ids.

## Evidence
Expected (AWS / the recording): each check completes — the stack reaches a terminal status and the function's SESv2 call succeeds or is refused as asserted.

Actual (emulator): all five report only
  Error: Check timed out after 60.0s
with no response captured:
- lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative
- lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive
- lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected
- lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative
- lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive

## Steps to Reproduce
1. CreateStack with a template containing an AWS::Lambda::Function, its AWS::IAM::Role (with/without an SESv2 policy) and SESv2 resources.
2. Poll DescribeStacks for a terminal status, then Invoke the deployed function so it calls SESv2 (SendEmail / GetEmailIdentity).
3. For the update leg, UpdateStack to add the SESv2 permission and re-invoke.
4. Observe that the awaited condition never arrives inside 60 s.
5. Re-replay the check ids in trace_ids.
