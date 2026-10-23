// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstance` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: `DescribeInstance` REPORTS the IAM Identity Center instance its
//! `InstanceArn` names, from the same records `ListInstances` enumerates — every member it
//! shares with `InstanceMetadata` carries the stored value, it reports no member the
//! instance has no value for, and it writes nothing; an `InstanceArn` this account holds
//! no instance for is refused with the modelled `AccessDeniedException` (403), the only
//! not-found channel the operation's model declares.
//!
//! The table spans the class the rule generalises over: the ARN that resolves, and ARNs
//! that do not — in each spelling the model's `InstanceArn` pattern
//! `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$` admits (the
//! `ins-` and `ssoins-` prefixes, each partition suffix the pattern allows, and the full
//! `[a-zA-Z0-9-.]` id character class), plus the held instance's id under another
//! partition, which is a DIFFERENT instance.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, is_instance_arn, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// What a row expects of `DescribeInstance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The ARN names an instance this account holds: 200, and the stored record.
    Reported,
    /// No instance of this account has that ARN: 403 `AccessDeniedException`.
    Denied,
}

/// One row: a label, the `InstanceArn` to describe, and what must come back.
struct Case {
    label: &'static str,
    /// `None` means "the ARN of the seeded instance", read at run time.
    instance_arn: Option<&'static str>,
    expected: Expected,
}

/// The rows differ along what the rule generalises over: whether the ARN resolves, the
/// `(sso)?ins-` prefix spelling, the partition segment the pattern admits, and the id's
/// character class.
const CASES: &[Case] = &[
    Case {
        label: "the ARN ListInstances reported",
        instance_arn: None,
        expected: Expected::Reported,
    },
    Case {
        label: "a pattern-valid ssoins- ARN that was never created",
        instance_arn: Some(UNKNOWN_INSTANCE_ARN),
        expected: Expected::Denied,
    },
    Case {
        // The pattern's `(sso)?` group makes the bare `ins-` spelling legal input.
        label: "the bare ins- spelling",
        instance_arn: Some("arn:aws:sso:::instance/ins-abcdefghij123456"),
        expected: Expected::Denied,
    },
    Case {
        // The id class is `[a-zA-Z0-9-.]`: mixed case, a hyphen and a dot are all legal.
        label: "an id using the full [a-zA-Z0-9-.] class",
        instance_arn: Some("arn:aws:sso:::instance/ssoins-Ab.9-cDeF0123456"),
        expected: Expected::Denied,
    },
    Case {
        // `aws(-[a-z]{1,5}){0,3}`: another partition is a legal ARN and a different
        // instance.
        label: "a GovCloud partition ARN",
        instance_arn: Some("arn:aws-us-gov:sso:::instance/ssoins-1111111111111111"),
        expected: Expected::Denied,
    },
    Case {
        label: "a China partition ARN",
        instance_arn: Some("arn:aws-cn:sso:::instance/ssoins-2222222222222222"),
        expected: Expected::Denied,
    },
];

/// The `InstanceMetadata` `ListInstances` reports for `arn`.
async fn listed_instance(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");
    body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInstances returned no Instances: {body}"))
        .iter()
        .find(|instance| instance["InstanceArn"].as_str() == Some(arn))
        .unwrap_or_else(|| panic!("ListInstances does not list {arn}: {body}"))
        .clone()
}

/// Whether `arn` has the shape the model's `InstanceIdentityStoreArn` pattern requires:
/// `arn:aws…:identitystore::<12 digits>:identitystore/` then `d-` and 10 hex digits (or
/// a hyphenated UUID).
fn is_identity_store_arn(arn: &str) -> bool {
    let Some((prefix, id)) = arn.split_once(":identitystore/") else {
        return false;
    };
    // `arn:<partition>:identitystore::<account>` — the identity-store ARN carries no
    // region, so the account id follows the empty region segment.
    let Some(account) = prefix.split(":identitystore::").nth(1) else {
        return false;
    };
    let directory = id
        .strip_prefix("d-")
        .is_some_and(|rest| rest.len() == 10 && rest.chars().all(|c| c.is_ascii_hexdigit()));
    let uuid = id.split('-').map(str::len).eq([8usize, 4, 4, 4, 12]);
    prefix.starts_with("arn:aws")
        && account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && (directory || uuid)
        && arn.len() >= 62
        && arn.len() <= 93
}

