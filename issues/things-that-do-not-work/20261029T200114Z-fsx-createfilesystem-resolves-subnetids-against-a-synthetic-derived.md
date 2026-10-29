Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fsx.backup-restores-file-system-configuration fsx.data-repository-association-invalid-repository-path-rejected fsx.storage-capacity-update-below-ten-percent-floor-refused
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx.backup-restores-file-system-configuration|_create_file_system#4|AssertionError|raise AssertionError(
Issue-Key: fsx.data-repository-association-invalid-repository-path-rejected|_create_file_system#5|AssertionError|raise AssertionError(
Issue-Key: fsx.storage-capacity-update-below-ten-percent-floor-refused|_create_file_system#4|AssertionError|raise AssertionError(
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# FSx CreateFileSystem resolves SubnetIds against a synthetic derived default-VPC subnet set instead of the account's EC2 subnets, so every client-created subnet is refused InvalidNetworkSettings

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
Cause 1 of 1: `CreateFileSystem` refuses any subnet id that the client actually created through EC2, because FSx never consults EC2's subnet state — it compares the requested id against a set of subnet ids it DERIVES by hashing Region/zone names.

AWS: `CreateFileSystem` places a file system in the subnets named by the required input member `SubnetIds` (Smithy: `CreateFileSystem.SubnetIds` -> `SubnetIds`, length 0..50, member `SubnetId` length 15..24 pattern `^(subnet-[0-9a-f]{8,})$`), and those subnets are ordinary EC2 subnets the caller creates with `ec2:CreateSubnet` in a VPC it owns. When the named subnets exist in the caller's account and Region, AWS returns HTTP 200 with a `FileSystem` whose `Lifecycle` is `CREATING` (documented: "The CreateFileSystem call returns while the file system's lifecycle state is still CREATING"), and whose `VpcId` is the VPC those subnets belong to. The modeled error `InvalidNetworkSettings` (HTTP 400, "One or more network settings specified in the request are invalid", with member `InvalidSubnetId` = "The subnet ID that is either invalid or not part of the VPC specified") is reserved for a subnet id that genuinely names no subnet of the account — NOT for a subnet the account just created.

