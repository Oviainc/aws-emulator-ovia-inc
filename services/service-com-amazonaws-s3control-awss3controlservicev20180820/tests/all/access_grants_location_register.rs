// SPDX-License-Identifier: CC0-1.0

//! Registering an S3 data location in an S3 Access Grants instance, driven end
//! to end over HTTP.
//!
//! The rule under test: `CreateAccessGrantsLocation` records ONE registered
//! location per S3 path in the Access Grants instance of the account the request
//! names, in the Region the request was signed for — assigning the id `default`
//! to the default location scope and a service-assigned id to every other scope,
//! reporting that id, an ARN naming it in the signing Region, the scope and the
//! IAM role as registered, and the instant of the registration — so every later
//! `GetAccessGrantsLocation` by that account in that Region reports exactly that
//! record; a registration for a scope the instance already holds is refused
//! rather than replacing it, and a registration for an account with no instance
//! in the Region the request is signed for is refused as not existing, recording
//! nothing.
//!
//! Every row below differs along something that rule generalises over: the form
//! of the S3 path registered (the default location, a bucket with and without
//! its trailing slash, a bucket and prefix, a bucket and a deep prefix, a single
//! object key, and a path at the modelled `S3Prefix` maximum of 2000
//! characters), the IAM role the location is registered with, how many tags the
//! registration submits (both modelled bounds of `TagList`, `len 0..50`), the
//! Region the request is signed for, and the account that owns the instance.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.
//! The router, the request builder and the XML readers are the sibling suites',
//! reused rather than copied.

use std::collections::BTreeSet;

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_INSTANCE_ID, ACCESS_GRANTS_LOCATION_DEFAULT_ID, DEFAULT_LOCATION_SCOPE,
};

/// A second signing Region, used to pin that a registered location belongs to
/// the instance of one account in one Region: the same S3 path registered here
/// is a separate resource.
const OTHER_REGION: &str = "eu-central-1";

/// A third signing Region, so a table can leave one Region with no instance in
/// it at all.
const THIRD_REGION: &str = "ap-southeast-2";

/// The IAM role most rows register their location with. Its shape is the
/// modelled `IAMRoleArn` pattern, `^arn:[^:]+:iam::\d{12}:role/`.
const LOCATION_ROLE: &str = "arn:aws:iam::123456789012:role/s3-access-grants-location";

/// A second IAM role, so a row can differ in the role it registers and a repeat
/// registration can differ from the one it conflicts with.
const OTHER_LOCATION_ROLE: &str = "arn:aws:iam::210987654321:role/team/access-grants-reader";

/// The largest tag set `CreateAccessGrantsLocation` accepts: `Tags` targets
/// `TagList`, which the model constrains to `len 0..50`.
const MAX_TAGS: usize = 50;

/// The longest S3 path `LocationScope` admits: it targets `S3Prefix`, which the
/// model constrains to `len 1..2000`.
const MAX_LOCATION_SCOPE_LEN: usize = 2000;

/// The characters the modelled `AccessGrantsLocationId` pattern
/// `^[a-zA-Z0-9\-]+$` admits beside the ASCII alphanumerics.
const LOCATION_ID_EXTRA: &str = "-";

/// The longest id `AccessGrantsLocationId` admits: `len 1..64`.
const MAX_LOCATION_ID_LEN: usize = 64;

/// One `CreateAccessGrantsLocation` request: the members the model lets a caller
/// vary, plus the account and Region whose instance holds the registration.
struct RegisterCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    /// The S3 path being registered — the member the id rule and the
    /// one-registration-per-scope rule both turn on.
    location_scope: String,
    iam_role_arn: &'static str,
    /// How many tags the registration submits.
    tags: usize,
}

impl RegisterCase {
    fn new(case: &'static str, location_scope: impl Into<String>) -> Self {
        Self {
            case,
            account_id: ACCOUNT,
            region: REGION,
            location_scope: location_scope.into(),
            iam_role_arn: LOCATION_ROLE,
            tags: 0,
        }
    }

    fn request_body(&self) -> String {
        let mut xml = format!(
            "<CreateAccessGrantsLocationRequest xmlns=\"{XMLNS}\"><LocationScope>{}\
             </LocationScope><IAMRoleArn>{}</IAMRoleArn>",
            xml_escape(&self.location_scope),
            xml_escape(self.iam_role_arn)
        );
        if self.tags > 0 {
            xml.push_str("<Tags>");
            for index in 0..self.tags {
                xml.push_str(&format!(
                    "<Tag><Key>key-{index}</Key><Value>value-{index}</Value></Tag>"
                ));
            }
            xml.push_str("</Tags>");
        }
        xml.push_str("</CreateAccessGrantsLocationRequest>");
        xml
    }

