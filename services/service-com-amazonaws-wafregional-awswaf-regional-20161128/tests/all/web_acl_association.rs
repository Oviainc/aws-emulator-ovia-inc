// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) web-ACL ASSOCIATION surface, end to end over the wire:
//! `AssociateWebACL`, `DisassociateWebACL`, `GetWebACLForResource` and `ListWebACLs`.
//!
//! The rule these tests pin: a web ACL protects exactly the resources `AssociateWebACL`
//! stored an association for, each resource is protected by AT MOST ONE web ACL (so
//! associating a resource again moves it), the only `ResourceArn`s any of these calls
//! accept are those naming an application load balancer or an Amazon API Gateway stage,
//! and a resource this service holds no association for reads back as the documented null
//! `WebACLSummary` while a removal naming one is refused as a nonexistent item.
//!
//! AWS WAF Classic is past its end of support, so no call mints a web ACL any more
//! (`web_acl_end_of_support.rs` pins that rule): the web ACLs an association can name are
//! the ones that already existed, put in the store with the `harness::store_web_acl`
//! fixture through the service's own state types.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these tests
//! assert is what an SDK client sees: each response is built from the persisted record,
//! not from the request.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_ASSOCIATED_RESOURCES, MESSAGE_NONEXISTENT_ITEM,
};

use crate::harness::store_web_acl;
use crate::wire::{call, error_message, expect_error, ok_call, router_on, DEFAULT_TEST_REGION};

/// The account id the test ARNs carry. Which account an ARN names is not what makes a
/// resource associable — the documented formats are — so one value is enough.
const ACCOUNT: &str = "123456789012";

/// An application load balancer ARN.
///
/// Source: `AssociateWebACL$ResourceArn`'s own documentation — "For an Application Load
/// Balancer:
/// `arn:aws:elasticloadbalancing:region:account-id:loadbalancer/app/load-balancer-name/load-balancer-id`".
fn alb_arn(region: &str, name: &str, id: &str) -> String {
    format!("arn:aws:elasticloadbalancing:{region}:{ACCOUNT}:loadbalancer/app/{name}/{id}")
}

/// An Amazon API Gateway stage ARN.
///
/// Source: the same member's documentation — "For an Amazon API Gateway stage:
/// `arn:aws:apigateway:region::/restapis/api-id/stages/stage-name`".
fn stage_arn(region: &str, api_id: &str, stage_name: &str) -> String {
    format!("arn:aws:apigateway:{region}::/restapis/{api_id}/stages/{stage_name}")
}

/// The `WebACLSummary` `GetWebACLForResource` reports for `resource_arn`, or the error
/// body when it refused.
async fn web_acl_for_resource(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "GetWebACLForResource",
        &json!({"ResourceArn": resource_arn}),
    )
    .await
}

/// A fresh engine, a router over it, and one stored web ACL to associate with.
async fn with_one_web_acl(
    name: &str,
) -> (
    Arc<engine::InMemoryEngine>,
    smithy_server_runtime::MultiServiceRouter,
    String,
) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let web_acl_id = store_web_acl(
        &engine,
        DEFAULT_TEST_REGION,
        name,
        "associationProbe",
        "BLOCK",
        &[],
    );
    (engine, router, web_acl_id)
}