/// The rule, over the whole table: an ARN this account holds is reported from the stored
/// record and agrees with `ListInstances` member for member; any other pattern-valid ARN
/// is refused `AccessDeniedException` with HTTP 403.
#[tokio::test]
async fn describe_instance_reports_the_stored_instance_and_denies_every_other_arn() {
    let router = router();
    let seeded = seeded_instance_arn(&router).await;
    assert!(
        is_instance_arn(&seeded),
        "the seeded instance must carry a modelled InstanceArn, got {seeded:?}"
    );

    for case in CASES {
        let arn = case.instance_arn.unwrap_or(seeded.as_str());
        let (status, body) = call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;

        match case.expected {
            Expected::Denied => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403 for {arn}, got {status} {body}",
                    case.label
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDeniedException",
                    "case {:?}: expected AccessDeniedException for {arn}, got {body}",
                    case.label
                );
                continue;
            }
            Expected::Reported => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected HTTP 200 for {arn}, got {status} {body}",
                    case.label
                );
            }
        }

        let listed = listed_instance(&router, arn).await;
        // Every member `DescribeInstance` shares with `InstanceMetadata` must carry the
        // SAME stored value: one record, two projections.
        for member in [
            "InstanceArn",
            "IdentityStoreId",
            "IdentityStoreArn",
            "OwnerAccountId",
            "Name",
            "CreatedDate",
            "Status",
            "PrimaryRegion",
            "Regions",
        ] {
            assert_eq!(
                body[member], listed[member],
                "case {:?}: member {member} must match the listed instance, got {:?} vs {:?}",
                case.label, body[member], listed[member]
            );
        }
        assert_eq!(
            body["Status"].as_str(),
            Some("ACTIVE"),
            "case {:?}: a seeded instance is ACTIVE, got {:?}",
            case.label,
            body["Status"]
        );
        assert!(
            is_identity_store_arn(body["IdentityStoreArn"].as_str().unwrap_or_default()),
            "case {:?}: expected a modelled IdentityStoreArn, got {:?}",
            case.label,
            body["IdentityStoreArn"]
        );
        assert_eq!(
            body["OwnerAccountId"].as_str(),
            Some(engine::EMULATOR_ACCOUNT_ID),
            "case {:?}: expected the emulator's account id, got {:?}",
            case.label,
            body["OwnerAccountId"]
        );
        assert_eq!(
            body["PermissionSetsEnabled"],
            Value::Bool(false),
            "case {:?}: the account's instance is an ACCOUNT instance, which serves no \
             permission sets, so it must report them disabled, got {:?}",
            case.label,
            body["PermissionSetsEnabled"]
        );
        // An ACTIVE instance has no additional status context, and nothing configures a
        // customer-managed key, so neither optional member is invented.
        assert_eq!(
            body["StatusReason"],
            Value::Null,
            "case {:?}: an ACTIVE instance must report no StatusReason, got {:?}",
            case.label,
            body["StatusReason"]
        );
        assert_eq!(
            body["EncryptionConfigurationDetails"],
            Value::Null,
            "case {:?}: no encryption configuration is held, so none may be reported, \
             got {:?}",
            case.label,
            body["EncryptionConfigurationDetails"]
        );

        // The primary Region is one of the enabled Regions, and it is the one flagged
        // primary.
        let regions = body["Regions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no Regions in {body}", case.label));
        let primary: Vec<&Value> = regions
            .iter()
            .filter(|region| region["IsPrimaryRegion"] == Value::Bool(true))
            .collect();
        assert_eq!(
            primary.len(),
            1,
            "case {:?}: exactly one Region is primary, got {regions:?}",
            case.label
        );
        assert_eq!(
            primary[0]["RegionName"], body["PrimaryRegion"],
            "case {:?}: the primary Region must be the one PrimaryRegion names, got {:?} \
             vs {:?}",
            case.label, primary[0]["RegionName"], body["PrimaryRegion"]
        );
    }
}

/// `DescribeInstance` is a READ: describing an instance twice reports the identical
/// record, and it changes nothing a later caller sees.
#[tokio::test]
async fn describing_an_instance_changes_nothing() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    let (status, before) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {before}");

    let (status, first) = call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(status, 200, "DescribeInstance expected 200, got {status} {first}");
    let (status, second) = call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(status, 200, "DescribeInstance expected 200, got {status} {second}");
    assert_eq!(
        first, second,
        "two reads of one instance must report the identical record, got {first} then {second}"
    );

    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        before, after,
        "a read must not change what a later caller sees, got {before} then {after}"
    );
}

/// An `InstanceArn` outside the model's pattern is a BAD REQUEST, not a missing
/// resource: the framework refuses it before the handler runs, with the service's
/// `ValidationException` (400). The rows span the ways the pattern and its 10..1224
/// length can be missed.
#[tokio::test]
async fn an_instance_arn_outside_the_pattern_is_a_validation_error() {
    let router = router();

    // (case, InstanceArn)
    let rows: [(&str, &str); 5] = [
        ("empty", ""),
        ("not an arn at all", "ssoins-7223b1d1ac5e1f0e"),
        ("another service's arn", "arn:aws:iam::123456789012:role/demo"),
        // The id must be exactly 16 characters of `[a-zA-Z0-9-.]`.
        ("an id one character short", "arn:aws:sso:::instance/ssoins-000000000000000"),
        ("an id one character long", "arn:aws:sso:::instance/ssoins-00000000000000000"),
    ];

    for (case, arn) in rows {
        let (status, body) =
            call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {arn:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException for {arn:?}, got {body}"
        );
    }
}
