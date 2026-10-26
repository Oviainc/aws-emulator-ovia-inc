// SPDX-License-Identifier: CC0-1.0

//! The EBS root volume a LAUNCH creates for the instance it stores.
//!
//! "When you launch an instance, the root device volume […] contains the image used to boot
//! the instance": launching from an EBS-backed AMI creates a volume, restored from the
//! snapshot that image's root block device mapping names (`Image$BlockDeviceMappings`,
//! `Image$RootDeviceName`, `Image$RootDeviceType`'s `ebs`). So the volume is a resource the
//! LAUNCH creates, which is why it is minted from
//! [`ComAmazonawsEc2Amazonec2State::insert_instance`] — the one place every launch path
//! stores its instance — rather than inside one operation's handler: the account's volume
//! store is then the single place "which volumes does this instance have" is answered, for
//! `DescribeVolumes` and for `CreateSnapshots` alike.

use std::time::{Instant, SystemTime};

use crate::ops::types::AttachVolumeOutput;

use super::{
    ComAmazonawsEc2Amazonec2State, DEFAULT_GP3_IOPS, DEFAULT_GP3_THROUGHPUT, DEFAULT_VOLUME_TYPE,
    IMAGE_ROOT_DEVICE_TYPE_EBS, MAX_STORED_VOLUMES, THROUGHPUT_VOLUME_TYPE, TRANSITION_DELAY,
    VOLUME_ATTACHMENT_STATE_ATTACHED, VOLUME_ID_PREFIX, VolumeRecord, VolumeView,
    system_time_to_date_time, unmanaged_operator, zone_id_for_zone_name,
};

