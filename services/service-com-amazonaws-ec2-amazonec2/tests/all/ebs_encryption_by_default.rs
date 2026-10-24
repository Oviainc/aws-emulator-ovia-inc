// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's EBS encryption-by-default setting —
//! `GetEbsEncryptionByDefault`, `EnableEbsEncryptionByDefault`,
//! `DisableEbsEncryptionByDefault` — and the `CreateVolume` default it decides, driven
//! over the wire the service speaks (`ec2Query`, through [`super::ec2_query`]).
//!
//! The rules under test are:
//!
//! 1. REPORTING: `GetEbsEncryptionByDefault` answers the setting of the Region the request
//!    is signed for and writes nothing — it is off until the account enables it, and one
//!    read never changes what a later read sees;
//! 2. ROUND TRIP: the enable and the disable SET the setting, each answering "the updated
//!    status of encryption by default" read back out of the store, and both are
//!    idempotent;
//! 3. REGION SCOPE: the setting is a fact "for your account in the current Region", so
//!    setting it in one Region leaves every other Region's where it was;
//! 4. a `DryRun` request performs nothing and is answered 412 `DryRunOperation`;
//! 5. `CreateVolume`'s encryption state is the request's `Encrypted` RAISED by the
//!    account's setting: once the setting is on, a volume cannot be created unencrypted.
//!
//! Each table row is a distinct input along what its rule generalises over — which
//! Region, which sequence of writes, which operation carries the `DryRun`, which volume
//! type, and whether the request asks for encryption, asks against it, or says nothing —
//! so no row is the specification.

use service_com_amazonaws_ec2_amazonec2::state::{
    availability_zones_in_region, MAX_ENCRYPTION_BY_DEFAULT_REGIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};

/// Regions spanning the partitions and the naming shapes the Region field takes — a
/// three-part name, a four-part one, and a non-commercial partition — so no assertion
/// below is a property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The setting as `GetEbsEncryptionByDefault` reports it in `region`, with the HTTP status
/// and body so a failing case can name them.
async fn get(router: &MultiServiceRouter, region: &str) -> (u16, String, Option<String>) {
    let (status, body) = call(router, region, "GetEbsEncryptionByDefault", &[]).await;
    let reported = element(&body, "ebsEncryptionByDefault");
    (status, body, reported)
}

/// The setting `GetEbsEncryptionByDefault` reports in `region`, asserting the call
/// succeeded.
async fn reported(router: &MultiServiceRouter, case: &str, region: &str) -> String {
    let (status, body, reported) = get(router, region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetEbsEncryptionByDefault in {region}, \
         got {status} with body {body}"
    );
    reported.unwrap_or_else(|| {
        panic!("case {case:?}: expected an ebsEncryptionByDefault member, got {body}")
    })
}

/// `EnableEbsEncryptionByDefault` (`enable` true) or `DisableEbsEncryptionByDefault`, in
/// `region`, with whatever extra parameters the case needs.
async fn set(
    router: &MultiServiceRouter,
    region: &str,
    enable: bool,
    extra: &[(String, String)],
) -> (u16, String) {
    let action = if enable {
        "EnableEbsEncryptionByDefault"
    } else {
        "DisableEbsEncryptionByDefault"
    };
    call(router, region, action, extra).await
}

/// RULE 1: a fresh account has the setting off in every Region, and asking twice reports
/// the same value — the read is reporting-only.
#[tokio::test]
async fn a_fresh_account_reports_the_setting_off_in_every_region() {
    let router = emulator();
    for region in REGIONS {
        let case = format!("a fresh account in {region}");
        let first = reported(&router, &case, region).await;
        let second = reported(&router, &case, region).await;
        assert_eq!(
            first, "false",
            "case {case:?}: expected the setting off, got {first:?}"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected a read to leave the setting at {first:?}, \
             got {second:?} on the next read"
        );
        // "Reserved for future use" (`GetEbsEncryptionByDefaultResult$SseType`): nothing
        // sets it, so nothing reports it.
        let (_, body, _) = get(&router, region).await;
        assert!(
            element(&body, "sseType").is_none(),
            "case {case:?}: expected no sseType member, got {body}"
        );
    }
}

