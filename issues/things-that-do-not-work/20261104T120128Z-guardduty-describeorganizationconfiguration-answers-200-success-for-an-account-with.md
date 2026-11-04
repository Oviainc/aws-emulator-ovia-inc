Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.concurrent-update-custom-detection-rule-org-config-conflict
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.concurrent-update-custom-detection-rule-org-config-conflict|_expect_denied#11|AssertionError|raise AssertionError(unexpected_success_detail)
Issue-Commit: 82da4739d7da032589e0750937aea6e968e9e6e3

# DescribeOrganizationConfiguration answers 200 success for an account with no delegated administrator enabled, instead of BadRequestException

> _Note: this issue was identified against the code base at commit `82da4739d7da032589e0750937aea6e968e9e6e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrganizationConfiguration

## Description
This issue carries 1 root cause, in the guardduty crate.

Cause 1 of 1: DescribeOrganizationConfiguration has no delegated-administrator precondition, so it succeeds for a caller whose account is not the organization's GuardDuty delegated administrator (and where no delegated administrator has been enabled at all).

AWS: Smithy DescribeOrganizationConfiguration (GET /detector/{DetectorId}/admin, success code 200) models exactly two errors — BadRequestException (errorKind client, httpError 400) and InternalServerErrorException (500). The model documents the operation as "Returns information about the account selected as the delegated administrator for GuardDuty", i.e. it reports the delegated-administrator organization configuration and therefore presupposes one exists. Recorded behavior: against an ordinary account with an enabled detector but no GuardDuty delegated administrator enabled, the call is refused HTTP 400 BadRequestException with the message "The request failed because a delegated administrator account has not been enabled." That message is entirely fixed text for this error class — it has no variable parts taken from the request or from state (no detector id, no account id, no timestamp), so it must be reproduced verbatim. The same recording corroborates the gate on the sibling surface: for this same non-delegated-admin caller the UpdateCustomDetectionRuleOrgConfiguration racers are refused AccessDeniedException / "The request is rejected because the caller is not authorized to call this API.", while the non-gated ListCustomDetectionRules keeps answering — so the account genuinely has no delegated-admin standing, and the gate is an entitlement gate on the org-configuration surface as a whole, not a quirk of one operation.

Emulator: the call succeeds. It returns HTTP 200 with an organization-configuration body (the stored crate::state::OrganizationConfiguration projection: AutoEnable / AutoEnableOrganizationMembers, Features, DataSources, MemberAccountLimitReached) for an account that is not a delegated administrator and in a region where no delegated administrator has been enabled. The check's assertion fires on the unexpected success: "describe_organization_configuration succeeded -- this account may have become the GuardDuty delegated admin". No BadRequestException is ever produced on this path.

Why: a client relying on AWS's behavior uses this call as the authoritative probe for "is GuardDuty organization management available to me?" Against the emulator it always answers yes and hands back a fabricated organization configuration, so (a) onboarding and reconciliation code skips the EnableOrganizationAdminAccount / delegated-administrator registration step it would have to take against AWS, (b) a test asserting the delegated-admin entitlement boundary (expect 400 BadRequestException with the quoted sentence) passes on the emulator and fails in production, and (c) the caller mis-reads org-wide auto-enable settings as effective when AWS would have told it the whole org surface is not enabled. It also makes this operation useless as the corroborating read that distinguishes "caller is not eligible" from "malformed request" or "caller has no permissions at all".

Fix: a delegated-administrator precondition in front of the handler body of DescribeOrganizationConfiguration in the guardduty crate — it must refuse with the modeled BadRequestException (HTTP 400) and GuardDuty's fixed sentence whenever the account/region has no GuardDuty delegated administrator enabled, and only read the stored organization configuration once that precondition holds. The same precondition belongs on the deprecated twin of this read if the crate exposes one. Note: open issue 20261029T183757Z-guardduty-performs-no-authorization-identity-policy-deny-is.md mentions this refusal in passing inside its Cause 2 prose, but it carries no replay key for the surviving failure (its recorded key for this check was the UpdateCustomDetectionRuleOrgConfiguration racer assertion, which now passes), so it does not cover this; fixed issue 20261029T190006Z-guardduty-custom-detection-rule-org-configuration-ignores-the-delegated.md gated the five *CustomDetectionRuleOrgConfiguration handlers only and never touched this read.

Traces: guardduty.concurrent-update-custom-detection-rule-org-config-conflict (the UpdateCustomDetectionRuleOrgConfiguration racer assertions in that check now pass; it fails solely on this DescribeOrganizationConfiguration success).

## Evidence
Failing check guardduty.concurrent-update-custom-detection-rule-org-config-conflict [mismatch]:

  File "check_guardduty_concurrent_update_custom_detection_rule_org_config_conflict.py", line 60, in run
    code, message = _expect_denied(
  File "check_guardduty_concurrent_update_custom_detection_rule_org_config_conflict.py", line 91, in _expect_denied
    raise AssertionError(unexpected_success_detail)
  AssertionError: describe_organization_configuration succeeded -- this account may have become the GuardDuty delegated admin; the concurrent-update race this check exists to test is now reachable and should be exercised directly instead of asserting the org-admin-gate withdrawal

Expected (AWS, from the recording): the call is REFUSED.
  HTTP status: 400
  Error code:  BadRequestException  (the only client error the Smithy model binds to this operation)
  Message:     "The request failed because a delegated administrator account has not been enabled."
               — entirely fixed text for this error class; no part of it is taken from the request or from emulator state, so no per-run value (detector id, account id, request id, clock time) appears in it.

Actual (emulator): HTTP 200 with an organization-configuration body; no ClientError is raised at all, so the check's _expect_denied helper reaches its `raise AssertionError(unexpected_success_detail)` branch.

Corroboration from the same check, which PASSED and so is not part of this finding: the five concurrent UpdateCustomDetectionRuleOrgConfiguration racers were each refused AccessDeniedException with "The request is rejected because the caller is not authorized to call this API.", confirming the caller has no delegated-admin standing — the account state under which AWS refuses DescribeOrganizationConfiguration.

## Steps to Reproduce
1. Replay guardduty.concurrent-update-custom-detection-rule-org-config-conflict.
2. Manual repro, as a plain account with no GuardDuty delegated administrator enabled in the region:
   a. guardduty.ListDetectors; if empty, guardduty.CreateDetector(Enable=True) and keep the DetectorId.
   b. guardduty.DescribeOrganizationConfiguration(DetectorId=<that id>).
   c. The emulator answers HTTP 200 with an organization-configuration body. AWS refuses it: HTTP 400, BadRequestException, message exactly "The request failed because a delegated administrator account has not been enabled."
3. Control, to show the account really has no delegated-admin standing: guardduty.ListCustomDetectionRules to pick a RuleId, then guardduty.UpdateCustomDetectionRuleOrgConfiguration(RuleId=<that id>, Mode='DRY_RUN', IncludeAccountIds=['999999999000']) — this is correctly refused AccessDeniedException / "The request is rejected because the caller is not authorized to call this API." The same non-delegated-admin condition must make step 2b a BadRequestException.