/// An association round-trips: `GetWebACLForResource` reports the web ACL
/// `AssociateWebACL` stored for that resource, projected from the stored record.
///
/// The table spans what the rule generalises over: both protectable kinds, the region and
/// the name/id segments of a load balancer ARN, and the api-id/stage-name segments of a
/// stage ARN, including a stage name with the separators AWS allows.
#[tokio::test]
async fn an_association_round_trips_through_get_web_acl_for_resource() {
    let (engine, router, web_acl_id) = with_one_web_acl("association-reader").await;
    let name = "association-reader";
    // A second web ACL, so a reader that answered "the only web ACL there is" rather
    // than the associated one would be caught.
    store_web_acl(
        &engine,
        DEFAULT_TEST_REGION,
        "not-the-associated-one",
        "otherProbe",
        "ALLOW",
        &[],
    );

    let cases: &[(&str, String)] = &[
        (
            "an application load balancer in the signing region",
            alb_arn(DEFAULT_TEST_REGION, "edge-alb", "50dc6c495c0c9188"),
        ),
        (
            "an application load balancer in another region",
            alb_arn("ap-southeast-4", "Public_ALB-2", "0123456789abcdef"),
        ),
        (
            "an application load balancer whose name is one character",
            alb_arn("eu-west-3", "a", "1"),
        ),
        (
            "an API Gateway stage",
            stage_arn(DEFAULT_TEST_REGION, "a1b2c3d4e5", "prod"),
        ),
        (
            "an API Gateway stage whose name carries separators",
            stage_arn("eu-central-1", "zz9pluralz", "Stage-2_test"),
        ),
    ];

    for (label, resource_arn) in cases {
        let associated = ok_call(
            &router,
            "AssociateWebACL",
            &json!({"WebACLId": &web_acl_id, "ResourceArn": resource_arn}),
        )
        .await;
        assert!(
            associated.get("WebACLSummary").is_none(),
            "case {label}: AssociateWebACL models an empty response, got {associated}"
        );

        let (status, body) = web_acl_for_resource(&router, resource_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: GetWebACLForResource expected 200, got {status}: {body}"
        );
        assert_eq!(
            body["WebACLSummary"]["WebACLId"].as_str(),
            Some(web_acl_id.as_str()),
            "case {label}: expected the associated WebACLId {web_acl_id:?}, got {body}"
        );
        assert_eq!(
            body["WebACLSummary"]["Name"].as_str(),
            Some(name),
            "case {label}: expected the stored Name {name:?}, got {body}"
        );

        // A read must not change what a later caller sees.
        let (_, again) = web_acl_for_resource(&router, resource_arn).await;
        assert_eq!(
            again, body,
            "case {label}: reading twice must answer the same thing, got {body} then {again}"
        );
    }
}

