// SPDX-License-Identifier: CC0-1.0

//! Four rules about addressing an identity store and changing what it holds, driven
//! end to end over HTTP through [`crate::harness`]:
//!
//! 1. every operation RESOLVES the `IdentityStoreId` it is given as the store the
//!    CALLER names — any value the modelled pattern admits is served, holding whatever
//!    has been written to it — and only an identifier naming a store of another account
//!    is refused, with the modelled `ResourceNotFoundException` having written
//!    nothing;
//! 2. a user this API creates reports a `UserStatus`, and it is `ENABLED`;
//! 3. a resource created or updated through the API records the CALLING principal, so
//!    a describe reports `CreatedBy` and `UpdatedBy` — the latter naming the principal
//!    of the LAST write, which is the create until something changes the resource;
//! 4. an update refuses an `AttributePath` the service does not let it change — the
//!    resource's own identifiers and service-generated values among them — with the
//!    modelled 400 `ValidationException`, leaving the resource exactly as it was.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: the
//! form of the store id and what it addresses, which operation is addressed, which
//! attribute path is requested and what kind of value it carries.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, emulator, error_message, ok, ok_as, refused, refused_as, string, MISSING_STORE,
    NOT_FOUND, STORE,
};

/// The emulator's account id, for the ARN forms the modelled `IdentityStoreId` pattern
/// admits (`arn:aws[a-z-]*:identitystore::\d{12}:identitystore/<id>`).
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// A well-formed `UserId`/`GroupId` nothing created, so a refusal comes from state
/// rather than from the boundary validator.
const MISSING_RESOURCE: &str = "0123abcd-4567-89ab-cdef-0123456789ab";

/// The modelled `UserStatus` of a created user, from the enum
/// `smithy_query(query="shape",shape="UserStatus")` reports (`ENABLED | DISABLED`).
const USER_STATUS_ENABLED: &str = "ENABLED";

/// Create a user in `STORE` as `akid` and return its `UserId`.
async fn create_user(
    router: &MultiServiceRouter,
    akid: &str,
    user_name: &str,
    case: &str,
) -> String {
    let created = ok_as(
        router,
        akid,
        "CreateUser",
        json!({ "IdentityStoreId": STORE, "UserName": user_name }),
        case,
    )
    .await;
    string(&created, "UserId", case)
}

/// Create a group in `STORE` as `akid` and return its `GroupId`.
async fn create_group(
    router: &MultiServiceRouter,
    akid: &str,
    display_name: &str,
    case: &str,
) -> String {
    let created = ok_as(
        router,
        akid,
        "CreateGroup",
        json!({ "IdentityStoreId": STORE, "DisplayName": display_name }),
        case,
    )
    .await;
    string(&created, "GroupId", case)
}

/// `DescribeUser` for `user_id`.
async fn describe_user(router: &MultiServiceRouter, user_id: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeUser",
        json!({ "IdentityStoreId": STORE, "UserId": user_id }),
        case,
    )
    .await
}

/// `DescribeGroup` for `group_id`.
async fn describe_group(router: &MultiServiceRouter, group_id: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeGroup",
        json!({ "IdentityStoreId": STORE, "GroupId": group_id }),
        case,
    )
    .await
}

/// What a row's `IdentityStoreId` addresses.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Addressed {
    /// The store this suite's seed resources live in, in one of the forms that name it.
    SeedStore,
    /// Another store of THIS account: one this emulator serves, holding nothing until
    /// something is written to it.
    OtherStoreOfThisAccount,
    /// A store owned by ANOTHER account — not a store this emulator serves at all.
    AnotherAccountsStore,
}

