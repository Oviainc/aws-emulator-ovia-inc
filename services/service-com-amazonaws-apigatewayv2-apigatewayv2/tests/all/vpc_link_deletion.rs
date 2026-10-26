// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteVpcLink`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules these cases assert:
//!
//! * **`DeleteVpcLink` ACCEPTS the removal of a link the account holds**
//!   (`DELETE /v2/vpclinks/{VpcLinkId}`, modelled HTTP 202 with an empty body): the link
//!   stays readable and reports `DELETING` while its deprovisioning is outstanding, and
//!   the account stops holding it once the span the clock owes has passed — `GetVpcLink`
//!   is then a 404. Reading it neither hurries the removal along nor holds it back, and a
//!   delete of a link already being deleted is accepted again.
//! * **Deleting one link touches no other**: every sibling link of the account is
//!   readable, unchanged, after the delete and after the removal falls due.
//! * **A link occupies its account's quota slot until it is gone**, so a create at the
//!   quota is still refused 429 while a deleted link is deprovisioning and is accepted,
//!   with an id of its own, once that link has gone.
//! * **A `VpcLinkId` no stored link carries is a 404 `NotFoundException` that persists
//!   nothing**, including the id of a link whose removal has already fallen due — and
//!   every operation addressed as `/v2/vpclinks/{VpcLinkId}` refuses it with the one
//!   sentence API Gateway answers an unresolved VPC link identifier with.
//!
//! The rows vary what the rules generalise over: the number of subnets and which of the
//! two published EC2 id forms each uses, whether the link carries security groups or
//! tags, the name's length, how many sibling links the account holds, and — for the
//! refusals — the way the id departs from a stored one. The recorded VPC-link lifecycle
//! that filed the issue (create, read, rename, delete) is ONE row of the first table.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    invalid_identifier_message, MAX_VPC_LINKS, VPC_LINK_DELETE_SPAN, VPC_LINK_RESOURCE,
    VPC_LINK_STATUS_DELETING,
};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, error_code};

/// The VPC-links collection URI (`CreateVpcLink`'s modelled `POST /v2/vpclinks`); one
/// link hangs off it as `{VPC_LINKS}/{id}`, the URI `GetVpcLink` and `DeleteVpcLink` are
/// both bound to.
const VPC_LINKS: &str = "/v2/vpclinks";

/// Long enough past a span for the instant it is due to have passed, whatever rounding
/// the clock does.
const PAST_THE_SPAN: Duration = Duration::from_millis(150);

/// The members of a stored link that a delete must not rewrite, so the link reports them
/// unchanged while it is still being deprovisioned. `VpcLinkStatus` and
/// `VpcLinkStatusMessage` are left out on purpose: those are what the delete MOVES.
const CARRIED_OVER: [&str; 7] = [
    "vpcLinkId",
    "name",
    "subnetIds",
    "securityGroupIds",
    "tags",
    "createdDate",
    "vpcLinkVersion",
];

/// The sentence API Gateway answers an unresolved VPC link identifier with, spelled out
/// here rather than taken from the crate's constant: a test that asserts a handler's own
/// constant back at it cannot catch the wording drifting away from the service's.
const UNRESOLVED_IDENTIFIER_SENTENCE: &str = "Invalid Vpc Link identifier specified";

/// A fresh emulator holding only this service, plus the engine it was built from, so a
/// test can assert on the store itself — what a refused delete must NOT have removed.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// How many VPC links the store holds.
fn stored_links(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .vpc_links
        .len()
}

/// Create one VPC link and answer the response body the create reported.
async fn create_link(emulator: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, created) = call(emulator, "POST", VPC_LINKS, Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateVpcLink, got {status} {created}"
    );
    created
}