/// Every one of these calls accepts only a `ResourceArn` naming an application load
/// balancer or an API Gateway stage: "Your request references an ARN that is malformed,
/// or corresponds to a resource with which a web ACL cannot be associated" is the
/// modelled `WAFInvalidParameterException`, naming the member and a modelled reason.
///
/// The rows differ along HOW the ARN fails the documented formats: the field count, the
/// service, the load-balancer family, a missing or empty segment, a segment too many, the
/// region's form, and an ARN of a WAF resource rather than of a protected one.
#[tokio::test]
async fn an_arn_naming_no_protectable_resource_is_refused_by_every_association_call() {
    let (_engine, router, web_acl_id) = with_one_web_acl("unassociable-arns").await;

    let cases: &[(&str, String)] = &[
        (
            "a network load balancer, not an application one",
            format!("arn:aws:elasticloadbalancing:{DEFAULT_TEST_REGION}:{ACCOUNT}:loadbalancer/net/edge-nlb/50dc6c495c0c9188"),
        ),
        (
            "a classic load balancer, which carries no family segment",
            format!("arn:aws:elasticloadbalancing:{DEFAULT_TEST_REGION}:{ACCOUNT}:loadbalancer/edge-clb"),
        ),
        (
            "an application load balancer with no id segment",
            format!("arn:aws:elasticloadbalancing:{DEFAULT_TEST_REGION}:{ACCOUNT}:loadbalancer/app/edge-alb"),
        ),
        (
            "an application load balancer with an empty name segment",
            alb_arn(DEFAULT_TEST_REGION, "", "50dc6c495c0c9188"),
        ),
        (
            "an application load balancer with a segment too many",
            format!("{}/extra", alb_arn(DEFAULT_TEST_REGION, "edge-alb", "50dc6c495c0c9188")),
        ),
        (
            "a target group, not a load balancer",
            format!("arn:aws:elasticloadbalancing:{DEFAULT_TEST_REGION}:{ACCOUNT}:targetgroup/app/50dc6c495c0c9188"),
        ),
        (
            "an API Gateway REST API, not one of its stages",
            format!("arn:aws:apigateway:{DEFAULT_TEST_REGION}::/restapis/a1b2c3d4e5"),
        ),
        (
            "an API Gateway stage with no stage name",
            stage_arn(DEFAULT_TEST_REGION, "a1b2c3d4e5", ""),
        ),
        (
            "an API Gateway resource that is not a stage",
            format!("arn:aws:apigateway:{DEFAULT_TEST_REGION}::/restapis/a1b2c3d4e5/deployments/dep1"),
        ),
        (
            "a WAF web ACL ARN, which is not a protected resource",
            format!("arn:aws:waf-regional:{DEFAULT_TEST_REGION}:{ACCOUNT}:webacl/{web_acl_id}"),
        ),
        (
            "an ARN of a service that has no protectable resource here",
            format!("arn:aws:s3:{DEFAULT_TEST_REGION}:{ACCOUNT}:bucket/edge-logs"),
        ),
        (
            "an empty region field",
            alb_arn("", "edge-alb", "50dc6c495c0c9188"),
        ),
        (
            "a region field that is not shaped like a region",
            alb_arn("Region One", "edge-alb", "50dc6c495c0c9188"),
        ),
        (
            "one field short of an ARN",
            format!("arn:aws:elasticloadbalancing:{ACCOUNT}:loadbalancer/app/edge-alb/50dc6c495c0c9188"),
        ),
        (
            "not an ARN at all",
            "edge-alb".to_string(),
        ),
    ];

    for (label, resource_arn) in cases {
        for operation in [
            "AssociateWebACL",
            "GetWebACLForResource",
            "DisassociateWebACL",
        ] {
            let request = if operation == "AssociateWebACL" {
                json!({"WebACLId": &web_acl_id, "ResourceArn": resource_arn})
            } else {
                json!({"ResourceArn": resource_arn})
            };
            let body = expect_error(
                &router,
                operation,
                &request,
                "WAFInvalidParameterException",
                label,
            )
            .await;
            assert_eq!(
                body["field"].as_str(),
                Some("RESOURCE_ARN"),
                "case {label}: {operation} must report the RESOURCE_ARN field, got {body}"
            );
            assert_eq!(
                body["reason"].as_str(),
                Some("ILLEGAL_ARGUMENT"),
                "case {label}: {operation} must report the ILLEGAL_ARGUMENT reason, got {body}"
            );
            assert_eq!(
                body["parameter"].as_str(),
                Some(resource_arn.as_str()),
                "case {label}: {operation} must name the rejected ARN, got {body}"
            );
        }
    }
}

