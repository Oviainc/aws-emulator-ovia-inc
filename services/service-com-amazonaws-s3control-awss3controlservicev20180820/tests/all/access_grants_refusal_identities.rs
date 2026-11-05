// SPDX-License-Identifier: CC0-1.0

//! What an S3 Access Grants refusal says, driven end to end over HTTP.
//!
//! The rule under test: a refusal on this surface states the CLASS of failure in
//! the fixed words the service answers for it and carries every value that is
//! about THIS request in a member beside that message — the id the request named
//! the resource by, in the `<Error>` member of the refusal's own class, plus the
//! `RequestId` and `HostId` that name the response — so no identifier is
//! interpolated into the message, a referential-integrity refusal is the 400 a
//! caller has to send different requests to clear rather than a retryable
//! conflict, and deleting a resource makes every later read of it the
//! missing-resource refusal.
//!
//! Every table below varies what the rule generalises over: the id the request
//! names (both modelled length bounds, hyphenated, mixed case, the documented
//! default location id, the id of a resource that really was there), which
//! operation addresses it, the account and the Region that own the instance, and
//! how many locations and grants the instance still holds.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder, the XML readers and the calls that create an instance,
//! register a location, create a grant and read any of them back are the sibling
//! suites', reused rather than copied.

use crate::access_grants_data_plane::{
    create_grant, create_instance, create_location, get_grant, get_location,
};
use crate::access_grants_instance::get as get_instance;
use crate::access_grants_location_delete::{delete_grant, delete_location};
use crate::access_grants_location_update::update_location;
use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_LOCATION_DEFAULT_ID, DEFAULT_LOCATION_SCOPE, PERMISSION_READ, PERMISSION_WRITE,
};

/// A second signing Region, used to pin that an instance, its locations and its
/// grants belong to one account in one Region.
const OTHER_REGION: &str = "eu-central-1";

/// A third signing Region, so a table can leave one Region with no instance in
/// it at all.
const THIRD_REGION: &str = "ap-southeast-2";

/// The fixed sentence S3 Access Grants answers when the account has no Access
/// Grants instance in the Region the request was signed for. No variable part:
/// the account and the Region are the request's own.
const NO_INSTANCE_MESSAGE: &str = "Access Grants Instance does not exist";

/// The fixed sentence S3 Access Grants answers for a location id its instance
/// holds no registration for.
const NO_SUCH_LOCATION_MESSAGE: &str = "The specified access grants location does not exist";

/// The fixed sentence S3 Access Grants answers for a grant id its instance holds
/// no grant of.
const NO_SUCH_GRANT_MESSAGE: &str = "The specified access grant does not exist";

/// The fixed sentence S3 Access Grants answers while grants still reference the
/// location a request asks to deregister.
const LOCATION_NOT_EMPTY_MESSAGE: &str =
    "Please delete access grants before deleting access grants location";

/// The fixed sentence S3 Access Grants answers while the instance still holds a
/// registered location or a grant.
const INSTANCE_NOT_EMPTY_MESSAGE: &str =
    "Please delete access grants and locations before deleting access grants instance";

/// The role an update moves a location to, for the update that is refused
/// because no such location is registered. Its shape is the modelled
/// `IAMRoleArn` pattern, `^arn:[^:]+:iam::\d{12}:role/`.
const UPDATE_ROLE: &str = "arn:aws:iam::123456789012:role/never-stored";

/// A resource policy document, for the put that is refused because the account
/// has no instance to put it on.
const POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                      \"Principal\":{\"AWS\":\"*\"},\"Action\":[\"s3:GetDataAccess\"],\
                      \"Resource\":\"*\"}]}";

/// Delete the Access Grants instance of `account_id` in `region`.
async fn delete_instance(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        "/v20180820/accessgrantsinstance",
        account_id,
        "",
    )
    .await
}

/// Store a resource policy on the instance of `account_id` in `region`.
async fn put_policy(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    let body = format!(
        "<PutAccessGrantsInstanceResourcePolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
         </PutAccessGrantsInstanceResourcePolicyRequest>",
        xml_escape(POLICY)
    );
    call_in_region(
        &emulator.router,
        region,
        "PUT",
        "/v20180820/accessgrantsinstance/resourcepolicy",
        account_id,
        &body,
    )
    .await
}

/// Read the resource policy of the instance of `account_id` in `region`.
async fn get_policy(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/accessgrantsinstance/resourcepolicy",
        account_id,
        "",
    )
    .await
}