/// RULES 2 and 3: every sequence of writes leaves the setting at what its LAST write said,
/// each write answers that updated status, a later read agrees, and a repeat of the same
/// write changes nothing. Run in each Region of [`REGIONS`], so the sequence is not a
/// property of one.
#[tokio::test]
async fn the_setting_follows_the_sequence_of_writes_and_reads_back() {
    // Each row is a sequence of writes: `true` enables, `false` disables.
    let sequences: [(&str, &[bool]); 6] = [
        ("one enable", &[true]),
        ("one disable in a Region it was never on in", &[false]),
        ("enable twice", &[true, true]),
        ("enable then disable", &[true, false]),
        ("disable twice", &[false, false]),
        ("enable, disable, enable", &[true, false, true]),
    ];

    for (case, sequence) in sequences {
        for region in REGIONS {
            let router = emulator();
            let case = format!("{case} in {region}");
            for (step, enable) in sequence.iter().enumerate() {
                let expected = if *enable { "true" } else { "false" };
                let (status, body) = set(&router, region, *enable, &[]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
                );
                assert_eq!(
                    element(&body, "ebsEncryptionByDefault").as_deref(),
                    Some(expected),
                    "case {case:?} step {step}: expected the updated status {expected:?}, \
                     got {body}"
                );
                // ROUND TRIP: the write is stored, not just echoed.
                let read_back = reported(&router, &case, region).await;
                assert_eq!(
                    read_back, expected,
                    "case {case:?} step {step}: expected the stored setting {expected:?}, \
                     got {read_back:?}"
                );
            }
        }
    }
}

/// RULE 3: the setting belongs to the Region the request is signed for, so one Region's
/// writes leave every other Region's setting alone.
#[tokio::test]
async fn the_setting_is_scoped_to_the_region_it_was_set_in() {
    let router = emulator();
    // Enable in two of the four Regions, leave the others alone, then disable one again.
    for region in [REGIONS[0], REGIONS[2]] {
        let (status, body) = set(&router, region, true, &[]).await;
        assert_eq!(
            status, 200,
            "case \"enable in {region}\": expected HTTP 200, got {status} with body {body}"
        );
    }
    for region in REGIONS {
        let expected = if region == REGIONS[0] || region == REGIONS[2] {
            "true"
        } else {
            "false"
        };
        let case = format!("two Regions enabled, asking in {region}");
        let read = reported(&router, &case, region).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected {expected:?}, got {read:?}"
        );
    }

    let (status, body) = set(&router, REGIONS[0], false, &[]).await;
    assert_eq!(
        status, 200,
        "case \"disable in {}\": expected HTTP 200, got {status} with body {body}",
        REGIONS[0]
    );
    for region in REGIONS {
        let expected = if region == REGIONS[2] { "true" } else { "false" };
        let case = format!("one Region disabled again, asking in {region}");
        let read = reported(&router, &case, region).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected {expected:?}, got {read:?}"
        );
    }
}

