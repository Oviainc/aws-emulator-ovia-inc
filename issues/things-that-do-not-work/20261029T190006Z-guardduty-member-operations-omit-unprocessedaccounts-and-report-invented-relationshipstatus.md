Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-member-disassociation-and-deletion
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-member-disassociation-and-deletion guardduty-member-registration-lifecycle guardduty-member-error-paths guardduty-member-invite-and-monitoring guardduty-member-monitoring-workflow
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|DisassociateMembers|2xx->2xx|$.unprocessedAccounts[*]
Issue-Key: guardduty|GetMembers|2xx->2xx|$.members[*]
Issue-Key: guardduty|GetMembers|2xx->2xx|$.unprocessedAccounts[*]
Issue-Key: guardduty|DeleteMembers|2xx->2xx|$.unprocessedAccounts[*].result
Issue-Key: guardduty|StartMonitoringMembers|2xx->2xx|$.unprocessedAccounts[*]
Issue-Key: guardduty|StopMonitoringMembers|2xx->2xx|$.unprocessedAccounts[*]
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Member operations omit unprocessedAccounts and report invented relationshipStatus transitions

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateMembers

## Description
The batch member operations drop the unprocessedAccounts entries that report per-account failures (or word them with the administrator's literal account id where AWS says 'the current account'), and the member records themselves are driven through relationship states GuardDuty does not produce. 10 causes.

Shared Smithy grounding: UnprocessedAccount models AccountId (required, @length 12..12) and Result (required, 'A reason why the account hasn't been processed'). Member models AccountId, MasterId, Email, RelationshipStatus ('The status of the relationship between the member and the administrator') and UpdatedAt as required, plus the optional DetectorId, InvitedAt and AdministratorId.

Cause 1 of 10: DisassociateMembers reports no unprocessedAccounts for accounts that are not associated.
AWS: each account that cannot be disassociated comes back as an unprocessedAccounts entry {accountId: <the id from the request>, result: 'The request is rejected because the current account cannot disassociate the given member account ID since the latter is not yet associated to it.'} — the result is fixed text, the accountId comes from the request.
Emulator: the unprocessedAccounts entries are absent, so the call reads as a clean success.
Why: this is a partial-failure API: a caller learns which accounts failed only from unprocessedAccounts. With it empty, a client believes every requested account was disassociated and skips the accounts that were not.
Fix: the DisassociateMembers handler's per-account loop, which must emit an UnprocessedAccount for every account it could not move.
Traces: guardduty-member-disassociation-and-deletion#task#4.

Cause 2 of 10: GetMembers reports 'Removed' for members that are still Created.
AWS: a member whose disassociation did not happen stays in relationshipStatus 'Created' (the value is the relationship state held in the administrator's member record).
Emulator: reports 'Removed' for those members.
Why: a client reading GetMembers to decide whether to re-invite or clean up sees members as already removed and takes the wrong branch; it is the read-side consequence of cause 1 — the emulator mutated state for accounts it should have refused.
Fix: the DisassociateMembers state transition, which must leave non-associated members untouched.
Traces: guardduty-member-disassociation-and-deletion#task#5.

Cause 3 of 10: GetMembers' unprocessedAccounts result names a literal account id where AWS says 'the current account'.
AWS: result is 'The request is rejected because the given account ID is not an associated member of the current account.' — entirely fixed text; the phrase is 'the current account', never the caller's account number.
Emulator: 'The request is rejected because the given account ID is not an associated member of account 000000000000.' — it substitutes the administrator's account id into the sentence.
Why: the message becomes environment-dependent: a client or test matching GuardDuty's fixed sentence never matches, and the emulator's variant leaks the caller's account id into text AWS keeps generic.
Fix: the shared member-lookup refusal message used by GetMembers.
Traces: guardduty-member-error-paths#task#3.

Cause 4 of 10: DeleteMembers reports the wrong reason for a still-associated member.
AWS: an account that cannot be deleted because it is still associated comes back as result 'The request is rejected because the current account cannot delete the given member account ID since it is still associated to it.' — fixed text.
Emulator: 'The request is rejected because the given account ID is not an associated member of account 000000000000.' — the opposite assertion (not a member) plus a literal account id.
Why: the caller is told the account is not a member when the real reason is that it must be disassociated first, so the obvious remediation (DisassociateMembers then DeleteMembers) is never attempted.
Fix: the DeleteMembers handler's per-account refusal classification and message.
Traces: guardduty-member-error-paths#task#5.

Cause 5 of 10: StartMonitoringMembers reports no unprocessedAccounts for non-members.
AWS: each requested account that is not an associated member comes back as {accountId: <from the request>, result: 'The request is rejected because the given account ID is not an associated member of the current account.'}.
Emulator: unprocessedAccounts is absent.
Why: as cause 1 — monitoring is silently not started for accounts the caller believes it enabled.
Fix: the StartMonitoringMembers per-account loop.
Traces: guardduty-member-invite-and-monitoring#task#4.

Cause 6 of 10: StopMonitoringMembers reports no unprocessedAccounts for non-members.
AWS: the same per-account entries as cause 5.
Emulator: absent.
Why: as cause 5, on the stop path — monitoring a caller believes it stopped may still be running.
Fix: the StopMonitoringMembers per-account loop.
Traces: guardduty-member-invite-and-monitoring#task#6.

Cause 7 of 10: StartMonitoringMembers again reports no unprocessedAccounts in the single-member monitoring workflow.
AWS: the non-member account comes back in unprocessedAccounts with the fixed 'not an associated member of the current account' result.
Emulator: absent.
Why: as cause 5; reproduced on a second fixture, so it is the handler and not one fixture's setup.
Fix: the StartMonitoringMembers per-account loop.
Traces: guardduty-member-monitoring-workflow#task#4.

Cause 8 of 10: StopMonitoringMembers again reports no unprocessedAccounts in the single-member monitoring workflow.
AWS: as cause 6.
Emulator: absent.
Why: as cause 6.
Fix: the StopMonitoringMembers per-account loop.
Traces: guardduty-member-monitoring-workflow#task#5.

Cause 9 of 10: GetMembers reports 'Disabled', a state the member never entered.
AWS: the member's relationshipStatus is 'Created' — a stop-monitoring call that was refused for a non-member does not change any relationship.
Emulator: reports 'Disabled'.
Why: 'Disabled' tells the client the administrator deliberately turned the member off, so remediation logic re-enables a member that was never disabled (and never looks at the real problem, that the account is not a member).
Fix: the StopMonitoringMembers state transition, which must not write a relationship status for accounts it refused.
Traces: guardduty-member-monitoring-workflow#task#6.

Cause 10 of 10: GetMembers' unprocessedAccounts result again names a literal account id, for two accounts at once.
AWS: both entries carry the fixed result 'The request is rejected because the given account ID is not an associated member of the current account.' with their own accountId from the request.
Emulator: both carry '...not an associated member of account 000000000000.'
Why: as cause 3; reproduced across a second fixture and multiple accounts.
Fix: the shared member-lookup refusal message.
Traces: guardduty-member-registration-lifecycle#task#8.

Related open issues (none covers these; none carries a replay key for them): 20261029T165950Z-guardduty-disassociatemembers-wrong-behavior.md, 20261029T115857Z-guardduty-stopmonitoringmembers-wrong-behavior.md, 20261029T113403Z-guardduty-deletemembers-wrong-behavior.md.

## Evidence
Cause 1: $.unprocessedAccounts[0] and [1] expected {'accountId': <id>, 'result': 'The request is rejected because the current account cannot disassociate the given member account ID since the latter is not yet associated to it.'}, got None.
Cause 2: $.members[0]/[1].relationshipStatus expected 'Created', got 'Removed'.
Cause 3: $.unprocessedAccounts[0].result expected '...not an associated member of the current account.', got '...not an associated member of account 000000000000.'
Cause 4: $.unprocessedAccounts[0].result expected 'The request is rejected because the current account cannot delete the given member account ID since it is still associated to it.', got '...is not an associated member of account 000000000000.'
Causes 5-8: $.unprocessedAccounts[0] (and [1]) expected {'accountId': <id>, 'result': 'The request is rejected because the given account ID is not an associated member of the current account.'}, got None.
Cause 9: $.members[0].relationshipStatus expected 'Created', got 'Disabled'.
Cause 10: both $.unprocessedAccounts entries expected the 'the current account' wording, got the literal-account-id wording.

## Steps to Reproduce
1. Replay guardduty-member-disassociation-and-deletion: step task#4 (DisassociateMembers on non-associated accounts) returns no unprocessedAccounts; step task#5 then shows those members as 'Removed' instead of 'Created'.
2. Replay guardduty-member-error-paths steps task#3 (GetMembers) and task#5 (DeleteMembers) for the unprocessedAccounts result wording.
3. Replay guardduty-member-invite-and-monitoring steps task#4/task#6 and guardduty-member-monitoring-workflow steps task#4/task#5: Start/StopMonitoringMembers omit unprocessedAccounts for non-members.
4. Replay guardduty-member-monitoring-workflow step task#6 ('Disabled' instead of 'Created') and guardduty-member-registration-lifecycle step task#8.
