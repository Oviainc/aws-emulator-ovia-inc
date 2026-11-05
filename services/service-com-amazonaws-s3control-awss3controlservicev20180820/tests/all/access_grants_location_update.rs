// SPDX-License-Identifier: CC0-1.0

//! `UpdateAccessGrantsLocation`, driven end to end over HTTP.
//!
//! The rule under test: an update of a registered location replaces the IAM
//! role of the location the request names in the Access Grants instance its
//! account owns in the Region the request is signed for, and changes nothing
//! else about that registration — the id the register call assigned, the ARN
//! derived from it, the S3 path it spans and the instant it was registered all
//! stay — so the call and every later read report the new role over the
//! unchanged rest of the record, and an id that instance holds no location of
//! is refused as not existing with no role replaced anywhere.
//!
//! Every row below differs along something that rule generalises over: the form
//! of the registered scope (the default location `s3://`, a bucket with and
//! without its trailing slash, a bucket and a prefix), the shape of the role
//! ARN the update moves to (another partition, a role path, the modelled
//! `IAMRoleArn` length bound), how many updates the location has had, and the
//! account and Region that own the instance.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model, runs the handler and serializes the restXml
//! response. The router, the request builder and the XML readers are the
//! sibling suites', reused rather than copied.

use std::collections::BTreeSet;

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_LOCATION_DEFAULT_ID, DEFAULT_LOCATION_SCOPE,
};

/// A second signing Region, used to pin that a registered location belongs to
/// the instance of one account in one Region: the same id names no location
/// here.
const OTHER_REGION: &str = "eu-central-1";

/// A third signing Region, so a row whose instance is its own still has one of
/// its own now that every row is the caller's one account's.
const THIRD_REGION: &str = "ap-southeast-2";

/// The IAM role every location in this suite is registered with, before any
/// update. Its shape is the modelled `IAMRoleArn` pattern,
/// `^arn:[^:]+:iam::\d{12}:role/`.
const INITIAL_ROLE: &str = "arn:aws:iam::123456789012:role/s3-access-grants-location";

/// The fixed head of an IAM role ARN the modelled `IAMRoleArn` pattern admits:
/// `^arn:[^:]+:iam::\d{12}:role/`. Everything after it is the role's name or
/// path, which is what [`role_of_length`] pads.
const ROLE_PREFIX: &str = "arn:aws:iam::123456789012:role/";

/// The longest role ARN `IAMRoleArn` admits: the model constrains it to
/// `len 1..2048`.
const MAX_ROLE_ARN_LEN: usize = 2048;

/// A role ARN of exactly `len` characters, for the modelled length bound. `len`
/// must leave room for [`ROLE_PREFIX`] and at least one character of role name.
fn role_of_length(len: usize) -> String {
    let name_len = len - ROLE_PREFIX.len();
    format!("{ROLE_PREFIX}{}", "r".repeat(name_len))
}

/// One update case: a registration, and the role an update moves it to.
struct UpdateCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    /// The S3 URI path the location is registered for, which the update may
    /// not change.
    location_scope: &'static str,
    /// The roles successive updates move the location to, in order. More than
    /// one row has several, because a location can be updated again.
    new_roles: Vec<String>,
}

/// The update cases every assertion below is run over. Each row owns a distinct
/// registration, and differs from the others in something the behaviour under
/// test generalises over.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "the default location, one update",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: DEFAULT_LOCATION_SCOPE,
            new_roles: vec!["arn:aws:iam::123456789012:role/default-location".to_string()],
        },
        UpdateCase {
            case: "a bucket without a trailing slash, updated twice",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://reports-bucket",
            new_roles: vec![
                "arn:aws:iam::210987654321:role/reports-reader".to_string(),
                "arn:aws:iam::210987654321:role/reports-writer".to_string(),
            ],
        },
        UpdateCase {
            case: "a bucket with a trailing slash, moved to a role under a path",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://finance-bucket/",
            new_roles: vec![
                "arn:aws:iam::123456789012:role/service-role/finance-location".to_string(),
            ],
        },
        UpdateCase {
            case: "a bucket and prefix, moved to another partition's role",
            account_id: ACCOUNT,
            region: OTHER_REGION,
            location_scope: "s3://finance-bucket/quarterly/",
            // `IAMRoleArn`'s pattern constrains the partition segment only to
            // `[^:]+`, so a role in another partition is the same member.
            new_roles: vec!["arn:aws-us-gov:iam::123456789012:role/quarterly".to_string()],
        },
        UpdateCase {
            case: "a third Region's instance, the longest role ARN IAMRoleArn admits",
            account_id: ACCOUNT,
            region: THIRD_REGION,
            location_scope: "s3://partner-bucket/inbound/",
            new_roles: vec![
                role_of_length(MAX_ROLE_ARN_LEN),
                // The shortest the pattern admits: the fixed head and a
                // one-character role name.
                role_of_length(ROLE_PREFIX.len() + 1),
            ],
        },
    ]
}

