Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ae9fe3612b346aad8c571a3d654fb37d429bcc9f

# Answering an invitation leaves the administrator-side member record asserting one is still pending, so the next InviteMembers delivers a fresh invitation while the member keeps the answered invitation's invitedAt

_Filed: 2026-10-29T16:03:43Z_
_Operation: DeclineInvitations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ae9fe3612b346aad8c571a3d654fb37d429bcc9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InviteMembers` writes BOTH halves of one send in one call: the administrator's own `MemberRecord` (`relationshipStatus` -> `Invited`, `invitedAt`, `updatedAt`) and the inbox the Region's endpoint serves (a `ReceivedInvitationRecord` with a minted `invitationId` and its own `invitedAt`). `DeclineInvitations` (and `DeleteInvitations`, which drives the identical helper) removes only the INBOX half and leaves the administrator-side member record exactly where the send put it.

The two halves then disagree about the one thing the send is guarded on. The inbox half asks the inbox itself whether an invitation is pending (`Detector::inbox_has_room_for` -> `!received_invitations.contains_key(..)`), so after a decline it mints a FRESH invitation with a new id and a new `invitedAt`. The member half asks the RELATIONSHIP STATUS instead (`member.relationship_status != RELATIONSHIP_INVITED`), which a decline never moved, so the second send writes nothing on the member at all.

Observable consequence: after Invite -> Decline -> Invite, `GetMembers`/`ListMembers` report `relationshipStatus: "Invited"` with the `invitedAt` of the invitation that was declined and an `updatedAt` that does not record the second send, while `ListInvitations` reports a different `invitationId` with a later `invitedAt`. The member's `invitedAt` therefore names an instant at which no invitation this store holds was sent. The same divergence arises after `DeleteInvitations` and after `AcceptAdministratorInvitation`/`AcceptInvitation`, all three of which take the inbox record out without touching the administrator-side member record.

Either half can carry the fix — the decline/delete/accept path can move the member record out of `Invited`, or `InviteMembers` can guard on the inbox the way the delivery half does — but today the two definitions of "still pending" are in the same crate and contradict each other.

## Evidence

THE SEND WRITES BOTH HALVES — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/invite_members.rs:272-288:
```
        // An invitation that was sent is pending for the account it was delivered to
        // until that account answers it, so the send writes the MEMBER side of the
        // relationship too: it delivers the invitation to the inbox the Region's
        // endpoint serves, ...
        if sent {
            state.deliver_invitation_in(&region, &administrator, now);
        }
```

THE MEMBER HALF'S GUARD, AND THE REASON IT STATES — src/ops/invite_members.rs:249-258:
```
                    // One invitation is pending per administrator/member pair, so
                    // inviting an account whose invitation is still pending re-sends
                    // that same invitation rather than issuing a second one: nothing
                    // about the member moves, `InvitedAt` and `UpdatedAt` included.
                    if member.relationship_status != crate::state::RELATIONSHIP_INVITED {
                        member.relationship_status =
                            crate::state::RELATIONSHIP_INVITED.to_string();
                        member.invited_at_millis = Some(now);
                        member.updated_at_millis = now;
                    }
```

THE INBOX HALF USES A DIFFERENT DEFINITION OF "PENDING", AND SAYS SO — src/state.rs:630-637 (`deliver_invitation_in`'s own doc):
```
    /// ONE invitation is pending per inviting account at a time ... so an account whose
    /// invitation is still pending KEEPS it: re-sending re-sends that same
    /// invitation rather than issuing a second one, exactly as the member record a
    /// re-send leaves where it was, and no id is minted for it. An account whose
    /// invitation has been accepted, declined or deleted has none pending, so a
    /// later send delivers a fresh one.
```
and the predicate it uses — src/state.rs:2958-2963:
```
    pub fn inbox_has_room_for(&self, administrator_account_id: &str) -> bool {
        !self
            .received_invitations
            .contains_key(administrator_account_id)
            && self.received_invitations.len() < MAX_INVITATIONS_PER_DETECTOR
    }
