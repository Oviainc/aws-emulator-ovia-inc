Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: macie2.list-members-only-associated-filter-toggle
Issue-Shape-Id: com.amazonaws.macie2#Macie2
Issue-Key: macie2.list-members-only-associated-filter-toggle|run#10|AssertionError|assert in_default_view is False, (
Issue-Commit: 5f5c8c74019b1fc5b04177af48d49ea878911ccb

# macie2 ListMembers associated-only view includes 'Created' members: onlyAssociated default does not exclude accounts with no established membership

> _Note: this issue was identified against the code base at commit `5f5c8c74019b1fc5b04177af48d49ea878911ccb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMembers

## Description
Cause 1 of 1: the default (onlyAssociated=true) ListMembers view wrongly includes member accounts whose relationshipStatus is 'Created'. Failing check: macie2.list-members-only-associated-filter-toggle.

AWS: Smithy model com-amazonaws-macie2-macie2.json, ListMembersRequest$onlyAssociated (__string, httpQuery "onlyAssociated"): "Specifies which accounts to include in the response, based on the status of an account's relationship with the administrator account. By default, the response includes only current member accounts. To include all accounts, set this value to false." The modeled RelationshipStatus enum is [Enabled, Paused, Invited, Created, Removed, Resigned, EmailVerificationInProgress, EmailVerificationFailed, RegionDisabled, AccountSuspended]. An account that CreateMember has only just associated carries 'Created' — the association record exists but no membership has been established for it — so AWS leaves it out of the associated-only (default) view and reports it only when onlyAssociated is explicitly "false". Recorded behavior confirms this: against real AWS, CreateMember followed by GetMember reports relationshipStatus 'Created', ListMembers with no onlyAssociated omits that account, and ListMembers(onlyAssociated="false") includes it. The model does not enumerate which statuses count as 'current', so the status-by-status partition beyond 'Created' is recorded behavior only.

Emulator: the account is present in BOTH views. The replay fails on the default view with "expected default (onlyAssociated=true) ListMembers to exclude a 'Created' member <accountId>, but it was present" (the account id is a value from the request; the surrounding text is the check's). The membership predicate the handler filters on treats every status except an explicitly ended one as current — it excludes only 'Removed' and 'Resigned' — so 'Created' (and, by the same inversion, 'EmailVerificationInProgress', 'EmailVerificationFailed', 'RegionDisabled', 'AccountSuspended') passes the associated-only filter. No status code or error code diverges here: both calls return HTTP 200; only the membership set in the body is wrong.

Why: a client uses the two views to tell apart accounts it has merely created an association for from accounts that are actually administered members — it reads the default view as "my current members" and onlyAssociated=false as "everything I have a record of". Against the emulator the two views are identical for a freshly created member, so a caller that enumerates current members to drive enrollment, per-member job scoping or administrator-side reporting counts not-yet-established associations as live members, and the onlyAssociated toggle appears to have no effect. Conversely a caller waiting for an account to APPEAR in the default view as its signal that membership took effect sees that signal fire immediately and never actually waits.

Fix: the member-plane state's membership predicate — the 'is this a current association' helper the ListMembers handler filters its enumeration with — should classify a status as current only when a membership is established, rather than defining 'current' as the complement of a two-entry ended-status list. The ListMembers handler itself already reads onlyAssociated correctly (absent or anything other than a case-insensitive "false" means associated-only); the defect is entirely in what that predicate admits.

Note: the open issue 20261101T224706Z-macie2-listmembers-stub.md ("Implement macie2 ListMembers: unimplemented scaffold stub") does NOT cover this — ListMembers is implemented and answers 200 here; this is a fidelity defect in its filtering, not a missing operation.

Traces: macie2.list-members-only-associated-filter-toggle.

## Evidence
Check macie2.list-members-only-associated-filter-toggle, verdict mismatch.

Sequence: EnableMacie (if no session) -> CreateMember{account:{accountId:<doc account>, email:<address>}, tags:{...}} -> GetMember{id:<accountId>} -> ListMembers (no onlyAssociated) -> ListMembers(onlyAssociated="false").

Expected (AWS): GetMember reports relationshipStatus 'Created'; ListMembers with onlyAssociated unset returns a members list NOT containing <accountId>; ListMembers(onlyAssociated="false") returns a members list containing <accountId>. Both ListMembers calls are HTTP 200.

Actual (emulator): GetMember reports 'Created' as expected, but the default ListMembers call also contains <accountId>. Assertion raised:
  AssertionError: expected default (onlyAssociated=true) ListMembers to exclude a 'Created' member 444455556666, but it was present
(the fixed text is the check's; 444455556666 is the request's accountId and is not part of the requirement). The onlyAssociated=false call would have passed, so the two views are indistinguishable.

Source of the divergence in the checkout: services/service-com-amazonaws-macie2-macie2/src/ops/list_members.rs lines 218-222 filter with member.is_current_association(); services/service-com-amazonaws-macie2-macie2/src/state.rs lines 211-216 and 404-409 define that predicate as !ENDED_RELATIONSHIP_STATUSES.contains(status) with ENDED_RELATIONSHIP_STATUSES = ["Removed", "Resigned"], so MEMBER_CREATED_RELATIONSHIP_STATUS ("Created", state.rs line 209) is admitted as a current association.

## Steps to Reproduce
1. Point a botocore macie2 client at the emulator.
2. If get_macie_session raises ResourceNotFoundException/AccessDeniedException, call enable_macie().
3. macie.create_member(account={"accountId": "<any account id not already a member>", "email": "<any address>"}).
4. macie.get_member(id="<that account id>") -> observe relationshipStatus == "Created".
5. macie.list_members() with NO onlyAssociated argument (paging nextToken to exhaustion) -> the emulator returns <that account id> among members; AWS does not.
6. macie.list_members(onlyAssociated="false") (paging to exhaustion) -> <that account id> is present, as AWS does.
7. Step 5 must omit the account while step 6 includes it; the emulator returns it in both.
Cleanup: macie.delete_member(id="<that account id>") and disable_macie() if step 2 enabled it.