/// Remove the resource policy of the instance of `account_id` in `region`.
async fn delete_policy(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        "/v20180820/accessgrantsinstance/resourcepolicy",
        account_id,
        "",
    )
    .await
}

/// Whether every character of `value` is a hex digit of the case `upper` asks
/// for.
fn is_hex(value: &str, upper: bool) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_hexdigit()
                && (character.is_ascii_digit()
                    || (upper && character.is_ascii_uppercase())
                    || (!upper && character.is_ascii_lowercase()))
        })
}

/// Assert `reply` names the request it answers: the `RequestId` and `HostId`
/// elements beside `<Error>`, in the shape an S3-family service renders them,
/// mirrored by the two correlation headers.
///
/// `pub(crate)` because every refusal of this service names the response it is,
/// so the sibling suites assert the same pair rather than copying it.
pub(crate) fn assert_names_the_request(case: &str, call: &str, reply: &Reply) {
    let request_id = reply.text("RequestId").unwrap_or_default();
    let host_id = reply.text("HostId").unwrap_or_default();
    assert!(
        is_hex(&request_id, true),
        "case {case:?}: expected {call} to carry an upper-case hex RequestId, got \
         {request_id:?} in {}",
        reply.body
    );
    assert!(
        is_hex(&host_id, false),
        "case {case:?}: expected {call} to carry a lower-case hex HostId, got {host_id:?} in {}",
        reply.body
    );
    assert_eq!(
        reply.header("x-amz-request-id").as_deref(),
        Some(request_id.as_str()),
        "case {case:?}: expected {call}'s x-amz-request-id header to mirror the element, got {:?}",
        reply.header("x-amz-request-id")
    );
    assert_eq!(
        reply.header("x-amz-id-2").as_deref(),
        Some(host_id.as_str()),
        "case {case:?}: expected {call}'s x-amz-id-2 header to mirror the element, got {:?}",
        reply.header("x-amz-id-2")
    );
}

/// One refusal's expected envelope: the status, the wire code, the fixed
/// message, and the `<Error>` member that carries the identity the request named
/// (none for a refusal about a resource that has no id of its own).
struct Expected<'a> {
    status: u16,
    code: &'a str,
    message: &'a str,
    named: Option<(&'a str, &'a str)>,
}

/// Assert `reply` is the refusal `expected` describes, whole: the status and code
/// a client branches on, the message as fixed text with nothing interpolated into
/// it, the member that names what the request named, and the correlation pair.
fn assert_refused(case: &str, call: &str, reply: &Reply, expected: &Expected<'_>) {
    assert_eq!(
        (reply.status, reply.code()),
        (expected.status, expected.code.to_string()),
        "case {case:?}: expected {call} to answer {} {}, got {} with body {}",
        expected.status,
        expected.code,
        reply.status,
        reply.body
    );
    let message = reply.text("Message").unwrap_or_default();
    assert_eq!(
        message, expected.message,
        "case {case:?}: expected {call} to answer the fixed message {:?}, got {message:?}",
        expected.message
    );
    match expected.named {
        None => {}
        Some((element, value)) => {
            let named = reply.text(element);
            assert_eq!(
                named.as_deref(),
                Some(value),
                "case {case:?}: expected {call} to name the resource it is about in \
                 <{element}>{value}</{element}>, got {named:?} in body {}",
                reply.body
            );
        }
    }
    assert_names_the_request(case, call, reply);
}

/// The ids every missing-resource refusal below is asked about.
///
/// Each row is a value the modelled id admits — `AccessGrantId` and
/// `AccessGrantsLocationId` are both `len 1..64`, pattern `^[a-zA-Z0-9\-]+$`
/// (`smithy_query(query="operation", operation="GetAccessGrant")` and
/// `…operation="GetAccessGrantsLocation"`) — and differs from the others in
/// something the rule generalises over: its length, at both modelled bounds,
/// whether it carries hyphens, its case, and whether it is an id this service
/// hands out at all.
fn unknown_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model admits", "a".to_string()),
        ("the longest id the model admits", "z".repeat(64)),
        ("a hyphenated id", "no-such-resource-0001".to_string()),
        ("a mixed-case id", "NoSuchResource".to_string()),
        (
            "the documented default location id, with no default location",
            ACCESS_GRANTS_LOCATION_DEFAULT_ID.to_string(),
        ),
    ]
}

