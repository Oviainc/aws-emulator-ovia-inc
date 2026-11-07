Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.async-invoke-alias-qualifier-applies-to-onfailure-destination lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected lambda.callback-timeout-exceeded-rejected lambda.durable-execution-callback-timeout-enforced lambda.durable-execution-kms-enforcement-rejected lambda.durable-execution-kms-errors lambda.send-callback-success-after-checkpoint-window lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected lambda.deploy-lambda-1-version-alias-code-update lambda.deploy-lambda-alias-weight-on-primary-version-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.async-invoke-alias-qualifier-applies-to-onfailure-destination|run#29|AssertionError|assert message is not None, "no message arrived on the OnFailure destination queue within 90s"
Issue-Key: lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected|_poll_tag#12|AssertionError|raise AssertionError(f"tag {key!r} never appeared on {fn_arn} in {timeout_s}s; tags={seen!r}")
Issue-Key: lambda.callback-timeout-exceeded-rejected|_wait_for_callback_id#7|Timeout|return None, None
Issue-Key: lambda.durable-execution-callback-timeout-enforced|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda.durable-execution-kms-enforcement-rejected|run#20|AssertionError|assert get_msg == _kms_disabled_msg(key_arn), f"unexpected GetDurableExecution message: {get_msg!r}"
Issue-Key: lambda.durable-execution-kms-errors|run#25|AssertionError|assert get_msg == disabled_msg, get_msg
Issue-Key: lambda.send-callback-success-after-checkpoint-window|_poll_until#8|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start|_poll_until#9|Timeout|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected|_poll_until#9|Timeout|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda.deploy-lambda-1-version-alias-code-update|_wait_stack#45|AssertionError|raise AssertionError(
Issue-Key: lambda.deploy-lambda-alias-weight-on-primary-version-rejected|run#23|AssertionError|assert msg == (
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 5 net-new causes: async destinations inert, durable-execution callbacks/KMS messages wrong, runtime pinning stalls, CFN alias RoutingConfig rejected, UpdateAlias refusal wording

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Five net-new root causes in the Lambda service. Open issues 20261107T135301Z-lambda-9-net-new-root-causes-in-durable.md and 20261107T014900Z-lambda-sqs-the-asynchronous-invocation-plane-is-inert.md cover other checks; none of the checks below carry their replay keys.

Cause 1 of 5 — an asynchronous invoke qualified by an alias never delivers its OnFailure destination record, and an async invoke's own side effect never happens.
AWS: recorded behavior only. An async Invoke (InvocationType=Event) of an alias runs the alias's target version and, on a handler failure, writes one invocation record to the OnFailure destination configured for that alias; a successful async invoke runs the handler to completion so its side effects (here, a TagResource on the function) are observable.
Emulator: nothing arrives and nothing happens. Replay details: `AssertionError: no message arrived on the OnFailure destination queue within 90s`, and `AssertionError: tag 'sim2real-got-band' never appeared on arn:aws:lambda:...:function:sim2real-behav-rt-async-... in 150s; tags={'sim2real': 'deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected'}`.
Why: a client that relies on async destinations for failure handling loses every failed event silently, and an async invoke that is accepted (202) may never actually run, so fire-and-forget work is dropped without any error.
Fix: the Lambda asynchronous invocation pipeline — the queue/worker that must execute an Event invoke and then dispatch the invocation record to the alias-qualified EventInvokeConfig's OnFailure/OnSuccess destination.
Traces: lambda.async-invoke-alias-qualifier-applies-to-onfailure-destination, lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected.

Cause 2 of 5 — durable execution callbacks never deliver, and the KMS failure message is wrong.
AWS: recorded behavior only. A durable execution whose callback is resolved (or whose callback window expires) drives the configured destination, so a record arrives on the destination queue; and when the function's durable-execution KMS key is disabled, GetDurableExecution reports the KMS failure with the message AWS words for that class.
Emulator: no callback record ever arrives and the KMS text is not AWS's. Replay details: `AssertionError: a message to arrive on https://sqs.../sim2real-behav-dest-q-... not met within 60s. Last result: None` (twice), `Error: Check timed out after 180.0s`, and `AssertionError: unexpected GetDurableExecution message: "Lambda was unable to decrypt the durable execution data because the KMS key arn:aws:kms:us-west-2:000000000000:key/<id> is disabled. Please check the function's KMS key settings."` — the fixed text of that sentence is the emulator's own wording, with the key ARN taken from the request; the recording rejects it as not AWS's message for a disabled durable-execution key.
Why: a client cannot complete or time out a durable execution's callback, so long-running workflows never progress and never report failure; and a client that matches on the KMS error message to distinguish a disabled key from a missing grant matches nothing.
Fix: the durable-execution engine — callback resolution/timeout and its destination dispatch — and the GetDurableExecution/durable-execution error serializer that must reproduce AWS's KMS failure message for a disabled key (fixed text from AWS, key ARN from the function's configuration).
Traces: lambda.callback-timeout-exceeded-rejected, lambda.durable-execution-callback-timeout-enforced, lambda.durable-execution-kms-enforcement-rejected, lambda.durable-execution-kms-errors, lambda.send-callback-success-after-checkpoint-window.

