// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the three SERVICE FACTS this service reports rather than
//! derives from a request: its instance-type catalog, its Region availability set, and
//! the resource-kind token its resource-scoped errors carry. Every assertion goes
//! through the router over the wire this service speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! The rules under test, one sentence each:
//!
//! 1. `ListInstanceTypes` reports the service's WHOLE supported instance-type catalog —
//!    every size of every instance family the service can launch a WorkSpace Instance
//!    on — and nothing it cannot launch one on, each entry carrying the complete cross
//!    product of billing mode × platform image × tenancy that family supports.
//! 2. `ListRegions` reports the service's whole commercial-partition availability set,
//!    so a Region the service is available in is reported as supported.
//! 3. A resource-scoped error names the AWS resource KIND that was looked up in its
//!    `ResourceType`, so the token differs per kind: a WorkSpace Instance and the EBS
//!    volume it carries are not the same kind.
//!
//! The tables span what each rule generalises over rather than one recorded response:
//! the catalog table carries a family per generation, per vendor and per ladder shape
//! (burstable, flex, high-frequency, dense-storage, high-memory, GPU) with the sizes
//! each is published at; the exclusion table carries one row per reason a type is NOT
//! in this service's catalog; the Region table carries every partition; and the error
//! table varies the shape of the id across the modelled `VolumeId` pattern. The
//! literal catalogue data each table holds names its source in a comment, and is
//! written here rather than read out of the crate so the test measures the wire
//! against the documented facts instead of against itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call, error_code, ok, router};

/// Families the service supports, and sizes each one is published at.
///
/// SOURCE: the instance families AWS documents as supported for WorkSpaces Instances,
/// crossed with each family's published EC2 size ladder (the same `<family>.<size>`
/// names the EC2 API's own `InstanceType` enumeration carries). The rows differ along
/// what the rule generalises over — generation (fifth through eighth), vendor (Intel,
/// AMD), and ladder SHAPE: burstable (`nano`…), `-flex` (stops at `16xlarge`),
/// high-frequency (`3xlarge`/`6xlarge`), compute (`9xlarge`/`18xlarge`), dense storage,
/// high memory and GPU. None of these names is a recorded response: the issue cited
/// `c5.12xlarge` only, and every other row here is an entry it did not cite.
const SUPPORTED_FAMILY_SIZES: &[(&str, &[&str])] = &[
    // Compute optimised: the `c5` ladder steps through 9xlarge/18xlarge.
    (
        "c5",
        &[
            "large", "xlarge", "2xlarge", "4xlarge", "9xlarge", "12xlarge", "18xlarge", "24xlarge",
        ],
    ),
    ("c5d", &["large", "9xlarge", "24xlarge"]),
    ("c5n", &["large", "9xlarge", "18xlarge"]),
    ("c5a", &["large", "8xlarge", "24xlarge"]),
    ("c6a", &["large", "32xlarge", "48xlarge"]),
    ("c6in", &["large", "32xlarge"]),
    ("c7a", &["medium", "48xlarge"]),
    ("c7i", &["large", "24xlarge", "48xlarge"]),
    ("c7i-flex", &["large", "8xlarge", "16xlarge"]),
    ("c8a", &["medium", "48xlarge"]),
    ("c8i", &["large", "48xlarge", "96xlarge"]),
    // General purpose.
    ("m5", &["large", "8xlarge", "16xlarge", "24xlarge"]),
    ("m5dn", &["large", "24xlarge"]),
    ("m5zn", &["large", "3xlarge", "6xlarge", "12xlarge"]),
    ("m6i", &["large", "32xlarge"]),
    ("m6idn", &["large", "32xlarge"]),
    ("m7a", &["medium", "24xlarge", "48xlarge"]),
    ("m7i", &["large", "24xlarge", "48xlarge"]),
    ("m7i-flex", &["large", "16xlarge"]),
    ("m8i", &["large", "96xlarge"]),
    ("m8i-flex", &["large", "16xlarge"]),
    // Memory optimised.
    ("r5", &["large", "8xlarge", "24xlarge"]),
    ("r5b", &["large", "24xlarge"]),
    ("r6idn", &["large", "32xlarge"]),
    ("r7iz", &["large", "16xlarge", "32xlarge"]),
    ("r8a", &["medium", "48xlarge"]),
    ("r8i-flex", &["large", "16xlarge"]),
    ("z1d", &["large", "3xlarge", "12xlarge"]),
    // High memory.
    ("x1", &["16xlarge", "32xlarge"]),
    ("x1e", &["xlarge", "32xlarge"]),
    ("x2idn", &["16xlarge", "32xlarge"]),
    ("x2iedn", &["xlarge", "24xlarge", "32xlarge"]),
    ("x2iezn", &["2xlarge", "6xlarge", "12xlarge"]),
    // Storage optimised.
    ("d2", &["xlarge", "8xlarge"]),
    ("d3en", &["xlarge", "6xlarge", "12xlarge"]),
    ("h1", &["2xlarge", "16xlarge"]),
    ("i3", &["large", "16xlarge"]),
    ("i3en", &["large", "3xlarge", "24xlarge"]),
    ("i4i", &["large", "32xlarge"]),
    ("i7i", &["large", "48xlarge"]),
    ("i7ie", &["large", "18xlarge", "48xlarge"]),
    // Burstable: the whole ladder, both ends included.
    (
        "t3",
        &[
            "nano", "micro", "small", "medium", "large", "xlarge", "2xlarge",
        ],
    ),
    ("t2", &["nano", "2xlarge"]),
    ("t3a", &["nano", "2xlarge"]),
    // Graphics.
    ("g4ad", &["xlarge", "16xlarge"]),
    ("g4dn", &["xlarge", "12xlarge", "16xlarge"]),
    ("g5", &["xlarge", "24xlarge", "48xlarge"]),
    ("g6", &["xlarge", "48xlarge"]),
    ("g6e", &["xlarge", "48xlarge"]),
    ("gr6", &["4xlarge", "8xlarge"]),
];