/// One delete row: the link to delete, and the sibling links the account holds beside it.
struct Row {
    case: &'static str,
    create: Value,
    siblings: Vec<Value>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only link of the account, created with only the required members",
            create: json!({ "name": "minimal", "subnetIds": ["subnet-0a1b2c3d"] }),
            siblings: vec![],
        },
        Row {
            case: "a link of the 17-character subnet id form, with one security group",
            create: json!({
                "name": "long-form-ids",
                "subnetIds": ["subnet-0123456789abcdef0"],
                "securityGroupIds": ["sg-0123456789abcdef0"]
            }),
            siblings: vec![json!({ "name": "sibling", "subnetIds": ["subnet-0a1b2c3d"] })],
        },
        Row {
            case: "a link holding several subnets of both id forms, security groups and tags",
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
            siblings: vec![
                json!({ "name": "sibling-one", "subnetIds": ["subnet-0a1b2c3d"] }),
                json!({
                    "name": "sibling-two",
                    "subnetIds": ["subnet-00112233445566778"],
                    "tags": { "keep": "me" }
                }),
            ],
        },
        Row {
            // `Name` is a `StringWithLengthBetween1And128`, so 128 characters is its
            // published upper bound: a link named at the bound deletes like any other.
            case: "a link whose name sits at the modelled 128-character bound",
            create: json!({
                "name": "n".repeat(128),
                "subnetIds": ["subnet-0a1b2c3d"]
            }),
            siblings: vec![json!({ "name": "sibling", "subnetIds": ["subnet-0a1b2c3d"] })],
        },
    ]
}

