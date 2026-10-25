// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `UpdateVpcLink`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules these cases assert:
//!
//! * **`UpdateVpcLink` is a PATCH of the stored link** (`PATCH /v2/vpclinks/{VpcLinkId}`):
//!   `Name` is the only member the operation takes, so a request carrying it replaces the
//!   stored name, a request omitting it leaves the stored name exactly as it was, and
//!   every member the operation does not take — the link's id, subnets, security groups,
//!   tags, creation instant and version — is carried over untouched. What the response
//!   reports is what a later `GetVpcLink` reports.
//! * **The provisioning the clock owes the link is not restarted by a patch**: a link that
//!   has reached `AVAILABLE` is still `AVAILABLE` after being renamed.
//! * **A link is patched only through the id it was minted with**: an id no stored link
//!   carries is 404 `NotFoundException` and writes nothing.
//! * **A name outside the documented bound is refused 400 `BadRequestException` and
//!   persists nothing**: `Name` targets `StringWithLengthBetween1And128`, documented as
//!   "A string with a length between [1-128]" and carrying no `@length` trait, so 0 and
//!   129 characters are both refused — on the create that mints a link as on the patch
//!   that renames it.
//!
//! The rows vary what the rules generalise over: whether the patch carries the name at
//! all, the name's length at and past each published bound, its script, whether it repeats
//! the value already stored, and how many subnets, security groups and tags the patched
//! link holds. The recorded VPC-link lifecycle that filed the issue — create, read, rename
//! — is ONE row of the first table.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    VPC_LINK_CREATE_SPAN, VPC_LINK_STATUS_AVAILABLE,
};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The VPC-links collection URI (`CreateVpcLink`'s modelled `POST /v2/vpclinks`); one
/// link hangs off it as `{VPC_LINKS}/{id}`, the URI both `GetVpcLink` and `UpdateVpcLink`
/// are bound to.
const VPC_LINKS: &str = "/v2/vpclinks";

/// The members of the stored link `UpdateVpcLink` does NOT take, so a patch must carry
/// every one of them over unchanged. `VpcLinkStatus` and `VpcLinkStatusMessage` are left
/// out on purpose: the CLOCK may move them between two calls, and
/// [`a_patch_does_not_restart_the_provisioning_the_clock_owes`] is the case over that.
const CARRIED_OVER: [&str; 6] = [
    "vpcLinkId",
    "subnetIds",
    "securityGroupIds",
    "tags",
    "createdDate",
    "vpcLinkVersion",
];

/// A fresh emulator holding only this service, plus the engine it was built from, so a
/// test can assert on the store itself — what a refused patch must NOT have written.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// The `Name` the store holds for one link, read from the state struct rather than over
/// the wire, so "nothing was persisted" is asserted against the record itself.
fn stored_name(engine: &engine::InMemoryEngine, vpc_link_id: &str) -> Option<String> {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .vpc_links
        .get(vpc_link_id)
        .map(|link| link.name.clone())
}

/// One patch row: the link to create, an optional patch applied BEFORE the one under test
/// (so a name an earlier patch stored is covered too), and the patch under test.
struct Row {
    case: &'static str,
    create: Value,
    earlier_patch: Option<Value>,
    patch: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a new name onto a link created with only the required members",
            create: json!({ "name": "original", "subnetIds": ["subnet-0a1b2c3d"] }),
            earlier_patch: None,
            patch: json!({ "name": "renamed" }),
        },
        Row {
            case: "a new name onto a link holding several subnets, security groups and tags",
            create: json!({
                "name": "multi-az",
                "subnetIds": [
                    "subnet-0a1b2c3d",
                    "subnet-00112233445566778",
                    "subnet-ffffffffffffffff0"
                ],
                "securityGroupIds": ["sg-0a1b2c3d", "sg-00112233445566778"],
                "tags": { "team": "platform", "env": "test" }
            }),
            earlier_patch: None,
            patch: json!({ "name": "multi-az-renamed" }),
        },
        Row {
            case: "a patch carrying no member at all, which leaves the stored name",
            create: json!({
                "name": "untouched",
                "subnetIds": ["subnet-0123456789abcdef0"],
                "tags": { "owner": "api-team" }
            }),
            earlier_patch: None,
            patch: json!({}),
        },
        Row {
            case: "the name the link already carries, repeated",
            create: json!({ "name": "idempotent", "subnetIds": ["subnet-0a1b2c3d"] }),
            earlier_patch: None,
            patch: json!({ "name": "idempotent" }),
        },
        Row {
            // `Name` is a `StringWithLengthBetween1And128`, so one character is its
            // published lower bound.
            case: "a one-character name, the modelled minimum",
            create: json!({ "name": "shortening", "subnetIds": ["subnet-0a1b2c3d"] }),
            earlier_patch: None,
            patch: json!({ "name": "x" }),
        },
        Row {
            // ... and 128 characters is its published upper bound.
            case: "a name at the modelled 128-character bound",
            create: json!({ "name": "lengthening", "subnetIds": ["subnet-0a1b2c3d"] }),
            earlier_patch: None,
            patch: json!({ "name": "n".repeat(128) }),
        },
        Row {
            case: "a name outside ASCII, which round-trips unchanged",
            create: json!({
                "name": "ascii",
                "subnetIds": ["subnet-0a1b2c3d"],
                "securityGroupIds": ["sg-0123456789abcdef0"]
            }),
            earlier_patch: None,
            patch: json!({ "name": "決済-VPC-Link — ünïcödé 🎉" }),
        },
        Row {
            case: "a second patch over the name an earlier one stored",
            create: json!({
                "name": "first",
                "subnetIds": ["subnet-0a1b2c3d", "subnet-00112233445566778"],
                "tags": { "stage": "two-patches" }
            }),
            earlier_patch: Some(json!({ "name": "second" })),
            patch: json!({ "name": "third" }),
        },
        Row {
            case: "an empty patch after an earlier one renamed the link",
            create: json!({ "name": "before", "subnetIds": ["subnet-0a1b2c3d"] }),
            earlier_patch: Some(json!({ "name": "after the earlier patch" })),
            patch: json!({}),
        },
    ]
}

