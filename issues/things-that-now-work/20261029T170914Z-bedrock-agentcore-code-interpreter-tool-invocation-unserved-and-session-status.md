Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-code-interpreter-basic-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-code-interpreter-basic-lifecycle bedrock-agentcore-code-interpreter-command-execution bedrock-agentcore-code-interpreter-file-operations bedrock-agentcore-code-interpreter-double-stop-conflict bedrock-agentcore-code-interpreter-multi-session-listing
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|2xx->4xx|$.__wire_eventstream__
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|2xx->4xx|$headers.content-type
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|2xx->4xx|status
Issue-Key: bedrock-agentcore|GetCodeInterpreterSession|2xx->2xx|$.status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Code-interpreter tool invocation unserved and session status never becomes TERMINATED after stop

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeCodeInterpreter

## Description
Two net-new causes in the code-interpreter session lifecycle: the tool-invocation call on a live session is not served at all, and stopping a session never moves its status out of READY. Together they mean a code-interpreter session can be started but can neither do work nor be observed to end.

Cause 1 of 2: InvokeCodeInterpreter on a session the emulator itself just created returns a 404 JSON error instead of an event-stream result.
AWS: Smithy: InvokeCodeInterpreter POST /code-interpreters/{codeInterpreterIdentifier}/tools/invoke, @http code 200; input name: ToolName (required enum) + arguments: ToolArguments, sessionId bound to header x-amzn-code-interpreter-session-id; output member stream: CodeInterpreterStreamOutput is @httpPayload @required @streaming, a union whose success member is result: CodeInterpreterResult { content (required), structuredContent, isError }. A streaming union payload is serialized as an AWS event stream, so the success response is HTTP 200 with Content-Type application/vnd.amazon.eventstream carrying a 'result' event exposing content / structuredContent / isError. ResourceNotFoundException(404) is modeled only for an identifier or session that does not exist — here the session was created by the immediately preceding StartCodeInterpreterSession step, so 404 is not the modeled outcome.
Emulator: HTTP 404 with Content-Type application/x-amz-json-1.1 and no event stream at all ($.__wire_eventstream__ absent). The operation is reachable but either does not route or cannot resolve the live session, and it never produces the streaming payload the output shape requires.
Why: every use of a code interpreter — running code, executing commands, reading and writing files — goes through this one operation, so a client can start a session and then do nothing with it. Because the reply is a plain JSON error rather than an event stream, SDK clients fail while deserializing the streaming payload instead of surfacing a usable error, and the structured result (isError / structuredContent) that agent code branches on is never produced. This is the widest cluster in the corpus: 9 steps across 4 fixtures (basic-lifecycle, command-execution, file-operations).
Fix: InvokeCodeInterpreter's handler and its serializer — resolve the session from the x-amzn-code-interpreter-session-id header against started-session state, execute the named ToolName with its arguments, and emit the CodeInterpreterStreamOutput union as an application/vnd.amazon.eventstream 'result' event with content, structuredContent and isError.
Traces: bedrock-agentcore-code-interpreter-basic-lifecycle#task#3, plus bedrock-agentcore-code-interpreter-command-execution#task#2..4 and bedrock-agentcore-code-interpreter-file-operations#task#2..5 in the same cluster.

Cause 2 of 2: after StopCodeInterpreterSession, GetCodeInterpreterSession still reports status READY.
AWS: Smithy: GetCodeInterpreterSession returns status: CodeInterpreterSessionStatus, an enum whose only values are READY and TERMINATED; StopCodeInterpreterSession PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/stop returns lastUpdatedAt and is @idempotent. A stopped session is therefore TERMINATED on subsequent reads, and a second stop is a ConflictException(409) case rather than a transition back; the recorded responses show 200 with $.status TERMINATED after the stop.
Emulator: 200 with $.status READY — the stop call is accepted but the session record's status field is never transitioned, so the stored state keeps the start-time value forever.
Why: a client cannot tell a stopped session from a live one. Poll-until-TERMINATED teardown loops never finish, double-stop conflict detection cannot trigger (the state it keys on never changes), and session listings report terminated sessions as available — so callers keep routing work to a session that is gone.
Fix: StopCodeInterpreterSession's state update — set the session's status to TERMINATED (and stamp lastUpdatedAt) in code-interpreter session state, so GetCodeInterpreterSession and the session listings read it back.
Traces: bedrock-agentcore-code-interpreter-basic-lifecycle#teardown#5, plus bedrock-agentcore-code-interpreter-double-stop-conflict#task#3 and bedrock-agentcore-code-interpreter-multi-session-listing#task#6..7 in the same cluster.

The open issue 20261027T162759Z (AgentCore code-interpreter sessions publish no ActiveSessionCount metric to CloudWatch) touches code-interpreter sessions but is about CloudWatch metric emission, not about tool invocation or session status, so neither cause here is covered by it.

## Evidence
Cause 1 InvokeCodeInterpreter (fixture bedrock-agentcore-code-interpreter-basic-lifecycle#task#3, cluster of 9 steps across 4 fixtures): status expected 200, got 404; $.__wire_eventstream__ expected [{'type': 'result', 'keys': ['content', 'isError', 'structuredContent']}], got None; $headers.content-type expected 'application/vnd.amazon.eventstream', got 'application/x-amz-json-1.1'.
Cause 2 GetCodeInterpreterSession (fixture bedrock-agentcore-code-interpreter-basic-lifecycle#teardown#5, cluster of 5 steps): status 200 on both sides, $.status expected 'TERMINATED', got 'READY'. TERMINATED and READY are the only two values of the modeled CodeInterpreterSessionStatus enum, so this is a state-transition defect, not a per-run value difference.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture bedrock-agentcore-code-interpreter-basic-lifecycle, step task#3 — StartCodeInterpreterSession, then POST /code-interpreters/<id>/tools/invoke with the session id in x-amzn-code-interpreter-session-id and a ToolName plus arguments. The emulator answers 404 application/x-amz-json-1.1; AWS answers 200 application/vnd.amazon.eventstream with a 'result' event. Repeats at bedrock-agentcore-code-interpreter-command-execution#task#2..4 and bedrock-agentcore-code-interpreter-file-operations#task#2..5.
3. Cause 2: same fixture, step teardown#5 — StopCodeInterpreterSession, then GetCodeInterpreterSession for that sessionId and read $.status. The emulator reports READY; AWS reports TERMINATED. Repeats at bedrock-agentcore-code-interpreter-double-stop-conflict#task#3 and bedrock-agentcore-code-interpreter-multi-session-listing#task#6..7.
