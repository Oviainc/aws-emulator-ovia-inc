// SPDX-License-Identifier: CC0-1.0

//! The integrations of a Connect instance, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rules under test:
//!
//! * `PutConnectInstanceIntegration` PERSISTS the integration config it is given for
//!   the named instance, whichever of the three kinds `IntegrationConfig` declares it
//!   is, and `ListConnectInstanceIntegrations` reports exactly the stored set — one
//!   summary per record, carrying the values that record holds;
//! * an integration is identified by the pair its `IntegrationIdentifier` member
//!   carries (its kind and the ARN of the resource it integrates), so a put under a
//!   pair the instance already holds UPDATES that record in place and a put under a
//!   new pair adds one beside it;
//! * the list pages in put order: any page size reads back the same sequence, a
//!   `nextToken` resumes after the record the previous page ended at, and the last
//!   page carries none;
//! * only an instance whose onboarding job has finished has integrations at all: any
//!   other instance id is HTTP 404 `ResourceNotFoundException` on both operations and
//!   a put against it stores nothing, while an onboarded instance that holds no
//!   integration is an empty list;
//! * listing only reports: repeated reads agree, and one instance's integrations are
//!   never another's;
//! * the store is bounded: past the ceiling a further put is refused with HTTP 409
//!   `ConflictException` and the integrations already stored are untouched.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::EVENT_TYPE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    MAX_INTEGRATIONS_PER_INSTANCE, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// The region every case here signs for; nothing in an integration record is
/// region-dependent — the ARNs it holds are the caller's — so one is enough.
const REGION: &str = "us-east-1";

/// An `objectTypeNames` map over the first `count` values of `EventType`'s catalog
/// (`EVENT_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="EventType")`), each mapped to an
/// `ObjectTypeName`. `count` of 0 is the empty map, which the shape admits: the
/// member is required but carries no length bound.
pub fn object_type_names(count: usize) -> Value {
    Value::Object(
        EVENT_TYPE_VALUES
            .iter()
            .take(count)
            .map(|event| ((*event).to_string(), json!(format!("{event}-Object"))))
            .collect(),
    )
}

/// One `IntegrationConfig` per row, spanning the three kinds the union declares
/// (`customerProfiles`, `qConnect`, `lambda`, from
/// `smithy_query(query="shape", shape="IntegrationConfig")`) and the forms their
/// ARNs and object-type maps take: no object types, one, and every value in
/// `EventType`'s catalog; a commercial-partition ARN and a `aws-cn` one; a bare
/// Lambda function ARN and one qualified with an alias.
pub fn integration_configs() -> Vec<(&'static str, Value)> {
    vec![
        (
            "customerProfiles with no object types",
            json!({"customerProfiles": {
                "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/campaigns-domain",
                "objectTypeNames": object_type_names(0),
            }}),
        ),
        (
            "customerProfiles with one object type",
            json!({"customerProfiles": {
                "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/one-object-type",
                "objectTypeNames": object_type_names(1),
            }}),
        ),
        (
            "customerProfiles with every event type in the catalog",
            json!({"customerProfiles": {
                "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/every-object-type",
                "objectTypeNames": object_type_names(EVENT_TYPE_VALUES.len()),
            }}),
        ),
        (
            "qConnect knowledge base",
            json!({"qConnect": {
                "knowledgeBaseArn":
                    "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/11111111-2222-3333-4444-555555555555",
            }}),
        ),
        (
            "qConnect knowledge base in another partition",
            json!({"qConnect": {
                "knowledgeBaseArn":
                    "arn:aws-cn:wisdom:cn-north-1:000000000000:knowledge-base/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            }}),
        ),
        (
            "lambda function",
            json!({"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:campaign-handler",
            }}),
        ),
        (
            "lambda function qualified with an alias",
            json!({"lambda": {
                "functionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:campaign-handler:PROD",
            }}),
        ),
    ]
}

/// `StartInstanceOnboardingJob` for `instance_id`, then let the clock reach the
/// instant the job is due at: an instance is a target for integrations only once its
/// onboarding job has finished. Nothing in the emulator sleeps for the transition —
/// the record stores the instant — so a case only has to let the clock pass it.
pub async fn onboard(router: &MultiServiceRouter, instance_id: &str) {
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance_id}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance_id:?}: onboarding the instance under test: expected HTTP 200, \
         got {status} {body}"
    );
    std::thread::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150));
}