/// Create one VPC link and return its minted `VpcLinkId`.
async fn create_link(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", VPC_LINKS, Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateVpcLink, got {status} {created}"
    );
    as_str(&created, "vpcLinkId")
}

/// Read one VPC link back, asserting the read succeeded.
async fn get_link(emulator: &MultiServiceRouter, case: &str, vpc_link_id: &str) -> Value {
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("{VPC_LINKS}/{vpc_link_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetVpcLink, got {status} {fetched}"
    );
    fetched
}

#[tokio::test]
async fn a_patch_replaces_the_name_it_carries_and_carries_every_other_member_over() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let vpc_link_id = create_link(&emulator, case, &row.create).await;
        let uri = format!("{VPC_LINKS}/{vpc_link_id}");

        if let Some(earlier) = &row.earlier_patch {
            let (status, patched) = call(&emulator, "PATCH", &uri, Some(earlier.clone())).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from the earlier UpdateVpcLink, got {status} {patched}"
            );
        }

        // The record as it stands before the patch under test.
        let before = get_link(&emulator, case, &vpc_link_id).await;

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(row.patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateVpcLink, got {status} {patched}"
        );

        // The name the request carried, else the one the record already held.
        let expected_name = row
            .patch
            .get("name")
            .cloned()
            .unwrap_or_else(|| before.get("name").cloned().unwrap_or(Value::Null));
        assert_eq!(
            patched.get("name"),
            Some(&expected_name),
            "case {case:?}: expected UpdateVpcLink to report Name {expected_name}, got {patched}"
        );

        // Every member this operation does not take survived the patch.
        for member in CARRIED_OVER {
            assert_eq!(
                patched.get(member),
                before.get(member),
                "case {case:?}: expected the stored {member} {:?} to survive the patch, got \
                 {patched}",
                before.get(member)
            );
        }
        // A status message is still reported, summarising whatever status the clock owes
        // the link at this instant.
        assert!(
            !as_str(&patched, "vpcLinkStatusMessage").is_empty(),
            "case {case:?}: expected a VpcLinkStatusMessage summarising the status, got {patched}"
        );

        // What the response reports is what a later reader reports: the patch was
        // persisted, not echoed.
        let fetched = get_link(&emulator, case, &vpc_link_id).await;
        assert_eq!(
            fetched.get("name"),
            Some(&expected_name),
            "case {case:?}: expected GetVpcLink to report Name {expected_name}, got {fetched}"
        );
        for member in CARRIED_OVER {
            assert_eq!(
                fetched.get(member),
                before.get(member),
                "case {case:?}: expected GetVpcLink to report the stored {member} {:?}, got \
                 {fetched}",
                before.get(member)
            );
        }
    }
}

