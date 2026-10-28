Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# DeleteKeyspace resets status_due_at on a table or type that is already DELETING, postponing a removal the clock already owed

_Filed: 2026-10-28T21:09:01Z_
_Operation: DeleteKeyspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteKeyspace` deliberately accepts a keyspace that holds a resource whose own deletion is already in flight — `Keyspace::resource_in_flight` excludes `DELETING` precisely because such a resource "is on its way out, which is where dropping the keyspace takes it anyway". But the acceptance path then writes a FRESH due instant onto every table and every type of the keyspace, unconditionally:

    table.status_due_at = Some(settles_at);   // settles_at = now + TRANSIENT_STATUS_SPAN

For a table or type that was already `DELETING`, that OVERWRITES the instant the clock already owed its removal at and moves it later, by up to a whole `TRANSIENT_STATUS_SPAN`.

The sibling that owns exactly this rule does the opposite and says so: `DeleteTable` detects a table that is already `DELETING` and deliberately leaves the owed instant alone, so "a repeated call cannot postpone it". `DeleteKeyspace` is the one other call that can put the same table into `DELETING`, and it postpones what `DeleteTable` refuses to postpone — so whether a resource's deletion can be deferred by a later call depends on which of the two sibling handlers served that later call.

Observable effect: a table whose removal was due at T is still reported by `GetTable` in status `DELETING` after T (instead of answering the modelled `ResourceNotFoundException`) because a `DeleteKeyspace` issued inside the span re-dated it; same for `GetType` on a type that `DeleteType` had already put into `DELETING`. The lingering window is bounded by `TRANSIENT_STATUS_SPAN`, which is why this is an edge case rather than a lost resource — but it is the rule the crate states for itself, broken by one handler.

The fix is the one `DeleteTable` already carries: only stamp `status_due_at` on a table or type that is not already `DELETING`, and leave the instant the clock already owes where it is. `note_transition_due` keeps the EARLIEST pending instant, so nothing else has to change for `settle` to still run on time.

## Evidence

The rule, stated and enforced by the sibling handler — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/delete_table.rs:216-228`:
```
        let settles_at = match table.status.as_str() {
            // "If the table is already in the DELETING state, no error is returned." The
            // deletion the clock already owes stays due at its original instant, so a
            // repeated call cannot postpone it.
            crate::state::TABLE_STATUS_DELETING => None,
            // "If the table is in the ACTIVE state, you can delete it." The table enters
            // DELETING now and the clock owes its removal one span from here.
            crate::state::TABLE_STATUS_ACTIVE => {
                let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
                table.status = crate::state::TABLE_STATUS_DELETING.to_string();
                table.status_due_at = Some(settles_at);
                Some(settles_at)
            }
```

The same crate breaking it — `src/ops/delete_keyspace.rs:249-259`:
```
        let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
        keyspace.status = crate::state::KEYSPACE_STATUS_DELETING.to_string();
        keyspace.status_due_at = Some(settles_at);
        for table in keyspace.tables.values_mut() {
            table.status = crate::state::TABLE_STATUS_DELETING.to_string();
            table.status_due_at = Some(settles_at);
        }
        for udt in keyspace.types.values_mut() {
            udt.status = crate::state::TYPE_STATUS_DELETING.to_string();
            udt.status_due_at = Some(settles_at);
        }
```
There is no `status`/`status_due_at` guard here, so an entry already in `DELETING` with an EARLIER `status_due_at` is re-dated to this call's span.

That this path is reached by design for an already-`DELETING` resource — `src/state.rs:909-920` (`Keyspace::resource_in_flight`, which is what `delete_keyspace.rs:232` calls):
```
    /// A resource in flight ([`is_in_flight_status`]) is one the clock still owes a
    /// transition to, and dropping it is exactly what `DeleteTable` refuses for a table
    /// in that state — so a keyspace holding one cannot be dropped either, since
    /// `DeleteKeyspace` "deletes a keyspace and all of its tables". A resource already
    /// `DELETING` does NOT count: it is on its way out, which is where dropping the
    /// keyspace takes it anyway.
```

What the overwritten field drives — `src/state.rs:1780-1799` (`settle`):
```
                    let Some(due) = table.status_due_at else {
                        return true;
                    };
                    if now < due {
                        earliest = earlier_of(earliest, due);
                        return true;
                    }
                    table.status_due_at = None;
                    match table.status.as_str() {
...
                        TABLE_STATUS_DELETING => false,
```
so a later `due` keeps the record — and its `DELETING` status — readable for longer.

Test coverage: the exact path is exercised but the instant is not pinned, so nothing constrains the fix. `tests/all/delete_keyspace_lifecycle.rs:619-669` (`a_resource_already_deleting_does_not_block_the_keyspace_delete`) deletes a table and a type, then deletes the keyspace while both are `DELETING`, and asserts only the SURVIVING table's status and that the keyspace is gone after one `settle_wait()`:
```
/// RULE 2's other half: a resource that is ALREADY `DELETING` is on its way out, so it
/// does not block the keyspace's own deletion.
```
and the sibling rule's own test asserts only the status code of the repeated delete, never the due instant — `tests/all/delete_table_in_flight.rs:332-336`:
```
            // "If the table is in the ACTIVE state, you can delete it. ... If the table is
            // already in the DELETING state, no error is returned."
            None => {
                assert_ok(table, got);
            }
```

## Steps to Reproduce

1. Start the Keyspaces emulator; create keyspace `ks`, then table `t` (minimal schema) and type `ty` in it, and wait out the transient CREATING span so both are ACTIVE
2. Call DeleteTable {"keyspaceName":"ks","tableName":"t"} and DeleteType {"keyspaceName":"ks","typeName":"ty"} — both are accepted and the clock now owes their removal at T = (that call) + TRANSIENT_STATUS_SPAN (500 ms)
3. Roughly 400 ms later — still inside the span, so `t` and `ty` are still reported DELETING — call DeleteKeyspace {"keyspaceName":"ks"}; it is accepted, because an already-DELETING resource deliberately does not block it
4. Roughly 150 ms later (i.e. after T, the instant the clock already owed `t` and `ty` their removal at, but less than 500 ms after the DeleteKeyspace call) call GetTable {"keyspaceName":"ks","tableName":"t"} and GetType {"keyspaceName":"ks","typeName":"ty"}
5. Expected (the rule delete_table.rs:216-220 states for the very same resource): the removal the clock owed at T has happened, so both answer 404 ResourceNotFoundException
6. Actual: both answer 200 with status DELETING — DeleteKeyspace re-stamped `status_due_at` to its own `now + TRANSIENT_STATUS_SPAN`, postponing a removal that was already due
