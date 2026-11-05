// SPDX-License-Identifier: CC0-1.0

//! Deregistering an S3 Access Grants location, driven end to end over HTTP.
//!
//! The rule under test: a registered location may be deregistered from the
//! Access Grants instance of its account and Region only while no grant of that
//! instance references it — while one does the call is refused as not empty and
//! the registration, the grants and every other location stay exactly as they
//! were; once none does, the registration is released and every later read,
//! update or delete of that id is refused as not existing. A grant is released
//! by deleting it, and only by deleting it.
//!
//! Every row below differs along something that rule generalises over: the form
//! of the registered scope (the default location `s3://`, a bucket with and
//! without its trailing slash, a bucket and a prefix), how many grants
//! reference the location (none, one, two, three), the sub-prefixes those
//! grants narrow it by, the permission each carries, and the account and Region
//! that own the instance.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder and the XML readers are the sibling suites', and so are the
//! calls that create an instance, register a location, read one back, create a
//! grant and read a grant — reused rather than copied.

use std::collections::BTreeSet;

use crate::access_grants_data_plane::{
    create_grant, create_instance, create_location, get_grant, get_location,
};
use crate::access_grants_location_update::update_location;
use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, Emulator, Reply, ACCOUNT, NO_CONTENT,
    OTHER_ACCOUNT, REGION,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_LOCATION_DEFAULT_ID, DEFAULT_LOCATION_SCOPE, PERMISSION_READ,
    PERMISSION_READWRITE, PERMISSION_WRITE,
};

/// A second signing Region, used to pin that a location belongs to the instance
/// of one account in one Region: the same id names no location there.
const OTHER_REGION: &str = "eu-central-1";

/// A third signing Region, so a row whose instance is its own still has one of
/// its own now that every row is the caller's one account's.
const THIRD_REGION: &str = "ap-southeast-2";

/// The role an update moves a location to, for the update that must be refused
/// once the registration is gone. Its shape is the modelled `IAMRoleArn`
/// pattern, `^arn:[^:]+:iam::\d{12}:role/`.
const UPDATE_ROLE: &str = "arn:aws:iam::123456789012:role/never-stored";

/// The fixed sentence S3 Access Grants answers for a location id its instance
/// holds no registration for.
const NO_SUCH_LOCATION_MESSAGE: &str = "The specified access grants location does not exist";

/// The fixed sentence S3 Access Grants answers while grants still reference the
/// location a request asks to deregister.
const LOCATION_NOT_EMPTY_MESSAGE: &str =
    "Please delete access grants before deleting access grants location";

/// The fixed sentence S3 Access Grants answers for a grant id its instance holds
/// no grant for.
const NO_SUCH_GRANT_MESSAGE: &str = "The specified access grant does not exist";

/// Deregister the location `location_id` names from the account's instance in
/// `region`.
///
/// `pub(crate)` because the sibling refusal suite deregisters a location the
/// same way rather than writing this again.
pub(crate) async fn delete_location(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_id: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        &format!("/v20180820/accessgrantsinstance/location/{location_id}"),
        account_id,
        "",
    )
    .await
}

/// Delete the grant `grant_id` names from the account's instance in `region`.
///
/// `pub(crate)` for the reason [`delete_location`] is.
pub(crate) async fn delete_grant(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    grant_id: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        &format!("/v20180820/accessgrantsinstance/grant/{grant_id}"),
        account_id,
        "",
    )
    .await
}

/// One deregistration case: a registration, and the grants that reference it.
struct DeleteCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    /// The S3 URI path the location is registered for.
    location_scope: &'static str,
    /// One (sub-prefix, permission) pair per grant created on the location, in
    /// order. An empty list is a location nothing references.
    grants: Vec<(&'static str, &'static str)>,
}

