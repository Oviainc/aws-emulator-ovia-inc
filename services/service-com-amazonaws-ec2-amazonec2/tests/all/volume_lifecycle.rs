// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EBS volume slice: `CreateVolume` and `DescribeVolumes`,
//! driven over the wire the service actually speaks — `ec2Query`, through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateVolume` MINTS A VOLUME FROM THE REQUEST and `DescribeVolumes` REPORTS THE
//!    STORE: every member the request sets round-trips, and the members the service owns
//!    (the id, the ARN, the account, the create time) are derived rather than taken;
//! 2. a volume is `creating` and the CLOCK, not a caller, moves it to `available`;
//! 3. a parameter outside what the model documents for the volume's TYPE is refused with
//!    EC2's own error code, and nothing is stored;
//! 4. one `ClientToken` names one volume.
//!
//! Each table row is a distinct input along what the rule generalises over — the volume
//! type, the size at each published bound and one past it, the zone designator, the IOPS
//! and throughput rules, which filter, which page size — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    MAX_KMS_KEY_ID_CHARS, MAX_TAGS_PER_RESOURCE, TRANSITION_DELAY, VOLUME_IOPS_RANGES,
    VOLUME_SIZE_RANGES, availability_zones_in_region,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed};

/// The Region every call here is signed for; the zone names and ids the assertions use
/// are derived from it.
pub(super) const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
pub(super) fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a status and, when it is a refusal, an error code:
/// `(case, the parameters under test, the HTTP status, the error code)`.
type StatusRow = (&'static str, Params, u16, &'static str);

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Params, &'static str);

/// One row of the filter table: `(case, the filter parameters, the ids it must select)`.
type FilterRow = (&'static str, Params, Vec<String>);

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
pub(super) async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVolume", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVolumes", params).await
}

/// The first zone of the signed Region, which most rows create into.
pub(super) fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

pub(super) fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

/// A `VolumeId` is `vol-` followed by 8 or 17 lowercase hex digits.
pub(super) fn is_volume_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("vol-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17) && suffix.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Create one volume of `volume_type` at `size` in the Region's first zone, with whatever
/// extra parameters the case needs, and answer the response.
async fn create_volume(
    router: &MultiServiceRouter,
    volume_type: &str,
    size: i64,
    extra: Vec<(String, String)>,
) -> (u16, String) {
    let mut params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("VolumeType", volume_type),
        p("Size", size.to_string()),
    ];
    params.extend(extra);
    create(router, &params).await
}

/// The ids `DescribeVolumes` reports for `params`, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVolumes, got {status} with body {body}"
    );
    elements(&body, "volumeId")
}

/// The IOPS a volume type needs on creation, as `Iops` parameters: "Required for io1 and
/// io2 volumes" (`CreateVolumeRequest$Iops`), at the bottom of that type's range.
pub(super) fn required_iops(volume_type: &str) -> Vec<(String, String)> {
    VOLUME_IOPS_RANGES
        .iter()
        .find(|(name, _, _)| *name == volume_type && matches!(*name, "io1" | "io2"))
        .map(|(_, min, _)| vec![p("Iops", min.to_string())])
        .unwrap_or_default()
}

