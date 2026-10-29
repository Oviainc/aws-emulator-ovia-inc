Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 99c19747b30de4fba88ebdfb7af58fcb882c179f

# CreateCloudFormationStack accepts an export snapshot record of a DISK snapshot, so a stack record claims a new EC2 instance is built from an EBS volume

_Filed: 2026-10-29T10:17:58Z_
_Operation: CreateCloudFormationStack | Class: validation_

> _Note: this issue was identified against the code base at commit `99c19747b30de4fba88ebdfb7af58fcb882c179f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCloudFormationStack` resolves `instances[].sourceName` to an export snapshot record and checks three things about it — that the account holds it, that its export has finished (`Succeeded`), and that its Region is the Region the new instance's Availability Zone is in. It never checks WHICH KIND of snapshot that record exported, although the record it just resolved carries that fact in `source_info.resourceType` (`InstanceSnapshot` or `DiskSnapshot`), and although the crate itself computes the export's Amazon EC2 destination from exactly that distinction: an exported instance snapshot becomes an AMI (`ami-…`), an exported disk snapshot becomes an EBS volume (`vol-…`).

The result is a stack record that describes work that cannot be done: the handler stores a `CloudFormationStackRecord` whose `sourceInfo` names an export record whose only destination is an EBS volume, while the record's whole purpose is to track "a new Amazon EC2 instance" created from the source. The model states what the member must name: the export snapshot record "contains the exported Lightsail instance snapshot that will be used as the source of the new Amazon EC2 instance". A disk-snapshot export contains no instance snapshot and produces no machine image, so there is nothing for the stack to launch an instance from — yet the call answers 200, the record settles to `Succeeded`, and `GetCloudFormationStackRecords` reports a stack that "created" an EC2 instance from a volume.

The handler's own comment enumerates the checks it means to apply and the snapshot KIND is absent from that list, so the gap is in the enumeration, not in a deliberate decision recorded anywhere.

## Evidence

The modelled contract — smithy_query(query="doc", shape="InstanceEntry", member="sourceName"):
```
The name of the export snapshot record, which contains the exported Lightsail instance
snapshot that will be used as the source of the new Amazon EC2 instance.
```

The checks the handler actually applies, as its own comment enumerates them — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_cloud_formation_stack.rs:177-183:
```
        // what the model cannot express: one entry per call, an Amazon EC2
        // instance type, a launch script Amazon EC2 accepts, a zone a served
        // Region publishes, and a source that is an export snapshot record of
        // this account whose export has finished and whose region the new
        // instance is created in. Every one of those is checked BEFORE the first
        // write, so a refused request creates nothing.
```
and the code that implements them — src/ops/create_cloud_formation_stack.rs:240-262 (existence, `RECORD_STATE_SUCCEEDED`, region) followed by src/ops/create_cloud_formation_stack.rs:263-272, which reads the resolved record but takes only its name and ARN:
```
            let source_info = CloudFormationStackRecordSourceInfo {
                // The one member the modelled
                // `CloudFormationStackRecordSourceType` enum carries: the source
                // of a stack record is an export snapshot record.
                resource_type: Some(
                    crate::state::RESOURCE_TYPE_EXPORT_SNAPSHOT_RECORD.to_string(),
                ),
                name: Some(source.name.clone()),
                arn: Some(source.arn.clone()),
            };
```
Nothing reads `source.source_info.resource_type`.

The crate DOES own the instance-vs-disk distinction on that very record — src/ops/export_snapshot.rs:291-310 (disk snapshot):
```
            source_info: ExportSnapshotRecordSourceInfo {
                // The modelled `ExportSnapshotRecordSourceType` value of a disk
                // snapshot, the same `DiskSnapshot` the snapshot's own
                // `resourceType` carries.
                resource_type: Some(crate::state::RESOURCE_TYPE_DISK_SNAPSHOT.to_string()),
...
            destination_id_prefix: crate::state::EC2_VOLUME_ID_PREFIX,
```
and src/ops/export_snapshot.rs:315-333 (instance snapshot) ends with `destination_id_prefix: crate::state::EC2_IMAGE_ID_PREFIX,` — the two prefixes being, src/state.rs:6068-6077:
```
/// The id prefix of the Amazon EC2 resource an exported INSTANCE snapshot
/// becomes, from the modelled `ExportSnapshot` documentation: "Exported instance
/// snapshots appear in Amazon EC2 as Amazon Machine Images (AMIs)". See
/// [`ec2_destination_id`].
pub const EC2_IMAGE_ID_PREFIX: &str = "ami-";

/// The id prefix of the Amazon EC2 resource an exported DISK snapshot becomes,
/// from the same modelled documentation: "Exported disk snapshots appear in
/// Amazon EC2 as Amazon EBS volumes". See [`ec2_destination_id`].
pub const EC2_VOLUME_ID_PREFIX: &str = "vol-";
```

Test coverage: the current behaviour IS exercised, and the reason the test states is that the rule "generalises over" the snapshot kind — tests/all/cloud_formation_stack.rs:244-246:
```
/// The rows differ along everything the rule generalises over: the KIND of
/// resource the exported snapshot was taken of, the Availability Zone (and so the
/// region in every ARN and in the location), the Amazon EC2 instance type's form
```
That premise is what the modelled `sourceName` sentence contradicts. Note also that EVERY row of the table creates its snapshot with `CreateDiskSnapshot` — tests/all/cloud_formation_stack.rs:374-379:
```
async fn create_snapshot(emulator: &Emulator, row: &Row) {
    create_source(emulator, row).await;
    let mut body = json!({"diskSnapshotName": row.snapshot_name});
    match row.source {
        Source::Disk { .. } => body["diskName"] = json!(row.source_name),
        Source::Instance { .. } => body["instanceName"] = json!(row.source_name),
```
so `Source::Instance` is an instance's SYSTEM VOLUME disk snapshot (tests/all/cloud_formation_stack.rs:210-215: "An instance's system volume"), and the table contains no instance-snapshot export at all: the only source kind the operation has ever been exercised with is the one the model says it cannot take.

## Steps to Reproduce

1. Call CreateDisk with diskName "data-disk", availabilityZone "us-east-1a", sizeInGb 32; wait for it to settle to available.
2. Call CreateDiskSnapshot with diskName "data-disk" and diskSnapshotName "data-disk-snapshot"; wait for the snapshot to complete.
3. Call ExportSnapshot with sourceSnapshotName "data-disk-snapshot" and note the minted record name (ExportSnapshot-…); wait for the record to reach Succeeded.
4. Call GetExportSnapshotRecords: the record reports sourceInfo.resourceType "DiskSnapshot" and destinationInfo.id "vol-…" (src/state.rs:6077, src/ops/export_snapshot.rs:310) — the export produced an EBS volume, not a machine image.
5. Call CreateCloudFormationStack with instances=[{"sourceName": "<that record name>", "instanceType": "t2.micro", "portInfoSource": "DEFAULT", "availabilityZone": "us-east-1a"}].
6. Observe HTTP 200 and a CloudFormation stack record created (src/ops/create_cloud_formation_stack.rs:314-332), which GetCloudFormationStackRecords then reports as having created an Amazon EC2 instance from an export whose only destination resource is an EBS volume — although the modelled sourceName says the record must contain "the exported Lightsail instance snapshot that will be used as the source of the new Amazon EC2 instance".
