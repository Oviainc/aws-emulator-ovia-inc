Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-domain-lifecycle-and-config-inspection
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-domain-lifecycle-and-config-inspection opensearch-domain-version-capacity-and-software-update-management opensearch-domain-lifecycle-workflow
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeDomainNodes|4xx->2xx|$.message
Issue-Key: es|DescribeDomainNodes|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeDomainNodes|4xx->2xx|status
Issue-Key: es|UpdateDomainConfig|4xx->2xx|$.message
Issue-Key: es|UpdateDomainConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|UpdateDomainConfig|4xx->2xx|status
Issue-Key: es|CancelDomainConfigChange|4xx->2xx|$.message
Issue-Key: es|CancelDomainConfigChange|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|CancelDomainConfigChange|4xx->2xx|status
Issue-Key: es|UpgradeDomain|4xx->2xx|$.message
Issue-Key: es|UpgradeDomain|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|UpgradeDomain|4xx->2xx|status
Issue-Key: es|GetUpgradeStatus|2xx->2xx|$.StepStatus
Issue-Key: es|GetUpgradeStatus|2xx->2xx|$.UpgradeName
Issue-Key: es|GetUpgradeStatus|2xx->2xx|$.UpgradeStep
Issue-Key: es|GetUpgradeHistory|2xx->2xx|$.UpgradeHistories[*]
Issue-Key: es|StartServiceSoftwareUpdate|4xx->4xx|$.message
Issue-Key: es|CancelServiceSoftwareUpdate|4xx->4xx|$.message
Issue-Key: es|RollbackServiceSoftwareUpdate|4xx->2xx|$.message
Issue-Key: es|RollbackServiceSoftwareUpdate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|RollbackServiceSoftwareUpdate|4xx->2xx|status
Issue-Key: es|ListDomainMaintenances|4xx->2xx|$.message
Issue-Key: es|ListDomainMaintenances|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|ListDomainMaintenances|4xx->2xx|status
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: domain config-change and service-software lifecycle gates never fire, so ten refusals are missing

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpgradeDomain

## Description
10 net-new root causes: the emulator has no notion of "this domain currently has work in flight", so the whole family of in-progress refusals is absent and two upgrade readers invent progress that never happened.

Cause 1 of 10 - DescribeDomainNodes while the domain is still settling. AWS: 424 DependencyFailureException with the fixed message "The node information is not available now, please try again later." (Smithy: DescribeDomainNodes errors include DependencyFailureException @httpError 424). Emulator: 200 with a node list. Trace: opensearch-domain-lifecycle-and-config-inspection#task#8.

Cause 2 of 10 - UpdateDomainConfig with a verbose dry run during ongoing activity. AWS: 400 BaseException "Verbose Dry Run cannot be performed at this time due to ongoing activities on the domain". Emulator: 200. Trace: opensearch-domain-lifecycle-and-config-inspection#task#10.

Cause 3 of 10 - CancelDomainConfigChange outside the states it is allowed in. AWS: 400 ValidationException "CancelDomainConfigChange only when domain state is in Modifying or UpdatingServiceSoftware.". Emulator: 200. Trace: opensearch-domain-lifecycle-and-config-inspection#task#13.

Cause 4 of 10 - UpgradeDomain with background tasks in progress. AWS: 400 BaseException "Upgrade failed. Domain has background tasks in progress". Emulator: 200, and it starts an upgrade. Trace: opensearch-domain-version-capacity-and-software-update-management#task#6.

Cause 5 of 10 - GetUpgradeStatus reports an upgrade that AWS never started. AWS returns the operation's output with StepStatus, UpgradeStep and UpgradeName absent when no upgrade is in flight; the emulator reports StepStatus 'IN_PROGRESS', UpgradeStep 'PRE_UPGRADE_CHECK' and a synthesized UpgradeName. Trace: opensearch-domain-version-capacity-and-software-update-management#task#7.

