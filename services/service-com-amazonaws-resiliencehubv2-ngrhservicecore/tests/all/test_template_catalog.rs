// SPDX-License-Identifier: CC0-1.0

//! `ListTestTemplates`, end to end over HTTP: the service-owned test-template catalog.
//!
//! The rule under test: a request signed for this service's signing name and addressing
//! this service's own `/v2/…` path space is served by THIS service's restJson1 renderer —
//! HTTP 200, `Content-Type: application/json` — and `ListTestTemplates` answers the
//! `@required` `testTemplates` member as every entry of the service-owned catalog, each
//! carrying the modelled `ServiceOwnedArn` for the Region the request was signed for, its
//! name and its description. Nothing in the answer depends on prior state, and a read
//! leaves the store untouched, so two reads agree.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over — the
//! Region the request is signed for (including an unsigned and an unparseable scope, which
//! fall back to the service's default Region) — not one recorded request. One suite pins
//! the catalogue's MEMBERSHIP against the rows AWS published for this operation (name and
//! ARN resource id); the Region suite asserts the PROJECTION of each stored row, read from
//! `crate::state::TEST_TEMPLATE_CATALOG`, the single table both this listing and any later
//! reader project from.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    ARN_SERVICE_NAMESPACE, DEFAULT_REGION, SERVICE_OWNED_ACCOUNT, TEST_TEMPLATE_CATALOG,
    TEST_TEMPLATE_RESOURCE,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The operation's modelled route: `GET /v2/list-test-templates`.
const LIST_TEST_TEMPLATES: &str = "/v2/list-test-templates";

/// `ServiceOwnedArn`'s modelled minimum length (`smithy_query(query="shape",
/// shape="ServiceOwnedArn")` reports `length {min: 31}`), which every ARN this service
/// reports must satisfy.
const SERVICE_OWNED_ARN_MIN_LENGTH: usize = 31;

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests the
    // operation, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered: the status, the response media type and the body.
#[derive(Debug)]
struct Reply {
    status: u16,
    content_type: String,
    body: Value,
}

/// Dispatch `GET {target}`, signed for `scope_region` when it is `Some` and unsigned when
/// it is `None` (which is how a caller with no SigV4 credential scope reaches the
/// emulator).
async fn get(
    router: &MultiServiceRouter,
    scope_region: Option<&str>,
    target: &str,
) -> Reply {
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(format!("https://resiliencehub.us-east-1.amazonaws.com{target}"));
    if let Some(region) = scope_region {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        );
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Reply {
        status,
        content_type,
        body,
    }
}

/// The `testTemplates` array of a successful listing, asserting the framing the rule
/// requires first.
fn templates(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from the modelled success status, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.content_type, "application/json",
        "case {case:?}: expected the restJson1 media type application/json, got {:?}",
        reply.content_type
    );
    let list = reply.body.get("testTemplates").unwrap_or_else(|| {
        panic!("case {case:?}: the @required testTemplates member is absent from {:?}", reply.body)
    });
    list.as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected testTemplates to be a list, got {list:?}")
        })
        .clone()
}

/// The ARN a template of `resource_id` must carry in `region`, rebuilt here from the
/// modelled `ServiceOwnedArn` form rather than from the handler's own helper:
/// `arn:aws:<arnNamespace>:<region>:aws:test-template/<id>`.
fn expected_arn(region: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:{ARN_SERVICE_NAMESPACE}:{region}:{SERVICE_OWNED_ACCOUNT}:\
         {TEST_TEMPLATE_RESOURCE}/{resource_id}"
    )
}

#[tokio::test]
async fn list_test_templates_names_the_published_catalog_rows() {
    let router = emulator();

    // The catalogue's MEMBERSHIP, pinned against the rows AWS published for this
    // operation rather than against the table the handler reads — so a row silently
    // dropped, renamed or re-identified is observable here and not only in `state.rs`.
    // The ARNs are asserted in the Region the request is signed for, which is the
    // us-west-2 form the recorded catalogue shows.
    //
    // (case, template name, the resource id its ARN must carry).
    let published: [(&str, &str, &str); 4] = [
        (
            "Availability Zone recovery",
            "Availability Zone: recovery",
            "aws-az-recovery:rtaz001",
        ),
        (
            "dependency validation",
            "Dependency validation",
            "aws-dependency-validation:rtdep001",
        ),
        (
            "Multi-Region isolation",
            "Multi-Region: isolation",
            "aws-multi-region-isolation:rtmr001",
        ),
        (
            "Multi-Region recovery",
            "Multi-Region: recovery",
            "aws-multi-region-recovery:rtmr002",
        ),
    ];

    let reply = get(&router, Some("us-west-2"), LIST_TEST_TEMPLATES).await;
    let listed = templates("the published catalog", &reply);

    for (case, name, resource_id) in published {
        let found = listed
            .iter()
            .find(|template| template["name"].as_str() == Some(name))
            .unwrap_or_else(|| {
                panic!("case {case:?}: no template named {name:?} in {listed:?}")
            });
        assert_eq!(
            found["testTemplateArn"].as_str(),
            Some(expected_arn("us-west-2", resource_id).as_str()),
            "case {case:?}: expected the published resource id {resource_id:?}, got {:?}",
            found["testTemplateArn"]
        );
        let description = found["description"].as_str().unwrap_or_default();
        assert!(
            !description.is_empty(),
            "case {case:?}: description is @required on TestTemplateSummary, got {description:?}"
        );
    }
    assert_eq!(
        listed.len(),
        published.len(),
        "expected exactly the published catalog rows, got {listed:?}"
    );
}

