Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# No operation ever writes the received-invitation inbox, so AcceptInvitation can never succeed and the whole administrator-relationship surface is dead

_Filed: 2026-10-29T03:06:58Z_
_Operation: AcceptInvitation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Detector::received_invitations` is the inbox that `AcceptInvitation` / `AcceptAdministratorInvitation` resolve `(MasterId, InvitationId)` against, that `ListInvitations` reports, that `GetInvitationsCount` sizes, and that `DeclineInvitations` / `DeleteInvitations` take entries out of. The crate's own field documentation admits that NO operation ever writes it, so it is empty for every request the emulator can answer.

The consequences are all inside this crate and all observable:

* `AcceptInvitation` and `AcceptAdministratorInvitation` reach `Detector::accept_invitation`, which returns `false` for every input because the map it looks in is always empty, so both handlers answer HTTP 400 `invitation_not_pending_message()` for every well-formed `(masterId, invitationId)` pair. Their success path is unreachable.
* `Detector::administrator` is only ever written by `accept_invitation`, so it is permanently `None`: `GetAdministratorAccount` and `GetMasterAccount` always report the empty `Administrator` / `Master` structure, and `DisassociateFromAdministratorAccount` / `DisassociateFromMasterAccount` always answer `AdministratorDisassociation::NoAdministrator` (HTTP 400).
* `ListInvitations` always reports an empty list, `GetInvitationsCount` always 0, and `DeclineInvitations` / `DeleteInvitations` always report every named account in `unprocessedAccounts`.

`InviteMembers` — the one operation that creates an invitation — writes only the administrator side of the relationship (`detector.members`, status `Invited`); it never delivers anything to an inbox, which the field doc states explicitly. So nine operations carry fully written handlers whose success path no sequence of API calls can reach, and the crate's own test suite has to reach around the API and seed `received_invitations` through the state module to exercise them at all.

This is not an unattempted feature: the inbox record type, the bounded map, the regional reader (`received_invitations_in`), the shared taker (`take_received_invitations_in`) and the acceptance write (`Detector::accept_invitation`) are all implemented — only the write that fills the map is missing, and a comment admits it.

## Evidence

THE ADMISSION — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:2363-2371:
```
    /// This emulator serves a single account ([`administrator_account_id`]) and
    /// models no way for another account to call `InviteMembers` against it, so no
    /// operation writes this map and it is EMPTY for every request the emulator
    /// answers — which is why `AcceptInvitation` refuses every well-formed pair
    /// and `ListInvitations` reports an empty inbox. `InviteMembers` does not
    /// write it either: it is called BY the administrator this emulator serves and
    /// delivers its invitation to the account it names, which is another account
    /// and so has no detector here; what it writes is the administrator side of
    /// the relationship ([`MemberRecord`]).
```
and the map it documents, state.rs:2383:
```
    pub received_invitations: BTreeMap<String, ReceivedInvitationRecord>,
```

THE UNREACHABLE SUCCESS PATH — the only read of that map, state.rs:2836-2845 (`Detector::accept_invitation`):
```
        let matches = self
            .received_invitations
            .get(administrator_account_id)
            .is_some_and(|invitation| invitation.invitation_id == invitation_id);
        if !matches {
            return false;
        }
```
and the handler that turns that `false` into the only answer it can ever give — src/ops/accept_invitation.rs:177-181:
```
        if !detector.accept_invitation(&input.master_id, &input.invitation_id, now) {
            return Err(crate::scaffolded::Scaffolded::invalid_input(
                crate::state::invitation_not_pending_message(),
            ));
        }
```
Same in src/ops/accept_administrator_invitation.rs:176-182.

WHAT `InviteMembers` WRITES INSTEAD — src/ops/invite_members.rs:240-245:
```
                    if member.relationship_status != crate::state::RELATIONSHIP_INVITED {
                        member.relationship_status =
                            crate::state::RELATIONSHIP_INVITED.to_string();
                        member.invited_at_millis = Some(now);
                        member.updated_at_millis = now;
                    }
```
(nothing in that handler touches `received_invitations`), while the model documents the other half of the flow — `smithy_query(query="doc",shape="InviteMembers")`: "The invited accounts can either accept or decline the invitation from their GuardDuty accounts."

DOWNSTREAM DEAD PATHS — `Detector::administrator` is written only by `accept_invitation` (state.rs:2846-2852) and read by src/ops/get_administrator_account.rs:170-182 and src/ops/get_master_account.rs:166-178, both of which therefore always emit the empty structure; src/ops/disassociate_from_administrator_account.rs:210-214 and src/ops/disassociate_from_master_account.rs:204-208 therefore always answer `NO_ADMINISTRATOR_MESSAGE`.

TEST COVERAGE, which reaches around the API because of this gap — tests/all/negative_class_rejections.rs:392-399:
```
/// Put one pending invitation in the detector's inbox, through this service's own
/// state module — the store the handlers read.
...
/// reason: no request this single-account emulator can answer delivers an invitation
/// to its own inbox.
```
and tests/all/invitation_listing.rs:36-40:
```
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), because the operation that
//! delivers one — `InviteMembers`, called by ANOTHER account — is a scaffold with an
//! issue of its own: this emulator serves a single account, so no request it can
//! answer puts an invitation in its own inbox.
```
No test asserts that `AcceptInvitation` must fail over the wire; the suites that cover acceptance all seed the inbox directly, so nothing pins the current behaviour.

## Steps to Reproduce

1. Create a detector: `POST /detector` with `{"enable": true}`; note the returned `detectorId`.
2. Record a member and invite it: `POST /detector/{detectorId}/member` (`CreateMembers`) with an `accountDetails` entry, then `POST /detector/{detectorId}/member/invite` (`InviteMembers`) with that account id. Both succeed.
3. Ask for the invitation inbox: `GET /invitation` (`ListInvitations`) and `GET /invitation/count` (`GetInvitationsCount`). Both report an empty inbox / 0, because `InviteMembers` wrote only `detector.members` (invite_members.rs:240-245) and nothing wrote `received_invitations` (state.rs:2363-2371).
4. Try to accept any invitation: `POST /detector/{detectorId}/master` (`AcceptInvitation`) with any well-formed `masterId` and `invitationId`. It answers HTTP 400 `InvalidInputException` / `invitation_not_pending_message()` at accept_invitation.rs:178-180, and there is no request sequence that makes it answer 200.
5. Read the relationship the acceptance would have established: `GET /detector/{detectorId}/administrator` (`GetAdministratorAccount`) returns the empty `administrator` structure, and `POST /detector/{detectorId}/administrator/disassociate` answers HTTP 400 `NO_ADMINISTRATOR_MESSAGE` — for every detector, always.