Emulator: the handler resolves the request's subnet ids through `resolve_requested_subnets` -> `resolve_subnet` -> `region_subnets`, and `region_subnets` SYNTHESIZES the account's network: a VPC id derived as `vpc-<sha256(region)[..4 bytes]>` and one subnet per hardcoded availability-zone suffix with id `subnet-<sha256("<region><suffix>")[..17 hex]>`. Nothing in that set can ever be an id EC2 minted, so every `CreateFileSystem` naming a client-created subnet is refused with HTTP 400 `InvalidNetworkSettings` and message `The subnet ID '<subnet-id>' does not exist` (fixed text `The subnet ID '` / `' does not exist`; the variable part is the subnet id copied from the request — in the replay it was the id EC2's `CreateSubnet` had just returned for a VPC/subnet the check itself created in each of us-west-2a/b/c). The handler's own doc comment admits this: "when EC2's subnet operations stop answering 501, `resolve_subnet` is the one place here that has to start consulting the stored subnets beside these derived ones" — EC2 `CreateSubnet` now mints and returns subnet ids, so that condition is met and the derived-only resolution is now a defect.

Why: a client cannot create ANY FSx file system in its own network. Every FSx workflow that begins by building a VPC, security group and subnet — which is the documented and only realistic way to call `CreateFileSystem` — dies at the first call with a 400 claiming a subnet the client just created does not exist. Everything downstream of a file system (backups and restores, data-repository associations, storage-capacity updates, administrative actions, volumes, SVMs) is therefore unreachable, and a client that trusts `InvalidNetworkSettings` will conclude its own EC2 network is broken. All three failing fsx checks never reached the behavior they test; they failed on this create.

Fix: the subnet resolution in this service's state module — `resolve_subnet` / `region_subnets` / `default_vpc_id` in the fsx crate's `src/state.rs`, used by `CreateFileSystem`'s pre-allocation guard (`resolve_requested_subnets`, called from the handler in `src/ops/create_file_system.rs`). It must resolve a requested `SubnetId` against the account's real EC2 subnet store for the signed Region (and take `VpcId`/AvailabilityZone from that record for `FileSystem$VpcId` and the per-type configurations), keeping `InvalidNetworkSettings` only for ids that resolve to nothing there. The same derived-network shortcut is filed for another service as the open issue `20261029T193620Z-dax-createsubnetgroup-refuses-every-client-created-subnet-id.md` (dax `CreateSubnetGroup`) — that issue is a different service crate and does not cover this one; `20261022T193451Z-ec2-createsubnet-stub.md` (ec2 `CreateSubnet`) also does not cover it, since in this replay `CreateSubnet` succeeded and it is FSx that refuses the resulting id.

Traces: fsx.backup-restores-file-system-configuration, fsx.data-repository-association-invalid-repository-path-rejected, fsx.storage-capacity-update-below-ten-percent-floor-refused — all three fail on the same `CreateFileSystem` refusal, for both an OpenZFS SINGLE_AZ_1 (64 GiB) and a Lustre SCRATCH_2 (1200 GiB) file system, in each of three availability zones.

## Evidence
Expected (AWS): ec2:CreateVpc + ec2:CreateSubnet, then fsx:CreateFileSystem with SubnetIds=[that subnet] -> HTTP 200, FileSystem.Lifecycle="CREATING", FileSystem.VpcId = the created VPC, later transitioning to AVAILABLE.

Actual (emulator): HTTP 400, Error.Code="InvalidNetworkSettings", Error.Message="The subnet ID 'subnet-00000000000000030' does not exist" (InvalidSubnetId echoes the same id), for a subnet EC2 in the same emulator had just created and returned. Retried in all three zones us-west-2a/us-west-2b/us-west-2c with a fresh subnet each time; every attempt refused identically.

Replay output:
- fsx.backup-restores-file-system-configuration: "AssertionError: no zone in ['us-west-2a', 'us-west-2b', 'us-west-2c'] accepted a 64 GiB OpenZFS SINGLE_AZ_1 file system, so the backup and restore under test were never reachable; last refusal: An error occurred (InvalidNetworkSettings) when calling the CreateFileSystem operation: The subnet ID 'subnet-00000000000000030' does not exist"
- fsx.data-repository-association-invalid-repository-path-rejected: "AssertionError: no zone in ['us-west-2a', 'us-west-2b', 'us-west-2c'] accepted a 1200 GiB LUSTRE SCRATCH_2 file system; last refusal: An error occurred (InvalidNetworkSettings) when calling the CreateFileSystem operation: The subnet ID 'subnet-00000000000000030' does not exist"
- fsx.storage-capacity-update-below-ten-percent-floor-refused: same refusal, "...so the capacity semantics under test were never reachable".

Code: the fsx crate's src/state.rs defines `region_subnets(region)` as the default VPC id `vpc-<sha256(region)>` plus one subnet per hardcoded AZ suffix with id `subnet-<sha256("<region><suffix>")[..17]>`; `resolve_subnet` only searches that derived list; `resolve_requested_subnets` returns Err(first unresolved id); `src/ops/create_file_system.rs` turns that Err into `subnet_does_not_exist()` -> InvalidNetworkSettings. No read of EC2 state exists anywhere in the path.

## Steps to Reproduce
1. Boot the emulator.
2. ec2:CreateVpc CidrBlock=10.0.0.0/16 -> note VpcId.
3. ec2:CreateSecurityGroup in that VPC; ec2:AuthorizeSecurityGroupIngress IpProtocol=-1 from 10.0.0.0/16.
4. ec2:CreateSubnet VpcId=<vpc> CidrBlock=10.0.1.0/24 AvailabilityZone=us-west-2a -> note the returned SubnetId (it succeeds).
5. fsx:CreateFileSystem FileSystemType=OPENZFS StorageCapacity=64 SubnetIds=[<that SubnetId>] SecurityGroupIds=[<sg>] OpenZFSConfiguration={DeploymentType:SINGLE_AZ_1, ThroughputCapacity:64, AutomaticBackupRetentionDays:0, RootVolumeConfiguration:{DataCompressionType:ZSTD, RecordSizeKiB:128}}.
6. Observe HTTP 400 InvalidNetworkSettings "The subnet ID '<SubnetId>' does not exist" instead of a 200 with Lifecycle=CREATING. Repeat with FileSystemType=LUSTRE StorageCapacity=1200 LustreConfiguration.DeploymentType=SCRATCH_2 for the same refusal.