/// The store holds the setting for [`MAX_ENCRYPTION_BY_DEFAULT_REGIONS`] Regions: the
/// enable that would hold one more is refused, a Region already stored costs nothing and
/// is still accepted, and a disable gives the slot back.
#[tokio::test]
async fn the_number_of_regions_the_setting_is_held_for_is_bounded() {
    let router = emulator();
    // Synthetic Region names of the shape the credential scope accepts, one per slot.
    let held: Vec<String> = (0..MAX_ENCRYPTION_BY_DEFAULT_REGIONS)
        .map(|nth| format!("ap-slot{nth}-1"))
        .collect();
    for region in &held {
        let (status, body) = set(&router, region, true, &[]).await;
        assert_eq!(
            status, 200,
            "case \"filling the store, {region}\": expected HTTP 200, \
             got {status} with body {body}"
        );
    }

    // A Region the store already holds costs nothing, so re-enabling it still succeeds.
    let (status, body) = set(&router, &held[0], true, &[]).await;
    assert_eq!(
        status, 200,
        "case \"re-enable a Region the store holds\": expected HTTP 200, \
         got {status} with body {body}"
    );

    // One Region past the ceiling is refused, and nothing is stored for it.
    let past = "ap-slot9999-1";
    let (status, body) = set(&router, past, true, &[]).await;
    assert_eq!(
        status, 503,
        "case \"one Region past the ceiling\": expected HTTP 503, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "RequestLimitExceeded",
        "case \"one Region past the ceiling\": expected RequestLimitExceeded, got {body}"
    );
    let read = reported(&router, "one Region past the ceiling", past).await;
    assert_eq!(
        read, "false",
        "case \"one Region past the ceiling\": expected a refused enable to store nothing, \
         got {read:?}"
    );

    // A disable gives the slot back, so the Region that was refused now fits.
    let (status, body) = set(&router, &held[0], false, &[]).await;
    assert_eq!(
        status, 200,
        "case \"free a slot\": expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = set(&router, past, true, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the freed slot admits the Region\": expected HTTP 200, \
         got {status} with body {body}"
    );
    let read = reported(&router, "the freed slot admits the Region", past).await;
    assert_eq!(
        read, "true",
        "case \"the freed slot admits the Region\": expected the setting on, got {read:?}"
    );
}

