Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.custom-detection-rule-association-lifecycle guardduty.ipset-lifecycle-active-then-deleted guardduty.threatintelset-lifecycle guardduty.member-association-lifecycle guardduty.invite-members-dedup-idempotent guardduty.threatentityset-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.custom-detection-rule-association-lifecycle|run#11|AssertionError|assert after_create.get("ExpiresAt") is not None, (
Issue-Key: guardduty|UpdateIPSet|BadRequestException
Issue-Key: guardduty|UpdateThreatIntelSet|BadRequestException
Issue-Key: guardduty.member-association-lifecycle|run#17|AssertionError|assert still_created_status == "Created", (
Issue-Key: guardduty.invite-members-dedup-idempotent|run#12|AssertionError|assert members[0]["RelationshipStatus"] == "EmailVerificationFailed", (
Issue-Key: guardduty|CreateThreatEntitySet|BadRequestException
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GuardDuty lifecycle state is wrong: missing DRY_RUN ExpiresAt, wrong post-delete mutation error, invented member transitions, and a valid ThreatEntitySet Location refused

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDetectionRuleAssociation

## Description
This issue bundles 5 distinct root causes, all in the guardduty crate, in the state machines and representations of associations, lists and members. Fix all 5 before submitting.

Cause 1 of 5: a DRY_RUN custom-detection-rule association carries no ExpiresAt.
AWS: Smithy CreateCustomDetectionRuleAssociation returns RuleAssociation (AssociationDetail, required). Recorded behavior: an association created with Mode='DRY_RUN' comes back — and reads back via Get — with ExpiresAt set, because dry runs expire; updating the association to LIVE clears ExpiresAt and advances UpdatedAt without touching CreatedAt.
Emulator: the created DRY_RUN association has no ExpiresAt member at all (only AssociationId, Arn, RuleId, AccountId, Mode, CreatedAt, UpdatedAt).
Why: a client that schedules around a dry run's expiry, or renders 'expires in N days', has no value to read and cannot distinguish a time-boxed DRY_RUN association from a permanent LIVE one.
Fix: the DRY_RUN branch of the CreateCustomDetectionRuleAssociation handler and the AssociationDetail serializer in the guardduty crate (src/ops/create_custom_detection_rule_association.rs plus the association state it writes).
Traces: guardduty.custom-detection-rule-association-lifecycle.

Cause 2 of 5: mutating a deleted IPSet/ThreatIntelSet answers the wrong error class.
AWS: Smithy UpdateIPSet (and UpdateThreatIntelSet) model both BadRequestException (httpError 400) and InternalServerErrorException (httpError 500). Recorded behavior: after DeleteIPSet, GetIPSet reports Status DELETED, and UpdateIPSet on that id is refused with HTTP 400 but error code InternalServerErrorException and the fixed text 'The request is rejected. This operation is already in progress.' — GuardDuty's documented quirk: a client-side refusal that carries the server error code.
Emulator: BadRequestException instead, so both the code and the message diverge.
Why: clients classify InternalServerErrorException and BadRequestException differently (retry vs. do-not-retry); a caller that must treat a delete-in-progress collision as retryable gets a permanent client error instead, and the 'already in progress' signal is lost.
Fix: the deleted/deleting-resource branch of the UpdateIPSet and UpdateThreatIntelSet handlers in the guardduty crate (src/ops/update_ip_set.rs, update_threat_intel_set.rs).
Traces: guardduty.ipset-lifecycle-active-then-deleted, guardduty.threatintelset-lifecycle.

Cause 3 of 5: DisassociateMembers transitions a member that was never associated.
AWS: Smithy DisassociateMembers (AccountIds 1..50, required) returns UnprocessedAccounts. Recorded behavior: DisassociateMembers only moves members whose RelationshipStatus is Enabled or Disabled to 'Removed'; a member added by CreateMembers but never invited/accepted stays in 'Created', its record intact, and no Removed transition is fabricated.
Emulator: GetMembers reports RelationshipStatus 'Removed' for a Created-only member right after DisassociateMembers.
Why: the member relationship state machine is the contract an administrator account uses to decide whether a member needs inviting, re-inviting or deleting; inventing a Removed state makes a never-associated member look like a deliberately detached one and hides that the invitation was never completed.
Fix: the per-member status guard in the DisassociateMembers handler in the guardduty crate (src/ops/disassociate_members.rs and the member state it mutates). Note: open issue 20261029T165950Z-guardduty-disassociatemembers-wrong-behavior.md concerns the monotonic clock only.
Traces: guardduty.member-association-lifecycle.