/// `PutConnectInstanceIntegration` for `instance_id` with `config`.
pub async fn put_integration(
    router: &MultiServiceRouter,
    instance_id: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance_id}/integrations"),
        &json!({"integrationConfig": config}),
    )
    .await
}

/// `ListConnectInstanceIntegrations` for `instance_id`, with `query` appended to the
/// URI (`""` for an unpaged call).
pub async fn list_integrations(
    router: &MultiServiceRouter,
    instance_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance_id}/integrations{query}"),
        &Value::Null,
    )
    .await
}

/// The `integrationSummaryList` a success body carries.
pub fn summaries(body: &Value) -> Vec<Value> {
    body.get("integrationSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every integration of `instance_id`, read by paging the list with `page_size` and
/// following each `nextToken` it hands out, in page order.
async fn page_through(
    router: &MultiServiceRouter,
    instance_id: &str,
    page_size: Option<usize>,
    case: &str,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(
            pages <= 1_000,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={token}"));
        }
        let (status, body) = list_integrations(router, instance_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?} page {pages}: expected HTTP 200, got {status} {body}"
        );
        let page = summaries(&body);
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?} page {pages}: expected at most {size} summaries, got {}",
                page.len()
            );
        }
        collected.extend(page);
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    collected
}

#[tokio::test]
async fn every_integration_kind_round_trips_through_the_list() {
    for (case, config) in integration_configs() {
        let router = router();
        let instance_id = "integration-round-trip";
        onboard(&router, instance_id).await;

        let (status, body) = put_integration(&router, instance_id, &config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from PutConnectInstanceIntegration, \
             got {status} {body}"
        );

        // The store is the source of truth, and the summary of an integration carries
        // the same members its config does, so the one record read back is the config
        // that was put.
        let (status, body) = list_integrations(&router, instance_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListConnectInstanceIntegrations, \
             got {status} {body}"
        );
        assert_eq!(
            summaries(&body),
            vec![config.clone()],
            "case {case:?}: expected the put config to be the one stored integration, got {body}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a single-page list carries no nextToken, got {body}"
        );

        // Listing only reports: a second read of the same store agrees with the first.
        let (status, second) = list_integrations(&router, instance_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the repeated list, got {status} {second}"
        );
        assert_eq!(
            summaries(&second),
            summaries(&body),
            "case {case:?}: reading the list must not change it, got {second}"
        );
    }
}

