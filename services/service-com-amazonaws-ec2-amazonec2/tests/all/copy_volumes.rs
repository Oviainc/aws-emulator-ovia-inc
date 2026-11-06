// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CopyVolumes`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A COPY IS A NEW VOLUME OF THE SOURCE'S DATA: a fresh id, ARN, account and create
//!    time, the SOURCE's Availability Zone, `SourceVolumeId` naming what it was copied
//!    from, and everything stored — `DescribeVolumes` reports it exactly as it reports a
//!    created volume;
//! 2. THE COPY IS AT LEAST AS LARGE AS ITS SOURCE: `Size` defaults to the source's size and
//!    any smaller size is refused with EC2's own `InvalidParameterValue`, while the copy's
//!    own volume TYPE bounds it from both ends;
//! 3. every other parameter follows the per-type rules the model publishes for the COPY's
//!    type — the same catalogs `CreateVolume` is held to — and the copy's encryption follows
//!    the source: an encrypted source is always copied encrypted;
//! 4. the CLOCK, not a caller, moves the copy from `creating` to `available`, and one
//!    `ClientToken` names one copy.
//!
//! Each table row is a distinct input along what the rule generalises over — the source's
//! type and size, the copy's type at each published size bound and one past it, the zone
//! the source sits in, which encryption combination, which IOPS/throughput rule — so no row
//! is the specification. The helpers a created volume is asserted with are reused from
//! [`super::volume_lifecycle`] rather than restated here.

use service_com_amazonaws_ec2_amazonec2::state::VOLUME_SIZE_RANGES;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};
use super::volume_lifecycle::{
    is_volume_id, p, past_the_transition, required_iops, zone_id, zone_name, REGION,
};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVolume", params).await
}

async fn copy(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CopyVolumes", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVolumes", params).await
}

/// Create one source volume of `volume_type` at `size` in the Region's zone `zone_index`,
/// with whatever extra parameters the case needs, and answer its id.
async fn source_volume(
    router: &MultiServiceRouter,
    case: &str,
    zone_index: usize,
    volume_type: &str,
    size: i64,
    extra: Params,
) -> String {
    let mut params = vec![
        p("AvailabilityZone", zone_name(zone_index)),
        p("VolumeType", volume_type),
        p("Size", size.to_string()),
    ];
    params.extend(required_iops(volume_type));
    params.extend(extra);
    let (status, body) = create(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 seeding the source volume, got {status} with body \
         {body}"
    );
    let volume_id = element(&body, "volumeId").unwrap_or_default();
    assert!(
        is_volume_id(&volume_id),
        "case {case:?}: expected a minted source volume id, got {volume_id:?}"
    );
    volume_id
}