#[tokio::test]
async fn a_patch_does_not_restart_the_provisioning_the_clock_owes() {
    let emulator = emulator();
    let case = "renaming a link that has finished provisioning";

    let vpc_link_id = create_link(
        &emulator,
        case,
        &json!({ "name": "provisioned", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;
    let uri = format!("{VPC_LINKS}/{vpc_link_id}");

    // Past the span the create started, so the link has reached the status the clock owes
    // it before the patch runs.
    tokio::time::sleep(VPC_LINK_CREATE_SPAN + Duration::from_millis(100)).await;
    let before = get_link(&emulator, case, &vpc_link_id).await;
    assert_eq!(
        as_str(&before, "vpcLinkStatus"),
        VPC_LINK_STATUS_AVAILABLE,
        "case {case:?}: expected {VPC_LINK_STATUS_AVAILABLE} once the span has passed, got {before}"
    );

    let (status, patched) = call(
        &emulator,
        "PATCH",
        &uri,
        Some(json!({ "name": "provisioned-renamed" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateVpcLink, got {status} {patched}"
    );
    assert_eq!(
        as_str(&patched, "name"),
        "provisioned-renamed",
        "case {case:?}: expected the patched Name, got {patched}"
    );
    // Renaming a link does not re-attach its interfaces, so the status it reports is the
    // one it had reached — a patch moves it neither back to provisioning nor onward.
    for (round, document) in [
        ("the patch response", patched),
        ("a later GetVpcLink", get_link(&emulator, case, &vpc_link_id).await),
    ] {
        assert_eq!(
            as_str(&document, "vpcLinkStatus"),
            VPC_LINK_STATUS_AVAILABLE,
            "case {case:?} ({round}): expected {VPC_LINK_STATUS_AVAILABLE} to survive the \
             patch, got {document}"
        );
    }
}

#[tokio::test]
async fn a_link_is_patched_only_through_the_id_it_was_minted_with() {
    let (emulator, engine) = emulator_with_store();
    let case = "a patch addressed at an id no link carries";

    // One link exists, so a miss is a miss on the identifier and not on an empty store.
    let vpc_link_id = create_link(
        &emulator,
        case,
        &json!({ "name": "the-one-link", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;
    let before = get_link(&emulator, case, &vpc_link_id).await;

    for (label, id) in [
        ("an id no link carries", "aaaaaaaaaa".to_string()),
        ("an id that is not of the minted form", "no-such-link".to_string()),
        ("the stored id in uppercase", vpc_link_id.to_uppercase()),
        (
            "the stored id with its first character dropped",
            vpc_link_id[1..].to_string(),
        ),
        ("a long id no link carries", "z".repeat(30)),
    ] {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("{VPC_LINKS}/{id}"),
            Some(json!({ "name": "should not land" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from UpdateVpcLink, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            stored_name(&engine, &id),
            None,
            "case {label:?}: a refused UpdateVpcLink must not create the link it missed"
        );
    }

    // None of those refusals wrote anything: the link that does exist is unchanged.
    let after = get_link(&emulator, case, &vpc_link_id).await;
    assert_eq!(
        after.get("name"),
        before.get("name"),
        "case {case:?}: expected the existing link to keep its Name, got {after} against {before}"
    );
    assert_eq!(
        stored_name(&engine, &vpc_link_id).as_deref(),
        Some("the-one-link"),
        "case {case:?}: expected the stored Name to be untouched by the refused patches"
    );
}

#[tokio::test]
async fn a_name_outside_the_documented_bound_is_refused_and_persists_nothing() {
    let (emulator, engine) = emulator_with_store();
    let case = "a patch whose Name is outside the documented bound";

    let vpc_link_id = create_link(
        &emulator,
        case,
        &json!({ "name": "valid", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;
    let uri = format!("{VPC_LINKS}/{vpc_link_id}");

    // `Name` targets `StringWithLengthBetween1And128`, documented as "A string with a
    // length between [1-128]", so each published bound is accepted and the length either
    // side of it is not.
    let rows: [(&str, String, u16); 4] = [
        ("an empty name, one below the minimum", String::new(), 400),
        ("a name at the minimum", "n".to_string(), 200),
        ("a name at the maximum", "n".repeat(128), 200),
        ("a name one character past the maximum", "n".repeat(129), 400),
    ];

    for (label, name, expected) in rows.clone() {
        let (status, body) = call(&emulator, "PATCH", &uri, Some(json!({ "name": name }))).await;
        assert_eq!(
            status, expected,
            "case {label:?} ({} characters): expected {expected} from UpdateVpcLink, got \
             {status} {body}",
            name.chars().count()
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {label:?}: expected BadRequestException from UpdateVpcLink, got {body}"
            );
            assert_ne!(
                stored_name(&engine, &vpc_link_id).as_deref(),
                Some(name.as_str()),
                "case {label:?}: a refused UpdateVpcLink must persist nothing"
            );
        } else {
            assert_eq!(
                stored_name(&engine, &vpc_link_id).as_deref(),
                Some(name.as_str()),
                "case {label:?}: expected the store to hold the patched Name"
            );
        }
    }

    // The bound belongs to the member, not to one operation: the create that mints a link
    // reads the same rule off the same shape.
    for (label, name, expected) in rows {
        let (status, body) = call(
            &emulator,
            "POST",
            VPC_LINKS,
            Some(json!({ "name": name, "subnetIds": ["subnet-0a1b2c3d"] })),
        )
        .await;
        let expected = if expected == 200 { 201 } else { expected };
        assert_eq!(
            status, expected,
            "case {label:?} ({} characters): expected {expected} from CreateVpcLink, got \
             {status} {body}",
            name.chars().count()
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {label:?}: expected BadRequestException from CreateVpcLink, got {body}"
            );
        }
    }
}