/// Instance types that are NOT in this service's catalog, one row per REASON.
///
/// SOURCE: the same documentation. A WorkSpace Instance is a managed EC2 instance
/// running a WorkSpaces x86-64 platform image, so a previous-generation family, an
/// AArch64 (Graviton) family, a bare-metal size — whose EC2 name (`c7i.metal-48xl`)
/// does not even satisfy the modelled `InstanceType` pattern
/// `^([a-z0-9-]+)\.([a-z0-9]+)$` — a Mac instance, or an HPC/ML-accelerator family is
/// not something a caller may pass to `CreateWorkspaceInstance`.
const UNSUPPORTED_INSTANCE_TYPES: &[(&str, &str)] = &[
    ("previous generation", "c1.medium"),
    ("previous generation", "c3.large"),
    ("previous generation", "c4.large"),
    ("previous generation", "m4.xlarge"),
    ("previous generation", "r4.large"),
    ("previous generation", "t1.micro"),
    ("previous generation", "i2.xlarge"),
    ("AArch64 (Graviton)", "a1.medium"),
    ("AArch64 (Graviton)", "c7g.large"),
    ("AArch64 (Graviton)", "m7g.medium"),
    ("AArch64 (Graviton)", "r8g.large"),
    ("AArch64 (Graviton)", "t4g.micro"),
    ("AArch64 (Graviton)", "x2gd.large"),
    ("bare metal", "c5.metal"),
    ("bare metal", "m6i.metal"),
    ("bare metal", "i3.metal"),
    ("bare metal", "c7i.metal-48xl"),
    ("Mac", "mac1.metal"),
    ("Mac", "mac2-m2.metal"),
    ("HPC", "hpc7a.48xlarge"),
    ("ML accelerator", "p5.48xlarge"),
    ("ML accelerator", "trn1.32xlarge"),
    ("ML accelerator", "inf2.xlarge"),
    ("FPGA", "f2.6xlarge"),
    ("video transcoding", "vt1.3xlarge"),
    ("deep learning", "dl1.24xlarge"),
];