/// RULE 1 and RULE 3 over the size catalog: every modelled volume type accepts its
/// published minimum and maximum size and refuses one past either bound, and an accepted
/// volume reads back through `DescribeVolumes` exactly as created.
///
/// The catalog is the service's own [`VOLUME_SIZE_RANGES`], whose source is
/// `CreateVolumeRequest$Size`, so every type is covered — including the ones the issue
/// never cited.
#[tokio::test]
async fn every_volume_type_accepts_its_published_size_bounds_and_refuses_one_past_them() {
    for (volume_type, min, max) in VOLUME_SIZE_RANGES {
        for (bound, size) in [("minimum", i64::from(min)), ("maximum", i64::from(max))] {
            let router = emulator();
            let case = format!("{volume_type} at its {bound} size ({size} GiB)");
            let (status, body) =
                create_volume(&router, volume_type, size, required_iops(volume_type)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
            );
            let volume_id = element(&body, "volumeId").unwrap_or_default();
            assert!(
                is_volume_id(&volume_id),
                "case {case:?}: expected a minted volume id, got {volume_id:?}"
            );
            for (member, expected) in [
                ("size", size.to_string()),
                ("volumeType", volume_type.to_string()),
                ("availabilityZone", zone_name(0)),
                ("availabilityZoneId", zone_id(0)),
                ("status", "creating".to_string()),
                ("encrypted", "false".to_string()),
                (
                    "volumeArn",
                    format!(
                        "arn:aws:ec2:{REGION}:{}:volume/{volume_id}",
                        engine::EMULATOR_ACCOUNT_ID
                    ),
                ),
                ("ownerId", engine::EMULATOR_ACCOUNT_ID.to_string()),
            ] {
                assert_eq!(
                    element(&body, member).as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {member}={expected:?}, got {:?}",
                    element(&body, member)
                );
            }
            // "This parameter is not returned by CreateVolume" (`Volume$Attachments`,
            // `Volume$FastRestored`, `Volume$SseType`).
            for absent in ["attachmentSet", "fastRestored", "sseType"] {
                assert!(
                    element(&body, absent).is_none(),
                    "case {case:?}: CreateVolume does not return {absent}, got {body}"
                );
            }

            // ROUND TRIP: the volume is in the store, not just echoed.
            let (status, described) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeVolumes, got {status} / {described}"
            );
            assert_eq!(
                elements(&described, "volumeId"),
                vec![volume_id.clone()],
                "case {case:?}: expected DescribeVolumes to report the created volume, \
                 got {described}"
            );
            for (member, expected) in [
                ("size", size.to_string()),
                ("volumeType", volume_type.to_string()),
                ("availabilityZone", zone_name(0)),
            ] {
                assert_eq!(
                    element(&described, member).as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected the stored {member}={expected:?}, got {:?}",
                    element(&described, member)
                );
            }
        }

        for (bound, size) in [
            ("one below the minimum", i64::from(min) - 1),
            ("one above the maximum", i64::from(max) + 1),
        ] {
            let router = emulator();
            let case = format!("{volume_type} at {bound} ({size} GiB)");
            let (status, body) =
                create_volume(&router, volume_type, size, required_iops(volume_type)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                described_ids(&router, &case, &[]).await.is_empty(),
                "case {case:?}: a refused CreateVolume must store nothing"
            );
        }
    }
}

/// RULE 2: a created volume is `creating`, every read inside the span still says so, and
/// the clock — nothing the caller does — moves it to `available`.
#[tokio::test]
async fn a_created_volume_is_creating_until_the_clock_makes_it_available() {
    let router = emulator();
    let mut volume_ids = Vec::new();
    // Three volumes of different types, so the transition is not a property of one row.
    for (volume_type, size) in [("gp3", 1), ("gp2", 8), ("standard", 2)] {
        let (status, body) = create_volume(&router, volume_type, size, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {volume_type:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "status").as_deref(),
            Some("creating"),
            "case {volume_type:?}: expected a fresh volume to be creating, got {body}"
        );
        volume_ids.push(element(&body, "volumeId").unwrap_or_default());
    }

    // The span is never zero: a caller reading straight after the create still sees the
    // transient state, and the `status` filter agrees with what the volume reports.
    for volume_id in &volume_ids {
        let (status, body) = describe(&router, &indexed("VolumeId", &[volume_id])).await;
        assert_eq!(status, 200, "expected HTTP 200, got {status} with body {body}");
        assert_eq!(
            element(&body, "status").as_deref(),
            Some("creating"),
            "expected {volume_id} to still be creating, got {body}"
        );
    }
    let creating = described_ids(
        &router,
        "status=creating inside the span",
        &filters(&[("status", &["creating"])]),
    )
    .await;
    assert_eq!(
        creating.len(),
        volume_ids.len(),
        "case \"status=creating inside the span\": expected every volume, got {creating:?}"
    );

    past_the_transition().await;

    for volume_id in &volume_ids {
        // Twice: the materialised state is stable, not consumed by the read.
        for read in 1..=2 {
            let (status, body) = describe(&router, &indexed("VolumeId", &[volume_id])).await;
            assert_eq!(
                status, 200,
                "read {read}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "status").as_deref(),
                Some("available"),
                "read {read}: expected {volume_id} to be available once the transition is due, \
                 got {body}"
            );
        }
    }
    assert!(
        described_ids(
            &router,
            "status=creating after the span",
            &filters(&[("status", &["creating"])])
        )
        .await
        .is_empty(),
        "case \"status=creating after the span\": no volume is creating any more"
    );
    assert_eq!(
        described_ids(
            &router,
            "status=available after the span",
            &filters(&[("status", &["available"])])
        )
        .await
        .len(),
        volume_ids.len(),
        "case \"status=available after the span\": expected every volume"
    );
}

