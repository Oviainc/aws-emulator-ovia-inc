// SPDX-License-Identifier: CC0-1.0

//! The Wickr network-and-users lifecycle, driven over HTTP against one running
//! emulator.
//!
//! Every case here goes through the real multi-service router — route resolution,
//! restJson1 deserialization, modelled-constraint validation, the handler, and
//! response serialization — so what it asserts is the wire behaviour a client sees,
//! not an internal call.
//!
//! The rules under test, and where each comes from:
//!
//! * `networkName` "must be between 1 and 20 characters"
//!   (`CreateNetworkRequest$networkName` documentation).
//! * `accessLevel` is one of `AccessLevel`'s values (`STANDARD`, `PREMIUM`).
//! * "A user can only be assigned to a single security group"; `username` "must be
//!   unique within the network" (`BatchCreateUserRequestItem` documentation).
//! * `UserStatus` 1 is PENDING, 2 is ACTIVE.
//! * `NetworkId` is `^[0-9]{8}$`, `UserId` is `^[0-9]+$` of 1..10 characters.
//! * `ListNetworks` / `ListUsers` are `@paginated(items=networks|users)`, so a page
//!   plus its `nextToken` must walk every stored item exactly once.
//! * A network, user, bot or security group "that doesn't exist or has been deleted"
//!   answers `ResourceNotFoundError` (HTTP 404).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A fresh emulator holding only this service, over its own engine, so one test's
/// networks are invisible to another's.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is not the subject here; routing and the handlers are.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_wickr_wickradminapi::EmulatedService::new(engine),
    );
    router
}

/// One request/response round trip: the HTTP status and the parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "admin.wickr.us-east-1.amazonaws.com")
        // The SigV4 credential scope is how the router picks the service and how a
        // handler learns the region for the ARNs it mints.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260102/us-east-1/wickr/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, parsed)
}

/// Create a network and return its minted id, failing the test if it was refused.
async fn create_network(router: &MultiServiceRouter, name: &str, access_level: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/networks",
        Some(json!({ "networkName": name, "accessLevel": access_level })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNetwork {name:?}/{access_level}: expected 200, got {status} with {body}"
    );
    body["networkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNetwork {name:?}: no networkId in {body}"))
        .to_string()
}

/// The `__type` discriminator of an error body, or `""` when there is none.
fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The items of a list response member, or an empty slice.
fn items(body: &Value, member: &str) -> Vec<Value> {
    body[member].as_array().cloned().unwrap_or_default()
}

