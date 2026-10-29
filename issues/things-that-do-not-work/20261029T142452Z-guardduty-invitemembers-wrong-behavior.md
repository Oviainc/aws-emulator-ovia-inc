Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# InviteMembers reads the monotonic clock before it knows whether any invitation is sent, so a call that invites nobody still consumes an instant

_Filed: 2026-10-29T14:24:52Z_
_Operation: InviteMembers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InviteMembers` advances the service's monotonic "this resource changed now" clock (`state.tick_millis()`) immediately after it resolves the detector, BEFORE the per-account loop that decides whether anything is written. An account the administrator holds no member record for is reported in `unprocessedAccounts` and nothing is persisted for it; the handler even tracks this with its own `sent` flag and skips the inbox write when `sent` is false. So a call whose every named account is unprocessed writes nothing at all, yet it has already spent an instant of the clock.

Because `tick_millis()` is strictly increasing (`state.rs` keeps `last_millis` and returns `last_millis + 1` whenever the wall clock has not moved past it), every such no-write call pushes the clock one millisecond into the future. The `CreatedAt`/`UpdatedAt` the NEXT genuinely-writing call stamps is therefore no longer the wall-clock instant of that change: it is the wall clock plus one per preceding no-write `InviteMembers`. That is exactly the outcome the crate's own clock rule forbids.

The fix is the ordering every sibling mutation in this crate already uses: decide what the call will write first, and read the clock only on the path that writes.

## Evidence

The handler reads the clock before it knows it will write — `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/invite_members.rs:222-225`:

    // The invitations this call sends are sent NOW, so the clock advances once for
    // the whole call: every member it invites carries the same `InvitedAt` and the
    // same `UpdatedAt`.
    let now = state.tick_millis();

but the same handler knows a call can send nothing — `invite_members.rs:234` `let mut sent = false;`, `invite_members.rs:260-269`:

    // An account this administrator holds no member record for cannot be
    // invited — GuardDuty has to be enabled in the account and the
    // account recorded first — so it is reported rather than refused, and
    // nothing is persisted for it. The administrator's own account is one
    // of these: it is never one of its own members.
    None => unprocessed.push(UnprocessedAccount { ... }),

and `invite_members.rs:276-288`:

    // ... A call that invited nobody
    // sent nothing, so it delivers nothing.
    ...
    if sent {
        state.deliver_invitation_in(&region, &administrator, now);
    }

The rule this breaks is the crate's own, stated in its test suite — `services/service-com-amazonaws-guardduty-guarddutyapiservice/tests/all/refused_mutation_clock.rs:13-18`:

    //! request NAMES and over the SET of ways each of them is refused: a mutating
    //! operation reads the service's monotonic "this resource changed now" clock only on
    //! the path that actually writes, so a refused call changes nothing at all — it
    //! neither moves a resource nor consumes an instant — and the `CreatedAt`/`UpdatedAt`
    //! a LATER accepted call stamps therefore still reports the wall-clock instant of
    //! that change rather than one pushed into the future by the refusals before it.

and on the clock itself — `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:209-212`:

    /// Call it only from a MUTATING operation: it is the "this resource changed
    /// now" clock, and a reporting operation must not advance it.

with the monotonic bump that makes it observable — `state.rs:213-222`:

    pub fn tick_millis(&mut self) -> i64 {
        let wall = now_millis();
        let next = if wall > self.last_millis { wall } else { self.last_millis.saturating_add(1) };

Sibling handlers that DO gate the stamp on having really written: `src/ops/update_threat_entity_set.rs:287-292` ("`UpdatedAt` moves only when this call really changed the set, so an update that writes back the values the set already held reports what it did: nothing.") and `src/ops/update_malware_scan_settings.rs:213-219` ("a request that carried no setting changed nothing, so it moves no timestamp either").

Coverage: `tests/all/refused_mutation_clock.rs` and `tests/all/refused_create_clock.rs` pin this rule for the refusal paths of the delete/update/accept and create families; neither suite has a row for a 200-with-everything-unprocessed `InviteMembers`, which is why this path slipped through.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector` with `{"enable": true}`) and note its `detectorId`.
2. Call `POST /detector/{detectorId}/member/invite` with `{"accountIds": ["111122223333"]}` — an account id no `CreateMembers` ever recorded — and observe HTTP 200 with that account in `unprocessedAccounts`: no member record changed and no invitation was delivered.
3. Repeat that same call 50 times in quick succession (well inside one wall-clock second).
4. Create any sub-resource that stamps the detector, e.g. `POST /detector/{detectorId}/ipset`, then `GET /detector/{detectorId}` and read `updatedAt`.
5. Observe that `updatedAt` is roughly 50 ms later than the wall-clock instant of the IPSet create: the 50 no-write InviteMembers calls each consumed an instant of `tick_millis()`, although by the handler's own `sent` flag they wrote nothing.
