// SPDX-License-Identifier: CC0-1.0

//! The ONE rule both operations of the user-association plane resolve a directory's
//! registration by, driven over the real wire path: every call goes through the router's
//! `dispatch`, so each request is deserialized, constraint-checked, handled and serialized
//! exactly as a client's would be.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded request:
//! `AssociateUser` and `ListUserAssociations` both require a LIVE registration for the
//! directory key they resolved — one state-layer rule, asked by both — and a directory
//! with none is each operation's own modelled `ResourceNotFoundException` (HTTP 404)
//! carrying that plane's single sentence. Because the two handlers ask through the same
//! rule, where it sits in each handler's sequence is part of the behaviour:
//!
//! * the instance-authorization gate runs BEFORE it in both handlers, so an instance the
//!   account's fleet does not hold is the fail-closed refusal whatever the directory;
//! * in `AssociateUser` the rule runs BEFORE the request's `Tags` are checked, so an
//!   unregistered directory is answered even when the payload also carries an illegal tag;
//! * in `ListUserAssociations` `Filters` are checked BEFORE it (well-formedness does not
//!   depend on what the store holds) and pagination AFTER it, so a malformed filter wins
//!   and a `MaxResults`/`NextToken` the service refuses does not;
//! * and a refused `AssociateUser` mints nothing.
//!
//! Each row pairs a directory with no live registration, or one with a registration, with
//! whatever ELSE about the request this service refuses, and the rows differ along what the
//! rule generalises over: the operation, the form the provider union addresses the
//! directory by, whether the directory is registered, which instance of the account's fleet
//! is named, and which other member of the request is refusable.
//!
//! The harness (router + engine + the account's directory, network and instance fleets),
//! the provider forms and the request builders are the siblings' — see
//! [`super::identity_provider_subscriptions`],
//! [`super::associations_follow_their_registration`] and
//! [`super::resource_resolution_refusals`] — so there is one wire harness for the service
//! and only the rule is new here.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    IDENTITY_PROVIDER_NOT_REGISTERED, MAX_TAG_VALUE_CHARS, RESERVED_TAG_PREFIX,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service, and one copy of each request builder: these come from the
// sibling suites rather than a second copy of the same plumbing.
use crate::associations_follow_their_registration::{registration, Directory};
use crate::identity_provider_subscriptions::{call, harness, register, Harness, Reply, PRODUCTS};
use crate::resource_resolution_refusals::{assert_fails_closed, associate, held_instance};

/// The two operations of the user-association plane: the ones that resolve a directory's
/// registration before they create or report an association through it.
#[derive(Debug, Clone, Copy)]
enum Op {
    AssociateUser,
    ListUserAssociations,
}

/// What else about the request this service refuses, beside the directory's registration.
#[derive(Debug, Clone, Copy)]
enum AlsoWrong {
    /// Nothing: the registration is the only thing to answer for.
    Nothing,
    /// An EC2 instance the account's fleet does not hold, which the authorization gate
    /// refuses before any of this service's own resources is looked at.
    UnheldInstance,
    /// A tag key in the namespace AWS reserves (`AssociateUser` only).
    ReservedTagKey,
    /// A tag value past the longest this service stores (`AssociateUser` only).
    OverLongTagValue,
    /// A filter on an attribute the operation does not expose (`ListUserAssociations`
    /// only).
    UnexposedFilter,
    /// A page size that could make no progress (`ListUserAssociations` only).
    PageSizeOfZero,
    /// A pagination cursor no page of this list minted (`ListUserAssociations` only).
    ForeignPageToken,
}

/// The answer a row expects.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The plane's modelled `ResourceNotFoundException` for a directory with no live
    /// registration: the shared rule answered.
    NotRegistered,
    /// The authorization gate's fixed refusal: it runs ahead of the shared rule.
    FailsClosed,
    /// The modelled `ValidationException` for the OTHER thing wrong with the request:
    /// whatever this handler asks before the shared rule, or asks once it passed.
    Validation,
}

#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    op: Op,
    directory: Directory,
    /// Whether the row registers the directory it names for one product first.
    registered: bool,
    /// Index into the account's seeded EC2 fleet, so a row names an instance the
    /// authorization gate holds without hard-coding its id.
    instance: usize,
    also: AlsoWrong,
    expected: Expected,
}

/// An instance id the account's fleet does not hold, in EC2's 17-hex-digit id form.
const UNHELD_INSTANCE: &str = "i-0123456789abcdef0";

/// The user every `AssociateUser` row would associate: which user is named is varied by
/// the suites that own that rule, so one name is used throughout here.
const USERNAME: &str = "alice";

