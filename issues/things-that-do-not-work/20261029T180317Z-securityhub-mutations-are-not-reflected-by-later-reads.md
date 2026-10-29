Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-action-target-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-action-target-lifecycle securityhub-automation-rule-and-members-lifecycle
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|DescribeActionTargets|2xx->2xx|$.ActionTargets[*].Description
Issue-Key: securityhub|GetMembers|2xx->2xx|$.UnprocessedAccounts[*]
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# securityhub mutations are not reflected by later reads: lost action-target update and spurious unprocessed account

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeActionTargets

## Description
Two securityhub read paths project state that disagrees with the mutations the same fixture already performed. 2 causes.

Cause 1 of 2 — UpdateActionTarget's new Description is not persisted (fixture securityhub-action-target-lifecycle, step task#5).
AWS: Smithy: UpdateActionTarget PATCH /actionTargets/{ActionTargetArn+} takes optional Name and Description (NonEmptyString) and returns an empty body; DescribeActionTargets POST /actionTargets/get returns ActionTargets with each target's Name and Description. AWS applies the patch, so a DescribeActionTargets issued after a successful UpdateActionTarget reports the Description that was just written.
Emulator: UpdateActionTarget answers 200, but the following DescribeActionTargets still reports the action target's ORIGINAL Description — the updated value is nowhere in the response, so the PATCH is accepted and discarded.
Why: a client that renames or re-describes a custom action and then reads it back sees its own write missing, so read-after-write consistency is broken; any flow that confirms a configuration change by re-reading it concludes the change failed, and idempotent reconcilers patch forever.
Fix: the UpdateActionTarget handler's state mutation — it must write Name/Description back into the stored action target that DescribeActionTargets projects, rather than validating and returning success only.

Cause 2 of 2 — GetMembers reports an associated member account as unprocessed (fixture securityhub-automation-rule-and-members-lifecycle, step task#10).
AWS: Smithy: GetMembersResponse has Members (MemberList) and UnprocessedAccounts (ResultList of Result{AccountId, ProcessingResult}). Recorded behavior: for the account the fixture already established against this administrator, GetMembers returns no UnprocessedAccounts entry at all — the account resolves and is reported through Members, and UnprocessedAccounts carries only accounts the administrator genuinely cannot resolve.
Emulator: adds an UnprocessedAccounts entry for that same account with ProcessingResult of the form 'The request is rejected because the account <AccountId> is not an associated member account of <AdministratorAccountId>' — an extra member that the recording does not contain. Hypothesis: the membership record written by the earlier member-lifecycle step is not visible to (or is dropped by) this later GetMembers, so association lookup fails for an account that is associated.
Why: callers treat UnprocessedAccounts as the authoritative partial-failure channel and skip or retry those accounts; an account wrongly listed there is silently excluded from member enumeration, and administrator-side tooling reports a healthy member as unenrolled.
Fix: the GetMembers handler's association lookup against member state — an account present in the administrator's member set must resolve to a Members entry and must not be appended to UnprocessedAccounts.

Traces: securityhub-action-target-lifecycle#task#5 (DescribeActionTargets after UpdateActionTarget), securityhub-automation-rule-and-members-lifecycle#task#10 (GetMembers).

## Evidence
Cause 1 — securityhub-action-target-lifecycle#task#5 DescribeActionTargets (200 vs 200): $.ActionTargets[0].Description value_mismatch — expected the post-update description ('sim2real probe action target updated'), got the pre-update one ('sim2real probe action target'), i.e. the UpdateActionTarget PATCH in the preceding step had no effect on stored state.
Cause 2 — securityhub-automation-rule-and-members-lifecycle#task#10 GetMembers (200 vs 200): $.UnprocessedAccounts[0]: extra — expected no entry, got {'AccountId': '<member-account-id>', 'ProcessingResult': 'The request is rejected because the account <member-account-id> is not an associated member account of <administrator-account-id>'}. The same fixture's earlier GetMembers (task#8) did resolve that account through Members.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus). Cause 1: fixture securityhub-action-target-lifecycle — CreateActionTarget, UpdateActionTarget on its ARN with a new Description, then DescribeActionTargets for that ARN (step task#5) and compare ActionTargets[0].Description with the value just written. Cause 2: fixture securityhub-automation-rule-and-members-lifecycle — run the member-lifecycle steps that associate the member account, then GetMembers with that AccountId at step task#10 and check that UnprocessedAccounts is empty and the account appears under Members.
