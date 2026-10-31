Issue-Source: trace-sweep-divergence
Issue-Trace: interconnect-tag-operations-on-nonexistent-connection
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: interconnect-tag-operations-on-nonexistent-connection
Issue-Shape-Id: com.amazonaws.interconnect#Interconnect
Issue-Key: interconnect|TagResource|4xx->4xx|$.__type
Issue-Key: interconnect|TagResource|4xx->4xx|$.message
Issue-Key: interconnect|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: interconnect|ListTagsForResource|2xx->4xx|status
Issue-Key: interconnect|UntagResource|4xx->4xx|$.__type
Issue-Key: interconnect|UntagResource|4xx->4xx|$.message
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Interconnect ARN-addressed tag operations refuse a cross-Region connection ARN as out-of-scope instead of resolving it

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource, ListTagsForResource, UntagResource

## Description
Replay of simparity-wire-aws-v1 against this checkout scored 40.9% (9 passed / 13 mismatched of 22 steps, 12 divergence clusters, nothing blocked or errored, emulator up, bootstrap_signal false; service interconnect is bootstrapped). Seven clusters are marked OWNED by 20261023T095130Z-interconnect-wire-fidelity-seeded-environment-attach-point-catalog.md and are skipped. This issue carries THREE net-new root causes, numbered Cause 1 of 3 .. Cause 3 of 3; ALL THREE must be closed before it is submitted, and the representative step in Issue-Trace/Issue-Step plus every fixture#step in Issue-Trace-Ids must be re-replayed.

Shared fault: all three ARN-addressed tag operations run `crate::state::arn_scope_violation` as a precheck and refuse an ARN whose Region field is not the Region the request addresses, as a fault in the REQUEST. AWS does not treat a connection ARN's Region field that way: `AmazonResourceName` is modelled @pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$, whose Region segment is any non-colon text, so such an ARN is WELL-FORMED input and is resolved against the account's connections like any other — it simply names no connection, which each operation reports in its own way (a tag write refuses it as this service's client fault; the tag read answers the empty tag set). The emulator's precheck short-circuits all three with a different code (and, on the read, with a failure where AWS succeeds).

Related but NOT covering: 20261029T190212Z-interconnect-tagresource-both-arn-rejection-messages-diverge-from.md. It is grounded in the BEHAVIORAL corpus and carries no wire replay keys; its Cause 1 is the ACCOUNT-mismatch ARN wording and its Cause 2 the not-found wording of TagResource for an ARN that is already in scope. Neither cause says the Region field must not be scope-checked at all, neither covers UntagResource, and the ListTagsForResource half of both its checks is recorded there as unreached/unverified. The replay report marks none of the three clusters below as owned.

=== Cause 1 of 3: TagResource refuses a cross-Region connection ARN with InterconnectValidationException and a Region-mismatch sentence, where AWS answers the fixed not-found refusal ===
AWS: Smithy: TagResource input {arn: AmazonResourceName @required, @length 59..150, @pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$; tags: TagMap @required}, output empty, @references com.amazonaws.interconnect#ConnectionResource by arn; errors include InterconnectClientException (@httpError 400) and InterconnectValidationException (@httpError 400). The Region segment of that pattern is unconstrained ([^:]+), so an ARN carrying a Region other than the endpoint's is a valid value of the member. Recorded behavior: such an ARN, naming no connection the account holds, is refused with HTTP 400, code InterconnectClientException and the message
  "Unable to process request: Resource does not exist or is deleted"
That message is ENTIRELY fixed text: no ARN, connection id, Region or resource type is spliced into it — there are no variable parts at all.
Emulator: answers HTTP 400 with code InterconnectValidationException and the message "Unable to process request: <arn> is in region <arn-region-field>, and this request is for region <request-region>" (observed: "Unable to process request: arn:aws:interconnect:us-east-1:000000000000:connection/mcc-c11630b7 is in region us-east-1, and this request is for region us-west-2" — the ARN, the two Region names and the id there are per-run values filling the template, not requirements). The handler calls arn_scope_violation before any lookup, so the Region field alone decides the verdict and the ARN is never resolved.
Why: the two codes are the ones a caller branches on — InterconnectValidationException says "the request you sent is malformed, do not retry it as-is", InterconnectClientException says "that resource is not there (any more)". A client cleaning up tags on a deleted connection, or one that stores ARNs and replays them against a regional endpoint, gets "bad request" instead of "gone", and the refusal additionally echoes the caller's ARN into a message AWS keeps value-free.
Fix: the shared ARN-scope precheck and its use by the tag operations — stop treating the ARN's Region field as a scope violation in src/state.rs arn_scope_violation and in the TagResource handler that calls it, let the ARN fall through to connection resolution, and word the resulting refusal with the fixed not-found sentence above (held beside the crate's other refusal wordings in src/state.rs rather than reusing the identifier-echoing connection_not_retrievable_message).
Traces: interconnect-tag-operations-on-nonexistent-connection#task#1. Source: services/service-com-amazonaws-interconnect-interconnect/src/ops/tag_resource.rs lines 170-182 (the arn_scope_violation precheck) and lines 205-217 (the resolution it short-circuits, whose InterconnectClientException carries connection_not_retrievable_message(&input.arn)); src/state.rs lines 1172-1194 (arn_scope_violation, the ("region", parsed.region, region) tuple and the message template).

