Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.ordering-upgrade-history-chronological-sequence
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.ordering-upgrade-history-chronological-sequence|run#19|AssertionError|assert history_after_2 == history_after_1, (
Issue-Commit: b24141e82a457f063895c58d5c810db49b8f6663

# OpenSearch: a repeat UpgradeDomain eligibility check appends a second GetUpgradeHistory entry instead of collapsing into the existing one

> _Note: this issue was identified against the code base at commit `b24141e82a457f063895c58d5c810db49b8f6663`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpgradeDomain

## Description
One net-new root cause in the opensearch control plane.

Cause 1 of 1: UpgradeDomain with PerformCheckOnly=true records a NEW upgrade-history run on every call, so a second eligibility check against a different target version grows GetUpgradeHistory by one entry. Real AWS collapses a repeat eligibility check into the single eligibility-check record the domain already holds, leaving GetUpgradeHistory byte-identical to what it reported after the first check. Failing check id: opensearch.ordering-upgrade-history-chronological-sequence.

AWS: recorded behavior only for the collapsing rule — the Smithy model documents GetUpgradeHistory as "Retrieves the complete history of the last 10 upgrades performed on an Amazon OpenSearch Service domain" and documents UpgradeDomain$PerformCheckOnly as "When true, indicates that an upgrade eligibility check needs to be performed. Does not actually perform the upgrade"; it states no per-call append rule. The recording shows that after a first eligibility check from the domain's engine version to target_1 the history holds exactly ONE entry (UpgradeName "Upgrade eligibility check from <source-version> to <target-version>", UpgradeStatus SUCCEEDED, StepsList with a single PRE_UPGRADE_CHECK/SUCCEEDED/ProgressPercent 100.0 step), and that a second eligibility check against a DIFFERENT compatible target (one offered by GetCompatibleVersions for the same domain) leaves that history unchanged — same single entry, same UpgradeName naming the FIRST target, same StartTimestamp. All of the variable parts (domain name, source and target versions, timestamps, step list) come from the request and the domain's state; only the collapsing invariant (history unchanged after the repeat check) is the requirement.

Emulator: the second UpgradeDomain(PerformCheckOnly=true) mints a second UpgradeRun, and GetUpgradeHistory then reports TWO entries — a new "Upgrade eligibility check from OpenSearch_1.3 to <target_2>" entry with a fresh StartTimestamp ahead of the retained "...to <target_1>" entry (per the replay: before=[one entry for target_1], after=[entry for target_2, entry for target_1]). Both calls return 200; the divergence is purely in the recorded history.

Why: a client that polls GetUpgradeHistory to decide whether an eligibility check has already been performed, or that counts history entries to detect a newly started run (the standard "one new entry per upgrade" idiom), sees a phantom second check against AWS-identical usage. Re-running an eligibility check — which AWS treats as idempotent for history purposes — makes the emulator's history diverge permanently, and anything keyed on the newest entry's UpgradeName/TargetVersion reads the latest probe rather than the recorded check.

Fix: the domain state's upgrade-run recording — the eligibility-check branch of the domain record's begin_upgrade must reuse the existing eligibility-check run rather than pushing a new one, so GetUpgradeHistory/GetUpgradeStatus report the collapsed record. No change is needed in GetUpgradeHistory's paging, which reports whatever runs the domain holds.

Traces: opensearch.ordering-upgrade-history-chronological-sequence. Relevant files: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs (Domain::begin_upgrade, around lines 2225-2266, which unconditionally does self.upgrades.push_back(run)), services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/upgrade_domain.rs (handle, around lines 243-248).

## Evidence
Check opensearch.ordering-upgrade-history-chronological-sequence (mismatch):

AssertionError: a second eligibility check against a different target version changed GetUpgradeHistory, contradicting the collapsing behavior observed on real AWS: target_1='OpenSearch_2.3' target_2='OpenSearch_2.5';
before=[{'UpgradeName': 'Upgrade eligibility check from OpenSearch_1.3 to OpenSearch_2.3', 'StartTimestamp': <t1>, 'UpgradeStatus': 'SUCCEEDED', 'StepsList': [{'UpgradeStep': 'PRE_UPGRADE_CHECK', 'UpgradeStepStatus': 'SUCCEEDED', 'Issues': [], 'ProgressPercent': 100.0}]}]
after=[{'UpgradeName': 'Upgrade eligibility check from OpenSearch_1.3 to OpenSearch_2.5', 'StartTimestamp': <t2>, 'UpgradeStatus': 'SUCCEEDED', 'StepsList': [...]}, {'UpgradeName': 'Upgrade eligibility check from OpenSearch_1.3 to OpenSearch_2.3', 'StartTimestamp': <t1>, 'UpgradeStatus': 'SUCCEEDED', 'StepsList': [...]}]

Expected (AWS): GetUpgradeHistory after the second eligibility check == GetUpgradeHistory after the first (one entry, naming the first target, with its original StartTimestamp).
Actual (emulator): two entries; the newest names the second target.
(Version strings, domain name and timestamps above are this run's values filling the template; the requirement is the unchanged-history invariant.)

## Steps to Reproduce
1. CreateDomain with EngineVersion=OpenSearch_1.3, ClusterConfig {InstanceType t3.small.search, InstanceCount 1}, EBSOptions {EBSEnabled true, VolumeType gp3, VolumeSize 10}; wait until DescribeDomain reports Created=true and Processing=false.
2. GetCompatibleVersions(DomainName=<domain>) and take two distinct TargetVersions, target_1 and target_2.
3. UpgradeDomain(DomainName=<domain>, TargetVersion=target_1, PerformCheckOnly=true); poll GetUpgradeStatus until StepStatus is terminal (SUCCEEDED).
4. GetUpgradeHistory(DomainName=<domain>) — record the result (one eligibility-check entry).
5. UpgradeDomain(DomainName=<domain>, TargetVersion=target_2, PerformCheckOnly=true); poll GetUpgradeStatus until terminal.
6. GetUpgradeHistory(DomainName=<domain>) again. AWS returns the identical single entry from step 4; the emulator returns two entries, the newest naming target_2.
