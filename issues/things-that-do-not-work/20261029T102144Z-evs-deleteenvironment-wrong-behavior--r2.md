Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 56752714ba65963d122a47feae790978b4aace00

# The environment's own modifiedAt — and its VLANs' in DeleteEnvironment — is raw-assigned, bypassing the note_modified_at guard this crate calls "the guard every record of this service is re-stamped through"

_Filed: 2026-10-29T10:21:44Z_
_Operation: DeleteEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `56752714ba65963d122a47feae790978b4aace00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers date a change to `StoredEnvironment::modified_at_ms` (and, in `DeleteEnvironment`, to `StoredVlan::modified_at_ms`) by PLAIN ASSIGNMENT of a freshly read clock value, instead of through `crate::state::note_modified_at`, the "take the LATER of the two instants" guard state.rs documents as the guard every record of this service is re-stamped through:

* `DeleteEnvironment` — `environment.modified_at_ms = accepted_at_ms` (delete_environment.rs:246) and, for every VLAN of the environment, `vlan.modified_at_ms = environment.modified_at_ms` (delete_environment.rs:251);
* `TagResource` — `environment.modified_at_ms = crate::state::now_ms()` (tag_resource.rs:252);
* `UntagResource` — `environment.modified_at_ms = crate::state::now_ms()` (untag_resource.rs:208).

The stamp these fields already carry can be AHEAD of what `now_ms()` answers. `StoredEnvironment::materialise_due_transition` writes `pending.due_at_ms` onto the environment AND onto every VLAN of it (state.rs:1503-1506), and `PendingTransition::starting_now` computes `due_at_ms` from a wall-clock read taken AFTER the monotonic `Instant::now()` that `due_at` is derived from (state.rs:657-658). So at the first call that materialises the transition — the `materialise_due_transitions()` each of these three handlers runs a few lines earlier — `now_ms()` can be strictly less than the `due_at_ms` just written onto the record (the two reads straddle a millisecond boundary; a SystemTime adjustment widens the gap arbitrarily, and `now_ms()` reads `SystemTime`, not the monotonic clock). The handler then overwrites the stamp with the earlier value, so the response reports an environment in `CREATED` (and, for the delete, VLANs whose state the same call just settled) with a `modifiedAt` EARLIER than the instant that state was reached — the backwards move the crate's own rule says must not happen.

In `DeleteEnvironment` the inconsistency is inside one field: `StoredVlan::modified_at_ms` has exactly three writers in this crate, and the other two (`AssociateEipToVlan`, `DisassociateEipFromVlan`) go through `note_modified_at`, each with a comment saying the stamp goes "through the same guard the environment's does below, so neither can move backwards past a transition the clock had already made due". `DeleteEnvironment` writes the same field raw.

This is not the gap the two existing stamp issues describe: 20261028T174304Z-evs-associateeiptovlan-wrong-behavior--r2 (fixed) and 20261028T204138Z-evs-deleteenvironmentconnector-wrong-behavior--r2 (open) are about a clock read taken BEFORE the store lock in the VLAN / connector / host writers, and they name only those five handlers. All three handlers named here read the clock AFTER the lock; what they skip is the guard itself, and they are the only writers of the ENVIRONMENT's own stamp that skip it.

## Evidence

The rule, stated in the crate's own state module — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:342-362:
```
/// Date one stored record's `modifiedAt` at a change applied at `at_ms`, taking the
/// LATER of that instant and the instant the record already carries.
///
/// The guard the environment and the VLANs it holds are both re-stamped through, because
/// those records share ONE lifecycle transition and so one stamp: a
/// [`PendingTransition`] moves the environment and every VLAN of it together and dates
/// them both at the instant it came due ([`PendingTransition::due_at_ms`]). A change to a
/// VLAN is a change to the environment that holds it, so the pair is re-stamped by one
/// call; dating either of them at an instant EARLIER than that transition would report
/// the record as last modified before the state change the same response reports, and
/// would let the VLAN's stamp fall behind the environment's for the very change both
/// carry.
...
pub fn note_modified_at(modified_at_ms: &mut i64, at_ms: i64) {
    *modified_at_ms = (*modified_at_ms).max(at_ms);
}
```
and state.rs:1744-1751:
```
    /// The stamp never goes backwards: a change is dated at the later of the instant it
    /// happened and the instant the record already carried, so a transition the clock
    /// owed at a future instant ([`Self::materialise_due_transition`]) is not undone by a
    /// child change evaluated before it comes due. That is [`note_modified_at`], the
    /// guard every record of this service is re-stamped through.
    pub fn note_child_change(&mut self, at_ms: i64) {
        note_modified_at(&mut self.modified_at_ms, at_ms);
    }