/// `CopyVolumes` parameters naming `source_id`, plus whatever the case adds.
fn copy_params(source_id: &str, extra: Params) -> Params {
    let mut params = vec![p("SourceVolumeId", source_id)];
    params.extend(extra);
    params
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

/// RULE 1: the copy is a NEW volume that carries the source's data — its own id, ARN,
/// account and create time, the source's Availability Zone, `SourceVolumeId` naming the
/// source, and the source's size when the request names none — and it is STORED, so
/// `DescribeVolumes` reports it beside the source.
///
/// The rows differ along the source's zone, type and size, so neither the zone nor the size
/// can be a constant in the handler.
#[tokio::test]
async fn a_copy_is_a_new_stored_volume_in_the_sources_zone_naming_its_source() {
    // One row per (zone designator, source type, source size): three different zones of the
    // signed Region, three different source types, three different sizes.
    let rows: [(&str, usize, &str, i64); 4] = [
        ("a gp3 source in the first zone", 0, "gp3", 40),
        ("a gp2 source in the second zone", 1, "gp2", 8),
        ("an io2 source in the third zone", 2, "io2", 500),
        ("a standard source in the first zone", 0, "standard", 1),
    ];

    for (case, zone_index, volume_type, size) in rows {
        let router = emulator();
        let source_id = source_volume(&router, case, zone_index, volume_type, size, Vec::new())
            .await;

        let (status, body) = copy(&router, &copy_params(&source_id, Vec::new())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CopyVolumes, got {status} with body {body}"
        );
        let copy_id = element(&body, "volumeId").unwrap_or_default();
        assert!(
            is_volume_id(&copy_id),
            "case {case:?}: expected a minted volume id for the copy, got {copy_id:?}"
        );
        assert_ne!(
            copy_id, source_id,
            "case {case:?}: the copy is its own volume, not the source"
        );

        // The members the service derives, and the ones the copy takes from its source.
        for (member, expected) in [
            ("sourceVolumeId", source_id.clone()),
            ("availabilityZone", zone_name(zone_index)),
            ("availabilityZoneId", zone_id(zone_index)),
            ("size", size.to_string()),
            // "If not specified, the volume type defaults to gp2"
            // (`CopyVolumesRequest$VolumeType`).
            ("volumeType", "gp2".to_string()),
            ("status", "creating".to_string()),
            ("encrypted", "false".to_string()),
            ("ownerId", engine::EMULATOR_ACCOUNT_ID.to_string()),
            (
                "volumeArn",
                format!(
                    "arn:aws:ec2:{REGION}:{}:volume/{copy_id}",
                    engine::EMULATOR_ACCOUNT_ID
                ),
            ),
        ] {
            assert_eq!(
                element(&body, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {member}={expected:?}, got {:?}",
                element(&body, member)
            );
        }
        // A fresh copy is attached to nothing, and nothing was restored from a snapshot.
        assert_eq!(
            element(&body, "fastRestored").as_deref(),
            Some("false"),
            "case {case:?}: a copy of a volume restores no snapshot, got {body}"
        );
        assert_eq!(
            element(&body, "snapshotId").as_deref(),
            Some(""),
            "case {case:?}: a copy of a volume has no snapshot behind it, so its snapshotId \
             must be reported carrying nothing, got {body}"
        );

        // ROUND TRIP: both volumes are in the store, and the copy still names its source.
        let mut expected_ids = vec![copy_id.clone(), source_id.clone()];
        expected_ids.sort();
        let mut reported = described_ids(&router, case, &[]).await;
        reported.sort();
        assert_eq!(
            reported, expected_ids,
            "case {case:?}: expected the source and its copy to be stored, got {reported:?}"
        );
        let (_, described) = describe(&router, &indexed("VolumeId", &[&copy_id])).await;
        for (member, expected) in [
            ("sourceVolumeId", source_id.clone()),
            ("availabilityZone", zone_name(zone_index)),
            ("size", size.to_string()),
        ] {
            assert_eq!(
                element(&described, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected DescribeVolumes to report {member}={expected:?}, got {:?}",
                element(&described, member)
            );
        }
        // The source is untouched: it is nobody's copy.
        let (_, source_described) = describe(&router, &indexed("VolumeId", &[&source_id])).await;
        assert!(
            element(&source_described, "sourceVolumeId").is_none(),
            "case {case:?}: the source volume is not a copy, got {source_described}"
        );
    }
}

/// RULE 2: "The size must be equal to or greater than the size of the source volume. If not
/// specified, the size defaults to the size of the source volume"
/// (`CopyVolumesRequest$Size`).
///
/// The rows differ along the source's type and size and along how far the requested size
/// sits from it, including the two boundary cases — exactly the source's size, and one GiB
/// below it.
#[tokio::test]
async fn the_copy_is_never_smaller_than_the_source_it_copies() {
    // (source type, source size); the source's size is what every requested size is
    // measured against, so each row moves it.
    let sources: [(&str, i64); 5] = [
        ("gp3", 100),
        ("gp2", 8),
        ("io1", 400),
        ("standard", 1_024),
        ("st1", 125),
    ];

    for (volume_type, size) in sources {
        // (case, the `Size` the copy asks for, the size it must end up with or None when the
        // request must be refused).
        let requested: [(&str, Option<i64>, Option<i64>); 5] = [
            ("no Size at all", None, Some(size)),
            ("exactly the source's size", Some(size), Some(size)),
            ("one GiB above the source", Some(size + 1), Some(size + 1)),
            ("one GiB below the source", Some(size - 1), None),
            ("a single GiB", Some(1), if size == 1 { Some(1) } else { None }),
        ];

        for (requested_case, requested_size, expected_size) in requested {
            let case = format!("{volume_type} source of {size} GiB, {requested_case}");
            let router = emulator();
            let source_id =
                source_volume(&router, &case, 0, volume_type, size, Vec::new()).await;
            let extra = requested_size
                .map(|size| vec![p("Size", size.to_string())])
                .unwrap_or_default();
            let (status, body) = copy(&router, &copy_params(&source_id, extra)).await;

            match expected_size {
                Some(expected) => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected HTTP 200, got {status} with body {body}"
                    );
                    assert_eq!(
                        element(&body, "size").as_deref(),
                        Some(expected.to_string().as_str()),
                        "case {case:?}: expected size={expected}, got {:?}",
                        element(&body, "size")
                    );
                    // The size is STORED, not just answered.
                    let copy_id = element(&body, "volumeId").unwrap_or_default();
                    let (_, described) =
                        describe(&router, &indexed("VolumeId", &[&copy_id])).await;
                    assert_eq!(
                        element(&described, "size").as_deref(),
                        Some(expected.to_string().as_str()),
                        "case {case:?}: expected DescribeVolumes to report size={expected}, got {:?}",
                        element(&described, "size")
                    );
                }
                None => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} with body {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "InvalidParameterValue",
                        "case {case:?}: expected InvalidParameterValue, got {}",
                        error_code(&body)
                    );
                    // Nothing was stored: the source is still the only volume.
                    assert_eq!(
                        described_ids(&router, &case, &[]).await,
                        vec![source_id.clone()],
                        "case {case:?}: a refused copy stores nothing"
                    );
                }
            }
        }
    }
}

