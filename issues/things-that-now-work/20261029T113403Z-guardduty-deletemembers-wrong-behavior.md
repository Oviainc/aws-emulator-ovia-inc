Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 11cecedcb82efd4bcd73e8891069557293144d32

# Adding or dropping a record in the detector's `members` collection leaves the detector's `UpdatedAt` where it was, the one detector collection that answers this differently from its siblings

_Filed: 2026-10-29T11:34:03Z_
_Operation: DeleteMembers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `11cecedcb82efd4bcd73e8891069557293144d32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other collection that hangs off `crate::state::Detector` moves the detector's own `updated_at_millis` when a call changes WHICH records it holds: the filter, IPSet, ThreatIntelSet, ThreatEntitySet, TrustedEntitySet and publishing-destination creates and deletes each write `detector.updated_at_millis = now` and each says so in a comment that names the rule as a class rule, not a per-family one. `detector.members` is the exception. `CreateMembers` inserts a brand-new `MemberRecord` into `detector.members` and `DeleteMembers` removes records from it, and neither touches `detector.updated_at_millis` — `DeleteMembers` does not even read the clock. So `GetDetector` reports the same `UpdatedAt` before and after the detector gained or lost member accounts, while the identical change to any other collection of the same detector moves it.

This is not the update-vs-add distinction the crate draws elsewhere: `UpdateIPSet` and `UpdatePublishingDestination` deliberately leave the detector's `UpdatedAt` alone because "the set of sub-resources it holds is unchanged" / "it holds the same destinations it did before". `CreateMembers` and `DeleteMembers` do change that set, which is exactly the condition every sibling writes the timestamp on. The crate's own state doc also places `members` in the same ownership class as the filters ("`DeleteDetector` drops them with the detector, like the findings and filters above").

The two sites to fix are `create_members.rs:317` (the `None =>` insert branch, which already has `now` in scope from `create_members.rs:225`) and `delete_members.rs:230` (the `remove`, which needs a clock read placed after the refusals the way `delete_filter.rs` places it). `InviteMembers`, `DisassociateMembers`, `StartMonitoringMembers` and `StopMonitoringMembers` are NOT part of this: they move a member's relationship without changing which members the detector holds, which is the `UpdateIPSet` case.

## Evidence

THE RULE, as every sibling delete states it — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/delete_filter.rs:184-188:
```
        // The detector holds one fewer sub-resource than before, which is the same
        // change `CreateFilter` records on it in the other direction and the same one
        // the other saved sub-resource deletes record. A refused delete returns above
        // and so moves nothing.
        detector.updated_at_millis = now;
```
and src/ops/delete_threat_entity_set.rs:201-207:
```
        if accepted {
            // The detector holds one fewer LIVE sub-resource than before, which is
            // the same change `CreateThreatEntitySet` records on it in the other
            // direction. A repeated delete changed nothing, so it moves nothing here
            // either.
            detector.updated_at_millis = now;
        }
```
and the create half — src/ops/create_publishing_destination.rs:271-272:
```
        // The detector itself changed: it holds one more sub-resource than before.
        detector.updated_at_millis = now;
```
The same line is present in every other create and delete of a detector collection: src/ops/create_filter.rs:323, src/ops/create_ip_set.rs:256, src/ops/create_threat_intel_set.rs:283, src/ops/create_threat_entity_set.rs:275, src/ops/create_trusted_entity_set.rs:273, src/ops/delete_ip_set.rs:190, src/ops/delete_threat_intel_set.rs:202, src/ops/delete_trusted_entity_set.rs:205, src/ops/delete_publishing_destination.rs:204-206.

THE VIOLATION, delete side — src/ops/delete_members.rs:224-236 (the whole write; the file contains no `tick_millis` and no `updated_at_millis` at all):
```
            // The RECORD is the condition here, not the association: deleting is what
            // removes the details a disassociation retains, so a member whose
            // relationship a disassociation already ended is dropped like any other.
            ...
            if detector.members.remove(account_id).is_none() {
                unprocessed.push(UnprocessedAccount { ... });
            }
