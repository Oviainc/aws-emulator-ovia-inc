Issue-Source: trace-sweep-divergence
Issue-Trace: ivschat-tagging-workflow
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivschat-tagging-workflow
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat|TagResource|2xx->2xx|status
Issue-Key: ivschat|UntagResource|2xx->2xx|status
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# ivschat tag writes answer HTTP 200 instead of the recorded 204 No Content

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two net-new causes sharing one fault: ivschat's two tag-mutating operations return a success status of 200 where AWS answers 204 No Content. Both have an empty output body, and the recording shows the status is the only thing the response conveys.

Cause 1 of 2: TagResource success status.
AWS: recorded behavior only for the status. The Smithy model binds TagResource to POST /tags/{resourceArn} with no @http code override, so the model's own value is the default 200 (reported as codeDefaulted) and its output shape TagResourceResponse has no members; the recording of the live service shows a successful tag write answered with HTTP 204 and an empty body. The sibling operations that AWS does model as 204 (DeleteRoom, DeleteLoggingConfiguration, both @http code 204) confirm ivschat uses 204 for empty-bodied successes.
Emulator: answers HTTP 200 (with an empty body) for the same successful tag write.
Why: a client or proxy that branches on the status — treating 204 as "applied, nothing to read" and 200 as "a document follows" — attempts to parse a body that is not there, and any assertion or conditional-request logic keyed on 204 misfires. The tags themselves are stored correctly, so only the status diverges; hence MEDIUM.
Fix: the ivschat crate's TagResource handler / its response builder — the success path must emit 204 No Content rather than 200.
Traces: ivschat-tagging-workflow#task#3.

Cause 2 of 2: UntagResource success status.
AWS: recorded behavior only, as above. UntagResource is bound to DELETE /tags/{resourceArn} with tagKeys as an @httpQuery list and an empty output shape (UntagResourceResponse), model code defaulted to 200; the recording shows a successful untag answered 204 with an empty body.
Emulator: answers HTTP 200.
Why: as Cause 1 — an empty 200 where 204 is expected, on the operation that removes tags.
Fix: the ivschat crate's UntagResource handler / its response builder — success must be 204 No Content.
Traces: ivschat-tagging-workflow#task#5.

## Evidence
Replay of simparity-wire-aws-v1, fixture ivschat-tagging-workflow. Both causes diverge in status only; the response bodies are empty on both sides and no member mismatch is reported.

Cause 1 — ivschat-tagging-workflow#task#3, TagResource:
  status: expected 204, got 200
  divergence_key: service=ivschat|TagResource|status=204->200|keys=status

Cause 2 — ivschat-tagging-workflow#task#5, UntagResource:
  status: expected 204, got 200
  divergence_key: service=ivschat|UntagResource|status=204->200|keys=status

Model basis (Smithy, query='operation'): TagResource http = {method POST, uri /tags/{resourceArn}, code 200, codeDefaulted true}, output_shape TagResourceResponse with no members; UntagResource http = {method DELETE, uri /tags/{resourceArn}, code 200, codeDefaulted true}, output_shape UntagResourceResponse with no members. The model therefore says nothing explicit about 204 — the 204 requirement is the recorded wire behavior. For contrast, DeleteRoom and DeleteLoggingConfiguration carry an explicit @http code 204 in the same model.

## Steps to Reproduce
1. Boot the emulator and replay fixture ivschat-tagging-workflow from simparity-wire-aws-v1.
2. Create a room: ivschat.create_room(name=<valid name>) and keep its ARN.
3. Cause 1 — ivschat.tag_resource(resourceArn=<room arn>, tags={<key>: <value>}) and inspect the HTTP status line. Expected 204 with an empty body; the emulator returns 200. (step task#3)
4. Confirm the write landed with ivschat.list_tags_for_resource(resourceArn=<room arn>) — the tags are present, so only the status is wrong.
5. Cause 2 — ivschat.untag_resource(resourceArn=<room arn>, tagKeys=[<key>]) and inspect the HTTP status line. Expected 204 with an empty body; the emulator returns 200. (step task#5)

## Resolution

Both causes were one fault and have one fix: a successful call of this service whose answer
conveys nothing but its status is answered `204 No Content` with no body. The status itself is
single-homed on `crate::state::NO_CONTENT_SUCCESS_STATUS` and read by both tag mutations'
`Operation::SUCCESS_STATUS`; the framing is stated at the one place every response of this
service is serialized (`Service::override_protocol` in `src/service.rs` serves the model's own
protocol with a `204` emptied of the `{}` its member-less output structure renders, framed
`content-length: 0`), so it holds for every operation of this model — the explicitly modelled
204s of `DeleteRoom` and `DeleteLoggingConfiguration` included — and not only for the two that
prompted it. `ListTagsForResource` keeps its 200: it reports the `tags` map.

CAVEAT: a client reached over HTTP/1.1 still does not see `content-length: 0`, because the HTTP
writer the vendored transport serves with removes that header from a 204. That writer is under
the read-only vendored tree, so the wire-level half of the framing is a platform limitation
rather than something this service can state its way out of; the serializer seam it does own is
pinned by `tests/all/empty_success_answers.rs`.

