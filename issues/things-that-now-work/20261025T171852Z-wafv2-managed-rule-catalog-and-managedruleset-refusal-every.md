Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.managed-rule-group-describe-catalog-contents wafv2.put-managed-rule-set-versions-invalid-version-format-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.managed-rule-group-describe-catalog-contents|run#7|AssertionError|assert aws_groups["AWSManagedRulesCommonRuleSet"].get("VersioningSupported") is True, \
Issue-Key: wafv2.put-managed-rule-set-versions-invalid-version-format-rejected|run#9|AssertionError|assert code == "WAFNonexistentItemException", f"got {code!r} instead"
Issue-Commit: f3b5c182376fbd512bc0d8c0af2ad2661dfcff47

# wafv2 managed-rule catalog and ManagedRuleSet refusal: every AWS-vendor rule group reports VersioningSupported=false, and PutManagedRuleSetVersions on a nonexistent set answers WAFInvalidParameterException (2 causes)

> _Note: this issue was identified against the code base at commit `f3b5c182376fbd512bc0d8c0af2ad2661dfcff47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAvailableManagedRuleGroups

## Description
Two net-new root causes in the wafv2 crate, both about the managed-rule surface. Neither is covered by an open issue: 20261025T032742Z-wafv2-managed-rule-group-version-expiry-is-not.md covers per-version EXPIRY state (WAFExpiredManagedRuleGroupVersionException) and says nothing about the catalog's VersioningSupported flag; 20261024T065754Z-wafv2-control-plane-referenced-ipset-deletes-below-minimum.md owns a different PutManagedRuleSetVersions check (put-managed-rule-set-versions-invalid-version-rejected, the malformed-version class) and not the nonexistent-target class below.

=== Cause 1 of 2: ListAvailableManagedRuleGroups reports the AWS-vendor catalog as unversioned ===
Failing check: wafv2.managed-rule-group-describe-catalog-contents

AWS: VersioningSupported is catalog reference data per managed rule group, not a request echo. Smithy (com-amazonaws-wafv2-awswaf-20190729): ListAvailableManagedRuleGroups -> ManagedRuleGroups: list<ManagedRuleGroupSummary>; ManagedRuleGroupSummary.VersioningSupported -> Boolean with @default false, documented "Indicates whether the managed rule group is versioned. If it is, you can retrieve the versions list by calling ListAvailableManagedRuleGroupVersions." The AWS-vendored rule-set groups ARE versioned and must report VersioningSupported=true (AWSManagedRulesCommonRuleSet is the one asserted here, and ListAvailableManagedRuleGroupVersions is defined for exactly those groups); the two IP-list groups, AWSManagedRulesAmazonIpReputationList and AWSManagedRulesAnonymousIpList, are the unversioned ones and correctly report false. The model's @default false is the wire default for an absent member, not a licence to report every group as unversioned.

Emulator: every AWS-vendor summary comes back with the defaulted value. The entry for the core rule set is returned as {'VendorName': 'AWS', 'Name': 'AWSManagedRulesCommonRuleSet', 'VersioningSupported': False, 'Description': 'Provides protection against exploitation of a wide range of vulnerabilities, including high risk and commonly occurring vulnerabilities described in OWASP publications.'} — the Description is real catalog data, so the catalog is populated but the versioning flag is not, i.e. the flag is left at the Smithy default rather than carried per group. The check fails on that summary before it reaches its later DescribeManagedRuleGroup assertions (core-rule-set Capacity, >=10 member Rules each with an Action, AvailableLabels under awswaf:managed:aws:core-rule-set:, LabelNamespace absent for an AWS-vendor group, ConsumedLabels absent on the core rule set and non-empty on AWSManagedRulesBotControlRuleSet, and the us-east-1 aws-managed-waf-rule-notifications SNS topic ARN), so those remain unverified here and may hide further catalog gaps once this flag is fixed.

Why: a client that keys version-pinning off the catalog gets the wrong answer. Code that reads VersioningSupported to decide whether to call ListAvailableManagedRuleGroupVersions and pin a Version in a ManagedRuleGroupStatement will conclude no AWS managed rule group can be pinned, skip pinning entirely, and silently follow AWS's rolling default version in production — the opposite of the pinned behavior it was written to get. Version-discovery code paths are never exercised against the emulator at all.

Fix: the managed-rule-group catalog reference data in the wafv2 crate (services/service-com-amazonaws-wafv2-awswaf-20190729) must carry VersioningSupported per group rather than leaving the modeled default, and the ListAvailableManagedRuleGroups handler must serialize that stored value: true for the versioned AWS rule-set groups, false only for the unversioned IP-list groups. This is knowledge the emulator has to hold, not compute.

Traces: wafv2.managed-rule-group-describe-catalog-contents (verdict mismatch).

=== Cause 2 of 2: PutManagedRuleSetVersions misclassifies a nonexistent ManagedRuleSet as an invalid parameter ===
Failing check: wafv2.put-managed-rule-set-versions-invalid-version-format-rejected

AWS: a PutManagedRuleSetVersions whose Name/Id/LockToken are all well-formed but name a ManagedRuleSet that does not exist in the account is a target-resolution failure, not a parameter failure: HTTP 400 with Error.Code 'WAFNonexistentItemException'. Smithy: PutManagedRuleSetVersions input Name -> EntityName @length 1..128 @pattern ^[\w\-]+$, Id -> EntityId @pattern ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$, LockToken -> LockToken same UUID pattern, VersionsToPublish -> map of VersionKeyString; modeled errors include both WAFNonexistentItemException (client, httpError 400) and WAFInvalidParameterException (client, 400). WAFInvalidParameterException is documented for "WAF didn't recognize a parameter in the request... a parameter name or value that isn't valid" / a malformed ARN — it is the wrong class once every parameter satisfies its modeled constraints. The message AWS returns for the nonexistent class is the fixed sentence "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist." (note the U+2019 right single quotation marks in "couldn\u2019t" and "doesn\u2019t"); it has no variable parts — it does not name the Name, Id or LockToken from the request.

Emulator: the same call answers Error.Code 'WAFInvalidParameterException'. AssertionError: got 'WAFInvalidParameterException' instead (check_wafv2_put_managed_rule_set_versions_invalid_version_format_rejected.py line 56). The check's healthy-account control in the same file, ListAvailableManagedRuleGroups(Scope='REGIONAL'), succeeded first, so this is operation-specific and not an account-wide failure; the request carried a valid UUID Id, a valid UUID LockToken and a VersionsToPublish entry '1.0' with a syntactically valid AssociatedRuleGroupArn, so no parameter in it is invalid. Because the code diverges, the exact message is not reached by the assertion and may diverge too.

Why: a client cannot distinguish "this managed rule set does not exist (create or publish it first)" from "my request is malformed (fix the request)". Both are 400, so retry/branching logic that switches on Error.Code takes the wrong arm: a vendor-side publishing workflow that creates the ManagedRuleSet on WAFNonexistentItemException and hard-fails on WAFInvalidParameterException will hard-fail here on a condition AWS tells it to recover from.

Fix: the PutManagedRuleSetVersions handler in the wafv2 crate (services/service-com-amazonaws-wafv2-awswaf-20190729) must order its refusals target-resolution-first: after the modeled input constraints are satisfied, resolve Name+Scope+Id against stored managed-rule-set state and, when nothing resolves, raise 400 WAFNonexistentItemException with the fixed sentence above, reserving WAFInvalidParameterException for inputs that actually violate a parameter rule. The state must also stay untouched on the refusal (ListManagedRuleSets must not show the refused Name or Id).

Traces: wafv2.put-managed-rule-set-versions-invalid-version-format-rejected (verdict mismatch).

## Evidence
Cause 1 of 2 — ListAvailableManagedRuleGroups VersioningSupported
Expected (AWS): the ManagedRuleGroupSummary for VendorName='AWS', Name='AWSManagedRulesCommonRuleSet' carries VersioningSupported=true (the group is versioned); only AWSManagedRulesAmazonIpReputationList and AWSManagedRulesAnonymousIpList carry false.
Actual (emulator): the summary is returned as
  {'VendorName': 'AWS', 'Name': 'AWSManagedRulesCommonRuleSet', 'VersioningSupported': False, 'Description': 'Provides protection against exploitation of a wide range of vulnerabilities, including high risk and commonly occurring vulnerabilities described in OWASP publications.'}
Failure:
  File "check_wafv2_managed_rule_group_describe_catalog_contents.py", line 145, in run
  assert aws_groups["AWSManagedRulesCommonRuleSet"].get("VersioningSupported") is True
  AssertionError: {'VendorName': 'AWS', 'Name': 'AWSManagedRulesCommonRuleSet', 'VersioningSupported': False, ...}
Verdict: mismatch. The catalog's 14 AWS-vendor group names and Descriptions were present (the preceding membership and Description assertions passed), and the two unversioned IP-list groups correctly reported false — so the defect is that the flag is uniformly the modeled default rather than per-group data. The later DescribeManagedRuleGroup payload assertions in the same check (Capacity, Rules/Action, AvailableLabels namespace, LabelNamespace absent, ConsumedLabels, SNS topic ARN) were not reached.

Cause 2 of 2 — PutManagedRuleSetVersions on a nonexistent ManagedRuleSet
Expected (AWS): HTTP 400, Error.Code 'WAFNonexistentItemException', Error.Message exactly the fixed sentence "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist." (all fixed text; no request- or state-derived parts — the Name/Id/LockToken are not echoed). ListManagedRuleSets afterwards contains neither the requested Name nor Id.
Actual (emulator): Error.Code 'WAFInvalidParameterException'.
Failure:
  File "check_wafv2_put_managed_rule_set_versions_invalid_version_format_rejected.py", line 56, in run
  assert code == "WAFNonexistentItemException", f"got {code!r} instead"
  AssertionError: got 'WAFInvalidParameterException' instead
Verdict: mismatch. Control in the same check: ListAvailableManagedRuleGroups(Scope='REGIONAL') returned a non-empty catalog immediately before, so the account is healthy and the misclassification is local to PutManagedRuleSetVersions. Every input in the refused request satisfies its modeled constraint (EntityName pattern ^[\w\-]+$, EntityId/LockToken UUID pattern, a well-formed regional rulegroup ARN in VersionsToPublish['1.0'].AssociatedRuleGroupArn), so no parameter is in fact invalid. The emulator's message text for this refusal is untested because the code assertion fails first.

## Steps to Reproduce
Cause 1 of 2:
1. Boot the emulator and point a wafv2 client at it.
2. Call ListAvailableManagedRuleGroups(Scope='REGIONAL', Limit=100), paginating on NextMarker, and keep the entries with VendorName=='AWS'.
3. Inspect the entry named 'AWSManagedRulesCommonRuleSet': observe VersioningSupported is False where AWS reports True. Observe the same uniform False on the other versioned AWS rule-set groups (AWSManagedRulesSQLiRuleSet, AWSManagedRulesKnownBadInputsRuleSet, AWSManagedRulesLinuxRuleSet, AWSManagedRulesUnixRuleSet, AWSManagedRulesWindowsRuleSet, AWSManagedRulesPHPRuleSet, AWSManagedRulesWordPressRuleSet, AWSManagedRulesAdminProtectionRuleSet, AWSManagedRulesBotControlRuleSet, AWSManagedRulesATPRuleSet, AWSManagedRulesACFPRuleSet), while AWSManagedRulesAmazonIpReputationList and AWSManagedRulesAnonymousIpList are correctly False.
4. Equivalently, replay check wafv2.managed-rule-group-describe-catalog-contents.

Cause 2 of 2:
1. Boot the emulator and point wafv2 and sts clients at it.
2. Confirm the control: ListAvailableManagedRuleGroups(Scope='REGIONAL') returns a non-empty ManagedRuleGroups list.
3. Call PutManagedRuleSetVersions with Scope='REGIONAL', a Name matching ^[\w\-]+$ that no ManagedRuleSet in the account uses, Id = a fresh random UUID, LockToken = a fresh random UUID, and VersionsToPublish={'1.0': {'AssociatedRuleGroupArn': 'arn:aws:wafv2:<region>:<account>:regional/rulegroup/sim2real-nonexistent/00000000-0000-0000-0000-000000000000'}}.
4. Observe Error.Code is 'WAFInvalidParameterException' instead of 'WAFNonexistentItemException' (HTTP 400 in both cases), and check the Message against the fixed sentence "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist."
5. Call ListManagedRuleSets(Scope='REGIONAL') and confirm nothing was created under the refused Name/Id.
6. Equivalently, replay check wafv2.put-managed-rule-set-versions-invalid-version-format-rejected.