/// The cases every assertion below is run over. Each row owns a distinct
/// registration and differs from the others in something the behaviour under
/// test generalises over.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a bucket and prefix no grant references",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://reports-bucket/data/",
            grants: Vec::new(),
        },
        DeleteCase {
            case: "a bucket without a trailing slash, one grant",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://reports-bucket",
            grants: vec![("team-a/*", PERMISSION_READ)],
        },
        DeleteCase {
            case: "a bucket with a trailing slash, two grants",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://finance-bucket/",
            grants: vec![
                ("quarterly/*", PERMISSION_READ),
                ("yearly/*", PERMISSION_WRITE),
            ],
        },
        DeleteCase {
            case: "the default location, three grants",
            account_id: ACCOUNT,
            region: OTHER_REGION,
            // The default location carries the documented id rather than a
            // minted one, which is what the delete must address it by too.
            location_scope: DEFAULT_LOCATION_SCOPE,
            grants: vec![
                ("archive-bucket/*", PERMISSION_READ),
                ("archive-bucket/2024/*", PERMISSION_READWRITE),
                ("staging-bucket/report.csv", PERMISSION_WRITE),
            ],
        },
        DeleteCase {
            case: "a third Region's instance, one grant",
            account_id: ACCOUNT,
            region: THIRD_REGION,
            location_scope: "s3://partner-bucket/inbound/",
            grants: vec![("supplier-a/*", PERMISSION_READWRITE)],
        },
    ]
}

/// Stand an emulator up with an Access Grants instance for every (account,
/// Region) pair the cases use.
async fn emulator_with_instances(cases: &[DeleteCase]) -> Emulator {
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

/// Register `case`'s location and create its grants, answering the location id
/// and the grant ids.
async fn register(emulator: &Emulator, case: &DeleteCase) -> (String, Vec<String>) {
    let registered =
        create_location(emulator, case.account_id, case.region, case.location_scope).await;
    assert_eq!(
        registered.status, 200,
        "case {:?}: expected CreateAccessGrantsLocation to answer 200, got {} with body {}",
        case.case, registered.status, registered.body
    );
    let location_id = registered
        .text("AccessGrantsLocationId")
        .unwrap_or_default();
    if case.location_scope == DEFAULT_LOCATION_SCOPE {
        assert_eq!(
            location_id, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
            "case {:?}: expected the default location's documented id, got {location_id:?}",
            case.case
        );
    }

    let mut grant_ids = Vec::new();
    for (sub_prefix, permission) in &case.grants {
        let granted = create_grant(
            emulator,
            case.account_id,
            case.region,
            &location_id,
            Some(sub_prefix),
            permission,
        )
        .await;
        assert_eq!(
            granted.status, 200,
            "case {:?}: expected CreateAccessGrant on {sub_prefix:?} to answer 200, got {} with \
             body {}",
            case.case, granted.status, granted.body
        );
        grant_ids.push(granted.text("AccessGrantId").unwrap_or_default());
    }
    (location_id, grant_ids)
}

/// Assert `reply` is the refusal a location id no registration sits at gets.
fn assert_no_such_location(case: &str, call: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.code()),
        (404, "NoSuchAccessGrantsLocationError".to_string()),
        "case {case:?}: expected {call} on a deregistered location to be refused as not \
         existing, got {} with body {}",
        reply.status,
        reply.body
    );
    assert!(
        reply.body.contains(NO_SUCH_LOCATION_MESSAGE),
        "case {case:?}: expected {call} to carry the not-exists message, got body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_location_is_deregistered_only_once_no_grant_references_it() {
    let cases = delete_cases();
    let emulator = emulator_with_instances(&cases).await;

    for case in &cases {
        let (location_id, grant_ids) = register(&emulator, case).await;
        let registered = get_location(&emulator, case.account_id, case.region, &location_id).await;
        assert_eq!(
            registered.status, 200,
            "case {:?}: expected the registration to read back, got {} with body {}",
            case.case, registered.status, registered.body
        );

        // While a grant references the location the deregistration is refused,
        // and refused again after each grant is deleted until the last one is
        // gone: it is the grants that are left, not how many there were.
        for (deleted_so_far, grant_id) in grant_ids.iter().enumerate() {
            let refused =
                delete_location(&emulator, case.account_id, case.region, &location_id).await;
            assert_eq!(
                (refused.status, refused.code()),
                (400, "AccessGrantsLocationNotEmptyError".to_string()),
                "case {:?}: expected the deregistration to be refused with {} of {} grants \
                 deleted, got {} with body {}",
                case.case,
                deleted_so_far,
                grant_ids.len(),
                refused.status,
                refused.body
            );
            assert!(
                refused.body.contains(LOCATION_NOT_EMPTY_MESSAGE),
                "case {:?}: expected the not-empty message, got body {}",
                case.case,
                refused.body
            );
            // The refusal removed nothing: the registration reads exactly as
            // it did, and so does every grant on it.
            let read = get_location(&emulator, case.account_id, case.region, &location_id).await;
            assert_eq!(
                read.body, registered.body,
                "case {:?}: expected the refused deregistration to leave the registration alone, \
                 got body {}",
                case.case, read.body
            );
            for still_there in &grant_ids[deleted_so_far..] {
                let grant = get_grant(&emulator, case.account_id, case.region, still_there).await;
                assert_eq!(
                    grant.status, 200,
                    "case {:?}: expected the refused deregistration to leave grant \
                     {still_there:?} alone, got {} with body {}",
                    case.case, grant.status, grant.body
                );
            }

            let released = delete_grant(&emulator, case.account_id, case.region, grant_id).await;
            assert_eq!(
                released.status, NO_CONTENT,
                "case {:?}: expected DeleteAccessGrant to answer {NO_CONTENT}, got {} with \
                 body {}",
                case.case, released.status, released.body
            );
            let gone = get_grant(&emulator, case.account_id, case.region, grant_id).await;
            assert_eq!(
                (gone.status, gone.code()),
                (404, "NoSuchAccessGrantError".to_string()),
                "case {:?}: expected the deleted grant to be gone, got {} with body {}",
                case.case,
                gone.status,
                gone.body
            );
            assert!(
                gone.body.contains(NO_SUCH_GRANT_MESSAGE),
                "case {:?}: expected the missing-grant message, got body {}",
                case.case,
                gone.body
            );
        }

        // With no grant left on it the location is deregistered, and the
        // modelled success carries no body.
        let deleted = delete_location(&emulator, case.account_id, case.region, &location_id).await;
        assert_eq!(
            deleted.status, NO_CONTENT,
            "case {:?}: expected DeleteAccessGrantsLocation to answer {NO_CONTENT}, got {} with \
             body {}",
            case.case, deleted.status, deleted.body
        );
        assert!(
            deleted.body.trim().is_empty(),
            "case {:?}: expected the deregistration to answer an empty body, got {}",
            case.case,
            deleted.body
        );

        // The id now names no registration, for every call that addresses one.
        assert_no_such_location(
            case.case,
            "GetAccessGrantsLocation",
            &get_location(&emulator, case.account_id, case.region, &location_id).await,
        );
        assert_no_such_location(
            case.case,
            "UpdateAccessGrantsLocation",
            &update_location(
                &emulator,
                case.account_id,
                case.region,
                &location_id,
                UPDATE_ROLE,
            )
            .await,
        );
        assert_no_such_location(
            case.case,
            "a second DeleteAccessGrantsLocation",
            &delete_location(&emulator, case.account_id, case.region, &location_id).await,
        );
    }
}

