Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.accept-invitation-without-pending-invite-rejected guardduty.archive-findings-foreign-id-rejected guardduty.create-custom-detection-rule-association-without-permission-denied guardduty.create-filter-invalid-criteria-field-rejected guardduty.create-members-malformed-account-id-rejected guardduty.custom-detection-rule-org-configuration-lifecycle guardduty.update-ipset-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.accept-invitation-without-pending-invite-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: guardduty.archive-findings-foreign-id-rejected|run#10|AssertionError|assert code == "BadRequestException", (
Issue-Key: guardduty.create-custom-detection-rule-association-without-permission-denied|run#7|AssertionError|assert admin_code == "ResourceNotFoundException", (
Issue-Key: guardduty.create-filter-invalid-criteria-field-rejected|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: guardduty.create-members-malformed-account-id-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: guardduty.custom-detection-rule-org-configuration-lifecycle|run#7|AssertionError|assert get_code in acceptable, f"GetCustomDetectionRuleOrgConfiguration: expected one of {acceptable}, got {get_code!r}"
Issue-Key: guardduty.update-ipset-nonexistent-rejected|run#7|AssertionError|assert error["Code"] == "BadRequestException", (
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# GuardDuty negative-class rejections never reached: seven operations answer 501 NotImplemented instead of their modeled BadRequest/ResourceNotFound refusals (7 causes)

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptInvitation, ArchiveFindings, CreateFilter, CreateMembers, UpdateIPSet, CreateCustomDetectionRuleAssociation, GetCustomDetectionRuleOrgConfiguration

## Description
Seven GuardDuty operations answer HTTP 501 with error code `NotImplemented` ("operation <Op> is scaffolded but not implemented in this emulator") for requests AWS refuses with a modeled, code-bearing client error. Because the 501 is produced before any handler logic runs, none of the documented rejection classes for these operations exist in the emulator at all. Seven distinct root causes, one per operation handler, all in the guardduty service crate (services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/). Related but NOT covering issues: the suite separately filed "unimplemented scaffold stub" issues for ArchiveFindings, CreateFilter, CreateMembers and CreateIPSet against *other* checks; those issues record only that the operation is a stub and state none of the specific refusal contracts below, so each cause here must still be closed on its own terms.

Cause 1 of 7 — AcceptInvitation with no pending invitation.
AWS: Smithy: AcceptInvitation (POST /detector/{DetectorId}/master) models exactly two errors — BadRequestException (httpError 400) and InternalServerErrorException (500); there is no success path for an invitation that does not exist, and no ResourceNotFoundException is modeled, so the refusal must be BadRequestException/400. Recorded behavior: calling AcceptInvitation with a MasterId/InvitationId for which no invitation is pending is rejected with BadRequestException, and no administrator relationship is established (GetMasterAccount still reports no Master.AccountId).
Emulator: error code `NotImplemented` (501).
Why: a client that provisions a GuardDuty administrator/member relationship cannot distinguish "no such invitation" from a service outage; retry logic keyed on 400-vs-501 retries forever, and code that treats a non-400 as "invitation accepted" proceeds as if an admin relationship exists.
Fix: the AcceptInvitation handler in the guardduty crate — ops/accept_invitation.rs — must resolve the (MasterId, InvitationId) pair against invitation state and refuse an unmatched pair with BadRequestException, leaving master-account state untouched.
Traces: guardduty.accept-invitation-without-pending-invite-rejected.

Cause 2 of 7 — ArchiveFindings with a FindingId that belongs to no finding of this detector.
AWS: Smithy: ArchiveFindings (POST /detector/{DetectorId}/findings/archive) models BadRequestException (400) and InternalServerErrorException (500); FindingIds is required, @length 0-50. Recorded behavior: a well-formed but foreign FindingId is rejected with BadRequestException and message "The request is rejected because an invalid finding identifiers is specified." (example text — the requirement is the 400/BadRequestException classification for an id the detector does not own); the caller's own findings are left un-archived, and archiving a real finding by its correct id still succeeds.
Emulator: error code `NotImplemented` (501).
Why: a client doing bulk archive cannot tell a bad id from a broken service, and an emulator that never validates ids will later also accept foreign ids silently — the "overly-permissive" failure mode this check exists to catch.
Fix: the ArchiveFindings handler — ops/archive_findings.rs — must validate every FindingId against the detector's findings and refuse the batch with BadRequestException without archiving anything.
Traces: guardduty.archive-findings-foreign-id-rejected.

