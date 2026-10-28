Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# The entity-set deleted-record eviction orders by `updated_at_millis`, which `settle` moves forward, so it drops a NEWER deletion and keeps an older one — the exact reordering the ThreatIntelSet sibling added a dedicated field to avoid

_Filed: 2026-10-28T19:50:01Z_
_Operation: DeleteThreatEntitySet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both entity-set deletes (`DeleteThreatEntitySet` and `DeleteTrustedEntitySet`) go through `crate::state::delete_entity_set`, which keeps the deleted record so a read can still report `DELETE_PENDING`/`DELETED`, and then bounds that retention with `evict_deleted_entity_sets`. That function promises "Drop deleted entity sets, oldest first" and justifies its key with: `updated_at_millis` on a record being deleted "is the instant its delete was accepted (or the instant `DELETED` was owed), and `tick_millis` makes those instants strictly increasing, so the ordering is total".

That premise is false in this crate. `EntitySetRecord::begin_delete` stamps `updated_at_millis = now` (the accepted instant, a `tick_millis` value), but `EntitySetRecord::settle` then overwrites it with `self.updated_at_millis = due`, where `due = accepted + ENTITY_SET_TRANSITION_MILLIS` (500 ms) — not a `tick_millis` value at all. So the eviction key is a MIXTURE of "accepted" instants and "accepted + 500 ms" instants, and it is no longer monotone in deletion order: a set deleted earlier whose `DELETED` has been settled sorts AFTER a set deleted later that has not settled yet. `evict_deleted_entity_sets` then removes the record of the NEWER deletion and keeps the older one, so a caller that deleted a set a moment ago gets the 400 id-refusal from `GetThreatEntitySet` while an older deleted set is still reported.

Which records get settled is caller-driven, because `GetThreatEntitySet`/`GetTrustedEntitySet` settle only the ONE set they read (`set.settle(now)`), and no listing settles anything. So a pure READ of one deleted set decides which other, newer deleted set a later delete throws away.

The sibling threat-list family implements the same retention correctly and its code says exactly why: `ThreatIntelSetRecord` carries a dedicated `deleted_at_millis` field precisely so "nothing else moves it — not the clock settling `THREAT_INTEL_SET_DELETE_PENDING` to `THREAT_INTEL_SET_DELETED`, which would otherwise reorder two deletions accepted within one span", and `evict_deleted_threat_intel_sets` orders by that field. The fix is the one that sibling already shows: evict the entity sets by the instant the delete was ACCEPTED rather than by a field `settle` rewrites. No successful response changes and the retention ceiling itself is untouched.

## Evidence

THE PROMISE AND THE KEY — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:4098-4119:
```
/// Drop deleted entity sets, oldest first, until at most
/// [`DELETED_ENTITY_SETS_RETAINED`] remain. `keep` is never evicted: it is the
/// record the call that triggered this just deleted, and a caller reads that one
/// back.
fn evict_deleted_entity_sets(sets: &mut BTreeMap<String, EntitySetRecord>, keep: &str) {
...
    // The oldest deletions first: `updated_at_millis` on a record being deleted is
    // the instant its delete was accepted (or the instant `DELETED` was owed), and
    // `tick_millis` makes those instants strictly increasing, so the ordering is
    // total and the id only breaks a tie that cannot arise.
    let mut deleted: Vec<(i64, String)> = sets
        .values()
        .filter(|set| set.is_deleting() && set.id != keep)
        .map(|set| (set.updated_at_millis, set.id.clone()))
        .collect();
    deleted.sort_unstable();
    for (_, id) in deleted.into_iter().take(surplus) {
        sets.remove(&id);
    }
```

WHAT BREAKS THE PREMISE — the same file, src/state.rs:4021-4038 (`EntitySetRecord::settle`):
```
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.status_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        ...
        self.status_due_millis = None;
        self.updated_at_millis = due;
    }
```
and the due instant is NOT a `tick_millis` value — src/state.rs:4009 (`begin_delete`): `self.status_due_millis = Some(now.saturating_add(ENTITY_SET_TRANSITION_MILLIS));` with src/state.rs:3764: `pub const ENTITY_SET_TRANSITION_MILLIS: i64 = 500;`

