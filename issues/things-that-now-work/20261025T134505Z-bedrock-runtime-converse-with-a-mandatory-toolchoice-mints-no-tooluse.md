Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected|run#5|AssertionError|assert first["stopReason"] == "tool_use", (
Issue-Commit: 93db7882a7dd57b95771a5327951c720d44e45a1

# Converse with a mandatory toolChoice mints no toolUse block (stopReason 'end_turn'), so the cross-turn toolUseId contract on ToolResultBlock is never enforced

> _Note: this issue was identified against the code base at commit `93db7882a7dd57b95771a5327951c720d44e45a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE CAUSE. Related open issue: 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md (its Cause 2 describes the same toolChoice neglect) — that issue does NOT carry this check's replay key, so this work item exists to make the toolUseId leg of the contract replayable and must be closed on its own trace_ids.

Cause 1 of 1: Converse ignores toolConfig.toolChoice and therefore never mints a server-side toolUseId, which makes the whole toolResult-addressing contract unobservable.

AWS: Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json) models ConverseRequest.toolConfig -> ToolConfiguration with ToolChoice, a union {auto, any, tool}; `tool` is documented "The Model must request the specified tool" and `any` "The model must request at least one tool (no text is generated)". With a mandatory choice the turn ends with ConverseResponse.stopReason = `tool_use` (StopReason enum values: end_turn, tool_use, max_tokens, stop_sequence, guardrail_intervened, content_filtered, malformed_model_output, malformed_tool_use, model_context_window_exceeded) and output.message.content carries exactly one toolUse block; ToolUseBlock requires toolUseId (@length 1..64, pattern ^[a-zA-Z0-9_.:-]+$), name and input. On the following turn ToolResultBlock.toolUseId is @required with the same constraints and must address an id the service actually minted: a toolResult naming an id that was never issued in this message history is refused with ValidationException (errorKind client, httpError 400), and the message names both the id the service expected and the id it was given — that is the rule; the two ids themselves are per-invocation values taken from the request and the server's own state, never fixed text.

Emulator: the first Converse call, sent with toolConfig.toolChoice={"tool":{"name":"get_stock_price"}}, returns HTTP 200 with stopReason 'end_turn' and a single text content block (the emulator's placeholder completion), and no toolUse block at all. Because no toolUseId is ever minted, the emulator is never asked the follow-up question: whether a toolResult addressed to an id it never issued is refused with 400 ValidationException naming both ids. The contract is therefore unenforced AND unobservable.

Why: an agent/tool-calling client that forces a specific tool gets a plain text answer instead of the toolUse block it must execute, so it deadlocks or mis-parses. Worse, downstream the emulator accepts any message history: a client (or a test of a client) can feed a toolResult whose toolUseId was never issued and the emulator serves it, while AWS refuses the turn — so the conversation the model is shown is not the one it produced, and this over-permissiveness is invisible until production.

Fix: the Converse/ConverseStream handler's toolConfig stage — honour toolChoice `any`/`tool` by emitting one toolUse content block with a freshly minted toolUseId matching the modeled constraints and setting stopReason='tool_use'; and add the cross-turn validation of ToolResultBlock.toolUseId against the ids present in the submitted message history, refusing a mismatch with 400 ValidationException whose message names the expected and the supplied id. These are request/response envelope decisions and are independent of what text any model generates.

Traces: bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock-runtime 16/67 passed. The emulator answered this call with a 200, so the service is up and answering wrongly.

[mismatch] bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected
  check_bedrock_runtime_bedrock_runtime_converse_tool_result_with_an_unknown_tool_use_id_rejected.py line 97 (assert first["stopReason"] == "tool_use")
  AssertionError: a mandatory toolChoice must stop the turn to call the tool; got 'end_turn' content=[{'text': 'No language model runs in this Amazon Bedrock Runtime emulator, so amazon.nova-lite-v1:0 generated no text for this request. The request carried 1 message(s) and 134 input token(s).'}]

Expected: stopReason 'tool_use' and exactly one toolUse block whose toolUseId the service minted; then, for the follow-up turn built from that history with one character of the id changed, HTTP 400 ValidationException whose message names the minted id and the supplied id.
Observed: HTTP 200, stopReason 'end_turn', zero toolUse blocks; the negative leg is never reached.

Model evidence: smithy_query query=shape shape=ToolChoice,ToolResultBlock,ToolUseBlock,StopReason on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json.

Note on scope: the assertions in this check about the model's generated wording (that the tool's price value reaches the final answer) are live-model values and are NOT part of this finding; the requirement filed here is the response envelope (stopReason, toolUse block, minted toolUseId) and the 400 ValidationException class and message rule.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected.
3. Call Converse with modelId=amazon.nova-lite-v1:0, inferenceConfig={maxTokens:500, temperature:0.0}, one user message, and toolConfig={'tools':[{'toolSpec':{'name':'get_stock_price','description':...,'inputSchema':{'json':{...}}}}], 'toolChoice':{'tool':{'name':'get_stock_price'}}}.
   Expected: 200 with stopReason='tool_use' and exactly one toolUse content block carrying a server-minted toolUseId (1..64 chars, ^[a-zA-Z0-9_.:-]+$). Observed: 200 with stopReason='end_turn' and a single text block, no toolUse.
4. Build the follow-up history [user turn, the assistant message from step 3, {'role':'user','content':[{'toolResult':{'toolUseId':<minted id>,'content':[{'json':{...}}],'status':'success'}}]}] and call Converse again with the same toolConfig minus toolChoice. Expected: 200.
5. Re-issue step 4 with the last character of toolUseId changed. Expected: 400 ValidationException whose message names both the minted id and the supplied id. Observed: unreachable today because step 3 mints no id; once step 3 is fixed, assert the refusal rather than a pass-through 200.