#[tokio::test]
async fn every_grant_refusal_names_the_grant_id_the_request_asked_about() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "expected the instance to be created"
    );

    // One row is the id of a grant that really was there and was deleted, so the
    // refusal is asserted for a service-minted id as well as for ids no create
    // ever handed out.
    let location = create_location(&emulator, ACCOUNT, REGION, "s3://reports-bucket/").await;
    let location_id = location.text("AccessGrantsLocationId").unwrap_or_default();
    let grant = create_grant(
        &emulator,
        ACCOUNT,
        REGION,
        &location_id,
        Some("team-a/*"),
        PERMISSION_READ,
    )
    .await;
    let deleted_id = grant.text("AccessGrantId").unwrap_or_default();
    assert_eq!(
        delete_grant(&emulator, ACCOUNT, REGION, &deleted_id)
            .await
            .status,
        200,
        "expected the grant to be deletable"
    );

    let mut cases = unknown_ids();
    cases.push(("the id of a deleted grant", deleted_id));
    // The grant read from the instance in another Region is addressed by an id
    // that exists, in an instance that does not hold it.
    assert_eq!(
        create_instance(&emulator, ACCOUNT, OTHER_REGION).await.status,
        200,
        "expected the instance in the other Region to be created"
    );

    for (case, grant_id) in &cases {
        let expected = Expected {
            status: 404,
            code: "NoSuchAccessGrantError",
            message: NO_SUCH_GRANT_MESSAGE,
            named: Some(("AccessGrantId", grant_id)),
        };
        for (call, reply) in [
            (
                "GetAccessGrant",
                get_grant(&emulator, ACCOUNT, REGION, grant_id).await,
            ),
            (
                "DeleteAccessGrant",
                delete_grant(&emulator, ACCOUNT, REGION, grant_id).await,
            ),
            (
                "GetAccessGrant in another Region's instance",
                get_grant(&emulator, ACCOUNT, OTHER_REGION, grant_id).await,
            ),
        ] {
            assert_refused(case, call, &reply, &expected);
        }
    }

    // The same read addressed in another account is refused before any grant is
    // resolved, so it names no grant id at all.
    assert_foreign_account_refused(
        "GetAccessGrant for another account",
        OTHER_ACCOUNT,
        &get_grant(&emulator, OTHER_ACCOUNT, REGION, &cases[0].1).await,
    );
}

#[tokio::test]
async fn every_location_refusal_names_the_location_id_the_request_asked_about() {
    let emulator = Emulator::start();
    for region in [REGION, OTHER_REGION] {
        assert_eq!(
            create_instance(&emulator, ACCOUNT, region).await.status,
            200,
            "expected the instance in {region:?} to be created"
        );
    }

    // As in the grant suite above, one row is an id this service really assigned
    // and then released.
    let location = create_location(&emulator, ACCOUNT, REGION, "s3://finance-bucket/").await;
    let released_id = location.text("AccessGrantsLocationId").unwrap_or_default();
    assert_eq!(
        delete_location(&emulator, ACCOUNT, REGION, &released_id)
            .await
            .status,
        200,
        "expected the location to be deregistered"
    );
    let mut cases = unknown_ids();
    cases.push(("the id of a deregistered location", released_id.clone()));
    // The same id in the instance of another Region, which never held it.
    cases.push((
        "a registered id read in another Region's instance",
        released_id,
    ));

    for (case, location_id) in &cases {
        let expected = Expected {
            status: 404,
            code: "NoSuchAccessGrantsLocationError",
            message: NO_SUCH_LOCATION_MESSAGE,
            named: Some(("AccessGrantsLocationId", location_id)),
        };
        for (call, reply) in [
            (
                "GetAccessGrantsLocation",
                get_location(&emulator, ACCOUNT, REGION, location_id).await,
            ),
            (
                "DeleteAccessGrantsLocation",
                delete_location(&emulator, ACCOUNT, REGION, location_id).await,
            ),
            (
                "UpdateAccessGrantsLocation",
                update_location(&emulator, ACCOUNT, REGION, location_id, UPDATE_ROLE).await,
            ),
            (
                "CreateAccessGrant on an unregistered location",
                create_grant(
                    &emulator,
                    ACCOUNT,
                    REGION,
                    location_id,
                    Some("team-a/*"),
                    PERMISSION_READ,
                )
                .await,
            ),
            (
                "GetAccessGrantsLocation in another Region's instance",
                get_location(&emulator, ACCOUNT, OTHER_REGION, location_id).await,
            ),
        ] {
            assert_refused(case, call, &reply, &expected);
        }
    }
}