    /// The id this scope must be assigned, when the documented rule fixes it:
    /// "S3 Access Grants assigns the ID default to the default location s3://
    /// and assigns an auto-generated ID to other locations that you register"
    /// (`AccessGrantsLocationId`).
    fn fixed_id(&self) -> Option<&'static str> {
        (self.location_scope == DEFAULT_LOCATION_SCOPE).then_some(ACCESS_GRANTS_LOCATION_DEFAULT_ID)
    }
}

/// An S3 path exactly [`MAX_LOCATION_SCOPE_LEN`] characters long: a bucket and a
/// prefix padded out to the modelled maximum.
fn longest_location_scope() -> String {
    let head = "s3://padded-bucket/";
    format!("{head}{}", "p".repeat(MAX_LOCATION_SCOPE_LEN - head.len()))
}

/// The registration cases every assertion below is run over. No two rows
/// register the same S3 path in the same instance — that pair is the conflict
/// the rule refuses — and each differs from the others in something the
/// behaviour under test generalises over.
fn register_cases() -> Vec<RegisterCase> {
    vec![
        // The default location, whose id the model fixes.
        RegisterCase::new("the default location", DEFAULT_LOCATION_SCOPE),
        RegisterCase::new("a bucket without its trailing slash", "s3://plain-bucket"),
        RegisterCase {
            tags: 1,
            ..RegisterCase::new("a bucket with its trailing slash", "s3://slashed-bucket/")
        },
        RegisterCase {
            iam_role_arn: OTHER_LOCATION_ROLE,
            ..RegisterCase::new("a bucket and prefix", "s3://reports-bucket/engineering/")
        },
        RegisterCase {
            tags: MAX_TAGS,
            ..RegisterCase::new(
                "a bucket and a deep prefix, with the largest tag set",
                "s3://reports-bucket/marketing/campaigns/2026/",
            )
        },
        RegisterCase::new(
            "a single object key",
            "s3://reports-bucket/marketing/campaigns/q1.csv",
        ),
        RegisterCase {
            region: OTHER_REGION,
            ..RegisterCase::new(
                "the default location in another Region",
                DEFAULT_LOCATION_SCOPE,
            )
        },
        RegisterCase {
            region: THIRD_REGION,
            tags: 2,
            ..RegisterCase::new("an instance in a third Region", "s3://third-region-bucket/")
        },
        RegisterCase::new(
            "the longest S3 path the model admits",
            longest_location_scope(),
        ),
    ]
}

/// The ARN a registered location of this id must report: the
/// `location/<id>` subresource of the Access Grants instance that holds it, in
/// the `s3` ARN namespace, in the signing Region, under the emulator's
/// canonical account — so the instance segment sits between `access-grants` and
/// `location`.
fn expected_arn(region: &str, location_id: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:access-grants/{}/location/{location_id}",
        ::emulator_server::emulator_account_id(),
        ACCESS_GRANTS_INSTANCE_ID
    )
}

/// Create the account's Access Grants instance in `region`, tolerating the
/// conflict a row whose pair an earlier row already created answers with.
async fn ensure_instance(emulator: &Emulator, account_id: &str, region: &str) {
    let created = call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance",
        account_id,
        &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
    )
    .await;
    assert!(
        created.status == 200 || created.status == 409,
        "case {account_id:?}/{region:?}: expected the instance to exist after the create, got \
         {} with body {}",
        created.status,
        created.body
    );
}

/// Register `case`'s S3 path, signed for that case's Region.
async fn register(emulator: &Emulator, case: &RegisterCase) -> Reply {
    call_in_region(
        &emulator.router,
        case.region,
        "POST",
        "/v20180820/accessgrantsinstance/location",
        case.account_id,
        &case.request_body(),
    )
    .await
}

/// Read the location `location_id` names in `account_id`'s instance in `region`.
async fn get_location(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_id: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/accessgrantsinstance/location/{location_id}"),
        account_id,
        "",
    )
    .await
}