/// RULE 2, the other end: the copy's own volume TYPE bounds its size, at every bound the
/// model publishes (`CreateVolumeRequest$Size`, the catalog [`VOLUME_SIZE_RANGES`]) — so
/// every modelled type is exercised, including the ones the issue never cited.
#[tokio::test]
async fn every_copy_type_accepts_its_published_size_bounds_and_refuses_one_past_them() {
    for (volume_type, min, max) in VOLUME_SIZE_RANGES {
        let sizes: [(&str, i64, u16); 4] = [
            ("the minimum", i64::from(min), 200),
            ("the maximum", i64::from(max), 200),
            ("one below the minimum", i64::from(min) - 1, 400),
            ("one above the maximum", i64::from(max) + 1, 400),
        ];
        for (bound, size, expected_status) in sizes {
            let case = format!("a {volume_type} copy at {bound} ({size} GiB)");
            let router = emulator();
            // A one-GiB gp3 source, so the copy's own type — never the source's — is what
            // the requested size is bounded by.
            let source_id = source_volume(&router, &case, 0, "gp3", 1, Vec::new()).await;
            let mut extra = vec![p("VolumeType", volume_type), p("Size", size.to_string())];
            extra.extend(required_iops(volume_type));
            let (status, body) = copy(&router, &copy_params(&source_id, extra)).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
            );
            if expected_status == 200 {
                assert_eq!(
                    element(&body, "volumeType").as_deref(),
                    Some(volume_type),
                    "case {case:?}: expected volumeType={volume_type:?}, got {:?}",
                    element(&body, "volumeType")
                );
                continue;
            }
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {}",
                error_code(&body)
            );
        }
    }
}

/// RULE 1: `SourceVolumeId` must name a volume of the account — an id that is not shaped
/// like a volume id is malformed, and a well-formed id the account holds no volume for is
/// not found.
#[tokio::test]
async fn the_source_volume_must_be_one_the_account_holds() {
    // (case, the id the request names, the error code it is refused with)
    let rows: [(&str, &str, &str); 6] = [
        (
            "a long id no volume has",
            "vol-0123456789abcdef0",
            "InvalidVolume.NotFound",
        ),
        (
            "a short id no volume has",
            "vol-01234567",
            "InvalidVolume.NotFound",
        ),
        ("an id of the wrong length", "vol-012345", "InvalidVolumeID.Malformed"),
        (
            "an id with a non-hex digit",
            "vol-0123456789abcdefg",
            "InvalidVolumeID.Malformed",
        ),
        ("another resource's id", "snap-0123456789abcdef0", "InvalidVolumeID.Malformed"),
        (
            "an id with an upper-case digit",
            "vol-0123456789ABCDEF0",
            "InvalidVolumeID.Malformed",
        ),
    ];

    for (case, source_id, expected_code) in rows {
        let router = emulator();
        // A real volume exists, so the refusal is about the id the request named.
        let stored = source_volume(&router, case, 0, "gp3", 4, Vec::new()).await;
        let (status, body) = copy(&router, &copy_params(source_id, Vec::new())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&body)
        );
        assert_eq!(
            described_ids(&router, case, &[]).await,
            vec![stored.clone()],
            "case {case:?}: a refused copy stores nothing"
        );
    }
}