```
so the fresh delivery stamps a NEW instant — src/state.rs:2938-2946:
```
        self.received_invitations.insert(
            administrator_account_id.to_string(),
            ReceivedInvitationRecord {
                administrator_account_id: administrator_account_id.to_string(),
                invitation_id: invitation_id.to_string(),
                relationship_status: RELATIONSHIP_INVITED.to_string(),
                invited_at_millis: now,
            },
        );
```

THE DECLINE WRITES ONLY THE INBOX HALF — src/ops/decline_invitations.rs:169-181:
```
        // The same write `DeleteInvitations` drives, through the same state helper: a
        // declined invitation is no longer pending, so it leaves the inbox and cannot
        // be accepted afterwards. Nothing else is persisted ...
        let unprocessed = state.take_received_invitations_in(&region, &input.account_ids);
```
and the helper it reaches — src/state.rs:596-610 (`delete_received_invitation_in`), which removes the inbox entry and writes nothing on the detector; its doc (src/state.rs:587-591) accounts only for the INVITATION's own instant:
```
    /// No instant is read: an invitation carries only the instant it was SENT
    /// ([`ReceivedInvitationRecord::invited_at_millis`], the one member of
    /// `Invitation` that is a time — `smithy_query(shape="Invitation")`), and the
    /// record that held it is gone, so there is nothing left for a clock to
    /// stamp.
```

WHAT THE MEMBER READS THEN REPORT — src/state.rs:2639-2643:
```
        relationship_status: member.relationship_status.clone(),
        // Present only once an invitation has been sent to the account, which is
        // what `InviteMembers` does; ...
        invited_at: member.invited_at_millis.map(iso8601),
```

TEST COVERAGE: tests/all/invitation_delivery.rs:30-33 states rule 3 — "One invitation is pending per inviting account: a re-send while it is pending re-sends that same invitation ... while a send after it has been answered delivers a FRESH invitation under a different id" — and tests/all/invitation_delivery.rs:306-389 asserts it over the rows `[still pending, accepted, declined, deleted]`, but every assertion in that test reads the INBOX (`only_invitation`, `invitation_id`, `invitations_count`); none reads `GetMembers`/`ListMembers` after the second send, so nothing pins the administrator-side record. tests/all/invitation_decline.rs:10-31 enumerates the decline suite's four rules and none of them concerns the administrator's member records (its only `invitedAt` assertion, :168-171, is on the KEPT inbox invitation). So the divergence is unpinned.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector` with `enable: true`) and note `detectorId`.
2. `POST /detector/{detectorId}/member` (CreateMembers) with `accountDetails: [{accountId: "111122223333", email: "member@example.com"}]` -> 200.
3. `POST /detector/{detectorId}/member/invite` (InviteMembers) with `accountIds: ["111122223333"]` -> 200.
4. `POST /detector/{detectorId}/member/get` (GetMembers) with `accountIds: ["111122223333"]`: record `relationshipStatus: "Invited"` and `invitedAt` as T1. `GET /invitation` (ListInvitations) reports one invitation with `invitationId` I1 and `invitedAt` T1.
5. `POST /invitation/decline` (DeclineInvitations) with `accountIds: [<the accountId ListInvitations reported>]` -> 200; `GET /invitation` now reports no invitation (pinned by tests/all/invitation_decline.rs:12-17).
6. Wait a few milliseconds, then `POST /detector/{detectorId}/member/invite` again with `accountIds: ["111122223333"]` -> 200.
7. `GET /invitation`: a FRESH invitation I2 (!= I1) with `invitedAt` T2 > T1, exactly as src/state.rs:630-637 documents and tests/all/invitation_delivery.rs:371-379 asserts.
8. `POST /detector/{detectorId}/member/get` again. Expected, by invite_members.rs:249-252's own reason (which only holds while "an invitation is still pending") and by the delivery half it is paired with: the member reports the second send — `invitedAt` T2 and an `updatedAt` at T2. Actual: `invitedAt` is still T1 and `updatedAt` is still T1, so the member record names the instant of the invitation that was declined while the inbox holds a different one sent at T2.
9. Repeat steps 5-8 with `POST /invitation/delete` (DeleteInvitations) and with `POST /detector/{detectorId}/administrator/accept` (AcceptAdministratorInvitation) in place of the decline: the same divergence appears, since all three take the inbox record out without touching the member record.