/// Create the account's Access Grants instance in `region`.
async fn create_instance(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance",
        account_id,
        &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
    )
    .await
}

/// Register `location_scope` with `iam_role_arn` in the account's instance in
/// `region`.
async fn create_location(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_scope: &str,
    iam_role_arn: &str,
) -> Reply {
    let body = format!(
        "<CreateAccessGrantsLocationRequest xmlns=\"{XMLNS}\">\
         <LocationScope>{}</LocationScope><IAMRoleArn>{}</IAMRoleArn>\
         </CreateAccessGrantsLocationRequest>",
        xml_escape(location_scope),
        xml_escape(iam_role_arn)
    );
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance/location",
        account_id,
        &body,
    )
    .await
}

/// Read the registered location `location_id` names.
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

/// Update the IAM role of the registered location `location_id` names.
pub(crate) async fn update_location(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_id: &str,
    iam_role_arn: &str,
) -> Reply {
    let body = format!(
        "<UpdateAccessGrantsLocationRequest xmlns=\"{XMLNS}\">\
         <IAMRoleArn>{}</IAMRoleArn></UpdateAccessGrantsLocationRequest>",
        xml_escape(iam_role_arn)
    );
    call_in_region(
        &emulator.router,
        region,
        "PUT",
        &format!("/v20180820/accessgrantsinstance/location/{location_id}"),
        account_id,
        &body,
    )
    .await
}

/// Everything a registration reports besides the role: the members an update
/// may not change.
#[derive(Debug, PartialEq, Eq)]
struct Registration {
    id: String,
    arn: String,
    location_scope: String,
    created_at: String,
}

impl Registration {
    fn of(reply: &Reply) -> Self {
        Self {
            id: reply.text("AccessGrantsLocationId").unwrap_or_default(),
            arn: reply.text("AccessGrantsLocationArn").unwrap_or_default(),
            location_scope: reply.text("LocationScope").unwrap_or_default(),
            created_at: reply.text("CreatedAt").unwrap_or_default(),
        }
    }
}

/// Register the location `case` describes, with [`INITIAL_ROLE`], and answer the
/// register call's reply.
async fn register(emulator: &Emulator, case: &UpdateCase) -> Reply {
    let created = create_location(
        emulator,
        case.account_id,
        case.region,
        case.location_scope,
        INITIAL_ROLE,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected CreateAccessGrantsLocation to answer 200, got {} with body {}",
        case.case, created.status, created.body
    );
    created
}

/// Stand an emulator up with an Access Grants instance for every (account,
/// Region) pair the cases use.
async fn emulator_with_instances(cases: &[UpdateCase]) -> Emulator {
    let emulator = Emulator::start();
    let mut created = BTreeSet::new();
    for case in cases {
        if created.insert((case.account_id, case.region)) {
            let instance = create_instance(&emulator, case.account_id, case.region).await;
            assert_eq!(
                instance.status, 200,
                "case {:?}: expected CreateAccessGrantsInstance to answer 200, got {} with body {}",
                case.case, instance.status, instance.body
            );
        }
    }
    emulator
}