/// Rule 1: every operation resolves the `IdentityStoreId` it is given as the store the
/// CALLER names — any value the modelled pattern admits names a store this emulator
/// serves, holding whatever has been written to it — and only an identifier that names a
/// store of another account is refused, with the modelled
/// `ResourceNotFoundException` naming `IDENTITY_STORE`.
///
/// Rows span the forms the modelled pattern admits (`smithy_query(query="shape",
/// shape="IdentityStoreId")`: `d-` + 10 hex or a UUID, each optionally written as
/// `arn:aws…:identitystore::<account>:identitystore/<id>`) across the three things an
/// identifier can address ([`Addressed`]). The seeded store is two of those rows, not a
/// privileged case.
#[tokio::test]
async fn every_well_formed_identity_store_id_is_served_and_only_another_accounts_is_not() {
    let router = emulator();
    // A user and a group in the seeded store, so a row addressed elsewhere is answered
    // for the right reason: the STORE when it is another account's, and the RESOURCE
    // when the store is served but does not hold it.
    let user = create_user(&router, "AKIDGATE", "gate-user", "seed").await;
    let group = create_group(&router, "AKIDGATE", "gate-group", "seed").await;
    ok(
        &router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": group.clone(),
            "MemberId": { "UserId": user.clone() },
        }),
        "seed",
    )
    .await;

    let other_uuid = "11111111-2222-3333-4444-555555555555";
    // (case, the IdentityStoreId as written, the id the store reports for it, what it
    // addresses)
    let rows: [(&str, String, &str, Addressed); 8] = [
        (
            "the seeded store",
            STORE.to_string(),
            STORE,
            Addressed::SeedStore,
        ),
        (
            "the seeded store by ARN",
            format!("arn:aws:identitystore::{}:identitystore/{STORE}", account_id()),
            STORE,
            Addressed::SeedStore,
        ),
        (
            // The id the recording this issue was filed from uses: a store the client
            // read out of its own environment, which is not the seeded value.
            "a recorded client's own store",
            "d-a2ede64c82".to_string(),
            "d-a2ede64c82",
            Addressed::OtherStoreOfThisAccount,
        ),
        (
            "another d- id nothing has written to",
            "d-00000000ff".to_string(),
            "d-00000000ff",
            Addressed::OtherStoreOfThisAccount,
        ),
        (
            "an id in UUID form",
            other_uuid.to_string(),
            other_uuid,
            Addressed::OtherStoreOfThisAccount,
        ),
        (
            "an id in UUID form, by ARN",
            format!(
                "arn:aws:identitystore::{}:identitystore/{other_uuid}",
                account_id()
            ),
            other_uuid,
            Addressed::OtherStoreOfThisAccount,
        ),
        (
            // The modelled pattern admits any 12-digit account, and an ARN carries the
            // store's OWNER: this names the SEEDED id under another account, which is a
            // different store and not one this emulator serves.
            "the seeded id under another account",
            format!("arn:aws:identitystore::210987654321:identitystore/{STORE}"),
            STORE,
            Addressed::AnotherAccountsStore,
        ),
        (
            "another account's own store",
            MISSING_STORE.to_string(),
            "d-9999999999",
            Addressed::AnotherAccountsStore,
        ),
    ];

    for (case, identity_store_id, reported_id, addressed) in rows {
        // The modelled `UserName` / `DisplayName` patterns admit no whitespace, so a
        // row's name is its case with the spaces hyphenated.
        let slug = case.replace(' ', "-");
        // Every operation that takes an `IdentityStoreId` and is implemented: the three
        // creates, the two describes, the two updates, the delete and the list.
        let requests: [(&str, Value); 9] = [
            (
                "CreateGroup",
                json!({ "IdentityStoreId": identity_store_id, "DisplayName": format!("g-{slug}") }),
            ),
            (
                "CreateUser",
                json!({ "IdentityStoreId": identity_store_id, "UserName": format!("u-{slug}") }),
            ),
            (
                "CreateGroupMembership",
                json!({
                    "IdentityStoreId": identity_store_id,
                    "GroupId": group.clone(),
                    "MemberId": { "UserId": user.clone() },
                }),
            ),
            (
                "DescribeUser",
                json!({ "IdentityStoreId": identity_store_id, "UserId": user.clone() }),
            ),
            (
                "DescribeGroup",
                json!({ "IdentityStoreId": identity_store_id, "GroupId": group.clone() }),
            ),
            (
                "UpdateUser",
                json!({
                    "IdentityStoreId": identity_store_id,
                    "UserId": user.clone(),
                    "Operations": [{ "AttributePath": "title", "AttributeValue": "Builder" }],
                }),
            ),
            (
                "UpdateGroup",
                json!({
                    "IdentityStoreId": identity_store_id,
                    "GroupId": group.clone(),
                    "Operations": [{ "AttributePath": "description", "AttributeValue": "d" }],
                }),
            ),
            (
                "DeleteGroupMembership",
                json!({ "IdentityStoreId": identity_store_id, "MembershipId": MISSING_RESOURCE }),
            ),
            (
                "ListGroupMemberships",
                json!({ "IdentityStoreId": identity_store_id, "GroupId": group.clone() }),
            ),
        ];

        for (operation, request) in requests {
            let case = format!("{case} / {operation}");
            if addressed == Addressed::AnotherAccountsStore {
                let body = refused(
                    &router,
                    operation,
                    request,
                    NOT_FOUND,
                    "ResourceNotFoundException",
                    &case,
                )
                .await;
                assert_eq!(
                    body["ResourceType"].as_str().unwrap_or_default(),
                    "IDENTITY_STORE",
                    "case {case:?}: expected the identity store to be the missing \
                     resource, got {body}"
                );
                continue;
            }

            // The store is served, so the call is answered on its own merits: never with
            // the store as the missing resource.
            let (status, code, body) = call(&router, operation, request).await;
            assert!(
                body["ResourceType"].as_str().unwrap_or_default() != "IDENTITY_STORE",
                "case {case:?}: expected the store to resolve, got {status} {code} \
                 with {body}"
            );
            let accepted: &[u16] = match (operation, addressed) {
                // A create is served by every store this emulator serves, and a
                // `DisplayName`/`UserName` claim is scoped to one store, so no row
                // collides with another.
                ("CreateGroup" | "CreateUser", _) => &[200],
                // The seed store holds the resources the other requests name: the
                // membership create repeats a pair it already holds (409) and the delete
                // names a membership nothing created (the not-found).
                ("DeleteGroupMembership", Addressed::SeedStore) => &[NOT_FOUND],
                ("CreateGroupMembership", Addressed::SeedStore) => &[200, 409],
                (_, Addressed::SeedStore) => &[200],
                // Any other store is served and holds none of those resources, so each
                // request is answered by what IT names being absent.
                _ => &[NOT_FOUND],
            };
            assert!(
                accepted.contains(&status),
                "case {case:?}: expected one of {accepted:?} from a store that is served, \
                 got {status} {code} with {body}"
            );
        }

        if addressed == Addressed::AnotherAccountsStore {
            // Nothing was written: the refused creates left no store behind, so a repeat
            // is refused exactly as the first attempt was.
            let case = format!("{case} / nothing stored");
            refused(
                &router,
                "CreateGroup",
                json!({ "IdentityStoreId": identity_store_id, "DisplayName": format!("g-{slug}") }),
                NOT_FOUND,
                "ResourceNotFoundException",
                &case,
            )
            .await;
            continue;
        }

        // What the creates above stored is readable back through the store the request
        // named, which reports that store's own id — the bare id, for either form that
        // names it — and the group is listed by it and by nothing else.
        let case = format!("{case} / round trip");
        let listed = ok(
            &router,
            "ListGroups",
            json!({ "IdentityStoreId": identity_store_id }),
            &case,
        )
        .await;
        let names: Vec<String> = listed["Groups"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|group| group["DisplayName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            names.contains(&format!("g-{slug}")),
            "case {case:?}: expected the group created in that store to be listed, got {names:?}"
        );
        for group in listed["Groups"].as_array().unwrap_or(&Vec::new()) {
            assert_eq!(
                group["IdentityStoreId"].as_str().unwrap_or_default(),
                reported_id,
                "case {case:?}: expected every listed group to report store \
                 {reported_id:?}, got {group}"
            );
        }
    }

    // The stores are ISOLATED: the seeded store lists only the group created in it, so a
    // create addressed to another store never landed there.
    let case = "stores are isolated";
    let seeded = ok(
        &router,
        "ListGroups",
        json!({ "IdentityStoreId": STORE }),
        case,
    )
    .await;
    let seeded_names: Vec<String> = seeded["Groups"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|group| group["DisplayName"].as_str().unwrap_or_default().to_string())
        .collect();
    for leaked in [
        "g-a-recorded-client's-own-store",
        "g-another-d--id-nothing-has-written-to",
        "g-an-id-in-UUID-form",
    ] {
        assert!(
            !seeded_names.contains(&leaked.to_string()),
            "case {case:?}: expected {leaked:?} to stay in its own store, got {seeded_names:?}"
        );
    }
}