/// RULE 3: the copy's IOPS, throughput and Multi-Attach follow the rules the model
/// publishes for the COPY's volume type, and the accepted values are stored.
#[tokio::test]
async fn iops_throughput_and_multi_attach_follow_the_copy_types_rules() {
    struct Row {
        case: &'static str,
        extra: Params,
        status: u16,
        code: &'static str,
        iops: Option<&'static str>,
        throughput: Option<&'static str>,
    }

    let rows = vec![
        Row {
            // "gp3: 3,000 (default)" (`CopyVolumesRequest$Iops`), and gp3 is the one type
            // throughput applies to.
            case: "a gp3 copy that names neither",
            extra: vec![p("VolumeType", "gp3")],
            status: 200,
            code: "",
            iops: Some("3000"),
            throughput: Some("125"),
        },
        Row {
            case: "a gp3 copy at the top of its IOPS range",
            extra: vec![p("VolumeType", "gp3"), p("Iops", "80000")],
            status: 200,
            code: "",
            iops: Some("80000"),
            throughput: Some("125"),
        },
        Row {
            case: "a gp3 copy one IOPS past its range",
            extra: vec![p("VolumeType", "gp3"), p("Iops", "80001")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            case: "a gp3 copy at the top of its throughput range",
            extra: vec![p("VolumeType", "gp3"), p("Throughput", "2000")],
            status: 200,
            code: "",
            iops: Some("3000"),
            throughput: Some("2000"),
        },
        Row {
            case: "a gp3 copy one MiB/s below its throughput range",
            extra: vec![p("VolumeType", "gp3"), p("Throughput", "124")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            // "Omit for all other volume types" (`CopyVolumesRequest$Iops`).
            case: "a gp2 copy that provisions IOPS",
            extra: vec![p("VolumeType", "gp2"), p("Iops", "100")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            // "Supported for gp3 volumes only" (`CopyVolumesRequest$Throughput`).
            case: "a standard copy that provisions throughput",
            extra: vec![p("VolumeType", "standard"), p("Throughput", "125")],
            status: 400,
            code: "InvalidParameterValue",
            iops: None,
            throughput: None,
        },
        Row {
            // "Required for io1 and io2 volumes" (`CopyVolumesRequest$Iops`).
            case: "an io1 copy that names no IOPS",
            extra: vec![p("VolumeType", "io1")],
            status: 400,
            code: "MissingParameter",
            iops: None,
            throughput: None,
        },
        Row {
            case: "an io1 copy at the bottom of its IOPS range",
            extra: vec![p("VolumeType", "io1"), p("Iops", "100")],
            status: 200,
            code: "",
            iops: Some("100"),
            throughput: None,
        },
        Row {
            case: "an io2 copy at the top of its IOPS range",
            extra: vec![p("VolumeType", "io2"), p("Iops", "256000")],
            status: 200,
            code: "",
            iops: Some("256000"),
            throughput: None,
        },
        Row {
            // "Supported with io1 and io2 volumes only"
            // (`CopyVolumesRequest$MultiAttachEnabled`).
            case: "a gp2 copy with Multi-Attach",
            extra: vec![p("VolumeType", "gp2"), p("MultiAttachEnabled", "true")],
            status: 400,
            code: "InvalidParameterCombination",
            iops: None,
            throughput: None,
        },
        Row {
            case: "an io2 copy with Multi-Attach",
            extra: vec![
                p("VolumeType", "io2"),
                p("Iops", "100"),
                p("MultiAttachEnabled", "true"),
            ],
            status: 200,
            code: "",
            iops: Some("100"),
            throughput: None,
        },
    ];

    for row in rows {
        let router = emulator();
        // A 400 GiB gp3 source: the copy's default size is inside every modelled type's
        // range, so each row tests only the parameter it names.
        let source_id = source_volume(&router, row.case, 0, "gp3", 400, Vec::new()).await;
        let (status, body) = copy(&router, &copy_params(&source_id, row.extra)).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} with body {body}",
            row.case, row.status
        );
        if row.status != 200 {
            assert_eq!(
                error_code(&body),
                row.code,
                "case {:?}: expected {}, got {}",
                row.case,
                row.code,
                error_code(&body)
            );
            assert_eq!(
                described_ids(&router, row.case, &[]).await,
                vec![source_id.clone()],
                "case {:?}: a refused copy stores nothing",
                row.case
            );
            continue;
        }
        for (member, expected) in [("iops", row.iops), ("throughput", row.throughput)] {
            assert_eq!(
                element(&body, member).as_deref(),
                expected,
                "case {:?}: expected {member}={expected:?}, got {:?}",
                row.case,
                element(&body, member)
            );
        }
        // The derived members are stored, not just answered.
        let copy_id = element(&body, "volumeId").unwrap_or_default();
        let (_, described) = describe(&router, &indexed("VolumeId", &[&copy_id])).await;
        assert_eq!(
            element(&described, "iops").as_deref(),
            row.iops,
            "case {:?}: expected DescribeVolumes to report iops={:?}, got {:?}",
            row.case,
            row.iops,
            element(&described, "iops")
        );
    }
}

/// RULE 3: "If the source volume is encrypted, the service always encrypts the copy
/// regardless of this value. Set to true to encrypt a copy of an unencrypted source volume"
/// (`CopyVolumesRequest$Encrypted`) — so the copy is encrypted when the request asks for it
/// OR the source already is, and a KMS key may only be named for a copy that is encrypted.
#[tokio::test]
async fn the_copy_is_encrypted_when_the_request_or_the_source_says_so() {
    /// The key a source volume is created under, and the one a copy may re-key to.
    const SOURCE_KEY: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";
    const COPY_KEY: &str = "alias/ExampleAlias";

    // (case, the source is encrypted, the copy's extra parameters, expected status,
    //  expected `encrypted`, expected `kmsKeyId`)
    type EncryptionRow = (
        &'static str,
        bool,
        Params,
        u16,
        &'static str,
        Option<&'static str>,
    );
    let rows: Vec<EncryptionRow> = vec![
        (
            "an unencrypted source copied as-is",
            false,
            Vec::new(),
            200,
            "false",
            None,
        ),
        (
            "an unencrypted source copied with Encrypted=true",
            false,
            vec![p("Encrypted", "true")],
            200,
            "true",
            None,
        ),
        (
            "an unencrypted source copied under a named key",
            false,
            vec![p("Encrypted", "true"), p("KmsKeyId", COPY_KEY)],
            200,
            "true",
            Some(COPY_KEY),
        ),
        (
            "an unencrypted source copied under a key without Encrypted",
            false,
            vec![p("KmsKeyId", COPY_KEY)],
            400,
            "",
            None,
        ),
        (
            "an unencrypted source copied with Encrypted=false and a key",
            false,
            vec![p("Encrypted", "false"), p("KmsKeyId", COPY_KEY)],
            400,
            "",
            None,
        ),
        (
            "an encrypted source copied as-is",
            true,
            Vec::new(),
            200,
            "true",
            Some(SOURCE_KEY),
        ),
        (
            "an encrypted source copied with Encrypted=false",
            true,
            vec![p("Encrypted", "false")],
            200,
            "true",
            Some(SOURCE_KEY),
        ),
        (
            "an encrypted source copied under another key",
            true,
            vec![p("KmsKeyId", COPY_KEY)],
            200,
            "true",
            Some(COPY_KEY),
        ),
    ];

    for (case, source_encrypted, extra, expected_status, expected_encrypted, expected_key) in rows
    {
        let router = emulator();
        let seed = if source_encrypted {
            vec![p("Encrypted", "true"), p("KmsKeyId", SOURCE_KEY)]
        } else {
            Vec::new()
        };
        let source_id = source_volume(&router, case, 0, "gp3", 4, seed).await;
        let (status, body) = copy(&router, &copy_params(&source_id, extra)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {}",
                error_code(&body)
            );
            continue;
        }
        let copy_id = element(&body, "volumeId").unwrap_or_default();
        let (_, described) = describe(&router, &indexed("VolumeId", &[&copy_id])).await;
        for (source, reported) in [("CopyVolumes", &body), ("DescribeVolumes", &described)] {
            assert_eq!(
                element(reported, "encrypted").as_deref(),
                Some(expected_encrypted),
                "case {case:?}: expected {source} to report encrypted={expected_encrypted:?}, \
                 got {:?}",
                element(reported, "encrypted")
            );
            assert_eq!(
                element(reported, "kmsKeyId").as_deref(),
                expected_key,
                "case {case:?}: expected {source} to report kmsKeyId={expected_key:?}, got {:?}",
                element(reported, "kmsKeyId")
            );
        }
    }
}