const ROWS: &[Row] = &[
    Row {
        case: "AssociateUser through a directory of the account nobody registered",
        op: Op::AssociateUser,
        directory: Directory::AwsManaged("d-123456789a"),
        registered: false,
        instance: 0,
        also: AlsoWrong::Nothing,
        expected: Expected::NotRegistered,
    },
    Row {
        case: "ListUserAssociations of a directory shared into the account, unregistered",
        op: Op::ListUserAssociations,
        directory: Directory::SharedIntoAccount("sd-abcdef0123"),
        registered: false,
        instance: 1,
        also: AlsoWrong::Nothing,
        expected: Expected::NotRegistered,
    },
    Row {
        case: "AssociateUser, unregistered directory and a reserved tag key",
        op: Op::AssociateUser,
        directory: Directory::SelfManaged("self.managed.example"),
        registered: false,
        instance: 2,
        also: AlsoWrong::ReservedTagKey,
        expected: Expected::NotRegistered,
    },
    Row {
        case: "AssociateUser, unregistered directory and an over-long tag value",
        op: Op::AssociateUser,
        directory: Directory::AwsManaged("d-123456789a"),
        registered: false,
        instance: 1,
        also: AlsoWrong::OverLongTagValue,
        expected: Expected::NotRegistered,
    },
    Row {
        case: "AssociateUser, registered directory and a reserved tag key",
        op: Op::AssociateUser,
        directory: Directory::AwsManaged("d-0123456789"),
        registered: true,
        instance: 0,
        also: AlsoWrong::ReservedTagKey,
        expected: Expected::Validation,
    },
    Row {
        case: "AssociateUser, registered self-managed directory and an over-long tag value",
        op: Op::AssociateUser,
        directory: Directory::SelfManaged("registered.example"),
        registered: true,
        instance: 2,
        also: AlsoWrong::OverLongTagValue,
        expected: Expected::Validation,
    },
    Row {
        case: "ListUserAssociations, unregistered directory and an unexposed filter",
        op: Op::ListUserAssociations,
        directory: Directory::AwsManaged("d-123456789a"),
        registered: false,
        instance: 0,
        also: AlsoWrong::UnexposedFilter,
        expected: Expected::Validation,
    },
    Row {
        case: "ListUserAssociations, registered directory and the same unexposed filter",
        op: Op::ListUserAssociations,
        directory: Directory::AwsManaged("d-00000000ff"),
        registered: true,
        instance: 0,
        also: AlsoWrong::UnexposedFilter,
        expected: Expected::Validation,
    },
    Row {
        case: "ListUserAssociations, unregistered directory and a page size of zero",
        op: Op::ListUserAssociations,
        directory: Directory::SelfManaged("self.managed.example"),
        registered: false,
        instance: 1,
        also: AlsoWrong::PageSizeOfZero,
        expected: Expected::NotRegistered,
    },
    Row {
        case: "ListUserAssociations, unregistered directory and a foreign page token",
        op: Op::ListUserAssociations,
        directory: Directory::SharedIntoAccount("sd-abcdef0123"),
        registered: false,
        instance: 2,
        also: AlsoWrong::ForeignPageToken,
        expected: Expected::NotRegistered,
    },
    Row {
        case: "ListUserAssociations, registered directory and the same foreign page token",
        op: Op::ListUserAssociations,
        directory: Directory::AwsManaged("d-00000000ff"),
        registered: true,
        instance: 1,
        also: AlsoWrong::ForeignPageToken,
        expected: Expected::Validation,
    },
    Row {
        case: "AssociateUser, unregistered directory on an instance the fleet does not hold",
        op: Op::AssociateUser,
        directory: Directory::AwsManaged("d-123456789a"),
        registered: false,
        instance: 0,
        also: AlsoWrong::UnheldInstance,
        expected: Expected::FailsClosed,
    },
    Row {
        case: "ListUserAssociations, unregistered directory on an instance the fleet does not hold",
        op: Op::ListUserAssociations,
        directory: Directory::SelfManaged("self.managed.example"),
        registered: false,
        instance: 0,
        also: AlsoWrong::UnheldInstance,
        expected: Expected::FailsClosed,
    },
];

/// `ListUserAssociations` is bound to POST /user/ListUserAssociations (smithy_query:
/// operation ListUserAssociations, http.uri). The sibling suite's helper builds the scope
/// only, and these rows carry further members, so the request goes through the shared wire
/// helper here.
async fn list_associations(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListUserAssociations", body).await
}

