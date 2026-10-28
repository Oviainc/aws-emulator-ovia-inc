Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# PutAccountSettings recomputes the environment health checks without first materialising the lifecycle transitions the crate requires that recomputation to follow

_Filed: 2026-10-28T14:44:14Z_
_Operation: PutAccountSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate takes the store lock and immediately calls `state.materialise_due_transitions()`, which is the paired operation "materialise the transitions the clock has made due, THEN recompute the derived checks" — the pairing the state module documents and the test harness's own `settle` helper reproduces. `PutAccountSettings` is the one handler that calls the second half alone: it writes the settings and then calls `state.recompute_environment_checks()` without ever materialising the due lifecycle transitions.

So a put that is the first call after an environment's `CREATING -> CREATED` (or a host's / connector's) transition came due recomputes the checks against states the clock has already moved past: the environment is still `CREATING` in the store, `refresh_checks` takes its `state != STATE_CREATED` branch, clears `checks` and `impaired_since_ms`, and the put leaves the environment with no checks at all — even though the setting it just wrote is a check input of that environment and the next read will compute four checks from it. That breaks the crate's own invariant that the checks a call leaves in the store are the checks the next read reports, and it silently discards the stored `impaired_since_ms` onsets of an environment whose transition happens to be overdue at that moment.

## Evidence

The pairing the state module states — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:2346-2360:
```
    /// Apply every lifecycle transition the clock has made due and recompute every
    /// environment's health checks, so a reporting operation answers with the states and
    /// the health that are owed at this instant.
...
    pub fn materialise_due_transitions(&mut self) {
        self.materialise_due_lifecycle_transitions();
        // The checks read the states the transitions above just settled, so they are
        // recomputed after them — and on every call, because a host deleted, a license
        // key freed or a core count set changes an environment's health at once and
        // nothing else would notice.
        self.recompute_environment_checks();
    }
```

The handler that calls only the second half — src/ops/put_account_settings.rs:184-202 (no `materialise_due_transitions()` anywhere in the file):
```
        let mut state = state.lock();
        if !input.settings.is_empty() && !state.has_room_for_account_settings(&region) {
...
        let modified = state.put_account_settings(&region, &input.settings);
        // `vcfPortedCoreCount` is what the environments of this Region check their
        // deployed cores against, so writing a setting changes a check input of every one
        // of them: their checks are recomputed from the settings this call leaves behind.
        state.recompute_environment_checks();
```

The siblings that do pair them, e.g. src/ops/get_environment.rs:166-169 and src/ops/tag_resource.rs:190-191:
```
        let mut state = state.lock();
        // Reporting only: this materialises the state change the clock already owed
        // this environment, which was due whether or not anybody read it.
        state.materialise_due_transitions();
```

What a `CREATING` environment's recomputation does to the stored checks — state.rs:1757-1764:
```
        // A check runs on a SETTLED environment: while one is still being created, or is
        // being or has been deleted, no check of it has run, so it reports none and
        // carries no impairment.
        if self.state != STATE_CREATED {
            self.checks.clear();
            self.impaired_since_ms.clear();
            return;
        }
```

The invariant this breaks, as this crate's own suite states and asserts it — tests/all/environment_check_input_recompute.rs:587-601 (for the put itself):
```
        // What the put LEFT in the store, captured before any read recomputes it.
        let left_behind = stored_check_results(&router, &environment_id);
...
        assert_eq!(
            left_behind, reported,
            "case {case:?}: the stored checks {left_behind:?} must be the checks the next read \
             reports, got {reported:?}"
        );
```
and the harness helper that reproduces the pairing — tests/all/environment_lifecycle.rs:386-393:
```
    let mut store = state.lock();
    for environment in store.environments.values_mut() {
        environment.materialise_due_transition(past_the_transition);
    }
    // The health checks are derived from the states just settled, exactly as the
    // handlers' own `materialise_due_transitions` recomputes them after a transition.
    store.recompute_environment_checks();
```
The existing test only passes because its helper settles the store before the put (`environment_at_its_minimum_host_count` calls `settle(router)` at line 128); nothing pins the case where the put itself is the first call after a transition came due.

## Steps to Reproduce

1. CreateEnvironment with an installed VCF version, `licenseInfo` and four hosts; do NOT read it afterwards.
2. Sleep past the create transition delay (the environment's CREATING -> CREATED change is now due but nobody has called).
3. Call PutAccountSettings with `settings: [{ "name": "vcfPortedCoreCount", "value": "1" }]`.
4. Inspect the stored environment: `environmentState` is still CREATING and `checks` is empty — the put recomputed the checks of a state the clock had already moved past and left none behind.
5. Call GetEnvironment: it materialises the transition and reports CREATED with four checks (KEY_COVERAGE FAILED), so the checks the put left in the store are not the checks the next read reports — the invariant tests/all/environment_check_input_recompute.rs:597-601 asserts for this very operation.