#[tokio::test]
async fn an_update_replaces_the_role_and_leaves_the_rest_of_the_registration_alone() {
    let cases = update_cases();
    let emulator = emulator_with_instances(&cases).await;

    for case in &cases {
        let registered = register(&emulator, case).await;
        let registration = Registration::of(&registered);
        let registered_role = registered.text("IAMRoleArn").unwrap_or_default();
        assert_eq!(
            registered_role, INITIAL_ROLE,
            "case {:?}: expected the register call to report the role it was given, got \
             {registered_role:?}",
            case.case
        );
        // The default location carries the documented id rather than a minted
        // one, which is what the update must address it by too.
        if case.location_scope == DEFAULT_LOCATION_SCOPE {
            assert_eq!(
                registration.id, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
                "case {:?}: expected the default location's documented id, got {:?}",
                case.case, registration.id
            );
        }

        for new_role in &case.new_roles {
            let updated = update_location(
                &emulator,
                case.account_id,
                case.region,
                &registration.id,
                new_role,
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {:?}: expected UpdateAccessGrantsLocation to answer 200 for role \
                 {new_role:?}, got {} with body {}",
                case.case, updated.status, updated.body
            );
            let reported_role = updated.text("IAMRoleArn").unwrap_or_default();
            assert_eq!(
                &reported_role, new_role,
                "case {:?}: expected the update to report the new role, got {reported_role:?}",
                case.case
            );
            assert_eq!(
                Registration::of(&updated),
                registration,
                "case {:?}: expected the update to leave the id, ARN, scope and registration \
                 instant alone, got body {}",
                case.case,
                updated.body
            );

            // The update is what a later read reports, and the read reports it
            // twice over: a read changes nothing.
            let read = get_location(&emulator, case.account_id, case.region, &registration.id).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected GetAccessGrantsLocation to answer 200, got {} with body {}",
                case.case, read.status, read.body
            );
            let read_role = read.text("IAMRoleArn").unwrap_or_default();
            assert_eq!(
                &read_role, new_role,
                "case {:?}: expected the read to report the updated role, got {read_role:?}",
                case.case
            );
            assert_eq!(
                Registration::of(&read),
                registration,
                "case {:?}: expected the read to report the registration as registered, got \
                 body {}",
                case.case,
                read.body
            );
            let read_again =
                get_location(&emulator, case.account_id, case.region, &registration.id).await;
            assert_eq!(
                read_again.body, read.body,
                "case {:?}: expected a second read to report the same location, got body {}",
                case.case, read_again.body
            );

            // Updating to the role it already carries is the same record
            // again: the update is idempotent in its own argument.
            let repeat = update_location(
                &emulator,
                case.account_id,
                case.region,
                &registration.id,
                new_role,
            )
            .await;
            assert_eq!(
                repeat.body, updated.body,
                "case {:?}: expected a repeat update to the same role to report the same \
                 location, got body {}",
                case.case, repeat.body
            );
        }
    }
}

#[tokio::test]
async fn an_update_touches_no_other_registration() {
    let cases = update_cases();
    let emulator = emulator_with_instances(&cases).await;

    // Register every case's location first, so each update below runs against a
    // store holding all of them.
    let mut ids = Vec::new();
    for case in &cases {
        let registered = register(&emulator, case).await;
        ids.push(Registration::of(&registered).id);
    }

    for (index, case) in cases.iter().enumerate() {
        let new_role = &case.new_roles[0];
        let updated =
            update_location(&emulator, case.account_id, case.region, &ids[index], new_role).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected the update to answer 200, got {} with body {}",
            case.case, updated.status, updated.body
        );

        // Every OTHER registration still carries the role it was registered
        // with: this update was about one location.
        for (other_index, other) in cases.iter().enumerate() {
            if other_index <= index {
                continue;
            }
            let read =
                get_location(&emulator, other.account_id, other.region, &ids[other_index]).await;
            let role = read.text("IAMRoleArn").unwrap_or_default();
            assert_eq!(
                role, INITIAL_ROLE,
                "case {:?}: expected the update of {:?} to leave this location's role alone, got \
                 {role:?}",
                other.case, case.case
            );
        }
    }
}

