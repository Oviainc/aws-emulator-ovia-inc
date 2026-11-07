Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.obs-lambda-1-fifo-queue-on-failure-destination-rejected-at-configuration
Issue-Key: cloudwatch__lambda.obs-lambda-1-fifo-queue-on-failure-destination-rejected-at-configuration|run#25|AssertionError|assert err == expected, (
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# Lambda UpdateFunctionEventInvokeConfig accepts a FIFO SQS queue as an OnFailure destination instead of refusing it InvalidParameterValueException

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionEventInvokeConfig

## Description
Cause 1 of 1: the async-invoke destination validator does not reject a FIFO SQS queue, so a destination AWS refuses at configuration time is stored.

AWS: an SQS FIFO queue (an ARN ending in `.fifo`) is not a valid asynchronous-invocation destination. PutFunctionEventInvokeConfig and UpdateFunctionEventInvokeConfig refuse such a DestinationConfig with `InvalidParameterValueException` (HTTP 400) and the message template `The provided destination config DestinationConfig(onSuccess=<onSuccess>, onFailure=<onFailure>) is invalid.` — the fixed text is `The provided destination config ` ... ` is invalid.` and the bracketed rendering in between is built from the request: `onSuccess=null` when OnSuccess is absent, `onFailure=OnFailure(destination=<the OnFailure.Destination ARN from the request>)`. A standard (non-FIFO) queue in the same position is accepted with HTTP 200. Because the update is refused, the function's previously stored event-invoke config is left unchanged, and a refused Put creates none. Smithy (lambda): UpdateFunctionEventInvokeConfig / PutFunctionEventInvokeConfig take DestinationConfig and both model `InvalidParameterValueException` (client, httpError 400); the FIFO restriction and the message template are recorded behavior.

Emulator: the call succeeds. The harness reports `AssertionError: UpdateFunctionEventInvokeConfig to a FIFO OnFailure returned '<call unexpectedly succeeded>', expected ('InvalidParameterValueException', 'The provided destination config DestinationConfig(onSuccess=null, onFailure=OnFailure(destination=<fifo queue arn>)) is invalid.')`. No 400 is produced and the invalid destination is written into the function's event-invoke config.

Why: a client gets a silently broken failure path — Lambda accepts a destination it can never deliver to, so failed async invocations are dropped with no refusal at configuration time and no error for the operator to branch on. It also inverts the recorded transactional contract: a refused update must leave the prior config intact, but here there is no refusal at all, so the prior config is overwritten with an unusable one.

Fix: the destination validation shared by the PutFunctionEventInvokeConfig and UpdateFunctionEventInvokeConfig handlers — it must reject an OnSuccess/OnFailure SQS destination whose queue is FIFO before mutating stored state, raising InvalidParameterValueException (400) with the message rendered from the request's DestinationConfig as above, and leave any existing config untouched.

Traces: cloudwatch__lambda.obs-lambda-1-fifo-queue-on-failure-destination-rejected-at-configuration

## Evidence
Expected: HTTP 400 `InvalidParameterValueException` with message `The provided destination config DestinationConfig(onSuccess=null, onFailure=OnFailure(destination=<arn of the FIFO queue named in the request>)) is invalid.` — fixed text `The provided destination config ` / ` is invalid.`, the inner rendering taken from the request's DestinationConfig; and the function's existing event-invoke config unchanged. A standard queue in the same position must be accepted (200).

Actual: the call unexpectedly succeeded — `AssertionError: UpdateFunctionEventInvokeConfig to a FIFO OnFailure returned '<call unexpectedly succeeded>', expected ('InvalidParameterValueException', 'The provided destination config DestinationConfig(onSuccess=null, onFailure=OnFailure(destination=arn:aws:sqs:<region>:<account>:<name>.fifo)) is invalid.')`.

## Steps to Reproduce
1. CreateQueue a standard queue and CreateQueue a FIFO queue (name ending `.fifo`, FifoQueue=true); note both queue ARNs.
2. CreateFunction a small Python function with an execution role.
3. PutFunctionEventInvokeConfig(FunctionName=<fn>, DestinationConfig={OnFailure:{Destination:<standard queue arn>}}) — succeeds (control).
4. UpdateFunctionEventInvokeConfig(FunctionName=<fn>, DestinationConfig={OnFailure:{Destination:<FIFO queue arn>}}).
5. Observe HTTP 200 instead of 400 InvalidParameterValueException, and GetFunctionEventInvokeConfig now reporting the FIFO destination.
