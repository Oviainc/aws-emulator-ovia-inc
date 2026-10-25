Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9d4ea5e78ce612465906f9a67fc5f9d068061c14

# ListTagsForResource answers 200 with tags for a composition the clock has already reclaimed, which GetComposition 404s

_Filed: 2026-10-25T00:21:15Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9d4ea5e78ce612465906f9a67fc5f9d068061c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A composition is ephemeral: `StopComposition` sets `state = STOPPING` and `ended_at = now`, and the clock then owes it `STOPPED` one `COMPOSITION_STOP_SPAN` later and the deletion of the record one span after that. Both steps are materialised lazily by `materialize_due_compositions`, which only operations holding the store MUTABLY call.

`CompositionRecord::is_reclaimed` asks `self.state == COMPOSITION_STATE_STOPPED && now >= ended_at + 2*SPAN`. `ListTagsForResource` reaches the store through `crate::state::resource_tags`, which takes `&State` and therefore never materialises anything — so if nothing else has touched the store since the stop, the record is still literally in `STOPPING`, `is_reclaimed` short-circuits on the state test and returns `false`, and the handler answers HTTP 200 with the dead composition's tags.

`GetComposition` at the same instant calls `materialize_due_compositions` first: `STOPPING` -> `STOPPED` (due at +1 span), then the `retain` drops the record (due at +2 spans), so it answers HTTP 404. The two reads disagree, which is exactly what the comment on the composition arm of `resource_tags` says cannot happen. Worse, the disagreement is self-erasing: the `GetComposition` call clears the record, so the same `ListTagsForResource` request answers 404 if it is repeated afterwards — the same request gets two different answers depending on which unrelated call ran in between.

## Evidence

The comment that states the rule — services/.../src/state.rs:2998-3006:
```
        "composition" => state
            .compositions
            .get(arn)
            // This reader holds the store immutably, so a composition the clock has
            // already reclaimed is skipped rather than dropped: a tag listing answers for
            // exactly the compositions `GetComposition` answers for, and the next call
            // that can write clears the record.
            .filter(|composition| !composition.is_reclaimed(SystemTime::now()))
            .map(|composition| composition.tags.clone()),
```

The code that breaks it — `is_reclaimed` requires the STOPPED state, which only a mutable call writes. services/.../src/state.rs:1248-1253:
```
    pub fn is_reclaimed(&self, now: SystemTime) -> bool {
        self.state == COMPOSITION_STATE_STOPPED
            && self
                .ended_at
                .is_some_and(|ended_at| now >= ended_at + COMPOSITION_STOP_SPAN * 2)
    }
```
services/.../src/state.rs:1141-1144 (the only writer of that state):
```
        if self.state == COMPOSITION_STATE_STOPPING
            && stopped_at.is_some_and(|stopped_at| now >= stopped_at)
        {
            self.state = COMPOSITION_STATE_STOPPED.to_string();
```

The two call sites that diverge:
* services/.../src/ops/list_tags_for_resource.rs:169 — `crate::state::resource_tags(&state.lock(), &input.resource_arn)`, under the handler comment "A pure read: the resource's tags are reported exactly as they are stored, and nothing is written".
* services/.../src/ops/get_composition.rs:157-164 — `let mut store = state.lock(); store.materialize_due_compositions(::std::time::SystemTime::now()); let record = store.compositions.get(&input.arn).ok_or_else(|| ... ResourceNotFoundException ...)`.
* services/.../src/state.rs:3029-3031 shows the sibling tagging path does materialise, so `TagResource`/`UntagResource` already answer 404 here while `ListTagsForResource` answers 200: "// A composition's own lifetime is the clock's: materialise what it already owed, so a tag cannot be written onto a composition that no longer exists. state.materialize_due_compositions(SystemTime::now());"

The reclaim deadline is pinned by an existing test — services/.../tests/all/composition_stop.rs:520-524:
```
        // Inside two spans the composition is still there to be refused as a conflict;
        // past them the clock has reclaimed the ephemeral resource and the honest answer
        // is the not-found one.
        let conflict_admitted = before < STOP_SPAN * 2;
        let not_found_admitted = after >= STOP_SPAN * 2;
```
No test covers `ListTagsForResource` after the reclaim: services/.../tests/all/compositions.rs:1112-1160 (`a_composition_is_taggable_and_does_not_outlive_its_stage`) only lists tags while the composition is running.

## Steps to Reproduce

1. `CreateStage {}` and keep `stage.arn`.
2. `StartComposition {"stageArn":"<stage arn>","destinations":[{"channel":{"channelArn":"arn:aws:ivs:us-east-1:<acct>:channel/live"}}],"tags":{"env":"test"}}` and keep `composition.arn`.
3. `StopComposition {"arn":"<composition arn>"}` (HTTP 200).
4. Wait longer than two `COMPOSITION_STOP_SPAN`s (> 1s) and make NO other call in between.
5. `GET /tags/<url-encoded composition arn>` — answers HTTP 200 with `{"tags":{"env":"test"}}`.
6. `GetComposition {"arn":"<composition arn>"}` — answers HTTP 404 `ResourceNotFoundException` for the same ARN at the same moment.
7. Repeat step 5: it now answers HTTP 404, because step 6 is what finally dropped the record.