#[tokio::test]
async fn list_test_templates_reports_the_catalog_for_the_signed_region() {
    let router = emulator();

    // (case, the Region the request is signed for, the Region its ARNs must name).
    //
    // The rows differ along the Region of the SigV4 credential scope — the one
    // per-request value in the answer — and span a commercial Region in each of three
    // partitions-by-shape, a GovCloud Region, a request with NO credential scope and one
    // whose scope Region is not an AWS Region at all. The last two fall back to the
    // service's default Region rather than interpolating an arbitrary header value into an
    // ARN.
    let rows: [(&str, Option<&str>, &str); 7] = [
        ("signed for us-east-1", Some("us-east-1"), "us-east-1"),
        ("signed for us-west-2", Some("us-west-2"), "us-west-2"),
        ("signed for eu-central-1", Some("eu-central-1"), "eu-central-1"),
        ("signed for ap-southeast-3", Some("ap-southeast-3"), "ap-southeast-3"),
        ("signed for us-gov-west-1", Some("us-gov-west-1"), "us-gov-west-1"),
        ("unsigned", None, DEFAULT_REGION),
        (
            "signed for a scope Region that is not one",
            Some("not-a-region"),
            DEFAULT_REGION,
        ),
    ];

    for (case, scope_region, arn_region) in rows {
        let reply = get(&router, scope_region, LIST_TEST_TEMPLATES).await;
        let listed = templates(case, &reply);

        assert_eq!(
            listed.len(),
            TEST_TEMPLATE_CATALOG.len(),
            "case {case:?}: expected every catalogued template ({}), got {listed:?}",
            TEST_TEMPLATE_CATALOG.len()
        );
        assert!(
            !listed.is_empty(),
            "case {case:?}: the catalog is service-owned and non-empty for every caller, \
             got {listed:?}"
        );

        for (entry, reported) in TEST_TEMPLATE_CATALOG.iter().zip(listed.iter()) {
            // Every member of `TestTemplateSummary` is @required, so each must be present
            // and non-empty — including the description of a template whose recorded
            // example carried none.
            let arn = reported["testTemplateArn"].as_str().unwrap_or_default();
            let name = reported["name"].as_str().unwrap_or_default();
            let description = reported["description"].as_str().unwrap_or_default();
            assert_eq!(
                arn,
                expected_arn(arn_region, entry.resource_id),
                "case {case:?}: expected a service-owned ARN in {arn_region} for \
                 {:?}, got {arn:?}",
                entry.resource_id
            );
            assert!(
                arn.len() >= SERVICE_OWNED_ARN_MIN_LENGTH,
                "case {case:?}: ServiceOwnedArn is len {SERVICE_OWNED_ARN_MIN_LENGTH}.., \
                 got {arn:?} ({} characters)",
                arn.len()
            );
            assert_eq!(
                name, entry.name,
                "case {case:?}: expected the catalogued name, got {name:?}"
            );
            assert_eq!(
                description, entry.description,
                "case {case:?}: expected the catalogued description, got {description:?}"
            );
            assert!(
                !description.is_empty(),
                "case {case:?}: description is @required on TestTemplateSummary, got {description:?}"
            );
        }

        // One listing names each template once, in name order: the catalog is a set, and
        // a client that pages or diffs it reads a stable order.
        let arns: BTreeSet<&str> = listed
            .iter()
            .map(|template| template["testTemplateArn"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            arns.len(),
            listed.len(),
            "case {case:?}: expected distinct template ARNs, got {listed:?}"
        );
        let names: Vec<&str> = listed
            .iter()
            .map(|template| template["name"].as_str().unwrap_or_default())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "case {case:?}: expected the catalog in name order, got {names:?}"
        );
    }
}

#[tokio::test]
async fn listing_the_catalog_changes_nothing_a_later_caller_sees() {
    let router = emulator();

    // Reading a service-owned catalog is an invariant: whatever a caller did before it,
    // and however often it is repeated, the answer is the same. The rows differ along what
    // happened BEFORE the two reads — nothing, an unrelated created service, and a listing
    // already taken.
    let rows: [(&str, &[&str]); 3] = [
        ("a fresh emulator", &[]),
        ("after an unrelated listing", &[LIST_TEST_TEMPLATES]),
        (
            "after two unrelated listings",
            &[LIST_TEST_TEMPLATES, LIST_TEST_TEMPLATES],
        ),
    ];

    for (case, preceding) in rows {
        for target in preceding {
            let reply = get(&router, Some("us-west-2"), target).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the preceding call to {target} failed with {} {:?}",
                reply.status, reply.body
            );
        }
        let first = get(&router, Some("us-west-2"), LIST_TEST_TEMPLATES).await;
        let second = get(&router, Some("us-west-2"), LIST_TEST_TEMPLATES).await;
        assert_eq!(
            templates(case, &first),
            templates(case, &second),
            "case {case:?}: two reads of the catalog must answer identically"
        );
    }
}