/// RULE 4: `DryRun` is answered 412 `DryRunOperation` on all three operations and leaves
/// the setting exactly where it was, whichever value it already held.
#[tokio::test]
async fn dry_run_reports_and_changes_nothing() {
    // `(case, the setting before the call, the dry-run call, the action it names)`.
    let rows: [(&str, bool, &str); 6] = [
        ("enable, setting off", false, "EnableEbsEncryptionByDefault"),
        ("enable, setting on", true, "EnableEbsEncryptionByDefault"),
        (
            "disable, setting off",
            false,
            "DisableEbsEncryptionByDefault",
        ),
        ("disable, setting on", true, "DisableEbsEncryptionByDefault"),
        ("get, setting off", false, "GetEbsEncryptionByDefault"),
        ("get, setting on", true, "GetEbsEncryptionByDefault"),
    ];
    let region = REGIONS[1];

    for (case, enabled_before, action) in rows {
        let router = emulator();
        if enabled_before {
            let (status, body) = set(&router, region, true, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 enabling the setting, \
                 got {status} with body {body}"
            );
        }
        let expected = if enabled_before { "true" } else { "false" };

        let (status, body) = call(&router, region, action, &[p("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun {action}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        let read = reported(&router, case, region).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected a DryRun to leave the setting at {expected:?}, \
             got {read:?}"
        );
    }

    // `DryRun=false` is an ordinary call.
    let router = emulator();
    let (status, body) = set(&router, region, true, &[p("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    let read = reported(&router, "DryRun=false", region).await;
    assert_eq!(
        read, "true",
        "case \"DryRun=false\": expected the setting on, got {read:?}"
    );
}

/// RULE 5: the encryption state of a created volume is the request's `Encrypted` raised by
/// the account's setting — once the setting is on, a volume cannot be created unencrypted —
/// and the state the create reports is the state `DescribeVolumes` reports.
#[tokio::test]
async fn create_volume_takes_its_encryption_state_from_the_account_setting() {
    struct Row {
        case: &'static str,
        enabled: bool,
        /// What the request says about `Encrypted`, or nothing at all.
        requested: Option<&'static str>,
        volume_type: &'static str,
        size: i64,
        expected: &'static str,
    }
    // One row per (setting, request) pair, each on a different volume type so the rule is
    // not read off one type.
    let rows = [
        Row {
            case: "setting off, request silent",
            enabled: false,
            requested: None,
            volume_type: "gp2",
            size: 8,
            expected: "false",
        },
        Row {
            case: "setting off, request asks against encryption",
            enabled: false,
            requested: Some("false"),
            volume_type: "gp3",
            size: 1,
            expected: "false",
        },
        Row {
            case: "setting off, request asks for encryption",
            enabled: false,
            requested: Some("true"),
            volume_type: "standard",
            size: 2,
            expected: "true",
        },
        Row {
            case: "setting on, request silent",
            enabled: true,
            requested: None,
            volume_type: "gp3",
            size: 4,
            expected: "true",
        },
        Row {
            case: "setting on, request asks against encryption",
            enabled: true,
            requested: Some("false"),
            volume_type: "st1",
            size: 125,
            expected: "true",
        },
        Row {
            case: "setting on, request asks for encryption",
            enabled: true,
            requested: Some("true"),
            volume_type: "gp2",
            size: 3,
            expected: "true",
        },
    ];

    let region = REGIONS[0];
    let zone = availability_zones_in_region(region)[0].zone_name.clone();

    for row in rows {
        let router = emulator();
        if row.enabled {
            let (status, body) = set(&router, region, true, &[]).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200 enabling the setting, got {status} with body {body}",
                row.case
            );
        }
        let mut params = vec![
            p("AvailabilityZone", zone.clone()),
            p("VolumeType", row.volume_type),
            p("Size", row.size.to_string()),
        ];
        if let Some(requested) = row.requested {
            params.push(p("Encrypted", requested));
        }
        let (status, body) = call(&router, region, "CreateVolume", &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}",
            row.case
        );
        assert_eq!(
            element(&body, "encrypted").as_deref(),
            Some(row.expected),
            "case {:?}: expected encrypted {:?}, got {body}",
            row.case,
            row.expected
        );
        // ROUND TRIP: the state is stored, not just answered.
        let volume_id = element(&body, "volumeId").unwrap_or_default();
        let (status, described) = call(
            &router,
            region,
            "DescribeVolumes",
            &indexed("VolumeId", &[&volume_id]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DescribeVolumes, got {status} with body {described}",
            row.case
        );
        assert_eq!(
            element(&described, "encrypted").as_deref(),
            Some(row.expected),
            "case {:?}: expected the stored volume to report encrypted {:?}, got {described}",
            row.case,
            row.expected
        );
    }
}

/// RULE 5 at the parameter that depends on the REQUEST's encryption flag: "If KmsKeyId is
/// specified, the encrypted state must be true" (`CreateVolumeRequest$KmsKeyId`) is a
/// parameter dependency of the request, so a key the request does not pair with
/// `Encrypted=true` is refused whatever the account's setting is — the setting raises the
/// stored volume's encryption state, it does not stand in for the missing parameter.
#[tokio::test]
async fn a_kms_key_needs_the_requests_own_encrypted_flag_whatever_the_setting_says() {
    // `(case, the setting, what the request says about Encrypted, status, error code)`.
    let rows: [(&str, bool, Option<&str>, u16, &str); 5] = [
        ("setting off, key alone", false, None, 400, "InvalidParameterValue"),
        ("setting on, key alone", true, None, 400, "InvalidParameterValue"),
        (
            "setting on, key with Encrypted=false",
            true,
            Some("false"),
            400,
            "InvalidParameterValue",
        ),
        ("setting off, key with Encrypted=true", false, Some("true"), 200, ""),
        ("setting on, key with Encrypted=true", true, Some("true"), 200, ""),
    ];
    let region = REGIONS[3];
    let zone = availability_zones_in_region(region)[0].zone_name.clone();
    let key_id = "alias/aws/ebs";

    for (case, enabled, requested, expected_status, expected_code) in rows {
        let router = emulator();
        if enabled {
            let (status, body) = set(&router, region, true, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 enabling the setting, \
                 got {status} with body {body}"
            );
        }
        let mut params = vec![
            p("AvailabilityZone", zone.clone()),
            p("VolumeType", "gp3"),
            p("Size", "4"),
            p("KmsKeyId", key_id),
        ];
        if let Some(requested) = requested {
            params.push(p("Encrypted", requested));
        }
        let (status, body) = call(&router, region, "CreateVolume", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from CreateVolume, \
             got {status} with body {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
            continue;
        }
        assert_eq!(
            element(&body, "encrypted").as_deref(),
            Some("true"),
            "case {case:?}: expected an encrypted volume, got {body}"
        );
        assert_eq!(
            element(&body, "kmsKeyId").as_deref(),
            Some(key_id),
            "case {case:?}: expected the volume to report the key it was given, got {body}"
        );
    }
}

/// RULE 5 reaches every operation that CREATES an EBS volume, so `CopyVolumes` takes the
/// setting the same way `CreateVolume` does: a copy of an UNENCRYPTED source is unencrypted
/// while the setting is off and encrypted once it is on, without the request asking.
#[tokio::test]
async fn a_volume_copy_takes_the_encryption_state_from_the_account_setting() {
    // `(case, the setting when the copy is made, the state the copy must report)`.
    let rows: [(&str, bool, &str); 2] = [
        ("setting off when the copy is made", false, "false"),
        ("setting on when the copy is made", true, "true"),
    ];
    let region = REGIONS[0];
    let zone = availability_zones_in_region(region)[0].zone_name.clone();

    for (case, enabled, expected) in rows {
        let router = emulator();
        // The SOURCE is created while the setting is off, so it is unencrypted whichever
        // row this is: the copy's state can then only come from the setting.
        let (status, body) = call(
            &router,
            region,
            "CreateVolume",
            &[
                p("AvailabilityZone", zone.clone()),
                p("VolumeType", "gp2"),
                p("Size", "8"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 creating the source, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "encrypted").as_deref(),
            Some("false"),
            "case {case:?}: expected an unencrypted source, got {body}"
        );
        let source_id = element(&body, "volumeId").unwrap_or_default();

        if enabled {
            let (status, body) = set(&router, region, true, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 enabling the setting, \
                 got {status} with body {body}"
            );
        }

        let (status, body) = call(
            &router,
            region,
            "CopyVolumes",
            &[
                p("SourceVolumeId", source_id.clone()),
                p("AvailabilityZone", zone.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CopyVolumes, got {status} with body {body}"
        );
        let copy_id = elements(&body, "volumeId")
            .into_iter()
            .find(|id| *id != source_id)
            .unwrap_or_default();
        assert!(
            !copy_id.is_empty(),
            "case {case:?}: expected CopyVolumes to report a volume other than the source, \
             got {body}"
        );
        // ROUND TRIP on the COPY's own id, so the state asserted is the copy's and not the
        // first `encrypted` element of a response that may describe more than one volume.
        let (status, described) = call(
            &router,
            region,
            "DescribeVolumes",
            &indexed("VolumeId", &[&copy_id]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeVolumes, \
             got {status} with body {described}"
        );
        assert_eq!(
            element(&described, "encrypted").as_deref(),
            Some(expected),
            "case {case:?}: expected the stored copy to report encrypted {expected:?}, \
             got {described}"
        );
        // The source is untouched: "disabling encryption by default does not change the
        // encryption status of your existing volumes", and neither does enabling it.
        let (_, source_described) = call(
            &router,
            region,
            "DescribeVolumes",
            &indexed("VolumeId", &[&source_id]),
        )
        .await;
        assert_eq!(
            element(&source_described, "encrypted").as_deref(),
            Some("false"),
            "case {case:?}: expected the source to stay unencrypted, got {source_described}"
        );
    }
}
