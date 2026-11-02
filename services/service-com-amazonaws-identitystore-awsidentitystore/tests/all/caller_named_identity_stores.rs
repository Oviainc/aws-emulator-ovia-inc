// SPDX-License-Identifier: CC0-1.0

//! The identity store a request names is the store its CALLER has, driven end to end
//! over HTTP through [`crate::harness`].
//!
//! The rule under test: this API models no operation that creates an identity store — in
//! AWS one comes with an IAM Identity Center instance, and a client reads its id out of
//! its own environment — so every operation serves whatever `IdentityStoreId` the
//! modelled constraints admit, each store holding exactly what has been written to it,
//! and the modelled `ResourceNotFoundException` naming `IDENTITY_STORE` is reserved
//! for an identifier that names a store this emulator cannot serve at all (an ARN
//! carrying another account). A value the modelled `@length`/`@pattern` refuse is a
//! `ValidationException`, never a not-found.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the FORM
//! of the identifier (`d-` + 10 hex or a UUID, bare or as the store's ARN, in either
//! partition spelling), its LENGTH (both published bounds of the modelled 1..93 and one
//! past the top), and WHICH store it names (the seeded one, a caller's own, another
//! account's). The recorded ids the issue cites are three rows among them.

use serde_json::{json, Value};
use service_com_amazonaws_identitystore_awsidentitystore as identitystore;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};

/// An account id that is NOT this emulator's, for the ARN form that names another
/// account's store (the modelled pattern admits any 12-digit account).
const OTHER_ACCOUNT: &str = "210987654321";

/// A `UUID`-form `IdentityStoreId`, the second form the modelled pattern admits
/// (`smithy_query(query="shape",shape="IdentityStoreId")`).
const UUID_FORM: &str = "a1b2c3d4-5678-90ab-cdef-111122223333";

/// The published bounds of the modelled `IdentityStoreId` length, from
/// `smithy_query(query="shape",shape="IdentityStoreId")` (length 1..93). The pattern
/// admits nothing shorter than `d-` + 10 hexadecimal digits, which is the shortest value
/// that can reach a handler at all; the longest is the ARN form at its maximum.
const ID_MIN_REACHABLE_LEN: usize = 12;
const ID_MAX_LEN: usize = 93;

/// The emulator's own account id, which the ARN form of a store it serves carries.
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// The store ARN form of `id` under `account`, in the `partition` spelling the modelled
/// pattern's `aws[a-z-]*` admits.
fn store_arn(partition: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:identitystore::{account}:identitystore/{id}")
}

/// `CreateGroup` in the store `identity_store_id` names.
async fn create_group(
    router: &MultiServiceRouter,
    identity_store_id: &str,
    display_name: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "CreateGroup",
        json!({ "IdentityStoreId": identity_store_id, "DisplayName": display_name }),
        case,
    )
    .await
}

/// `ListGroups` in the store `identity_store_id` names, as the whole response body.
async fn list_groups(router: &MultiServiceRouter, identity_store_id: &str, case: &str) -> Value {
    ok(
        router,
        "ListGroups",
        json!({ "IdentityStoreId": identity_store_id }),
        case,
    )
    .await
}

/// Every store id form that names a store this emulator SERVES, paired with the id that
/// store reports for it (the bare id, for either form that names it).
fn served_rows() -> Vec<(&'static str, String, String)> {
    let longest_uuid_arn = store_arn("aws-us-gov", &account_id(), UUID_FORM);
    vec![
        // The three ids the recorded fixtures this issue was filed from use: each is a
        // store a client read out of its own environment, and none is the seeded value.
        (
            "a recorded client's store",
            "d-a2ede64c82".to_string(),
            "d-a2ede64c82".to_string(),
        ),
        (
            "a second recorded client's store",
            "d-e00797f110".to_string(),
            "d-e00797f110".to_string(),
        ),
        (
            "a third recorded client's store",
            "d-486a49b0e4".to_string(),
            "d-486a49b0e4".to_string(),
        ),
        // The store another service advertises, which is one row and not a privileged
        // case.
        (
            "the seeded store",
            STORE.to_string(),
            STORE.to_string(),
        ),
        (
            "the seeded store by ARN",
            store_arn("aws", &account_id(), STORE),
            STORE.to_string(),
        ),
        (
            "an id in UUID form",
            UUID_FORM.to_string(),
            UUID_FORM.to_string(),
        ),
        (
            "an id in UUID form, by ARN",
            store_arn("aws", &account_id(), UUID_FORM),
            UUID_FORM.to_string(),
        ),
        (
            // The partition is a wildcard in the modelled pattern, so a store ARN in
            // another partition names the same store.
            "a d- id by ARN in another partition",
            store_arn("aws-cn", &account_id(), "d-0f1e2d3c4b"),
            "d-0f1e2d3c4b".to_string(),
        ),
        (
            // The shortest value the modelled pattern admits: `d-` + 10 hex digits.
            "the shortest id the pattern admits",
            "d-0000000000".to_string(),
            "d-0000000000".to_string(),
        ),
        (
            // The longest the modelled length admits: the ARN form at 93 characters.
            "an id at the modelled maximum length",
            longest_uuid_arn,
            UUID_FORM.to_string(),
        ),
    ]
}

