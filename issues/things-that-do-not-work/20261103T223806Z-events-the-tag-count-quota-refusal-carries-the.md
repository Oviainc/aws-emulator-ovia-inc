Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count|run#8|AssertionError|assert (code, msg) == ("ValidationException", _TOO_MANY), (
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# events: the tag-count quota refusal carries the generic resource-quota sentence instead of AWS's per-resource tag message

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause in the EventBridge tagging plane. The 50-tag-per-resource quota IS enforced and the refusal carries the right status and error code — only the MESSAGE is emulator-authored, and it is the sentence this service uses for its *resource*-count quotas, so a tag-quota refusal is indistinguishable from a rule/target-quota refusal.

Cause 1 of 1: a tag write past the 50-tag per-resource ceiling is refused with the shared count-quota sentence instead of AWS's tag-specific sentence.

AWS: Smithy: TagResource (com.amazonaws.eventbridge#TagResource) takes ResourceARN (Arn, len 1..1600, required) and Tags (TagList, required); TagList carries NO @length, and the operation's own documentation states "You can associate as many as 50 tags with a resource". The model binds only ConcurrentModificationException, InternalException, ManagedRuleException and ResourceNotFoundException for this operation, so the quota refusal itself is recorded behavior only: the recording shows HTTP 400 with error code `ValidationException` and the message `The request exceeds the maximum number of tags allowed per resource.` That message is ENTIRELY fixed text — it names neither the resource that was tagged, nor the tag count submitted, nor the numeric limit, so nothing in it comes from the request or from state. The rule is about the RESULT of the merge, not about the request alone: a request whose keys the resource already carries does not grow the set (an overwrite on a full resource is accepted), while a request whose new keys would push the merged set past 50 is refused with nothing applied.

Emulator: the refusal is raised with the right code and status (400 `ValidationException`) but with the message `The requested resource exceeds the maximum number allowed.` — the crate's shared count-quota sentence (`crate::state::QUOTA_EXCEEDED`, services/service-com-amazonaws-eventbridge-awsevents/src/state.rs:149), which `too_many_tags()` (state.rs ~:4575) deliberately reuses and which `bound_tag_count` (state.rs:4568) and `merge_tags` (state.rs:4536-4554) return to the handler (services/service-com-amazonaws-eventbridge-awsevents/src/ops/tag_resource.rs:180 and :207). Replayed: tagging a bus that already carries 40 tags with 11 further keys was refused `'ValidationException': 'The requested resource exceeds the maximum number allowed.'` where AWS states `The request exceeds the maximum number of tags allowed per resource.`

Why: a client (or a docs-driven test, or an operator reading a CLI failure) that branches on the tag-quota message never matches, and because the emulator sends the sentence AWS reserves for its resource-count quotas — the same text AWS returns for PutTargets past the per-rule target cap, as established by resolved issue 20261029T235600Z-events-client-error-messages-are-emulator-authored-instead (its Cause 8) — a consumer cannot tell "too many tags on this resource" from "too many targets/rules/buses". The two quotas are different refusal classes with different wording in AWS and must not share one sentence.

Fix: the tag-count refusal in the events crate's state helper — `too_many_tags()`, the single reason string `bound_tag_count` and `merge_tags` return — must emit AWS's tag-quota sentence instead of the shared `QUOTA_EXCEEDED` constant, so every tag write of this service (TagResource, and the Tags a CreateEventBus/PutRule request carries) refuses in AWS's words while the resource-count quotas keep theirs. The error class (400 ValidationException) and the all-or-nothing merge semantics already match and need no change. Note for the fixer: the grounding check asserts the whole quota contract, and it stops at the FIRST assertion, so the rest of it (40+10 accepted; a 51st NEW key on a full resource refused with the same sentence and not stored; an OVERWRITE of an existing key on a full resource accepted; CreateEventBus with 51 Tags refused with that same ValidationException sentence and leaving no bus behind, so DescribeEventBus answers ResourceNotFoundException `Event bus <name> does not exist.`) is only reachable once the message is corrected — all of it must be green before submitting.

Traces: behavioral check events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count. Code: services/service-com-amazonaws-eventbridge-awsevents/src/ops/tag_resource.rs:180,207; services/service-com-amazonaws-eventbridge-awsevents/src/state.rs:149,205,4536-4575.

## Evidence
Failing behavioral check: events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count (verdict: mismatch).

Assertion from the replay:
  File "check_events_event_bus_tag_limit_counts_tags_already_on_the_bus_and_overwrites_do_not_count.py", line 34, in run
    assert (code, msg) == ("ValidationException", _TOO_MANY)
  AssertionError: adding 11 tags to a bus with 40 gave 'ValidationException': 'The requested resource exceeds the maximum number allowed.'

Expected (AWS, recorded): HTTP 400, Error.Code `ValidationException`, Error.Message `The request exceeds the maximum number of tags allowed per resource.` (fixed text; no variable parts).
Actual (emulator): HTTP 400, Error.Code `ValidationException`, Error.Message `The requested resource exceeds the maximum number allowed.`

Status and error code agree; only the message text diverges. The emulator's text is the crate-wide count-quota sentence `crate::state::QUOTA_EXCEEDED` (state.rs:149), returned for tag writes by `too_many_tags()` via `bound_tag_count` (state.rs:4568, called at ops/tag_resource.rs:180) and `merge_tags` (state.rs:4553, called at ops/tag_resource.rs:207). state.rs:4575-4583 documents the reuse as intentional, which is why the tag quota and the resource-count quotas are currently indistinguishable.

Quota enforcement itself is correct: 40 existing tags + 11 new keys is refused, so only the wording has to change.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite; the check events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count fails as a mismatch (service heading `events`).
2. By hand against the emulator endpoint:
   a. events.CreateEventBus(Name=<bus>, Tags=[{Key: base-00..base-39, Value: v}]) — 40 tags; note the returned EventBusArn.
   b. events.ListTagsForResource(ResourceARN=<arn>) — confirms 40 tags.
   c. events.TagResource(ResourceARN=<arn>, Tags=[{Key: over-00..over-10, Value: v}]) — 11 further keys, i.e. a merged set of 51.
3. Inspect the refusal: AWS answers HTTP 400 ValidationException with message `The request exceeds the maximum number of tags allowed per resource.`; the emulator answers HTTP 400 ValidationException with `The requested resource exceeds the maximum number allowed.`
4. Confirm the same sentence is produced for the other tag writes in this service (a 51st NEW key on a 50-tag resource, and CreateEventBus with 51 Tags), and that it is the same text PutTargets correctly uses for its per-rule target quota — which is why the two classes must be separated.
5. ListTagsForResource after step 2c: the resource still carries exactly its 40 tags (the all-or-nothing behavior is already correct).