/// Rules 2 and 3: a created user reports `UserStatus` `ENABLED`, and every resource
/// created through the API records the principal that created it — a different one per
/// caller — while `UpdatedBy` names the principal of the LAST write, which is that same
/// create until something updates the resource.
///
/// Rows differ along which caller creates the resource and how much of a profile the
/// create carried.
#[tokio::test]
async fn a_created_resource_reports_its_status_and_the_principal_that_created_it() {
    let router = emulator();
    // (case, access key id, the CreateUser body beyond IdentityStoreId)
    let rows: [(&str, &str, Value); 4] = [
        ("a bare create", "AKIDFIRST", json!({})),
        (
            "a full profile",
            "AKIDFIRST",
            json!({
                "DisplayName": "Ada Lovelace",
                "Name": { "GivenName": "Ada", "FamilyName": "Lovelace" },
                "Emails": [{ "Value": "ada@example.com", "Type": "work", "Primary": true }],
            }),
        ),
        (
            "another caller",
            "AKIDSECOND",
            json!({ "DisplayName": "Grace Hopper" }),
        ),
        (
            "a third caller with no profile",
            "AKIDTHIRD",
            json!({ "Title": "Builder" }),
        ),
    ];

    let mut by_caller: Vec<(String, String)> = Vec::new();
    for (index, (case, akid, profile)) in rows.iter().enumerate() {
        let mut request = profile.clone();
        request["IdentityStoreId"] = json!(STORE);
        request["UserName"] = json!(format!("user-{index}"));
        let created = ok_as(&router, akid, "CreateUser", request, case).await;
        let user_id = string(&created, "UserId", case);

        let described = describe_user(&router, &user_id, case).await;
        assert_eq!(
            described["UserStatus"].as_str().unwrap_or_default(),
            USER_STATUS_ENABLED,
            "case {case:?}: expected UserStatus {USER_STATUS_ENABLED}, got {described}"
        );
        let created_by = described["CreatedBy"].as_str().unwrap_or_default().to_string();
        assert!(
            !created_by.is_empty(),
            "case {case:?}: expected CreatedBy populated on a fresh user, got {described}"
        );
        assert!(
            created_by.contains(akid),
            "case {case:?}: expected CreatedBy to name the calling principal {akid}, \
             got {created_by:?}"
        );
        assert_eq!(
            described.get("UpdatedBy").and_then(Value::as_str),
            Some(created_by.as_str()),
            "case {case:?}: expected UpdatedBy to report the create as the last write of \
             the user, got {described}"
        );
        by_caller.push(((*akid).to_string(), created_by));

        // A group created by the same caller records the same principal, so the rule is
        // the resource's, not one operation's.
        let group_id = create_group(&router, akid, &format!("group-{index}"), case).await;
        let group = describe_group(&router, &group_id, case).await;
        assert_eq!(
            group["CreatedBy"].as_str().unwrap_or_default(),
            by_caller[index].1,
            "case {case:?}: expected the group to record the same creating principal, \
             got {group}"
        );

        // So does a membership, which `ListGroupMemberships` reports.
        let created_membership = ok_as(
            &router,
            akid,
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group_id.clone(),
                "MemberId": { "UserId": user_id.clone() },
            }),
            case,
        )
        .await;
        // Both readers of one record report the principal it holds: a describe and a
        // listing of the same membership cannot disagree about who created it.
        let membership_id = string(&created_membership, "MembershipId", case);
        let described_membership = ok(
            &router,
            "DescribeGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": membership_id }),
            case,
        )
        .await;
        assert_eq!(
            described_membership["CreatedBy"]
                .as_str()
                .unwrap_or_default(),
            by_caller[index].1,
            "case {case:?}: expected DescribeGroupMembership to record the creating \
             principal, got {described_membership}"
        );
        let listed = ok(
            &router,
            "ListGroupMemberships",
            json!({ "IdentityStoreId": STORE, "GroupId": group_id }),
            case,
        )
        .await;
        let membership = &listed["GroupMemberships"][0];
        assert_eq!(
            membership["CreatedBy"].as_str().unwrap_or_default(),
            by_caller[index].1,
            "case {case:?}: expected the membership to record the creating principal, \
             got {listed}"
        );
    }

    // Two different callers are two different principals, and one caller is always the
    // same principal.
    for (akid, principal) in &by_caller {
        for (other_akid, other_principal) in &by_caller {
            if akid == other_akid {
                assert_eq!(
                    principal, other_principal,
                    "expected one caller ({akid}) to be recorded as one principal"
                );
            } else {
                assert_ne!(
                    principal, other_principal,
                    "expected callers {akid} and {other_akid} to be different principals"
                );
            }
        }
    }
}

