Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6d79682d5fafb93b7e79af5f6250590ad910f660

# Connector and host modifiedAt are raw-assigned a pre-lock instant, so a child record's stamp can move backwards past a transition the same call just materialised

_Filed: 2026-10-28T20:41:38Z_
_Operation: DeleteEnvironmentConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6d79682d5fafb93b7e79af5f6250590ad910f660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers that mutate a CHILD record of an environment read the wall clock BEFORE taking this service's store lock, then — after `materialise_due_transitions()` has already written a stamp onto the very record they are about to touch — assign that stale instant straight onto the child's `modified_at_ms`:

* `DeleteEnvironmentConnector` (clock at delete_environment_connector.rs:220, lock at :222, materialise at :227, raw assign at :298),
* `UpdateEnvironmentConnector` (clock at update_environment_connector.rs:224, lock at :226, materialise at :230, raw assign at :311),
* `DeleteEnvironmentHost` (clock at delete_environment_host.rs:219, lock at :221, materialise at :225, raw assign at :265).

A connector's and a host's pending change carries a FUTURE wall-clock instant (`due_at_ms = now_ms() + TRANSITION_DELAY`), and materialising it writes that instant onto the record (`self.modified_at_ms = pending.due_at_ms`). So when a call reads the clock just before a pending `CREATING -> ACTIVE` / `CREATING -> CREATED` comes due and then waits on the store lock until after it is due, the handler overwrites the stamp the materialisation just wrote with an EARLIER one. The parent stamp written a few lines later goes through `StoredEnvironment::note_child_change`, whose documented rule is that the stamp "never goes backwards" precisely because of this hazard; the child stamps get no such guard. The answered `Connector`/`Host` then reports `state`/`hostState` from the transition that landed with a `modifiedAt` from before it landed, and earlier than the `modifiedAt` of the environment that holds it (and, for an ACTIVE connector reported elsewhere, earlier than the `checks[].lastCheckAttempt` derived from that same field).

The already-open issue 20261028T174304Z-evs-associateeiptovlan-wrong-behavior--r2 describes this pattern for the two VLAN writers (`AssociateEipToVlan`, `DisassociateEipFromVlan`) only; it names neither the connector writers nor the host writer, which keep their own copies of the unguarded assignment.

## Evidence

The stale clock read, the materialisation of the same record, and the raw assignment — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_environment_connector.rs:218-227:
```
        // One instant for everything this call changes, so the connector record and the
        // environment that holds it are stamped with the same `modifiedAt`.
        let modified_at_ms = crate::state::now_ms();

        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        // which were due whether or not anybody called, so the environment and its
        // connectors are in the state that is due at this instant before the delete is
        // evaluated.
        state.materialise_due_transitions();
```
and delete_environment_connector.rs:296-307:
```
            connector.state = crate::state::CONNECTOR_DELETING.to_string();
            connector.modified_at_ms = modified_at_ms;
            connector.pending = Some(pending);
...
            environment.note_child_change(modified_at_ms);
```

What the materialisation writes onto that same field — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:744-750:
```
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.state = pending.state.to_string();
        self.modified_at_ms = pending.due_at_ms;
        self.pending = None;
```
and the host counterpart — state.rs:1101-1107 (`self.state = pending.status.to_string(); self.modified_at_ms = pending.due_at_ms;`).

The due instant is a FUTURE wall-clock instant, which is what makes the stale read observable — state.rs:670-676:
```
    pub fn starting_now(state: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
```
(and state.rs:641-646 for `PendingStatus`, the host/entitlement form).

The rule the crate states for exactly this hazard, applied to the PARENT stamp only — state.rs:1623-1628:
```
    /// The stamp never goes backwards: a change is dated at the later of the instant it
    /// happened and the instant the record already carried, so a transition the clock
    /// owed at a future instant ([`Self::materialise_due_transition`]) is not undone by a
    /// child change evaluated before it comes due.
    pub fn note_child_change(&mut self, at_ms: i64) {
        self.modified_at_ms = self.modified_at_ms.max(at_ms);
```

The two sibling handlers with the identical unguarded assignment — src/ops/update_environment_connector.rs:224 (`let modified_at_ms = crate::state::now_ms();`), :230 (`state.materialise_due_transitions();`), :310-311 (`connector.state = crate::state::CONNECTOR_UPDATING.to_string(); connector.modified_at_ms = modified_at_ms;`), :324 (`environment.note_child_change(modified_at_ms);`); and src/ops/delete_environment_host.rs:219, :225, :264-265 (`host.state = crate::state::HOST_DELETING.to_string(); host.modified_at_ms = modified_at_ms;`), :274.

Test coverage: the existing suites compare the child stamp only against weaker bounds, so nothing pins the value against the transition that landed — tests/all/connectors.rs:500-504 asserts only `modified_at >= before["modifiedAt"]` after an update, and tests/all/connector_deletion.rs:360-361 asserts only `modifiedAt >= createdAt`; neither exercises a call that waits on the lock across a due instant.

## Steps to Reproduce

1. CreateEnvironment and let it settle, then CreateEnvironmentConnector: the connector is stored CREATING with a pending move to ACTIVE whose `due_at_ms` is the create instant + TRANSITION_DELAY (src/state.rs:670-676).
2. Within that TRANSITION_DELAY window, issue two requests concurrently: any call that holds the EVS store lock across the due instant (for example a second CreateEnvironment, which provisions ten EC2 subnets while holding the guard), and a DeleteEnvironmentConnector (or UpdateEnvironmentConnector changing the FQDN) for that connector.
3. The connector handler reads the clock at delete_environment_connector.rs:220 before the ACTIVE transition is due, then blocks on `state.lock()` (:222) until after it is due.
4. On acquiring the lock, `materialise_due_transitions()` (:227) sets the connector to ACTIVE and its `modified_at_ms` to the transition's `due_at_ms` (src/state.rs:749); the handler then overwrites `connector.modified_at_ms` with its stale instant (:298) while `note_child_change` keeps the later value for the environment (:307).
5. Read the answered `connector` (and the same connector from ListEnvironmentConnectors) beside GetEnvironment: the connector's `modifiedAt` is EARLIER than the moment it became ACTIVE and earlier than `Environment$modifiedAt` — the backwards move the rule quoted at src/state.rs:1623-1626 says must not happen. The same sequence with CreateEnvironmentHost + DeleteEnvironmentHost reproduces it for `Host$modifiedAt` (delete_environment_host.rs:265).
