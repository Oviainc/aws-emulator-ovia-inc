// SPDX-License-Identifier: CC0-1.0

//! The image catalog this service's store holds, and the record form it is written in.
//!
//! An AppStream image is either one AWS publishes for every account to launch fleets and
//! image builders from — a base image — or a private one an account mints in its own
//! Region (an image builder's snapshot, an imported image, a copy, a managed update). No
//! account creates a base image, so the published ones are SEEDED into this service's
//! store ([`seed_image_catalog`]) and only read from, while every image an operation of
//! this API mints is written into the SAME map: `DescribeImages` reads one catalog, so a
//! private image and a published one are addressed, filtered and paged by exactly the
//! same rules.
//!
//! The shipped table below is a fixed snapshot of the base-image families AWS publishes,
//! written with AWS's own base-image naming (`AppStream-<operating system>-<MM-DD-YYYY>`,
//! the release date of the published image). One row per operating system this API models
//! an image platform for; the platform tokens are `PlatformType`'s
//! (`PLATFORM_TYPE_VALUES` in `ops/types/shared_types.rs`), and the supported instance
//! families are the ones the model's `Image$SupportedInstanceFamilies` doc enumerates
//! (`smithy_query(query="doc", shape="Image", member="SupportedInstanceFamilies")`:
//! General Purpose, Compute Optimized, Memory Optimized, Graphics G4, Graphics G5,
//! Graphics G6).
//!
//! A row carries only what the published image states: the members an account's own
//! activity fills — the image builder that produced it, the base image it was created
//! from, its applications, its sharing permissions, its errors — are ABSENT on a base
//! image, and a private image record fills them.

use std::collections::BTreeMap;

use crate::ops::types::{
    Application, Image, ImagePermissions, ImageSoftwareMetadata, ImageStateChangeReason,
    ResourceError,
};

use super::{
    arn_resource, reported, reported_text, ComAmazonawsAppstreamPhotonadminproxyserviceState,
    ARN_NAMESPACE,
};

/// The ARN `resource-type` segment of an image, the segment before the image's name in
/// `arn:aws:appstream:<region>:<account>:image/<name>`.
pub const IMAGE_ARN_RESOURCE_TYPE: &str = "image";

/// The word AWS's refusal sentences use for an image, the `<kind>` slot of
/// [`super::resource_not_found_message`].
pub const IMAGE_KIND: &str = "image";

/// The `ImageState` an image that can be launched from carries
/// (`IMAGE_STATE_VALUES` in `ops/types/shared_types.rs`). The model's `Image$State` doc:
/// "The image starts in the `PENDING` state. If image creation succeeds, the state is
/// `AVAILABLE`." A published base image was created long before any request reaches this
/// emulator, so it is a terminal `AVAILABLE` the clock owes no transition out of.
pub const IMAGE_STATE_AVAILABLE: &str = "AVAILABLE";

/// The `VisibilityType` of an image AWS publishes to every account
/// (`VISIBILITY_TYPE_VALUES` in `ops/types/shared_types.rs`).
pub const VISIBILITY_PUBLIC: &str = "PUBLIC";

/// The `ImageType` of an AWS-published image. The model's `Image$ImageType` doc:
/// "Images created through AMI import have type \"custom\", while WorkSpaces Applications
/// provided images have type \"native\"" (`IMAGE_TYPE_VALUES` in
/// `ops/types/shared_types.rs`).
pub const IMAGE_TYPE_NATIVE: &str = "NATIVE";

/// How many images one `DescribeImages` page carries.
///
/// The operation's `MaxResults` is `DescribeImagesMaxResults`, which the model bounds at
/// 25 (`smithy_query(query="operation", operation="DescribeImages")`: `range 0..25`), so
/// no page of this read is larger than that whatever a request asks for.
pub const DESCRIBE_IMAGES_PAGE_SIZE: usize = 25;

/// `arn:aws:appstream:<region>:<account>:image/<name>`.
pub fn image_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{IMAGE_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// An RFC 3339 instant as the modelled timestamp; the epoch for text that is not one, so
/// a mis-written table row cannot panic a read.
fn instant(rfc3339: &str) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_str(
        rfc3339,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|_| ::aws_smithy_types::DateTime::from_secs(0))
}

