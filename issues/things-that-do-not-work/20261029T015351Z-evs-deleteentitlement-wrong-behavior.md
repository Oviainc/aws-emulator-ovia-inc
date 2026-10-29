Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7f16775ab75866f4d67c2e0f0f0192fd6ebf8c3d

# DeleteEntitlement dates stoppedAt from a pre-lock clock read, so a record can report stopping before the startedAt the same call materialised

_Filed: 2026-10-29T01:53:51Z_
_Operation: DeleteEntitlement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7f16775ab75866f4d67c2e0f0f0192fd6ebf8c3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteEntitlement` reads the wall clock BEFORE it takes this service's store lock, and then stamps `stoppedAt` with that stale instant onto a record whose `startedAt` the same call may have just written from the materialisation it runs after acquiring the lock.

An entitlement's owed `CREATING -> CREATED` change carries a FUTURE wall-clock instant (`due_at_ms = now_ms() + TRANSITION_DELAY`), and materialising it sets `started_at_ms = pending.due_at_ms` — "Usage tracking starts when the entitlement is created, which is the instant the asynchronous validation was owed at". So when a delete reads the clock just before that change comes due and then waits on the store lock until after it is due, `materialise_due_transitions()` writes `startedAt = due_at_ms` and the handler then reports `stoppedAt` = its own earlier, pre-lock instant: an entitlement that stopped tracking before it started.

The crate states the opposite rule in two places. Every other mutating handler of this crate reads its stamp instant WITH the lock held and AFTER the materialisation, each with a comment saying why ("the stamp cannot predate a transition the same call just materialised onto the record it is about to change"), and the OTHER writer of the very same field — `DeleteEnvironment`, which stops an environment's entitlements through `StoredEntitlement::stop` — reads its instant after the lock and the materialisation. So one field, `VmEntitlement$stoppedAt`, has two different dating rules in this crate, and the test suite asserts the invariant the pre-lock one can break ("tracking cannot stop before it started").

This is not the already-open stamp issues: 20261028T174304Z-evs-associateeiptovlan-wrong-behavior--r2 names the two VLAN writers, and 20261028T204138Z-evs-deleteenvironmentconnector-wrong-behavior--r2 names exactly `DeleteEnvironmentConnector`, `UpdateEnvironmentConnector` and `DeleteEnvironmentHost`. Neither names the entitlement writer, and the observable here is a different member (`stoppedAt` versus a child record's `modifiedAt`).

## Evidence

The pre-lock clock read, the lock, and the materialisation of the very records this call stops — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_entitlement.rs:210-219:
```
        // The instant tracking stops, read once so every record this call stops carries
        // the same `stoppedAt`.
        let stopped_at_ms = crate::state::now_ms();

        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        // which were due whether or not anybody called, so the environment and its
        // entitlements are in the state that is due at this instant before the delete is
        // evaluated.
        state.materialise_due_transitions();
```
and the stale instant being stamped on — src/ops/delete_entitlement.rs:271-273:
```
            if let Some(entitlement) = environment.entitlements.remove(&key) {
                deleted.push(entitlement.stopped(stopped_at_ms).to_wire(&environment_id));
            }
```

What the materialisation writes onto that same record — src/state.rs:1006-1017:
```
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.status = pending.status.to_string();
        if pending.status == ENTITLEMENT_CREATED {
            // Usage tracking starts when the entitlement is created, which is the
            // instant the asynchronous validation was owed at.
            self.started_at_ms = Some(pending.due_at_ms);
        }
        self.pending = None;
    }
```

The due instant is a FUTURE wall-clock instant, which is what makes the stale read observable — src/state.rs:661-669:
```
impl PendingStatus {
    /// The change a create starts, due [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(status: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
            status,
        }
    }
```
(`CreateEntitlement` builds exactly this: src/ops/create_entitlement.rs:281-283 `let pending = crate::state::PendingStatus::starting_now(crate::state::ENTITLEMENT_CREATED);`.)

The OTHER writer of the same field, dated from a POST-lock instant — src/ops/delete_environment.rs:191-192 and :241-275:
```
        let mut state = state.lock();
        state.materialise_due_transitions();
...
        if !already_going_away {
            // The instant the deletion was accepted, read once so the environment and
            // every resource of it that goes away with it carry one stamp.
            let accepted_at_ms = crate::state::now_ms();
...
            environment.start_dependent_deletions(&deletion, accepted_at_ms);
```
which stops each entitlement through the same primitive — src/state.rs:995-999 (`pub fn stop(&mut self, stopped_at_ms: i64) { self.status = ENTITLEMENT_DELETED.to_string(); self.stopped_at_ms = Some(stopped_at_ms);`).

The ordering rule the sibling handlers of this crate state and follow — src/ops/delete_environment_host.rs:222-227:
```
        // One instant for everything this call changes, so the host record and the
        // environment that holds it are stamped with the same `modifiedAt`. Read with the
        // store lock HELD and after the materialisation above: the delete is applied at
        // this instant, not at the one the request arrived at, so the stamp cannot predate
        // a transition the same call just materialised onto the host.
        let modified_at_ms = crate::state::now_ms();
```
(the same comment and ordering at src/ops/update_environment_connector.rs:227-232, src/ops/delete_environment_connector.rs:224-229, src/ops/associate_eip_to_vlan.rs:231-235 and src/ops/disassociate_eip_from_vlan.rs:204-207).

Test coverage: the invariant is asserted, but only for the sequential case where the create's transition settled BEFORE the delete call is made, so no test exercises a delete whose clock read straddles the due instant — tests/all/delete_entitlement.rs:458-464:
```
        if let Some(started_at) = started_at {
            let stopped_at = reported[0]["stoppedAt"].as_f64().unwrap_or_default();
            assert!(
                stopped_at >= started_at,
                "case {case:?}: tracking cannot stop before it started, expected a stoppedAt \
                 at or after {started_at}, got {stopped_at}"
            );
        }
```

## Steps to Reproduce

1. CreateEnvironment, let it settle, and CreateEnvironmentConnector with type VCENTER; let the connector settle to ACTIVE.
2. Call CreateEntitlement for `vm-1`: the record is stored CREATING with a pending move to CREATED whose `due_at_ms` is the call instant + TRANSITION_DELAY (src/state.rs:663-668).
3. Within that TRANSITION_DELAY window, issue two requests concurrently: any call that holds the EVS store lock across the due instant (for example a second CreateEnvironment, which provisions ten EC2 subnets while holding the guard), and a DeleteEntitlement for `vm-1`.
4. The delete reads its instant at src/ops/delete_entitlement.rs:212 before the CREATED change is due, then blocks on `state.lock()` (:214) until after it is due.
5. On acquiring the lock, `materialise_due_transitions()` (:219) sets the entitlement to CREATED with `startedAt = due_at_ms` (src/state.rs:1011-1015); the handler then removes the record and reports it stopped at its stale pre-lock instant (:271-272).
6. Read the answered `entitlements[0]`: `stoppedAt` is EARLIER than `startedAt` — the invariant tests/all/delete_entitlement.rs:460-464 states ("tracking cannot stop before it started"). Compare the same stop performed by DeleteEnvironment (src/ops/delete_environment.rs:244), which dates it from an instant read after the lock and the materialisation and therefore cannot precede the `startedAt` it just wrote.
