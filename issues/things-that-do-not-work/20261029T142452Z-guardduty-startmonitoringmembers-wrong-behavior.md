Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# StartMonitoringMembers advances the monotonic clock before the loop that may move no member at all

_Filed: 2026-10-29T14:24:52Z_
_Operation: StartMonitoringMembers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartMonitoringMembers` calls `state.tick_millis()` right after it resolves the detector, before the per-account loop. The loop then writes a member only when that member's `relationshipStatus` is `Disabled`: the handler's own comment states that "a member already monitored, and one whose invitation is still outstanding, keep their relationship and their `UpdatedAt`", and an account with no member record at all is reported in `unprocessedAccounts` with nothing persisted.

So a 200 response in which every named account is either already monitored, still `Invited`, or unknown writes nothing — but the instant has already been taken off the state's strictly-increasing clock. Since `tick_millis()` returns `last_millis + 1` whenever the wall clock has not advanced past it, each such no-write call pushes the clock one millisecond ahead, and the `UpdatedAt` the next genuinely-writing call stamps no longer reports the wall-clock instant of that change.

The ordering the rest of the crate uses is to decide the write first and read the clock only on the writing path.

## Evidence

The clock is read before the loop — `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/start_monitoring_members.rs:194-196`:

    // The relationship changes now, so the clock advances once for the whole
    // call: every member this call moves carries the same `UpdatedAt`.
    let now = state.tick_millis();

but the loop may move nothing — `start_monitoring_members.rs:213-224`:

    Some(member) if member.is_associated() => {
        // Only monitoring restarts: ... Monitoring is a desired state, so only a member
        // whose monitoring was STOPPED moves — a member already monitored,
        // and one whose invitation is still outstanding, keep their
        // relationship and their `UpdatedAt`.
        if member.relationship_status == crate::state::RELATIONSHIP_DISABLED {
            member.relationship_status = crate::state::RELATIONSHIP_ENABLED.to_string();
            member.updated_at_millis = now;
        }
    }

and `start_monitoring_members.rs:226-235`:

    // Only an account ASSOCIATED with this administrator is monitored
    // under it ... are both reported rather than refused — and neither changes any
    // stored member.
    _ => unprocessed.push(UnprocessedAccount { ... }),

The rule this breaks is the crate's own — `services/service-com-amazonaws-guardduty-guarddutyapiservice/tests/all/refused_mutation_clock.rs:13-18`:

    //! ... a mutating
    //! operation reads the service's monotonic "this resource changed now" clock only on
    //! the path that actually writes, so a refused call changes nothing at all — it
    //! neither moves a resource nor consumes an instant — and the `CreatedAt`/`UpdatedAt`
    //! a LATER accepted call stamps therefore still reports the wall-clock instant of
    //! that change rather than one pushed into the future by the refusals before it.

and `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:209-222`:

    /// Call it only from a MUTATING operation: it is the "this resource changed
    /// now" clock, and a reporting operation must not advance it.
    ...
    let next = if wall > self.last_millis { wall } else { self.last_millis.saturating_add(1) };

Sibling handlers that gate the clock on a real change: `src/ops/update_threat_entity_set.rs:287-292` and `src/ops/update_malware_scan_settings.rs:213-219` ("a request that carried no setting changed nothing, so it moves no timestamp either").

Coverage: `tests/all/member_monitoring.rs` and `tests/all/member_monitoring_restart.rs` exercise the writing path; `tests/all/refused_mutation_clock.rs` pins the clock rule only for refusal paths, so a 200 that moves no member is untested.

## Steps to Reproduce

1. Create a detector in us-east-1 and note its `detectorId`.
2. Call `POST /detector/{detectorId}/member/start` with `{"accountIds": ["111122223333"]}` — an account with no member record — and observe HTTP 200 with that account in `unprocessedAccounts` and nothing written.
3. Repeat the same call 50 times inside one wall-clock second.
4. Create a sub-resource that stamps the detector (e.g. `POST /detector/{detectorId}/ipset`), then `GET /detector/{detectorId}` and read `updatedAt`.
5. Observe `updatedAt` is ~50 ms ahead of the wall-clock instant of the IPSet create, because each no-write StartMonitoringMembers call consumed an instant of `tick_millis()`.