/// Whether `id` satisfies `NetworkId` (`^[0-9]{8}$`).
fn is_network_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `id` satisfies `UserId` (len 1..10, `^[0-9]+$`).
fn is_user_id(id: &str) -> bool {
    (1..=10).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

#[tokio::test]
async fn the_network_and_user_lifecycle_round_trips_through_stored_state() {
    let router = emulator();
    let key_arn = "arn:aws:kms:us-east-1:000000000000:key/9b1c-demo";

    // CREATE -------------------------------------------------------------------
    let (status, created) = call(
        &router,
        "POST",
        "/networks",
        Some(json!({
            "networkName": "demo-net",
            "accessLevel": "STANDARD",
            "encryptionKeyArn": key_arn,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNetwork: expected 200, got {status} with {created}"
    );
    let network_id = created["networkId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_network_id(&network_id),
        "CreateNetwork: networkId must match ^[0-9]{{8}}$, got {network_id:?}"
    );
    assert_eq!(
        created["networkName"],
        json!("demo-net"),
        "CreateNetwork: networkName must come from what was stored, got {created}"
    );
    assert_eq!(
        created["encryptionKeyArn"],
        json!(key_arn),
        "CreateNetwork: encryptionKeyArn must come from what was stored, got {created}"
    );

    // READ BACK ----------------------------------------------------------------
    let (status, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetNetwork: expected 200, got {status} with {fetched}"
    );
    for (member, expected) in [
        ("networkId", json!(network_id)),
        ("networkName", json!("demo-net")),
        ("accessLevel", json!("STANDARD")),
        ("awsAccountId", json!("000000000000")),
        (
            "networkArn",
            json!(format!(
                "arn:aws:wickr:us-east-1:000000000000:network/{network_id}"
            )),
        ),
        ("encryptionKeyArn", json!(key_arn)),
        ("migrationState", json!(0)),
    ] {
        assert_eq!(
            fetched[member], expected,
            "GetNetwork {member}: expected {expected}, got {}",
            fetched[member]
        );
    }
    assert!(
        fetched.get("freeTrialExpiration").is_none(),
        "GetNetwork: a network created without enablePremiumFreeTrial has no trial \
         expiry, got {fetched}"
    );

    // LIST ---------------------------------------------------------------------
    let (status, listed) = call(&router, "GET", "/networks", None).await;
    assert_eq!(
        status, 200,
        "ListNetworks: expected 200, got {status} with {listed}"
    );
    let networks = items(&listed, "networks");
    assert_eq!(
        networks.len(),
        1,
        "ListNetworks: expected the one created network, got {listed}"
    );
    assert_eq!(
        networks[0]["networkId"],
        json!(network_id),
        "ListNetworks: expected the created network, got {listed}"
    );

    // CREATE USERS -------------------------------------------------------------
    let batch_body = json!({
        "users": [{
            "username": "alice@example.com",
            "firstName": "Alice",
            "securityGroupIds": ["0"],
        }],
    });
    let (status, batch) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(batch_body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchCreateUser: expected 200, got {status} with {batch}"
    );
    let successful = items(&batch, "successful");
    assert_eq!(
        successful.len(),
        1,
        "BatchCreateUser: expected one created user, got {batch}"
    );
    assert!(
        items(&batch, "failed").is_empty(),
        "BatchCreateUser: expected no failures, got {batch}"
    );
    let user = &successful[0];
    let user_id = user["userId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_user_id(&user_id),
        "BatchCreateUser: userId must match ^[0-9]+$ of 1..10 characters, got {user_id:?}"
    );
    for (member, expected) in [
        ("username", json!("alice@example.com")),
        ("firstName", json!("Alice")),
        ("securityGroups", json!(["0"])),
        ("isAdmin", json!(false)),
        ("suspended", json!(false)),
        ("status", json!(1)),
    ] {
        assert_eq!(
            user[member], expected,
            "BatchCreateUser {member}: expected {expected}, got {}",
            user[member]
        );
    }
    assert!(
        user["inviteCode"].as_str().is_some_and(|code| !code.is_empty()),
        "BatchCreateUser: an item without an inviteCode must be given one, got {user}"
    );

    // The same batch again creates nothing: the username is taken.
    let (status, repeat) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(batch_body),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchCreateUser repeat: expected 200 partial success, got {status} with {repeat}"
    );
    assert!(
        items(&repeat, "successful").is_empty(),
        "BatchCreateUser repeat: a duplicate username must create nothing, got {repeat}"
    );
    let failed = items(&repeat, "failed");
    assert_eq!(
        failed.len(),
        1,
        "BatchCreateUser repeat: expected one failed item, got {repeat}"
    );
    assert_eq!(
        failed[0]["field"],
        json!("username"),
        "BatchCreateUser repeat: the failure must name username, got {repeat}"
    );
    assert_eq!(
        failed[0]["userId"],
        json!(user_id),
        "BatchCreateUser repeat: the failure must name the user that holds the \
         username, got {repeat}"
    );

    // READ USERS BACK ----------------------------------------------------------
    let (status, users) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListUsers: expected 200, got {status} with {users}"
    );
    let listed_users = items(&users, "users");
    assert_eq!(
        listed_users.len(),
        1,
        "ListUsers: expected exactly the one created user, got {users}"
    );
    assert_eq!(
        listed_users[0]["userId"],
        json!(user_id),
        "ListUsers: expected the created user, got {users}"
    );
    assert_eq!(
        listed_users[0]["firstName"],
        json!("Alice"),
        "ListUsers: firstName must come from what was stored, got {users}"
    );

    let (status, filtered) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users?username=alice@example.com"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListUsers?username: expected 200, got {status} with {filtered}"
    );
    assert_eq!(
        items(&filtered, "users").len(),
        1,
        "ListUsers?username=alice@example.com: expected the matching user, got {filtered}"
    );

    // DELETE -------------------------------------------------------------------
    let (status, deleted) = call(&router, "DELETE", &format!("/networks/{network_id}"), None).await;
    assert_eq!(
        status, 200,
        "DeleteNetwork: expected 200, got {status} with {deleted}"
    );
    assert!(
        deleted["message"].as_str().is_some_and(|m| !m.is_empty()),
        "DeleteNetwork: expected a message, got {deleted}"
    );

    for (operation, method, uri) in [
        ("GetNetwork", "GET", format!("/networks/{network_id}")),
        (
            "ListUsers",
            "GET",
            format!("/networks/{network_id}/users"),
        ),
        ("DeleteNetwork", "DELETE", format!("/networks/{network_id}")),
    ] {
        let (status, body) = call(&router, method, &uri, None).await;
        assert_eq!(
            status, 404,
            "{operation} after delete: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundError",
            "{operation} after delete: expected ResourceNotFoundError, got {body}"
        );
    }

    let (_, listed_after) = call(&router, "GET", "/networks", None).await;
    assert!(
        items(&listed_after, "networks").is_empty(),
        "ListNetworks after delete: the deleted network must be gone, got {listed_after}"
    );
}

#[tokio::test]
async fn create_network_enforces_the_documented_network_name_bound() {
    // `CreateNetworkRequest$networkName`: "Must be between 1 and 20 characters."
    // Both published bounds, one character past each, and a multi-byte name whose
    // character count differs from its byte count.
    let cases: &[(&str, usize, bool)] = &[
        ("empty", 0, false),
        ("one character", 1, true),
        ("nineteen characters", 19, true),
        ("the documented maximum", 20, true),
        ("one past the maximum", 21, false),
        ("far past the maximum", 64, false),
    ];

    let router = emulator();
    for (case, length, accepted) in cases {
        let name = "n".repeat(*length);
        let (status, body) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({ "networkName": name, "accessLevel": "STANDARD" })),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({length} characters): expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["networkName"],
                json!(name),
                "case {case:?}: expected the stored name back, got {body}"
            );
        } else {
            assert_eq!(
                status, 422,
                "case {case:?} ({length} characters): expected the modelled \
                 ValidationError status 422, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {case:?}: expected ValidationError, got {body}"
            );
            assert_eq!(
                body["reasons"][0]["field"],
                json!("networkName"),
                "case {case:?}: the refusal must name networkName, got {body}"
            );
        }
    }

    // A multi-byte name of 20 characters is 40 bytes: the bound is in characters.
    let (status, body) = call(
        &router,
        "POST",
        "/networks",
        Some(json!({ "networkName": "ü".repeat(20), "accessLevel": "STANDARD" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"20 multi-byte characters\": expected 200, got {status} with {body}"
    );
}

#[tokio::test]
async fn create_network_accepts_exactly_the_modelled_access_levels() {
    // Source: the `AccessLevel` enum's values.
    let router = emulator();
    for access_level in ["STANDARD", "PREMIUM"] {
        let network_id = create_network(&router, "catalog-net", access_level).await;
        let (status, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {access_level}: expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["accessLevel"],
            json!(access_level),
            "case {access_level}: expected the stored access level, got {fetched}"
        );
    }

    for off_catalog in ["standard", "ENTERPRISE", ""] {
        let (status, body) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({ "networkName": "catalog-net", "accessLevel": off_catalog })),
        )
        .await;
        assert_ne!(
            status, 200,
            "case {off_catalog:?}: a value outside AccessLevel must be refused, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {off_catalog:?}: expected this service's ValidationError code, got {body}"
        );
    }
}