/// The request a row sends: the scope both operations require, plus whatever else about it
/// this service refuses.
fn request(row: &Row, instance_id: &str) -> Value {
    let mut body = json!({
        "InstanceId": instance_id,
        "IdentityProvider": row.directory.union(),
    });
    if matches!(row.op, Op::AssociateUser) {
        body["Username"] = json!(USERNAME);
    }
    match row.also {
        AlsoWrong::Nothing | AlsoWrong::UnheldInstance => {}
        AlsoWrong::ReservedTagKey => {
            let mut tags = serde_json::Map::new();
            tags.insert(format!("{RESERVED_TAG_PREFIX}owner"), json!("finance"));
            body["Tags"] = Value::Object(tags);
        }
        AlsoWrong::OverLongTagValue => {
            body["Tags"] = json!({ "owner": "v".repeat(MAX_TAG_VALUE_CHARS + 1) });
        }
        AlsoWrong::UnexposedFilter => {
            body["Filters"] = json!([{ "Attribute": "Nope", "Value": "x" }]);
        }
        AlsoWrong::PageSizeOfZero => body["MaxResults"] = json!(0),
        AlsoWrong::ForeignPageToken => body["NextToken"] = json!("not-a-token"),
    }
    body
}

/// The plane's one refusal for a directory no live registration of the account is for.
fn assert_not_registered(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: ResourceNotFoundException is modelled at HTTP 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.message(),
        Some(IDENTITY_PROVIDER_NOT_REGISTERED),
        "case {case:?}: expected {IDENTITY_PROVIDER_NOT_REGISTERED:?}, got {:?}",
        reply.message()
    );
}

/// The modelled refusal of the OTHER thing wrong with the request.
fn assert_validation(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?}",
        reply.error_type
    );
    assert_ne!(
        reply.message(),
        Some(IDENTITY_PROVIDER_NOT_REGISTERED),
        "case {case:?}: the registration rule's sentence is not this refusal's, got {:?}",
        reply.message()
    );
}

/// How many user associations the STORE holds, read through the engine the handlers write:
/// a refused association must mint nothing.
fn held_association_count(h: &Harness) -> usize {
    let store = h.engine.state::<ServiceState>();
    let guard = store.lock();
    guard.instance_users.len()
}

/// RULE: both operations of the user-association plane require a live registration for the
/// directory key they resolved, through the one state-layer rule, and that rule keeps its
/// place in each handler's sequence — the authorization gate ahead of it in both, `Filters`
/// ahead of it and pagination behind it in the list, the `Tags` check behind it in the
/// associate.
#[tokio::test]
async fn one_registration_rule_answers_both_operations_in_its_own_place() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        if row.registered {
            // One registration of the directory the row names is all this plane resolves:
            // an association carries no `Product`.
            register(&h, &registration(&row.directory.union(), PRODUCTS[0]))
                .await
                .ok(&format!("{case}: registering the row's directory"));
        }
        let instance_id = match row.also {
            AlsoWrong::UnheldInstance => UNHELD_INSTANCE.to_string(),
            _ => held_instance(row.instance),
        };

        let body = request(row, &instance_id);
        let reply = match row.op {
            Op::AssociateUser => associate(&h, &body).await,
            Op::ListUserAssociations => list_associations(&h, &body).await,
        };

        match row.expected {
            Expected::NotRegistered => assert_not_registered(case, &reply),
            Expected::FailsClosed => assert_fails_closed(case, &instance_id, &reply),
            Expected::Validation => assert_validation(case, &reply),
        }
        assert_eq!(
            held_association_count(&h),
            0,
            "case {case:?}: a refused call must mint no association, got {:?}",
            reply.body
        );
    }
}

/// RULE, from the other side: the same rule's PASSING answer is what both operations go on
/// through — a directory with a live registration is associated through and listed from, so
/// the refusals above report the absence of a registration and nothing about the requests.
#[tokio::test]
async fn the_same_rule_lets_a_registered_directory_through_both_operations() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        let provider = row.directory.union();
        register(&h, &registration(&provider, PRODUCTS[0]))
            .await
            .ok(&format!("{case}: registering the row's directory"));
        let instance_id = held_instance(row.instance);

        let reply = associate(
            &h,
            &json!({
                "InstanceId": &instance_id,
                "IdentityProvider": &provider,
                "Username": USERNAME,
            }),
        )
        .await;
        let created = reply.ok(&format!("AssociateUser: {case}"));
        assert_eq!(
            created["InstanceUserSummary"]["Username"].as_str(),
            Some(USERNAME),
            "case {case:?}: expected the associated user, got {:?}",
            created["InstanceUserSummary"]
        );

        let reply = list_associations(
            &h,
            &json!({ "InstanceId": &instance_id, "IdentityProvider": &provider }),
        )
        .await;
        let listed = reply.ok(&format!("ListUserAssociations: {case}"));
        let usernames: Vec<&str> = listed["InstanceUserSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummaries in {listed:?}"))
            .iter()
            .filter_map(|summary| summary["Username"].as_str())
            .collect();
        assert_eq!(
            usernames,
            vec![USERNAME],
            "case {case:?}: expected the association just made, got {usernames:?}"
        );
    }
}
