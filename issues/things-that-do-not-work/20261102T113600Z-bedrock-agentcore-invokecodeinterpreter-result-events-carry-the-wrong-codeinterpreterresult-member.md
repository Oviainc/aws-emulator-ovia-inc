Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-code-interpreter-command-execution
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-code-interpreter-command-execution bedrock-agentcore-code-interpreter-file-operations
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|2xx->2xx|$.__wire_eventstream__[*].keys[*]
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# InvokeCodeInterpreter result events carry the wrong CodeInterpreterResult member set (2 causes)

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeCodeInterpreter

## Description
TWO net-new root causes, both in how InvokeCodeInterpreter serializes the `result` event of its streaming payload: the emulator populates an optional CodeInterpreterResult member AWS leaves out, and omits one AWS always reports. Status 200 and the event framing match on both sides; only the member set of the result diverges. A fixer must close BOTH.

AWS (shared model facts): Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json): InvokeCodeInterpreter is POST /code-interpreters/{codeInterpreterIdentifier}/tools/invoke with @http code 200; its output member `stream` is @required @httpPayload @streaming targeting the union CodeInterpreterStreamOutput, whose success member is `result` -> CodeInterpreterResult { content: ContentBlockList @required, structuredContent: ToolResultStructuredContent (optional), isError: Boolean (optional) }. `isError` is documented "Indicates whether the result represents an error. If true, the content contains error messages or exception information. If false, the content contains successful execution results"; `structuredContent` is documented as additional execution metadata (taskId/taskStatus/stdout/stderr/exitCode/executionTime).

Cause 1 of 2: the emulator adds `structuredContent` to a result event where AWS emits only the content/isError pair.
AWS: recorded behavior only — for the command-execution tool calls in this fixture AWS's `result` event carries exactly TWO members and no `structuredContent` at all; which optional members appear is per tool class, and for this class AWS reports none.
Emulator: the same call serializes a third member, `structuredContent`, into the result event. (The emulator is known to duplicate its own "nothing ran" sentence into structuredContent.stderr — see the open behavioral issue 20261102T063922Z-bedrock-agentcore-code-interpreter-tool-plane-diverges-python-executecode-the.md, whose Causes 2-4 describe that tool plane running nothing. That issue carries behavioral check ids and NOT this replay key, and it does not state the wire requirement that the result member set must match; these steps must be replayed even if it is closed first.)
Why: a client that enumerates the result's members, or that treats the presence of structuredContent as "this tool reported structured execution metadata", branches into a metadata path AWS never offers and reads emulator-authored diagnostics as if they were execution metadata.
Fix: the result serializer / ToolOutcome->CodeInterpreterResult conversion in InvokeCodeInterpreter's handler (src/ops/invoke_code_interpreter.rs in services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore): emit `structuredContent` only for the tool classes whose recorded results carry it, and never as a carrier for a diagnostic sentence.
Traces: bedrock-agentcore-code-interpreter-command-execution#task#3 (cluster also covers task#4).

Cause 2 of 2: the emulator OMITS `isError` from a result event where AWS reports it.
AWS: recorded behavior only — for the file-tool calls in this fixture AWS's `result` event reports `isError` as its second member alongside `content`, i.e. the success/failure flag is present on a normal (non-error) tool result, not only when the tool failed.
Emulator: the result event has no `isError` member at that position, so the flag is absent from the serialized result.
Why: `isError` is the ONLY in-band signal that distinguishes a tool that failed from one that succeeded on a 200 response; a client reading result["isError"] gets a missing key and must either guess success or crash (the companion behavioral issue above shows clients doing exactly that read). Treating a missing flag as false also silently converts a tool failure into a success.
Fix: the same result serializer in src/ops/invoke_code_interpreter.rs — always serialize `isError` for the tool classes whose recorded results carry it, including the false/success case, rather than skipping the member when the value is false or unset.
Traces: bedrock-agentcore-code-interpreter-file-operations#task#2 (cluster also covers task#3).

## Evidence
Both from the wire replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false). Status 200 expected and 200 actual in both clusters; the divergence is only in the member set of the streaming `result` event.

Cause 1 — fixture bedrock-agentcore-code-interpreter-command-execution, step task#3 (cluster members: task#3, task#4), InvokeCodeInterpreter [mismatch]:
  $.__wire_eventstream__[0].keys[2]: extra (expected None, got 'structuredContent')
The recorded event has two members at indices 0-1 and nothing at index 2; the emulator supplies a third, 'structuredContent'.

Cause 2 — fixture bedrock-agentcore-code-interpreter-file-operations, step task#2 (cluster members: task#2, task#3), InvokeCodeInterpreter [mismatch]:
  $.__wire_eventstream__[0].keys[1]: missing (expected 'isError', got None)
The recorded event reports 'isError' as the member at index 1; the emulator's event has no such member there.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus simparity-wire-aws-v1.
2. Cause 1 — bedrock-agentcore-code-interpreter-command-execution, task#3 (and task#4): StartCodeInterpreterSession on aws.codeinterpreter.v1, then POST /code-interpreters/aws.codeinterpreter.v1/tools/invoke for the fixture's command-execution tool call and inspect the first event of the `stream` payload: the emulator's result object carries a third member 'structuredContent' that the recorded AWS result does not have.
3. Cause 2 — bedrock-agentcore-code-interpreter-file-operations, task#2 (and task#3): in a started session, invoke the fixture's file tool and inspect the first `result` event: the member 'isError' that AWS reports at index 1 is absent from the emulator's result.