#[tokio::test]
async fn a_premium_free_trial_is_reported_only_when_it_was_asked_for() {
    let router = emulator();
    for (case, enable, expect_expiry) in [
        ("trial not asked for", false, false),
        ("trial asked for", true, true),
    ] {
        let (status, created) = call(
            &router,
            "POST",
            "/networks",
            Some(json!({
                "networkName": "trial-net",
                "accessLevel": "PREMIUM",
                "enablePremiumFreeTrial": enable,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {created}"
        );
        let network_id = created["networkId"].as_str().unwrap_or_default();
        let (_, fetched) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        let expiry = fetched.get("freeTrialExpiration").cloned();
        assert_eq!(
            expiry.is_some(),
            expect_expiry,
            "case {case:?}: expected freeTrialExpiration present={expect_expiry}, got {fetched}"
        );
        if expect_expiry {
            // Reporting the same resource twice must report the same value.
            let (_, again) = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
            assert_eq!(
                again["freeTrialExpiration"], fetched["freeTrialExpiration"],
                "case {case:?}: the trial expiry must be stable across reads, got \
                 {} then {}",
                fetched["freeTrialExpiration"], again["freeTrialExpiration"]
            );
        }
    }
}

#[tokio::test]
async fn batch_create_user_decides_each_item_on_its_own() {
    let router = emulator();
    let network_id = create_network(&router, "batch-net", "STANDARD").await;

    // One request, one row per class of item: the creatable ones must be created and
    // the rest must land in `failed` without stopping them.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(json!({
            "users": [
                { "username": "ok-1@example.com", "securityGroupIds": ["0"] },
                { "username": "no-group@example.com", "securityGroupIds": [] },
                { "username": "two-groups@example.com", "securityGroupIds": ["0", "0"] },
                { "username": "unknown-group@example.com", "securityGroupIds": ["77"] },
                { "username": "ok-2@example.com", "firstName": "Bo", "lastName": "Li",
                  "securityGroupIds": ["0"] },
                { "username": "ok-1@example.com", "securityGroupIds": ["0"] },
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchCreateUser mixed batch: expected 200 partial success, got {status} with {body}"
    );

    let created: BTreeSet<String> = items(&body, "successful")
        .iter()
        .map(|user| user["username"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        created,
        ["ok-1@example.com".to_string(), "ok-2@example.com".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "case \"mixed batch\": expected exactly the creatable items to succeed, got {body}"
    );

    let failed = items(&body, "failed");
    assert_eq!(
        failed.len(),
        4,
        "case \"mixed batch\": expected the four uncreatable items to fail, got {body}"
    );
    for (index, expected_field) in [
        (0usize, "securityGroupIds"),
        (1, "securityGroupIds"),
        (2, "securityGroupIds"),
        (3, "username"),
    ] {
        assert_eq!(
            failed[index]["field"],
            json!(expected_field),
            "case \"mixed batch\" failure {index}: expected field {expected_field}, got {}",
            failed[index]
        );
        let reported = failed[index]["userId"].as_str().unwrap_or_default();
        assert!(
            is_user_id(reported),
            "case \"mixed batch\" failure {index}: userId must satisfy UserId, got \
             {reported:?}"
        );
    }

    // Only the creatable items are in the store.
    let (_, users) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users"),
        None,
    )
    .await;
    let stored: BTreeSet<String> = items(&users, "users")
        .iter()
        .map(|user| user["username"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        stored, created,
        "case \"mixed batch\": the store must hold exactly the created users, got {users}"
    );

    // An empty `users` list is a refusal, not a silent success.
    let (status, empty) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(json!({ "users": [] })),
    )
    .await;
    assert_eq!(
        status, 422,
        "case \"empty users list\": expected 422, got {status} with {empty}"
    );
    assert_eq!(
        error_code(&empty),
        "ValidationError",
        "case \"empty users list\": expected ValidationError, got {empty}"
    );
}

#[tokio::test]
async fn list_users_applies_every_filter_to_stored_state() {
    let router = emulator();
    let network_id = create_network(&router, "filter-net", "STANDARD").await;
    let (status, created) = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/users"),
        Some(json!({
            "users": [
                { "username": "ann@example.com", "firstName": "Ann", "lastName": "Ng",
                  "securityGroupIds": ["0"] },
                { "username": "bob@example.com", "firstName": "Bob", "lastName": "Ng",
                  "securityGroupIds": ["0"] },
                { "username": "cat@example.com", "firstName": "Cat", "lastName": "Oz",
                  "securityGroupIds": ["0"] },
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding users: expected 200, got {status} with {created}"
    );

    // Each row is one filter over the same stored set: the query, and the usernames
    // it must select.
    let cases: &[(&str, &[&str])] = &[
        ("", &["ann@example.com", "bob@example.com", "cat@example.com"]),
        ("?username=bob@example.com", &["bob@example.com"]),
        ("?username=nobody@example.com", &[]),
        ("?firstName=Cat", &["cat@example.com"]),
        ("?lastName=Ng", &["ann@example.com", "bob@example.com"]),
        ("?status=1", &["ann@example.com", "bob@example.com", "cat@example.com"]),
        ("?status=2", &[]),
        ("?groupId=0", &["ann@example.com", "bob@example.com", "cat@example.com"]),
        ("?groupId=77", &[]),
        ("?lastName=Ng&firstName=Ann", &["ann@example.com"]),
        ("?lastName=Ng&firstName=Cat", &[]),
    ];

    for (query, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let selected: Vec<String> = items(&body, "users")
            .iter()
            .map(|user| user["username"].as_str().unwrap_or_default().to_string())
            .collect();
        let wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            selected, wanted,
            "case {query:?}: expected {wanted:?}, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_through_every_item_exactly_once() {
    let router = emulator();

    // Five networks, each with a different number of users, so both listings are
    // paged at several page sizes against several collection sizes.
    let mut network_ids = Vec::new();
    for index in 0..5 {
        network_ids.push(create_network(&router, &format!("page-net-{index}"), "STANDARD").await);
    }
    let crowded = &network_ids[0];
    for index in 0..7 {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/networks/{crowded}/users"),
            Some(json!({
                "users": [{
                    "username": format!("user-{index}@example.com"),
                    "securityGroupIds": ["0"],
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding user {index}: expected 200, got {status} with {body}"
        );
    }

    for max_results in [1usize, 2, 3, 5, 8] {
        for (case, path, member, expected_total) in [
            ("networks", "/networks".to_string(), "networks", 5usize),
            (
                "users",
                format!("/networks/{crowded}/users"),
                "users",
                7,
            ),
        ] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let uri = match &token {
                    Some(value) => format!("{path}?maxResults={max_results}&nextToken={value}"),
                    None => format!("{path}?maxResults={max_results}"),
                };
                let (status, body) = call(&router, "GET", &uri, None).await;
                assert_eq!(
                    status, 200,
                    "case {case}/maxResults={max_results} page {pages}: expected 200, \
                     got {status} with {body}"
                );
                let page = items(&body, member);
                assert!(
                    page.len() <= max_results,
                    "case {case}/maxResults={max_results} page {pages}: a page must not \
                     exceed maxResults, got {} items",
                    page.len()
                );
                for item in &page {
                    let id = item[if member == "networks" {
                        "networkId"
                    } else {
                        "userId"
                    }]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                    seen.push(id);
                }
                pages += 1;
                token = body["nextToken"].as_str().map(ToString::to_string);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= expected_total + 1,
                    "case {case}/maxResults={max_results}: paging did not terminate \
                     after {pages} pages"
                );
            }

            let unique: BTreeSet<&String> = seen.iter().collect();
            assert_eq!(
                seen.len(),
                expected_total,
                "case {case}/maxResults={max_results}: expected {expected_total} items \
                 across all pages, got {seen:?}"
            );
            assert_eq!(
                unique.len(),
                expected_total,
                "case {case}/maxResults={max_results}: every item must appear exactly \
                 once, got {seen:?}"
            );
        }
    }

    // A page size below one, and a token this service never issued, are refusals.
    for (case, uri) in [
        ("maxResults=0", "/networks?maxResults=0".to_string()),
        ("maxResults=-1", "/networks?maxResults=-1".to_string()),
        ("a token from nowhere", "/networks?nextToken=zzz".to_string()),
        (
            "users with maxResults=0",
            format!("/networks/{crowded}/users?maxResults=0"),
        ),
        (
            "users with a token from nowhere",
            format!("/networks/{crowded}/users?nextToken=zzz"),
        ),
    ] {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listing_orders_by_the_requested_member_and_direction() {
    let router = emulator();
    // Created in an order that is neither the name order nor the reverse of it, so a
    // listing that ignored `sortFields` would be caught.
    for name in ["mid-net", "aaa-net", "zzz-net"] {
        create_network(&router, name, "STANDARD").await;
    }

    let cases: &[(&str, &[&str])] = &[
        ("", &["mid-net", "aaa-net", "zzz-net"]),
        ("?sortFields=networkId", &["mid-net", "aaa-net", "zzz-net"]),
        (
            "?sortFields=networkId&sortDirection=DESC",
            &["zzz-net", "aaa-net", "mid-net"],
        ),
        ("?sortFields=networkName", &["aaa-net", "mid-net", "zzz-net"]),
        (
            "?sortFields=networkName&sortDirection=DESC",
            &["zzz-net", "mid-net", "aaa-net"],
        ),
        (
            "?sortFields=networkName&sortDirection=ASC",
            &["aaa-net", "mid-net", "zzz-net"],
        ),
    ];
    for (query, expected) in cases {
        let (status, body) = call(&router, "GET", &format!("/networks{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let order: Vec<String> = items(&body, "networks")
            .iter()
            .map(|network| network["networkName"].as_str().unwrap_or_default().to_string())
            .collect();
        let wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            order, wanted,
            "case {query:?}: expected {wanted:?}, got {order:?}"
        );
    }

    // A sort field that is not a member of the listed shape is refused, not ignored.
    for (case, uri) in [
        ("networks", "/networks?sortFields=nickname"),
        ("networks, a user member", "/networks?sortFields=username"),
    ] {
        let (status, body) = call(&router, "GET", uri, None).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} with {body}"
        );
        assert_eq!(
            body["reasons"][0]["field"],
            json!("sortFields"),
            "case {case:?}: the refusal must name sortFields, got {body}"
        );
    }
}

#[tokio::test]
async fn users_sort_by_the_requested_member_and_by_id_numerically() {
    let router = emulator();
    let network_id = create_network(&router, "sort-net", "STANDARD").await;
    // Twelve users, so the ids cross the 1-to-2-digit boundary that a lexicographic
    // id order would get wrong.
    for index in 0..12 {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/users"),
            Some(json!({
                "users": [{
                    "username": format!("user-{index:02}@example.com"),
                    "securityGroupIds": ["0"],
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding user {index}: expected 200, got {status} with {body}"
        );
    }

    for (case, query) in [
        ("default order", ""),
        ("by userId", "?sortFields=userId"),
        ("by username", "?sortFields=username"),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let ids: Vec<i64> = items(&body, "users")
            .iter()
            .map(|user| {
                user["userId"]
                    .as_str()
                    .unwrap_or_default()
                    .parse::<i64>()
                    .unwrap_or_default()
            })
            .collect();
        let mut ascending = ids.clone();
        ascending.sort_unstable();
        assert_eq!(
            ids, ascending,
            "case {case:?}: expected ids in ascending numeric order, got {ids:?}"
        );
    }

    let (_, descending) = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/users?sortFields=username&sortDirection=DESC"),
        None,
    )
    .await;
    let names: Vec<String> = items(&descending, "users")
        .iter()
        .map(|user| user["username"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected = names.clone();
    expected.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(
        names, expected,
        "case \"by username DESC\": expected descending usernames, got {names:?}"
    );
}

#[tokio::test]
async fn an_unknown_but_well_formed_network_id_is_not_found() {
    let router = emulator();
    // Well formed per `NetworkId` (`^[0-9]{8}$`) and never minted by this store.
    let unknown = "99999999";
    let cases: &[(&str, &str, &str, Option<Value>)] = &[
        ("GetNetwork", "GET", "/networks/99999999", None),
        ("ListUsers", "GET", "/networks/99999999/users", None),
        ("DeleteNetwork", "DELETE", "/networks/99999999", None),
    ];
    for (operation, method, uri, body) in cases {
        let (status, response) = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {operation} on {unknown}: expected 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundError",
            "case {operation} on {unknown}: expected ResourceNotFoundError, got {response}"
        );
    }

    let (status, response) = call(
        &router,
        "POST",
        "/networks/99999999/users",
        Some(json!({ "users": [{ "username": "a@example.com", "securityGroupIds": ["0"] }] })),
    )
    .await;
    assert_eq!(
        status, 404,
        "case BatchCreateUser on {unknown}: expected 404, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundError",
        "case BatchCreateUser on {unknown}: expected ResourceNotFoundError, got {response}"
    );
}

#[tokio::test]
async fn caller_driven_work_is_bounded_by_the_server() {
    // Neither `users` nor `maxResults` carries a model bound, so the service imposes
    // its own; the test names the constants rather than repeating their values.
    use service_com_amazonaws_wickr_wickradminapi::state::{MAX_BATCH_USERS, MAX_PAGE_SIZE};

    let router = emulator();
    let network_id = create_network(&router, "bounded-net", "STANDARD").await;

    // A batch up to the cap is served in full; one item past it is refused whole.
    for (case, size, accepted) in [
        ("one item", 1usize, true),
        ("the batch cap", MAX_BATCH_USERS, true),
        ("one past the batch cap", MAX_BATCH_USERS + 1, false),
    ] {
        let users: Vec<Value> = (0..size)
            .map(|index| {
                json!({
                    "username": format!("b{size}-{index}@example.com"),
                    "securityGroupIds": ["0"],
                })
            })
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/users"),
            Some(json!({ "users": users })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({size} items): expected 200, got {status} with {body}"
            );
            assert_eq!(
                items(&body, "successful").len(),
                size,
                "case {case:?} ({size} items): expected every item created, got {body}"
            );
        } else {
            assert_eq!(
                status, 422,
                "case {case:?} ({size} items): expected 422, got {status} with {body}"
            );
            assert_eq!(
                body["reasons"][0]["field"],
                json!("users"),
                "case {case:?}: the refusal must name users, got {body}"
            );
        }
    }

    // The store now holds one more user than a page may carry, so a listing cannot be
    // made to serialize all of it: a request for everything, and a request for far
    // more than the cap, both come back at the cap with a token for the rest.
    let stored = MAX_BATCH_USERS + 1;
    for (case, query) in [
        ("no maxResults", String::new()),
        ("maxResults far above the cap", "?maxResults=1000".to_string()),
        (
            "maxResults at the cap",
            format!("?maxResults={MAX_PAGE_SIZE}"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            items(&body, "users").len(),
            MAX_PAGE_SIZE,
            "case {case:?}: a page must carry at most MAX_PAGE_SIZE items, got {}",
            items(&body, "users").len()
        );
        let token = body["nextToken"].as_str().unwrap_or_default().to_string();
        assert!(
            !token.is_empty(),
            "case {case:?}: a truncated page must carry a nextToken, got {body}"
        );
        let (status, rest) = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/users{query}{}nextToken={token}",
                if query.is_empty() { "?" } else { "&" }),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} page 2: expected 200, got {status} with {rest}"
        );
        assert_eq!(
            items(&body, "users").len() + items(&rest, "users").len(),
            stored,
            "case {case:?}: the two pages must together carry every stored user"
        );
        assert!(
            rest["nextToken"].as_str().is_none(),
            "case {case:?}: the last page must carry no nextToken, got {rest}"
        );
    }
}

#[tokio::test]
async fn a_network_id_outside_the_modelled_pattern_is_refused() {
    let router = emulator();
    // `NetworkId` is `^[0-9]{8}$`: one past each end of the length, and a value of
    // the right length that is not digits.
    for case in ["1234567", "123", "123456789", "abcdefgh", "1234567a"] {
        for (operation, method, uri) in [
            ("GetNetwork", "GET", format!("/networks/{case}")),
            ("ListUsers", "GET", format!("/networks/{case}/users")),
            ("DeleteNetwork", "DELETE", format!("/networks/{case}")),
        ] {
            let (status, body) = call(&router, method, &uri, None).await;
            // The status of a refusal the MODEL's own constraints produce is the
            // protocol's (400): it is decided before any handler runs, and a service
            // may reword the code but not the status. The modelled 422 is what a
            // handler-returned `ValidationError` carries.
            assert_eq!(
                status, 400,
                "case {operation}/{case:?}: expected the boundary refusal status 400, \
                 got {status} with {body}"
            );
            assert_ne!(
                status, 501,
                "case {operation}/{case:?}: a malformed id is a refusal, not an \
                 unimplemented operation, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {operation}/{case:?}: expected this service's ValidationError \
                 code, got {body}"
            );
        }
    }
}
