Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appflow.concurrent-start-flow-second-call-conflicts
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow.concurrent-start-flow-second-call-conflicts|run#18|AssertionError|assert errors[0] == ("ConflictException", "Conflicts detected while executing request"), (
Issue-Commit: 6f0c16a0b241f53b494cf0d5c14f6ff644768faf

# AppFlow StartFlow: ConflictException message for an already-running flow is emulator prose, not AWS's conflict sentence

> _Note: this issue was identified against the code base at commit `6f0c16a0b241f53b494cf0d5c14f6ff644768faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartFlow

## Description
AppFlow answers StartFlow on the wire and correctly refuses a second, concurrent StartFlow on the same on-demand flow with HTTP 409 / ConflictException, but the error MESSAGE it serves is emulator-authored prose instead of the sentence AWS sends for this conflict class. 1 cause.

Cause 1 of 1: the StartFlow already-running ConflictException carries the wrong message text.
AWS: Smithy (services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json): StartFlow input flowName @required @length 0..256 @pattern ^[a-zA-Z0-9][\w!@#.-]+$; modeled errors ConflictException (httpError 409), ResourceNotFoundException (404), ServiceQuotaExceededException (402), InternalServerException (500); http POST /start-flow. The model does not state the message text, so recorded behavior only: when two near-simultaneous StartFlow calls target one on-demand flow, exactly one is accepted and returns an executionId, and the loser is refused 409 ConflictException whose message is the FIXED text "Conflicts detected while executing request" with NO variable parts at all — no flow name, no execution id, no trailing period.
Emulator: the same 409 and the same error code ConflictException, but the message is "Flow <flowName> is already running and cannot be started again" — different fixed text, and it interpolates the request's flowName into a message AWS keeps parameter-free (observed as 'Flow sim2real-af-flow-9ca69029ca05 is already running and cannot be started again'; the name portion is the flowName from the request, the rest is the emulator's own fixed text).
Why: the code and status being right makes this divergence silent. A client or test that matches AWS's conflict sentence to distinguish "another run is already in flight" from the other ConflictException causes AppFlow raises on StartFlow (an already-activated scheduled flow, a flow being modified) does not recognise the emulator's text and either mis-classifies the refusal or fails an equality assertion, even though the concurrency semantics themselves are correct.
Fix: the appflow StartFlow handler's already-running/conflict branch, where the ConflictException message is built — it must emit AWS's parameter-free conflict sentence rather than formatting the flow name into custom prose.
Traces: appflow.concurrent-start-flow-second-call-conflicts (assertion at check line 57 on errors[0]).

Scope note: the SAME check also later asserts the winning execution reaches executionStatus=Successful; that second failure mode (every AppFlow execution terminating in Error because the emulator performs no transfer) is already covered as Cause 2 of open issue 20261029T194452Z-appflow-flow-lifecycle-wrong-initial-flowstatus-executions-always.md and is NOT re-filed here. That issue does not carry this message-text cause (its StartFlow/ConflictException material concerns the wrongly-Active scheduled flow produced by its Cause 1), so this issue is net-new; fixing it is only the message text on the already-running refusal.

## Evidence
appflow.concurrent-start-flow-second-call-conflicts — AssertionError: expected the loser's error to be ConflictException, got [('ConflictException', 'Flow sim2real-af-flow-9ca69029ca05 is already running and cannot be started again')]. The check asserts errors[0] == ('ConflictException', 'Conflicts detected while executing request').

Expected (AWS): HTTP 409, Error.Code "ConflictException", Error.Message "Conflicts detected while executing request" — the whole message is fixed text; there are no request- or state-derived parts.
Actual (emulator): HTTP 409, Error.Code "ConflictException" (both correct), Error.Message "Flow <flowName> is already running and cannot be started again" — fixed text "Flow " / " is already running and cannot be started again" with <flowName> taken from the StartFlow request (the recording's 'sim2real-af-flow-9ca69029ca05' is a per-run generated name, not part of the requirement).

The concurrency outcome itself matched AWS: exactly one of the two racers was accepted (one executionId) and exactly one was refused, so only the message diverges.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite); re-run appflow.concurrent-start-flow-second-call-conflicts.
2. Create two S3 buckets, attach the AppFlow source/destination bucket policies, and put an object under the source prefix.
3. CreateFlow with triggerConfig={'triggerType': 'OnDemand'}, an S3 source config on the source prefix and an S3 destination config on the destination prefix, with passthrough tasks.
4. Issue two StartFlow(flowName=<flow>) calls concurrently (e.g. ThreadPoolExecutor with 2 workers).
5. Inspect the refused call's ClientError: observe HTTP 409 / Code 'ConflictException' with Message 'Flow <flowName> is already running and cannot be started again' where AWS sends Message 'Conflicts detected while executing request'.