/// One referential-integrity case: a registration and the grants on it, which
/// hold its deregistration back.
struct NotEmptyCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    location_scope: &'static str,
    /// One (sub-prefix, permission) pair per grant on the location.
    grants: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn the_referential_integrity_refusal_is_a_bad_request_naming_the_location() {
    // The rows differ in the form of the registered scope, how many grants hold
    // the deregistration back, the permissions they carry, and the account and
    // Region whose instance holds them.
    let cases = vec![
        NotEmptyCase {
            case: "a bucket, one grant",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://reports-bucket",
            grants: vec![("team-a/*", PERMISSION_READ)],
        },
        NotEmptyCase {
            case: "a bucket and prefix, two grants",
            account_id: ACCOUNT,
            region: OTHER_REGION,
            location_scope: "s3://finance-bucket/quarterly/",
            grants: vec![("2024/*", PERMISSION_READ), ("2025/*", PERMISSION_WRITE)],
        },
        NotEmptyCase {
            // The default location carries the documented id rather than a
            // minted one, so the member names that id here.
            case: "the default location, three grants, a third Region",
            account_id: ACCOUNT,
            region: THIRD_REGION,
            location_scope: DEFAULT_LOCATION_SCOPE,
            grants: vec![
                ("archive-bucket/*", PERMISSION_READ),
                ("archive-bucket/2024/*", PERMISSION_WRITE),
                ("staging-bucket/report.csv", PERMISSION_READ),
            ],
        },
    ];

    let emulator = Emulator::start();
    for case in &cases {
        assert_eq!(
            create_instance(&emulator, case.account_id, case.region)
                .await
                .status,
            200,
            "case {:?}: expected the instance to be created",
            case.case
        );
        let registered =
            create_location(&emulator, case.account_id, case.region, case.location_scope).await;
        let location_id = registered
            .text("AccessGrantsLocationId")
            .unwrap_or_default();
        let mut grant_ids = Vec::new();
        for (sub_prefix, permission) in &case.grants {
            let granted = create_grant(
                &emulator,
                case.account_id,
                case.region,
                &location_id,
                Some(sub_prefix),
                permission,
            )
            .await;
            assert_eq!(
                granted.status, 200,
                "case {:?}: expected the grant on {sub_prefix:?} to be created, got {} with \
                 body {}",
                case.case, granted.status, granted.body
            );
            grant_ids.push(granted.text("AccessGrantId").unwrap_or_default());
        }

        let before = get_location(&emulator, case.account_id, case.region, &location_id).await;
        // Refused once per grant still on the location: it is the grants that
        // are left that hold it back, not how many there were.
        for (released, grant_id) in grant_ids.iter().enumerate() {
            let refused =
                delete_location(&emulator, case.account_id, case.region, &location_id).await;
            assert_refused(
                case.case,
                &format!("DeleteAccessGrantsLocation with {} grants left", {
                    grant_ids.len() - released
                }),
                &refused,
                &Expected {
                    status: 400,
                    code: "AccessGrantsLocationNotEmptyError",
                    message: LOCATION_NOT_EMPTY_MESSAGE,
                    named: Some(("AccessGrantsLocationID", &location_id)),
                },
            );
            // The refusal released nothing.
            let after = get_location(&emulator, case.account_id, case.region, &location_id).await;
            assert_eq!(
                after.body, before.body,
                "case {:?}: expected the refusal to leave the registration alone, got body {}",
                case.case, after.body
            );
            assert_eq!(
                delete_grant(&emulator, case.account_id, case.region, grant_id)
                    .await
                    .status,
                200,
                "case {:?}: expected the grant to be deletable",
                case.case
            );
        }

        // With nothing left referencing it the deregistration goes through.
        let deleted = delete_location(&emulator, case.account_id, case.region, &location_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected the emptied location to be deregistered, got {} with body {}",
            case.case, deleted.status, deleted.body
        );
    }
}

