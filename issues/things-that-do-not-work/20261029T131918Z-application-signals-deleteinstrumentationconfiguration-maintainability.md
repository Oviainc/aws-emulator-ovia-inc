Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c2f259515d2dac338aede2023e8bb8381afb339f

# The "removing a configuration takes its status history and its tags with it" rule is implemented twice, once in the expiry pass and once in the explicit removal

_Filed: 2026-10-29T13:19:18Z_
_Operation: DeleteInstrumentationConfiguration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c2f259515d2dac338aede2023e8bb8381afb339f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` holds TWO copies of the same removal rule for a dynamic instrumentation configuration:

1. `ComAmazonawsApplicationsignalsApplicationsignalsState::expire_instrumentation_configurations` removes each lapsed record inline (state.rs:4481-4493);
2. `ComAmazonawsApplicationsignalsApplicationsignalsState::remove_instrumentation_configuration` removes a named record (state.rs:4997-5020).

Both do the same four things in the same order — drop the entry from `set.configurations`, drop the same key from `set.status_history`, call `set.record_change(<instant>)`, and `self.tags.remove(<the record's ARN>)` — and both carry the SAME two justifying comments, word for word. `DeleteInstrumentationConfiguration::handle` calls both copies two lines apart (delete_instrumentation_configuration.rs:193-195), so the two live side by side on one request path.

Because they are two copies, they can drift: anything a future configuration record owns beside its status history and its tags (the crate already keys three things off `instrumentation_key` and the ARN) has to be torn down in BOTH places, and a change made in one leaves the other serving a half-removed record. The expiry pass is exactly the path that is hardest to reach over HTTP (the crate's own test file says so: "One rule cannot be reached over HTTP at all: a configuration's lapse", tests/all/instrumentation_configurations.rs:22-24), so it is the copy a drift would hide in.

The expiry loop can call `remove_instrumentation_configuration(&scope, &key, expires_at)` instead of re-deriving the removal. That is observably identical: the only thing the helper does beyond the inline copy is `schedule_instrumentation_pass(at + EMPTY_SCOPE_RETENTION)` for a scope it empties, and `expire_instrumentation_configurations` overwrites `instrumentation_due_at` with `next_instrumentation_pass()` immediately after the loop (state.rs:4507), which already computes that same droppable instant for an emptied scope (state.rs:4427-4432). No response and no stored value changes.

## Evidence

COPY 1 — the expiry pass, services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:4481-4493:
```
        for (scope, key, expires_at, arn) in lapsed {
            if let Some(set) = self.instrumentation_configurations.get_mut(&scope) {
                set.configurations.remove(&key);
                // The status history belongs to the configuration, so it goes with it: a
                // later configuration created at the same location is a NEW resource and
                // has reported nothing yet.
                set.status_history.remove(&key);
                set.record_change(expires_at);
            }
            // Tags are addressed by resource ARN, so a record that is gone takes its tags
            // with it rather than leaving them for the next resource minted at that ARN.
            self.tags.remove(&arn);
        }
```

COPY 2 — the explicit removal, src/state.rs:4997-5020 (same four steps, same two comments):
```
    pub fn remove_instrumentation_configuration(
        &mut self,
        scope: &InstrumentationScope,
        key: &str,
        at: DateTime,
    ) -> Option<InstrumentationConfiguration> {
        let (removed, emptied) = {
            let set = self.instrumentation_configurations.get_mut(scope)?;
            let removed = set.configurations.remove(key)?;
            // The status history belongs to the configuration, so it goes with it: a later
            // configuration created at the same location is a NEW resource and has reported
            // nothing yet.
            set.status_history.remove(key);
            set.record_change(at);
            (removed, set.configurations.is_empty())
        };
        // Tags are addressed by resource ARN, so a record that is gone takes its tags with
        // it rather than leaving them for the next resource minted at that ARN.
        self.tags.remove(&removed.arn);
        if emptied {
            self.schedule_instrumentation_pass(instant_plus(&at, EMPTY_SCOPE_RETENTION));
        }
        Some(removed)
    }
```

BOTH COPIES ON ONE REQUEST PATH — src/ops/delete_instrumentation_configuration.rs:192-196:
```
        let mut store = state.lock();
        store.expire_instrumentation_configurations(&now);
        if store
            .remove_instrumentation_configuration(&scope, &key, now)
            .is_none()
```

WHY THE FOLD IS OBSERVABLY FREE — src/state.rs:4507 overwrites the schedule straight after the loop:
```
        self.instrumentation_due_at = self.next_instrumentation_pass();
```
and `next_instrumentation_pass` already derives the emptied-scope horizon the helper would have scheduled, src/state.rs:4427-4432:
```
                let droppable = if set.configurations.is_empty() {
                    set.last_changed_at
                        .map(|changed| instant_plus(&changed, EMPTY_SCOPE_RETENTION))
                } else {
                    None
                };
```

THE CRATE'S OWN SINGLE-HOMING RULE, which this pair breaks — src/state.rs:6-12 (module doc): "The derivation helpers below live here, beside the store, because the ... lifecycle operations ... must all agree on identity, on the ARN they mint and on the server-derived members of a stored record — two copies of that logic would let a Get disagree with the Create that wrote the record."

TEST COVERAGE THAT MUST STAY GREEN (it pins both copies' observable effects):
- copy 1: tests/all/instrumentation_configurations.rs `an_expired_configuration_lapses_and_its_emptied_scope_is_dropped` (lines 1064-1127, drives `store.expire_instrumentation_configurations` directly, asserting the record is gone, that the lapse is news to an earlier sync, and that the emptied scope is dropped past the horizon);
- copy 1, tags: tests/all/instrumentation_configuration_tags.rs `a_lapsed_configurations_tags_are_no_longer_reported` (lines 555-652, asserts `!store.tags.contains_key(..)` after the lapse);
- copy 1, status history: tests/all/instrumentation_configuration_status.rs `a_lapsed_configuration_serves_no_status_history` (lines 1338-1485);
- copy 1 via a read: tests/all/get_instrumentation_configuration.rs lines 479-610;
- copy 2: tests/all/instrumentation_configuration_deletion.rs and tests/all/batch_instrumentation_configuration_deletion.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs and read lines 4481-4493 (`expire_instrumentation_configurations`).
2. Read lines 4997-5020 (`remove_instrumentation_configuration`) in the same file.
3. Observe that both perform `configurations.remove(key)`, `status_history.remove(key)`, `set.record_change(<instant>)` and `self.tags.remove(<record ARN>)`, and that the two justifying comments are byte-identical in both places.
4. Open src/ops/delete_instrumentation_configuration.rs lines 192-196 and observe one handler invoking both copies two lines apart.
5. Fold copy 1 into a call to `remove_instrumentation_configuration(&scope, &key, expires_at)` and re-run `cargo test -p service-com-amazonaws-applicationsignals-applicationsignals` — the suites named in the evidence (instrumentation_configurations, instrumentation_configuration_tags, instrumentation_configuration_status, get_instrumentation_configuration, instrumentation_configuration_deletion, batch_instrumentation_configuration_deletion) must stay green, and no response body changes.