/// An association needs a web ACL that exists: an id naming none is "the referenced
/// object doesn't exist", and the refused request stores nothing.
#[tokio::test]
async fn associating_with_a_web_acl_that_does_not_exist_is_refused() {
    let (engine, router, web_acl_id) = with_one_web_acl("absent-web-acl").await;
    // An id minted for a DIFFERENT resource kind is not a WebACLId either.
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({
            "Name": "not-a-web-acl",
            "ChangeToken": crate::wire::change_token(&router).await,
        }),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {ip_set}"))
        .to_string();

    let resource_arn = alb_arn(DEFAULT_TEST_REGION, "absent-probe", "50dc6c495c0c9188");
    for (label, candidate) in [
        (
            "a UUID this service never minted",
            "11111111-2222-4333-8444-555555555555",
        ),
        ("an id of another resource kind", ip_set_id.as_str()),
        ("a value that is not an id at all", "no-such-web-acl"),
        (
            "an id that differs from a stored one in its last character",
            &format!("{}x", &web_acl_id[..web_acl_id.len() - 1]),
        ),
    ] {
        let body = expect_error(
            &router,
            "AssociateWebACL",
            &json!({"WebACLId": candidate, "ResourceArn": &resource_arn}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: expected the nonexistent-item sentence, got {body}"
        );

        // Nothing was stored: the resource is still protected by no web ACL, which the
        // documented null `WebACLSummary` on a 200 is how the reader says so, and no
        // stored record gained an association.
        let (status, after) = web_acl_for_resource(&router, &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: GetWebACLForResource expected 200, got {status}: {after}"
        );
        assert!(
            after["WebACLSummary"].is_null(),
            "case {label}: a refused association must leave the resource unprotected, \
             got {after}"
        );
        let stored = stored_resources(&engine, &web_acl_id);
        assert!(
            stored.is_empty(),
            "case {label}: a refused association must store nothing, got {stored:?}"
        );
    }
}

/// The ARNs the stored web ACL `web_acl_id` protects, read out of the service's own
/// state: what a response claims is checked against the record behind it.
fn stored_resources(engine: &engine::InMemoryEngine, web_acl_id: &str) -> Vec<String> {
    let state = service_com_amazonaws_wafregional_awswaf_regional_20161128::state::service_state(
        engine,
    )
    .expect("the service state is reachable from the engine");
    let state = state.lock();
    state
        .web_acls
        .get(web_acl_id)
        .map(|entry| entry.associated_resources.iter().cloned().collect())
        .unwrap_or_default()
}

/// A resource is protected by at most ONE web ACL: associating it again moves it to the
/// new web ACL and leaves the old one no longer protecting it, and associating it with
/// the web ACL it already has changes nothing.
#[tokio::test]
async fn associating_a_resource_again_moves_it_to_the_new_web_acl() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let first = store_web_acl(&engine, DEFAULT_TEST_REGION, "first-acl", "firstAcl", "ALLOW", &[]);
    let second = store_web_acl(&engine, DEFAULT_TEST_REGION, "second-acl", "secondAcl", "BLOCK", &[]);

    // Rows differing in the protectable kind, so the move is pinned for both.
    for (label, resource_arn) in [
        (
            "an application load balancer",
            alb_arn(DEFAULT_TEST_REGION, "moving-alb", "50dc6c495c0c9188"),
        ),
        (
            "an API Gateway stage",
            stage_arn(DEFAULT_TEST_REGION, "a1b2c3d4e5", "beta"),
        ),
    ] {
        for (step, owner) in [
            ("the first association", &first),
            ("the same association again", &first),
            ("a move to the other web ACL", &second),
            ("a move back", &first),
        ] {
            ok_call(
                &router,
                "AssociateWebACL",
                &json!({"WebACLId": owner, "ResourceArn": &resource_arn}),
            )
            .await;

            let (status, body) = web_acl_for_resource(&router, &resource_arn).await;
            assert_eq!(
                status, 200,
                "case {label}/{step}: GetWebACLForResource expected 200, got {status}: {body}"
            );
            assert_eq!(
                body["WebACLSummary"]["WebACLId"].as_str(),
                Some(owner.as_str()),
                "case {label}/{step}: expected the protecting WebACLId {owner:?}, got {body}"
            );

            // Exactly one stored record holds the ARN: the other one does not.
            let other = if owner == &first { &second } else { &first };
            assert!(
                stored_resources(&engine, owner).contains(&resource_arn),
                "case {label}/{step}: the new web ACL must hold the ARN, got {:?}",
                stored_resources(&engine, owner)
            );
            assert!(
                !stored_resources(&engine, other).contains(&resource_arn),
                "case {label}/{step}: a resource is protected by at most one web ACL, \
                 got {:?}",
                stored_resources(&engine, other)
            );
        }
    }
}