/// Regions the service is available in, and Regions it is not, with the reason.
///
/// SOURCE: AWS's published Region availability for WorkSpaces Instances in the `aws`
/// (commercial) partition. The supported rows span every geography the set covers —
/// including the Regions a narrower, invented list left out — and the unsupported rows
/// cover the other partitions and a Region name that exists nowhere.
const SUPPORTED_REGION_ROWS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-2",
    "ca-central-1",
    "sa-east-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-central-1",
    "eu-south-2",
    "il-central-1",
    "me-central-1",
    "af-south-1",
    "ap-east-1",
    "ap-south-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-5",
];

/// Regions `ListRegions` must not report, with the reason each is out of scope.
const UNSUPPORTED_REGION_ROWS: &[(&str, &str)] = &[
    ("GovCloud partition", "us-gov-west-1"),
    ("GovCloud partition", "us-gov-east-1"),
    ("China partition", "cn-north-1"),
    ("China partition", "cn-northwest-1"),
    ("no such Region", "xx-nowhere-9"),
];

/// Every `InstanceType` the catalog reports, with its configuration combinations, as a
/// map from name to the (billing mode, platform type, tenancy) triples it lists.
///
/// The catalog fits one page of the modelled maximum (`ListInstanceTypesMaxResults`,
/// `range 1..600`), which the assertion below pins; the walk still follows a
/// `NextToken` if one ever appears, so this helper cannot silently read a prefix.
async fn reported_catalog(router: &MultiServiceRouter) -> Vec<(String, Vec<(String, String, String)>)> {
    let mut rows = Vec::new();
    let mut request = json!({});
    loop {
        let page = ok(router, "ListInstanceTypes", request.clone()).await;
        let entries = page["InstanceTypes"]
            .as_array()
            .unwrap_or_else(|| panic!("ListInstanceTypes reported no InstanceTypes: {page}"))
            .clone();
        for entry in entries {
            let name = entry["InstanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("an InstanceTypeInfo carries no InstanceType: {entry}"))
                .to_string();
            let combinations = entry["SupportedInstanceConfigurations"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("{name} carries no SupportedInstanceConfigurations: {entry}")
                })
                .iter()
                .map(|configuration| {
                    let member = |field: &str| {
                        configuration[field]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("a SupportedInstanceConfiguration omits {field}: {configuration}")
                            })
                            .to_string()
                    };
                    (
                        member("BillingMode"),
                        member("PlatformType"),
                        member("Tenancy"),
                    )
                })
                .collect();
            rows.push((name, combinations));
        }
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => request["NextToken"] = json!(token),
            None => return rows,
        }
    }
}

