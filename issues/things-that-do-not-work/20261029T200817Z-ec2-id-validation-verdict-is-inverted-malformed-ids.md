Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-carrier-gateway-vpc-attachment
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-carrier-gateway-vpc-attachment ec2-copy-volumes-error-handling ec2-describe-launch-template-versions-malformed-id-rejected ec2-get-image-ancestry-contract ec2-image-tasks-and-usage-report-invalid-input-errors ec2-interruptible-capacity-reservation-allocation ec2-ipam-transit-gateway-and-instance-association-error-paths ec2-describe-volumes-attributes-and-unknown-id-rejection ec2-page-describe-volumes-tag-filtered-two-pages ec2-public-ipv4-pool-lifecycle ec2-hardware-and-byoip-not-found-error-catalog
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateCarrierGateway|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateCarrierGateway|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CopyVolumes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CopyVolumes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeLaunchTemplateVersions|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeLaunchTemplateVersions|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|GetImageAncestry|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|GetImageAncestry|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateStoreImageTask|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateStoreImageTask|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateInterruptibleCapacityReservationAllocation|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateInterruptibleCapacityReservationAllocation|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssociateIamInstanceProfile|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AssociateIamInstanceProfile|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeVolumes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeVolumes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribePublicIpv4Pools|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteCoipPool|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteCoipPool|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 id-validation verdict is inverted: malformed ids answered as NotFound and unknown ids as Malformed

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCarrierGateway

## Description
Ten net-new causes, all one defect family: the EC2 emulator decides the WRONG verdict when it screens a resource id. AWS distinguishes a syntactically invalid id (`<Resource>ID.Malformed`, message "The <resource> ID '<id>' is malformed") from a syntactically valid id that does not resolve (`<Resource>ID.NotFound` / `<Resource>.NotFound`, message "The <resource> ... does not exist"), and it also uses a per-resource error-code spelling. The emulator returns the opposite verdict, or the wrong code spelling, for each operation below. In every message quoted here the fixed text is the requirement; the id, name or value inside quotes is taken from the request.

