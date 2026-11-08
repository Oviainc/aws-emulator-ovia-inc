// SPDX-License-Identifier: CC0-1.0

//! The image METADATA hierarchies AWS publishes in Parameter Store beside an image's id.
//!
//! An AMI alias publishes ONE value — the id of the image it names
//! ([`super::published_amazon_linux`]) — but a service that recommends an image for its own
//! workload publishes a HIERARCHY about it: the recommended ECS-optimized AMI and the
//! recommended Bottlerocket build are each published as a path whose `image_id` leaf names
//! the image and whose sibling leaves carry the facts a client reads beside it (the image's
//! name and build version, the operating system it is built on, and the container-runtime
//! versions it ships). A client reads the hierarchy, not one name: that is how an ECS
//! container instance or a Bottlerocket node group picks an image without naming a
//! per-Region id.
//!
//! One source of truth, as for the aliases: the `image_id` leaf is the
//! `Image$PublicSsmParameterName` of an image of the published catalog, so the id a read
//! reports is an AMI the account can really describe and launch, and the `image_name` leaf is
//! that image's own `Image$Name` rather than a second copy of it. Only the facts no image
//! attribute holds are written in the table below, and they are service-published data that
//! no model describes: recorded behaviour.
//!
//! Nothing here writes: [`ComAmazonawsEc2Amazonec2State::public_ssm_parameter_values`]
//! projects the hierarchies from the image catalogue on each read, so a hierarchy whose image
//! has been deregistered resolves to nothing, exactly as the describe reports none.

use std::collections::BTreeMap;

use super::{
    published_amazon_image, ComAmazonawsEc2Amazonec2State, ImageSpec, BOOT_MODE_UEFI,
    BOOT_MODE_UEFI_PREFERRED, IMDS_SUPPORT_V2,
};

/// What a public Systems Manager parameter of the published catalog resolves to.
#[derive(Debug, Clone)]
pub struct PublicSsmParameterValue {
    /// The value the parameter carries.
    pub value: String,
    /// The `CreationDate` of the image the parameter is published beside — the instant that
    /// publication last changed. `None` for an image that reports none.
    pub published_at: Option<String>,
}

/// The leaf an image's id is published under in a metadata hierarchy, and the one the
/// hierarchy is reached through: it is the `Image$PublicSsmParameterName` of the image the
/// hierarchy describes.
const IMAGE_ID_LEAF: &str = "image_id";

/// The leaf the image's own name is published under.
const IMAGE_NAME_LEAF: &str = "image_name";

/// One published image-metadata hierarchy.
struct ImageMetadataHierarchy {
    /// The path the hierarchy is published at.
    path: &'static str,
    /// Whether it publishes the image's own name beside its id.
    publishes_image_name: bool,
    /// Every other leaf, as `(leaf, value)`: the facts published about the image that no
    /// image attribute holds.
    facts: &'static [(&'static str, &'static str)],
}

impl ImageMetadataHierarchy {
    /// The public parameter the hierarchy's image is published under.
    fn image_id_parameter(&self) -> String {
        format!("{}/{IMAGE_ID_LEAF}", self.path)
    }
}

/// The release of the recommended ECS-optimized Amazon Linux 2023 AMIs: the version their
/// names carry, and the `image_version` the hierarchy publishes, which is its build date.
const ECS_AL2023_RELEASE: &str = "2023.8.20251104";

/// See [`ECS_AL2023_RELEASE`].
const ECS_AL2023_IMAGE_VERSION: &str = "20251104";

/// The `CreationDate` of those images.
const ECS_AL2023_CREATION_DATE: &str = "2025-11-04T07:11:29.000Z";

/// The name of the recommended ECS-optimized Amazon Linux 2023 AMI for 64-bit x86.
const ECS_AL2023_X86_64_IMAGE: &str = "al2023-ami-ecs-hvm-2023.8.20251104-kernel-6.18-x86_64";

/// The same for 64-bit Arm.
const ECS_AL2023_ARM64_IMAGE: &str = "al2023-ami-ecs-arm64-2023.8.20251104-kernel-6.18-arm64";

