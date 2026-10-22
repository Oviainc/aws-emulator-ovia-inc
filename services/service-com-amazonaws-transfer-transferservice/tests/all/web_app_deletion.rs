// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteWebApp`.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns. The web apps are CREATED through `CreateWebApp`, so what
//! the delete removes is the record that create stored.
//!
//! RULE under test: `DeleteWebApp` removes the addressed web app from the store, so
//! afterwards every reader of it (`DescribeWebApp`, `ListWebApps`) answers as if it had
//! never been created and the store capacity it held is released, while every other
//! stored web app is left exactly as it was; a `WebAppId` the store does not hold —
//! never minted, or already deleted — is refused with this service's
//! `ResourceNotFoundException`, and removes nothing.
//!
//! Each suite asserts that rule over a TABLE of inputs rather than one recorded call:
//! the rows differ along which members the create named, which `WebAppEndpointType` the
//! web app ended up with, how many tags and subnets it carries, which Region it was
//! created in, which position in the store's order it occupies, and how many web apps
//! are deleted at once.

use serde_json::{json, Value};

use crate::harness::{
    assert_members, drain_pages, is_modelled_web_app_id, member_of, Emulator, REGION, ROLE,
};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s modelled
/// pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// `MAX_STORED_WEB_APPS` in `src/state.rs`: the emulator's own bound on the web app
/// store, which a delete releases one entry of.
const MAX_STORED_WEB_APPS: usize = 200;

/// Syntactically valid `WebAppId`s (`len 24..24`, `^webapp-[0-9a-f]{17}$`) that no
/// `CreateWebApp` ever minted: the low end of the id space, a mid value, and the high
/// end.
const NEVER_MINTED: [&str; 3] = [
    "webapp-00000000000000000",
    "webapp-0000000000000beef",
    "webapp-fffffffffffffffff",
];

/// The `IdentityProviderDetails` union every `CreateWebApp` must send:
/// `IdentityCenterConfig` is the union's only member and the union itself is `@required`.
fn identity_provider_details() -> Value {
    json!({"IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE}})
}

/// Create one web app from `extra` (merged over the required identity provider details),
/// signed for `region`, and return its minted `WebAppId`.
async fn create_web_app_in(emulator: &Emulator, region: &str, case: &str, extra: &Value) -> String {
    let mut request = json!({"IdentityProviderDetails": identity_provider_details()});
    for (member, value) in extra.as_object().expect("the row's members are an object") {
        request[member] = value.clone();
    }
    let answer = emulator.call_in(region, "CreateWebApp", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    let id = member_of(&answer.body, "WebAppId")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateWebApp returned no WebAppId: {}",
                answer.body
            )
        })
        .to_string();
    assert!(
        is_modelled_web_app_id(&id),
        "case {case:?}: expected a WebAppId matching ^webapp-[0-9a-f]{{17}}$ (len 24), got {id:?}"
    );
    id
}

/// [`create_web_app_in`] signed for the suite's default Region.
async fn create_web_app(emulator: &Emulator, case: &str, extra: &Value) -> String {
    create_web_app_in(emulator, REGION, case, extra).await
}

