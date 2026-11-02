Issue-Source: trace-sweep-divergence
Issue-Trace: translate-resource-tagging
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-resource-tagging
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|ListTagsForResource|2xx->2xx|$.Tags[Key="team"]
Issue-Key: translate|TagResource|4xx->4xx|status
Issue-Commit: f3889f9a9021e35e583e2d714849e49437198a06

# Translate resource tagging diverges: an untagged key is still listed and TagResource not-found answers 404

> _Note: this issue was identified against the code base at commit `f3889f9a9021e35e583e2d714849e49437198a06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Two net-new causes in the Amazon Translate resource-tagging surface, both in fixture `translate-resource-tagging`. They share one fault: the tagging operations of this crate do not reproduce AWS's tagging contract — the removal half of the surface is not wired to state at all, and the ARN-addressed refusal carries the modelled HTTP status instead of the status Amazon Translate puts on the wire. A fixer must close BOTH before submitting.

Cause 1 of 2 - a tag key removed from a terminology is still reported by ListTagsForResource.
AWS: Smithy: UntagResource(ResourceArn @length 1..512 required, TagKeys TagKeyList @length 0..200 required) -> empty output, errors ConcurrentModificationException 409, InternalServerException 500, InvalidParameterValueException 400, ResourceNotFoundException 404; ListTagsForResource -> Tags (TagList). Model doc for UntagResource: "Removes a specific tag associated with an Amazon Translate resource." So after a successful UntagResource naming a key, the resource no longer carries that key and the following ListTagsForResource answers 200 with the surviving tag set only — the removed key is absent from `Tags`. The recording shows exactly that: the `Tags` list of the ListTagsForResource response at this step contains no entry for the key that was removed.
Emulator: ListTagsForResource answers 200 but its `Tags` list still carries the removed key — the replay reports `$.Tags[Key="team"]: extra (expected None, got {'Key': 'team', 'Value': 'mt'})`. Reason in the checkout: `UntagResource` is an unmodified scaffold — `services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/untag_resource.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` ends in `Err(crate::scaffolded::Scaffolded::not_implemented("UntagResource"))`, so no tag is ever removed from the stored record and `ListTagsForResource` (which clones `record.tags` straight out of state) reports the pre-removal set. Note the key name `team` and the value `mt` are this recording's values filling the template; the requirement is that whatever keys a removal names are gone from the next list.
Why: a client that removes a tag and then reads the resource's tags back sees the tag it just deleted. Tag-driven logic (cost allocation, inventory filters, idempotent reconcilers that delete-then-verify) concludes the delete failed or loops forever, and a resource's tag set in the emulator can only ever grow.
Fix: implement the `UntagResource` handler in `services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/untag_resource.rs` against the shared terminology store — resolve the record by ARN the way the sibling tag-write door does (`crate::state::terminology_name_for_arn`), drop every key the request's `TagKeys` names from `record.tags`, leave keys the resource does not hold untouched, and answer the empty 200 output; then delete that file's SCAFFOLDED marker line. `ListTagsForResource` needs no change once the removal reaches state.
Traces: translate-resource-tagging#task#5.

Cause 2 of 2 - TagResource against an ARN this account holds no resource for answers HTTP 404 where Amazon Translate answers HTTP 400.
AWS: recorded behavior only for the status (Smithy: TagResource errors = ConcurrentModificationException 409, InternalServerException 500, InvalidParameterValueException 400, ResourceNotFoundException 404, TooManyTagsException 400 — the model's `@httpError` on ResourceNotFoundException is 404, so the model alone does not pick the wire status). The recording shows Amazon Translate answering this refusal with HTTP **400** and the body message unchanged from the resource-not-found wording: the replay compared the message key and found it equal, only `status` diverged. This is the same wire convention the already-open issue `20261029T184304Z-translate-error-responses-use-wrong-status-and-message.md` records for GetTerminology/DeleteTerminology/TranslateText (AWS answers 400 for Translate's not-found refusals); that issue does NOT cover TagResource and does not carry this step's replay key, so this cause is net-new. Message: the fixed sentence of the resource-not-found error class is the requirement (no variable part — the ARN the caller asked for is in its own request); only the status is wrong here.
Emulator: HTTP 404 for the same request — replay reports `status: expected 400, got 404`. `tag_resource.rs` returns `TagResourceErrorModeled::ResourceNotFoundException(resource_not_found())`, which is serialized with the modelled 404.
Why: a client that branches on the HTTP status class sees a 404 where AWS raises a 400 client-error, so retry/branch logic (and any assertion on the status of a bad-ARN tagging call) takes the wrong path; SDK-level status checks and proxies that treat 404 as "route/resource missing" rather than "bad request" behave differently against the emulator than against AWS.
Fix: the TagResource handler's not-found path in the translate crate (and the shared error-status mapping it goes through) must put Amazon Translate's wire status 400 on this refusal instead of the modelled 404, keeping the error class and message text as they are. Fix it alongside the status half of the open error-response issue above so one convention governs every Translate refusal.
Traces: translate-resource-tagging#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct=29.3).

Cause 1 - fixture translate-resource-tagging, step task#5, operation ListTagsForResource: status 200 on both sides; body diverges — `$.Tags[Key="team"]: extra (expected None, got {'Key': 'team', 'Value': 'mt'})`. The recorded response omits that key (it was removed earlier in the fixture); the emulator still reports it. Supporting code: `services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/untag_resource.rs` lines 3-12 (SCAFFOLDED marker) and line 162 (`Err(crate::scaffolded::Scaffolded::not_implemented("UntagResource"))`); `src/ops/list_tags_for_resource.rs` lines 132-143 reports `record.tags.clone()` straight from state.

Cause 2 - fixture translate-resource-tagging, step task#6, operation TagResource: expected status 400, got 404; the body message matched (only the `status` key is in the divergence key `service=translate|TagResource|status=400->404|keys=status`). Supporting code: `src/ops/tag_resource.rs` lines 143-147 raise `ResourceNotFoundException` (modelled `@httpError` 404) via `crate::state::resource_not_found()` (state.rs lines 594-603).

Neither cluster is marked `stub_path` or `owned_by` in the replay report. The two pre-existing stub issues `20261029T192448Z-translate-tagresource-stub.md` and `20261029T192448Z-translate-listtagsforresource-stub.md` describe those two operations answering 501; in this checkout both are implemented and answer 200/404, so those issues do not cover either cause here. (The generated terminology ARN, the account id 000000000000 and the recorded tag key/value are environment values filling the templates, not requirements.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture translate-resource-tagging, step task#5 — import a terminology, tag it, remove one of its tag keys with UntagResource, then call ListTagsForResource on the resource ARN; observe the removed key still present in `Tags` (and observe UntagResource itself answering 501 NotImplemented in this checkout).
4. Cause 2: fixture translate-resource-tagging, step task#6 — call TagResource with a ResourceArn this account holds no terminology for; observe HTTP 404 instead of the recorded HTTP 400 with the same resource-not-found message body.