/// Rule 3, the update half: an update records the UPDATING principal as `UpdatedBy`,
/// leaves `CreatedBy` alone and bumps the revision.
#[tokio::test]
async fn an_update_records_the_principal_that_made_it() {
    let router = emulator();
    let case = "a user updated by another caller";
    let user = create_user(&router, "AKIDCREATOR", "audited", case).await;
    let before = describe_user(&router, &user, case).await;
    let created_by = string(&before, "CreatedBy", case);

    let updated = ok_as(
        &router,
        "AKIDUPDATER",
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user.clone(),
            "Operations": [{ "AttributePath": "title", "AttributeValue": "Builder" }],
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&updated, "Revision", case),
        "2",
        "case {case:?}: expected the update to bump the revision, got {updated}"
    );

    let after = describe_user(&router, &user, case).await;
    assert_eq!(
        after["CreatedBy"].as_str().unwrap_or_default(),
        created_by,
        "case {case:?}: expected CreatedBy to survive an update, got {after}"
    );
    let updated_by = after["UpdatedBy"].as_str().unwrap_or_default();
    assert!(
        updated_by.contains("AKIDUPDATER"),
        "case {case:?}: expected UpdatedBy to name the updating principal, got {after}"
    );
    assert!(
        after["UpdatedAt"].as_f64() >= after["CreatedAt"].as_f64(),
        "case {case:?}: expected UpdatedAt at or after CreatedAt, got {after}"
    );

    // A group behaves the same way.
    let case = "a group updated by another caller";
    let group = create_group(&router, "AKIDCREATOR", "audited-group", case).await;
    let created_by = string(&describe_group(&router, &group, case).await, "CreatedBy", case);
    ok_as(
        &router,
        "AKIDUPDATER",
        "UpdateGroup",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": group.clone(),
            "Operations": [
                { "AttributePath": "description", "AttributeValue": "the audited group" },
            ],
        }),
        case,
    )
    .await;
    let after = describe_group(&router, &group, case).await;
    assert_eq!(
        after["CreatedBy"].as_str().unwrap_or_default(),
        created_by,
        "case {case:?}: expected CreatedBy to survive an update, got {after}"
    );
    assert!(
        after["UpdatedBy"]
            .as_str()
            .unwrap_or_default()
            .contains("AKIDUPDATER"),
        "case {case:?}: expected UpdatedBy to name the updating principal, got {after}"
    );
    assert_eq!(
        after["Description"].as_str().unwrap_or_default(),
        "the audited group",
        "case {case:?}: expected the described group to carry the updated description, got {after}"
    );
}

