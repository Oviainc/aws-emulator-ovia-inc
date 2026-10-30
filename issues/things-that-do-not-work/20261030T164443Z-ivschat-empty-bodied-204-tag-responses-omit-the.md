Issue-Source: trace-sweep-divergence
Issue-Trace: ivschat-tagging-workflow
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivschat-tagging-workflow
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat|TagResource|2xx->2xx|$headers.content-length
Issue-Key: ivschat|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 962bc15d984e86ce56d75221c3331a6151fbca59

# ivschat empty-bodied 204 tag responses omit the content-length: 0 framing header

> _Note: this issue was identified against the code base at commit `962bc15d984e86ce56d75221c3331a6151fbca59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two net-new causes sharing one fault: ivschat's two tag-mutating operations now answer the correct 204 No Content with an empty body, but the response that reaches the client carries NO `content-length` header at all, where the recorded AWS answer frames the empty body explicitly as `content-length: 0`. The status and the (absent) body match on both causes; only the framing header is missing.

Cause 1 of 2: TagResource success response omits `content-length: 0`.
AWS: recorded behavior only for the header. Smithy (query='operation', ivschat model): TagResource is bound to POST /tags/{resourceArn}, output shape TagResourceResponse has no members, and http = {code 200, codeDefaulted true} — so the model states nothing about either the 204 or its framing. The recording of the live service shows a successful tag write answered with HTTP 204, an empty body, and the response header `content-length: 0`. The header's value is fixed text for this class of answer: an empty-bodied success states a length of exactly `0`; there is no request- or state-derived part to it.
Emulator: answers HTTP 204 with an empty body but with no `content-length` header present (replay reports `$headers.content-length: missing (expected '0', got None)`).
Why: a client, SDK or intermediary that requires an explicit framing header on a response — rather than inferring zero length from the status — sees an unframed answer. Length-driven readers and proxies that re-frame or buffer the response cannot distinguish "declared empty" from "length unknown", and any assertion on the response's framing fails even though the tag write itself landed correctly. The tags are stored and the status is right, so the impact is confined to framing; hence LOW.
Fix: the ivschat crate's response-framing seam for no-content answers, and the HTTP writer underneath it. The serializer seam already does its half — it empties the body and inserts `content-length: 0` for the no-content status — but the header does not survive to the wire, because the HTTP/1.1 writer the vendored transport serves with strips `content-length` from a 204. The fix therefore belongs in the transport/writer layer that drops the header (or in how the service frames that status so the declared length survives), not in the TagResource handler, whose status and body are already correct.
Traces: ivschat-tagging-workflow#task#3.

Cause 2 of 2: UntagResource success response omits `content-length: 0`.
AWS: recorded behavior only, as above. Smithy: UntagResource is bound to DELETE /tags/{resourceArn} with tagKeys as an @httpQuery list, output shape UntagResourceResponse has no members, http = {code 200, codeDefaulted true}. The recording shows a successful untag answered 204 with an empty body and `content-length: 0`.
Emulator: answers HTTP 204 with an empty body and no `content-length` header (`$headers.content-length: missing (expected '0', got None)`).
Why: identical to Cause 1, on the operation that removes tags — the untag is applied and the status is correct, but the answer is unframed.
Fix: the same framing seam and HTTP writer as Cause 1; both causes are one fault reached through two operations, so one fix at the framing/transport layer closes both, but BOTH fixtures below must be re-replayed to confirm it.
Traces: ivschat-tagging-workflow#task#3 and ivschat-tagging-workflow#task#5.

Relation to already-filed work: the resolved issue 20261029T171751Z-ivschat-tag-writes-answer-http-200-instead-of.md fixed the status (200 -> 204) for these same two steps and explicitly recorded the surviving framing gap as a caveat about the vendored HTTP writer. A resolved issue does not cover a live divergence, and replay still reports these two clusters as unowned (owned_by: []), so they are filed here. The open issue 20261029T171751Z-ivschat-client-error-responses-carry-no-error-envelope.md covers the error-envelope divergences on this service and is unrelated to these two success responses.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. Fixture ivschat-tagging-workflow; 2 divergence clusters, verdict `mismatch`, `stub_path` empty, `owned_by` empty.

Cause 1 — ivschat-tagging-workflow#task#3, TagResource:
  expected_status 204, actual_status 204 (status matches)
  detail: `$headers.content-length: missing (expected '0', got None)`
  divergence_key: service=ivschat|TagResource|status=204->204|keys=$headers.content-length

Cause 2 — ivschat-tagging-workflow#task#5, UntagResource:
  expected_status 204, actual_status 204 (status matches)
  detail: `$headers.content-length: missing (expected '0', got None)`
  divergence_key: service=ivschat|UntagResource|status=204->204|keys=$headers.content-length

Expected vs actual, both causes: expected a 204 response carrying header `content-length: 0` with an empty body; actual is a 204 response with an empty body and no `content-length` header. Only the header diverges — no body member mismatch is reported on either step. The expected header value `0` is fixed text for an empty-bodied answer and is not a per-run value.

Model basis (smithy_query query='operation' on the ivschat model): TagResource output_shape TagResourceResponse {} , http {method POST, uri /tags/{resourceArn}, code 200, codeDefaulted true}; UntagResource output_shape UntagResourceResponse {}, http {method DELETE, uri /tags/{resourceArn}, code 200, codeDefaulted true}. The model declares neither the 204 nor its framing, so the `content-length: 0` requirement is recorded wire behavior.

Code pointers: services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat/src/service.rs lines 98-103 (`answer_no_content` empties the body and inserts `content-length: 0` for `crate::state::NO_CONTENT_SUCCESS_STATUS`) and lines 134-149 (`serialize_output` applies it to every response of this service); lines 95-97 state that the HTTP/1.1 writer in the vendored transport removes `content-length` from a 204, which is why the header never reaches the wire.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay fixture ivschat-tagging-workflow from trace set simparity-wire-aws-v1.
2. Create a room to tag: ivschat.CreateRoom(name=<valid name matching ^[a-zA-Z0-9-_]*$>) and keep the returned room ARN.
3. Cause 1 (step task#3) — issue POST /tags/<room arn> with a tags map, e.g. ivschat.TagResource(resourceArn=<room arn>, tags={<key>: <value>}), and inspect the raw HTTP response headers (not just the SDK-parsed result). Expected: `HTTP/1.1 204 No Content` with `content-length: 0` and no body. Actual: 204 with an empty body and no `content-length` header.
4. Confirm the write itself landed with ivschat.ListTagsForResource(resourceArn=<room arn>) — the tags are present, so only the response framing is wrong.
5. Cause 2 (step task#5) — issue DELETE /tags/<room arn>?tagKeys=<key>, e.g. ivschat.UntagResource(resourceArn=<room arn>, tagKeys=[<key>]), and inspect the raw HTTP response headers. Expected: 204 with `content-length: 0`; actual: 204 with no `content-length` header.
6. Both causes must be re-replayed after a fix: ivschat-tagging-workflow#task#3 and ivschat-tagging-workflow#task#5.
