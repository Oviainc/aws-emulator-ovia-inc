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
//!
//! That holds for EVERY instance this emulator stores, including one launched from an AMI
//! the account cannot see: the launch was served, so the instance exists and is EBS-backed,
//! and an instance with no root volume at all would be one no describe of volumes can
//! account for. [`ComAmazonawsEc2Amazonec2State::launched_root_device`] is the one place
//! that root device is resolved — off the image's own mapping where there is one, and off
//! the crate's default EBS-backed Linux image otherwise.

use std::time::{Instant, SystemTime};

use crate::ops::types::{AttachVolumeOutput, EbsBlockDevice};

use super::{
    ComAmazonawsEc2Amazonec2State, DEFAULT_GP3_IOPS, DEFAULT_GP3_THROUGHPUT, DEFAULT_VOLUME_TYPE,
    IMAGE_ROOT_DEVICE_TYPE_EBS, ImageSpec, MAX_STORED_VOLUMES, THROUGHPUT_VOLUME_TYPE,
    TRANSITION_DELAY, VOLUME_ATTACHMENT_STATE_ATTACHED, VOLUME_ID_PREFIX, VolumeRecord, VolumeView,
    system_time_to_date_time, unmanaged_operator, zone_id_for_zone_name,
};

impl ComAmazonawsEc2Amazonec2State {
    /// The device an instance launched from `image_id` holds its EBS ROOT volume at, and the
    /// block device facts that volume is restored from — or `None` for a launch that creates
    /// no root volume at all.
    ///
    /// The facts are the AMI's own root block device mapping whenever the account can SEE
    /// that AMI, which is the only reading of them: the device name is `Image$RootDeviceName`
    /// and the rest is the `EbsBlockDevice` the mapping at that device publishes. An AMI the
    /// account cannot see publishes no mapping to read, and the launch still stored an
    /// EBS-backed instance — "when you launch an instance, the root device volume […]
    /// contains the image used to boot the instance" holds for every instance, so one with no
    /// root volume at all would be an instance this emulator reports and no describe of
    /// volumes can account for. Such a launch is therefore given the root device of the
    /// EBS-backed Linux image this crate publishes by default ([`ImageSpec::default`], the row
    /// every published catalog image starts from), rather than a size and a type invented
    /// here; the volume restores no snapshot, because no image was read to name one.
    ///
    /// `None` is for an AMI the account CAN see that is not EBS-backed
    /// (`Image$RootDeviceType`) or publishes no mapping at its root device: an instance store
    /// root device is no volume, so there is nothing to snapshot and nothing to create.
    ///
    /// The ONE answer to "which device is this instance's root one", so the volume a launch
    /// mints ([`Self::mint_launched_root_volume`]) and the boot volume a snapshot call
    /// excludes cannot disagree.
    pub fn launched_root_device(&self, image_id: Option<&str>) -> Option<(String, EbsBlockDevice)> {
        let Some(image) = image_id.and_then(|image_id| self.images.get(image_id)) else {
            return Some(default_root_device());
        };
        // "Indicates whether the root device […] is an Amazon EBS volume or an instance
        // store volume" (`Image$RootDeviceType`): only an EBS-backed AMI restores its root
        // device into a volume.
        if image.root_device_type.as_deref() != Some(IMAGE_ROOT_DEVICE_TYPE_EBS) {
            return None;
        }
        let device_name = image.root_device_name.clone()?;
        let ebs = image
            .block_device_mappings
            .iter()
            .flatten()
            .find(|mapping| mapping.device_name.as_deref() == Some(device_name.as_str()))
            .and_then(|mapping| mapping.ebs.clone())?;
        Some((device_name, ebs))
    }

    /// Give the instance `instance_id` the EBS root volume its launch creates, restored from
    /// the root block device [`Self::launched_root_device`] resolves for it — see the module
    /// docs for why it is minted here.
    ///
    /// Everything about the volume comes off that block device — the size, the type, the
    /// encryption state and key, the snapshot it is restored from and whether it is deleted
    /// with the instance — and its zone is the instance's own. Nothing comes from the
    /// request, which is why no byte charge is made for it: the record holds no
    /// caller-chosen string (a launch's own `TagSpecifications` are charged by
    /// `RunInstances` for the resources it tags).
    ///
    /// Nothing is minted when the launch named no AMI at all, when the AMI the account can
    /// see is not EBS-backed or publishes no root device mapping (so there is no volume to
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
        let Some((device_name, ebs)) = self.launched_root_device(Some(&image_id)) else {
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

/// The root block device of a launch whose AMI the account cannot see: the device name, size
/// and volume type the crate's default EBS-backed Linux image publishes
/// ([`ImageSpec::default`], which every published catalog row starts from), kept with the
/// image's defaults rather than written out here so the volume a launch from an unseen AMI
/// gets and the volume a launch from a published one gets are the same kind of volume.
///
/// The device restores NO snapshot ("the snapshot from which the volume was created, if
/// applicable", `Volume$SnapshotId`): no image was read, so there is no root device snapshot
/// to name. It is unencrypted and deleted on termination, both the documented defaults for a
/// root device the request said nothing about.
fn default_root_device() -> (String, EbsBlockDevice) {
    let defaults = ImageSpec::default();
    (
        defaults.root_device_name,
        EbsBlockDevice {
            delete_on_termination: Some(true),
            snapshot_id: None,
            volume_size: Some(defaults.root_volume_size),
            volume_type: Some(defaults.root_volume_type),
            encrypted: Some(false),
            iops: None,
            kms_key_id: None,
            throughput: None,
            outpost_arn: None,
            availability_zone: None,
            volume_initialization_rate: None,
            availability_zone_id: None,
            ebs_card_index: None,
        },
    )
}