/// A stored AppStream image: a published base image the catalog ships, or one an
/// operation of this API minted for this account.
#[derive(Debug, Clone)]
pub struct ImageRecord {
    pub name: String,
    /// The ARN of an image minted in one Region, and `None` for a published base image.
    ///
    /// AWS publishes the same base images in every Region, each addressed by that
    /// Region's own ARN, so a base image has no one stored ARN: it reports the ARN of the
    /// Region the request addresses ([`ImageRecord::arn`]). An image this account minted
    /// belongs to the Region it was created in, so it carries the ARN the create stamped.
    pub arn: Option<String>,
    pub base_image_arn: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// One of `IMAGE_STATE_VALUES`.
    pub state: String,
    /// One of `VISIBILITY_TYPE_VALUES`.
    pub visibility: String,
    pub image_builder_supported: bool,
    pub image_builder_name: Option<String>,
    /// One of `PLATFORM_TYPE_VALUES`.
    pub platform: String,
    pub state_change_reason: Option<ImageStateChangeReason>,
    /// The applications the image carries; empty for an image that advertises none.
    pub applications: Vec<Application>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub public_base_image_released_date: Option<::aws_smithy_types::DateTime>,
    pub appstream_agent_version: Option<String>,
    pub image_permissions: Option<ImagePermissions>,
    /// The errors of the image; empty for one nothing has gone wrong with.
    pub image_errors: Vec<ResourceError>,
    /// One of `LATEST_APPSTREAM_AGENT_VERSION_VALUES`.
    pub latest_appstream_agent_version: Option<String>,
    /// The instance families a fleet or image builder may launch this image on; empty for
    /// an image that states none.
    pub supported_instance_families: Vec<String>,
    /// One of `DYNAMIC_APP_PROVIDERS_ENABLED_VALUES`.
    pub dynamic_app_providers_enabled: Option<String>,
    /// One of `IMAGE_SHARED_WITH_OTHERS_VALUES`.
    pub image_shared_with_others: Option<String>,
    pub managed_software_included: Option<bool>,
    /// One of `IMAGE_TYPE_VALUES`.
    pub image_type: Option<String>,
    pub image_software_metadata: Option<ImageSoftwareMetadata>,
}

impl ImageRecord {
    /// The ARN this image reports to a request addressing `region`: the one its create
    /// stamped, or — for a published base image, which every Region carries — the ARN of
    /// the Region asked about.
    pub fn arn(&self, region: &str) -> String {
        self.arn
            .clone()
            .unwrap_or_else(|| image_arn(region, &self.name))
    }

    /// Render the stored record as the modelled `Image` every reader returns.
    pub fn to_image(&self, region: &str) -> Image {
        Image {
            name: self.name.clone(),
            arn: Some(self.arn(region)),
            base_image_arn: self.base_image_arn.clone(),
            display_name: reported_text(self.display_name.as_deref()),
            state: Some(self.state.clone()),
            visibility: Some(self.visibility.clone()),
            image_builder_supported: Some(self.image_builder_supported),
            image_builder_name: self.image_builder_name.clone(),
            platform: Some(self.platform.clone()),
            description: reported_text(self.description.as_deref()),
            state_change_reason: self.state_change_reason.clone(),
            // A collection with no entries is something the image does not have, so the
            // member is ABSENT rather than present-and-empty — the same representation
            // every other resource of this service renders its collections with.
            applications: reported(&self.applications),
            created_time: Some(self.created_time),
            public_base_image_released_date: self.public_base_image_released_date,
            appstream_agent_version: self.appstream_agent_version.clone(),
            image_permissions: self.image_permissions.clone(),
            image_errors: reported(&self.image_errors),
            latest_appstream_agent_version: self.latest_appstream_agent_version.clone(),
            supported_instance_families: reported(&self.supported_instance_families),
            dynamic_app_providers_enabled: self.dynamic_app_providers_enabled.clone(),
            image_shared_with_others: self.image_shared_with_others.clone(),
            managed_software_included: self.managed_software_included,
            image_type: self.image_type.clone(),
            image_software_metadata: self.image_software_metadata.clone(),
        }
    }
}

/// One row of the shipped base-image table, as the published image states it.
///
/// Held as `&'static str` data rather than as built records so the table reads as the
/// catalog it transcribes; [`seed_image_catalog`] projects each row onto an
/// [`ImageRecord`].
#[derive(Debug, Clone, Copy)]
struct PublishedImage {
    /// The image name a fleet or image builder names the image by.
    name: &'static str,
    /// The name to display for it.
    display_name: &'static str,
    /// What the image is.
    description: &'static str,
    /// The image's operating system, one of `PLATFORM_TYPE_VALUES`.
    platform: &'static str,
    /// The day AWS released this base image, RFC 3339 — the `PublicBaseImageReleasedDate`
    /// the image reports, and the date its name carries.
    released: &'static str,
    /// The instance families a fleet or image builder may launch it on.
    instance_families: &'static [&'static str],
}

/// The instance families a general-purpose base image supports: the non-graphics families
/// the model's `Image$SupportedInstanceFamilies` doc enumerates.
const GENERAL_INSTANCE_FAMILIES: &[&str] =
    &["General Purpose", "Compute Optimized", "Memory Optimized"];

/// The instance families a graphics base image supports: the accelerated families of that
/// same enumeration.
const GRAPHICS_INSTANCE_FAMILIES: &[&str] = &["Graphics G4", "Graphics G5", "Graphics G6"];

