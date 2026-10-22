Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-instances-catalog-list-instance-types
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-instances-catalog-list-instance-types workspaces-instances-catalog-list-regions workspaces-instances-delete-volume-not-found
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances|ListInstanceTypes|2xx->2xx|$.InstanceTypes[*]
Issue-Key: workspaces-instances|ListRegions|2xx->2xx|$.Regions[RegionName="ap-east-1"]
Issue-Key: workspaces-instances|ListRegions|2xx->2xx|$.Regions[RegionName="ap-southeast-5"]
Issue-Key: workspaces-instances|ListRegions|2xx->2xx|$.Regions[RegionName="eu-south-2"]
Issue-Key: workspaces-instances|ListRegions|2xx->2xx|$.Regions[RegionName="eu-west-3"]
Issue-Key: workspaces-instances|ListRegions|2xx->2xx|$.Regions[RegionName="me-central-1"]
Issue-Key: workspaces-instances|ListRegions|2xx->2xx|$.Regions[RegionName="us-east-2"]
Issue-Key: workspaces-instances|DeleteVolume|4xx->4xx|$.ResourceType
Issue-Commit: 5e075c36a45dbd6adbae35f94fdd23b97a2c86ba

# WorkSpaces Instances catalogs and error ResourceType diverge from recorded service

