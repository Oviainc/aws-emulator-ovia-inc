Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 145e568e7ef9b3115be9abe08039cd724717eead

# The HCX VLAN's modifiedAt is raw-assigned a pre-lock instant, so it can move backwards past a transition the same call just materialised

_Filed: 2026-10-28T17:43:04Z_
_Operation: AssociateEipToVlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `145e568e7ef9b3115be9abe08039cd724717eead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateEipToVlan` reads the wall clock BEFORE it takes this service's store lock, then — after `materialise_due_transitions()` has already written a stamp onto the very VLAN it is about to touch — assigns that stale instant straight onto `vlan.modified_at_ms`. The parent stamp written five lines later goes through `StoredEnvironment::note_child_change`, whose documented rule is that the stamp 'never goes backwards' precisely because a transition the clock owed may have landed first. The child stamp gets no such guard, so the VLAN's own `modifiedAt` can end up EARLIER than the state change the same response reports and earlier than the environment that holds it.

Two reachable orderings:

1. A VLAN transition settles while this call waits for the lock. The environment's pending `CREATING -> CREATED` carries `due_at_ms = <create instant> + TRANSITION_DELAY`; `materialise_due_transitions()` (associate_eip_to_vlan.rs:232) writes that instant onto every VLAN (`vlan.modified_at_ms = pending.due_at_ms`, state.rs:1385) and flips `vlan_state` to `CREATED`. The handler then overwrites it with the instant it read before blocking on the lock (associate_eip_to_vlan.rs:443). The answered `Vlan` therefore reports `vlanState: CREATED` with a `modifiedAt` from before it became `CREATED`, and with a `modifiedAt` earlier than the `Environment$modifiedAt` the max-guarded parent path kept.
2. Two concurrent associates against the same VLAN: the call that read the clock later wins the lock first and stamps the VLAN, then the earlier-reading call stamps it back, so the VLAN claims it was last modified before the association it now carries.

The same raw assignment of a pre-lock instant onto a child record sits in `DisassociateEipFromVlan` (`modified_at_ms` captured at disassociate_eip_from_vlan.rs:200, assigned at :282, with `note_child_change` at :286) — one fix, taking the later of the instant and the stamp the record already carries (or reading the clock after the lock, as `DeleteEnvironment` does), covers both VLAN writers.

## Evidence

The stale clock read, the materialisation that may stamp the same record, and the two stamps side by side — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/associate_eip_to_vlan.rs:226-232:
```
        let modified_at_ms = crate::state::now_ms();

        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        // which were due whether or not anybody called, so the VLAN is in the state that
        // is due at this instant before the association is evaluated.
        state.materialise_due_transitions();
```
and associate_eip_to_vlan.rs:440-449:
```
                    .map(|vlan| {
                        if let Some(association) = association.clone() {
                            vlan.eip_associations.push(association);
                            vlan.modified_at_ms = modified_at_ms;
                        }
                        vlan.to_wire()
                    });
                if wire.is_some() && !already_associated {
                    environment.note_child_change(modified_at_ms);
                }
```

What the materialisation writes onto that same field — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1379-1386:
```
    pub fn materialise_due_transition(&mut self, now: Instant) {
        if let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) {
            self.state = pending.environment_state.to_string();
            self.modified_at_ms = pending.due_at_ms;
            for vlan in &mut self.vlans {
                vlan.vlan_state = pending.vlan_state.to_string();
                vlan.modified_at_ms = pending.due_at_ms;
```

The rule the crate states for exactly this hazard, applied to the parent stamp only — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1623-1628:
```
    /// The stamp never goes backwards: a change is dated at the later of the instant it
    /// happened and the instant the record already carried, so a transition the clock
    /// owed at a future instant ([`Self::materialise_due_transition`]) is not undone by a
    /// child change evaluated before it comes due.
    pub fn note_child_change(&mut self, at_ms: i64) {
        self.modified_at_ms = self.modified_at_ms.max(at_ms);
```

The due instant is a FUTURE wall-clock instant, which is what makes the stale read observable — src/state.rs:611-618:
```
    pub fn starting_now(environment_state: &'static str, vlan_state: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
```

The sibling VLAN writer with the same pattern — src/ops/disassociate_eip_from_vlan.rs:200-205 (`let modified_at_ms = crate::state::now_ms();` … `let mut state = state.lock();` … `state.materialise_due_transitions();`) and :280-286 (`vlan.modified_at_ms = modified_at_ms;` … `environment.note_child_change(modified_at_ms);`).

Test coverage: the existing stamp suites assert only the ENVIRONMENT's stamp, so nothing pins the VLAN's — tests/all/environment_child_change_stamps.rs:475 `a_change_to_a_record_an_environment_holds_stamps_the_environment` reads `body["environment"]["modifiedAt"]` (tests/all/environment_child_change_stamps.rs:471), and tests/all/hcx_eip_associations.rs:109 `an_eip_associates_with_the_public_hcx_vlan_and_is_read_back` asserts no `modifiedAt` at all.

## Steps to Reproduce

1. CreateEnvironment with `initialVlans.isHcxPublic` true and a /28 `initialVlans.hcx` block (as tests/all/hcx_eip_associations.rs:70 `create_public_hcx` does), so the environment is CREATING with a pending CREATED transition whose `due_at_ms` is the create instant + TRANSITION_DELAY (src/state.rs:611-618).
2. Immediately issue two requests concurrently: any call that holds the EVS store lock across the transition's due instant (for example a second CreateEnvironment, which provisions ten EC2 subnets while holding the guard), and an AssociateEipToVlan for `vlanName` "hcx" with an allocationId the account's EC2 store holds.
3. The AssociateEipToVlan handler reads the clock at src/ops/associate_eip_to_vlan.rs:226 before the transition is due, then blocks on `state.lock()` (:228) until after it is due.
4. When it gets the lock, `materialise_due_transitions()` (:232) sets the HCX VLAN's `vlanState` to CREATED and its `modified_at_ms` to the transition's `due_at_ms` (src/state.rs:1384-1385); the handler then overwrites `vlan.modified_at_ms` with its stale instant (:443) while `note_child_change` keeps the later value for the environment (:448).
5. Observe the answered `vlan` (and the same VLAN from ListEnvironmentVlans): `vlanState` CREATED and one `eipAssociations` entry, but `modifiedAt` EARLIER than the moment it became CREATED and earlier than `Environment$modifiedAt` from GetEnvironment — the backwards move the rule quoted at src/state.rs:1623-1626 says must not happen.