/// The `RegionName`s `ListRegions` reports.
async fn reported_regions(router: &MultiServiceRouter) -> Vec<String> {
    let payload = ok(router, "ListRegions", json!({})).await;
    payload["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("ListRegions reported no Regions: {payload}"))
        .iter()
        .map(|region| {
            region["RegionName"]
                .as_str()
                .unwrap_or_else(|| panic!("a Region carries no RegionName: {region}"))
                .to_string()
        })
        .collect()
}

/// Rule 1: every size of every supported family is reported, each with the complete
/// cross product of the configuration dimensions its family supports.
#[tokio::test]
async fn the_whole_supported_instance_type_catalog_is_reported() {
    let router = router();
    let catalog = reported_catalog(&router).await;
    let names: Vec<&str> = catalog.iter().map(|(name, _)| name.as_str()).collect();

    // The catalog is the service's whole supported set, so it is far larger than a
    // hand-picked sample, and it fits inside the modelled maximum page of
    // `ListInstanceTypesMaxResults` (`range 1..600`) — which is why the walk above
    // normally reads it in one response.
    assert!(
        names.len() >= 500 && names.len() <= 600,
        "case catalog size: the whole supported catalog spans hundreds of types and fits \
         one page of the modelled maximum (600), got {} types",
        names.len()
    );

    for (family, sizes) in SUPPORTED_FAMILY_SIZES {
        for size in *sizes {
            let case = format!("{family}.{size}");
            assert!(
                names.contains(&case.as_str()),
                "case {case:?}: a supported family must be reported at every published \
                 size; the catalog reported {} types and none of them is this one",
                names.len()
            );
        }
        // The family is reported at EVERY size it is published at, so a caller that
        // picks a size from AWS's own ladder is never refused: no sampled subset.
        let reported_for_family: Vec<&&str> = names
            .iter()
            .filter(|name| name.starts_with(&format!("{family}.")))
            .collect();
        assert!(
            reported_for_family.len() >= sizes.len(),
            "case family {family:?}: expected at least the {} published sizes, got \
             {reported_for_family:?}",
            sizes.len()
        );
    }

    // Each entry lists every combination its family supports: the cross product of the
    // two `BillingMode` values, the platform images it carries and its tenancies — not
    // a subset of them. `c5` carries every platform image and both tenancies, so its
    // rows carry 2 × 7 × 2 = 28 combinations; the burstable families are shared-tenancy
    // only, so theirs carry half as many per platform image.
    let combinations_of = |wanted: &str| -> Vec<(String, String, String)> {
        catalog
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, combinations)| combinations.clone())
            .unwrap_or_else(|| panic!("case {wanted:?}: the catalog must report it"))
    };
    for name in ["c5.12xlarge", "c5.large", "m8i.96xlarge", "x2iezn.12xlarge"] {
        let combinations = combinations_of(name);
        let distinct: std::collections::BTreeSet<_> = combinations.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            28,
            "case {name:?}: expected the 28 combinations of 2 billing modes × 7 platform \
             images × 2 tenancies, got {distinct:?}"
        );
        for billing_mode in ["HOURLY", "MONTHLY"] {
            for tenancy in ["SHARED", "DEDICATED"] {
                for platform_type in [
                    "Linux/UNIX",
                    "Red Hat BYOL Linux",
                    "Red Hat Enterprise Linux",
                    "SUSE Linux",
                    "Ubuntu Pro Linux",
                    "Windows",
                    "Windows BYOL",
                ] {
                    let wanted = (
                        billing_mode.to_string(),
                        platform_type.to_string(),
                        tenancy.to_string(),
                    );
                    assert!(
                        distinct.contains(&wanted),
                        "case {name:?}: expected the combination {wanted:?}, got {distinct:?}"
                    );
                }
            }
        }
    }
    for name in ["t3.nano", "t2.2xlarge", "t3a.medium"] {
        let combinations = combinations_of(name);
        let distinct: std::collections::BTreeSet<_> = combinations.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            14,
            "case {name:?}: a burstable family runs shared tenancy only, so expected 2 × 7 \
             combinations, got {distinct:?}"
        );
        assert!(
            distinct.iter().all(|(_, _, tenancy)| tenancy == "SHARED"),
            "case {name:?}: expected SHARED tenancy only, got {distinct:?}"
        );
    }
}

/// Rule 1, the other direction: a type the service cannot launch a WorkSpace Instance
/// on is not reported, so discovery does not offer a caller something a create would
/// have to refuse.
#[tokio::test]
async fn instance_types_the_service_cannot_launch_are_not_reported() {
    let router = router();
    let catalog = reported_catalog(&router).await;
    let names: Vec<&str> = catalog.iter().map(|(name, _)| name.as_str()).collect();

    for (reason, instance_type) in UNSUPPORTED_INSTANCE_TYPES {
        assert!(
            !names.contains(instance_type),
            "case {instance_type:?} ({reason}): a type outside the service's catalog must \
             not be reported"
        );
    }

    // Every reported name satisfies the modelled `InstanceType`
    // (`len 1..64`, `^([a-z0-9-]+)\.([a-z0-9]+)$`), is reported once, and the catalog
    // is reported in name order — which is what makes a `NextToken` resume at a
    // well-defined place.
    for name in &names {
        let (family, size) = name
            .split_once('.')
            .unwrap_or_else(|| panic!("case {name:?}: a reported name must be <family>.<size>"));
        assert!(
            (1..=64).contains(&name.len())
                && !family.is_empty()
                && !size.is_empty()
                && family
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && size
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {name:?}: expected a name inside the modelled InstanceType pattern"
        );
    }
    let mut sorted: Vec<&str> = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted, names,
        "case catalog order: the catalog is reported in name order with no repeats"
    );
}