impl ComAmazonawsEc2Amazonec2State {
    /// Give the instance `instance_id` the EBS root volume its launch creates, restored from
    /// its AMI's root block device mapping — see the module docs for why it is minted here.
    ///
    /// Everything about the volume comes off the image's mapping — the size, the type, the
    /// encryption state and key, the snapshot it is restored from and whether it is deleted
    /// with the instance — and its zone is the instance's own. Nothing comes from the
    /// request, which is why no byte charge is made for it: the record holds no
    /// caller-chosen string (a launch's own `TagSpecifications` are charged by
    /// `RunInstances` for the resources it tags).
    ///
    /// Nothing is minted when the launch named an AMI this store cannot see, when that AMI
    /// is not EBS-backed or publishes no root device mapping (so there is no volume to
    /// restore), or when the volume store is already full ([`MAX_STORED_VOLUMES`]) — the
    /// instance is still stored in each case, exactly as it was before.
    pub(super) fn mint_launched_root_volume(&mut self, instance_id: &str) {
        let Some(instance) = self.instances.get(instance_id) else {
            return;
        };
        let zone_name = instance.placement_az.clone();
        let Some(image_id) = instance.image_id.clone() else {
            return;
        };
        let Some(image) = self.images.get(&image_id) else {
            return;
        };
        // "Indicates whether the root device […] is an Amazon EBS volume or an instance
        // store volume" (`Image$RootDeviceType`): only an EBS-backed AMI restores its root
        // device into a volume.
        if image.root_device_type.as_deref() != Some(IMAGE_ROOT_DEVICE_TYPE_EBS) {
            return;
        }
        let Some(device_name) = image.root_device_name.clone() else {
            return;
        };
        let Some(ebs) = image
            .block_device_mappings
            .iter()
            .flatten()
            .find(|mapping| mapping.device_name.as_deref() == Some(device_name.as_str()))
            .and_then(|mapping| mapping.ebs.clone())
        else {
            return;
        };
        if self.volumes.len() >= MAX_STORED_VOLUMES {
            return;
        }

        let volume_id = self.next_id(VOLUME_ID_PREFIX);
        // The Region the volume's ARN names is the one its Availability Zone belongs to —
        // read off the zone name the way [`zone_id_for_zone_name`] reads the zone's id, so
        // the volume and the instance that holds it can never be placed in two Regions.
        let region = zone_name
            .strip_suffix(|letter: char| letter.is_ascii_lowercase())
            .unwrap_or(zone_name.as_str())
            .to_string();
        let volume_type = ebs
            .volume_type
            .clone()
            .unwrap_or_else(|| DEFAULT_VOLUME_TYPE.to_string());
        let now = SystemTime::now();
        let view = VolumeView {
            volume_id: Some(volume_id.clone()),
            volume_arn: Some(::engine::emulator_arn(
                "ec2",
                &region,
                &format!("volume/{volume_id}"),
            )),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            availability_zone: Some(zone_name.clone()),
            availability_zone_id: zone_id_for_zone_name(&zone_name),
            create_time: Some(system_time_to_date_time(now)),
            size: ebs.volume_size,
            // "The number of I/O operations per second (IOPS)" and "the throughput that the
            // volume supports": the mapping's own, or — for the ONE type AWS documents a
            // default for, "gp3: 3,000 (default)" and the bottom of the gp3 throughput
            // range — that default where the mapping names none. A mapping of a type that
            // REQUIRES provisioned IOPS and names none is left reporting none rather than
            // given the gp3 figure, which is a different type's documented default.
            iops: ebs.iops.or(if volume_type == THROUGHPUT_VOLUME_TYPE {
                Some(DEFAULT_GP3_IOPS)
            } else {
                None
            }),
            throughput: ebs.throughput.or(if volume_type == THROUGHPUT_VOLUME_TYPE {
                Some(DEFAULT_GP3_THROUGHPUT)
            } else {
                None
            }),
            volume_type: Some(volume_type),
            // Multi-Attach is asked for at creation and a launch asks for none.
            multi_attach_enabled: Some(false),
            encrypted: Some(ebs.encrypted.unwrap_or(false)),
            kms_key_id: ebs.kms_key_id.clone(),
            // A launch tags the volumes its own `TagSpecifications` name; nothing tags this
            // record here.
            tags: Some(Vec::new()),
            // "The snapshot from which the volume was created, if applicable"
            // (`Volume$SnapshotId`): the root device snapshot of the AMI the instance was
            // launched from, which is exactly what this volume restores.
            snapshot_id: ebs.snapshot_id.clone(),
            source_volume_id: None,
            // "Indicates whether the volume was created using fast snapshot restore"
            // (`Volume$FastRestored`): nothing enabled fast snapshot restore for the
            // image's snapshot, so it was not.
            fast_restored: Some(false),
            sse_type: None,
            operator: Some(unmanaged_operator()),
            outpost_arn: None,
            volume_initialization_rate: None,
            // Derived from `available_at` on every read.
            state: None,
            // "The device name" the volume is attached at is the image's root device, and
            // the instance holds it from the moment it is launched.
            attachments: Some(vec![AttachVolumeOutput {
                volume_id: Some(volume_id.clone()),
                instance_id: Some(instance_id.to_string()),
                device: Some(device_name),
                state: Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
                attach_time: Some(system_time_to_date_time(now)),
                // "Indicates whether the volume is deleted on instance termination"
                // (`EbsBlockDevice$DeleteOnTermination`), whose own documented default for
                // a root volume is `true`.
                delete_on_termination: Some(ebs.delete_on_termination.unwrap_or(true)),
                // An instance's own attachment, not one to an Amazon Web
                // Services-managed resource.
                associated_resource: None,
                instance_owning_service: None,
                ebs_card_index: Some(0),
            }]),
        };
        self.volumes.insert(
            volume_id,
            // The same clock-owed `creating` -> `available` span every volume this store
            // mints has, so a launch's volume and a `CreateVolume` volume are reported the
            // same way.
            VolumeRecord::created(view, Instant::now() + TRANSITION_DELAY),
        );
    }
}
