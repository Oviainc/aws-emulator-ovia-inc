Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-tagging-workflow
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-tagging-workflow
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mediapackagev2|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# TagResource and UntagResource 204 responses omit the Content-Length: 0 header

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Wire replay of simparity-wire-aws-v1 shows both mediapackagev2 tag-mutating operations returning the correct empty 204 but WITHOUT the `content-length: 0` response header AWS sends with it. This is a TWO-cause bundle; a fixer MUST close both causes below and re-replay both fixture/steps before submitting. No open mediapackagev2 issue covers either cause.

Cause 1 of 2: TagResource's 204 response has no content-length header.
  AWS: Smithy TagResource -> POST /tags/{ResourceArn}, http code 204 (explicit, not defaulted), output shape Unit (no members) — the response carries no body. Recorded behavior only for the framing: AWS terminates this empty 204 with an explicit `content-length: 0` header rather than omitting the header. The value is the fixed literal `0` for this operation (the body is always empty), not a per-run quantity.
  Emulator: answers 204 with the header absent entirely ($headers.content-length: expected '0', got None). The status code and the empty body are correct.
  Why: a client or intermediary that reads framing strictly sees a response whose body length is unstated instead of explicitly zero. HTTP-level consumers that key on the header (proxies, caching layers, SDK response-length validation, assertions that an empty mutation response is explicitly zero-length) behave differently against the emulator than against AWS, and the difference appears only at the transport layer — the parsed API result looks identical, which makes it hard to diagnose.
  Fix: the TagResource handler's empty-response construction (or the shared 204/no-content response builder the mediapackagev2 serializer uses), which must set content-length: 0 on an empty 204 rather than leaving the header off.
  Traces: mediapackagev2-tagging-workflow#task#3.

Cause 2 of 2: UntagResource's 204 response has no content-length header.
  AWS: Smithy UntagResource -> DELETE /tags/{ResourceArn}, http code 204 (explicit), @idempotent, output shape Unit. Same framing contract as cause 1: an empty 204 carrying `content-length: 0`.
  Emulator: answers 204 with the header absent ($headers.content-length: expected '0', got None).
  Why: as cause 1. It is listed as its own cause because it is a separate operation on a different HTTP method (DELETE) and is clustered separately by the replay, so a fix applied only to the POST path would leave it diverging; if both are served by one shared no-content builder, one fix closes both but both steps must still be re-replayed.
  Fix: the UntagResource handler's empty-response construction / the same shared 204 response builder.
  Traces: mediapackagev2-tagging-workflow#task#5.

## Evidence
Replay: trace_set_id simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, 96 steps, 72 passed, 24 mismatched, score 75.0%. Neither cluster carried a stub_path or an owned_by.

Cause 1 — fixture mediapackagev2-tagging-workflow step task#3, TagResource, expected status 204 / actual 204:
  $headers.content-length: missing (expected '0', got None). divergence_key service=mediapackagev2|TagResource|status=204->204|keys=$headers.content-length. Status and empty body matched; only the header is absent.

Cause 2 — mediapackagev2-tagging-workflow#task#5, UntagResource, expected 204 / actual 204:
  $headers.content-length: missing (expected '0', got None). divergence_key service=mediapackagev2|UntagResource|status=204->204|keys=$headers.content-length.

Note the expected value '0' is a fixed literal for an always-empty response, not a per-run value, so this is not a request-id/clock-style difference.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus, or reproduce at the HTTP level against the emulator endpoint:

1. CreateChannelGroup(ChannelGroupName=g) and take the group's Arn.
2. Cause 1: POST /tags/<url-encoded Arn> with body {"tags":{"k":"v"}} (i.e. TagResource). Inspect the RAW response headers, not the parsed SDK result: the response is 204 with no content-length header, where AWS sends `content-length: 0`. Fixture: mediapackagev2-tagging-workflow#task#3.
3. Cause 2: DELETE /tags/<url-encoded Arn>?tagKeys=k (i.e. UntagResource). Same observation: 204 with no content-length header. Fixture: mediapackagev2-tagging-workflow#task#5.

Pass criterion for both: the empty 204 response carries `content-length: 0` explicitly. The status must stay 204 and the body must stay empty.