/// The release of the recommended Bottlerocket builds, carried by their names as well as by
/// the `image_version` the hierarchies publish.
const BOTTLEROCKET_VERSION: &str = "1.46.0";

/// The `CreationDate` of those images.
const BOTTLEROCKET_CREATION_DATE: &str = "2025-10-28T09:03:51.000Z";

/// The name of the recommended Bottlerocket build for Amazon ECS on 64-bit x86. A
/// Bottlerocket build's name carries its variant, its architecture, its version and the
/// commit it was built from.
const BOTTLEROCKET_ECS_X86_64_IMAGE: &str = "bottlerocket-aws-ecs-2-x86_64-v1.46.0-7f7f0a6c";

/// The same for 64-bit Arm, whose name spells the architecture the way the project does.
const BOTTLEROCKET_ECS_ARM64_IMAGE: &str = "bottlerocket-aws-ecs-2-aarch64-v1.46.0-7f7f0a6c";

/// The container-runtime facts the ECS-optimized hierarchies publish beside their image:
/// the agent and the runtime the build ships, and the schema the hierarchy itself is
/// published under.
const ECS_AGENT_VERSION: &str = "1.97.0";

/// See [`ECS_AGENT_VERSION`].
const ECS_RUNTIME_VERSION: &str = "Docker version 25.0.8";

/// See [`ECS_AGENT_VERSION`].
const ECS_SCHEMA_VERSION: &str = "1";

/// The operating system the ECS-optimized Amazon Linux 2023 builds are built on, as the
/// hierarchy publishes it.
const ECS_AL2023_OS: &str = "Amazon Linux 2023";

/// Every image-metadata hierarchy the catalog publishes.
///
/// The ECS-optimized rows publish the image's name and build version, the operating system
/// it is built on, the base AMI it was built from, the two container-runtime versions it
/// ships and the schema version of the hierarchy itself; the Bottlerocket rows publish the
/// build's version beside its id, which is what that project publishes.
const PUBLISHED_IMAGE_METADATA: &[ImageMetadataHierarchy] = &[
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended",
        publishes_image_name: true,
        facts: &[
            ("ecs_agent_version", ECS_AGENT_VERSION),
            ("ecs_runtime_version", ECS_RUNTIME_VERSION),
            ("image_version", ECS_AL2023_IMAGE_VERSION),
            ("os", ECS_AL2023_OS),
            ("schema_version", ECS_SCHEMA_VERSION),
            (
                "source_image_name",
                "al2023-ami-2023.8.20251104.0-kernel-6.18-x86_64",
            ),
        ],
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/recommended",
        publishes_image_name: true,
        facts: &[
            ("ecs_agent_version", ECS_AGENT_VERSION),
            ("ecs_runtime_version", ECS_RUNTIME_VERSION),
            ("image_version", ECS_AL2023_IMAGE_VERSION),
            ("os", ECS_AL2023_OS),
            ("schema_version", ECS_SCHEMA_VERSION),
            (
                "source_image_name",
                "al2023-ami-2023.8.20251104.0-kernel-6.18-arm64",
            ),
        ],
    },
    ImageMetadataHierarchy {
        path: "/aws/service/bottlerocket/aws-ecs-2/x86_64/latest",
        publishes_image_name: false,
        facts: &[("image_version", BOTTLEROCKET_VERSION)],
    },
    ImageMetadataHierarchy {
        path: "/aws/service/bottlerocket/aws-ecs-2/arm64/latest",
        publishes_image_name: false,
        facts: &[("image_version", BOTTLEROCKET_VERSION)],
    },
];

