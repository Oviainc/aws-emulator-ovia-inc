Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.converse-tool-use-full-cycle bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.converse-tool-use-full-cycle|run#10|AssertionError|assert "location" in tool_use["input"], f"expected a 'location' argument; got {tool_use['input']!r}"
Issue-Key: bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas|run#26|AssertionError|assert set(parsed) == {"city", "description"}, (
Issue-Commit: bb1deb844c31e7c796662ade4c70ae6e9590dcbf

# Converse/ConverseStream toolUse block carries an empty input document: the tool's declared arguments are never populated

> _Note: this issue was identified against the code base at commit `bb1deb844c31e7c796662ade4c70ae6e9590dcbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE CAUSE on the Converse / ConverseStream tool-use surface. It is a follow-on of (but NOT covered by) the open issue 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md, whose Cause 2 was "stopReason is never tool_use": the emulator now DOES honour toolChoice (it stops with `tool_use`, emits exactly one toolUse content block, names the declared tool, mints a toolUseId, opens a contentBlockStart without `input`, streams string delta fragments on the opened index and closes the block with one contentBlockStop). The replay keys of both checks below have moved past the assertions that issue carries, and the remaining divergence is a different one: the argument document itself.

Cause 1 of 1 — the toolUse `input` document is an empty JSON object, so none of the tool's declared/required arguments are present.
AWS: Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json) — ToolUseBlock.input targets Document and is @required, documented "The input to pass to the tool"; ToolSpecification.inputSchema is @required (ToolInputSchema union member `json`, top-level schema type must be `object`); ToolChoice.any is documented "The model must request at least one tool (no text is generated)" and ToolChoice.tool "The Model must request the specified tool". On ConverseStream the same value arrives as ToolUseBlockDelta.input (@required String) fragments that a client concatenates. When a tool call is forced, the arguments AWS returns are a JSON object that instantiates that inputSchema — in particular every name listed in the schema's `required` array is a key of `input`, and the streamed fragments concatenate to exactly that object. The argument VALUES are model output and are explicitly NOT the requirement here; the requirement is that `input` is the tool's argument object keyed by the schema it was given, not an empty document.
Emulator: `input` is `{}` on the non-streaming path, and the concatenated contentBlockDelta fragments are the two characters `{}` on the streaming path — for a schema declaring required ["location"] and for a schema declaring required ["city","description"] alike. The declared schema is accepted and then discarded: nothing in the request's inputSchema reaches the emitted argument document.
Why: a tool-calling/agent client reads `toolUse.input`, dispatches the named tool with it, and gets a KeyError / schema-validation failure on the very first turn — `input["location"]` does not exist. The round trip can never be driven forward: there is no argument to execute the tool with, so no toolResult turn can be synthesized, and every agent loop built against Bedrock tool use dies at step one while the API reports 200 and stopReason='tool_use'. On the streaming path the accumulating client reassembles a well-formed but empty object, which is the hardest variant to diagnose because every frame-grammar assertion passes.
Fix: the Converse/ConverseStream handler's tool-use synthesis — the stage that builds the toolUse content block must project the request's toolConfig.tools[].toolSpec.inputSchema.json into the emitted `input` document (at minimum a key per name in the schema's `required` array, with values of the declared JSON type), and the ConverseStream framing must emit that same serialized object as its ToolUseBlockDelta.input fragments so the streamed reassembly and the unary parse agree.
Traces: bedrock-runtime.converse-tool-use-full-cycle, bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock-runtime 17/67 passed. The emulator answered both calls with HTTP 200 and stopReason='tool_use', so the service is up and answering wrongly.

bedrock-runtime.converse-tool-use-full-cycle (non-streaming path):
  request: Converse modelId='amazon.nova-lite-v1:0', toolConfig={'tools':[{'toolSpec':{'name':'get_weather','inputSchema':{'json':{'type':'object','properties':{'location':{'type':'string'}},'required':['location']}}}}],'toolChoice':{'any':{}}}
  expected: output.message.content[0].toolUse.input is an object containing the key 'location'
  observed: AssertionError: expected a 'location' argument; got {}
  (the assertions before it passed: stopReason=='tool_use', exactly one toolUse block, name=='get_weather')

bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas (streaming path):
  request: ConverseStream, same model, toolSpec 'file_ticket' with inputSchema required ['city','description'], toolChoice={'tool':{'name':'file_ticket'}}
  expected: the concatenated contentBlockDelta.delta.toolUse.input fragments parse to an object whose keys are exactly {'city','description'}
  observed: AssertionError: the reassembled arguments must be exactly the schema's two properties; got [] from '{}'
  (the assertions before it passed: one messageStop with stopReason 'tool_use'; exactly one contentBlockStart naming 'file_ticket' with a toolUseId and no 'input'; fragments present and all str; no identity leaked into the fragments; every fragment on the opened contentBlockIndex; exactly one contentBlockStop for that index)

Model evidence: smithy_query shape ToolUseBlock / ToolUseBlockStart / ToolUseBlockDelta / ToolSpecification / ToolInputSchema / ToolChoice / ContentBlockStart on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — ToolUseBlock.input: Document, @required; ToolUseBlockDelta.input: String, @required; ToolSpecification.inputSchema: @required; ToolChoice union {auto, any, tool}.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite and re-replay the two check ids in trace_ids.
3. Non-streaming: Converse with modelId='amazon.nova-lite-v1:0', a single user text block, and toolConfig={'tools':[{'toolSpec':{'name':'get_weather','description':'Get the current weather for a named location.','inputSchema':{'json':{'type':'object','properties':{'location':{'type':'string'}},'required':['location']}}}}],'toolChoice':{'any':{}}}. Read output.message.content[0].toolUse.input. Expected: an object carrying the schema's required key 'location'. Observed: {}.
4. Streaming: ConverseStream with the same model and a toolSpec whose inputSchema requires two properties, toolChoice={'tool':{'name':<that tool>}}. Collect every contentBlockDelta.delta.toolUse.input fragment and concatenate them. Expected: a JSON object whose keys are the schema's required properties. Observed: the literal '{}'.
5. Confirm the two paths agree once fixed: the streamed reassembly and the non-streaming toolUse.input must have the same key set for the identical request.
