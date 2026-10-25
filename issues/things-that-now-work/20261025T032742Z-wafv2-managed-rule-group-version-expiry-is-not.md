Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.create-webacl-managed-rule-group-pinned-version-persisted
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.create-webacl-managed-rule-group-pinned-version-persisted|run#15|AssertionError|raise AssertionError(
Issue-Commit: 2c5164fde9f80995fd19c7806eff84f0d6083107

# wafv2: managed rule group version expiry is not modeled — no VersionName ever resolves to WAFExpiredManagedRuleGroupVersionException, so the expired-pin rejection path is unreachable

> _Note: this issue was identified against the code base at commit `2c5164fde9f80995fd19c7806eff84f0d6083107`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedRuleGroup

## Description
Single root cause (Cause 1 of 1): the emulator has no notion of an *expired* version of a vendor managed rule group, so neither DescribeManagedRuleGroup nor CreateWebACL can ever produce WAFExpiredManagedRuleGroupVersionException, and the whole expired-pin negative class is unreachable.

AWS: WAFv2 distinguishes three states for a VersionName of an AWS-vendored managed rule group: currently offered (DescribeManagedRuleGroup returns 200 with Capacity/Rules/LabelNamespace), existed-but-expired (modeled error WAFExpiredManagedRuleGroupVersionException, HTTP 400), and never existed (WAFNonexistentItemException, HTTP 400). Smithy (com-amazonaws-wafv2-awswaf-20190729): DescribeManagedRuleGroup input VersionName -> VersionKeyString @length 1..64 @pattern ^[\w#:\.\-/]+$; errors include WAFExpiredManagedRuleGroupVersionException (client, httpError 400) and WAFNonexistentItemException (client, 400). CreateWebACL carries the same WAFExpiredManagedRuleGroupVersionException (client, 400) in its error list, i.e. pinning an expired Version in a Rule's ManagedRuleGroupStatement is a modeled 400 refusal and must not create the web ACL. AWS also prunes fully-expired versions out of ListAvailableManagedRuleGroupVersions while DescribeManagedRuleGroup still reports them as expired — that asymmetry is the behavior under test.

Emulator: the check probes eleven historical VendorName/Name/VersionName triples on AWS-owned rule groups (AWSManagedRulesCommonRuleSet Version_1.0/1.1/1.2, AWSManagedRulesSQLiRuleSet Version_1.0/1.1, KnownBadInputs/Linux/Windows/Unix/AnonymousIpList/AmazonIpReputationList Version_1.0) via DescribeManagedRuleGroup and not one of them is classified as expired; every probe returns something other than WAFExpiredManagedRuleGroupVersionException (DescribeManagedRuleGroup is still an unimplemented scaffold in this build — see open issue 20261025T023104Z-wafv2-describemanagedrulegroup-stub.md, which covers only the stub itself and not this version-state semantics). The check therefore fails, and the subsequent CreateWebACL-with-expired-pin refusal is never exercised at all. The earlier half of the same check (CreateWebACL pinning a currently-valid Version, GetWebACL echoing Rules[0].Statement.ManagedRuleGroupStatement.Version) did reach the emulator and did not diverge.

Why: a client that pins managed rule group versions gets no refusal when it pins a version AWS has retired. Against AWS such a CreateWebACL/UpdateWebACL fails with HTTP 400 WAFExpiredManagedRuleGroupVersionException and no web ACL is created; against the emulator the call is accepted (or the version state is simply unknowable), so version-pinning lifecycle code — the retire-and-roll-forward path and its error handling — cannot be tested and passes here while breaking in production. The companion read path is equally wrong: a caller cannot tell 'this version expired' from 'this version never existed'.

Fix: the managed-rule-group catalog state for wafv2 (services/service-com-amazonaws-wafv2-awswaf-20190729) needs per-version state — offered vs expired vs unknown — for the AWS vendor rule groups it already serves, plus two consumers of it: the DescribeManagedRuleGroup handler (return 400 WAFExpiredManagedRuleGroupVersionException for a known-but-expired VersionName, 400 WAFNonexistentItemException for an unknown one, and omit expired versions from ListAvailableManagedRuleGroupVersions), and the CreateWebACL/UpdateWebACL rule validation (reject a ManagedRuleGroupStatement whose Version is expired with 400 WAFExpiredManagedRuleGroupVersionException and persist nothing, so ListWebACLs does not show the rejected name).

Traces: wafv2.create-webacl-managed-rule-group-pinned-version-persisted (verdict mismatch).

Note for the fixer: which concrete historical VersionName is expired on real AWS rotates over time, so the fix must make *some* retired version of the AWS vendor groups report as expired rather than match one recorded VersionName; the check accepts any triple from its candidate list.

## Evidence
Expected (AWS): for at least one retired VersionName of an AWS-vendored managed rule group, DescribeManagedRuleGroup(VendorName='AWS', Name=<group>, Scope='REGIONAL', VersionName=<retired version>) answers HTTP 400 with Error.Code 'WAFExpiredManagedRuleGroupVersionException'; and CreateWebACL with a Rule whose ManagedRuleGroupStatement pins that version answers HTTP 400 'WAFExpiredManagedRuleGroupVersionException' and creates nothing (the name is absent from ListWebACLs). Example recorded message text, not a requirement: "You're trying to use an expired version (<VersionName>) of <RuleGroupName> by <VendorName>."

Actual (emulator): all eleven DescribeManagedRuleGroup probes return a non-expired outcome, so no expired version exists to drive the negative, and the check aborts with:
  AssertionError: no confirmed-expired managed rule group version could be discovered among the sampled historical VersionNames (AWS prunes expired versions from the offered catalog, and which old VersionName still resolves to WAFExpiredManagedRuleGroupVersionException rotates over time); rerun later or widen _CANDIDATE_VERSIONS -- the pinned-version CreateWebACL above still ran and was asserted for real state
  (check_wafv2_create_webacl_managed_rule_group_pinned_version_persisted.py line 85)
Verdict: mismatch. Consequence: CreateWebACL's expired-pin 400 refusal is never reached, and DescribeManagedRuleGroup cannot distinguish expired from nonexistent versions.

## Steps to Reproduce
1. Boot the emulator and point a wafv2 client at it (Scope='REGIONAL').
2. Call ListAvailableManagedRuleGroupVersions(VendorName='AWS', Name='AWSManagedRulesCommonRuleSet', Scope='REGIONAL'), take CurrentDefaultVersion, and CreateWebACL with one Rule whose Statement.ManagedRuleGroupStatement pins that Version; GetWebACL and confirm the Version round-trips (this part currently holds).
3. For each of VendorName='AWS' with (AWSManagedRulesCommonRuleSet, Version_1.0/Version_1.1/Version_1.2), (AWSManagedRulesSQLiRuleSet, Version_1.0/Version_1.1), (AWSManagedRulesKnownBadInputsRuleSet|AWSManagedRulesLinuxRuleSet|AWSManagedRulesWindowsRuleSet|AWSManagedRulesUnixRuleSet|AWSManagedRulesAnonymousIpList|AWSManagedRulesAmazonIpReputationList, Version_1.0), call DescribeManagedRuleGroup(Scope='REGIONAL', VersionName=<version>). Observe that none answers 400 WAFExpiredManagedRuleGroupVersionException.
4. Take any such retired version and call CreateWebACL with a Rule pinning it: observe it is not refused with 400 WAFExpiredManagedRuleGroupVersionException.
5. Equivalently, replay check wafv2.create-webacl-managed-rule-group-pinned-version-persisted in the behavioral suite.