/// Rule 4: `UpdateUser` refuses an attribute path it cannot change, and the user is
/// left exactly as it was.
///
/// Rows are the kinds of path a client can name that an update does not own: the
/// resource's identifiers (`userId` — the issue's case), its ARN, the store it lives
/// in, its revision, the attributes an identity provider owns, its audit members, a
/// sub-path into a multi-valued attribute (which AWS resolves with JMESPath and this
/// emulator does not evaluate), and an attribute a user simply does not have.
#[tokio::test]
async fn an_update_refuses_an_attribute_path_it_cannot_change() {
    let router = emulator();
    let case = "seed";
    let user = create_user(&router, "AKIDIMMUTABLE", "immutable", case).await;
    let other = create_user(&router, "AKIDIMMUTABLE", "other", case).await;
    let before = describe_user(&router, &user, case).await;

    let rows: [(&str, Value); 10] = [
        ("userId", json!(other.clone())),
        ("UserId", json!(other.clone())),
        ("userArn", json!(format!("arn:aws:identitystore:::user/{other}"))),
        ("identityStoreId", json!("d-9999999999")),
        ("revision", json!("7")),
        ("externalIds", json!([{ "Issuer": "idp", "Id": "x" }])),
        ("createdAt", json!("2026-01-01T00:00:00Z")),
        ("createdBy", json!("someone")),
        ("emails.value", json!("ada@example.com")),
        ("favouriteColour", json!("blue")),
    ];

    for (path, value) in rows {
        let case = format!("UpdateUser {path}");
        let body = refused(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user.clone(),
                "Operations": [{ "AttributePath": path, "AttributeValue": value }],
            }),
            400,
            "ValidationException",
            &case,
        )
        .await;
        assert_eq!(
            error_message(&body),
            format!("Updates for AttributePath: {path} is not supported"),
            "case {case:?}: expected the refusal to name the requested path, got {body}"
        );
        let after = describe_user(&router, &user, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to change nothing"
        );
    }

    // A request whose FIRST operation is applicable and whose second is not changes
    // nothing either: the whole request is refused.
    let case = "a mixed request";
    refused(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user.clone(),
            "Operations": [
                { "AttributePath": "title", "AttributeValue": "Builder" },
                { "AttributePath": "userId", "AttributeValue": other },
            ],
        }),
        400,
        "ValidationException",
        case,
    )
    .await;
    assert_eq!(
        describe_user(&router, &user, case).await,
        before,
        "case {case:?}: expected a request with one bad operation to apply none of them"
    );

    // The group side of the same rule.
    let group = create_group(&router, "AKIDIMMUTABLE", "immutable-group", "seed").await;
    let before = describe_group(&router, &group, "seed").await;
    for path in [
        "groupId",
        "groupArn",
        "identityStoreId",
        "revision",
        "externalIds",
        "createdBy",
        "members",
    ] {
        let case = format!("UpdateGroup {path}");
        let body = refused(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "Operations": [{ "AttributePath": path, "AttributeValue": "x" }],
            }),
            400,
            "ValidationException",
            &case,
        )
        .await;
        assert_eq!(
            error_message(&body),
            format!("Updates for AttributePath: {path} is not supported"),
            "case {case:?}: expected the refusal to name the requested path, got {body}"
        );
        assert_eq!(
            describe_group(&router, &group, &case).await,
            before,
            "case {case:?}: expected the refused update to change nothing"
        );
    }
}