Cause 3 of 7 — CreateCustomDetectionRuleAssociation for a RuleId that does not exist.
AWS: Smithy: CreateCustomDetectionRuleAssociation (POST /custom-detection-rule/association) models AccessDeniedException (403), BadRequestException (400), ConflictException (409), ResourceNotFoundException (404) and InternalServerErrorException (500); RuleId is required with pattern ^[a-z0-9]+(-[a-z0-9]+)*$ and Mode is required. Recorded behavior: a fully permissioned caller passing a syntactically valid but nonexistent RuleId is refused with ResourceNotFoundException.
Emulator: error code `NotImplemented` (501).
Why: the not-found class is the only way a client can tell "this rule was never authored" from "you may not do this" (AccessDeniedException); collapsing both into 501 makes authorization and existence indistinguishable and no association state is reachable at all.
Fix: the CreateCustomDetectionRuleAssociation handler — ops/create_custom_detection_rule_association.rs — must resolve RuleId and refuse an unknown one with ResourceNotFoundException, creating no association.
Traces: guardduty.create-custom-detection-rule-association-without-permission-denied.

Cause 4 of 7 — CreateFilter with an unrecognized FindingCriteria field name.
AWS: Smithy: CreateFilter (POST /detector/{DetectorId}/filter) models BadRequestException (400) and InternalServerErrorException (500); FindingCriteria is required. Recorded behavior: a Criterion keyed by a field GuardDuty does not recognize (e.g. "this.field.does.not.exist") is refused with BadRequestException and message "The request was rejected because the parameter findingCriteria has an invalid value." (example text; the requirement is the 400/BadRequestException classification for an unknown criterion field), and no filter is left behind under that name.
Emulator: error code `NotImplemented` (501).
Why: a client that builds filter criteria from user input gets no feedback that a field name is wrong; a filter silently accepted with an unknown field matches nothing, so findings go unsuppressed or unrouted.
Fix: the CreateFilter handler — ops/create_filter.rs — must validate each FindingCriteria.Criterion key against the supported finding-criteria field set and refuse unknown keys with BadRequestException, persisting no filter.
Traces: guardduty.create-filter-invalid-criteria-field-rejected.

Cause 5 of 7 — CreateMembers with a malformed AWS account ID.
AWS: Smithy: CreateMembers (POST /detector/{DetectorId}/member) models BadRequestException (400) and InternalServerErrorException (500); AccountDetails is required, @length 1-50, and its AccountId member is an AWS account id (12 digits). Recorded behavior: a 12-character but non-numeric AccountId (e.g. "abcdefghijkl") is refused with BadRequestException rather than accepted and silently dropped.
Emulator: error code `NotImplemented` (501).
Why: member enrollment is reported through UnprocessedAccounts; a client cannot distinguish "your account id is malformed" from an outage, and an emulator that accepts malformed ids reports a member that will never exist.
Fix: the CreateMembers handler — ops/create_members.rs — must validate each AccountDetails[].AccountId as a 12-digit account id and refuse the request with BadRequestException.
Traces: guardduty.create-members-malformed-account-id-rejected.

Cause 6 of 7 — GetCustomDetectionRuleOrgConfiguration for a RuleId with no org configuration.
AWS: Smithy: GetCustomDetectionRuleOrgConfiguration (GET /custom-detection-rule/org-configuration/{RuleId}, @readonly) models AccessDeniedException (403), BadRequestException (400), ResourceNotFoundException (404) and InternalServerErrorException (500); RuleId is a required @httpLabel with pattern ^[a-z0-9]+(-[a-z0-9]+)*$ and Mode is a required @httpQuery enum. Recorded behavior: a RuleId that references no org configuration is refused with one of ResourceNotFoundException / BadRequestException / AccessDeniedException — never a success, and never a fabricated Configuration.
Emulator: error code `NotImplemented` (501).
Why: the org-configuration lifecycle has no reachable not-found boundary, so a client cannot detect that a rule configuration is absent; a 501 is also not one of the modeled error shapes, so SDK error-shape handling (ResourceNotFoundException catch blocks) never fires.
Fix: the GetCustomDetectionRuleOrgConfiguration handler — ops/get_custom_detection_rule_org_configuration.rs — must resolve (RuleId, Mode) against org-configuration state and refuse an absent configuration with one of the modeled client errors (ResourceNotFoundException for the absence case).
Traces: guardduty.custom-detection-rule-org-configuration-lifecycle.