/// The images those hierarchies describe, as rows of the published catalog
/// ([`super::published_image_specs`]): each carries the `image_id` leaf of its own hierarchy
/// as its `Image$PublicSsmParameterName`, which is what makes the id a read reports an AMI of
/// this catalog.
pub fn published_image_metadata_specs() -> Vec<ImageSpec> {
    vec![
        ImageSpec {
            description: format!(
                "Amazon Linux 2023 ECS-Optimized AMI {ECS_AL2023_RELEASE} x86_64 HVM kernel-6.18"
            ),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            public_ssm_parameter_name: Some(PUBLISHED_IMAGE_METADATA[0].image_id_parameter()),
            ..published_amazon_image(ECS_AL2023_X86_64_IMAGE, "x86_64", ECS_AL2023_CREATION_DATE)
        },
        ImageSpec {
            description: format!(
                "Amazon Linux 2023 ECS-Optimized AMI {ECS_AL2023_RELEASE} arm64 HVM kernel-6.18"
            ),
            boot_mode: Some(BOOT_MODE_UEFI.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            public_ssm_parameter_name: Some(PUBLISHED_IMAGE_METADATA[1].image_id_parameter()),
            ..published_amazon_image(ECS_AL2023_ARM64_IMAGE, "arm64", ECS_AL2023_CREATION_DATE)
        },
        ImageSpec {
            description: format!("Bottlerocket OS {BOTTLEROCKET_VERSION} (aws-ecs-2) x86_64"),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            // A container host is no Free Tier image, unlike the Amazon Linux base images.
            free_tier_eligible: false,
            public_ssm_parameter_name: Some(PUBLISHED_IMAGE_METADATA[2].image_id_parameter()),
            ..published_amazon_image(
                BOTTLEROCKET_ECS_X86_64_IMAGE,
                "x86_64",
                BOTTLEROCKET_CREATION_DATE,
            )
        },
        ImageSpec {
            description: format!("Bottlerocket OS {BOTTLEROCKET_VERSION} (aws-ecs-2) aarch64"),
            boot_mode: Some(BOOT_MODE_UEFI.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            free_tier_eligible: false,
            public_ssm_parameter_name: Some(PUBLISHED_IMAGE_METADATA[3].image_id_parameter()),
            ..published_amazon_image(
                BOTTLEROCKET_ECS_ARM64_IMAGE,
                "arm64",
                BOTTLEROCKET_CREATION_DATE,
            )
        },
    ]
}

impl ComAmazonawsEc2Amazonec2State {
    /// Every public Systems Manager parameter this account can resolve, and the value it
    /// resolves to.
    ///
    /// The ONE projection Parameter Store reads the published catalog through: the AMI
    /// aliases ([`Self::public_ssm_parameter_image_ids`]), each resolving to the id of the
    /// image it names, and the leaves of the metadata hierarchies published beside an image,
    /// each resolving to the fact published there. A hierarchy whose image this account
    /// cannot see resolves to nothing at all — not to a hierarchy of facts about an image
    /// that is gone — the same way the alias of a deregistered image resolves to nothing.
    pub fn public_ssm_parameter_values(
        &self,
        account: &str,
    ) -> BTreeMap<String, PublicSsmParameterValue> {
        let ids = self.public_ssm_parameter_image_ids(account);
        let published_at = |image_id: &str| -> Option<String> {
            self.images
                .get(image_id)
                .and_then(|image| image.creation_date.clone())
        };
        let mut published: BTreeMap<String, PublicSsmParameterValue> = ids
            .iter()
            .map(|(name, image_id)| {
                (
                    name.clone(),
                    PublicSsmParameterValue {
                        value: image_id.clone(),
                        published_at: published_at(image_id),
                    },
                )
            })
            .collect();
        for hierarchy in PUBLISHED_IMAGE_METADATA {
            let Some(image) = ids
                .get(&hierarchy.image_id_parameter())
                .and_then(|image_id| self.images.get(image_id))
            else {
                continue;
            };
            let at = image.creation_date.clone();
            let mut publish = |leaf: &str, value: String| {
                published.insert(
                    format!("{}/{leaf}", hierarchy.path),
                    PublicSsmParameterValue {
                        value,
                        published_at: at.clone(),
                    },
                );
            };
            if hierarchy.publishes_image_name {
                if let Some(name) = image.name.clone() {
                    publish(IMAGE_NAME_LEAF, name);
                }
            }
            for (leaf, value) in hierarchy.facts {
                publish(leaf, (*value).to_string());
            }
        }
        published
    }
}