Cause 4 of 5: InviteMembers leaves an undeliverable invitation in 'Invited'.
AWS: Smithy InviteMembers / GetMembers expose Member.RelationshipStatus. Recorded behavior: after InviteMembers for a member whose email domain cannot be reached, GetMembers reports RelationshipStatus 'EmailVerificationFailed' rather than remaining 'Invited'.
Emulator: the status stays 'Invited' indefinitely.
Why: an administrator polling for invitation outcome never learns the invitation failed and waits forever on a terminal failure state AWS exposes explicitly.
Fix: the invitation-outcome transition in the InviteMembers handler / the member state machine in the guardduty crate (src/ops/invite_members.rs). Note: open issue 20261029T142452Z-guardduty-invitemembers-wrong-behavior.md concerns the monotonic clock only.
Traces: guardduty.invite-members-dedup-idempotent.

Cause 5 of 5: CreateThreatEntitySet refuses a Location that CreateIPSet accepts.
AWS: Smithy CreateThreatEntitySet.Location (required, length 1..300, no pattern) is the same shape CreateIPSet.Location targets; recorded behavior: an https object URL of the form <s3-endpoint>/<bucket>/<key> is accepted and the set proceeds to ACTIVATING/ACTIVE.
Emulator: CreateThreatEntitySet is refused with HTTP 400 BadRequestException 'The request is rejected because the parameter location has an invalid value.' for the very Location form the emulator's own CreateIPSet accepts, so the whole create->update->delete lifecycle is unreachable.
Why: the ThreatEntitySet surface cannot be used at all with an endpoint-relative S3 object URL; the two list families disagree about what a valid Location is, which no AWS behavior justifies (and note the message also names 'location' rather than AWS's set-specific parameter wording).
Fix: the Location validation in the CreateThreatEntitySet handler, aligning it with the shared Location validator used by CreateIPSet (src/ops/create_threat_entity_set.rs).
Traces: guardduty.threatentityset-lifecycle.

## Evidence
- custom-detection-rule-association-lifecycle: 'expected a DRY_RUN association to carry ExpiresAt, got {'AssociationId': ..., 'Arn': ..., 'RuleId': 'cloudtrail-logging-disabled', 'AccountId': ..., 'Mode': 'DRY_RUN', 'CreatedAt': ..., 'UpdatedAt': ...}' — no ExpiresAt member.
- ipset-lifecycle-active-then-deleted: 'expected InternalServerErrorException mutating a deleted IPSet, got BadRequestException' (HTTP 400 matched); expected message 'The request is rejected. This operation is already in progress.'
- threatintelset-lifecycle: 'expected InternalServerErrorException, got BadRequestException' on the same post-delete Update.
- member-association-lifecycle: 'a member that never reached Enabled/Disabled must not transition on DisassociateMembers -- expected RelationshipStatus to stay 'Created', got 'Removed''.
- invite-members-dedup-idempotent: 'expected RelationshipStatus 'EmailVerificationFailed' after InviteMembers with an unreachable email domain, got 'Invited''.
- threatentityset-lifecycle: 'BadRequestException ... when calling the CreateThreatEntitySet operation: The request is rejected because the parameter location has an invalid value.' for an https <s3-endpoint>/<bucket>/<key> Location that CreateIPSet accepts in the sibling ipset-lifecycle check.

## Steps to Reproduce
1. Run the six guardduty checks in trace_ids.
2. Cause 1: CreateCustomDetectionRuleAssociation(RuleId=<a rule from ListCustomDetectionRules>, Mode='DRY_RUN') and inspect the returned RuleAssociation plus GetCustomDetectionRuleAssociation — no ExpiresAt.
3. Cause 2: CreateIPSet(Activate=True) -> DeleteIPSet -> GetIPSet (Status DELETED) -> UpdateIPSet(Activate=True): observe BadRequestException instead of InternalServerErrorException with 'The request is rejected. This operation is already in progress.'. Repeat for ThreatIntelSet.
4. Cause 3: CreateMembers for an uninvited account, GetMembers (Created), DisassociateMembers, GetMembers — observe 'Removed'.
5. Cause 4: InviteMembers for a member with an unreachable email domain, then poll GetMembers — status stays 'Invited'.
6. Cause 5: put an object in a bucket and call CreateThreatEntitySet(Format='TXT', Location='<s3 endpoint>/<bucket>/<key>', Activate=True) — observe the BadRequestException, while CreateIPSet with the identical Location succeeds.