=== Cause 2 of 3: ListTagsForResource FAILS with 400 on a cross-Region connection ARN, where AWS answers HTTP 200 with an empty tag map ===
AWS: Smithy: ListTagsForResource is @readonly, input {arn: AmazonResourceName @required, same @length/@pattern as above}, output {tags: TagMap}. Recorded behavior: for the same well-formed ARN naming no connection, the read answers HTTP status 200 with `tags` present and EMPTY ({}) — it does not scope-check the Region field and does not validate connection existence; the empty map is the answer, not an error. (This is the same answer the recording shows for the effect-absent read after a refused tag write: nothing was tagged.)
Emulator: answers HTTP 400 (InterconnectValidationException with the Region-mismatch sentence of Cause 1) and no `tags` member at all — the replay reports `status: expected 200, got 400` and `$.tags: missing (expected {}, got None)`. The handler runs the same arn_scope_violation precheck before reading the store, and its own doc comment asserts the opposite rule ("an `arn` that names no resource of this service is refused rather than answered with an empty listing").
Why: this is a successful call turned into a failure. Any client that reads tags before writing them — a tag reconciler, an IaC refresh, a "do I already own this?" probe — raises an exception against the emulator where AWS returns an empty map, so the whole flow aborts instead of proceeding to create or tag. It also makes the emulator's read and write paths mutually inconsistent in a way no AWS caller can reproduce.
Fix: the ListTagsForResource handler — drop the Region scope precheck and answer the empty tag map for an ARN that resolves to no connection (status 200, `tags` serialized as {}), bringing the handler and its doc comment in line with the recorded read semantics.
Traces: interconnect-tag-operations-on-nonexistent-connection#task#2. Source: services/service-com-amazonaws-interconnect-interconnect/src/ops/list_tags_for_resource.rs lines 176-190 (the precheck), lines 197-216 (the lookup and its InterconnectClientException for a missing connection), lines 131-144 (the doc comment stating the rule the recording contradicts).

=== Cause 3 of 3: UntagResource refuses the same cross-Region ARN as out-of-scope instead of answering the fixed not-found refusal ===
AWS: Smithy: UntagResource input {arn: AmazonResourceName @required with the same @length/@pattern; tagKeys: TagKeyList @required}, output empty, @references ConnectionResource by arn; errors include InterconnectClientException (400) and InterconnectValidationException (400). Recorded behavior: the same ARN that names no connection is refused with HTTP 400, code InterconnectClientException and the message "Unable to process request: Resource does not exist or is deleted" — the identical, entirely fixed sentence the tag write uses; the two tag writers answer a missing resource the same way.
Emulator: answers HTTP 400 with code InterconnectValidationException and the same "Unable to process request: <arn> is in region <arn-region>, and this request is for region <request-region>" template as Cause 1, because UntagResource calls the same precheck in the same position.
Why: a tag-removal path is the common cleanup step after a connection is deleted, and it is exactly where a stored ARN no longer resolves. A caller that treats InterconnectValidationException as "my request is broken" (alarm, abort, page) rather than "already gone" (ignore and continue) fails its teardown; and because the write and the read diverge in different directions (Causes 2 and 3), no consistent client-side handling exists.
Fix: the UntagResource handler — same change as Cause 1 on the shared helper's Region arm plus this call site, with the shared fixed not-found wording for an ARN that resolves to nothing.
Traces: interconnect-tag-operations-on-nonexistent-connection#task#3. Source: services/service-com-amazonaws-interconnect-interconnect/src/ops/untag_resource.rs lines 170-182 (the arn_scope_violation precheck) and lines 205-217 (the resolution and its InterconnectClientException message).

## Evidence
replay_wire_corpus, trace_set simparity-wire-aws-v1: emulator_up true, bootstrap_signal false, steps_total 22, steps_passed 9, steps_mismatched 13, steps_blocked 0, steps_errored 0, score_pct 40.9, divergence_count 12. All three clusters below are service=interconnect with empty stub_path and empty owned_by.