#[tokio::test]
async fn an_account_with_no_instance_is_refused_in_fixed_words_by_every_operation() {
    let emulator = Emulator::start();
    // Each row is a pair with no instance: the owning account in two Regions it
    // holds no instance in.
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "expected the one instance to be created"
    );

    let expected = Expected {
        status: 404,
        code: "AccessGrantsInstanceNotExistsError",
        message: NO_INSTANCE_MESSAGE,
        // An instance has no id of its own to name: every instance carries the
        // documented `default` id, and the account and Region are the request's.
        named: None,
    };

    for (case, account_id, region) in [
        ("the owning account in another Region", ACCOUNT, OTHER_REGION),
        ("a Region nothing was created in", ACCOUNT, THIRD_REGION),
    ] {
        for (call, reply) in [
            (
                "GetAccessGrantsInstance",
                get_instance(&emulator, account_id, region).await,
            ),
            (
                "DeleteAccessGrantsInstance",
                delete_instance(&emulator, account_id, region).await,
            ),
            (
                "GetAccessGrantsInstanceResourcePolicy",
                get_policy(&emulator, account_id, region).await,
            ),
            (
                "PutAccessGrantsInstanceResourcePolicy",
                put_policy(&emulator, account_id, region).await,
            ),
            (
                "DeleteAccessGrantsInstanceResourcePolicy",
                delete_policy(&emulator, account_id, region).await,
            ),
            (
                "CreateAccessGrantsLocation",
                create_location(&emulator, account_id, region, "s3://reports-bucket/").await,
            ),
            (
                "CreateAccessGrant",
                create_grant(
                    &emulator,
                    account_id,
                    region,
                    ACCESS_GRANTS_LOCATION_DEFAULT_ID,
                    Some("reports-bucket/*"),
                    PERMISSION_READ,
                )
                .await,
            ),
        ] {
            assert_refused(case, call, &reply, &expected);
            // Nothing the request carried is interpolated into the message: the
            // sentence states the class and nothing else.
            let message = reply.text("Message").unwrap_or_default();
            assert!(
                !message.contains(account_id) && !message.contains(region),
                "case {case:?}: expected {call}'s message to name neither the account nor the \
                 Region, got {message:?}"
            );
        }
    }

    // The one instance that was created is untouched by all of that.
    let read = get_instance(&emulator, ACCOUNT, REGION).await;
    assert_eq!(
        read.status, 200,
        "expected the created instance to still read, got {} with body {}",
        read.status, read.body
    );

    // A call addressed in another account is refused ahead of the instance
    // lookup, so no operation of this surface reports whether that account
    // holds one.
    for (call, reply) in [
        (
            "GetAccessGrantsInstance",
            get_instance(&emulator, OTHER_ACCOUNT, REGION).await,
        ),
        (
            "DeleteAccessGrantsInstance",
            delete_instance(&emulator, OTHER_ACCOUNT, REGION).await,
        ),
        (
            "CreateAccessGrantsLocation",
            create_location(&emulator, OTHER_ACCOUNT, REGION, "s3://reports-bucket/").await,
        ),
    ] {
        assert_foreign_account_refused(call, OTHER_ACCOUNT, &reply);
    }
}