/// The other half of rule 4: the paths an update DOES own are applied, and a later
/// describe reports exactly what was applied.
///
/// Rows differ along the kind of attribute the path names — a plain string, a
/// structure, a sub-attribute of that structure, each multi-valued attribute, and a
/// removal (a `null` `AttributeValue`).
#[tokio::test]
async fn an_update_applies_the_attribute_paths_it_owns() {
    let router = emulator();
    // (case, attribute path, attribute value, the member DescribeUser must report it
    // as, the value expected back)
    let rows: [(&str, &str, Value, &str, Value); 10] = [
        (
            "a plain string",
            "displayName",
            json!("Ada Lovelace"),
            "DisplayName",
            json!("Ada Lovelace"),
        ),
        (
            "a differently cased path",
            "DisplayName",
            json!("Ada L."),
            "DisplayName",
            json!("Ada L."),
        ),
        (
            "a structure",
            "name",
            json!({ "GivenName": "Ada", "FamilyName": "Lovelace" }),
            "Name",
            json!({ "GivenName": "Ada", "FamilyName": "Lovelace" }),
        ),
        (
            "a sub-attribute of a structure",
            "name.givenName",
            json!("Augusta"),
            "Name",
            json!({ "GivenName": "Augusta" }),
        ),
        (
            "a single-entry list",
            "emails",
            json!([{ "Value": "ada@example.com", "Type": "work", "Primary": true }]),
            "Emails",
            json!([{ "Value": "ada@example.com", "Type": "work", "Primary": true }]),
        ),
        (
            "a one-entry list of phone numbers",
            "phoneNumbers",
            json!([{ "Value": "+1 (800) 123-4567", "Type": "mobile" }]),
            "PhoneNumbers",
            json!([{ "Value": "+1 (800) 123-4567", "Type": "mobile", "Primary": false }]),
        ),
        (
            "a list at its modelled maximum",
            "photos",
            json!([
                { "Value": "https://example.com/1.png", "Primary": true },
                { "Value": "https://example.com/2.png" },
                { "Value": "https://example.com/3.png" },
            ]),
            "Photos",
            json!([
                { "Value": "https://example.com/1.png", "Primary": true },
                { "Value": "https://example.com/2.png", "Primary": false },
                { "Value": "https://example.com/3.png", "Primary": false },
            ]),
        ),
        (
            "an address",
            "addresses",
            json!([{ "StreetAddress": "1 Main St", "Locality": "Springfield", "Country": "US" }]),
            "Addresses",
            json!([{
                "StreetAddress": "1 Main St",
                "Locality": "Springfield",
                "Country": "US",
                "Primary": false,
            }]),
        ),
        (
            "a string at the modelled upper length bound",
            "title",
            json!("t".repeat(1024)),
            "Title",
            json!("t".repeat(1024)),
        ),
        (
            "the shortest string the model admits",
            "nickName",
            json!("a"),
            "NickName",
            json!("a"),
        ),
    ];

    for (index, (case, path, value, member, expected)) in rows.iter().enumerate() {
        let user = create_user(&router, "AKIDAPPLY", &format!("applied-{index}"), case).await;
        let updated = ok(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user.clone(),
                "Operations": [{ "AttributePath": path, "AttributeValue": value }],
            }),
            case,
        )
        .await;
        assert_eq!(
            string(&updated, "Revision", case),
            "2",
            "case {case:?}: expected the applied update to bump the revision, got {updated}"
        );
        let described = describe_user(&router, &user, case).await;
        assert_eq!(
            described.get(*member).unwrap_or(&Value::Null),
            expected,
            "case {case:?}: expected DescribeUser to report {member} as {expected}, \
             got {described}"
        );

        // And the removal: the same path with no value leaves the attribute absent.
        let case = format!("{case} removed");
        ok(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user.clone(),
                "Operations": [{ "AttributePath": path, "AttributeValue": Value::Null }],
            }),
            &case,
        )
        .await;
        let described = describe_user(&router, &user, &case).await;
        assert!(
            described.get(*member).is_none_or(Value::is_null),
            "case {case:?}: expected {member} to be gone after a removal, got {described}"
        );
    }

    // `UserStatus` is a modelled ENUM, and every stored user has one, so each of its
    // values is applied and reported — but a removal names no status a user can be in
    // and is refused.
    let user = create_user(&router, "AKIDAPPLY", "statused", "user status").await;
    for status in ["DISABLED", "ENABLED"] {
        let case = format!("user status {status}");
        ok(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user.clone(),
                "Operations": [{ "AttributePath": "userStatus", "AttributeValue": status }],
            }),
            &case,
        )
        .await;
        assert_eq!(
            describe_user(&router, &user, &case).await["UserStatus"]
                .as_str()
                .unwrap_or_default(),
            status,
            "case {case:?}: expected the applied status to be reported"
        );
    }
    refused(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user.clone(),
            "Operations": [{ "AttributePath": "userStatus", "AttributeValue": Value::Null }],
        }),
        400,
        "ValidationException",
        "a removed user status",
    )
    .await;
    assert_eq!(
        describe_user(&router, &user, "a removed user status").await["UserStatus"]
            .as_str()
            .unwrap_or_default(),
        "ENABLED",
        "case \"a removed user status\": expected the stored status to survive the refusal"
    );

    // A value of the wrong kind for the path is refused rather than stored.
    let user = create_user(&router, "AKIDAPPLY", "typed", "typed values").await;
    let bad_rows: [(&str, &str, Value); 6] = [
        ("a number where a string belongs", "title", json!(7)),
        ("an object where a string belongs", "locale", json!({ "a": "b" })),
        ("a string where an object belongs", "name", json!("Ada")),
        (
            // The model types a multi-valued attribute as a LIST, so a lone entry still
            // needs its array.
            "a list entry sent unwrapped",
            "phoneNumbers",
            json!({ "Value": "+1 (800) 123-4567" }),
        ),
        (
            "a list past its modelled maximum",
            "emails",
            json!([{ "Value": "a@example.com" }, { "Value": "b@example.com" }]),
        ),
        (
            // `UserStatus` is a modelled enum, so only its own values are a status.
            "a value outside the modelled enum",
            "userStatus",
            json!("PENDING"),
        ),
    ];
    for (case, path, value) in bad_rows {
        refused(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user.clone(),
                "Operations": [{ "AttributePath": path, "AttributeValue": value }],
            }),
            400,
            "ValidationException",
            case,
        )
        .await;
    }

    // `UpdateGroup` owns exactly `displayName` and `description`.
    let group = create_group(&router, "AKIDAPPLY", "applied-group", "group paths").await;
    for (case, path, value, member) in [
        ("a renamed group", "displayName", "renamed-group", "DisplayName"),
        ("a described group", "description", "the applied group", "Description"),
    ] {
        ok(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "Operations": [{ "AttributePath": path, "AttributeValue": value }],
            }),
            case,
        )
        .await;
        let described = describe_group(&router, &group, case).await;
        assert_eq!(
            described[member].as_str().unwrap_or_default(),
            value,
            "case {case:?}: expected DescribeGroup to report {member} as {value:?}, \
             got {described}"
        );
    }
}