/// RULE 3: "Either AvailabilityZone or AvailabilityZoneId must be specified, but not
/// both" (`CreateVolumeRequest$AvailabilityZone`), and the zone must be one the Region
/// has.
#[tokio::test]
async fn create_volume_requires_exactly_one_known_availability_zone() {
    let rows: Vec<StatusRow> = vec![
        (
            "the zone's name",
            vec![p("AvailabilityZone", zone_name(1))],
            200,
            "",
        ),
        (
            "the zone's id",
            vec![p("AvailabilityZoneId", zone_id(2))],
            200,
            "",
        ),
        (
            "both designators",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("AvailabilityZoneId", zone_id(0)),
            ],
            400,
            "InvalidParameterCombination",
        ),
        ("neither designator", Vec::new(), 400, "MissingParameter"),
        (
            "a zone name no Region has",
            vec![p("AvailabilityZone", "eu-south-9z")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a zone id no Region has",
            vec![p("AvailabilityZoneId", "zzz9-az42")],
            400,
            "InvalidParameterValue",
        ),
        (
            "another Region's zone",
            vec![p("AvailabilityZone", "us-east-1a")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, designator, expected_status, expected_code) in rows {
        let router = emulator();
        let mut params = vec![p("VolumeType", "gp3"), p("Size", "4")];
        params.extend(designator);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert!(
                is_volume_id(&element(&body, "volumeId").unwrap_or_default()),
                "case {case:?}: expected a minted volume, got {body}"
            );
            continue;
        }
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            described_ids(&router, case, &[]).await.is_empty(),
            "case {case:?}: a refused CreateVolume must store nothing"
        );
    }
}