#[tokio::test]
async fn deregistering_one_location_leaves_every_other_registration_and_grant_alone() {
    let cases = delete_cases();
    let emulator = emulator_with_instances(&cases).await;

    // Register every case's location and grants first, so each deregistration
    // below runs against a store holding all of them.
    let mut registered = Vec::new();
    for case in &cases {
        registered.push(register(&emulator, case).await);
    }

    for (index, case) in cases.iter().enumerate() {
        let (location_id, grant_ids) = &registered[index];
        for grant_id in grant_ids {
            assert_eq!(
                delete_grant(&emulator, case.account_id, case.region, grant_id)
                    .await
                    .status,
                NO_CONTENT,
                "case {:?}: expected the grant to be deletable",
                case.case
            );
        }
        let deleted = delete_location(&emulator, case.account_id, case.region, location_id).await;
        assert_eq!(
            deleted.status, NO_CONTENT,
            "case {:?}: expected the emptied location to be deregistered, got {} with body {}",
            case.case, deleted.status, deleted.body
        );

        // Every registration this call was not about still reads, and so does
        // every grant on it: a deregistration is about one location.
        for (other_index, other) in cases.iter().enumerate() {
            if other_index <= index {
                continue;
            }
            let (other_location, other_grants) = &registered[other_index];
            let read = get_location(&emulator, other.account_id, other.region, other_location).await;
            assert_eq!(
                read.text("LocationScope").unwrap_or_default(),
                other.location_scope,
                "case {:?}: expected deregistering {:?} to leave this registration alone, got \
                 body {}",
                other.case,
                case.case,
                read.body
            );
            for grant_id in other_grants {
                let grant = get_grant(&emulator, other.account_id, other.region, grant_id).await;
                assert_eq!(
                    grant.status, 200,
                    "case {:?}: expected deregistering {:?} to leave grant {grant_id:?} alone, \
                     got {} with body {}",
                    other.case, case.case, grant.status, grant.body
                );
            }
        }
    }
}