/// RULE 1: "The tags to apply to the volume copy during creation"
/// (`CopyVolumesRequest$TagSpecifications`) — the REQUEST says which tags the copy carries,
/// so the source's tags are not inherited, and a specification naming any other resource
/// type is refused rather than accepted and discarded.
#[tokio::test]
async fn tag_specifications_tag_the_copy_and_nothing_else() {
    let router = emulator();
    let source_id = source_volume(
        &router,
        "a tagged source",
        0,
        "gp3",
        4,
        vec![
            p("TagSpecification.1.ResourceType", "volume"),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "source"),
        ],
    )
    .await;

    // A copy that names no tags carries none: the source's tags are the source's.
    let (status, body) = copy(&router, &copy_params(&source_id, Vec::new())).await;
    assert_eq!(
        status, 200,
        "case \"an untagged copy of a tagged source\": expected HTTP 200, got {status} / {body}"
    );
    assert!(
        elements(&body, "key").is_empty(),
        "case \"an untagged copy of a tagged source\": expected no tags, got {body}"
    );

    // A copy that names tags carries exactly those.
    let tags = [("Name", "copy"), ("Owner", "TeamB"), ("Stage", "test")];
    let mut extra = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let nth = index + 1;
        extra.push(p("TagSpecification.1.ResourceType", "volume"));
        extra.push(p(&format!("TagSpecification.1.Tag.{nth}.Key"), *key));
        extra.push(p(&format!("TagSpecification.1.Tag.{nth}.Value"), *value));
    }
    let (status, body) = copy(&router, &copy_params(&source_id, extra)).await;
    assert_eq!(
        status, 200,
        "case \"three tags on the copy\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "key"),
        vec!["Name", "Owner", "Stage"],
        "case \"three tags on the copy\": expected the keys it was given, got {body}"
    );
    assert_eq!(
        elements(&body, "value"),
        vec!["copy", "TeamB", "test"],
        "case \"three tags on the copy\": expected the values it was given, got {body}"
    );
    // ROUND TRIP: the tags are stored on the copy, not echoed.
    let copy_id = element(&body, "volumeId").unwrap_or_default();
    let (_, described) = describe(&router, &indexed("VolumeId", &[&copy_id])).await;
    assert_eq!(
        elements(&described, "key"),
        vec!["Name", "Owner", "Stage"],
        "case \"three tags on the copy\": expected the stored tags, got {described}"
    );

    // A specification for anything but the volume it creates is refused.
    for resource_type in ["instance", "snapshot", "launch-template"] {
        let refused = vec![
            p("TagSpecification.1.ResourceType", resource_type),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "elsewhere"),
        ];
        let (status, body) = copy(&router, &copy_params(&source_id, refused)).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400 for a foreign resourceType, got {status} \
             / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {resource_type:?}: expected InvalidParameterValue, got {}",
            error_code(&body)
        );
    }
}