Cause 3 of 5 — runtime-version pinning never converges.
AWS: recorded behavior only. PutRuntimeManagementConfig with UpdateRuntimeOn=Manual and a RuntimeVersionArn pins the version: the pin survives a cold start and is reported in RuntimeVersionConfig, and a RuntimeVersionArn from another runtime family is refused.
Emulator: both legs produce no verdict — `Error: Check timed out after 180.0s` for each.
Why: a client that pins a runtime version for reproducibility cannot confirm the pin, and a bad pin is neither applied nor refused, so runtime management is unusable.
Fix: PutRuntimeManagementConfig / GetRuntimeManagementConfig handling and validation, and propagation of the pinned RuntimeVersionArn into a cold execution environment.
Traces: lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start, lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected.

Cause 4 of 5 — a CloudFormation update of AWS::Lambda::Alias is refused by Lambda with a deserialization error, rolling the stack back.
AWS: recorded behavior only. A stack update that repoints an AWS::Lambda::Alias (and sets its RoutingConfig) reaches UPDATE_COMPLETE; Smithy models UpdateAlias.RoutingConfig as the structure AliasRoutingConfiguration whose AdditionalVersionWeights is a map, and UpdateAlias returns 200.
Emulator: UpdateAlias answers 400 with the message `expected object start, got array start`, so the Alias resource fails and the stack ends UPDATE_ROLLBACK_COMPLETE. Replay detail: `AssertionError: stack reached 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Alias]. '), expected 'UPDATE_COMPLETE'; failed resources: [... 'Alias UPDATE_FAILED: Resource handler returned message: "expected object start, got array start (Service: Lambda, Status Code: 400, Request ID: ...)" ... HandlerErrorCode: InvalidRequest']`.
Why: every template that manages a Lambda alias fails to update and rolls back, so version/alias-based deployment through CloudFormation is impossible — and the 400 the client sees is a parser message, not a modelled Lambda error it can act on.
Fix: the UpdateAlias request deserializer (the RoutingConfig / AdditionalVersionWeights member shape it expects) and/or the CloudFormation AWS::Lambda::Alias resource handler that builds that request.
Traces: lambda.deploy-lambda-1-version-alias-code-update.

Cause 5 of 5 — UpdateAlias refuses a self-referential routing weight with the wrong message.
AWS: recorded behavior only. UpdateAlias/CreateAlias refuse a RoutingConfig.AdditionalVersionWeights entry naming the alias's own FunctionVersion with InvalidParameterValueException (Smithy: UpdateAlias errors include InvalidParameterValueException, httpError 400); the message is AWS's fixed text for that class, naming the offending version from the request.
Emulator: the refusal carries the emulator's own sentence. Replay detail: `AssertionError: UpdateAlias refusal message: 'RoutingConfig.AdditionalVersionWeights names version 1, which is the version this alias already invokes'` — the whole sentence is emulator wording; only the version number ('1') comes from the request.
Why: a client that matches on AWS's message (or surfaces it to an operator) sees text AWS never emits, so error handling keyed on the message string breaks even though the error code is right.
Fix: the UpdateAlias/CreateAlias RoutingConfig validation's message text.
Traces: lambda.deploy-lambda-alias-weight-on-primary-version-rejected.

## Evidence
Cause 1: expected an OnFailure record on the alias's destination queue within 90 s and the async handler's TagResource side effect within 150 s; actual `no message arrived on the OnFailure destination queue within 90s` and `tag 'sim2real-got-band' never appeared ... in 150s`.
Cause 2: expected a durable-execution callback record on the destination queue; actual `not met within 60s. Last result: None` and a 180 s harness timeout. Expected AWS's wording for a disabled durable-execution KMS key; actual "Lambda was unable to decrypt the durable execution data because the KMS key <keyArn> is disabled. Please check the function's KMS key settings."
Cause 3: expected a pinned RuntimeVersionArn to survive a cold start and a foreign runtime ARN to be refused; actual `Error: Check timed out after 180.0s` on both.
Cause 4: expected UpdateAlias 200 and stack UPDATE_COMPLETE; actual UpdateAlias 400 "expected object start, got array start" and stack UPDATE_ROLLBACK_COMPLETE with Alias UPDATE_FAILED.
Cause 5: expected AWS's InvalidParameterValueException message for a self-referential AdditionalVersionWeights entry; actual 'RoutingConfig.AdditionalVersionWeights names version 1, which is the version this alias already invokes'.

## Steps to Reproduce
Cause 1: PutFunctionEventInvokeConfig on an alias with an OnFailure SQS destination; Invoke the alias with InvocationType=Event and a payload that makes the handler raise; poll the queue (empty). Separately, async-invoke a handler that calls TagResource on itself and poll ListTags (tag never appears).
Cause 2: start a durable execution with a destination queue and let its callback window expire / send the callback; poll the queue (empty). Disable the function's durable-execution KMS key and call GetDurableExecution; compare the message with AWS's.
Cause 3: PutRuntimeManagementConfig(UpdateRuntimeOn=Manual, RuntimeVersionArn=<pin>), force a cold start and read RuntimeVersionConfig; then try a RuntimeVersionArn from another runtime family.
Cause 4: CreateStack with AWS::Lambda::Function + Version + Alias, then UpdateStack changing the code so the alias repoints — UpdateAlias returns 400 'expected object start, got array start'.
Cause 5: UpdateAlias with RoutingConfig.AdditionalVersionWeights = { "<the alias's own FunctionVersion>": 0.5 } and compare the InvalidParameterValueException message with AWS's.