Cause 1 of 10 - CreateCarrierGateway (fixture ec2-carrier-gateway-vpc-attachment, step task#4). AWS: a well-formed-but-unknown VPC id is NotFound - code `InvalidVpcID.NotFound`, message "The vpc ID '<vpc-id>' does not exist". Smithy: ec2 is awsQuery/ec2Query and models no per-operation error shapes, so the code/message pair is recorded behavior only; the recording shows that pair at HTTP 400. Emulator: 400 with code `InvalidVpcId.Malformed` and "The vpc ID '<vpc-id>' is malformed" - both the verdict and the code's capitalisation (`InvalidVpcId` vs `InvalidVpcID`) are wrong. Why: a client that retries or creates the missing VPC on NotFound instead treats its own input as unparseable and gives up. Fix: the id-screening step in CreateCarrierGateway's validation.

Cause 2 of 10 - CopyVolumes (ec2-copy-volumes-error-handling#task#1). AWS: a volume id that does not match the vol- id grammar is `InvalidVolumeID.Malformed`, "The volume ID '<vol-id>' is malformed" (recorded behavior only). Emulator: `InvalidVolume.NotFound`, "The volume '<vol-id>' does not exist." Fix: CopyVolumes validation must screen id syntax before lookup.

Cause 3 of 10 - DescribeLaunchTemplateVersions (ec2-describe-launch-template-versions-malformed-id-rejected#task#4). AWS: `InvalidLaunchTemplateId.Malformed`, "Malformed launch template Id: <lt-id>" (recorded behavior only). Emulator: `InvalidLaunchTemplateId.NotFound`, "The launch template ID '<lt-id>' does not exist".

Cause 4 of 10 - GetImageAncestry (ec2-get-image-ancestry-contract#task#3). AWS: `InvalidAMIID.Malformed`, "The image ID '<ami-id>' is malformed". Emulator: `InvalidAMIID.NotFound`, "The image id '[<ami-id>]' does not exist".

Cause 5 of 10 - CreateStoreImageTask (ec2-image-tasks-and-usage-report-invalid-input-errors#task#3). The same inversion in the other direction: AWS answers `InvalidAMIID.NotFound`, "The image id '[<ami-id>]' does not exist" (note AWS's own bracketed-list rendering of the id), the emulator answers `InvalidAMIID.Malformed`, "The image ID '<ami-id>' is malformed". Causes 4 and 5 together show the AMI screen is applying a single fixed verdict rather than testing the id.

Cause 6 of 10 - CreateInterruptibleCapacityReservationAllocation (ec2-interruptible-capacity-reservation-allocation#task#2). AWS: `InvalidCapacityReservationId.NotFound`, "The capacity-reservation ID '<cr-id>' does not exist". Emulator: `InvalidCapacityReservationId.Malformed`, "Capacity Reservation ID '<cr-id>' is malformed".

Cause 7 of 10 - AssociateIamInstanceProfile (ec2-ipam-transit-gateway-and-instance-association-error-paths#task#1). AWS: `InvalidInstanceID.Malformed`, "The instance ID '<i-id>' is malformed". Emulator: `InvalidInstanceID.NotFound`, "The instance ID '<i-id>' does not exist".

Cause 8 of 10 - DescribeVolumes (ec2-describe-volumes-attributes-and-unknown-id-rejection#task#3). AWS rejects an id whose random part is the wrong width as a parameter problem: `InvalidParameterValue`, "Value (<vol-id>) for parameter volumes is invalid. Expected: 'vol-...'." Emulator: `InvalidVolume.NotFound`, "The volume '<vol-id>' does not exist."

Cause 9 of 10 - DescribePublicIpv4Pools (ec2-public-ipv4-pool-lifecycle#task#3). Code spelling only: AWS uses `InvalidPublicIpv4PoolID.NotFound`, the emulator `InvalidPublicIpv4Pool.NotFound`; message matches.

Cause 10 of 10 - DeleteCoipPool (ec2-hardware-and-byoip-not-found-error-catalog#task#2). AWS: `InvalidIpv4PoolCoipId.Malformed`, "The coip-pool ID '<pool-id>' is malformed". Emulator: `InvalidPoolID.Malformed` with emulator-authored prose that explains the grammar ('Invalid id: "...". A customer-owned address pool id is ...').

Why (all causes): error codes are the contract SDKs, waiters and retry policies branch on. An inverted verdict makes a recoverable NotFound look like a client bug and vice versa, and a misspelled code matches no SDK error class at all.

Fix: the shared EC2 id-screening helper plus each named operation's validation step must (a) test id syntax first and emit the resource's Malformed code, (b) emit the resource's NotFound code only after a syntactically valid id fails lookup, and (c) use the AWS spelling of the code for that resource. The open issue '20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md' covers a different set of operations and does not carry these replay keys.

Traces: see the fixture/step list; handlers live under services/service-com-amazonaws-ec2-amazonec2/src/ops/ (create_carrier_gateway.rs, copy_volumes.rs, describe_launch_template_versions.rs, get_image_ancestry.rs, create_store_image_task.rs, create_interruptible_capacity_reservation_allocation.rs, associate_iam_instance_profile.rs, describe_volumes.rs, describe_public_ipv4_pools.rs, delete_coip_pool.rs).

## Evidence
Expected vs actual, per cause (replay detail, verbatim):
1. CreateCarrierGateway: expected code 'InvalidVpcID.NotFound' / "The vpc ID 'vpc-00000000' does not exist"; actual 'InvalidVpcId.Malformed' / "The vpc ID 'vpc-00000000' is malformed" (400 vs 400).
2. CopyVolumes: expected 'InvalidVolumeID.Malformed' / "The volume ID 'vol-0123456789abcdef0' is malformed"; actual 'InvalidVolume.NotFound' / "The volume 'vol-0123456789abcdef0' does not exist."
3. DescribeLaunchTemplateVersions: expected 'InvalidLaunchTemplateId.Malformed' / 'Malformed launch template Id: lt-0123456789abcdef0'; actual 'InvalidLaunchTemplateId.NotFound' / "The launch template ID 'lt-0123456789abcdef0' does not exist".
4. GetImageAncestry: expected 'InvalidAMIID.Malformed' / "The image ID 'ami-0123456789abcdef0' is malformed"; actual 'InvalidAMIID.NotFound' / "The image id '[ami-0123456789abcdef0]' does not exist".
5. CreateStoreImageTask: expected 'InvalidAMIID.NotFound' / "The image id '[ami-00000000]' does not exist"; actual 'InvalidAMIID.Malformed' / "The image ID 'ami-00000000' is malformed".
6. CreateInterruptibleCapacityReservationAllocation: expected 'InvalidCapacityReservationId.NotFound' / "The capacity-reservation ID 'cr-00000000000000000' does not exist"; actual 'InvalidCapacityReservationId.Malformed' / "Capacity Reservation ID 'cr-00000000000000000' is malformed".
7. AssociateIamInstanceProfile: expected 'InvalidInstanceID.Malformed' / "The instance ID 'i-0123456789abcdef0' is malformed"; actual 'InvalidInstanceID.NotFound' / "The instance ID 'i-0123456789abcdef0' does not exist".
8. DescribeVolumes: expected 'InvalidParameterValue' / "Value (vol-1234567890abcdef0) for parameter volumes is invalid. Expected: 'vol-...'."; actual 'InvalidVolume.NotFound' / "The volume 'vol-1234567890abcdef0' does not exist."
9. DescribePublicIpv4Pools: expected 'InvalidPublicIpv4PoolID.NotFound'; actual 'InvalidPublicIpv4Pool.NotFound'.
10. DeleteCoipPool: expected 'InvalidIpv4PoolCoipId.Malformed' / "The coip-pool ID 'coip-pool-00000000000000000' is malformed"; actual 'InvalidPoolID.Malformed' / 'Invalid id: "coip-pool-00000000000000000". A customer-owned address pool id is ...'.
All ten clusters report verdict=mismatch with empty stub_path and empty owned_by.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Inspect the clusters for fixture ec2-carrier-gateway-vpc-attachment step task#4 and the nine fixture#step bindings in trace_ids: each is an ec2 Query-protocol request carrying a malformed or unknown resource id; compare the Response.Errors.Error.Code and .Message of the emulator's 400 against the recorded 400.
