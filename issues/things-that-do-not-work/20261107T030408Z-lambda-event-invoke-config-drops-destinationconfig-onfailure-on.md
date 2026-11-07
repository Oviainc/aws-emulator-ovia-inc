Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-async-invoke-destinations
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-async-invoke-destinations
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|UpdateFunctionEventInvokeConfig|2xx->2xx|$.DestinationConfig.OnFailure.Destination
Issue-Key: lambda|GetFunctionEventInvokeConfig|2xx->2xx|$.DestinationConfig.OnFailure.Destination
Issue-Commit: a89f2b297f2a633390e709c28e627328b3f2666a

# Lambda event-invoke config drops DestinationConfig.OnFailure on write and read-back

> _Note: this issue was identified against the code base at commit `a89f2b297f2a633390e709c28e627328b3f2666a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionEventInvokeConfig

## Description
Two net-new causes in the lambda service sharing one fault: the asynchronous-invocation configuration's DestinationConfig.OnFailure is neither echoed on write nor persisted for the read-back, so a configured failure destination silently disappears. Both must be closed before this issue is resolved.

Cause 1 of 2 - UpdateFunctionEventInvokeConfig does not echo the OnFailure destination it was given (fixture lambda-async-invoke-destinations, step task#8).
AWS: Smithy: UpdateFunctionEventInvokeConfig POST /2019-09-25/functions/{FunctionName}/event-invoke-config, input member DestinationConfig, output shape FunctionEventInvokeConfig which also carries DestinationConfig; DestinationConfig.OnFailure targets OnFailure whose member Destination targets DestinationArn (@length 0..350, pattern admitting any service ARN or a kafka:// URI). The model's own documentation for the sibling OnSuccess states the supported async destinations are "an Amazon SNS topic, Amazon SQS queue, Lambda function, or Amazon EventBridge event bus", so an EventBridge event-bus ARN is a valid OnFailure/destination value. The recorded response is 200 with DestinationConfig.OnFailure.Destination equal to the destination ARN supplied in the request.
Emulator: answers 200 but the response's DestinationConfig has no OnFailure at all (replay: $.DestinationConfig.OnFailure.Destination missing, got None), so the destination the caller just configured is absent from the reply.
Why: a client cannot confirm the write; worse, since the value is not retained, nothing the client does later can discover that failure records have no destination. Configuration-as-code tools that diff the response against the desired state see a permanent drift and re-apply forever.
Fix: the UpdateFunctionEventInvokeConfig handler's state (persist the whole DestinationConfig, including OnFailure, not only OnSuccess) and the FunctionEventInvokeConfig serializer that must emit the nested member.
Traces: lambda-async-invoke-destinations#task#8.

Cause 2 of 2 - GetFunctionEventInvokeConfig returns a configuration with no OnFailure (fixture lambda-async-invoke-destinations, step task#9).
AWS: Smithy: GetFunctionEventInvokeConfig GET /2019-09-25/functions/{FunctionName}/event-invoke-config, readonly, output shape FunctionEventInvokeConfig with DestinationConfig. The recorded read-back returns the OnFailure destination that the preceding update set.
Emulator: 200 with DestinationConfig.OnFailure missing - the read path confirms the value was never stored (replay: $.DestinationConfig.OnFailure.Destination missing, got None).
Why: the read path is what clients and auditors use to verify that failed asynchronous invocations are being retained; it reports "no failure destination configured" for a function that has one.
Fix: the event-invoke-config state the Get handler reads from, plus the same FunctionEventInvokeConfig serializer as cause 1 - the Get path must not be left unfixed when the write path is corrected.
Traces: lambda-async-invoke-destinations#task#9.

No open issue owns these steps (the replay reports owned_by empty for both). The open issue 20261107T014900Z-lambda-sns-sqs-async-destinations-and-dead-letter.md concerns the async plane never DELIVERING to destinations; it does not cover this control-plane echo/persist gap and does not carry these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout:

Cause 1 - UpdateFunctionEventInvokeConfig, lambda-async-invoke-destinations#task#8: status 200 on both sides; $.DestinationConfig.OnFailure.Destination missing - expected the EventBridge event-bus ARN supplied by the request ('arn:aws:events:us-west-2:000000000000:event-bus/default' in this recording), got None.

Cause 2 - GetFunctionEventInvokeConfig, lambda-async-invoke-destinations#task#9: status 200 on both sides; $.DestinationConfig.OnFailure.Destination missing - expected the same persisted destination ARN, got None.

The specific event-bus ARN is this recording's value; the requirement is that whatever DestinationArn the request supplied is echoed and later read back under DestinationConfig.OnFailure.Destination.

## Steps to Reproduce
1. Clone this branch and call replay_wire_corpus().
2. Cause 1: replay fixture lambda-async-invoke-destinations, step task#8 - UpdateFunctionEventInvokeConfig with DestinationConfig.OnFailure.Destination set to an EventBridge event-bus ARN; inspect the 200 response for DestinationConfig.OnFailure.
3. Cause 2: replay fixture lambda-async-invoke-destinations, step task#9 - GetFunctionEventInvokeConfig for the same function and check that DestinationConfig.OnFailure.Destination comes back with the value set in step 2.