Cause 7 of 7 — UpdateIPSet against an IpSetId that was never created.
AWS: Smithy: UpdateIPSet (POST /detector/{DetectorId}/ipset/{IpSetId}) models AccessDeniedException (403), BadRequestException (400) and InternalServerErrorException (500) — no not-found shape exists, so the absence case must be BadRequestException/400. Recorded behavior: UpdateIPSet with an unknown IpSetId is refused with BadRequestException and message "The request is rejected since no such resource found." (example text; the requirement is the 400/BadRequestException classification), and the bogus id never appears in ListIPSets.
Emulator: error code `NotImplemented` (501).
Why: a client that updates a threat list by id cannot tell that the set is gone; an emulator that later accepts the call silently would report success for an update that changed nothing.
Fix: the UpdateIPSet handler — ops/update_ip_set.rs — must resolve IpSetId within the detector and refuse an unknown id with BadRequestException, mutating no state.
Traces: guardduty.update-ipset-nonexistent-rejected.

## Evidence
Replay output, per check (expected -> actual):
- guardduty.accept-invitation-without-pending-invite-rejected [mismatch]: AssertionError: expected BadRequestException, got 'NotImplemented'
- guardduty.archive-findings-foreign-id-rejected [mismatch]: AssertionError: expected BadRequestException archiving foreign FindingId 'ffffffffffffffffffffffffffffffff' against detector <detectorId>, got 'NotImplemented'
- guardduty.create-custom-detection-rule-association-without-permission-denied [mismatch]: AssertionError: expected the permissioned caller's identical call to fail on RuleId existence (ResourceNotFoundException), got 'NotImplemented'
- guardduty.create-filter-invalid-criteria-field-rejected [mismatch]: AssertionError: expected BadRequestException, got 'NotImplemented'
- guardduty.create-members-malformed-account-id-rejected [mismatch]: AssertionError: expected BadRequestException, got 'NotImplemented'
- guardduty.custom-detection-rule-org-configuration-lifecycle [mismatch]: AssertionError: GetCustomDetectionRuleOrgConfiguration: expected one of {'ResourceNotFoundException', 'BadRequestException', 'AccessDeniedException'}, got 'NotImplemented'
- guardduty.update-ipset-nonexistent-rejected [mismatch]: AssertionError: expected BadRequestException, got 'NotImplemented'

The 501 body is the scaffold channel's text: "operation <Op> is scaffolded but not implemented in this emulator". Modeled error sets taken from services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json (AcceptInvitation, ArchiveFindings, CreateFilter, CreateMembers, UpdateIPSet, CreateCustomDetectionRuleAssociation, GetCustomDetectionRuleOrgConfiguration).

## Steps to Reproduce
1. Boot the emulator and point a botocore guardduty client at it.
2. CreateDetector(Enable=true) to get a DetectorId.
3. Cause 1: AcceptInvitation(DetectorId=..., MasterId='111122223333', InvitationId='bogus') -> observe NotImplemented/501 instead of BadRequestException/400.
4. Cause 2: CreateSampleFindings, poll ListFindings for an id, then ArchiveFindings(DetectorId=..., FindingIds=['f'*32]) -> NotImplemented instead of BadRequestException.
5. Cause 3: CreateCustomDetectionRuleAssociation(RuleId=<uuid4 hex>, Mode='DRY_RUN') -> NotImplemented instead of ResourceNotFoundException/404.
6. Cause 4: CreateFilter(DetectorId=..., Name=<name>, FindingCriteria={'Criterion': {'this.field.does.not.exist': {'Eq': ['x']}}}) -> NotImplemented instead of BadRequestException.
7. Cause 5: CreateMembers(DetectorId=..., AccountDetails=[{'AccountId': 'abcdefghijkl', 'Email': ...}]) -> NotImplemented instead of BadRequestException.
8. Cause 6: GetCustomDetectionRuleOrgConfiguration(RuleId='e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1', Mode='DRY_RUN') -> NotImplemented instead of ResourceNotFoundException.
9. Cause 7: UpdateIPSet(DetectorId=..., IpSetId='never-existed-ipset', Name='x') -> NotImplemented instead of BadRequestException.
Or replay the seven check ids listed in trace_ids.
