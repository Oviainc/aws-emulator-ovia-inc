Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a4f3385d5a5802b3fd1582cf927bdd2b48aa00d

# The three child-record CREATE handlers date the new record from a pre-lock clock read, the instant the crate's own rule forbids

_Filed: 2026-10-29T03:37:55Z_
_Operation: CreateEnvironmentHost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a4f3385d5a5802b3fd1582cf927bdd2b48aa00d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::note_modified_at` states the stamping rule this crate holds for the child records of an environment: a host, a connector and an entitlement are "dated from the instant their change is applied, read with the store lock held and after the materialisation". Every handler that MUTATES such a record obeys it — `DeleteEnvironmentHost`, `DeleteEnvironmentConnector`, `UpdateEnvironmentConnector`, `AssociateEipToVlan`, `DisassociateEipFromVlan` all take `state.lock()`, call `materialise_due_transitions()` and only THEN read `now_ms()`, each with a comment saying why — and so does `CreateEnvironment` for the environment it mints (lock at :507, materialise at :508, clock at :666).

The three handlers that ADD a child record do the opposite: they read `crate::state::now_ms()` BEFORE taking the lock and before the materialisation, then assign that pre-lock instant straight onto the new record's `createdAt`/`modifiedAt`:

* `CreateEnvironmentHost` — clock at :250, lock at :252, materialise at :256, stamp used at :406-413 (`StoredHost::creating` sets both `created_at_ms` and `modified_at_ms` from it) and at :424;
* `CreateEnvironmentConnector` — clock at :245, lock at :247, materialise at :252, stamp used at :308-315 and :339;
* `CreateEntitlement` — clock at :203, lock at :205, materialise at :209, stamp used at :291-297 and :310.

Because a pending change carries a FUTURE wall-clock instant (`due_at_ms = now_ms() + TRANSITION_DELAY`) and materialising it writes that instant onto the environment and its VLANs, a call whose pre-lock clock read falls just before a transition becomes due — and which then waits on the store lock until after it is due — answers a child record dated at the instant the REQUEST ARRIVED, earlier than the instant the store was written and earlier than the `Environment`/`EnvironmentSummary` `modifiedAt` the same response reports. The same stale instant is then handed to `note_child_change`, whose `max` guard silently drops it, so the add does not re-stamp the environment at all — contradicting the handler's own comment that "the record is re-stamped and the summary this operation answers with reports that".

## Evidence

The rule, stated in this crate for exactly these three record kinds — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:348-352:
```
/// A host, a connector and an entitlement each ride a transition of their OWN and are
/// dated from the instant their change is applied, read with the store lock held and
/// after the materialisation ([`ComAmazonawsEvsAmazonelasticvmwareserviceState::materialise_due_transitions`]),
/// which is never earlier than a stamp that materialisation wrote: a transition is
/// materialised only once its due instant has passed.
```

The sibling that obeys it — src/ops/delete_environment_host.rs:217-227:
```
        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        ...
        state.materialise_due_transitions();
        // One instant for everything this call changes, so the host record and the
        // environment that holds it are stamped with the same `modifiedAt`. Read with the
        // store lock HELD and after the materialisation above: the delete is applied at
        // this instant, not at the one the request arrived at, so the stamp cannot predate
        // a transition the same call just materialised onto the host.
        let modified_at_ms = crate::state::now_ms();
```

The create handler that breaks it — src/ops/create_environment_host.rs:250-256:
```
        let created_at_ms = crate::state::now_ms();

        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        // which were due whether or not anybody called, so "the environment is
        // deployed" is judged against the state that is due at this instant.
        state.materialise_due_transitions();
```
and where that pre-lock instant lands — src/ops/create_environment_host.rs:406-413 (`crate::state::StoredHost::creating(&input.host, ip_address, esx_version, created_at_ms, ec2, pending)`), with state.rs:1113-1114 storing it as BOTH stamps:
```
            created_at_ms,
            modified_at_ms: created_at_ms,
```

The same shape in the two other create handlers — src/ops/create_environment_connector.rs:245 (`let created_at_ms = crate::state::now_ms();`), :247 (`let mut state = state.lock();`), :252 (`state.materialise_due_transitions();`), :308-315 (`StoredConnector::creating(connector_id.clone(), &input.r#type, &input.appliance_fqdn, secret_arn, created_at_ms, pending)`; state.rs:755-756 stores it as both stamps), :339 (`environment.note_child_change(created_at_ms);`) — and src/ops/create_entitlement.rs:203, :205, :209, :291-297, :310.

The guard that swallows the stale parent re-stamp — state.rs:1645-1651:
```
    /// The stamp never goes backwards: a change is dated at the later of the instant it
    /// happened and the instant the record already carried ...
    pub fn note_child_change(&mut self, at_ms: i64) {
        note_modified_at(&mut self.modified_at_ms, at_ms);
```
against the claim it is supposed to satisfy — create_environment_host.rs:421-424:
```
        // Adding a host changes the environment's host inventory, which is part of the
        // environment, so the record is re-stamped and the summary this operation
        // answers with reports that — exactly as removing one does.
        environment.note_child_change(created_at_ms);
```

What makes the window real — state.rs:663-667 (`due_at: Instant::now() + TRANSITION_DELAY, due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64`) and state.rs:1402-1408, where materialising writes `pending.due_at_ms` onto the environment and every VLAN.

Test coverage: nothing pins the create side. tests/all/record_stamp_monotonicity.rs:66-77 enumerates only mutations of EXISTING records (`AssociateEip`, `DisassociateEip`, `RemoveHost`, `ReconfigureConnector`, `RemoveConnector`), and tests/all/environment_child_change_stamps.rs:44 backdates its baseline by `BACKDATE_MS: i64 = 60_000` before the add, so an add stamped from a pre-lock read still passes.

## Steps to Reproduce

1. CreateEnvironment and let it settle to CREATED, then CreateEnvironmentConnector for the VCENTER appliance: the connector is stored CREATING with a pending move to ACTIVE whose `due_at_ms` is the create instant + TRANSITION_DELAY (src/state.rs:663-667).
2. Inside that TRANSITION_DELAY window, issue two requests concurrently: one that holds the EVS store lock across the due instant (for example a second CreateEnvironment, which provisions ten EC2 subnets while holding the guard), and a CreateEnvironmentHost (or CreateEntitlement) against the first environment.
3. The create handler reads the clock at create_environment_host.rs:250 before that transition is due, then blocks on `state.lock()` (:252) until after it is due.
4. On acquiring the lock, `materialise_due_transitions()` (:256) stamps the environment/VLANs (and the connector) with the transition's `due_at_ms`; the handler then stores the new host with `createdAt` = `modifiedAt` = its stale pre-lock instant (:406-413, state.rs:1113-1114), and `note_child_change(created_at_ms)` (:424) is dropped by the `max` guard.
5. Read the answered `host` beside the answered `environmentSummary` (or GetEnvironment): the host that was just added reports `createdAt`/`modifiedAt` EARLIER than the environment's `modifiedAt`, i.e. the instant the request arrived rather than the instant the change was applied — the stamp the rule quoted at src/state.rs:348-352 forbids — and the environment's stamp does not move for the add.