Cause 6 of 10 - GetUpgradeHistory returns a fabricated history entry for the same non-upgrade, with a synthesized UpgradeName, StartTimestamp, UpgradeStatus IN_PROGRESS and a PRE_UPGRADE_CHECK step at 3% progress. AWS returns no history entry. Trace: opensearch-domain-version-capacity-and-software-update-management#task#8.

Cause 7 of 10 - StartServiceSoftwareUpdate refusal text. AWS: the fixed message "The domain is not available for update." (no variable part). Emulator: its own prose "There is no software update available for domain <domainName>.", interpolating the domain name. Status 400 already matches. Trace: opensearch-domain-version-capacity-and-software-update-management#task#9.

Cause 8 of 10 - CancelServiceSoftwareUpdate refusal text. AWS: "The domain update cannot be cancelled. There is no pending update." (fixed). Emulator: "There is no scheduled service software update to cancel on domain <domainName>.". Trace: opensearch-domain-version-capacity-and-software-update-management#task#10.

Cause 9 of 10 - RollbackServiceSoftwareUpdate during a change. AWS: 400 ValidationException "A change/update is in progress. Please wait for it to complete before requesting another change.". Emulator: 200. Trace: opensearch-domain-version-capacity-and-software-update-management#task#11.

Cause 10 of 10 - ListDomainMaintenances with a node id that does not exist. AWS: 404 ResourceNotFoundException "The requested node ID is invalid" (fixed text). Emulator: 200 with an empty list. Trace: opensearch-domain-lifecycle-workflow#task#2.

Why: a client that drives a domain through a config change or a software update relies on these refusals to serialize its own work - it issues the change, gets told "in progress", and backs off. Against the emulator every call succeeds, so concurrent changes are accepted and the client never learns it must wait; conversely causes 5 and 6 make an idle domain look like it is mid-upgrade, so a client that waits for the upgrade to finish waits forever.

Fix: the domain's config-change/software-update state machine (the per-domain in-flight state that these handlers must consult) plus the refusal text in StartServiceSoftwareUpdate and CancelServiceSoftwareUpdate, and the upgrade-status/history readers which must report nothing when no upgrade exists.

## Evidence
Cause 1: expected 424 + x-amzn-errortype DependencyFailureException + 'The node information is not available now, please try again later.'; actual 200.
Cause 2: expected 400 + BaseException + 'Verbose Dry Run cannot be performed at this time due to ongoing activities on the domain'; actual 200.
Cause 3: expected 400 + ValidationException + 'CancelDomainConfigChange only when domain state is in Modifying or UpdatingServiceSoftware.'; actual 200.
Cause 4: expected 400 + BaseException + 'Upgrade failed. Domain has background tasks in progress'; actual 200.
Cause 5: "$.StepStatus: type_mismatch (expected None, got 'IN_PROGRESS')", "$.UpgradeStep: type_mismatch (expected None, got 'PRE_UPGRADE_CHECK')", "$.UpgradeName: type_mismatch (expected None, got 'Upgrade eligibility check from ... to ...')".
Cause 6: "$.UpgradeHistories[0]: extra (expected None, got {...UpgradeStatus 'IN_PROGRESS'...})".
Cause 7: expected 'The domain is not available for update.'; actual 'There is no software update available for domain <domainName>.'
Cause 8: expected 'The domain update cannot be cancelled. There is no pending update.'; actual 'There is no scheduled service software update to cancel on domain <domainName>.'
Cause 9: expected 400 + ValidationException + 'A change/update is in progress. Please wait for it to complete before requesting another change.'; actual 200.
Cause 10: expected 404 + ResourceNotFoundException + 'The requested node ID is invalid'; actual 200.

## Steps to Reproduce
1. Replay opensearch-domain-lifecycle-and-config-inspection and inspect steps task#8 (DescribeDomainNodes), task#10 (UpdateDomainConfig verbose dry run) and task#13 (CancelDomainConfigChange): each answers 200 where the recording is a refusal.
2. Replay opensearch-domain-version-capacity-and-software-update-management steps task#6 through task#11 for the upgrade/software-update gates and the two fabricated upgrade readers.
3. Replay opensearch-domain-lifecycle-workflow step task#2 for the ListDomainMaintenances node-id refusal.