#[tokio::test]
async fn an_id_the_instance_holds_no_location_of_is_refused_and_changes_nothing() {
    let cases = update_cases();
    let emulator = emulator_with_instances(&cases).await;
    let case = &cases[1];
    let registered = register(&emulator, case).await;
    let registration = Registration::of(&registered);
    let new_role = "arn:aws:iam::123456789012:role/never-stored";

    // Each row is an address no location of the request's instance sits at: a
    // never-assigned id, the default id when no default location is
    // registered, and the registered id read from another Region's instance.
    let unknown: Vec<(&str, &str, &str, String)> = vec![
        (
            "an id no register call assigned",
            case.account_id,
            case.region,
            "no-such-location".to_string(),
        ),
        (
            "the default id with no default location registered",
            case.account_id,
            case.region,
            ACCESS_GRANTS_LOCATION_DEFAULT_ID.to_string(),
        ),
        (
            "another Region's instance",
            case.account_id,
            OTHER_REGION,
            registration.id.clone(),
        ),
    ];

    for (label, account_id, region, location_id) in unknown {
        let refused = update_location(&emulator, account_id, region, &location_id, new_role).await;
        assert_eq!(
            refused.status, 404,
            "case {label:?}: expected an update of an unknown location to answer 404, got {} \
             with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "NoSuchAccessGrantsLocationError",
            "case {label:?}: expected the not-exists error code, got body {}",
            refused.body
        );
        // The refusal is the same sentence whichever id was asked about: it
        // says the location does not exist, and nothing about the request.
        assert!(
            refused
                .body
                .contains("The specified access grants location does not exist"),
            "case {label:?}: expected the not-exists message, got body {}",
            refused.body
        );
    }

    // The same update addressed in another account is refused before the
    // location is resolved.
    assert_foreign_account_refused(
        "UpdateAccessGrantsLocation for another account",
        OTHER_ACCOUNT,
        &update_location(
            &emulator,
            OTHER_ACCOUNT,
            case.region,
            &registration.id,
            new_role,
        )
        .await,
    );

    // The refused updates recorded nothing and replaced nothing: the one
    // registration in the store still reads as registered.
    let read = get_location(&emulator, case.account_id, case.region, &registration.id).await;
    assert_eq!(
        read.text("IAMRoleArn").unwrap_or_default(),
        INITIAL_ROLE,
        "expected the refused updates to leave the registered role alone, got body {}",
        read.body
    );
    assert_eq!(
        Registration::of(&read),
        registration,
        "expected the refused updates to leave the registration alone, got body {}",
        read.body
    );
}

#[tokio::test]
async fn a_role_arn_past_the_modelled_length_bound_is_refused_and_replaces_nothing() {
    let cases = update_cases();
    let emulator = emulator_with_instances(&cases).await;
    let case = &cases[1];
    let registered = register(&emulator, case).await;
    let registration = Registration::of(&registered);

    // One character past the modelled `IAMRoleArn` maximum, and far past it.
    for len in [MAX_ROLE_ARN_LEN + 1, MAX_ROLE_ARN_LEN * 2] {
        let role = role_of_length(len);
        let refused = update_location(
            &emulator,
            case.account_id,
            case.region,
            &registration.id,
            &role,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {len}: expected a role ARN past the modelled maximum to be refused, got {} \
             with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ValidationException",
            "case {len}: expected the constraint refusal's error code, got body {}",
            refused.body
        );

        let read = get_location(&emulator, case.account_id, case.region, &registration.id).await;
        assert_eq!(
            read.text("IAMRoleArn").unwrap_or_default(),
            INITIAL_ROLE,
            "case {len}: expected the refused update to leave the registered role alone, got \
             body {}",
            read.body
        );
    }

    // The bound itself is accepted, which is what makes the refusals above
    // about the length rather than about long role names.
    let at_the_bound = role_of_length(MAX_ROLE_ARN_LEN);
    let updated = update_location(
        &emulator,
        case.account_id,
        case.region,
        &registration.id,
        &at_the_bound,
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "expected a role ARN at the modelled maximum to be accepted, got {} with body {}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.text("IAMRoleArn").unwrap_or_default(),
        at_the_bound,
        "expected the update at the modelled maximum to report that role, got body {}",
        updated.body
    );
}