/// RULE 3 over the IOPS and throughput catalog: `Iops` is required for io1 and io2,
/// optional for gp3 with a documented default, and refused for every other type;
/// `Throughput` is gp3-only with its own range and default.
///
/// Source: `CreateVolumeRequest$Iops` and `CreateVolumeRequest$Throughput`, held as
/// [`VOLUME_IOPS_RANGES`] and the service's gp3 constants.
#[tokio::test]
async fn iops_and_throughput_follow_the_published_per_type_rules() {
    struct Row {
        case: &'static str,
        volume_type: &'static str,
        extra: Vec<(String, String)>,
        status: u16,
        code: &'static str,
        /// The IOPS and throughput the accepted volume must report.
        iops: Option<&'static str>,
        throughput: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "gp3 without iops takes the documented default",
            volume_type: "gp3",
            extra: Vec::new(),
            status: 200,
            code: "",
            iops: Some("3000"),
            throughput: Some("125"),
        },
        Row {
            case: "gp3 at the bottom of its iops range",
            volume_type: "gp3",
            extra: vec![p("Iops", "3000")],
            status: 200,
            code: "",
            iops: Some("3000"),
            throughput: Some("125"),
        },
        Row {
            case: "gp3 at the top of its iops range",
            volume_type: "gp3",
            extra: vec![p("Iops", "80000")],
            status: 200,
            code: "",
            iops: Some("80000"),
            throughput: Some("125"),
        },
        Row {
            case: "gp3 one below its iops range",
            volume_type: "gp3",
            extra: vec![p("Iops", "2999")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "gp3 one above its iops range",
            volume_type: "gp3",
            extra: vec![p("Iops", "80001")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "io1 without the iops it requires",
            volume_type: "io1",
            extra: Vec::new(),
            status: 400,
            code: "MissingParameter",
            iops: None,
            throughput: None,
        },
        Row {
            case: "io1 at the top of its iops range",
            volume_type: "io1",
            extra: vec![p("Iops", "64000")],
            status: 200,
            code: "",
            iops: Some("64000"),
            throughput: None,
        },
        Row {
            case: "io2 at the top of its iops range",
            volume_type: "io2",
            extra: vec![p("Iops", "256000")],
            status: 200,
            code: "",
            iops: Some("256000"),
            throughput: None,
        },
        Row {
            case: "io2 one above its iops range",
            volume_type: "io2",
            extra: vec![p("Iops", "256001")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "gp2 provisions no iops",
            volume_type: "gp2",
            extra: vec![p("Iops", "3000")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "st1 provisions no iops",
            volume_type: "st1",
            extra: vec![p("Iops", "100")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "gp3 at the bottom of its throughput range",
            volume_type: "gp3",
            extra: vec![p("Throughput", "125")],
            status: 200,
            code: "",
            iops: Some("3000"),
            throughput: Some("125"),
        },
        Row {
            case: "gp3 at the top of its throughput range",
            volume_type: "gp3",
            extra: vec![p("Throughput", "2000")],
            status: 200,
            code: "",
            iops: Some("3000"),
            throughput: Some("2000"),
        },
        Row {
            case: "gp3 one below its throughput range",
            volume_type: "gp3",
            extra: vec![p("Throughput", "124")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "gp3 one above its throughput range",
            volume_type: "gp3",
            extra: vec![p("Throughput", "2001")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "sc1 supports no throughput",
            volume_type: "sc1",
            extra: vec![p("Throughput", "125")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "multi-attach is supported with io2",
            volume_type: "io2",
            extra: vec![p("Iops", "100"), p("MultiAttachEnabled", "true")],
            status: 200,
            code: "",
            iops: Some("100"),
            throughput: None,
        },
        Row {
            case: "multi-attach is not supported with gp3",
            volume_type: "gp3",
            extra: vec![p("MultiAttachEnabled", "true")],
            status: 400,
            code: "InvalidParameterCombination",
            iops: None,
            throughput: None,
        },
    ];

    for row in rows {
        let router = emulator();
        // Every row is at a size its type accepts, so only the member under test can
        // refuse it.
        let (_, min, _) = VOLUME_SIZE_RANGES
            .iter()
            .find(|(name, _, _)| *name == row.volume_type)
            .copied()
            .expect("every tested type has a published size range");
        let (status, body) =
            create_volume(&router, row.volume_type, i64::from(min), row.extra).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} with body {body}",
            row.case, row.status
        );
        if row.status != 200 {
            assert_eq!(
                error_code(&body),
                row.code,
                "case {:?}: expected {}, got {body}",
                row.case,
                row.code
            );
            assert!(
                described_ids(&router, row.case, &[]).await.is_empty(),
                "case {:?}: a refused CreateVolume must store nothing",
                row.case
            );
            continue;
        }
        assert_eq!(
            element(&body, "iops").as_deref(),
            row.iops,
            "case {:?}: expected iops={:?}, got {:?}",
            row.case,
            row.iops,
            element(&body, "iops")
        );
        assert_eq!(
            element(&body, "throughput").as_deref(),
            row.throughput,
            "case {:?}: expected throughput={:?}, got {:?}",
            row.case,
            row.throughput,
            element(&body, "throughput")
        );
        // The derived members are stored, not just answered.
        let volume_id = element(&body, "volumeId").unwrap_or_default();
        let (_, described) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
        assert_eq!(
            element(&described, "iops").as_deref(),
            row.iops,
            "case {:?}: expected the stored iops={:?}, got {described}",
            row.case,
            row.iops
        );
    }
}

/// RULE 4: one `ClientToken` names the one volume it minted, so a replay reports that
/// volume and creates nothing; a different token creates another volume.
#[tokio::test]
async fn one_client_token_names_one_volume() {
    let router = emulator();
    for token in ["token-0001", "a-second-token"] {
        let first =
            create_volume(&router, "gp3", 2, vec![p("ClientToken", token)]).await;
        let second =
            create_volume(&router, "gp3", 2, vec![p("ClientToken", token)]).await;
        assert_eq!(
            first.0, 200,
            "case {token:?}: expected HTTP 200, got {} with body {}",
            first.0, first.1
        );
        assert_eq!(
            second.0, 200,
            "case {token:?}: expected HTTP 200 on the replay, got {} with body {}",
            second.0, second.1
        );
        assert_eq!(
            element(&second.1, "volumeId"),
            element(&first.1, "volumeId"),
            "case {token:?}: a replayed token must answer with the volume it minted, \
             got {} then {}",
            first.1,
            second.1
        );
    }
    let stored = described_ids(&router, "two tokens, two volumes", &[]).await;
    assert_eq!(
        stored.len(),
        2,
        "expected one volume per distinct client token, got {stored:?}"
    );
}

/// RULE 3: a request whose `Size` is absent, or whose `SnapshotId` names a snapshot this
/// account has none of, is refused — "You must specify either a snapshot ID or a volume
/// size" (`CreateVolumeRequest$Size`).
#[tokio::test]
async fn a_sizeless_or_snapshot_backed_create_is_refused() {
    let rows: Vec<RefusalRow> = vec![
        (
            "neither size nor snapshot",
            vec![p("AvailabilityZone", zone_name(0))],
            "MissingParameter",
        ),
        (
            "a snapshot id this account has no snapshot for",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("SnapshotId", "snap-00000000000000001"),
            ],
            "InvalidSnapshot.NotFound",
        ),
        (
            "an id that is not shaped like a snapshot id",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("SnapshotId", "snap-not-hex"),
            ],
            "InvalidSnapshotID.Malformed",
        ),
        (
            "a snapshot id beside a size",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("Size", "8"),
                p("SnapshotId", "snap-0123456789abcdef0"),
            ],
            "InvalidSnapshot.NotFound",
        ),
    ];
    for (case, params, expected_code) in rows {
        let router = emulator();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            described_ids(&router, case, &[]).await.is_empty(),
            "case {case:?}: a refused CreateVolume must store nothing"
        );
    }
}

/// RULE 1: `TagSpecifications` tags the volume the call creates and nothing else, and the
/// tags round-trip through `DescribeVolumes` and its tag filters.
#[tokio::test]
async fn tag_specifications_tag_the_created_volume() {
    let router = emulator();
    let tags = [("Name", "data"), ("Owner", "TeamA"), ("empty", "")];
    let mut extra = vec![p("TagSpecification.1.ResourceType", "volume")];
    for (index, (key, value)) in tags.iter().enumerate() {
        extra.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        extra.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    let (status, body) = create_volume(&router, "gp3", 4, extra).await;
    assert_eq!(
        status, 200,
        "case \"three tags on the volume\": expected HTTP 200, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId").unwrap_or_default();
    assert_eq!(
        elements(&body, "key"),
        vec!["Name", "Owner", "empty"],
        "case \"three tags on the volume\": expected every tag key, got {body}"
    );

    // The tag filters select on what was stored.
    for (case, filter, expected) in [
        ("tag:Name", filters(&[("tag:Name", &["data"])]), true),
        ("tag:Name mismatched", filters(&[("tag:Name", &["other"])]), false),
        ("tag-key", filters(&[("tag-key", &["Owner"])]), true),
        ("tag-key mismatched", filters(&[("tag-key", &["Absent"])]), false),
        (
            "tag:Name with a wildcard",
            filters(&[("tag:Name", &["da*"])]),
            true,
        ),
    ] {
        let reported = described_ids(&router, case, &filter).await;
        let selected = reported.contains(&volume_id);
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the volume to be {}, got {reported:?}",
            if expected { "selected" } else { "excluded" }
        );
    }

    // A specification naming any other resource type is refused: this call creates only
    // the volume, so a tag for anything else could never be read back.
    let refused = vec![
        p("TagSpecification.1.ResourceType", "instance"),
        p("TagSpecification.1.Tag.1.Key", "Name"),
        p("TagSpecification.1.Tag.1.Value", "nope"),
    ];
    let (status, body) = create_volume(&router, "gp3", 4, refused).await;
    assert_eq!(
        status, 400,
        "case \"tagging an instance on CreateVolume\": expected HTTP 400, got {status} / {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"tagging an instance on CreateVolume\": expected InvalidParameterValue, got {body}"
    );

    // One past the documented tags-per-resource quota.
    let mut too_many = vec![p("TagSpecification.1.ResourceType", "volume")];
    for nth in 0..=MAX_TAGS_PER_RESOURCE {
        too_many.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", nth + 1),
            format!("k{nth}"),
        ));
        too_many.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", nth + 1),
            format!("v{nth}"),
        ));
    }
    let (status, body) = create_volume(&router, "gp3", 4, too_many).await;
    assert_eq!(
        status, 400,
        "case \"one tag past the quota\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "TagLimitExceeded",
        "case \"one tag past the quota\": expected TagLimitExceeded, got {body}"
    );
}