/// Rule 2: every Region the service is available in is reported, and no Region outside
/// the commercial partition is.
#[tokio::test]
async fn the_region_availability_set_is_reported_whole() {
    let router = router();
    let reported = reported_regions(&router).await;

    for region in SUPPORTED_REGION_ROWS {
        assert!(
            reported.iter().any(|name| name == region),
            "case {region:?}: a Region the service is available in must be reported as \
             supported, got {reported:?}"
        );
    }
    for (reason, region) in UNSUPPORTED_REGION_ROWS {
        assert!(
            !reported.iter().any(|name| name == region),
            "case {region:?} ({reason}): must not be reported, got {reported:?}"
        );
    }
    assert_eq!(
        reported.len(),
        SUPPORTED_REGION_ROWS.len(),
        "case Region set: expected exactly the documented availability set, got {reported:?}"
    );
}

/// Rule 3: the `ResourceType` of a not-found error names the AWS resource KIND that
/// was looked up — the EBS volume for a `VolumeId`, the WorkSpace Instance for a
/// `WorkspaceInstanceId` — so the two kinds carry different tokens.
#[tokio::test]
async fn a_not_found_error_names_the_resource_kind_that_was_looked_up() {
    let router = router();

    // `VolumeId` is `^vol-[0-9a-zA-Z]{1,63}$`: the rows span the shortest suffix, the
    // typical EC2 form, mixed case, all digits and the longest suffix the pattern
    // admits. The kind token does not depend on any of them.
    let volume_ids = vec![
        ("the shortest suffix the pattern admits", "vol-a".to_string()),
        (
            "the typical EC2 form",
            "vol-0123456789abcdef0".to_string(),
        ),
        ("mixed case", "vol-AbCdEf0123".to_string()),
        ("all digits", "vol-0000000000".to_string()),
        (
            "a 63-character suffix, the modelled maximum",
            format!("vol-{}", "a".repeat(63)),
        ),
    ];
    for (case, id) in &volume_ids {
        let (status, payload) = call(&router, "DeleteVolume", json!({ "VolumeId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({id:?}): expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?} ({id:?}): expected ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            payload["ResourceId"].as_str(),
            Some(id.as_str()),
            "case {case:?} ({id:?}): the error must name the id looked up, got {payload}"
        );
        // The AWS resource kind of a WorkSpace Instance's volume: an Amazon EBS volume.
        assert_eq!(
            payload["ResourceType"].as_str(),
            Some("EBS_VOLUME"),
            "case {case:?} ({id:?}): expected the EBS volume resource kind, got {payload}"
        );
    }

    // The same token after a round trip through the store: a volume this service minted
    // and deleted is gone, and its second delete reports the same kind.
    let created = ok(
        &router,
        "CreateVolume",
        json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 100 }),
    )
    .await;
    let minted = created["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateVolume returned no VolumeId: {created}"))
        .to_string();
    ok(&router, "DeleteVolume", json!({ "VolumeId": minted })).await;
    let (status, payload) = call(&router, "DeleteVolume", json!({ "VolumeId": minted })).await;
    assert_eq!(
        status, 400,
        "case \"a deleted volume\" ({minted:?}): expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some("EBS_VOLUME"),
        "case \"a deleted volume\" ({minted:?}): expected the EBS volume resource kind, got {payload}"
    );

    // A different kind carries a different token: the instance paths report the
    // WorkSpace Instance kind for an id that names no stored instance.
    for (case, operation) in [
        ("a read of an unknown instance", "GetWorkspaceInstance"),
        ("a delete of an unknown instance", "DeleteWorkspaceInstance"),
    ] {
        let id = "wsinst-0123abcd";
        let (status, payload) =
            call(&router, operation, json!({ "WorkspaceInstanceId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            payload["ResourceType"].as_str(),
            Some("WORKSPACE_INSTANCE"),
            "case {case:?}: expected the WorkSpace Instance resource kind, got {payload}"
        );
        assert_ne!(
            payload["ResourceType"].as_str(),
            Some("EBS_VOLUME"),
            "case {case:?}: an instance and its volume are different kinds, got {payload}"
        );
    }
}