```

The code under that rule — src/ops/delete_environment.rs:241-251:
```
        if !already_going_away {
            // The instant the deletion was accepted, read once so the environment and
            // every resource of it that goes away with it carry one stamp.
            let accepted_at_ms = crate::state::now_ms();
            environment.state = crate::state::STATE_DELETING.to_string();
            environment.modified_at_ms = accepted_at_ms;
            for vlan in &mut environment.vlans {
                // "Environment deletion also deletes the associated Amazon EVS VLAN
                // subnets": they go away with the environment.
                vlan.vlan_state = crate::state::STATE_DELETING.to_string();
                vlan.modified_at_ms = environment.modified_at_ms;
```
(the same call ran `state.materialise_due_transitions();` at delete_environment.rs:192), src/ops/tag_resource.rs:250-253:
```
        if merged != environment.tags {
            environment.tags = merged;
            environment.modified_at_ms = crate::state::now_ms();
        }
```
(materialise at tag_resource.rs:191), and src/ops/untag_resource.rs:207-209:
```
        if removed {
            environment.modified_at_ms = crate::state::now_ms();
        }
```
(materialise at untag_resource.rs:170).

What the materialisation writes onto those same two fields — state.rs:1500-1507:
```
    pub fn materialise_due_transition(&mut self, now: Instant) {
        if let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) {
            self.state = pending.environment_state.to_string();
            self.modified_at_ms = pending.due_at_ms;
            for vlan in &mut self.vlans {
                vlan.vlan_state = pending.vlan_state.to_string();
                vlan.modified_at_ms = pending.due_at_ms;
            }
```
and why that value can exceed the wall clock at the moment `due_at` is reached — state.rs:655-661:
```
    pub fn starting_now(environment_state: &'static str, vlan_state: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
```
(`due_at_ms` is built from a wall-clock read taken after the monotonic read `due_at` comes from, so it denotes an instant strictly later than `due_at`.)

The two sibling writers of the SAME `StoredVlan::modified_at_ms` field, which do use the guard — src/ops/disassociate_eip_from_vlan.rs:311-317:
```
            // Remembered as a removal of THIS VLAN, so the idempotent repeat of this
            // call resolves to it instead of to no association at all.
            vlan.note_eip_association_removed(&input.association_id);
            // The VLAN's own stamp goes through the same guard the environment's does
            // below, so neither can move backwards past a transition the clock had
            // already made due.
            crate::state::note_modified_at(&mut vlan.modified_at_ms, modified_at_ms);
```
and src/ops/associate_eip_to_vlan.rs:446-452 (`crate::state::note_modified_at(&mut vlan.modified_at_ms, modified_at_ms);` under the same comment).

Test coverage: the rule is pinned for the two VLAN writers only — tests/all/record_stamp_monotonicity.rs:505-550 (`a_vlan_change_is_dated_at_the_later_of_the_change_and_the_transition_it_rides`) postdates the environment and its VLANs with the helper at record_stamp_monotonicity.rs:400-414 ("What a lifecycle transition leaves behind: it moves the environment and every VLAN of it together and dates them both at the instant it came due, which is an instant the clock has not reached when the call that materialises it evaluates a change") and then asserts `after_ms == became_created_ms`. Its row table (record_stamp_monotonicity.rs:511-520) contains only `AssociateEip` and `DisassociateEip`; no row drives `DeleteEnvironment`, `TagResource` or `UntagResource`. tests/all/tag_resource_idempotent_stamp.rs only BACKDATES the stamp (backdate_modified_at, lines 221-232) before asserting `after_ms >= called_at_ms`, so it cannot catch a backwards move, and it would still pass with the guard in place.

## Steps to Reproduce

1. Create an environment and let its CREATING -> CREATED transition settle (as tests/all/record_stamp_monotonicity.rs does with `settle`).
2. Place the environment and its VLANs at a known instant AHEAD of the clock with the crate's own helper `postdate_environment_and_vlans` (tests/all/record_stamp_monotonicity.rs:400-414) — the state a materialised `PendingTransition` leaves behind, since `due_at_ms` is derived from a wall-clock read taken after the monotonic read `due_at` comes from (src/state.rs:657-658).
3. Call TagResource with a tag that really changes the stored map (or UntagResource removing a stored key, or DeleteEnvironment on an environment with no live host).
4. Read the environment back with GetEnvironment: `modifiedAt` has moved BACKWARDS to the call instant, earlier than the instant the environment reached `CREATED`; for DeleteEnvironment the VLANs reported by ListEnvironmentVlans move back with it (delete_environment.rs:251), while their state is the one the same response reports.
5. Repeat steps 1-2 and call AssociateEipToVlan or DisassociateEipFromVlan instead: the stamp stays at the later instant (asserted at tests/all/record_stamp_monotonicity.rs:546-550), because those two writers go through `crate::state::note_modified_at` — the guard the three handlers above skip.
