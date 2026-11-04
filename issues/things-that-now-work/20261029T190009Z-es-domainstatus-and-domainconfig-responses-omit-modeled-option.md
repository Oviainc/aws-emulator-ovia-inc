Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-create-domain-gp3-server-defaults
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-create-domain-gp3-server-defaults opensearch-domain-lifecycle-and-config-inspection esdef-create-domain-gp2-server-defaults esval-ebs-gp2-iops-not-accepted esval-ebs-gp3-iops-below-floor esval-ebs-gp3-throughput-below-floor esval-ebs-gp3-volume-size-above-instance-cap esval-range-snapshot-start-hour-25 esval-topology-standby-single-data-node esval-topology-three-az-two-data-nodes opensearch-domain-lifecycle-workflow opensearch-domain-tagging-roundtrip opensearch-domain-version-capacity-and-software-update-management opensearch-tag-domain-lifecycle
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AIMLOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AccessPolicies
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AdvancedOptions.override_main_response_version
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AdvancedOptions.rest.action.multi.allow_explicit_index
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AdvancedSecurityOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AutoTuneOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.AutomatedSnapshotPauseOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ClusterConfig.ColdStorageOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ClusterConfig.DedicatedMasterEnabled
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ClusterConfig.MultiAZWithStandbyEnabled
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ClusterConfig.WarmEnabled
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ClusterConfig.ZoneAwarenessEnabled
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.CognitoOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.DeploymentStrategyOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.DomainEndpointOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.EBSOptions.Iops
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.EBSOptions.Throughput
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.EncryptionAtRestOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.Endpoint
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.IPAddressType
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.IdentityCenterOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.NodeToNodeEncryptionOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.OffPeakWindowOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.AutomatedUpdateDate
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.CurrentVersion
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.NewVersion
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.SnapshotOptions
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.SoftwareUpdateOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AIMLOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AccessPolicies
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AdvancedOptions.override_main_response_version
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AdvancedOptions.rest.action.multi.allow_explicit_index
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AdvancedSecurityOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AutoTuneOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.AutomatedSnapshotPauseOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ClusterConfig.ColdStorageOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ClusterConfig.DedicatedMasterEnabled
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ClusterConfig.MultiAZWithStandbyEnabled
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ClusterConfig.WarmEnabled
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ClusterConfig.ZoneAwarenessEnabled
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.CognitoOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.DeploymentStrategyOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.DomainEndpointOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.EBSOptions.Iops
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.EBSOptions.Throughput
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.EncryptionAtRestOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.IPAddressType
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.IdentityCenterOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.NodeToNodeEncryptionOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.OffPeakWindowOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.AutomatedUpdateDate
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.CurrentVersion
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.NewVersion
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.SnapshotOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.SoftwareUpdateOptions
Issue-Key: es|DescribeDomains|2xx->2xx|$.DomainStatusList[*]
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AIMLOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AccessPolicies.Options
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AccessPolicies.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AdvancedOptions.Options.override_main_response_version
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AdvancedOptions.Options.rest.action.multi.allow_explicit_index
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AdvancedOptions.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AdvancedSecurityOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AutoTuneOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AutomatedSnapshotPauseOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ChangeProgressDetails.ConfigChangeStatus
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ClusterConfig.Options.ColdStorageOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ClusterConfig.Options.DedicatedMasterEnabled
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ClusterConfig.Options.MultiAZWithStandbyEnabled
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ClusterConfig.Options.WarmEnabled
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ClusterConfig.Options.ZoneAwarenessEnabled
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ClusterConfig.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.CognitoOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.DeploymentStrategyOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.DomainEndpointOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EBSOptions.Options.Iops
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EBSOptions.Options.Throughput
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EBSOptions.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EncryptionAtRestOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EngineMode.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EngineVersion.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.IPAddressType
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.IdentityCenterOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.LogPublishingOptions.Status.State
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.LogPublishingOptions.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.NodeToNodeEncryptionOptions.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.OffPeakWindowOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.SnapshotOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.SoftwareUpdateOptions
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.UseCase.Status.UpdateVersion
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.VPCOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AIMLOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AccessPolicies
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AdvancedOptions.override_main_response_version
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AdvancedOptions.rest.action.multi.allow_explicit_index
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AdvancedSecurityOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AutoTuneOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.AutomatedSnapshotPauseOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ClusterConfig.ColdStorageOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ClusterConfig.DedicatedMasterEnabled
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ClusterConfig.MultiAZWithStandbyEnabled
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ClusterConfig.WarmEnabled
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ClusterConfig.ZoneAwarenessEnabled
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.CognitoOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.DeploymentStrategyOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.DomainEndpointOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.EBSOptions.Iops
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.EBSOptions.Throughput
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.EncryptionAtRestOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.Endpoint
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.IPAddressType
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.IdentityCenterOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ModifyingProperties
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.OffPeakWindowOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.AutomatedUpdateDate
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.CurrentVersion
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.NewVersion
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.SnapshotOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.SoftwareUpdateOptions
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.Endpoint
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.ChangeProgressStages
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.ConfigChangeStatus
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.PendingProperties[*]
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.Status
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.TotalNumberOfStages
Issue-Key: es|CreateDomain|2xx->4xx|$.DomainStatus
Issue-Key: es|CreateDomain|2xx->4xx|status
Issue-Key: es|DeleteDomain|4xx->2xx|status
Issue-Key: es|DescribeDomainHealth|2xx->2xx|$.ActiveAvailabilityZoneCount
Issue-Key: es|DescribeDomainHealth|2xx->2xx|$.EnvironmentInformation
Issue-Key: es|DescribeDomainHealth|2xx->2xx|$.MasterNode
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.EngineVersion
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.NodeToNodeEncryptionOptions
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.EngineVersion
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: DomainStatus and DomainConfig responses omit modeled option blocks and misreport change state

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
10 net-new root causes in the domain state model and its serializer. All of these answer 200 (or an unexpected 409/200) with a body that is a subset of, or contradicts, what AWS returns.