/// RULE 4: "The volume copy can be attached to an Amazon EC2 instance once it reaches the
/// available state" (`CopyVolumes`) — the copy is handed back `creating`, stays `creating`
/// for the service's own transition span, and the CLOCK, not a caller, moves it to
/// `available`.
#[tokio::test]
async fn a_copy_is_creating_until_the_clock_makes_it_available() {
    let router = emulator();
    let mut copy_ids = Vec::new();

    for (volume_type, size) in [("gp3", 40), ("gp2", 8), ("standard", 2)] {
        let case = format!("a {volume_type} source of {size} GiB");
        let source_id = source_volume(&router, &case, 0, volume_type, size, Vec::new()).await;
        let (status, body) = copy(&router, &copy_params(&source_id, Vec::new())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "status").as_deref(),
            Some("creating"),
            "case {case:?}: expected a fresh copy to be creating, got {body}"
        );
        copy_ids.push(element(&body, "volumeId").unwrap_or_default());
    }

    // Inside the span the copy is still creating, however often it is read.
    for copy_id in &copy_ids {
        let (status, body) = describe(&router, &indexed("VolumeId", &[copy_id])).await;
        assert_eq!(status, 200, "expected HTTP 200, got {status} with body {body}");
        assert_eq!(
            element(&body, "status").as_deref(),
            Some("creating"),
            "expected {copy_id} to still be creating, got {body}"
        );
    }

    past_the_transition().await;

    for copy_id in &copy_ids {
        for read in 1..=2 {
            let (status, body) = describe(&router, &indexed("VolumeId", &[copy_id])).await;
            assert_eq!(
                status, 200,
                "read {read}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "status").as_deref(),
                Some("available"),
                "read {read}: expected {copy_id} to be available once the transition is due, \
                 got {body}"
            );
        }
    }
}

