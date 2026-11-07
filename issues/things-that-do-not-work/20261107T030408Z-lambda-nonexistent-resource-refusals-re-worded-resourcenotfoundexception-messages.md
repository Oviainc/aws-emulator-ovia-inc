Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker lambda-lvau-traj-wrong-function-identifier lambda-snx-traj-cross-region-function-arn lambda-async-invoke-destinations
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateEventSourceMapping|4xx->2xx|$.Message
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|$.Message
Issue-Key: lambda|GetFunctionConfiguration|4xx->4xx|$.Message
Issue-Key: lambda|UpdateFunctionEventInvokeConfig|4xx->4xx|$.Message
Issue-Related: 20261029T233239Z-lambda-event-source-mappings-are-created-for-sources.md
Issue-Commit: a89f2b297f2a633390e709c28e627328b3f2666a

# Lambda nonexistent-resource refusals: re-worded ResourceNotFoundException messages and one missing 404

> _Note: this issue was identified against the code base at commit `a89f2b297f2a633390e709c28e627328b3f2666a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Four net-new causes, all in the lambda service, sharing one fault: when a request names a resource that does not exist in this region, the emulator either does not refuse at all or refuses with its own invented ResourceNotFoundException message instead of the message AWS words for that error class. All four must be closed before this issue is resolved.

Cause 1 of 4 - CreateEventSourceMapping does not validate EventSourceArn against the broker/queue it names (fixture lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker, step task#7).
AWS: Smithy: CreateEventSourceMapping POST /2015-03-31/event-source-mappings, success code 202, errors include ResourceNotFoundException (errorKind client, httpError 404, members Type and Message, doc "The resource specified in the request does not exist."). The recorded response for an EventSourceArn naming an Amazon MQ broker that does not exist is HTTP 404 with Type "User", Message "The broker resource: eventSourceArn you requested does not exist." and an x-amzn-errortype header carrying the error shape. Fixed text of that message is "The broker resource: " ... " you requested does not exist."; its one variable part is the name of the request member that named the missing broker (here the input member EventSourceArn, rendered by AWS in its wire/lowerCamel form "eventSourceArn").
Emulator: answers HTTP 202 and creates the mapping, with no error body at all (replay: status expected 404, got 202; $.Type, $.Message and the x-amzn-errortype header all missing).
Why: a client that wires an event source mapping to a deleted or mistyped broker is told the mapping was created; it then polls nothing forever, and the client's ResourceNotFoundException handling - the only path that reports the typo - never runs.
Fix: CreateEventSourceMapping's validation in the lambda operation handler: resolve EventSourceArn against the event-source state before the mapping is persisted and raise the modeled 404 instead of accepting it.
Traces: lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker#task#7.

Cause 2 of 4 - UpdateFunctionConfiguration with a function ARN whose region is not the endpoint's region (fixture lambda-lvau-traj-wrong-function-identifier, step task#6).
AWS: Smithy: UpdateFunctionConfiguration PUT /2015-03-31/functions/{FunctionName}/configuration, FunctionName target FunctionName whose pattern admits a full "arn:aws:lambda:<region>:<account>:function:<name>" identifier; errors include ResourceNotFoundException (404). The recorded response for an ARN in another region is HTTP 404 with the message "Functions from 'us-east-1' are not reachable in this region ('us-west-2')". Fixed text: "Functions from '" ... "' are not reachable in this region ('" ... "')"; the two variable parts are the region parsed out of the supplied ARN and the region of the endpoint serving the request.
Emulator: same 404 status, but Message "Function not found: <the supplied ARN>" - the generic not-found wording, which never tells the caller that the identifier was cross-region.
Why: a client that mis-copies an ARN between regions is told its function does not exist, so it recreates or re-deploys the function in the wrong region instead of correcting the ARN; log/alert matching on AWS's cross-region sentence never fires.
Fix: the shared function-identifier resolution used by the lambda handlers (ARN parsing plus its refusal construction): when the parsed ARN's region differs from the serving region, emit the cross-region ResourceNotFoundException message rather than the generic not-found one.
Traces: lambda-lvau-traj-wrong-function-identifier#task#6.

Cause 3 of 4 - GetFunctionConfiguration has the same cross-region gap (fixture lambda-snx-traj-cross-region-function-arn, step task#7).
AWS: Smithy: GetFunctionConfiguration GET /2015-03-31/functions/{FunctionName}/configuration, FunctionName target NamespacedFunctionName (pattern admits a full ARN), errors include ResourceNotFoundException (404). Recorded response: 404 with "Functions from 'us-east-1' are not reachable in this region ('us-west-2')" - same template and same variable parts as cause 2.
Emulator: 404 with Message "Function not found: <the supplied ARN>".
Why: same as cause 2 on the read path - a cross-region read is indistinguishable from a deleted function.
Fix: same resolution component as cause 2; the read path must use it too, so fixing one must not leave the other.
Traces: lambda-snx-traj-cross-region-function-arn#task#7.

Cause 4 of 4 - UpdateFunctionEventInvokeConfig for a qualifier that does not exist (fixture lambda-async-invoke-destinations, step task#10).
AWS: Smithy: UpdateFunctionEventInvokeConfig POST /2019-09-25/functions/{FunctionName}/event-invoke-config with Qualifier bound to the query string, errors include ResourceNotFoundException (404). Recorded response when the qualifier names no version or alias: 404 with Message "The function doesn't exist." - the whole sentence is fixed text, with no variable part; AWS deliberately does not echo the identifier here.
Emulator: 404 with Message "Function not found: <function ARN>:<qualifier>", i.e. its own wording plus an echo of the request identifier.
Why: clients and tooling that branch on AWS's exact event-invoke-config not-found sentence do not recognise the refusal, and the echoed identifier is not something AWS emits for this error class.
Fix: the UpdateFunctionEventInvokeConfig handler's refusal path (and the event-invoke-config error construction it shares with the Get/Delete/Put forms) must emit AWS's fixed sentence.
Traces: lambda-async-invoke-destinations#task#10.

No open issue owns any of these four steps (the replay reports owned_by empty for each); open lambda issues such as 20261106T225443Z-lambda-7-net-new-causes-createfunction-publishlayerversion-accept.md concern other re-worded refusals and do not carry these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout:

Cause 1 - CreateEventSourceMapping, lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker#task#7: expected status 404 with $.Type 'User' and $.Message 'The broker resource: eventSourceArn you requested does not exist.' plus an x-amzn-errortype header; actual status 202 with none of those keys present.

Cause 2 - UpdateFunctionConfiguration, lambda-lvau-traj-wrong-function-identifier#task#6: $.Message value_mismatch - expected "Functions from 'us-east-1' are not reachable in this region ('us-west-2')", got 'Function not found: arn:aws:lambda:us-east-1:000000000000:function:sim2real-probe-lvau-id-fec60728' (status 404 both sides).

Cause 3 - GetFunctionConfiguration, lambda-snx-traj-cross-region-function-arn#task#7: $.Message value_mismatch - expected "Functions from 'us-east-1' are not reachable in this region ('us-west-2')", got 'Function not found: arn:aws:lambda:us-east-1:000000000000:function:sim2real-probe-snx-xr-fn-5b07da92' (status 404 both sides).

Cause 4 - UpdateFunctionEventInvokeConfig, lambda-async-invoke-destinations#task#10: $.Message value_mismatch - expected "The function doesn't exist.", got 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-probe-lambda-fn-async-ecbc42a0:nonexistent-qualifier' (status 404 both sides).

The ARNs, function names and region pair above are this recording's values filling the templates; the requirement is the status, the error class and the message template, not those literals.

## Steps to Reproduce
1. Replay the wire corpus against the emulator: replay_wire_corpus() after cloning this branch.
2. Cause 1: replay fixture lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker, step task#7 - CreateEventSourceMapping with an EventSourceArn naming an Amazon MQ broker that does not exist; observe 202 instead of the 404 ResourceNotFoundException.
3. Cause 2: replay fixture lambda-lvau-traj-wrong-function-identifier, step task#6 - UpdateFunctionConfiguration with a function ARN whose region differs from the endpoint region; compare $.Message with the cross-region sentence.
4. Cause 3: replay fixture lambda-snx-traj-cross-region-function-arn, step task#7 - GetFunctionConfiguration with the same cross-region ARN form.
5. Cause 4: replay fixture lambda-async-invoke-destinations, step task#10 - UpdateFunctionEventInvokeConfig with Qualifier naming no existing version or alias; compare $.Message with "The function doesn't exist.".