/// Assert `reply` reports `case`'s registered location, answering the id and the
/// creation instant it carries.
fn assert_reports_location(case: &RegisterCase, reply: &Reply, call: &str) -> (String, String) {
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected {call} to answer 200, got {} with body {}",
        case.case, reply.status, reply.body
    );
    let location_id = reply.text("AccessGrantsLocationId").unwrap_or_else(|| {
        panic!(
            "case {:?}: no AccessGrantsLocationId in {call}'s body {}",
            case.case, reply.body
        )
    });
    match case.fixed_id() {
        Some(fixed) => assert_eq!(
            location_id, fixed,
            "case {:?}: expected {call} to report the id the model fixes for the default \
             location, got {location_id:?}",
            case.case
        ),
        None => assert_ne!(
            location_id, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
            "case {:?}: expected {call} to report a service-assigned id for a scope other than \
             the default location",
            case.case
        ),
    }
    assert!(
        !location_id.is_empty()
            && location_id.len() <= MAX_LOCATION_ID_LEN
            && location_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || LOCATION_ID_EXTRA.contains(character)),
        "case {:?}: expected an AccessGrantsLocationId the modelled pattern and length admit, \
         got {location_id:?}",
        case.case
    );

    for (element, expected) in [
        ("LocationScope", case.location_scope.clone()),
        ("IAMRoleArn", case.iam_role_arn.to_string()),
        (
            "AccessGrantsLocationArn",
            expected_arn(case.region, &location_id),
        ),
    ] {
        let reported = reply.text(element).unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {:?}: expected {call} to report {element} as {expected:?}, got {reported:?} in \
             body {}",
            case.case, reply.body
        );
    }

    let created_at = reply.text("CreatedAt").unwrap_or_else(|| {
        panic!(
            "case {:?}: no CreatedAt in {call}'s body {}",
            case.case, reply.body
        )
    });
    let recorded = ::emulator_server::aws_smithy_types::DateTime::from_str(
        &created_at,
        ::emulator_server::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!(
            "case {:?}: expected {call} to report an ISO-8601 CreatedAt, got {created_at:?}: \
             {error}",
            case.case
        )
    });
    let now = i64::try_from(
        ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch seconds fit in an i64");
    assert!(
        (now - recorded.secs()).abs() <= 300,
        "case {:?}: expected {call} to report the instant of the registration, got \
         {created_at:?} against a clock reading {now}",
        case.case
    );
    (location_id, created_at)
}

#[tokio::test]
async fn register_then_get_reports_the_registered_location_for_every_shape_of_scope() {
    let emulator = Emulator::start();
    let mut arns = BTreeSet::new();

    for case in register_cases() {
        ensure_instance(&emulator, case.account_id, case.region).await;

        let registered = register(&emulator, &case).await;
        let (location_id, created_at) =
            assert_reports_location(&case, &registered, "CreateAccessGrantsLocation");

        let read = get_location(&emulator, case.account_id, case.region, &location_id).await;
        let (read_id, read_created_at) =
            assert_reports_location(&case, &read, "GetAccessGrantsLocation");
        assert_eq!(
            read_id, location_id,
            "case {:?}: expected the read to report the id the registration assigned, got body {}",
            case.case, read.body
        );
        assert_eq!(
            read_created_at, created_at,
            "case {:?}: expected the read to report the instant of the registration, got body {}",
            case.case, read.body
        );

        // A read may not change what a later reader sees: every member the
        // first read reported is the one the next read reports.
        let read_again = get_location(&emulator, case.account_id, case.region, &location_id).await;
        assert_eq!(
            read_again.body, read.body,
            "case {:?}: expected a second read to report the same location, got body {}",
            case.case, read_again.body
        );

        assert!(
            arns.insert(read.text("AccessGrantsLocationArn").unwrap_or_default()),
            "case {:?}: expected an ARN no other registered location carries, got body {}",
            case.case,
            read.body
        );
    }
}

#[tokio::test]
async fn registering_without_an_instance_in_the_signing_region_is_refused_and_records_nothing() {
    let emulator = Emulator::start();

    // Before any instance exists, no (account, Region) pair can hold a
    // registration.
    for case in register_cases() {
        let refused = register(&emulator, &case).await;
        assert_eq!(
            refused.status, 404,
            "case {:?}: expected a registration with no instance to answer 404, got {} with \
             body {}",
            case.case, refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "AccessGrantsInstanceNotExistsError",
            "case {:?}: expected the instance not-exists error code, got body {}",
            case.case,
            refused.body
        );
        let message = refused.text("Message").unwrap_or_default();
        assert_eq!(
            message, "Access Grants Instance does not exist",
            "case {:?}: expected the refusal to state the missing instance in fixed words, \
             naming neither the account nor the Region looked up, got {message:?}",
            case.case
        );

        // Nothing was recorded: the id the default scope would have been
        // assigned is readable nowhere.
        let read = get_location(
            &emulator,
            case.account_id,
            case.region,
            ACCESS_GRANTS_LOCATION_DEFAULT_ID,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {:?}: expected the refused registration to record nothing, got {} with body {}",
            case.case, read.status, read.body
        );
    }

    // An instance is Region-scoped, so creating one leaves every OTHER Region
    // of that account still unable to hold a registration.
    ensure_instance(&emulator, ACCOUNT, REGION).await;
    for region in [OTHER_REGION, THIRD_REGION] {
        let case = RegisterCase {
            region,
            ..RegisterCase::new("a Region with no instance", "s3://elsewhere-bucket/")
        };
        let refused = register(&emulator, &case).await;
        assert_eq!(
            refused.status, 404,
            "case {region:?}: expected a registration in a Region with no instance to answer \
             404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "AccessGrantsInstanceNotExistsError",
            "case {region:?}: expected the instance not-exists error code, got body {}",
            refused.body
        );
    }

    // A registration addressed in another account is refused before any
    // instance is resolved, so it says nothing about whether that account has
    // one.
    assert_foreign_account_refused(
        "CreateAccessGrantsLocation for another account",
        OTHER_ACCOUNT,
        &register(
            &emulator,
            &RegisterCase {
                account_id: OTHER_ACCOUNT,
                ..RegisterCase::new("another account", "s3://their-bucket/")
            },
        )
        .await,
    );
}