Cause 1 — fixture interconnect-tag-operations-on-nonexistent-connection, step task#1, operation TagResource (status 400 vs 400):
  $.__type: expected 'com.amazon.awsairspanpassportservice#InterconnectClientException' (error code InterconnectClientException), got 'InterconnectValidationException'
  $.message: expected 'Unable to process request: Resource does not exist or is deleted', got 'Unable to process request: arn:aws:interconnect:us-east-1:000000000000:connection/mcc-c11630b7 is in region us-east-1, and this request is for region us-west-2' (the ARN, account id, connection id and the two Region names are this run's values filling the emulator's template; the requirement is the fixed sentence on the left, which carries no values at all).

Cause 2 — same fixture, step task#2, operation ListTagsForResource:
  status: expected 200, got 400
  $.tags: missing (expected {}, got None)

Cause 3 — same fixture, step task#3, operation UntagResource (status 400 vs 400):
  $.__type: expected 'com.amazon.awsairspanpassportservice#InterconnectClientException' (error code InterconnectClientException), got 'InterconnectValidationException'
  $.message: expected 'Unable to process request: Resource does not exist or is deleted', got 'Unable to process request: arn:aws:interconnect:us-east-1:000000000000:connection/mcc-c11630b7 is in region us-east-1, and this request is for region us-west-2'

Model evidence (smithy_query, model_path services/service-com-amazonaws-interconnect-interconnect/smithy-model/com-amazonaws-interconnect-interconnect.json): TagResource input {arn: AmazonResourceName required @length 59..150 @pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$, tags: TagMap required}, output empty, input @references ConnectionResource{id: arn}; UntagResource input {same arn, tagKeys: TagKeyList required}, output empty, same @references; ListTagsForResource @readonly, input {same arn}, output {tags: TagMap}. All three model AccessDeniedException(403), InterconnectClientException(400), InterconnectServerException(500), InterconnectValidationException(400), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429). The ARN pattern's Region segment is [^:]+ — unconstrained — so a cross-Region ARN satisfies the member's constraints.

Source corroboration in the checkout (services/service-com-amazonaws-interconnect-interconnect): src/state.rs:1172-1194 arn_scope_violation, which compares ("partition", "region", "account") and renders "{REFUSAL_ENVELOPE}{arn} is in {field} {carried}, and this request is for {field} {addressed}"; src/ops/tag_resource.rs:170-182 and :205-217; src/ops/untag_resource.rs:170-182 and :205-217; src/ops/list_tags_for_resource.rs:131-144 (doc comment), :176-190 (precheck), :197-216 (lookup + 400).

## Steps to Reproduce
Boot the emulator from this checkout and either replay the corpus (replay_wire_corpus) or drive a botocore/AWS SDK interconnect client at the data listener, with the client's Region set to the endpoint's Region (e.g. us-west-2).

Build an ARN for the caller's own account that carries a DIFFERENT Region and names no connection:
  own = sts.get_caller_identity()['Account']; arn = f"arn:aws:interconnect:us-east-1:{own}:connection/mcc-c11630b7"  (any (mcc|lmcc)-[a-z0-9]{8} id that was never allocated)

1. Cause 1: ic.tag_resource(arn=arn, tags={'k': 'v'}) -> expected HTTP 400 with error code InterconnectClientException and message exactly 'Unable to process request: Resource does not exist or is deleted' (fixed text; no ARN, id or Region spliced in). Observe HTTP 400 with code InterconnectValidationException and 'Unable to process request: <arn> is in region us-east-1, and this request is for region us-west-2'. Re-replay interconnect-tag-operations-on-nonexistent-connection#task#1.
2. Cause 2: ic.list_tags_for_resource(arn=arn) -> expected HTTP 200 with tags == {} (no error at all). Observe an exception, HTTP 400, no tags member. Re-replay interconnect-tag-operations-on-nonexistent-connection#task#2.
3. Cause 3: ic.untag_resource(arn=arn, tagKeys=['k']) -> expected HTTP 400 with code InterconnectClientException and the same fixed 'Unable to process request: Resource does not exist or is deleted'. Observe code InterconnectValidationException with the Region-mismatch sentence. Re-replay interconnect-tag-operations-on-nonexistent-connection#task#3.
4. Regression guards: (a) an ARN whose ACCOUNT field is not the caller's must still be refused as invalid input (that refusal's wording is the subject of 20261029T190212Z, do not regress its code); (b) an ARN naming an EXISTING connection, in either Region spelling, must still tag/untag/list that connection's tags successfully and must not be answered with the not-found refusal; (c) an ARN that does not parse as an Interconnect connection ARN at all must keep its InterconnectValidationException from not_a_connection_arn_message.

All three causes must be closed, then re-replay the representative step together with every fixture#step in Issue-Trace-Ids.
