Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.invoke-model-response-stream-byte-ordering
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.invoke-model-response-stream-byte-ordering|run#7|AssertionError|assert events[0] == "messageStart", f"stream must open with messageStart, got {events!r}"
Issue-Commit: 448dbaa5987c679f4ac9bf289637b03678756bff

# InvokeModelWithResponseStream on a live Nova model emits the unary body as a single chunk: no messageStart/contentBlockDelta/contentBlockStop/messageStop event sequence

> _Note: this issue was identified against the code base at commit `448dbaa5987c679f4ac9bf289637b03678756bff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModelWithResponseStream

## Description
ONE CAUSE on the InvokeModelWithResponseStream event-stream surface. (Related open issue: 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md, whose Cause 3 reports the same single-frame framing from the frame-count/metrics angle; that issue does not carry this check's replay key, so this divergence - the ORDER and NAMES of the events inside the stream - is filed here and a fixer should treat the two as the same framing stage.)

Cause 1 of 1 - the response stream carries the NON-streaming response body in one chunk, so the per-event envelope is missing entirely.
AWS: Smithy InvokeModelWithResponseStream (POST /model/{modelId}/invoke-with-response-stream, http 200) binds output member `body` -> ResponseStream as the @required @streaming httpPayload, with `contentType` @required on header X-Amzn-Bedrock-Content-Type. The payload is therefore a SEQUENCE of application/vnd.amazon.eventstream messages, each `chunk` carrying one incremental event of the invoked model's native streaming protocol. For the Nova message-API body shape, the recorded event sequence opens with `messageStart`, is followed by content-block events (`contentBlockStart`/`contentBlockDelta`/`contentBlockStop`, the last block closed by `contentBlockStop`), then exactly one `messageStop`, with no `contentBlockDelta` after `messageStop`; the service additionally injects an `amazon-bedrock-invocationMetrics` key on a terminal chunk (recorded behavior only - it is not a modeled member). The modeled keys `output`, `stopReason` and `usage` belong to the UNARY InvokeModel/Converse response, never to a ResponseStream chunk.
Emulator: the stream yields chunk payload(s) whose only keys are ['output', 'stopReason', 'usage'] - the unary body serialized once - so the first event is `output` instead of `messageStart`, and `messageStart`, `contentBlockDelta`, `contentBlockStop` and `messageStop` never appear at all.
Why: a streaming client decodes each chunk against the model's streaming event union. It sees no `messageStart` to open the message, no deltas to append (so it renders nothing incrementally, defeating the operation's purpose), and no `messageStop` to terminate, so the turn never completes and any state machine keyed on these event names either stalls or throws on an unknown event. The divergence is structural - it is independent of what text the model generates.
Fix: the InvokeModelWithResponseStream handler's event-stream serializer / framing stage (translate the generated turn into the per-event chunk sequence messageStart -> contentBlock* -> messageStop plus the terminal invocationMetrics chunk, one eventstream message per event, instead of serializing the unary response body into a single chunk). Shared with the ConverseStream framing stage.
Traces: bedrock-runtime.invoke-model-response-stream-byte-ordering.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock-runtime 15/63 passed - the emulator answered this call with a 200 and a parseable event-stream body, so the service is up and answering wrongly.

bedrock-runtime.invoke-model-response-stream-byte-ordering [mismatch]:
  File check_bedrock_runtime_invoke_model_response_stream_byte_ordering.py, line 58, in run
  assert events[0] == "messageStart", f"stream must open with messageStart, got {events!r}"
  AssertionError: stream must open with messageStart, got ['output', 'stopReason', 'usage']

The check collected the chunk payload keys with:
  live_chunks = [json.loads(event["chunk"]["bytes"]) for event in live_resp["body"]]
  events = [k for c in live_chunks for k in c if k != "amazon-bedrock-invocationMetrics"]
Expected events: ['messageStart', ..., 'contentBlockDelta', ..., 'contentBlockStop', 'messageStop'] with exactly one 'messageStop' and no delta after it. Observed: ['output', 'stopReason', 'usage'].

Model evidence: smithy_query query=operation operation=InvokeModelWithResponseStream on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json - output {body -> ResponseStream, httpPayload, required, streaming; contentType -> MimeType header X-Amzn-Bedrock-Content-Type, required}, http {method POST, uri /model/{modelId}/invoke-with-response-stream, code 200}.

Note on scope: the same check later asserts end-of-life handling for amazon.titan-text-express-v1 (ResourceNotFoundException from InvokeModelWithResponseStream and GetFoundationModel, absence from ListFoundationModels). The run never reached those assertions, so they are NOT part of this finding's evidence.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay bedrock-runtime.invoke-model-response-stream-byte-ordering.
3. Call InvokeModelWithResponseStream with modelId='amazon.nova-lite-v1:0', contentType='application/json', accept='application/json' and body={"messages":[{"role":"user","content":[{"text":"Reply with exactly the word PONG and nothing else."}]}],"inferenceConfig":{"maxTokens":20,"temperature":0}}.
4. Decode the response payload as application/vnd.amazon.eventstream and JSON-parse each chunk's bytes, collecting the top-level key of each event (ignoring 'amazon-bedrock-invocationMetrics').
5. Expected: the first key is 'messageStart', content-block events follow, the last content-block event is 'contentBlockStop', and exactly one 'messageStop' closes the sequence with no 'contentBlockDelta' after it.
6. Observed: the keys are ['output', 'stopReason', 'usage'] - the unary InvokeModel/Converse response body delivered as a single chunk, with none of the streaming event names present.