#[tokio::test]
async fn a_repeat_registration_of_one_scope_conflicts_and_leaves_the_location_alone() {
    let emulator = Emulator::start();

    for case in register_cases() {
        ensure_instance(&emulator, case.account_id, case.region).await;
        let registered = register(&emulator, &case).await;
        let (location_id, _) =
            assert_reports_location(&case, &registered, "CreateAccessGrantsLocation");
        let first = get_location(&emulator, case.account_id, case.region, &location_id).await;

        // The repeat differs from the first registration in its other members,
        // so a conflict that silently overwrote the record would be visible
        // below.
        let repeat = RegisterCase {
            account_id: case.account_id,
            region: case.region,
            iam_role_arn: if case.iam_role_arn == LOCATION_ROLE {
                OTHER_LOCATION_ROLE
            } else {
                LOCATION_ROLE
            },
            tags: 3,
            ..RegisterCase::new(case.case, case.location_scope.clone())
        };
        let conflict = register(&emulator, &repeat).await;
        assert_eq!(
            conflict.status, 409,
            "case {:?}: expected a repeat registration of one scope to answer 409, got {} with \
             body {}",
            case.case, conflict.status, conflict.body
        );
        assert_eq!(
            conflict.code(),
            "AccessGrantsLocationAlreadyExistsError",
            "case {:?}: expected the conflict error code, got body {}",
            case.case,
            conflict.body
        );
        let message = conflict.text("Message").unwrap_or_default();
        assert!(
            message.contains(case.location_scope.as_str()),
            "case {:?}: expected the refusal to name the scope already registered, got \
             {message:?}",
            case.case
        );

        let after = get_location(&emulator, case.account_id, case.region, &location_id).await;
        assert_eq!(
            after.body, first.body,
            "case {:?}: expected the refused registration to leave the stored location alone, \
             got body {}",
            case.case, after.body
        );
    }
}

#[tokio::test]
async fn one_s3_path_is_registered_once_per_instance_and_read_only_there() {
    let emulator = Emulator::start();
    // A scope other than the default location, so each instance's registration
    // carries a service-assigned id of its own.
    let location_scope = "s3://shared-name-bucket/";
    let owners = [
        (ACCOUNT, REGION),
        (ACCOUNT, OTHER_REGION),
        (ACCOUNT, THIRD_REGION),
    ];

    let mut registered = Vec::new();
    for (account_id, region) in owners {
        ensure_instance(&emulator, account_id, region).await;
        let case = RegisterCase {
            account_id,
            region,
            ..RegisterCase::new("one scope per instance", location_scope)
        };
        let reply = register(&emulator, &case).await;
        let (location_id, _) = assert_reports_location(&case, &reply, "CreateAccessGrantsLocation");
        registered.push((account_id, region, location_id));
    }

    for (account_id, region, location_id) in &registered {
        // The instance that registered it reports it.
        let read = get_location(&emulator, account_id, region, location_id).await;
        assert_eq!(
            read.text("LocationScope").unwrap_or_default(),
            location_scope,
            "case {account_id:?}/{region:?}: expected the registering instance to report the \
             scope, got body {}",
            read.body
        );
        assert_eq!(
            read.text("AccessGrantsLocationArn").unwrap_or_default(),
            expected_arn(region, location_id),
            "case {account_id:?}/{region:?}: expected the ARN to name the registering Region, \
             got body {}",
            read.body
        );

        // No OTHER instance does: the id belongs to the instance it was
        // assigned in.
        for (other_account, other_region) in owners {
            if other_account == *account_id && other_region == *region {
                continue;
            }
            let elsewhere = get_location(&emulator, other_account, other_region, location_id).await;
            assert_eq!(
                elsewhere.status, 404,
                "case {account_id:?}/{region:?}: expected {other_account:?}/{other_region:?} not \
                 to hold this location, got {} with body {}",
                elsewhere.status, elsewhere.body
            );
            assert_eq!(
                elsewhere.code(),
                "NoSuchAccessGrantsLocationError",
                "case {account_id:?}/{region:?}: expected the location not-exists error code \
                 from {other_account:?}/{other_region:?}, got body {}",
                elsewhere.body
            );
        }
    }
}