#[tokio::test]
async fn deleting_an_instance_makes_every_later_read_of_it_refuse() {
    let emulator = Emulator::start();
    // The rows differ in the account and the Region that own the instance, so a
    // delete is shown to be about one (account, Region) pair.
    let pairs = [
        ("the owning account", ACCOUNT, REGION),
        ("the same account in another Region", ACCOUNT, OTHER_REGION),
        ("the same account in a third Region", ACCOUNT, THIRD_REGION),
    ];
    for (case, account_id, region) in pairs {
        assert_eq!(
            create_instance(&emulator, account_id, region).await.status,
            200,
            "case {case:?}: expected the instance to be created"
        );
    }

    for (index, (case, account_id, region)) in pairs.iter().enumerate() {
        let before = get_instance(&emulator, account_id, region).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected the instance to read before the delete, got {} with body {}",
            before.status, before.body
        );

        let deleted = delete_instance(&emulator, account_id, region).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected DeleteAccessGrantsInstance to answer 204, got {} with \
             body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.trim().is_empty(),
            "case {case:?}: expected the delete to answer an empty body, got {}",
            deleted.body
        );

        let expected = Expected {
            status: 404,
            code: "AccessGrantsInstanceNotExistsError",
            message: NO_INSTANCE_MESSAGE,
            named: None,
        };
        assert_refused(
            case,
            "GetAccessGrantsInstance after the delete",
            &get_instance(&emulator, account_id, region).await,
            &expected,
        );
        assert_refused(
            case,
            "a second DeleteAccessGrantsInstance",
            &delete_instance(&emulator, account_id, region).await,
            &expected,
        );

        // Every pair this delete was not about still holds its instance.
        for (other_case, other_account, other_region) in pairs.iter().skip(index + 1) {
            let read = get_instance(&emulator, other_account, other_region).await;
            assert_eq!(
                read.status, 200,
                "case {other_case:?}: expected deleting {case:?}'s instance to leave this one \
                 alone, got {} with body {}",
                read.status, read.body
            );
        }

        // The pair is free again: a create records a new instance there, which
        // reads back exactly as a fresh one does.
        let recreated = create_instance(&emulator, account_id, region).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: expected a create after the delete to succeed, got {} with body {}",
            recreated.status, recreated.body
        );
        let read = get_instance(&emulator, account_id, region).await;
        assert_eq!(
            read.text("AccessGrantsInstanceArn"),
            before.text("AccessGrantsInstanceArn"),
            "case {case:?}: expected the re-created instance to report the pair's ARN, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_instance_that_still_holds_locations_or_grants_is_not_deleted() {
    let emulator = Emulator::start();
    // The rows differ in what the instance still holds: a location nothing
    // references, a location with a grant on it, and two locations.
    let cases: Vec<(&str, &str, &str, Vec<&str>, bool)> = vec![
        (
            "one location, no grant",
            ACCOUNT,
            REGION,
            vec!["s3://reports-bucket/"],
            false,
        ),
        (
            "one location with a grant on it",
            ACCOUNT,
            OTHER_REGION,
            vec!["s3://finance-bucket/"],
            true,
        ),
        (
            "two locations, one granted",
            ACCOUNT,
            THIRD_REGION,
            vec!["s3://partner-bucket/inbound/", "s3://partner-bucket/out/"],
            true,
        ),
    ];

    for (case, account_id, region, scopes, grant_on_first) in cases {
        assert_eq!(
            create_instance(&emulator, account_id, region).await.status,
            200,
            "case {case:?}: expected the instance to be created"
        );
        let mut location_ids = Vec::new();
        for scope in &scopes {
            let registered = create_location(&emulator, account_id, region, scope).await;
            assert_eq!(
                registered.status, 200,
                "case {case:?}: expected {scope:?} to be registered, got {} with body {}",
                registered.status, registered.body
            );
            location_ids.push(
                registered
                    .text("AccessGrantsLocationId")
                    .unwrap_or_default(),
            );
        }
        let mut grant_ids = Vec::new();
        if grant_on_first {
            let granted = create_grant(
                &emulator,
                account_id,
                region,
                &location_ids[0],
                Some("team-a/*"),
                PERMISSION_READ,
            )
            .await;
            assert_eq!(
                granted.status, 200,
                "case {case:?}: expected the grant to be created, got {} with body {}",
                granted.status, granted.body
            );
            grant_ids.push(granted.text("AccessGrantId").unwrap_or_default());
        }

        let expected = Expected {
            status: 400,
            code: "AccessGrantsInstanceNotEmptyError",
            message: INSTANCE_NOT_EMPTY_MESSAGE,
            named: None,
        };
        // Refused while anything is still held, and refused again as each held
        // resource is released, until the last one is gone.
        assert_refused(
            case,
            "DeleteAccessGrantsInstance with everything still held",
            &delete_instance(&emulator, account_id, region).await,
            &expected,
        );
        let still_there = get_instance(&emulator, account_id, region).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: expected the refused delete to leave the instance alone, got {} with \
             body {}",
            still_there.status, still_there.body
        );

        for grant_id in &grant_ids {
            assert_eq!(
                delete_grant(&emulator, account_id, region, grant_id)
                    .await
                    .status,
                200,
                "case {case:?}: expected the grant to be deletable"
            );
            assert_refused(
                case,
                "DeleteAccessGrantsInstance with its locations still registered",
                &delete_instance(&emulator, account_id, region).await,
                &expected,
            );
        }
        for (index, location_id) in location_ids.iter().enumerate() {
            assert_eq!(
                delete_location(&emulator, account_id, region, location_id)
                    .await
                    .status,
                200,
                "case {case:?}: expected location {location_id:?} to be deregistered"
            );
            if index + 1 < location_ids.len() {
                assert_refused(
                    case,
                    "DeleteAccessGrantsInstance with a location still registered",
                    &delete_instance(&emulator, account_id, region).await,
                    &expected,
                );
            }
        }

        // With nothing left in it the instance is deleted, and the read refuses.
        let deleted = delete_instance(&emulator, account_id, region).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected the emptied instance to be deleted, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_refused(
            case,
            "GetAccessGrantsInstance after the delete",
            &get_instance(&emulator, account_id, region).await,
            &Expected {
                status: 404,
                code: "AccessGrantsInstanceNotExistsError",
                message: NO_INSTANCE_MESSAGE,
                named: None,
            },
        );
    }
}
