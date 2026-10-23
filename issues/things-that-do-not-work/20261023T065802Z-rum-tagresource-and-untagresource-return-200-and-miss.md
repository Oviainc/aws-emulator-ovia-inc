Issue-Source: trace-sweep-divergence
Issue-Trace: rum-app-monitor-lifecycle-with-tags
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-app-monitor-lifecycle-with-tags rum-empty-and-boundary-tag-operations rum-multiple-app-monitors-tag-isolation rum-tag-unicode-value-round-trip rum-tag-value-unicode-and-empty-string-preserved
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|TagResource|2xx->2xx|status
Issue-Key: rum|UntagResource|2xx->2xx|status
Issue-Key: rum|UntagResource|2xx->5xx|$
Issue-Key: rum|UntagResource|2xx->5xx|status
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM TagResource and UntagResource return 200 and miss empty-tagKeys route

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three root causes in the rum emulator's tagging surface. All three are in service rum; a fixer must close ALL THREE before submitting.

=== Cause 1 of 3: TagResource answers HTTP 200 where RUM answers 204 ===
AWS: Smithy: TagResource, POST /tags/{ResourceArn}, output TagResourceResponse is empty and the @http response code is defaulted in the model (200), so the status is recorded behavior: every recorded TagResource response in the corpus is HTTP 204 with an empty body. Eight recorded steps across five fixtures agree, including repeated tagging of an existing monitor and unicode/empty tag values.
Emulator: answers HTTP 200 (empty body).
Why: the status is part of the contract a client sees; code or infrastructure that distinguishes 204 No Content from 200 (empty-body handling, HTTP caches/proxies, assertions on the status class of a successful tag write) behaves differently against the emulator. Nothing else about these steps diverges — only the status.
Fix: the TagResource handler's success response — return 204 with no body.
Traces: rum-app-monitor-lifecycle-with-tags#task#2 (cluster also covers rum-app-monitor-lifecycle-with-tags#task#5, rum-empty-and-boundary-tag-operations#task#2, rum-multiple-app-monitors-tag-isolation#task#3/#4/#7, rum-tag-unicode-value-round-trip#task#2, rum-tag-value-unicode-and-empty-string-preserved#task#2).

=== Cause 2 of 3: UntagResource answers HTTP 200 where RUM answers 204 ===
AWS: Smithy: UntagResource, DELETE /tags/{ResourceArn} with TagKeys bound to @httpQuery 'tagKeys', output UntagResourceResponse empty. Recorded behavior: a successful UntagResource is HTTP 204 with an empty body (3 recorded steps across 2 fixtures).
Emulator: answers HTTP 200 (empty body).
Why: same as cause 1 — a successful tag removal reports a different status class than AWS does.
Fix: the UntagResource handler's success response — return 204 with no body.
Traces: rum-app-monitor-lifecycle-with-tags#task#7 (cluster also covers rum-empty-and-boundary-tag-operations#task#6 and #task#8).

=== Cause 3 of 3: UntagResource with an empty tagKeys list is not routed at all — HTTP 501 NotImplemented ===
AWS: Smithy: UntagResource.TagKeys is @required, bound to @httpQuery 'tagKeys', and targets TagKeyList with @length min 0 max 50 — an EMPTY key list is a modeled, valid request, so DELETE /tags/{ResourceArn} with no tagKeys values is a well-formed call. Recorded behavior: RUM accepts it and answers 204, removing nothing.
Emulator: returns HTTP 501 with an unmodeled body {"__type": "NotImplemented", "message": "operation DELETE /tags/<arn> is not implemented in this emulator"} — note the message names the raw method+path, and the SAME operation succeeds (cause 2) when tagKeys carries at least one value. The route is therefore only matched when the query string is non-empty.
Why: 501 is not a modeled error for UntagResource, so an SDK cannot classify it and no retry/except branch handles it; a caller clearing a computed (possibly empty) key set gets a server-error instead of a no-op success. Hypothesis: the request router keys the DELETE /tags/{ResourceArn} route on the presence of the tagKeys query parameter, so a zero-length list falls through to the not-implemented fallback.
Fix: the request routing/dispatch for DELETE /tags/{ResourceArn} in the rum crate plus the UntagResource input deserializer — match the route independently of the tagKeys query parameter and deserialize a missing/empty tagKeys as an empty list.
Traces: rum-empty-and-boundary-tag-operations#task#4.

## Evidence
Cause 1 — cluster service=rum|TagResource|status=204->200|keys=status (8 affected steps, representative rum-app-monitor-lifecycle-with-tags#task#2):
  status: expected 204, got 200   (body matched)

Cause 2 — cluster service=rum|UntagResource|status=204->200|keys=status (3 affected steps, representative rum-app-monitor-lifecycle-with-tags#task#7):
  status: expected 204, got 200

Cause 3 — cluster service=rum|UntagResource|status=204->501|keys=$,status (rum-empty-and-boundary-tag-operations#task#4):
  status: expected 204, got 501
  $: value_mismatch (expected {}, got {'__type': 'NotImplemented', 'message': 'operation DELETE /tags/arn%3Aaws%3Arum%3Aus-west-2%3A000000000000%3Aappmonitor%2Fsim2real-probe-rum-rumapp-boundary-2a167fc7 is not implemented in this emulator'})

## Steps to Reproduce
Boot the emulator, create an app monitor and take its ARN (arn:aws:rum:<region>:<account>:appmonitor/<name>).
1) Cause 1: curl -i -X POST 'http://<emulator>/tags/<url-encoded-arn>' -d '{"Tags":{"k":"v"}}' -> HTTP 200; AWS answers 204.
2) Cause 2: curl -i -X DELETE 'http://<emulator>/tags/<url-encoded-arn>?tagKeys=k' -> HTTP 200; AWS answers 204.
3) Cause 3: curl -i -X DELETE 'http://<emulator>/tags/<url-encoded-arn>' (no tagKeys, i.e. an empty key list) -> HTTP 501 {"__type":"NotImplemented"}; AWS answers 204.
Or replay fixtures rum-app-monitor-lifecycle-with-tags (task#2, task#7) and rum-empty-and-boundary-tag-operations (task#4).