/// RULE 4: "Unique, case-sensitive identifier that you provide to ensure the idempotency of
/// the request" (`CopyVolumesRequest$ClientToken`) — a replay with the same token reports
/// the copy that token already named and creates nothing; a different token copies again.
#[tokio::test]
async fn one_client_token_names_one_copy() {
    let router = emulator();
    let source_id = source_volume(&router, "the source", 0, "gp3", 40, Vec::new()).await;

    let mut minted = Vec::new();
    for token in ["token-0001", "a-second-token"] {
        let first = copy(
            &router,
            &copy_params(&source_id, vec![p("ClientToken", token)]),
        )
        .await;
        let second = copy(
            &router,
            &copy_params(&source_id, vec![p("ClientToken", token), p("Size", "80")]),
        )
        .await;
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
            "case {token:?}: one token names one copy, got {} then {}",
            first.1,
            second.1
        );
        // The replay reports the STORED copy, not the parameters it was replayed with.
        assert_eq!(
            element(&second.1, "size").as_deref(),
            Some("40"),
            "case {token:?}: expected the replay to report the stored size, got {}",
            second.1
        );
        minted.push(element(&first.1, "volumeId").unwrap_or_default());
    }

    let stored = described_ids(&router, "two tokens, two copies", &[]).await;
    assert_eq!(
        stored.len(),
        3,
        "case \"two tokens, two copies\": expected the source and two copies, got {stored:?}"
    );
    for copy_id in &minted {
        assert!(
            stored.contains(copy_id),
            "case \"two tokens, two copies\": expected {copy_id} to be stored, got {stored:?}"
        );
    }
}

/// RULE 1: "Checks whether you have the required permissions for the action, without
/// actually making the request" (`CopyVolumesRequest$DryRun`) — a dry run that would have
/// worked answers `DryRunOperation` and copies nothing, and one that would have been refused
/// is refused with THAT reason instead.
#[tokio::test]
async fn dry_run_copies_nothing() {
    let router = emulator();
    let source_id = source_volume(&router, "the source", 0, "gp3", 40, Vec::new()).await;

    let (status, body) = copy(
        &router,
        &copy_params(&source_id, vec![p("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"a dry run that would have worked\": expected HTTP 412, got {status} / {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry run that would have worked\": expected DryRunOperation, got {}",
        error_code(&body)
    );

    // A dry run of a request that would have been refused answers the refusal, not 412.
    let (status, body) = copy(
        &router,
        &copy_params(&source_id, vec![p("DryRun", "true"), p("Size", "39")]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a dry run of an undersized copy\": expected HTTP 400, got {status} / {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"a dry run of an undersized copy\": expected InvalidParameterValue, got {}",
        error_code(&body)
    );

    assert_eq!(
        described_ids(&router, "after two dry runs", &[]).await,
        vec![source_id.clone()],
        "case \"after two dry runs\": a dry run stores nothing"
    );

    // `DryRun=false` is not a dry run: the copy is made.
    let (status, body) = copy(
        &router,
        &copy_params(&source_id, vec![p("DryRun", "false")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        is_volume_id(&element(&body, "volumeId").unwrap_or_default()),
        "case \"DryRun=false\": expected a minted copy, got {body}"
    );
    assert_eq!(
        described_ids(&router, "after DryRun=false", &[]).await.len(),
        2,
        "case \"after DryRun=false\": expected the source and one copy"
    );
}