Cause 1 of 10 - CreateDomain's DomainStatus omits every server-defaulted option block. AWS returns a fully populated DomainStatus for a minimal CreateDomain: AIMLOptions (NaturalLanguageQueryGenerationOptions CurrentState NOT_ENABLED / DesiredState DISABLED, S3VectorsEngine.Enabled false, ServerlessVectorAcceleration.Enabled false), AccessPolicies as the empty string when none was supplied, AdvancedOptions.override_main_response_version 'false', AdvancedSecurityOptions, AutoTuneOptions (State DISABLED, UseOffPeakWindow false), AutomatedSnapshotPauseOptions (Enabled false, State 'Disabled'), ClusterConfig.ColdStorageOptions/DedicatedMasterEnabled/MultiAZWithStandbyEnabled/WarmEnabled/ZoneAwarenessEnabled, CognitoOptions, DeploymentStrategyOptions (DeploymentStrategy 'CapacityOptimized'), DomainEndpointOptions (EnforceHTTPS false, TLSSecurityPolicy 'Policy-Min-TLS-1-2-2019-07'), EBSOptions.Iops and .Throughput for gp3, EncryptionAtRestOptions, IPAddressType, IdentityCenterOptions, ModifyingProperties, NodeToNodeEncryptionOptions, OffPeakWindowOptions, ServiceSoftwareOptions.CurrentVersion/NewVersion/AutomatedUpdateDate, SnapshotOptions and SoftwareUpdateOptions. The emulator returns null for all of them. It also sets ChangeProgressDetails.ConfigChangeStatus to 'ApplyingChanges' where AWS reports 'Pending' at create time, and populates Endpoint while the domain is still being created where AWS leaves it absent. Traces: esdef-create-domain-gp3-server-defaults#task#1 (representative), opensearch-domain-lifecycle-workflow#setup#1, opensearch-domain-tagging-roundtrip#setup#1, opensearch-domain-version-capacity-and-software-update-management#setup#1, opensearch-tag-domain-lifecycle#setup#1.

Cause 2 of 10 - DescribeDomain returns the same truncated DomainStatus, so the omission is a property of the stored domain, not of the create response. Trace: esdef-create-domain-gp3-server-defaults#task#2.

Cause 3 of 10 - DescribeDomains' DomainStatusList entries are truncated the same way. Trace: opensearch-domain-lifecycle-and-config-inspection#task#3.

Cause 4 of 10 - DescribeDomainConfig omits the corresponding {Options,Status} wrappers (AIMLOptions, AdvancedSecurityOptions, AutoTuneOptions, AutomatedSnapshotPauseOptions, CognitoOptions, DeploymentStrategyOptions, DomainEndpointOptions, EncryptionAtRestOptions, IPAddressType, IdentityCenterOptions, ModifyingProperties, OffPeakWindowOptions, SnapshotOptions, SoftwareUpdateOptions, VPCOptions) and reports Status.UpdateVersion 1 where AWS reports 2 after the create-time settle, and LogPublishingOptions.Status.State wrongly. Trace: opensearch-domain-lifecycle-and-config-inspection#task#4.

Cause 5 of 10 - DeleteDomain's returned DomainStatus is truncated the same way, and reports ChangeProgressDetails.ConfigChangeStatus 'Cancelled' where AWS reports 'ApplyingChanges'. Traces: opensearch-domain-lifecycle-and-config-inspection#teardown#17, opensearch-domain-lifecycle-workflow#teardown#4, opensearch-domain-tagging-roundtrip#teardown#10, opensearch-domain-version-capacity-and-software-update-management#teardown#13, opensearch-tag-domain-lifecycle#teardown#11.

Cause 6 of 10 - AccessPolicies is not stored verbatim. AWS echoes the exact policy string the client sent; the emulator re-serializes the JSON, inserting a space after every ':' and ',', so a client that compares the policy it sent with the policy it reads back always sees a difference. Trace: opensearch-domain-lifecycle-and-config-inspection#task#2.