#[tokio::test]
async fn a_put_updates_the_integration_of_the_same_identity_and_adds_any_other() {
    let router = router();
    let instance_id = "integration-identity";
    onboard(&router, instance_id).await;

    let domain = "arn:aws:profile:us-east-1:000000000000:domains/first-domain";
    let other_domain = "arn:aws:profile:us-east-1:000000000000:domains/second-domain";
    let lambda = "arn:aws:lambda:us-east-1:000000000000:function:campaign-handler";

    // Each row: the config to put, and the whole set the instance holds afterwards —
    // in the order the records were FIRST put, which an update does not disturb.
    let first = json!({"customerProfiles": {
        "domainArn": domain,
        "objectTypeNames": object_type_names(1),
    }});
    let updated = json!({"customerProfiles": {
        "domainArn": domain,
        "objectTypeNames": object_type_names(EVENT_TYPE_VALUES.len()),
    }});
    let second_domain = json!({"customerProfiles": {
        "domainArn": other_domain,
        "objectTypeNames": object_type_names(2),
    }});
    let lambda_config = json!({"lambda": {"functionArn": lambda}});
    let cases: Vec<(&str, &Value, Vec<&Value>)> = vec![
        ("the first put of a kind", &first, vec![&first]),
        (
            "the same kind and ARN again, with new object types",
            &updated,
            vec![&updated],
        ),
        (
            "the same kind under another ARN",
            &second_domain,
            vec![&updated, &second_domain],
        ),
        (
            "repeating a put changes nothing (the operation is idempotent)",
            &second_domain,
            vec![&updated, &second_domain],
        ),
        (
            "another kind",
            &lambda_config,
            vec![&updated, &second_domain, &lambda_config],
        ),
    ];

    for (case, config, expected) in cases {
        let (status, body) = put_integration(&router, instance_id, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the put, got {status} {body}"
        );

        let (status, body) = list_integrations(&router, instance_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the list, got {status} {body}"
        );
        let expected: Vec<Value> = expected.into_iter().cloned().collect();
        assert_eq!(
            summaries(&body),
            expected,
            "case {case:?}: expected the instance to hold exactly {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn the_list_reads_back_the_put_order_at_every_page_size() {
    let router = router();
    let instance_id = "integration-paging";
    onboard(&router, instance_id).await;

    // Seven lambda integrations, each a distinct function ARN, so the stored order is
    // the order they were put in.
    let expected: Vec<Value> = (0..7)
        .map(|n| {
            json!({"lambda": {
                "functionArn":
                    format!("arn:aws:lambda:us-east-1:000000000000:function:handler-{n}"),
            }})
        })
        .collect();
    for config in &expected {
        let (status, body) = put_integration(&router, instance_id, config).await;
        assert_eq!(
            status, 200,
            "case \"seed {config}\": expected HTTP 200 from the put, got {status} {body}"
        );
    }

    // Each page size spans the sequence differently — one record a page, an exact
    // divisor, the whole list in one page, the model's `maxResults` maximum (range
    // 1..50) and no `maxResults` at all — and every one reads back the same order.
    for page_size in [Some(1), Some(2), Some(3), Some(7), Some(50), None] {
        let case = format!("maxResults {page_size:?}");
        let collected = page_through(&router, instance_id, page_size, &case).await;
        assert_eq!(
            collected, expected,
            "case {case:?}: expected paging to read back every integration in put order, \
             got {collected:?}"
        );
    }

    // A first page smaller than the list hands out a cursor, and the page after it
    // resumes where that one ended rather than starting over.
    let (status, body) = list_integrations(&router, instance_id, "?maxResults=3").await;
    assert_eq!(
        status, 200,
        "case \"first page of three\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        expected[..3].to_vec(),
        "case \"first page of three\": expected the first three in put order, got {body}"
    );
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"first page of three\": expected a cursor, got {body}"))
        .to_string();
    let (status, body) =
        list_integrations(&router, instance_id, &format!("?maxResults=3&nextToken={token}")).await;
    assert_eq!(
        status, 200,
        "case \"second page of three\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        expected[3..6].to_vec(),
        "case \"second page of three\": expected the next three in put order, got {body}"
    );

    // A cursor this service did not issue is a bad request, not an empty page.
    for case in ["not-a-token", "zz", "6e6f7420686578"] {
        let (status, body) =
            list_integrations(&router, instance_id, &format!("?nextToken={case}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a cursor this service did not issue, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn a_put_past_the_ceiling_is_refused_and_changes_nothing() {
    let router = router();
    let instance_id = "integration-ceiling";
    onboard(&router, instance_id).await;

    // One distinct Lambda function per integration, up to the ceiling the store
    // enforces.
    let function_arn = |n: usize| {
        format!("arn:aws:lambda:us-east-1:000000000000:function:ceiling-handler-{n}")
    };
    for n in 0..MAX_INTEGRATIONS_PER_INSTANCE {
        let config = json!({"lambda": {"functionArn": function_arn(n)}});
        let (status, body) = put_integration(&router, instance_id, &config).await;
        assert_eq!(
            status, 200,
            "case \"integration {n} of {MAX_INTEGRATIONS_PER_INSTANCE}\": expected HTTP 200, \
             got {status} {body}"
        );
    }

    let refused = json!({"lambda": {"functionArn": function_arn(MAX_INTEGRATIONS_PER_INSTANCE)}});
    let (status, body) = put_integration(&router, instance_id, &refused).await;
    assert_eq!(
        status, 409,
        "case \"one past the ceiling\": expected HTTP 409, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case \"one past the ceiling\": expected ConflictException, got {}",
        error_code(&body)
    );

    // The refusal stored nothing: the instance still holds exactly what it held, and
    // not the integration that was refused.
    let held = page_through(&router, instance_id, Some(50), "one past the ceiling").await;
    assert_eq!(
        held.len(),
        MAX_INTEGRATIONS_PER_INSTANCE,
        "case \"one past the ceiling\": expected the stored integrations to be untouched, \
         got {} of them",
        held.len()
    );
    assert!(
        !held.contains(&refused),
        "case \"one past the ceiling\": a refused put must store nothing, got {held:?}"
    );

    // A put under an identity the instance already holds is an update, so the ceiling
    // does not block it.
    let update = json!({"lambda": {"functionArn": function_arn(0)}});
    let (status, body) = put_integration(&router, instance_id, &update).await;
    assert_eq!(
        status, 200,
        "case \"update at the ceiling\": expected HTTP 200, got {status} {body}"
    );
}

#[tokio::test]
async fn only_an_onboarded_instance_has_integrations() {
    let router = router();
    let onboarded = "onboarded-instance";
    let config = json!({"qConnect": {
        "knowledgeBaseArn":
            "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/11111111-2222-3333-4444-555555555555",
    }});

    // An instance whose onboarding job has only just started has not finished
    // onboarding, so it is not a target for integrations yet.
    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        "/v2/connect-instance/in-progress-instance/onboarding",
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"start an onboarding job\": expected HTTP 200, got {status} {body}"
    );

    for case in [
        "never-onboarded",
        "in-progress-instance",
        "onboarded-instance-with-a-suffix",
        "x",
    ] {
        let (status, body) = put_integration(&router, case, &config).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from a put against an instance that is not \
             onboarded, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );

        let (status, body) = list_integrations(&router, case, "").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from a list of an instance that is not \
             onboarded, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }

    // An onboarded instance that holds no integration is an empty list, which is a
    // different answer from "no such instance".
    onboard(&router, onboarded).await;
    let (status, body) = list_integrations(&router, onboarded, "").await;
    assert_eq!(
        status, 200,
        "case \"onboarded with no integration\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        Vec::<Value>::new(),
        "case \"onboarded with no integration\": expected an empty list, got {body}"
    );

    // And an integration belongs to the instance it was put under: another onboarded
    // instance holds none of it.
    let other = "other-onboarded-instance";
    onboard(&router, other).await;
    let (status, body) = put_integration(&router, onboarded, &config).await;
    assert_eq!(
        status, 200,
        "case \"put on one instance\": expected HTTP 200, got {status} {body}"
    );
    let (status, body) = list_integrations(&router, other, "").await;
    assert_eq!(
        status, 200,
        "case \"list the other instance\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        Vec::<Value>::new(),
        "case \"list the other instance\": one instance's integration is not another's, \
         got {body}"
    );
    let (status, body) = list_integrations(&router, onboarded, "").await;
    assert_eq!(
        status, 200,
        "case \"list the instance put on\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        vec![config.clone()],
        "case \"list the instance put on\": expected the integration that was put, got {body}"
    );
}