/// Delete one web app and assert the delete succeeded with the modelled empty output
/// (`DeleteWebApp`'s output shape is `Unit`, so it carries no member at all).
async fn delete_web_app(emulator: &Emulator, case: &str, web_app_id: &str) {
    let answer = emulator
        .call("DeleteWebApp", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    let members = answer
        .body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object body, got {}", answer.body));
    assert!(
        members.is_empty(),
        "case {case:?}: DeleteWebApp models an empty output, got the members {members:?}"
    );
}

/// Every `WebAppId` `ListWebApps` reports, through the shared paging drain every
/// `@paginated` suite in this crate uses (it bounds the page COUNT, so a listing that
/// never terminates fails instead of hanging).
async fn listed_web_app_ids(emulator: &Emulator) -> Vec<String> {
    let (_pages, ids) =
        drain_pages(emulator, "ListWebApps", &json!({}), "WebApps", "WebAppId", None).await;
    ids
}

/// Assert that `web_app_id` is gone from every reader of a web app: `DescribeWebApp`
/// refuses it with this service's not-found refusal, and `ListWebApps` does not list it.
async fn assert_absent(emulator: &Emulator, case: &str, web_app_id: &str) {
    let answer = emulator
        .call("DescribeWebApp", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected DescribeWebApp of a deleted web app to answer HTTP 400, \
         got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException after the delete, got {}",
        answer.body
    );

    let listed = listed_web_app_ids(emulator).await;
    assert!(
        !listed.contains(&web_app_id.to_string()),
        "case {case:?}: expected ListWebApps not to list the deleted {web_app_id:?}, \
         got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A delete removes the addressed record and nothing else.
// ---------------------------------------------------------------------------

/// RULE: `DeleteWebApp` removes exactly the addressed web app — afterwards no reader
/// answers for it — and leaves every other stored web app readable and listed.
///
/// The rows span the create class: a web app with every member defaulted, one with a
/// custom access endpoint and a FIPS endpoint policy and tags, a VPC-hosted one, one
/// with provisioned units and many tags, and one created in another Region. They are
/// deleted in an order that is not their stored (`WebAppId`) order, so a handler that
/// removed the first or the last record instead of the addressed one fails.
#[tokio::test]
async fn deleting_a_web_app_removes_only_that_web_app_from_every_reader() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, &str, Value)> = vec![
        ("every member defaulted", REGION, json!({})),
        (
            "a custom access endpoint, a FIPS policy and one tag",
            REGION,
            json!({
                "AccessEndpoint": "https://files.example.invalid",
                "WebAppEndpointPolicy": "FIPS",
                "Tags": [{"Key": "env", "Value": "dev"}],
            }),
        ),
        (
            "a VPC-hosted web app with two subnets",
            REGION,
            json!({"EndpointDetails": {"Vpc": {
                "VpcId": "vpc-0123456789abcdef0",
                "SubnetIds": ["subnet-0123456789abcdef0", "subnet-00000000000000001"],
            }}}),
        ),
        (
            "four provisioned units and three tags",
            REGION,
            json!({
                "WebAppUnits": {"Provisioned": 4},
                "Tags": [
                    {"Key": "env", "Value": "prod"},
                    {"Key": "team", "Value": "files"},
                    {"Key": "cost-center", "Value": "42"},
                ],
            }),
        ),
        ("created in another Region", "ap-southeast-2", json!({})),
    ];

    let mut live: Vec<(&str, String)> = Vec::new();
    for (case, region, extra) in &rows {
        live.push((*case, create_web_app_in(&emulator, region, case, extra).await));
    }

    // Delete order: the middle record, then the last, then the first, then what is
    // left — never simply front-to-back.
    let delete_order = [2usize, 4, 0, 1, 3];
    let mut deleted: Vec<String> = Vec::new();

    for index in delete_order {
        let (case, web_app_id) = live[index].clone();
        delete_web_app(&emulator, case, &web_app_id).await;
        deleted.push(web_app_id.clone());
        assert_absent(&emulator, case, &web_app_id).await;

        // Every web app not yet deleted is still there, unchanged in identity.
        let listed = listed_web_app_ids(&emulator).await;
        for (other_case, other) in &live {
            if deleted.contains(other) {
                continue;
            }
            let answer = emulator
                .call("DescribeWebApp", json!({"WebAppId": other}))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the surviving web app of {other_case:?} to still \
                 describe, got {} with body {}",
                answer.status, answer.body
            );
            assert_members(case, member_of(&answer.body, "WebApp"), &json!({"WebAppId": other}));
            assert!(
                listed.contains(other),
                "case {case:?}: expected ListWebApps to still list {other:?} of \
                 {other_case:?}, got {listed:?}"
            );
        }
    }

    // With every row deleted the store is empty again.
    let listed = listed_web_app_ids(&emulator).await;
    assert!(
        listed.is_empty(),
        "case \"after deleting every web app\": expected an empty listing, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A web app the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: `DeleteWebApp` for a `WebAppId` the store does not hold — one never minted, or
/// one already deleted — answers this service's not-found refusal (HTTP 400
/// `ResourceNotFoundException`, the capitalized `ResourceType`, the requested identifier
/// in `Resource` alone and a fixed generic `Message` that carries no identifier) and
/// removes nothing.
///
/// The rows are the ends and the middle of the id space that was never minted, plus a
/// web app that WAS created and then deleted, so the refusal does not depend on how the
/// id came to be absent.
#[tokio::test]
async fn deleting_a_web_app_the_store_does_not_hold_answers_the_generic_not_found() {
    let emulator = Emulator::new();

    // A store that DOES hold a web app, so the refusal is about the id, not emptiness.
    let survivor = create_web_app(&emulator, "a live web app", &json!({})).await;

    let once_created = create_web_app(&emulator, "a web app to delete twice", &json!({})).await;
    delete_web_app(&emulator, "a web app to delete twice", &once_created).await;

    let mut absent: Vec<String> = NEVER_MINTED.iter().map(|id| id.to_string()).collect();
    absent.push(once_created);

    let mut messages: Vec<String> = Vec::new();

    for web_app_id in &absent {
        assert!(
            is_modelled_web_app_id(web_app_id),
            "the probe id {web_app_id:?} must satisfy WebAppId's modelled pattern"
        );
        let answer = emulator
            .call("DeleteWebApp", json!({"WebAppId": web_app_id}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {web_app_id:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {web_app_id:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_members(
            web_app_id,
            &answer.body,
            &json!({"Resource": web_app_id, "ResourceType": "WebApp"}),
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty() && !message.contains(web_app_id.as_str()),
            "case {web_app_id:?}: expected a non-empty Message carrying no identifier, \
             got {message:?}"
        );
        messages.push(message);
    }

    for message in &messages {
        assert_eq!(
            message, &messages[0],
            "expected one fixed WebApp not-found message, got {message:?} beside {:?}",
            messages[0]
        );
    }

    // None of the refusals removed the web app the store does hold.
    let listed = listed_web_app_ids(&emulator).await;
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "case \"after the refused deletes\": expected only {survivor:?} to remain, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A delete releases the store's capacity.
// ---------------------------------------------------------------------------

/// RULE: the entry a deleted web app held is RELEASED, so a store at its bound takes
/// exactly as many further creates as were deleted — no more.
///
/// The rows differ along how many web apps are deleted before creating again (one, then
/// several), because the released capacity is a count rather than a single slot.
#[tokio::test]
async fn deleting_a_web_app_releases_the_store_capacity_it_held() {
    let emulator = Emulator::new();

    let mut stored: Vec<String> = Vec::with_capacity(MAX_STORED_WEB_APPS);
    for index in 0..MAX_STORED_WEB_APPS {
        let case = format!("web app {index} of the bound");
        stored.push(create_web_app(&emulator, &case, &json!({})).await);
    }

    for released in [1usize, 3] {
        let case = format!("after releasing {released} of the bound");

        // At the bound, a create is refused.
        let refused = emulator
            .call(
                "CreateWebApp",
                json!({"IdentityProviderDetails": identity_provider_details()}),
            )
            .await;
        assert_eq!(
            refused.status, 429,
            "case {case:?}: expected a create at the bound to answer HTTP 429, got {} with \
             body {}",
            refused.status, refused.body
        );

        for _ in 0..released {
            let victim = stored.remove(0);
            delete_web_app(&emulator, &case, &victim).await;
            assert_absent(&emulator, &case, &victim).await;
        }

        // Exactly `released` creates fit, and the next one is refused again.
        for slot in 0..released {
            let case = format!("{case}: create {slot} into the released capacity");
            stored.push(create_web_app(&emulator, &case, &json!({})).await);
        }
        let refused = emulator
            .call(
                "CreateWebApp",
                json!({"IdentityProviderDetails": identity_provider_details()}),
            )
            .await;
        assert_eq!(
            refused.status, 429,
            "case {case:?}: expected the create past the refilled bound to answer HTTP 429, \
             got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ThrottlingException",
            "case {case:?}: expected ThrottlingException, got {}",
            refused.body
        );
    }

    // The store is back at its bound, and every record in it is still readable.
    let listed = listed_web_app_ids(&emulator).await;
    assert_eq!(
        listed.len(),
        MAX_STORED_WEB_APPS,
        "case \"the refilled store\": expected {MAX_STORED_WEB_APPS} web apps, got {}",
        listed.len()
    );
}
