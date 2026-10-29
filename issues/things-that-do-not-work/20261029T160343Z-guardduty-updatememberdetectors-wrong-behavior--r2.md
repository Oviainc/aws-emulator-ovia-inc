Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: ae9fe3612b346aad8c571a3d654fb37d429bcc9f

# UpdateMemberDetectors advances the monotonic clock before the loop that may configure no member at all

_Filed: 2026-10-29T16:03:43Z_
_Operation: UpdateMemberDetectors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ae9fe3612b346aad8c571a3d654fb37d429bcc9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis` is not a read — it writes `self.last_millis = max(wall, last_millis + 1)` — so every call permanently advances the store's "this resource changed now" counter by at least a millisecond. `UpdateMemberDetectors` calls it at src/ops/update_member_detectors.rs:219, before the loop at :235-265 that decides whether anything is written.

A request whose `accountIds` name only accounts the detector holds no associated member record for writes NOTHING: every id falls into the `_ =>` arm, which pushes an `UnprocessedAccount` and leaves the store exactly as it found it. The call still answers 200 and still burned an instant. The same holds for an EMPTY `accountIds` list — `AccountIds` is `len 0..50`, so an empty list is a legal request and the loop body never runs.

The consequence is the one this crate's own clock suites were written to catch: a burst of such calls inside one wall-clock millisecond pushes `last_millis` past the wall clock, after which the next genuinely accepted create or update stamps a `CreatedAt`/`UpdatedAt` in the future relative to real time — a value no caller's activity produced. The crate states and implements the correct ordering in its siblings; the fix (read the clock on the path that writes) changes no successful response.

This is distinct from the already-filed `UpdateMemberDetectors` organization-auto-enable issue (20261028T231738Z, fixed), which concerns which switches are written rather than when the clock is read.

## Evidence

WHAT THE CLOCK IS — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:206-226:
```
    /// The current instant in epoch milliseconds, strictly greater than every
    /// instant this state handed out before.
    ///
    /// Call it only from a MUTATING operation, and only once that operation knows
    /// it is going to write: it is the "this resource changed now" clock, so a
    /// reporting operation must not advance it and neither may a call that leaves
    /// the store exactly as it found it — an accepted request that writes nothing
    /// would otherwise push this counter past the wall clock and make the next
    /// real change stamp a future instant.
    pub fn tick_millis(&mut self) -> i64 {
```

THE VIOLATION — src/ops/update_member_detectors.rs:217-219:
```
        // The configuration changes now, so the clock advances once for the whole
        // call: every switch this call moves carries the same instant.
        let now = state.tick_millis();
```
and the loop that may move nothing — src/ops/update_member_detectors.rs:241-264:
```
            match detector.members.get_mut(account_id) {
                Some(member) if member.is_associated() => {
                    ...
                    crate::state::apply_member_configuration(
                        member,
                        input.data_sources.as_ref(),
                        input.features.as_ref(),
                        &organization,
                        now,
                    );
                }
                // Only an account ASSOCIATED with this administrator is monitored
                // under it ... an account it never recorded, and one a disassociation
                // already moved, are both reported rather than refused — and neither
                // changes any stored member.
                _ => unprocessed.push(UnprocessedAccount {
                    account_id: account_id.clone(),
                    result: crate::state::not_a_member_message(&administrator),
                }),
            }
```
The `_ =>` arm's own comment states that neither case "changes any stored member" — so on that path the instant the tick consumed paid for no write.

THE SIBLING THAT ORDERS IT CORRECTLY — src/ops/update_malware_scan_settings.rs:186-197:
```
        // Both value members are OPTIONAL
        // (`smithy_query(operation="UpdateMalwareScanSettings")`), so a request that
        // carries neither names no setting to write: it changes nothing, so it must
        // neither move the detector's `UpdatedAt` nor consume an instant of this
        // state's "this resource changed now" counter.
        if input.ebs_snapshot_preservation.is_none() && input.scan_resource_criteria.is_none() {
            return Ok(UpdateMalwareScanSettingsOutput {});
        }

        // Past the refusals and past that no-op this is a MUTATION, so it advances
        // this state's clock.
        let now = state.tick_millis();
```
and src/ops/update_custom_detection_rule_org_configuration.rs:245-251, which reads the clock INSIDE its `if changed` block.

THE CRATE STATES THE RULE AS A CLASS RULE — tests/all/no_op_mutation_clock.rs:11-18:
```
//! The rule under test is the one the `refused_create_clock` and
//! `refused_mutation_clock` suites assert for the calls that are REFUSED, here over
//! the SET of calls that are ACCEPTED and still write nothing: a mutating operation
//! reads the service's monotonic "this resource changed now" clock only on the path
//! that actually writes, so a call which leaves the store exactly as it found it
//! consumes no instant either — and the `CreatedAt`/`UpdatedAt` a LATER accepted call
//! stamps therefore still reports the wall-clock instant of that change rather than
//! one pushed into the future by the no-ops before it.
```

TEST COVERAGE: the `rows()` of tests/all/no_op_mutation_clock.rs enumerate eleven cases (`update trusted entity set …`, `update threat entity set …`, `update ip set …`, `update threat intel set …`, `update publishing destination …`, `update malware scan settings, carrying no setting at all`, and four repeated deletes) and none of them is a member-batch operation, so nothing pins this path. tests/all/member_detector_configuration.rs asserts each member's own `updatedAt`, never the service clock.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector` with `enable: true`) and note `detectorId`.
2. Issue a burst of several hundred `POST /detector/{detectorId}/member/detector/update` (UpdateMemberDetectors) calls with `accountIds: ["111122223333"]` — an account for which no `CreateMembers` was ever run — and `features: [{name: "S3_DATA_EVENTS", status: "ENABLED"}]`. Each answers 200 with that account in `unprocessedAccounts` and, per src/ops/update_member_detectors.rs:255-264, changes no stored member.
3. Immediately create a sub-resource on the same detector (`POST /detector/{detectorId}/trustedentityset` with a name, format and location) and read the detector with `GET /detector/{detectorId}`.
4. Expected, by the ordering `UpdateMalwareScanSettings` uses (src/ops/update_malware_scan_settings.rs:186-197) and by the rule tests/all/no_op_mutation_clock.rs:11-18 states for the whole class: the detector's `updatedAt` is the wall-clock instant of that create. Actual: it sits ahead of real wall-clock time by roughly the number of no-op calls issued in the preceding millisecond, because each one ran `state.tick_millis()` at src/ops/update_member_detectors.rs:219.
5. Repeat with `accountIds: []` (legal: `AccountIds` is `len 0..50`), where the loop body never runs at all — same result.