/// RULE 1: `DescribeVolumes` reports the store narrowed by `VolumeIds` and every
/// documented filter, and paged by `MaxResults`/`NextToken`.
#[tokio::test]
async fn describe_volumes_narrows_by_id_filter_and_page() {
    let router = emulator();

    // Volumes differing along every attribute the documented filters select on.
    struct Seed {
        volume_type: &'static str,
        size: i64,
        zone_index: usize,
        encrypted: bool,
    }
    let seeds = [
        Seed { volume_type: "gp3", size: 1, zone_index: 0, encrypted: false },
        Seed { volume_type: "gp2", size: 2, zone_index: 1, encrypted: true },
        Seed { volume_type: "io1", size: 4, zone_index: 2, encrypted: false },
        Seed { volume_type: "st1", size: 125, zone_index: 0, encrypted: true },
        Seed { volume_type: "standard", size: 3, zone_index: 1, encrypted: false },
    ];
    let mut created: Vec<String> = Vec::new();
    for (nth, seed) in seeds.iter().enumerate() {
        let mut params = vec![
            p("AvailabilityZone", zone_name(seed.zone_index)),
            p("VolumeType", seed.volume_type),
            p("Size", seed.size.to_string()),
        ];
        params.extend(required_iops(seed.volume_type));
        if seed.encrypted {
            params.push(p("Encrypted", "true"));
        }
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "seed #{nth}: expected HTTP 200, got {status} with body {body}"
        );
        created.push(element(&body, "volumeId").unwrap_or_default());
    }
    created.sort();

    // Default: every volume of the account, in id order.
    assert_eq!(
        described_ids(&router, "no narrowing", &[]).await,
        created,
        "case \"no narrowing\": expected every stored volume in id order"
    );

    // `VolumeIds` narrows to the ids named.
    for count in 1..=created.len() {
        let wanted: Vec<&str> = created.iter().take(count).map(String::as_str).collect();
        let case = format!("{count} of {} ids", created.len());
        assert_eq!(
            described_ids(&router, &case, &indexed("VolumeId", &wanted)).await,
            wanted
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly the ids named"
        );
    }

    // One row per documented filter whose value this slice stores, plus the two families
    // that select nothing for an unattached, unmanaged volume.
    let rows: Vec<FilterRow> = vec![
        (
            "volume-type gp3",
            filters(&[("volume-type", &["gp3"])]),
            vec![created[0].clone()],
        ),
        (
            "volume-type gp2 or io1",
            filters(&[("volume-type", &["gp2", "io1"])]),
            vec![created[1].clone(), created[2].clone()],
        ),
        ("size 125", filters(&[("size", &["125"])]), vec![created[3].clone()]),
        (
            "availability-zone of the second zone",
            filters(&[("availability-zone", &[zone_name(1).as_str()])]),
            vec![created[1].clone(), created[4].clone()],
        ),
        (
            "availability-zone-id of the third zone",
            filters(&[("availability-zone-id", &[zone_id(2).as_str()])]),
            vec![created[2].clone()],
        ),
        (
            "encrypted true",
            filters(&[("encrypted", &["true"])]),
            vec![created[1].clone(), created[3].clone()],
        ),
        (
            "encrypted false",
            filters(&[("encrypted", &["false"])]),
            vec![created[0].clone(), created[2].clone(), created[4].clone()],
        ),
        (
            "fast-restored false",
            filters(&[("fast-restored", &["false"])]),
            created.clone(),
        ),
        (
            "multi-attach-enabled false",
            filters(&[("multi-attach-enabled", &["false"])]),
            created.clone(),
        ),
        (
            "volume-id of one volume",
            filters(&[("volume-id", &[created[2].as_str()])]),
            vec![created[2].clone()],
        ),
        (
            "two filters are joined with AND",
            filters(&[("encrypted", &["false"]), ("volume-type", &["standard"])]),
            vec![created[4].clone()],
        ),
        (
            "an attachment filter selects an unattached volume never",
            filters(&[("attachment.status", &["attached"])]),
            Vec::new(),
        ),
        (
            "an operator filter selects an unmanaged volume never",
            filters(&[("operator.managed", &["true"])]),
            Vec::new(),
        ),
        (
            "a snapshot filter selects a volume created empty never",
            filters(&[("snapshot-id", &["snap-00000000000000001"])]),
            Vec::new(),
        ),
    ];
    for (case, filter, expected) in rows {
        let mut reported = described_ids(&router, case, &filter).await;
        reported.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly {expected:?}, got {reported:?}"
        );
    }

    // A filter name this operation does not document is refused, never ignored.
    for name in ["volume-size", "tag:", "Encrypted", "status "] {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an undocumented filter, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // An id that names no volume is an error, not an absent row; an id that is not a volume
    // id at all is refused before it is looked up, and because this operation validates its
    // id LIST as a request parameter, that refusal is the parameter-level
    // `InvalidParameterValue` naming the parameter and the form it expects.
    for (case, volume_id, expected_code) in [
        ("unknown", "vol-00000000000000999", "InvalidVolume.NotFound"),
        ("malformed", "vol-not-hex", "InvalidParameterValue"),
        ("not an id at all", "banana", "InvalidParameterValue"),
        ("another resource's prefix", "snap-00000000000000999", "InvalidParameterValue"),
    ] {
        let (status, body) = describe(&router, &indexed("VolumeId", &[volume_id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Every page size from one volume per page to more than the whole collection walks
    // the same set exactly once.
    for page_size in [1usize, 2, 3, created.len(), created.len() + 1] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "volumeId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults volumes, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: paging must walk every volume exactly once, got {walked:?}"
        );
    }

    // A token this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "00", ""] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // A `MaxResults` that asks for a page of none is refused; one above the largest page
    // this service serves is not — the model declares no `@range` on the member.
    for (case, max_results) in [("zero", "0"), ("negative", "-1")] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
    for (case, max_results) in [("the cap", "1000"), ("past the cap", "1001")] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
    }
}

/// RULE 3 over the one caller-chosen string the model leaves unbounded: `KmsKeyId` is
/// accepted up to the length AWS KMS publishes for a key identifier and refused past it,
/// may be given only with `Encrypted=true`, and round-trips when it is accepted.
#[tokio::test]
async fn the_kms_key_identifier_is_bounded_and_round_trips() {
    let rows = [
        ("a key id", "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(), true, 200),
        ("an alias", "alias/ExampleAlias".to_string(), true, 200),
        (
            "a key identifier at the published bound",
            "a".repeat(MAX_KMS_KEY_ID_CHARS),
            true,
            200,
        ),
        (
            "one character past the bound",
            "a".repeat(MAX_KMS_KEY_ID_CHARS + 1),
            true,
            400,
        ),
        (
            "a key id without Encrypted=true",
            "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
            false,
            400,
        ),
    ];

    for (case, key_id, encrypted, expected) in rows {
        let router = emulator();
        let mut extra = vec![p("KmsKeyId", key_id.clone())];
        if encrypted {
            extra.push(p("Encrypted", "true"));
        }
        let (status, body) = create_volume(&router, "gp3", 4, extra).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with body {body}"
        );
        if expected != 200 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                described_ids(&router, case, &[]).await.is_empty(),
                "case {case:?}: a refused CreateVolume must store nothing"
            );
            continue;
        }
        assert_eq!(
            element(&body, "kmsKeyId").as_deref(),
            Some(key_id.as_str()),
            "case {case:?}: expected the volume to report the key it was given"
        );
        assert_eq!(
            element(&body, "encrypted").as_deref(),
            Some("true"),
            "case {case:?}: expected an encrypted volume, got {body}"
        );
        // ROUND TRIP: the key is stored, not just echoed.
        let volume_id = element(&body, "volumeId").unwrap_or_default();
        let (_, described) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
        assert_eq!(
            element(&described, "kmsKeyId").as_deref(),
            Some(key_id.as_str()),
            "case {case:?}: expected the stored key, got {described}"
        );
    }
}