#[tokio::test]
async fn only_the_grants_on_this_location_hold_its_deregistration_back() {
    let cases = delete_cases();
    let emulator = emulator_with_instances(&cases).await;

    // Two locations in ONE instance: the grant is on the first one only.
    let granted = create_location(&emulator, ACCOUNT, REGION, "s3://held-bucket/").await;
    let granted_id = granted.text("AccessGrantsLocationId").unwrap_or_default();
    let free = create_location(&emulator, ACCOUNT, REGION, "s3://free-bucket/").await;
    let free_id = free.text("AccessGrantsLocationId").unwrap_or_default();
    let grant = create_grant(
        &emulator,
        ACCOUNT,
        REGION,
        &granted_id,
        Some("team-a/*"),
        PERMISSION_READ,
    )
    .await;
    assert_eq!(
        grant.status, 200,
        "expected the grant on the first location to be created, got {} with body {}",
        grant.status, grant.body
    );
    let grant_id = grant.text("AccessGrantId").unwrap_or_default();

    // The location the grant does NOT reference is deregistered straight away,
    // though the instance holds a grant.
    let deleted = delete_location(&emulator, ACCOUNT, REGION, &free_id).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "expected a location no grant references to be deregistered, got {} with body {}",
        deleted.status, deleted.body
    );
    // The one it does reference is refused until that grant is deleted.
    let refused = delete_location(&emulator, ACCOUNT, REGION, &granted_id).await;
    assert_eq!(
        (refused.status, refused.code()),
        (400, "AccessGrantsLocationNotEmptyError".to_string()),
        "expected the referenced location to be refused, got {} with body {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        delete_grant(&emulator, ACCOUNT, REGION, &grant_id)
            .await
            .status,
        NO_CONTENT,
        "expected the grant to be deletable"
    );
    let deleted = delete_location(&emulator, ACCOUNT, REGION, &granted_id).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "expected the location to be deregistered once its grant was deleted, got {} with body {}",
        deleted.status, deleted.body
    );
}

#[tokio::test]
async fn an_id_the_instance_holds_no_location_of_is_refused_and_removes_nothing() {
    let cases = delete_cases();
    let emulator = emulator_with_instances(&cases).await;
    let case = &cases[1];
    let (location_id, grant_ids) = register(&emulator, case).await;
    let registered = get_location(&emulator, case.account_id, case.region, &location_id).await;

    // Each row is an address no location of the request's instance sits at: a
    // never-assigned id, the default id with no default location registered,
    // and the registered id in another Region's instance.
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
            location_id.clone(),
        ),
    ];

    for (label, account_id, region, unknown_id) in unknown {
        let refused = delete_location(&emulator, account_id, region, &unknown_id).await;
        assert_no_such_location(label, "DeleteAccessGrantsLocation", &refused);
    }

    // The same deregistration addressed in another account is refused before
    // the location is resolved.
    assert_foreign_account_refused(
        "DeleteAccessGrantsLocation for another account",
        OTHER_ACCOUNT,
        &delete_location(&emulator, OTHER_ACCOUNT, case.region, &location_id).await,
    );

    // The refused deregistrations released nothing: the registration and its
    // grants are all still there.
    let read = get_location(&emulator, case.account_id, case.region, &location_id).await;
    assert_eq!(
        read.body, registered.body,
        "expected the refused deregistrations to leave the registration alone, got body {}",
        read.body
    );
    for grant_id in &grant_ids {
        let grant = get_grant(&emulator, case.account_id, case.region, grant_id).await;
        assert_eq!(
            grant.status, 200,
            "expected the refused deregistrations to leave grant {grant_id:?} alone, got {} with \
             body {}",
            grant.status, grant.body
        );
    }

    // A grant id that instance holds no grant of is refused the same way, and
    // deletes nothing.
    let refused = delete_grant(&emulator, case.account_id, case.region, "no-such-grant").await;
    assert_eq!(
        (refused.status, refused.code()),
        (404, "NoSuchAccessGrantError".to_string()),
        "expected deleting an unknown grant to be refused as not existing, got {} with body {}",
        refused.status,
        refused.body
    );
    assert!(
        refused.body.contains(NO_SUCH_GRANT_MESSAGE),
        "expected the missing-grant message, got body {}",
        refused.body
    );
    for grant_id in &grant_ids {
        assert_eq!(
            get_grant(&emulator, case.account_id, case.region, grant_id)
                .await
                .status,
            200,
            "expected the refused grant deletion to leave grant {grant_id:?} alone"
        );
    }
}