/// `DisassociateWebACL` removes the association; afterwards the reader reports the
/// documented null `WebACLSummary` on a 200 and a second removal has nothing to remove.
#[tokio::test]
async fn disassociating_removes_the_association_and_leaves_nothing_to_remove() {
    let (engine, router, web_acl_id) = with_one_web_acl("disassociating").await;

    for (label, resource_arn) in [
        (
            "an application load balancer",
            alb_arn(DEFAULT_TEST_REGION, "leaving-alb", "50dc6c495c0c9188"),
        ),
        (
            "an API Gateway stage",
            stage_arn("eu-west-3", "a1b2c3d4e5", "prod"),
        ),
    ] {
        // Before any association: "If there is no associated resource, a null
        // WebACLSummary is returned" — a 200 carrying no summary, not a refusal.
        let (status, body) = web_acl_for_resource(&router, &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: GetWebACLForResource expected 200, got {status}: {body}"
        );
        assert!(
            body["WebACLSummary"].is_null(),
            "case {label}: an unprotected resource must answer a null WebACLSummary, \
             got {body}"
        );

        ok_call(
            &router,
            "AssociateWebACL",
            &json!({"WebACLId": &web_acl_id, "ResourceArn": &resource_arn}),
        )
        .await;
        let removed = ok_call(
            &router,
            "DisassociateWebACL",
            &json!({"ResourceArn": &resource_arn}),
        )
        .await;
        assert!(
            removed.as_object().is_some_and(serde_json::Map::is_empty),
            "case {label}: DisassociateWebACL models an empty response, got {removed}"
        );
        assert!(
            !stored_resources(&engine, &web_acl_id).contains(&resource_arn),
            "case {label}: the removal must drop the ARN off the record, got {:?}",
            stored_resources(&engine, &web_acl_id)
        );

        // With the association gone, each call answers as it did before it was made: the
        // reader with the documented null summary, and a removal with nothing to remove.
        let (status, after) = web_acl_for_resource(&router, &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: GetWebACLForResource expected 200, got {status}: {after}"
        );
        assert!(
            after["WebACLSummary"].is_null(),
            "case {label}: a removed association must leave no summary, got {after}"
        );
        let body = expect_error(
            &router,
            "DisassociateWebACL",
            &json!({"ResourceArn": &resource_arn}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: expected the nonexistent-item sentence, got {body}"
        );
    }
}

/// One web ACL protects at most `MAX_ASSOCIATED_RESOURCES` resources: the bound itself is
/// accepted, one past it is refused, re-associating a resource it already protects is
/// still accepted at the bound, and another web ACL has its own budget.
#[tokio::test]
async fn one_web_acl_protects_at_most_the_bounded_number_of_resources() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let filled = store_web_acl(&engine, DEFAULT_TEST_REGION, "filled-acl", "filledAcl", "ALLOW", &[]);
    let spare = store_web_acl(&engine, DEFAULT_TEST_REGION, "spare-acl", "spareAcl", "BLOCK", &[]);

    let arn_at = |index: usize| alb_arn(DEFAULT_TEST_REGION, &format!("alb-{index}"), "50dc6c495c0c9188");
    for index in 0..MAX_ASSOCIATED_RESOURCES {
        ok_call(
            &router,
            "AssociateWebACL",
            &json!({"WebACLId": &filled, "ResourceArn": arn_at(index)}),
        )
        .await;
    }
    assert_eq!(
        stored_resources(&engine, &filled).len(),
        MAX_ASSOCIATED_RESOURCES,
        "case at the bound: the web ACL must protect every resource up to the bound, got {:?}",
        stored_resources(&engine, &filled).len()
    );

    // One past the bound.
    let past = arn_at(MAX_ASSOCIATED_RESOURCES);
    let body = expect_error(
        &router,
        "AssociateWebACL",
        &json!({"WebACLId": &filled, "ResourceArn": &past}),
        "WAFInvalidParameterException",
        "one past the bound",
    )
    .await;
    assert_eq!(
        body["field"].as_str(),
        Some("RESOURCE_ARN"),
        "case one past the bound: expected the RESOURCE_ARN field, got {body}"
    );
    assert!(
        !stored_resources(&engine, &filled).contains(&past),
        "case one past the bound: the refused ARN must not be stored, got {:?}",
        stored_resources(&engine, &filled).len()
    );

    // A resource the full web ACL already protects is still accepted: it adds nothing.
    ok_call(
        &router,
        "AssociateWebACL",
        &json!({"WebACLId": &filled, "ResourceArn": arn_at(0)}),
    )
    .await;
    assert_eq!(
        stored_resources(&engine, &filled).len(),
        MAX_ASSOCIATED_RESOURCES,
        "case re-associating at the bound: the association set must not grow, got {:?}",
        stored_resources(&engine, &filled).len()
    );

    // The bound is per web ACL: another one still takes the ARN the full one refused.
    ok_call(
        &router,
        "AssociateWebACL",
        &json!({"WebACLId": &spare, "ResourceArn": &past}),
    )
    .await;
    let (status, body) = web_acl_for_resource(&router, &past).await;
    assert_eq!(
        status, 200,
        "case another web ACL's budget: GetWebACLForResource expected 200, got {status}: {body}"
    );
    assert_eq!(
        body["WebACLSummary"]["WebACLId"].as_str(),
        Some(spare.as_str()),
        "case another web ACL's budget: expected WebACLId {spare:?}, got {body}"
    );
}

