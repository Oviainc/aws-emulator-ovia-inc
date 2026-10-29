Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# DisassociateMembers advances the monotonic clock before the loop that may move no member at all

_Filed: 2026-10-29T16:59:50Z_
_Operation: DisassociateMembers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisassociateMembers` calls `state.tick_millis()` immediately after its refusals and before the per-account loop (disassociate_members.rs:225). The loop writes a member only when the account resolves to a member record that `is_associated()`; an account with no member record, and one a disassociation already moved to `Removed`, are both reported in `unprocessedAccounts` with nothing persisted — the handler's own comment says "neither changes any stored member".

So a 200 response in which every named account is unknown or already disassociated writes nothing, yet the instant has already been taken off the state's strictly-increasing clock. `tick_millis()` returns `last_millis + 1` whenever the wall clock has not moved past it (state.rs:217-226), so each such no-write call pushes the counter one millisecond into the future, and the `CreatedAt`/`UpdatedAt` the next genuinely-writing call stamps no longer reports the wall-clock instant of that change.

The crate states the rule — read the clock only on the path that actually writes — and four sibling updates implement it by deciding the write first (`UpdateIPSet`, `UpdateThreatEntitySet`, `UpdateTrustedEntitySet`, `UpdatePublishingDestination` all return before `tick_millis()` when the request asks for the state the record is already in). The fix is that ordering and changes no successful response. (`StopMonitoringMembers` at stop_monitoring_members.rs:216 has the same shape; its own wrong-behavior slot is already filed under a different defect, so it is named here only as the sibling.)

## Evidence

THE CLOCK IS READ BEFORE THE LOOP — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/disassociate_members.rs:223-225:
```
        // The relationship changes now, so the clock advances once for the whole
        // call: every member this call moves carries the same `UpdatedAt`.
        let now = state.tick_millis();
```
BUT THE LOOP MAY MOVE NOTHING — src/ops/disassociate_members.rs:241-256:
```
            match detector.members.get_mut(account_id) {
                Some(member) if member.is_associated() => {
                    ...
                    member.relationship_status = crate::state::RELATIONSHIP_REMOVED.to_string();
                    member.updated_at_millis = now;
                }
                // Only an account ASSOCIATED with this administrator can be
                // disassociated from it: an account it never recorded, and one a
                // disassociation already moved, are both reported rather than
                // refused — and neither changes any stored member.
                _ => unprocessed.push(UnprocessedAccount {
                    account_id: account_id.clone(),
                    result: crate::state::NOT_ASSOCIATED_MESSAGE.to_string(),
                }),
            }
```

WHAT THE CLOCK IS, AND THE RULE — src/state.rs:206-226:
```
    /// Call it only from a MUTATING operation, and only once that operation knows
    /// it is going to write: it is the "this resource changed now" clock, so a
    /// reporting operation must not advance it and neither may a call that leaves
    /// the store exactly as it found it — an accepted request that writes nothing
    /// would otherwise push this counter past the wall clock and make the next
    /// real change stamp a future instant.
    pub fn tick_millis(&mut self) -> i64 {
        let wall = now_millis();
        let next = if wall > self.last_millis { wall } else { self.last_millis.saturating_add(1) };
```
and the suite that asserts it as a class rule — tests/all/no_op_mutation_clock.rs:11-18:
```
//! The rule under test is the one the `refused_create_clock` and
//! `refused_mutation_clock` suites assert for the calls that are REFUSED, here over
//! the SET of calls that are ACCEPTED and still write nothing: a mutating operation
//! reads the service's monotonic "this resource changed now" clock only on the path
//! that actually writes, so a call which leaves the store exactly as it found it
//! consumes no instant either
```

SIBLINGS THAT GATE THE CLOCK ON A REAL CHANGE — src/ops/update_publishing_destination.rs:197-207:
```
        if !changes {
            // Nothing is written, so nothing moves: ... and the
            // clock keeps the instant the next real change is entitled to.
            return Ok(UpdatePublishingDestinationOutput {});
        }
        // Past the refusals and past that no-op this is a MUTATION: ...
        let now = state.tick_millis();
```
and src/ops/update_malware_scan_settings.rs:191-197 (returns before the tick when the request carries no setting).

TEST COVERAGE: tests/all/member_accounts.rs and tests/all/member_deletion.rs exercise the writing path; tests/all/no_op_mutation_clock.rs:89-291 enumerates only the entity-set/ipset/threat-intel-set/publishing-destination/scan-settings rows, and tests/all/refused_mutation_clock.rs pins the rule only for REFUSED calls — so a 200 that moves no member is untested.

## Steps to Reproduce

1. CreateDetector in us-east-1 (`POST /detector` with `enable: true`) and note `detectorId`.
2. Call `POST /detector/{detectorId}/member/disassociate` with `{"accountIds": ["111122223333"]}` — an account with no member record. Observe HTTP 200 with that account in `unprocessedAccounts` and nothing written.
3. Repeat the same call 300 times inside one wall-clock second.
4. Create a sub-resource that stamps the detector (`POST /detector/{detectorId}/ipset`), then `GET /detector/{detectorId}` and read `updatedAt`.
5. Observe `updatedAt` sitting ahead of the wall-clock instant of the IPSet create by roughly the number of no-write DisassociateMembers calls, because each consumed an instant of `tick_millis()`. Contrast with the same burst of no-op `POST /detector/{detectorId}/malware-scan-settings` with body `{}`, which returns before the tick (update_malware_scan_settings.rs:191-193) and leaves the clock where the wall clock is.
