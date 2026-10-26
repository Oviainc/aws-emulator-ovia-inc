Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.upgrade-domain-server-managed-progress-tracking
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.upgrade-domain-server-managed-progress-tracking|_wait_domain_active#8|Timeout|time.sleep(interval_s)
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# opensearch UpgradeDomain and GetUpgradeStatus answer 501 NotImplemented, so a service-managed engine upgrade can never be started or observed

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpgradeDomain

## Description
One root cause in the opensearch service.

Cause 1 of 1: the engine-upgrade surface is unimplemented — UpgradeDomain and GetUpgradeStatus both answer HTTP 501 with error code NotImplemented, so no upgrade can be started and no upgrade progress can be read.

AWS: UpgradeDomain(DomainName, TargetVersion) starts a service-managed engine upgrade and answers HTTP 200 with UpgradeId/DomainName/TargetVersion/ChangeProgressDetails; the upgrade then advances on the service's own clock, with no further client-driven step call. GetUpgradeStatus(DomainName) answers HTTP 200 carrying UpgradeStep (PRE_UPGRADE_CHECK | SNAPSHOT | UPGRADE), StepStatus (IN_PROGRESS | SUCCEEDED | SUCCEEDED_WITH_ISSUES | FAILED) and UpgradeName for the most recent upgrade or eligibility check. Smithy: opensearch#UpgradeDomain POST /2021-01-01/opensearch/upgradeDomain, success 200, modeled errors BaseException(400), DisabledOperationException(409), InternalException(500), ResourceAlreadyExistsException(409), ResourceNotFoundException(409), ValidationException(400); opensearch#GetUpgradeStatus GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/status, success 200, same error set minus ResourceAlreadyExistsException. NotImplemented is in neither error set, and 501 is no part of either contract.

Emulator: both operations are untouched scaffold stubs that return Scaffolded::not_implemented(..) — 'operation UpgradeDomain is scaffolded but not implemented in this emulator' (HTTP 501, code NotImplemented), likewise for GetUpgradeStatus. Consistent with that, DomainStatus hard-codes upgrade_processing: Some(false) with the in-tree comment 'No version upgrade is in flight: UpgradeDomain is still a stub', so even the domain read can never report an upgrade. In the replay this check reaches the harness ceiling instead of recording the 501 — 'Error: Check timed out after 60.0s'. Hypothesis for the timeout mechanism (neither the model nor the recording states it): the check's _wait_domain_active polls DescribeDomain at a 30 s interval and requires two consecutive Processing=False reads, while the emulator deliberately holds a just-created domain Processing for PROVISIONING = 500 ms (services/.../src/state.rs), so the first poll still sees Processing=true and the third poll falls past the 60 s deadline — the run never gets as far as the UpgradeDomain call whose 501 is the defect. No open issue covers either operation (the filed stub issues cover GetUpgradeHistory, ListVersions, CreateVpcEndpoint, AddTags and others, not these two).

Why: a client cannot upgrade an emulated domain at all, and cannot distinguish 'this emulator has no handler' from a real refusal such as DisabledOperationException or ValidationException on an incompatible TargetVersion. Any upgrade workflow — start an upgrade, poll GetUpgradeStatus until a terminal StepStatus, cross-check GetUpgradeHistory — dies on the first call, and version-migration automation written against the emulator is never exercised.

Fix: implement the UpgradeDomain handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/upgrade_domain.rs) and the GetUpgradeStatus handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_upgrade_status.rs) over the domain record in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs, including the upgrade_processing flag the DomainStatus projection currently pins to false, and delete their SCAFFOLDED markers.

Traces: opensearch.upgrade-domain-server-managed-progress-tracking.

## Evidence
Replay: [mismatch] opensearch.upgrade-domain-server-managed-progress-tracking — 'Error: Check timed out after 60.0s'. The check's contract is: get_compatible_versions -> create domain on the source version -> upgrade_domain(TargetVersion=...) -> poll get_upgrade_status until StepStatus is present and terminal -> get_upgrade_history.

Checkout at the replayed commit: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/upgrade_domain.rs line 186 returns Err(crate::scaffolded::Scaffolded::not_implemented("UpgradeDomain")) under the '//! SCAFFOLDED, NOT IMPLEMENTED' marker; src/ops/get_upgrade_status.rs line 167 returns Err(crate::scaffolded::Scaffolded::not_implemented("GetUpgradeStatus")). Expected per model: HTTP 200 UpgradeDomainResponse / GetUpgradeStatusResponse; actual: HTTP 501, Error.Code 'NotImplemented'. src/state.rs sets upgrade_processing: Some(false) unconditionally with the comment 'No version upgrade is in flight: UpgradeDomain is still a stub'.

## Steps to Reproduce
1. opensearch.get_compatible_versions() and pick a pair whose SourceVersion and one TargetVersion both start with 'OpenSearch_'.
2. opensearch.create_domain(DomainName=<d>, EngineVersion=<source>, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, AccessPolicies=<account-root policy>) and wait until describe_domain reports Processing=False.
3. opensearch.upgrade_domain(DomainName=<d>, TargetVersion=<target>) -> HTTP 501 NotImplemented (expected HTTP 200 with UpgradeId).
4. opensearch.get_upgrade_status(DomainName=<d>) -> HTTP 501 NotImplemented (expected HTTP 200 with StepStatus and UpgradeName).

Replay the check id listed in trace_ids (note: the check polls DescribeDomain on a 30 s interval, so re-replaying it needs a per-check timeout above the default 60 s to reach the UpgradeDomain call).