/// The base images this emulator publishes to every account, one per operating system the
/// model declares an image platform for, plus one graphics image — so a client choosing an
/// image sees the whole shape of the published catalog rather than a single row.
///
/// Sorted by name, which is also the order a page of `DescribeImages` walks them in.
const PUBLISHED_IMAGES: &[PublishedImage] = &[
    PublishedImage {
        name: "AppStream-AmazonLinux2-06-17-2024",
        display_name: "Amazon Linux 2",
        description: "AppStream 2.0 base image for Amazon Linux 2",
        platform: "AMAZON_LINUX2",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-Graphics-WinServer2019-06-17-2024",
        display_name: "Windows Server 2019 Graphics",
        description: "AppStream 2.0 graphics base image for Windows Server 2019",
        platform: "WINDOWS_SERVER_2019",
        released: "2024-06-17T00:00:00Z",
        instance_families: GRAPHICS_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-RHEL8-06-17-2024",
        display_name: "Red Hat Enterprise Linux 8",
        description: "AppStream 2.0 base image for Red Hat Enterprise Linux 8",
        platform: "RHEL8",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-RockyLinux8-06-17-2024",
        display_name: "Rocky Linux 8",
        description: "AppStream 2.0 base image for Rocky Linux 8",
        platform: "ROCKY_LINUX8",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-UbuntuPro2404-06-17-2024",
        display_name: "Ubuntu Pro 24.04",
        description: "AppStream 2.0 base image for Ubuntu Pro 24.04",
        platform: "UBUNTU_PRO_2404",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-WinServer2019-06-17-2024",
        display_name: "Windows Server 2019",
        description: "AppStream 2.0 base image for Windows Server 2019",
        platform: "WINDOWS_SERVER_2019",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-WinServer2022-06-17-2024",
        display_name: "Windows Server 2022",
        description: "AppStream 2.0 base image for Windows Server 2022",
        platform: "WINDOWS_SERVER_2022",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
    PublishedImage {
        name: "AppStream-WinServer2025-06-17-2024",
        display_name: "Windows Server 2025",
        description: "AppStream 2.0 base image for Windows Server 2025",
        platform: "WINDOWS_SERVER_2025",
        released: "2024-06-17T00:00:00Z",
        instance_families: GENERAL_INSTANCE_FAMILIES,
    },
];

/// The image catalog a fresh store starts from: one record per published base image,
/// keyed by the name a fleet or image builder names it by.
///
/// BOUNDED by the shipped table: this seed adds no record per request, and an operation
/// that mints an image checks the map against
/// [`super::MAX_RECORDS_PER_STORE`] before inserting.
pub fn seed_image_catalog() -> BTreeMap<String, ImageRecord> {
    PUBLISHED_IMAGES
        .iter()
        .map(|published| {
            let released = instant(published.released);
            let record = ImageRecord {
                name: published.name.to_string(),
                // Every Region carries the published image, so its ARN is the one of the
                // Region a request addresses rather than a stored one.
                arn: None,
                base_image_arn: None,
                display_name: Some(published.display_name.to_string()),
                description: Some(published.description.to_string()),
                state: IMAGE_STATE_AVAILABLE.to_string(),
                visibility: VISIBILITY_PUBLIC.to_string(),
                // "Indicates whether an image builder can be launched from this image":
                // a published base image is what an image builder starts from.
                image_builder_supported: true,
                // No image builder produced it, and nothing has gone wrong with it.
                image_builder_name: None,
                platform: published.platform.to_string(),
                state_change_reason: None,
                applications: Vec::new(),
                // A published image was created when AWS released it.
                created_time: released,
                public_base_image_released_date: Some(released),
                appstream_agent_version: None,
                // Sharing permissions belong to an account's own private image.
                image_permissions: None,
                image_errors: Vec::new(),
                latest_appstream_agent_version: None,
                supported_instance_families: published
                    .instance_families
                    .iter()
                    .map(|family| (*family).to_string())
                    .collect(),
                dynamic_app_providers_enabled: None,
                image_shared_with_others: None,
                managed_software_included: None,
                image_type: Some(IMAGE_TYPE_NATIVE.to_string()),
                image_software_metadata: None,
            };
            (record.name.clone(), record)
        })
        .collect()
}

impl ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// The image stored under the ARN asked about, as the Region `region` addresses it;
    /// `None` when this service holds none under it.
    ///
    /// An image is addressed by name or by ARN, and a stored ARN must match in full, so an
    /// ARN naming another Region's or another account's resource addresses no image here.
    /// The name in the ARN is what the record is keyed by, so the lookup is one map probe
    /// rather than a scan.
    pub fn image_by_arn(&self, region: &str, arn: &str) -> Option<&ImageRecord> {
        let (resource_type, name) = arn_resource(arn);
        (resource_type == IMAGE_ARN_RESOURCE_TYPE)
            .then(|| self.images.get(&name))
            .flatten()
            .filter(|record| record.arn(region) == arn)
    }
}