/// `ListWebACLs` reports the stored web ACLs as `WebACLSummary` objects, one page at a
/// time: `Limit` bounds the page and `NextMarker` resumes it.
#[tokio::test]
async fn list_web_acls_pages_through_the_stored_web_acls() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    let mut expected: Vec<(String, String)> = Vec::new();
    for index in 0..3 {
        let name = format!("listed-acl-{index}");
        let id = store_web_acl(&engine, DEFAULT_TEST_REGION, &name, "listedAcl", "ALLOW", &[]);
        expected.push((id, name));
    }
    expected.sort();

    // The whole listing: `Limit` 0 (the model's declared default) asks for the service's
    // own page size.
    let body = ok_call(&router, "ListWebACLs", &json!({"Limit": 0})).await;
    let listed: Vec<(String, String)> = summaries(&body);
    assert_eq!(
        listed, expected,
        "case one page: expected every stored web ACL as a summary, got {body}"
    );

    // Two pages of one, then the rest.
    for (label, limit, page_size) in [("a page of one", 1, 1), ("a page of two", 2, 2)] {
        let first = ok_call(&router, "ListWebACLs", &json!({"Limit": limit})).await;
        let page = summaries(&first);
        assert_eq!(
            page.len(),
            page_size,
            "case {label}: expected {page_size} summaries, got {first}"
        );
        assert_eq!(
            page,
            expected[..page_size],
            "case {label}: expected the first {page_size} stored web ACLs, got {first}"
        );
        let marker = first["NextMarker"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: a page with more behind it must carry a NextMarker: {first}"))
            .to_string();

        let rest = ok_call(
            &router,
            "ListWebACLs",
            &json!({"Limit": 0, "NextMarker": marker}),
        )
        .await;
        assert_eq!(
            summaries(&rest),
            expected[page_size..],
            "case {label}: the resumed page must hold the rest, got {rest}"
        );
        assert!(
            rest["NextMarker"].is_null(),
            "case {label}: a complete listing has no next page, got {rest}"
        );
    }
}

/// The `(WebACLId, Name)` pairs a `ListWebACLs` response carries, in the order it
/// reported them.
fn summaries(body: &Value) -> Vec<(String, String)> {
    body["WebACLs"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWebACLs returned no WebACLs array: {body}"))
        .iter()
        .map(|summary| {
            (
                summary["WebACLId"].as_str().unwrap_or_default().to_string(),
                summary["Name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}