#[tokio::test]
async fn delete_vpc_link_is_accepted_and_the_link_goes_away_once_its_span_has_passed() {
    for row in rows() {
        let case = row.case;
        let (emulator, engine) = emulator_with_store();

        // The siblings first, so the link under test is neither the first nor the last
        // key of the store in every row.
        let mut sibling_ids = Vec::new();
        for sibling in &row.siblings {
            let created = create_link(&emulator, case, sibling.clone()).await;
            sibling_ids.push(as_str(&created, "vpcLinkId"));
        }
        let created = create_link(&emulator, case, row.create.clone()).await;
        let vpc_link_id = as_str(&created, "vpcLinkId");
        let uri = format!("{VPC_LINKS}/{vpc_link_id}");
        let (_, before) = call(&emulator, "GET", &uri, None).await;
        let status_before = as_str(&before, "vpcLinkStatusMessage");

        // The delete is ACCEPTED: the modelled 202, with a response structure the model
        // declares no members on, so there is nothing about the link in the answer.
        let (status, body) = call(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from DeleteVpcLink, got {status} {body}"
        );
        assert!(
            body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: expected DeleteVpcLink to report no members, got {body}"
        );

        // While the deprovisioning is outstanding the link is still readable and reports
        // DELETING — and reading it twice neither hurries it along nor holds it back.
        for round in 0..2 {
            let (status, read) = call(&emulator, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?} (GetVpcLink round {round} within the span): expected 200, got {status} {read}"
            );
            assert_eq!(
                as_str(&read, "vpcLinkStatus"),
                VPC_LINK_STATUS_DELETING,
                "case {case:?} (GetVpcLink round {round} within the span): expected \
                 {VPC_LINK_STATUS_DELETING}, got {read}"
            );
            // The message summarises the status it is reported with, so it moved with it.
            let message = as_str(&read, "vpcLinkStatusMessage");
            assert!(
                !message.is_empty() && message != status_before,
                "case {case:?} (GetVpcLink round {round} within the span): expected a message \
                 summarising {VPC_LINK_STATUS_DELETING}, got {message:?}"
            );
            // Nothing else about the record was rewritten by the delete.
            for member in CARRIED_OVER {
                assert_eq!(
                    read.get(member),
                    created.get(member),
                    "case {case:?} (GetVpcLink round {round} within the span): expected the \
                     stored {member} {:?}, got {read}",
                    created.get(member)
                );
            }
        }

        // A delete of a link already being deleted is accepted again: the removal was
        // already asked for, so there is nothing to refuse.
        let (status, body) = call(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 202,
            "case {case:?} (a second DeleteVpcLink within the span): expected 202, got {status} {body}"
        );

        // The removal is one the clock owes: once the span has passed the account no
        // longer holds the link, for every reader and in the store itself.
        tokio::time::sleep(VPC_LINK_DELETE_SPAN + PAST_THE_SPAN).await;
        let (status, read) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?} (GetVpcLink after the span): expected 404, got {status} {read}"
        );
        assert_eq!(
            error_code(&read),
            "NotFoundException",
            "case {case:?} (GetVpcLink after the span): expected NotFoundException, got {read}"
        );
        assert_eq!(
            as_str(&read, "message"),
            UNRESOLVED_IDENTIFIER_SENTENCE,
            "case {case:?} (GetVpcLink after the span): expected the sentence API Gateway \
             answers an unresolved VPC link identifier with, got {read}"
        );
        assert_eq!(
            stored_links(&engine),
            row.siblings.len(),
            "case {case:?}: expected only the {} sibling link(s) left in the store",
            row.siblings.len()
        );

        // Deleting one link touched no other: every sibling is readable, unchanged.
        for sibling_id in &sibling_ids {
            let (status, read) =
                call(&emulator, "GET", &format!("{VPC_LINKS}/{sibling_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?} (sibling {sibling_id:?} after the delete): expected 200, got {status} {read}"
            );
            assert_ne!(
                as_str(&read, "vpcLinkStatus"),
                VPC_LINK_STATUS_DELETING,
                "case {case:?} (sibling {sibling_id:?} after the delete): expected a sibling not \
                 to be deleted too, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn an_id_no_stored_link_carries_cannot_be_deleted() {
    let (emulator, engine) = emulator_with_store();

    // One link exists, so a miss is a miss on the identifier and not on an empty store.
    let created = create_link(
        &emulator,
        "the surviving link",
        json!({ "name": "the-one-link", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;
    let known = as_str(&created, "vpcLinkId");

    // And one link that was deleted and whose removal has already fallen due: its id
    // names nothing the account holds any more, so it is a miss like any other.
    let gone = create_link(
        &emulator,
        "the link that was already deleted",
        json!({ "name": "already-deleted", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;
    let gone_id = as_str(&gone, "vpcLinkId");
    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("{VPC_LINKS}/{gone_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"the first delete of the already-deleted link\": expected 202, got {status} {body}"
    );
    tokio::time::sleep(VPC_LINK_DELETE_SPAN + PAST_THE_SPAN).await;

    for (case, vpc_link_id) in [
        ("an id no link carries", "aaaaaaaaaa".to_string()),
        ("an id that is not of the minted form", "no-such-link".to_string()),
        (
            "the stored id with its last character changed",
            // Map the last character onto one it is not, so the row is a miss whatever
            // the id bijection mints.
            format!(
                "{}{}",
                &known[..known.len() - 1],
                if known.ends_with('z') { 'a' } else { 'z' }
            ),
        ),
        ("the id of a link whose removal already fell due", gone_id.clone()),
    ] {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("{VPC_LINKS}/{vpc_link_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteVpcLink, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            UNRESOLVED_IDENTIFIER_SENTENCE,
            "case {case:?}: expected the sentence API Gateway answers an unresolved VPC link \
             identifier with, got {body}"
        );
        // A refused delete removes nothing: the one surviving link is still there.
        assert_eq!(
            stored_links(&engine),
            1,
            "case {case:?}: a refused DeleteVpcLink must remove nothing"
        );
        let (status, read) = call(&emulator, "GET", &format!("{VPC_LINKS}/{known}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the surviving link to still read back, got {status} {read}"
        );
    }
}

#[tokio::test]
async fn a_deleted_links_quota_slot_is_free_only_once_the_link_is_gone() {
    let (emulator, engine) = emulator_with_store();

    let mut ids = Vec::new();
    for index in 0..MAX_VPC_LINKS {
        let created = create_link(
            &emulator,
            "filling the account to its quota",
            json!({
                "name": format!("link-{index}"),
                "subnetIds": ["subnet-0a1b2c3d"]
            }),
        )
        .await;
        ids.push(as_str(&created, "vpcLinkId"));
    }

    let one_too_many = json!({ "name": "one-too-many", "subnetIds": ["subnet-0a1b2c3d"] });
    let (status, body) = call(&emulator, "POST", VPC_LINKS, Some(one_too_many.clone())).await;
    assert_eq!(
        status, 429,
        "case \"at the quota\": expected 429 from CreateVpcLink, got {status} {body}"
    );

    // A deleted link still occupies its slot while it is being deprovisioned: the
    // account holds it until the removal the clock owes falls due.
    let deleted = ids.first().expect("the quota is at least one link").clone();
    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("{VPC_LINKS}/{deleted}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"deleting one link of a full account\": expected 202, got {status} {body}"
    );
    let (status, body) = call(&emulator, "POST", VPC_LINKS, Some(one_too_many.clone())).await;
    assert_eq!(
        status, 429,
        "case \"a create while the deleted link is still deprovisioning\": expected 429, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "case \"a create while the deleted link is still deprovisioning\": expected \
         TooManyRequestsException, got {body}"
    );
    assert_eq!(
        stored_links(&engine),
        MAX_VPC_LINKS,
        "case \"a create while the deleted link is still deprovisioning\": expected the store to \
         still hold {MAX_VPC_LINKS} links"
    );

    // Once it is gone the slot is free, and what fills it is a NEW link with an id of
    // its own rather than the one that was deleted.
    tokio::time::sleep(VPC_LINK_DELETE_SPAN + PAST_THE_SPAN).await;
    let created = create_link(
        &emulator,
        "a create once the deleted link has gone",
        one_too_many,
    )
    .await;
    let replacement = as_str(&created, "vpcLinkId");
    assert!(
        !ids.contains(&replacement),
        "case \"a create once the deleted link has gone\": expected a VpcLinkId no earlier link \
         carried, got {replacement:?}"
    );
    assert_eq!(
        stored_links(&engine),
        MAX_VPC_LINKS,
        "case \"a create once the deleted link has gone\": expected the store to hold \
         {MAX_VPC_LINKS} links"
    );
    let (status, read) = call(&emulator, "GET", &format!("{VPC_LINKS}/{deleted}"), None).await;
    assert_eq!(
        status, 404,
        "case \"the deleted link after its span\": expected 404, got {status} {read}"
    );
}

#[tokio::test]
async fn every_operation_on_one_link_refuses_an_unresolved_identifier_the_same_way() {
    let (emulator, _engine) = emulator_with_store();

    // One link exists, so a miss is a miss on the identifier and not on an empty store.
    let created = create_link(
        &emulator,
        "the surviving link",
        json!({ "name": "the-one-link", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;
    let known = as_str(&created, "vpcLinkId");

    // The three operations addressed as `/v2/vpclinks/{VpcLinkId}`: the reader, the patch
    // and the delete all resolve the same identifier, so all three refuse an unresolved
    // one with the same code and the same sentence.
    for (operation, method, body) in [
        ("GetVpcLink", "GET", None),
        ("UpdateVpcLink", "PATCH", Some(json!({ "name": "renamed" }))),
        ("DeleteVpcLink", "DELETE", None),
    ] {
        for (case, vpc_link_id) in [
            ("an id no link carries", "aaaaaaaaaa".to_string()),
            ("an id that is not of the minted form", "no-such-link".to_string()),
            (
                "the stored id with its last character changed",
                format!(
                    "{}{}",
                    &known[..known.len() - 1],
                    if known.ends_with('z') { 'a' } else { 'z' }
                ),
            ),
        ] {
            let (status, answer) = call(
                &emulator,
                method,
                &format!("{VPC_LINKS}/{vpc_link_id}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {operation} with {case:?}: expected 404, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "NotFoundException",
                "case {operation} with {case:?}: expected NotFoundException, got {answer}"
            );
            // The sentence the handlers report is built by the shared helper from the
            // VPC-link kind's own spelling, so it is asserted to BE the literal above and
            // cannot drift from it.
            assert_eq!(
                invalid_identifier_message(VPC_LINK_RESOURCE),
                UNRESOLVED_IDENTIFIER_SENTENCE,
                "case {operation} with {case:?}: expected the service's sentence, got {:?}",
                invalid_identifier_message(VPC_LINK_RESOURCE)
            );
            assert_eq!(
                as_str(&answer, "message"),
                UNRESOLVED_IDENTIFIER_SENTENCE,
                "case {operation} with {case:?}: expected the sentence API Gateway answers an \
                 unresolved VPC link identifier with, got {answer}"
            );
        }
    }
}
