Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.create-volume-delete-volume-lifecycle workspaces-instances.create-volume-invalid-tag-key-pattern-rejected workspaces-instances.create-volume-iops-exceeds-range-rejected workspaces-instances.associate-volume-nonexistent-instance-not-found workspaces-instances.create-volume-clienttoken-reuse-rejected workspaces-instances.create-volume-nonexistent-snapshot-conflict-rejected workspaces-instances.list-regions-malformed-next-token-validation workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation workspaces-instances.delete-workspace-instance-state-transition-to-deallocated
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances|DescribeVolumes|InvalidVolume.NotFound
Issue-Key: workspaces-instances|CreateTags|InvalidID
Issue-Key: workspaces-instances.create-volume-clienttoken-reuse-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: workspaces-instances.create-volume-nonexistent-snapshot-conflict-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: workspaces-instances|ListRegions|ValidationException
Issue-Key: workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation|run#4|AssertionError|assert message == (
Issue-Key: workspaces-instances.delete-workspace-instance-state-transition-to-deallocated|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Commit: c4171394ecd9e8acdff90bddf637ffaae1174b81

# WorkSpaces Instances: 6 fidelity divergences across CreateVolume, ListRegions, ListWorkspaceInstances and the service-linked role name

> _Note: this issue was identified against the code base at commit `c4171394ecd9e8acdff90bddf637ffaae1174b81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume

## Description
This issue bundles SIX distinct root causes found in one behavioral replay of the workspaces-instances emulator (7/25 checks passed). Each cause below is independent and must be closed on its own before this issue is submitted; the failing check ids for each are listed with it. Causes are numbered 1 of 6 .. 6 of 6.

=== Cause 1 of 6: a volume CreateVolume mints exists in no EC2/EBS state, so no cross-service read resolves it ===
AWS: Smithy: CreateVolume output VolumeId pattern ^vol-[0-9a-zA-Z]{1,63}$ and the request carries EBS's own configuration surface (AvailabilityZone, SizeInGB range 1..16384, VolumeType enum standard|io1|io2|gp2|sc1|st1|gp3, Iops, Throughput, SnapshotId ^snap-[0-9a-zA-Z]{1,63}$, Encrypted, KmsKeyId, TagSpecifications whose ResourceType is `volume`); AssociateVolume/DisassociateVolume attach that volume to an instance at a Device, which is an EBS attachment. So the volume a successful CreateVolume reports is a real EBS volume in the caller's account and the named Availability Zone: ec2:DescribeVolumes with that VolumeId resolves an `available` volume of the requested size/type/zone carrying the requested tags, ec2:CreateTags on it succeeds, and DeleteVolume makes it disappear from EC2's view.
Emulator: the id lives only in this crate's own store. ec2:DescribeVolumes(VolumeIds=[the reported id]) answers HTTP 400 `InvalidVolume.NotFound` with message "The volume '<VolumeId>' does not exist." (fixed text; the quoted id is the value from the request), and ec2:CreateTags on the same id answers `InvalidID` with "The ID '<VolumeId>' is not valid" — note that even for a well-formed but unknown volume id real EC2 answers `InvalidVolume.NotFound`, not `InvalidID`, so the EC2 CreateTags id-classification is a secondary wrongness that disappears once the volume really exists in EC2 state.
Why: every client workflow that creates a volume here and then inspects, tags, snapshots or attaches it with EC2 breaks; worse, three checks whose real subject is a REJECTION rule (tag-key pattern, gp3 Iops range) never got to assert it, because their positive control could not be read back, so the emulator's permissiveness on those rules is still unverified.
Fix: CreateVolume's state/propagation path must materialise the volume in the EC2 emulator's EBS volume state (and DeleteVolume must remove it there) — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_volume.rs + src/state.rs, against services/service-com-amazonaws-ec2-amazonec2/src/state.rs volume state. The already-open issue 20261024T122725Z-workspaces-instances-createvolume-event-propagation.md describes this same propagation gap from static inspection; this cause is its replayed evidence and the check ids that confirm a fix. It was NOT reported as owning these checks by the replay, so it is named here rather than treated as covering them.
Traces: workspaces-instances.create-volume-delete-volume-lifecycle, workspaces-instances.create-volume-invalid-tag-key-pattern-rejected, workspaces-instances.create-volume-iops-exceeds-range-rejected, workspaces-instances.associate-volume-nonexistent-instance-not-found.

=== Cause 2 of 6: CreateVolume replays a spent ClientToken instead of refusing it ===
AWS: recorded behavior only (the model does suggest caching: CreateVolume is @idempotent and ClientToken carries @idempotencyToken, length 1..64, pattern ^[\x20-\x7E]+$). The recording shows ClientToken is a single-use replay guard, not a request cache: a second CreateVolume carrying an already-committed ClientToken — even with byte-identical parameters — is REFUSED with the modeled ConflictException (HTTP 409 per the model) and message "The client token provided does not match the request" (entirely fixed text, no request values).
Emulator: the second call succeeds with HTTP 200 and hands back the first volume's id (observed: 200 with the same vol- id as the first create). src/ops/create_volume.rs explicitly implements EC2-style replay: "a repeat call carrying a token this service has already created a volume FOR THE SAME PARAMETERS replays the SAME id and creates nothing" via crate::state::resolve_client_token.
Why: a client that reuses a token by mistake believes it created a second volume and never sees the conflict AWS raises; a client that relies on the refusal to detect a double-submit gets silence, and quota/billing accounting diverges.
Fix: the ClientToken branch of CreateVolume's handler (src/ops/create_volume.rs, crate::state::resolve_client_token) must refuse a spent token with ConflictException regardless of parameter equality, not replay it.
Traces: workspaces-instances.create-volume-clienttoken-reuse-rejected.

=== Cause 3 of 6: CreateVolume accepts a well-formed but nonexistent SnapshotId ===
AWS: recorded behavior only for the classification (the model lists both ValidationException HTTP 400 and ConflictException HTTP 409 for CreateVolume, and SnapshotId pattern ^snap-[0-9a-zA-Z]{1,63}$). The recording shows a SnapshotId that matches the pattern but names no snapshot is rejected with ValidationException (HTTP 400) and message "Create volume failed (InvalidSnapshotID.Malformed: Value ( <SnapshotId> ) for parameter SNAPSHOT is invalid. )" — fixed text is "Create volume failed (InvalidSnapshotID.Malformed: Value ( " ... " ) for parameter SNAPSHOT is invalid. )", and the only variable part is the SnapshotId taken verbatim from the request (spaces inside the parentheses are part of the fixed text).
Emulator: the call SUCCEEDS with HTTP 200 and mints a volume (observed 200 {"VolumeId": "vol-…"}), i.e. a blank volume is created for a snapshot that does not exist. validate_configuration in src/ops/create_volume.rs checks size/iops/throughput/kms but never resolves SnapshotId against any snapshot.
Why: a restore-from-snapshot workflow silently gets an empty volume instead of an error, so data loss is reported as success; the client's error path is never exercised.
Fix: CreateVolume's validation (validate_configuration in src/ops/create_volume.rs) must resolve SnapshotId and refuse an unresolvable one with the ValidationException above.
Traces: workspaces-instances.create-volume-nonexistent-snapshot-conflict-rejected.

=== Cause 4 of 6: ListRegions refuses any NextToken instead of ignoring an unissued one ===
AWS: Smithy: ListRegions is @paginated (inputToken/outputToken NextToken, pageSize MaxResults range 1..25) and models ValidationException HTTP 400. Recorded behavior: the real service returns its whole Region catalog in one response and does NOT validate NextToken — a malformed, never-issued NextToken is IGNORED and a normal first page (a non-empty Regions list) is returned. The MaxResults bound IS enforced, with the message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25" (that part matched).
Emulator: any request carrying NextToken is refused with ValidationException (HTTP 400) "NextToken is not a token this service issued" — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/list_regions.rs returns crate::state::invalid_next_token() whenever input.next_token.is_some().
Why: a client or SDK paginator that carries over a stale/foreign token (or any caller that passes a token through) gets a hard 400 where AWS serves the first page, turning a harmless input into a failed call.
Fix: src/ops/list_regions.rs must ignore an unrecognised NextToken and answer the full catalog, rather than share the sibling lists' refusal builder.
Traces: workspaces-instances.list-regions-malformed-next-token-validation.

=== Cause 5 of 6: ListWorkspaceInstances enum-rejection message lists the ProvisionStates value set in the wrong order ===
AWS: Smithy: ListWorkspaceInstances$ProvisionStates is a list of ProvisionStateEnum and the operation models ValidationException HTTP 400. Recorded message (the requirement): "1 validation error detected: Value at 'provisionStates' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [DEALLOCATED, ALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING, ALLOCATING, DEALLOCATING]]". Fixed text is the whole sentence including the member path 'provisionStates' and the bracketed value set in exactly that order; nothing in it comes from the request.
Emulator: the code (ValidationException) and sentence shape are right, but the value set is emitted in the model's declaration order: "… enum value set: [ALLOCATING, ALLOCATED, DEALLOCATING, DEALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING]]" (the model's ProvisionStateEnum order), so the message text does not match AWS's.
Why: clients and test suites that assert on, log or diff the validation message see a different sentence than AWS produces; the message is part of the error contract, so this is a wording defect, not a per-run value.
Fix: the enum-rejection message builder for ProvisionStates must emit AWS's fixed ordering of the value set rather than iterating the generated PROVISION_STATE_ENUM_VALUES order (crate::state / crate::ops::types in services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice).
Traces: workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation.

=== Cause 6 of 6: the WorkSpaces Instances service-linked role is created under a derived name, so iam:GetRole for the published name never resolves ===
AWS: recorded behavior only (IAM's model carries no role-name label). AWS publishes the service-linked role for principal workspaces-instances.amazonaws.com as AWSServiceRoleForWorkspacesInstances; after a successful iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com"), iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") resolves that role (under path /aws-service-role/workspaces-instances.amazonaws.com/). Every WorkSpaces Instances workflow depends on it, since CreateWorkspaceInstance requires the role.
Emulator: workspaces-instances.amazonaws.com is absent from PUBLISHED_SERVICE_LINKED_ROLE_NAMES (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs), so create_service_linked_role.rs falls back to derived_service_linked_role_name, which capitalises only the principal's first label and keeps its hyphen — the role is stored as "AWSServiceRoleForWorkspaces-instances". iam:GetRole for the published name therefore answers NoSuchEntity forever; the check's bounded 60s read-your-writes poll on that name never converges and the check is killed ("Check timed out after 60.0s", exactly the poll's own 60s budget). Nothing in the WorkSpaces Instances state machine itself is implicated: ALLOCATION_SPAN/DEALLOCATION_SPAN are 500ms and GetWorkspaceInstance settles them lazily.
Why: any client that creates the SLR and then reads it back under AWS's published name (the documented prerequisite check before CreateWorkspaceInstance) hangs or fails, and the DeleteWorkspaceInstance ALLOCATED -> DEALLOCATING -> DEALLOCATED transition stays entirely unverified behind it.
Fix: add the published row ("workspaces-instances.amazonaws.com", "AWSServiceRoleForWorkspacesInstances") to PUBLISHED_SERVICE_LINKED_ROLE_NAMES in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs so CreateServiceLinkedRole stops deriving the name. (Filed under service workspaces-instances because that is the leading token of the failing check id; the code to change lives in the IAM crate.)
Traces: workspaces-instances.delete-workspace-instance-state-transition-to-deallocated.

## Evidence
Replay report: workspaces-instances 7/25 (28%), 18 failed. Skipped per the report's own markings: associate-disassociate-volume-roundtrip and its cluster (ALREADY FILED — unimplemented ec2 CreateInternetGateway) and list-tags-for-resource-not-found (OWNED by 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md).

Cause 1 — expected: ec2:DescribeVolumes(VolumeIds=[<id from CreateVolume>]) resolves an available volume of the requested size/type/AZ; actual: ClientError InvalidVolume.NotFound "The volume 'vol-…' does not exist." (create-volume-delete-volume-lifecycle, create-volume-invalid-tag-key-pattern-rejected, create-volume-iops-exceeds-range-rejected). ec2:CreateTags(Resources=[<id>]) expected to succeed; actual ClientError InvalidID "The ID 'vol-…' is not valid" (associate-volume-nonexistent-instance-not-found).

Cause 2 — expected: second CreateVolume with the same ClientToken and identical parameters => ConflictException (409) "The client token provided does not match the request"; actual: HTTP 200 {'VolumeId': 'vol-…'} replaying the first id (AssertionError: expected the call to be rejected, but it SUCCEEDED).

Cause 3 — expected: CreateVolume(SnapshotId=<well-formed, nonexistent snap- id>) => ValidationException (400) "Create volume failed (InvalidSnapshotID.Malformed: Value ( <SnapshotId> ) for parameter SNAPSHOT is invalid. )"; actual: HTTP 200 {'VolumeId': 'vol-…'}.

Cause 4 — expected: ListRegions(NextToken="not-a-real-token-###garbage###") => 200 with a non-empty Regions list; actual: ValidationException (400) "NextToken is not a token this service issued" (botocore.errorfactory.ValidationException raised out of the check). Source: src/ops/list_regions.rs lines 180-185.

Cause 5 — expected message "… enum value set: [DEALLOCATED, ALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING, ALLOCATING, DEALLOCATING]]"; actual "… enum value set: [ALLOCATING, ALLOCATED, DEALLOCATING, DEALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING]]" (AssertionError on the message; the ValidationException code itself matched).

Cause 6 — expected: after iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com"), iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") resolves; actual: NoSuchEntity indefinitely, because the role is stored as "AWSServiceRoleForWorkspaces-instances" (derived_service_linked_role_name in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_service_linked_role.rs lines 117-148; the principal is absent from PUBLISHED_SERVICE_LINKED_ROLE_NAMES in that crate's src/state.rs lines 2986-3125). Check ended as "Check timed out after 60.0s", matching the check's 60s GetRole poll budget.

## Steps to Reproduce
Boot the multi-service emulator with workspaces-instances, ec2 and iam mounted, then:

Cause 1: CreateVolume {AvailabilityZone: "<region>a", SizeInGB: 20, VolumeType: "gp3"}; take the returned VolumeId; call ec2:DescribeVolumes(VolumeIds=[that id]) — InvalidVolume.NotFound; call ec2:CreateTags(Resources=[that id], Tags=[{Key:"k",Value:"v"}]) — InvalidID.

Cause 2: CreateVolume {AvailabilityZone: "<region>a", SizeInGB: 8, VolumeType: "gp3", ClientToken: "tok-1"}; repeat the identical call with ClientToken "tok-1" — expect ConflictException, observe 200 with the first VolumeId.

Cause 3: CreateVolume {AvailabilityZone: "<region>a", SnapshotId: "snap-00000000deadbeef"} — expect ValidationException, observe 200 with a new VolumeId.

Cause 4: ListRegions(MaxResults=5) to confirm the catalog, then ListRegions(NextToken="not-a-real-token-###garbage###") — expect a normal first page, observe ValidationException "NextToken is not a token this service issued".

Cause 5: ListWorkspaceInstances(ProvisionStates=["NOT_A_REAL_STATE"]) and compare the ValidationException message's bracketed enum value set with AWS's ordering.

Cause 6: iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com"), then iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") — NoSuchEntity; iam:ListRoles(PathPrefix="/aws-service-role/workspaces-instances.amazonaws.com/") shows the role stored under the name "AWSServiceRoleForWorkspaces-instances".
