Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fsx.backup-restores-file-system-configuration
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx.backup-restores-file-system-configuration|_openzfs_shape#20|KeyError|"DiskIopsMode": cfg["DiskIopsConfiguration"]["Mode"],
Issue-Commit: c17c2e35cd2971fbefbedcbb04c35ef2babfeba4

# fsx: OpenZFS file systems report no DiskIopsConfiguration — the AUTOMATIC/3-IOPS-per-GB default is never materialized, so DescribeFileSystems, CreateBackup/DescribeBackups and restores all omit the member

> _Note: this issue was identified against the code base at commit `c17c2e35cd2971fbefbedcbb04c35ef2babfeba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFileSystems

## Description
One net-new root cause in the bootstrapped `fsx` emulator, grounded in a failing behavioral check. The one open `fsx` issue (20261105T044046Z-fsx-2-net-new-root-causes-updatefilesystem-enforces.md) covers UpdateFileSystem's storage-capacity floor and CreateDataRepositoryAssociation's 501 stub; neither mentions DiskIopsConfiguration, so this is not covered by it.

=== Cause 1 of 1: the server-side DiskIopsConfiguration default is never computed, so OpenZFSConfiguration comes back without the member ===
Failing check: fsx.backup-restores-file-system-configuration

AWS: Smithy model, `OpenZFSFileSystemConfiguration$DiskIopsConfiguration` targets `DiskIopsConfiguration`, a structure of `Mode` (enum `DiskIopsConfigurationMode`: "Specifies whether the file system is using the AUTOMATIC setting of SSD IOPS of 3 IOPS per GB of storage capacity, or if it using a USER_PROVISIONED value") and `Iops` (long, range 0..2147483647, "The total number of SSD IOPS provisioned for the file system ... The minimum value is calculated as StorageCapacity * 3 * HAPairs (3 IOPS per GB of StorageCapacity)"). The shape's own documentation states the default plainly: "By default, Amazon FSx automatically provisions 3 IOPS per GB of storage capacity." DiskIopsConfiguration is therefore a SERVER-SIDE DEFAULT, not an echo of the request: when `CreateFileSystem` carries no `OpenZFSConfiguration.DiskIopsConfiguration`, the created file system still has one, and every read of that file system reports it. Concretely, for an OpenZFS file system created with no DiskIopsConfiguration, `DescribeFileSystems` returns `OpenZFSConfiguration.DiskIopsConfiguration = {"Mode": "AUTOMATIC", "Iops": 3 * StorageCapacity}` — the Mode is the fixed literal `AUTOMATIC`, and the Iops value is derived from the file system's own stored StorageCapacity (so a 64 GiB file system reports 192; the number is computed from state, never a constant from one recording). The same member is present on the `FileSystem` that `CreateBackup`/`DescribeBackups` embed as the backup's recorded source configuration, and on the file system `CreateFileSystemFromBackup` produces when the restore states no OpenZFSConfiguration of its own, because a backup records what it was taken from. The equivalent default applies to the Windows (SSD) and ONTAP configurations, which carry the same `DiskIopsConfiguration` member and the same "3 IOPS per GB" documented default.

Emulator: `OpenZFSConfiguration` comes back with NO `DiskIopsConfiguration` member at all whenever the create request did not supply one. The create path copies the member straight through from the request instead of defaulting it: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs line 971, inside `stored_open_zfs_configuration`, sets `disk_iops_configuration: requested.disk_iops_configuration.clone()`, so an omitted member is stored as `None` and every later projection of the record (`to_file_system`) serializes it as absent. The check creates a 64 GiB OpenZFS SINGLE_AZ_1 file system without a DiskIopsConfiguration, waits for AVAILABLE, takes a backup, and reads the configuration the backup recorded; building that configuration shape raises `KeyError: 'DiskIopsConfiguration'` at `cfg["DiskIopsConfiguration"]["Mode"]`. The same absence is visible in an unrelated response body from this service: `'OpenZFSConfiguration': {'AutomaticBackupRetentionDays': 0, 'DeploymentType': 'SINGLE_AZ_1', 'ThroughputCapacity': 64, 'PreferredSubnetId': 'subnet-...'}` — four members, no DiskIopsConfiguration. Because the member is missing on the source file system, it is also missing from the backup's embedded FileSystem and from any restore, so the whole backup-records-its-source-configuration behavior is unverifiable.

Why: DiskIopsConfiguration is one of the handful of fields that define an FSx file system's provisioned performance, and AWS guarantees it is always present once the file system exists. A client that reads `OpenZFSConfiguration["DiskIopsConfiguration"]["Mode"]` or `["Iops"]` — as any capacity-planning, drift-detection, cost-reporting or infrastructure-as-code reconciler does — gets a KeyError/missing-field crash rather than `AUTOMATIC`/`3 x StorageCapacity`. A reconciler that compares desired against actual state concludes the field is unset and tries to "correct" it with an UpdateFileSystem it never needed; a backup/restore workflow that verifies a restore inherited the source's IOPS configuration cannot make that check at all, because neither side reports the field.

Fix: the operation's state projection — `stored_open_zfs_configuration` in services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs must stop treating DiskIopsConfiguration as a pass-through of the request and instead materialize the documented default when the request omits it (Mode `AUTOMATIC`, Iops derived as 3 x the record's StorageCapacity), so that the stored record — and therefore DescribeFileSystems, the FileSystem embedded in CreateBackup/DescribeBackups, and CreateFileSystemFromBackup's restore — always carries it. A request that DOES supply a DiskIopsConfiguration keeps being honoured (Mode `USER_PROVISIONED` with the stated Iops). The sibling helpers for Windows and ONTAP in the same module share the identical pass-through and need the same default.

Traces: fsx.backup-restores-file-system-configuration.

## Evidence
Cause 1 — fsx.backup-restores-file-system-configuration [error]

Setup: a self-owned VPC, security group and subnet, then
  fsx.create_file_system(FileSystemType='OPENZFS', StorageCapacity=64, SubnetIds=[<subnet>],
                         SecurityGroupIds=[<sg>],
                         OpenZFSConfiguration={'DeploymentType':'SINGLE_AZ_1',
                                               'ThroughputCapacity':64,
                                               'AutomaticBackupRetentionDays':0,
                                               'RootVolumeConfiguration':{'DataCompressionType':'ZSTD',
                                                                          'RecordSizeKiB':128}})
  — note: no DiskIopsConfiguration is supplied. Polled to Lifecycle AVAILABLE, then
  fsx.create_backup(FileSystemId=<fs>) and polled describe_backups to Lifecycle AVAILABLE.

Expected (AWS): the source file system from DescribeFileSystems, and the FileSystem embedded in the
AVAILABLE backup, both carry
  OpenZFSConfiguration.DiskIopsConfiguration = {'Mode': 'AUTOMATIC', 'Iops': 3 * StorageCapacity}
('AUTOMATIC' is the fixed literal; the Iops value is computed from the file system's own stored
StorageCapacity — 192 for the 64 GiB file system in this setup, not a constant), so the configuration
projection {FileSystemType, StorageCapacity, StorageType, DeploymentType, ThroughputCapacity,
DiskIopsMode, DiskIops} is readable on both sides and compares equal.

Actual (emulator): OpenZFSConfiguration has no DiskIopsConfiguration member, so reading it raises:
  File "check_fsx_backup_restores_file_system_configuration.py", line 219, in run
    assert _openzfs_shape(embedded) == _openzfs_shape(source), (
  File "check_fsx_backup_restores_file_system_configuration.py", line 151, in _openzfs_shape
    "DiskIopsMode": cfg["DiskIopsConfiguration"]["Mode"],
  KeyError: 'DiskIopsConfiguration'

Corroborating response body from the same emulator for an OpenZFS file system created the same way
(the OpenZFSConfiguration this service projects onto every read of the record):
  'OpenZFSConfiguration': {'AutomaticBackupRetentionDays': 0, 'DeploymentType': 'SINGLE_AZ_1',
                           'ThroughputCapacity': 64, 'PreferredSubnetId': 'subnet-...'}
DiskIopsConfiguration is absent. (The ids, subnet names, host, timestamps and request ids in any
quoted body are per-run values, not requirements; the requirement is the member's presence and its
AUTOMATIC / 3-IOPS-per-GB content.)

Code: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs line 971 —
`disk_iops_configuration: requested.disk_iops_configuration.clone()` inside
`stored_open_zfs_configuration` (declared at line 959), reached from
services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/create_file_system.rs
lines 419-422, which map the request's OpenZFSConfiguration onto the stored record. An omitted
member is stored as None and the record projection serializes it as absent. The same pass-through
appears for Windows at state.rs line 881 and for ONTAP at state.rs line 940.

## Steps to Reproduce
1. Create a VPC (10.0.0.0/16), a security group admitting the file system's data-plane traffic, and a subnet in an available AZ.
2. fsx.create_file_system(FileSystemType='OPENZFS', StorageCapacity=64, SubnetIds=[<subnet>], SecurityGroupIds=[<sg>], OpenZFSConfiguration={'DeploymentType':'SINGLE_AZ_1','ThroughputCapacity':64,'AutomaticBackupRetentionDays':0,'RootVolumeConfiguration':{'DataCompressionType':'ZSTD','RecordSizeKiB':128}}) — deliberately supplying NO DiskIopsConfiguration.
3. Poll fsx.describe_file_systems(FileSystemIds=[<fs>]) until Lifecycle == 'AVAILABLE', then read FileSystems[0]['OpenZFSConfiguration']['DiskIopsConfiguration']. It must be present with Mode == 'AUTOMATIC' and Iops == 3 * StorageCapacity (192 for this 64 GiB file system); the emulator omits the member entirely, so the lookup raises KeyError.
4. fsx.create_backup(FileSystemId=<fs>) and poll fsx.describe_backups(BackupIds=[<backup>]) until Lifecycle == 'AVAILABLE'. Backups[0]['FileSystem']['OpenZFSConfiguration']['DiskIopsConfiguration'] must carry the same Mode and Iops as the source — the backup records what it was taken from.
5. fsx.create_file_system_from_backup(BackupId=<backup>, SubnetIds=[<subnet>], SecurityGroupIds=[<sg>]) with no OpenZFSConfiguration of its own, poll the new file system to AVAILABLE, and confirm its OpenZFSConfiguration.DiskIopsConfiguration equals the source's.
6. Repeat step 2 supplying OpenZFSConfiguration.DiskIopsConfiguration={'Mode':'USER_PROVISIONED','Iops':400} and confirm that value is reported back unchanged — the default must not overwrite an explicit request.
7. Replay: run_behavioral_suite and confirm fsx.backup-restores-file-system-configuration passes.