Cause 7 of 10 - DescribeDomainChangeProgress reports the wrong pending-change detail. AWS's PendingProperties entries are 'Member : <json value>' strings naming the leaf properties being applied (e.g. 'AIMLOptions.S3VectorsEngine : {"Enabled":false}', 'EBSOptions : {"EBSEnabled":true,...}'); the emulator lists bare top-level option names ('AccessPolicies', 'AdvancedOptions', 'ClusterConfig', ...), omits ChangeProgressStages (AWS returns []) and reports ConfigChangeStatus 'ApplyingChanges' instead of 'Pending'. Trace: opensearch-domain-lifecycle-and-config-inspection#task#5.

Cause 8 of 10 - a domain name freed by DeleteDomain stays occupied, so a CreateDomain AWS answers 200 is refused 409. Trace: esdef-create-domain-gp2-server-defaults#task#2 (affected also esdef-create-domain-gp3-server-defaults#task#3/#task#4, opensearch-domain-lifecycle-and-config-inspection#task#14).

Cause 9 of 10 - deleting an already-deleted domain succeeds. AWS answers 404 ResourceNotFoundException for a DeleteDomain on a domain that is already gone; the emulator answers 200, i.e. the delete is idempotent and the domain is never actually removed from state. Trace: esval-ebs-gp2-iops-not-accepted#teardown#2 (affected also the gp3-iops, gp3-throughput, gp3-volume-size, snapshot-start-hour, standby and three-az teardowns).

Cause 10 of 10 - DescribeDomainHealth reports a dead cluster for a healthy domain. AWS reports ActiveAvailabilityZoneCount equal to the configured AZ count ('1'), MasterNode 'Available' and no EnvironmentInformation for a single-AZ domain; the emulator reports ActiveAvailabilityZoneCount 'NotAvailable', MasterNode 'UnAvailable' and emits an EnvironmentInformation block with every per-AZ counter set to 'NotAvailable'. Trace: opensearch-domain-lifecycle-and-config-inspection#task#7.

Why: a client that reads any of these option blocks - to decide whether encryption at rest is on, whether HTTPS is enforced, what TLS policy applies, what the EBS Iops/Throughput are, or whether a change is still Pending - gets null and must treat it as unknown or crash on a missing field. Infrastructure tooling that diffs desired against actual config sees every defaulted block as a drift and rewrites it forever; the AccessPolicies whitespace defect makes that diff permanent even for the one field that is present. Causes 8 and 9 make domain lifecycle unreproducible: teardown appears to succeed twice and the name can never be reused.

Fix: the domain state model and the DomainStatus / DomainConfig serializers (server-side defaulting of the option blocks, verbatim storage of AccessPolicies, the change-progress state machine), plus DeleteDomain's state removal and DescribeDomainHealth's health projection.

## Evidence
Cause 1/2/3/5: replay detail lists each omission as "$.DomainStatus.<Block>: missing (expected {...}, got None)" - e.g. "$.DomainStatus.AIMLOptions: missing (expected {'NaturalLanguageQueryGenerationOptions': {'CurrentState': 'NOT_ENABLED', 'DesiredState': 'DISABLED'}, 'S3VectorsEngine': {'Enabled': False}, 'ServerlessVectorAcceleration': {'Enabled': False}}, got None)" and "$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus: value_mismatch (expected 'Pending', got 'ApplyingChanges')".
Cause 4: "$.DomainConfig.AccessPolicies.Status.UpdateVersion: value_mismatch (expected 2, got 1)" plus the missing {Options,Status} wrappers.
Cause 6: expected '{"Version":"2012-10-17","Statement":[{...}]}' (as sent); actual '{"Version": "2012-10-17", "Statement": [{...}]}'.
Cause 7: "$.ChangeProgressStatus.PendingProperties[0]: value_mismatch (expected 'AIMLOptions.NaturalLanguageQueryGenerationOptions : {\"CurrentState\":\"NOT_ENABLED\",\"DesiredState\":\"DISABLED\"}', got 'AccessPolicies')" and "$.ChangeProgressStatus.ChangeProgressStages: missing (expected [], got None)".
Cause 8: "status: expected 200, got 409" on CreateDomain.
Cause 9: "status: expected 404, got 200" on DeleteDomain.
Cause 10: "$.ActiveAvailabilityZoneCount: value_mismatch (expected '1', got 'NotAvailable')", "$.MasterNode: value_mismatch (expected 'Available', got 'UnAvailable')", "$.EnvironmentInformation: type_mismatch (expected None, got [...])".

## Steps to Reproduce
1. Replay esdef-create-domain-gp3-server-defaults step task#1 (minimal CreateDomain) and compare DomainStatus field by field with the recording; then step task#2 (DescribeDomain) to confirm the omission is in stored state.
2. Replay opensearch-domain-lifecycle-and-config-inspection steps task#2 (AccessPolicies whitespace), task#3 (DescribeDomains), task#4 (DescribeDomainConfig/UpdateVersion), task#5 (DescribeDomainChangeProgress), task#7 (DescribeDomainHealth) and teardown#17 (DeleteDomain body).
3. Replay esval-ebs-gp2-iops-not-accepted teardown#2 for the double-delete, and esdef-create-domain-gp2-server-defaults task#2 for the 409 on a create AWS accepts.