/// The rule for a WRITE: a create is served in whatever store the caller names, answers
/// the required members of its output, and what it stored is readable back from that
/// store — and from no other.
#[tokio::test]
async fn a_create_is_served_in_whatever_store_the_caller_names() {
    let router = emulator();
    let rows = served_rows();
    assert_eq!(
        rows[8].1.len(),
        ID_MIN_REACHABLE_LEN,
        "expected the shortest row to be the shortest id the pattern admits, got {:?}",
        rows[8].1
    );
    assert_eq!(
        rows[9].1.len(),
        ID_MAX_LEN,
        "expected the longest row to sit on the modelled maximum, got {:?}",
        rows[9].1
    );

    let mut minted: Vec<(String, String, String)> = Vec::new();
    for (case, identity_store_id, reported_id) in &rows {
        let display_name = format!("group-for-{}", case.replace(' ', "-"));
        let created = create_group(&router, identity_store_id, &display_name, case).await;
        // CreateGroup's output members are all required: the store it was addressed to,
        // the minted id and ARN, and the revision.
        assert_eq!(
            &string(&created, "IdentityStoreId", case),
            reported_id,
            "case {case:?}: expected the store the request named to be echoed, got {created}"
        );
        let group_id = string(&created, "GroupId", case);
        let group_arn = string(&created, "GroupArn", case);
        assert_eq!(
            group_arn,
            format!("arn:aws:identitystore:::group/{group_id}"),
            "case {case:?}: expected the group's ARN to carry its own id, got {created}"
        );
        assert_eq!(
            string(&created, "Revision", case),
            "1",
            "case {case:?}: expected a create to be revision 1, got {created}"
        );

        // Read back through the store the request named: a describe reports what was
        // stored, and the `Description` nothing supplied is ABSENT rather than empty.
        let described = ok(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": identity_store_id, "GroupId": group_id.clone() }),
            case,
        )
        .await;
        assert_eq!(
            string(&described, "DisplayName", case),
            display_name,
            "case {case:?}: expected the stored DisplayName, got {described}"
        );
        assert_eq!(
            &string(&described, "IdentityStoreId", case),
            reported_id,
            "case {case:?}: expected the describe to report that store, got {described}"
        );
        assert!(
            described.get("Description").is_none_or(Value::is_null),
            "case {case:?}: expected no Description on a group created without one, \
             got {described}"
        );
        minted.push((reported_id.clone(), group_id, display_name));
    }

    // Each store lists exactly the group created in it: a create addressed to one store
    // never lands in another, however the two ids are written.
    for (reported_id, group_id, display_name) in &minted {
        let case = format!("{reported_id} lists its own group");
        let listed = list_groups(&router, reported_id, &case).await;
        let groups = listed["Groups"].as_array().cloned().unwrap_or_default();
        let names: Vec<String> = groups
            .iter()
            .map(|group| group["DisplayName"].as_str().unwrap_or_default().to_string())
            .collect();
        // The seeded store is named by two rows, so it holds two groups; every other
        // store holds exactly the one created in it.
        assert!(
            names.contains(display_name),
            "case {case:?}: expected the group created in that store to be listed, \
             got {names:?}"
        );
        for group in &groups {
            assert_eq!(
                group["IdentityStoreId"].as_str().unwrap_or_default(),
                reported_id,
                "case {case:?}: expected every listed group to report its own store, \
                 got {group}"
            );
        }
        // A group minted in one store is not readable through another.
        let elsewhere = minted
            .iter()
            .find(|(other, _, _)| other != reported_id)
            .expect("the table names more than one store");
        let body = refused(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": elsewhere.0.clone(), "GroupId": group_id.clone() }),
            NOT_FOUND,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        assert_eq!(
            body["ResourceType"].as_str().unwrap_or_default(),
            "GROUP",
            "case {case:?}: expected the GROUP to be what another store is missing, \
             got {body}"
        );
    }
}

/// The rule for a unique-attribute READ: `GetUserId` resolves the user of the store the
/// request names, in every form that names it.
#[tokio::test]
async fn a_user_resolves_by_unique_attribute_in_whatever_store_the_caller_names() {
    let router = emulator();
    for (case, identity_store_id, reported_id) in served_rows() {
        let user_name = format!("user-for-{}", case.replace(' ', "-"));
        let created = ok(
            &router,
            "CreateUser",
            json!({ "IdentityStoreId": identity_store_id, "UserName": user_name.clone() }),
            case,
        )
        .await;
        let user_id = string(&created, "UserId", case);
        let user_arn = string(&created, "UserArn", case);
        assert_eq!(
            string(&created, "IdentityStoreId", case),
            reported_id,
            "case {case:?}: expected the store the request named to be echoed, got {created}"
        );

        let resolved = ok(
            &router,
            "GetUserId",
            json!({
                "IdentityStoreId": identity_store_id,
                "AlternateIdentifier": {
                    "UniqueAttribute": {
                        "AttributePath": "userName",
                        "AttributeValue": user_name.clone(),
                    },
                },
            }),
            case,
        )
        .await;
        // GetUserId's output members are all required.
        assert_eq!(
            string(&resolved, "IdentityStoreId", case),
            reported_id,
            "case {case:?}: expected the lookup to report that store, got {resolved}"
        );
        assert_eq!(
            string(&resolved, "UserId", case),
            user_id,
            "case {case:?}: expected the created user's id, got {resolved}"
        );
        assert_eq!(
            string(&resolved, "UserArn", case),
            user_arn,
            "case {case:?}: expected the created user's ARN, got {resolved}"
        );
    }
}

/// The rule for a store nothing has written to: it is served, reports the nothing it
/// holds, and a read of it writes nothing.
#[tokio::test]
async fn a_store_nothing_has_written_to_is_served_and_holds_nothing() {
    let router = emulator();
    // Rows differ along the form of the id and whether anything was ever addressed to it.
    let rows: Vec<(&str, String)> = vec![
        ("a d- id", "d-abcdef0123".to_string()),
        ("a UUID id", UUID_FORM.to_string()),
        (
            "an ARN of this account",
            store_arn("aws", &account_id(), "d-abcdef0124"),
        ),
    ];

    for (case, identity_store_id) in &rows {
        for attempt in 0..2 {
            let case = format!("{case}, read {attempt}");
            let groups = list_groups(&router, identity_store_id, &case).await;
            assert_eq!(
                groups["Groups"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected no group in a store nothing has written to, \
                 got {groups}"
            );
            let users = ok(
                &router,
                "ListUsers",
                json!({ "IdentityStoreId": identity_store_id }),
                &case,
            )
            .await;
            assert_eq!(
                users["Users"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected no user in a store nothing has written to, \
                 got {users}"
            );
            // A resource read of such a store reports the RESOURCE as missing, never the
            // store.
            let body = refused(
                &router,
                "DescribeGroup",
                json!({
                    "IdentityStoreId": identity_store_id,
                    "GroupId": "a1b2c3d4-5678-90ab-cdef-999999999999",
                }),
                NOT_FOUND,
                "ResourceNotFoundException",
                &case,
            )
            .await;
            assert_eq!(
                body["ResourceType"].as_str().unwrap_or_default(),
                "GROUP",
                "case {case:?}: expected the GROUP to be the missing resource, got {body}"
            );
        }

        // The reads wrote nothing: the first create addressed to that store is still the
        // first group it holds.
        let created = create_group(&router, identity_store_id, "after the reads", case).await;
        let listed = list_groups(&router, identity_store_id, case).await;
        let groups = listed["Groups"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            groups.len(),
            1,
            "case {case:?}: expected exactly the one created group, got {listed}"
        );
        assert_eq!(
            groups[0]["GroupId"].as_str().unwrap_or_default(),
            string(&created, "GroupId", case),
            "case {case:?}: expected the listing to report the created group, got {listed}"
        );
    }
}

/// The rule's NEGATIVE side: an identifier that names a store this emulator cannot serve
/// is the modelled not-found, and one the modelled constraints refuse is a
/// `ValidationException` — so a not-found is never the answer to a well-formed id of this
/// account.
#[tokio::test]
async fn only_a_store_outside_this_account_is_not_found_and_a_bad_value_is_a_400() {
    let router = emulator();

    // Another account's store, in every form that names one.
    let not_served: Vec<(&str, String)> = vec![
        ("another account's d- store", MISSING_STORE.to_string()),
        (
            "another account's store in UUID form",
            store_arn("aws", OTHER_ACCOUNT, UUID_FORM),
        ),
        (
            "another account's store in another partition",
            store_arn("aws-cn", OTHER_ACCOUNT, STORE),
        ),
    ];
    for (case, identity_store_id) in &not_served {
        for operation in ["CreateGroup", "CreateUser", "ListGroups", "GetUserId"] {
            let case = format!("{case} / {operation}");
            let request = match operation {
                "CreateGroup" => json!({
                    "IdentityStoreId": identity_store_id,
                    "DisplayName": "never stored",
                }),
                "CreateUser" => json!({
                    "IdentityStoreId": identity_store_id,
                    "UserName": "never-stored",
                }),
                "GetUserId" => json!({
                    "IdentityStoreId": identity_store_id,
                    "AlternateIdentifier": {
                        "UniqueAttribute": {
                            "AttributePath": "userName",
                            "AttributeValue": "never-stored",
                        },
                    },
                }),
                _ => json!({ "IdentityStoreId": identity_store_id }),
            };
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
                "case {case:?}: expected the identity store to be the missing resource, \
                 got {body}"
            );
        }
    }

    // A value the modelled constraints refuse never reaches resolution: it is the 400 the
    // boundary validator answers, one character past the modelled maximum length and for
    // a value outside the pattern.
    let too_long = store_arn("aws-us-govz", &account_id(), UUID_FORM);
    assert_eq!(
        too_long.len(),
        ID_MAX_LEN + 1,
        "expected the over-long row to sit one past the modelled maximum, got {too_long:?}"
    );
    for (case, identity_store_id) in [
        ("one character past the modelled maximum", too_long),
        ("not the modelled pattern", "d-NOTAVALIDST".to_string()),
        ("an id the pattern rejects for length", "d-abc".to_string()),
    ] {
        let (status, code, body) = call(
            &router,
            "ListGroups",
            json!({ "IdentityStoreId": identity_store_id }),
        )
        .await;
        assert_eq!(
            (status, code.ends_with("ValidationException")),
            (400, true),
            "case {case:?}: expected the modelled constraint to refuse it with 400 \
             ValidationException, got {status} {code} with {body}"
        );
    }
}

/// The RECORDS this emulator keeps are bounded: only a create registers one, never past
/// `MAX_IDENTITY_STORES`, and past the bound a create in a store that is already recorded
/// is still served.
#[tokio::test]
async fn the_number_of_recorded_identity_stores_is_bounded() {
    let router = emulator();
    // The seeded store is already one record, so that many MORE stores can be recorded.
    let headroom = identitystore::state::MAX_IDENTITY_STORES - 1;
    for index in 0..headroom {
        let store = format!("d-{index:010x}");
        let case = format!("store {index} of the headroom");
        create_group(&router, &store, &format!("group-{index}"), &case).await;
    }

    // One more NEW store does not fit: the create is refused with the modelled quota
    // error (HTTP 402), and nothing is stored for it.
    let over = format!("d-{headroom:010x}");
    let body = refused(
        &router,
        "CreateGroup",
        json!({ "IdentityStoreId": over.clone(), "DisplayName": "one store too many" }),
        402,
        "ServiceQuotaExceededException",
        "one store past the bound",
    )
    .await;
    assert!(
        body.get("Message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "case \"one store past the bound\": expected the quota refusal to carry a message, \
         got {body}"
    );
    // Rows differ along which operation addresses the store that was refused: none of
    // them sees it hold anything, because the refusal stored nothing.
    for (case, operation, request) in [
        (
            "the refused store lists nothing",
            "ListGroups",
            json!({ "IdentityStoreId": over.clone() }),
        ),
        (
            "and no user either",
            "ListUsers",
            json!({ "IdentityStoreId": over.clone() }),
        ),
    ] {
        let listed = ok(&router, operation, request, case).await;
        let items = listed["Groups"]
            .as_array()
            .or_else(|| listed["Users"].as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            items.is_empty(),
            "case {case:?}: expected the refused create to have stored nothing, got {listed}"
        );
    }

    // The bound is on RECORDS, not on writes: a create in a store that is already
    // recorded is still served at capacity.
    for (case, store) in [
        ("the seeded store", STORE.to_string()),
        ("a store recorded earlier", "d-0000000000".to_string()),
    ] {
        create_group(&router, &store, &format!("after capacity {case}"), case).await;
    }
}

/// A write that stores NO new resource registers no store record either, so it cannot
/// consume the bound: only a create does.
#[tokio::test]
async fn a_write_that_stores_nothing_records_no_identity_store() {
    let router = emulator();
    // Rows are the writes that only ever change what is already there, each addressed to
    // a store nothing has written to, and what each must answer there.
    let probes: Vec<(&str, Value, u16, &str)> = vec![
        (
            "DeleteUser",
            json!({ "UserId": "a1b2c3d4-5678-90ab-cdef-999999999999" }),
            NOT_FOUND,
            "USER",
        ),
        (
            "DeleteGroupMembership",
            json!({ "MembershipId": "a1b2c3d4-5678-90ab-cdef-999999999999" }),
            NOT_FOUND,
            "GROUP_MEMBERSHIP",
        ),
        (
            "UpdateUser",
            json!({
                "UserId": "a1b2c3d4-5678-90ab-cdef-999999999999",
                "Operations": [{ "AttributePath": "displayName", "AttributeValue": "Ada" }],
            }),
            NOT_FOUND,
            "USER",
        ),
        (
            "UpdateGroup",
            json!({
                "GroupId": "a1b2c3d4-5678-90ab-cdef-999999999999",
                "Operations": [{ "AttributePath": "description", "AttributeValue": "d" }],
            }),
            NOT_FOUND,
            "GROUP",
        ),
        (
            "CreateGroupMembership",
            json!({
                "GroupId": "a1b2c3d4-5678-90ab-cdef-999999999999",
                "MemberId": { "UserId": "a1b2c3d4-5678-90ab-cdef-888888888888" },
            }),
            NOT_FOUND,
            "GROUP",
        ),
    ];

    // Each probe is repeated enough times, each time against a store of its own, that a
    // registering write would have exhausted the bound several times over.
    let rounds = identitystore::state::MAX_IDENTITY_STORES;
    for (probe, (operation, body, status, resource_type)) in probes.iter().enumerate() {
        for round in 0..rounds {
            // `d-` + 10 hex digits, unique per (probe, round).
            let store = format!("d-f{probe:01x}0000{round:04x}");
            let case = format!("{operation} in {store}");
            let mut request = body.clone();
            request["IdentityStoreId"] = json!(store);
            let refusal = refused(
                &router,
                operation,
                request,
                *status,
                "ResourceNotFoundException",
                &case,
            )
            .await;
            assert_eq!(
                refusal["ResourceType"].as_str().unwrap_or_default(),
                *resource_type,
                "case {case:?}: expected {resource_type} to be the missing resource in a \
                 store that is served, got {refusal}"
            );
        }
    }

    // `DeleteGroup` is `@idempotent`, so its answer there is the empty success — which
    // also stores nothing.
    for round in 0..rounds {
        let store = format!("d-e00000{round:04x}");
        let case = format!("DeleteGroup in {store}");
        ok(
            &router,
            "DeleteGroup",
            json!({
                "IdentityStoreId": store,
                "GroupId": "a1b2c3d4-5678-90ab-cdef-999999999999",
            }),
            &case,
        )
        .await;
    }

    // None of those writes recorded a store, so the whole bound is still free: a create
    // in each of that many fresh stores is served.
    for index in 0..(identitystore::state::MAX_IDENTITY_STORES - 1) {
        let store = format!("d-{index:010x}");
        let case = format!("create in fresh store {index}");
        create_group(&router, &store, &format!("group-{index}"), &case).await;
    }
}