THE SIBLING THAT STATES THE HAZARD AND AVOIDS IT — src/state.rs:4179-4185:
```
    /// The instant a delete was ACCEPTED for this list, or `None` while no delete
    /// has been. It is the key [`Detector::delete_threat_intel_set`] evicts the
    /// retained deleted records by, oldest deletion first, so it records the
    /// acceptance and nothing else moves it — not the clock settling
    /// [`THREAT_INTEL_SET_DELETE_PENDING`] to [`THREAT_INTEL_SET_DELETED`], which
    /// would otherwise reorder two deletions accepted within one span.
    pub deleted_at_millis: Option<i64>,
```
and src/state.rs:2704-2712:
```
        // The oldest deletions first: `deleted_at_millis` is the instant the delete
        // was ACCEPTED and nothing moves it afterwards, and `tick_millis` makes
        // those instants strictly increasing, so the ordering is total and the id
        // only breaks a tie that cannot arise.
        let mut deleted: Vec<(i64, String)> = self
            .threat_intel_sets
            .values()
            .filter(|set| set.is_deleting() && Some(set.id.as_str()) != keep)
            .map(|set| (set.deleted_at_millis.unwrap_or_default(), set.id.clone()))
```

THE PATH THAT REACHES IT — src/ops/delete_threat_entity_set.rs:169 `let Some(accepted) = detector.delete_threat_entity_set(&input.threat_entity_set_id, now)`, which is src/state.rs:3077-3079 `delete_entity_set(&mut self.threat_entity_sets, id, now)` → src/state.rs:4091-4094 `evict_deleted_entity_sets(sets, id)`. `DeleteTrustedEntitySet` reaches the same helper at src/state.rs:3013-3015.

WHAT PERTURBS THE KEY — src/ops/get_threat_entity_set.rs:167-191 settles ONLY the set it reads:
```
        // The lock is taken MUTABLY only to materialise the clock-owed status
        // change below; nothing here depends on the caller.
...
        set.settle(now);
```
while the listing takes an immutable lock and settles nothing (src/ops/list_threat_entity_sets.rs:164 `let state = state.lock();`).

TEST COVERAGE: the threat-list family pins the rule — tests/all/threat_intel_set_delete.rs:560-566 `// Rule 4: the retained deleted records are bounded.` / `async fn the_retained_deleted_lists_are_bounded_oldest_deletion_first()`. The entity-set suites have NO retention/eviction test at all (tests/all/threat_entity_set_delete.rs rules 1-5 cover `DELETE_PENDING`→`DELETED`, persistence, slot freeing, isolation and refusals; tests/all/trusted_entity_set_delete.rs likewise), so nothing pins the current ordering.

## Steps to Reproduce

1. Create a detector in us-east-1.
2. Create a threat entity set and delete it; call this set A, accepted at t≈0 (its `updated_at_millis` is 0, `DELETED` owed at 500 ms).
3. Create and delete four more threat entity sets at t≈200 ms (keys 200, unsettled).
4. Create and delete one more set X at t≈400 ms (key 400, `DELETED` owed at 900 ms). Six deleted records are now retained, the ceiling `DELETED_ENTITY_SETS_RETAINED` (= `THREAT_ENTITY_SETS_PER_DETECTOR` = 6).
5. At t≈550 ms call `GET /detector/{detectorId}/threatentityset/{A}`. get_threat_entity_set.rs:191 settles A ONLY (X is not due until 900 ms), so A's eviction key becomes 500 while X's stays 400.
6. Create and delete a seventh set K. delete_threat_entity_set.rs:169 → `evict_deleted_entity_sets` computes surplus 1 and, sorting by `updated_at_millis`, picks the minimum among the six — one of the t≈200 records, or X at 400 — never A at 500, even though A's deletion is the OLDEST.
7. Read back each deleted set: the record the eviction dropped answers 400 `InvalidInputException` (`minted_id_rejection` at delete/get), while A — deleted before it — still reports `DELETED`. Contrast with the same sequence against `DELETE /detector/{detectorId}/threatintelset/{id}`: evict_deleted_threat_intel_sets orders by `deleted_at_millis`, which `settle` never moves, so there the oldest deletion is the one dropped.