> _Note: this issue was identified against the code base at commit `5e075c36a45dbd6adbae35f94fdd23b97a2c86ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypes

## Description
Three distinct net-new root causes on the WorkSpaces Instances (sdkId workspaces-instances, crate services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice) read/catalog and resource-error paths, found by replaying the wire fixture corpus simparity-wire-aws-v1. They share one fault: the service answers from small hand-written tables and token constants that this emulator invented, instead of the service facts the recordings show, so every reported value is a plausible-looking subset or near-miss of the real one. A fixer must close ALL THREE causes before submitting. Smithy citations are from services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/smithy-model/com-amazonaws-workspacesinstances-eucmifrontendapiservice.json. No other issue is open on this service.

Cause 1 of 3: ListInstanceTypes reports an invented 18-row instance-type catalog, and none of the instance types the real service reports.
AWS: ListInstanceTypes (Smithy: output InstanceTypes, target InstanceTypes = list of InstanceTypeInfo, @required; InstanceTypeInfo.InstanceType @length 1..64 @pattern ^([a-z0-9-]+)\.([a-z0-9]+)$, SupportedInstanceConfigurations = list of SupportedInstanceConfiguration {BillingMode, PlatformType, Tenancy}; @readonly, @paginated items=InstanceTypes, MaxResults @range 1..600; errors AccessDeniedException 403, ValidationException 400, ThrottlingException 429, InternalServerException 500) reports the WHOLE supported instance-type catalog, which is service data and is large: the recording answers HTTP 200 with 597 InstanceTypeInfo entries, the first being InstanceType 'c5.12xlarge' carrying its full set of (BillingMode, PlatformType, Tenancy) combinations — MONTHLY/HOURLY crossed with the platform types ('Red Hat Enterprise Linux', 'Windows BYOL', 'Red Hat BYOL Linux', 'Linux/UNIX', 'Windows', 'Ubuntu Pro Linux', 'SUSE Linux', ...) and tenancies SHARED/DEDICATED. Recorded behavior only for catalog MEMBERSHIP: the model enumerates no instance type, so the requirement is that the catalog this operation reports is the service's real one (every family/size a caller can ask for, with the configuration combinations each really supports), not a sample of it.
Emulator: HTTP 200 with a list that contains NONE of the recorded entries — the replay reports every expected index $.InstanceTypes[0] .. $.InstanceTypes[596] as 'missing ... got None'. The handler is implemented and pages correctly, but it iterates a hard-coded 18-row table of this emulator's own choosing (c5.2xlarge, c5.large, c5.xlarge, g4dn.2xlarge, g4dn.xlarge, g6.xlarge, m5.2xlarge, m5.large, m5.xlarge, m6i.large, m6i.xlarge, r5.2xlarge, r5.large, r5.xlarge, t3.large, t3.medium, t3.small, t3.xlarge) whose doc comment states the rows are 'CHOSEN'. c5.12xlarge and the other 596 recorded types are simply absent.
Why: this operation exists so a client can discover which instance types it may pass to CreateWorkspaceInstance. Against the emulator, discovery returns a 18-element list that overlaps the real catalog almost nowhere, so a client that picks a type from the real service's answer is refused by the emulator, a client that picks one from the emulator's answer builds a request the real service rejects, and anything that asserts on catalog size or on a specific family (compute-optimised 12xlarge, GPU, etc.) is wrong in both directions. Capacity-planning and type-selection logic cannot be exercised at all.
Fix: this service's instance-type catalog data — the INSTANCE_TYPE_CATALOG table the ListInstanceTypes handler reports from (the handler's paging, filtering and NextToken logic are not at fault); it must carry the service's real instance-type set and each row's real supported configuration combinations.
Traces: workspaces-instances-catalog-list-instance-types#task#1

Cause 2 of 3: ListRegions omits six Regions the real service reports as supported.
AWS: ListRegions (Smithy: output Regions, target RegionList = list of Region, @required; Region.RegionName @pattern ^[-0-9a-z]{1,31}$; @readonly, @paginated items=Regions, MaxResults @range 1..25; errors AccessDeniedException 403, ValidationException 400, ThrottlingException 429, InternalServerException 500) reports the Regions in which WorkSpaces Instances is available. The recording answers HTTP 200 with a Region set that includes ap-east-1, ap-southeast-5, eu-south-2, eu-west-3, me-central-1 and us-east-2. Recorded behavior only for membership (the model enumerates no Region); the requirement is that the reported set is the service's availability set, and in particular that a commercial-partition Region the service supports — notably us-east-2 — is in it.
Emulator: HTTP 200, but those six entries are absent: $.Regions[RegionName="ap-east-1"], ["ap-southeast-5"], ["eu-south-2"], ["eu-west-3"], ["me-central-1"] and ["us-east-2"] each report 'missing (expected {...}, got None)'. The handler reports a 14-entry hard-coded list whose doc comment states it is 'a set this emulator OWNS'.
Why: callers use ListRegions to decide where they may deploy, and to gate region-parameterised tests. With these Regions missing, a client legitimately targeting us-east-2 (or any of the other five) is told by the emulator that the service is unavailable there, so region-selection code takes the wrong branch and multi-region deployment flows fail against the emulator while succeeding on AWS.
Fix: this service's Region catalog data — the SUPPORTED_REGIONS list the ListRegions handler reports from; extend it to the service's real commercial-partition availability set, keeping it sorted and duplicate-free as the handler's contract assumes.
Traces: workspaces-instances-catalog-list-regions#task#1

Cause 3 of 3: the not-found error for a volume reports ResourceType 'VOLUME' where the service reports 'EBS_VOLUME'.
AWS: DeleteVolume (Smithy: input VolumeId @required @pattern ^vol-[0-9a-zA-Z]{1,63}$; errors ResourceNotFoundException, ValidationException 400, ConflictException 409, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500) for an unknown volume id answers the ResourceNotFoundException shape, whose members Message, ResourceId and ResourceType are all @required and whose ResourceType is a free String (the model enumerates no token for it — ResourceTypeEnum is the EC2-style tag-specification enum and is unrelated). Recorded behavior only for the token: the recording answers HTTP 400 carrying ResourceType 'EBS_VOLUME'. The status and the error code already match; the diverging value is a structured, required, machine-read member of the error body, not a message.
Emulator: HTTP 400 with the right shape but $.ResourceType = 'VOLUME' (value_mismatch, expected 'EBS_VOLUME'). The token comes from a single constant, VOLUME_RESOURCE_TYPE, declared in this service's state module.
Why: ResourceType is the field a client switches on to tell which kind of resource was missing (a WorkSpace Instance versus its EBS volume). A client that branches on 'EBS_VOLUME' — the token AWS sends — falls through its volume-not-found branch against the emulator and reports an unknown resource kind or an unhandled error instead.
Fix: this service's resource-error token constants — the volume ResourceType constant used by the DeleteVolume (and any sibling volume-scoped) not-found path, which must emit the recorded 'EBS_VOLUME'.
Traces: workspaces-instances-delete-volume-not-found#task#1

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, state_reset_failures=0, so none of these ran against a stale fixture's state).

Cause 1 — fixture workspaces-instances-catalog-list-instance-types, step task#1, verdict mismatch, expected_status 200 / actual_status 200. Divergence keys: $.InstanceTypes[0] .. $.InstanceTypes[596] (597 keys). Example: '$.InstanceTypes[0]: missing (expected {"InstanceType": "c5.12xlarge", "SupportedInstanceConfigurations": [{"BillingMode": "MONTHLY", "PlatformType": "Red Hat Enterprise Linux", "Tenancy": "DEDICATED"}, {"BillingMode": "HOURLY", "PlatformType": "Windows BYOL", "Tenancy": "SHARED"}, ... ]}, got None)'. Expected: 200 with the service's full instance-type catalog (597 entries in the recording, c5.12xlarge among them). Actual: 200 with a list holding none of them. Code: services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/state.rs lines 262-353 (INSTANCE_TYPE_CATALOG, 18 rows, doc comment 'the rows cover the general-purpose, burstable, compute-optimised, memory-optimised and GPU families at several sizes each'); handler services/.../src/ops/list_instance_types.rs lines 213-227 iterates exactly that table.

Cause 2 — fixture workspaces-instances-catalog-list-regions, step task#1, verdict mismatch, expected_status 200 / actual_status 200. Detail: '$.Regions[RegionName="ap-east-1"]: missing (expected {"RegionName": "ap-east-1"}, got None)' and the same for ap-southeast-5, eu-south-2, eu-west-3, me-central-1, us-east-2. Expected: 200 whose Regions list contains those six. Actual: 200 with the 14 Regions of services/.../src/state.rs lines 382-397 (SUPPORTED_REGIONS: af-south-1, ap-northeast-1, ap-northeast-2, ap-south-1, ap-southeast-1, ap-southeast-2, ca-central-1, eu-central-1, eu-west-1, eu-west-2, il-central-1, sa-east-1, us-east-1, us-west-2), reported whole by services/.../src/ops/list_regions.rs lines 197-205.

Cause 3 — fixture workspaces-instances-delete-volume-not-found, step task#1, verdict mismatch, expected_status 400 / actual_status 400. Detail: '$.ResourceType: value_mismatch (expected "EBS_VOLUME", got "VOLUME")'. Status and error shape/code agree; only the required ResourceType member diverges. Code: services/.../src/state.rs line 51, 'pub const VOLUME_RESOURCE_TYPE: &str = "VOLUME";'.

Not part of this issue, for context: the same corpus shows ec2 CreateVpc answering 501 from the stub services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc.rs (already filed by the sweep) and an ssm GetParameter setup step answering 400 where 200 was recorded; that ssm step is deferred by the per-service bootstrap gate and is adjacent to the already-open FIS issue 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md (its Cause 3 covers SSM GetParameter / ParameterNotFound).

## Steps to Reproduce
Boot the emulator from this checkout and point a botocore 'workspaces-instances' client at its data listener (region us-west-2, any credentials), then:
1. Cause 1: client.list_instance_types() (optionally paginating with MaxResults up to 600) — observe a list of 18 instance types (c5.2xlarge, c5.large, c5.xlarge, g4dn.*, g6.xlarge, m5.*, m6i.*, r5.*, t3.*) and that 'c5.12xlarge' is absent, where the recording returns 597 entries headed by c5.12xlarge with its full SupportedInstanceConfigurations set.
2. Cause 2: client.list_regions() — observe that RegionName 'us-east-2', 'eu-west-3', 'ap-east-1', 'ap-southeast-5', 'eu-south-2' and 'me-central-1' are all missing from Regions.
3. Cause 3: client.delete_volume(VolumeId='vol-0123456789abcdef0') for a volume that was never created — observe HTTP 400 ResourceNotFoundException whose ResourceType member is 'VOLUME' instead of the recorded 'EBS_VOLUME'.
Equivalently, re-replay the wire fixture steps named in Issue-Trace/Issue-Step and in trace_ids: workspaces-instances-catalog-list-instance-types#task#1, workspaces-instances-catalog-list-regions#task#1, workspaces-instances-delete-volume-not-found#task#1.