/// An update is conditional on `Revision` when the caller sends one, and a rename
/// still answers to the uniqueness claim the name carries.
#[tokio::test]
async fn an_update_honours_the_revision_precondition_and_the_uniqueness_claim() {
    let router = emulator();
    let case = "a stale revision";
    let user = create_user(&router, "AKIDREV", "revisioned", case).await;
    let before = describe_user(&router, &user, case).await;

    // Rows: the stored revision (applied), and revisions on either side of it (refused).
    for (case, revision, accepted) in [
        ("a revision behind", "0", false),
        ("a revision ahead", "2", false),
        ("the stored revision", "1", true),
    ] {
        let request = json!({
            "IdentityStoreId": STORE,
            "UserId": user.clone(),
            "Revision": revision,
            "Operations": [{ "AttributePath": "title", "AttributeValue": "Builder" }],
        });
        if accepted {
            let updated = ok(&router, "UpdateUser", request, case).await;
            assert_eq!(
                string(&updated, "Revision", case),
                "2",
                "case {case:?}: expected the conditional update to apply, got {updated}"
            );
        } else {
            refused(
                &router,
                "UpdateUser",
                request,
                409,
                "ConflictException",
                case,
            )
            .await;
            assert_eq!(
                describe_user(&router, &user, case).await,
                before,
                "case {case:?}: expected a refused conditional update to change nothing"
            );
        }
    }

    // A rename onto a name another user holds conflicts; renaming to a free name
    // releases the old claim, so the old name can then be claimed by someone else.
    let case = "a rename";
    let taken = create_user(&router, "AKIDREV", "taken", case).await;
    refused(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user.clone(),
            "Operations": [{ "AttributePath": "userName", "AttributeValue": "taken" }],
        }),
        409,
        "ConflictException",
        case,
    )
    .await;
    assert_eq!(
        describe_user(&router, &user, case).await["UserName"]
            .as_str()
            .unwrap_or_default(),
        "revisioned",
        "case {case:?}: expected the refused rename to leave the name alone"
    );
    ok(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user.clone(),
            "Operations": [{ "AttributePath": "userName", "AttributeValue": "renamed" }],
        }),
        case,
    )
    .await;
    assert_eq!(
        describe_user(&router, &user, case).await["UserName"]
            .as_str()
            .unwrap_or_default(),
        "renamed",
        "case {case:?}: expected the applied rename to be reported"
    );
    // The released name is free again.
    let reclaimed = create_user(&router, "AKIDREV", "revisioned", case).await;
    assert_ne!(
        reclaimed, taken,
        "case {case:?}: expected a distinct user to claim the released name"
    );

    // An update of a user that does not exist is not found, whatever it asks for.
    refused_as(
        &router,
        "AKIDREV",
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": MISSING_RESOURCE,
            "Operations": [{ "AttributePath": "userId", "AttributeValue": user }],
        }),
        NOT_FOUND,
        "ResourceNotFoundException",
        "an unknown user",
    )
    .await;
}