/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, on both
/// operations; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_creates_and_describes_nothing() {
    let router = emulator();
    let (status, body) = create_volume(&router, "gp3", 4, vec![p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"CreateVolume DryRun\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"CreateVolume DryRun\": expected DryRunOperation, got {body}"
    );
    assert!(
        described_ids(&router, "CreateVolume DryRun", &[]).await.is_empty(),
        "case \"CreateVolume DryRun\": a dry run must create nothing"
    );

    // `DryRun=false` is not a dry run: the request is performed.
    let (status, body) = create_volume(&router, "gp3", 4, vec![p("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "case \"CreateVolume DryRun=false\": expected HTTP 200, got {status} / {body}"
    );
    let volume_id = element(&body, "volumeId").unwrap_or_default();

    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"DescribeVolumes DryRun\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"DescribeVolumes DryRun\": expected DryRunOperation, got {body}"
    );
    assert!(
        element(&body, "volumeId").is_none(),
        "case \"DescribeVolumes DryRun\": a dry run reports no volume, got {body}"
    );
    assert_eq!(
        described_ids(&router, "DescribeVolumes DryRun=false", &[p("DryRun", "false")]).await,
        vec![volume_id],
        "case \"DescribeVolumes DryRun=false\": expected the ordinary report"
    );
}