```

THE VIOLATION, create side — src/ops/create_members.rs:317, inside the `None =>` branch that mints a new member, with `now` already in hand from src/ops/create_members.rs:225 (`let now = state.tick_millis();`):
```
                    detector.members.insert(detail.account_id.clone(), member);
```
and nothing after the loop (src/ops/create_members.rs:320-325) writes `detector.updated_at_millis`.

MEMBERS ARE ONE OF THOSE COLLECTIONS — src/state.rs:2407-2410:
```
    /// The member accounts this detector's account administers, keyed by account
    /// id. `DeleteDetector` drops them with the detector, like the findings and
    /// filters above.
    pub members: BTreeMap<String, MemberRecord>,
```

THE CRATE STATES THE RULE AS A CLASS RULE — tests/all/detector_sub_resource_deletes.rs:10-22:
```
//! The rule under test: a call that changes WHICH sub-resources a detector holds
//! changes the detector record itself, so `GetDetector`'s `UpdatedAt` moves to the
//! instant of that call ... The answer
//! does not depend on WHICH collection the sub-resource lives in ... one rule
//! covers them all.
...
//! This suite is the DELETE half over the whole class, so no
//! family can answer this question differently from its siblings.
```
and tests/all/detector_sub_resource_creates.rs:10-15 / :46-48:
```
//! The rule under test, asserted over the SET of sub-resource families a detector
//! holds rather than over one of them: a call that changes the set of sub-resources a
//! detector holds changes the detector, so `GetDetector`'s `UpdatedAt` moves to the
//! instant of that change
...
/// Every sub-resource family whose create is implemented, so the rule is asserted on
/// the whole class and not on the one family a recorded call cited.
```

TEST COVERAGE: neither suite enumerates the member family — `families()` in tests/all/detector_sub_resource_creates.rs:53 onward lists only `ipset`, `threatintelset`, `threatentityset`, `trustedentityset` and `publishingDestination`, and the delete suite's rows come from the same set (`smithy_query(operation="CreateFilter,CreateIPSet,CreateThreatIntelSet,CreateThreatEntitySet,CreateTrustedEntitySet,CreatePublishingDestination")`, tests/all/detector_sub_resource_deletes.rs:29-31). The member suites (tests/all/member_creation.rs, tests/all/member_deletion.rs, tests/all/member_accounts.rs) assert only each MEMBER record's own `updatedAt` (e.g. tests/all/member_creation.rs:134-145 reads `member["updatedAt"]` through `GetMembers`); no test reads `GetDetector`'s `updatedAt` across a `CreateMembers`/`DeleteMembers`, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateDetector in us-east-1 (`POST /detector` with `enable: true`) and note `detectorId`.
2. `GET /detector/{detectorId}` and record `updatedAt` as T0.
3. CreateMembers (`POST /detector/{detectorId}/member` with `accountDetails: [{accountId: "111122223333", email: "member@example.com"}]`) → HTTP 200, `unprocessedAccounts: []`.
4. `GET /detector/{detectorId}` again. Expected, by the rule tests/all/detector_sub_resource_creates.rs:10-15 states for every collection a detector holds: `updatedAt` has moved past T0, the way it does after `POST /detector/{detectorId}/ipset`. Actual: it is still T0.
5. DeleteMembers (`POST /detector/{detectorId}/member/delete` with `accountIds: ["111122223333"]`) → HTTP 200, `unprocessedAccounts: []`; `GET /detector/{detectorId}/member` now reports no members.
6. `GET /detector/{detectorId}` once more. Expected, by delete_filter.rs:184-188 ("The detector holds one fewer sub-resource than before … the same one the other saved sub-resource deletes record"): `updatedAt` has moved again. Actual: still T0.
7. For contrast, repeat steps 3-6 with a trusted entity set instead of a member (`POST /detector/{detectorId}/trustedentityset`, then `DELETE /detector/{detectorId}/trustedentityset/{id}`): the detector's `updatedAt` moves on both calls.
