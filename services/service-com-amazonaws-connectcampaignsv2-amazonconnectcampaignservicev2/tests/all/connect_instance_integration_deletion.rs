// SPDX-License-Identifier: CC0-1.0

//! Deleting one integration of a Connect instance, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rules under test:
//!
//! * `DeleteConnectInstanceIntegration` removes the integration stored under the
//!   identity its `integrationIdentifier` carries — the kind and the ARN of the
//!   resource it integrates, the same identity `PutConnectInstanceIntegration` stores
//!   under — so `ListConnectInstanceIntegrations` stops carrying exactly that record
//!   and carries every other one the instance holds, in the order they were first put;
//! * an identity the instance holds no integration under names no resource, so the
//!   delete is HTTP 404 `ResourceNotFoundException` and removes nothing — including a
//!   second delete of an identity already removed, and a delete against an instance
//!   whose onboarding job has not finished;
//! * an integration belongs to the instance it was put under: deleting it from one
//!   instance leaves another instance's integration of the same identity in place.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};
use super::connect_instance_integrations::{
    integration_configs, list_integrations, object_type_names, onboard, put_integration, summaries,
};

/// The region every case here signs for; nothing in an integration record is
/// region-dependent — the ARNs it holds are the caller's — so one is enough.
const REGION: &str = "us-east-1";

/// An integration of a kind no case under test puts or deletes, used to show that a
/// delete removes the record its identifier names and nothing else.
fn sentinel_config() -> Value {
    json!({"lambda": {
        "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:sentinel-untouched",
    }})
}

/// The `IntegrationIdentifier` that names the integration `config` configures: the
/// same union member, carrying only the ARN of the integrated resource.
///
/// The members come from the model: `CustomerProfilesIntegrationIdentifier$domainArn`,
/// `QConnectIntegrationIdentifier$knowledgeBaseArn` and
/// `LambdaIntegrationIdentifier$functionArn`
/// (`smithy_query(query="shape", shape="IntegrationIdentifier")`).
fn identifier_for(config: &Value) -> Value {
    for (member, arn_member) in [
        ("customerProfiles", "domainArn"),
        ("qConnect", "knowledgeBaseArn"),
        ("lambda", "functionArn"),
    ] {
        if let Some(inner) = config.get(member) {
            let arn = inner
                .get(arn_member)
                .unwrap_or_else(|| panic!("config {config} carries no {arn_member}"));
            return json!({member: {arn_member: arn}});
        }
    }
    panic!("config {config} is none of the three integration kinds")
}

/// `DeleteConnectInstanceIntegration` for `instance_id` with `identifier`.
async fn delete_integration(
    router: &MultiServiceRouter,
    instance_id: &str,
    identifier: &Value,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/connect-instance/{instance_id}/integrations/delete"),
        &json!({"integrationIdentifier": identifier}),
    )
    .await
}

/// The integrations `instance_id` holds, read through the list.
async fn held(router: &MultiServiceRouter, instance_id: &str, case: &str) -> Vec<Value> {
    let (status, body) = list_integrations(router, instance_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the list, got {status} {body}"
    );
    summaries(&body)
}

#[tokio::test]
async fn every_integration_kind_is_deleted_by_its_identifier() {
    // One row per `IntegrationConfig` form the put suite spans: the three kinds the
    // union declares, with no object types, one and every value in `EventType`'s
    // catalog, ARNs in two partitions, and a Lambda ARN qualified with an alias.
    for (case, config) in integration_configs() {
        let router = router();
        let instance_id = "integration-deletion";
        onboard(&router, instance_id).await;

        let sentinel = sentinel_config();
        for seed in [&config, &sentinel] {
            let (status, body) = put_integration(&router, instance_id, seed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 seeding {seed}, got {status} {body}"
            );
        }

        let identifier = identifier_for(&config);
        let (status, body) = delete_integration(&router, instance_id, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} {body}"
        );

        // The store is the source of truth: the record the identifier named is gone and
        // the one beside it is untouched.
        let remaining = held(&router, instance_id, case).await;
        assert_eq!(
            remaining,
            vec![sentinel.clone()],
            "case {case:?}: expected only the sentinel integration to survive the delete, \
             got {remaining:?}"
        );

        // Deleting again finds nothing to delete: the identity is no longer held.
        let (status, body) = delete_integration(&router, instance_id, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from a repeated delete, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );

        // And the identity is free again: putting it back stores one record, so the
        // delete removed the record rather than marking it.
        let (status, body) = put_integration(&router, instance_id, &config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 re-putting the deleted integration, \
             got {status} {body}"
        );
        let restored = held(&router, instance_id, case).await;
        assert_eq!(
            restored,
            vec![sentinel.clone(), config.clone()],
            "case {case:?}: expected the re-put integration beside the sentinel, got {restored:?}"
        );
    }
}

#[tokio::test]
async fn an_identity_the_instance_holds_nothing_under_is_not_found() {
    let router = router();
    let instance_id = "integration-deletion-unheld";
    onboard(&router, instance_id).await;

    let domain = "arn:aws:profile:us-east-1:000000000000:domains/held-domain";
    let function = "arn:aws:lambda:us-east-1:000000000000:function:held-handler";
    let profiles = json!({"customerProfiles": {
        "domainArn": domain,
        "objectTypeNames": object_type_names(1),
    }});
    let lambda = json!({"lambda": {"functionArn": function}});
    for seed in [&profiles, &lambda] {
        let (status, body) = put_integration(&router, instance_id, seed).await;
        assert_eq!(
            status, 200,
            "case \"seed {seed}\": expected HTTP 200 from the put, got {status} {body}"
        );
    }
    let expected = vec![profiles.clone(), lambda.clone()];

    // Each row names an identity the instance does NOT hold an integration under: the
    // kind it holds under another ARN, another kind under an ARN it does hold, the held
    // ARN altered in the ways an ARN differs (a suffix, a partition, the case of its
    // resource), and a kind it holds nothing of at all.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the held kind under another ARN",
            json!({"customerProfiles": {
                "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/other-domain",
            }}),
        ),
        (
            "another kind under a held ARN",
            json!({"qConnect": {"knowledgeBaseArn": domain}}),
        ),
        (
            "the held ARN with a suffix",
            json!({"customerProfiles": {"domainArn": format!("{domain}-copy")}}),
        ),
        (
            "the held ARN in another partition",
            json!({"lambda": {
                "functionArn": function.replace("arn:aws:", "arn:aws-cn:"),
            }}),
        ),
        (
            "the held ARN with its resource name in another case",
            json!({"lambda": {"functionArn": function.replace("held-handler", "HELD-HANDLER")}}),
        ),
        (
            "a kind the instance holds nothing of",
            json!({"qConnect": {
                "knowledgeBaseArn":
                    "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/11111111-2222-3333-4444-555555555555",
            }}),
        ),
    ];

    for (case, identifier) in cases {
        let (status, body) = delete_integration(&router, instance_id, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an identity the instance holds nothing \
             under, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );

        // A refusal removes nothing.
        let remaining = held(&router, instance_id, case).await;
        assert_eq!(
            remaining, expected,
            "case {case:?}: a refused delete must remove nothing, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn deleting_one_integration_leaves_the_others_in_put_order() {
    let router = router();
    let instance_id = "integration-deletion-order";
    onboard(&router, instance_id).await;

    // Five Lambda integrations, each a distinct function ARN, so the stored order is
    // the order they were put in.
    let configs: Vec<Value> = (0..5)
        .map(|n| {
            json!({"lambda": {
                "functionArn":
                    format!("arn:aws:lambda:us-east-1:000000000000:function:order-handler-{n}"),
            }})
        })
        .collect();
    for config in &configs {
        let (status, body) = put_integration(&router, instance_id, config).await;
        assert_eq!(
            status, 200,
            "case \"seed {config}\": expected HTTP 200 from the put, got {status} {body}"
        );
    }

    // Each row removes one record from a different position — the middle, the first,
    // the last — and names the whole set the instance holds afterwards, in put order.
    let cases: Vec<(&str, usize, Vec<usize>)> = vec![
        ("delete from the middle", 2, vec![0, 1, 3, 4]),
        ("delete the first", 0, vec![1, 3, 4]),
        ("delete the last", 4, vec![1, 3]),
    ];

    for (case, target, survivors) in cases {
        let (status, body) =
            delete_integration(&router, instance_id, &identifier_for(&configs[target])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} {body}"
        );

        let expected: Vec<Value> = survivors.iter().map(|n| configs[*n].clone()).collect();
        let remaining = held(&router, instance_id, case).await;
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the surviving integrations in put order, got {remaining:?}"
        );

        // The same set reads back a page at a time: a delete renumbers nothing, so the
        // cursors the list hands out still walk the order.
        let mut paged: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                None => "?maxResults=1".to_string(),
                Some(token) => format!("?maxResults=1&nextToken={token}"),
            };
            let (status, body) = list_integrations(&router, instance_id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the paged list, got {status} {body}"
            );
            paged.extend(summaries(&body));
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            paged, expected,
            "case {case:?}: expected paging to read back the surviving integrations in put \
             order, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn only_an_onboarded_instance_has_an_integration_to_delete() {
    let router = router();
    let config = json!({"qConnect": {
        "knowledgeBaseArn":
            "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    }});
    let identifier = identifier_for(&config);

    // An instance whose onboarding job has only just started has not finished
    // onboarding, so it holds no integration to delete yet.
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
        let (status, body) = delete_integration(&router, case, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from a delete against an instance that is not \
             onboarded, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }

    // An onboarded instance that holds the integration can delete it, and that delete
    // reaches only that instance: another instance's integration of the same identity
    // stays as it was.
    let onboarded = "onboarded-instance";
    let other = "other-onboarded-instance";
    onboard(&router, onboarded).await;
    onboard(&router, other).await;
    for instance in [onboarded, other] {
        let (status, body) = put_integration(&router, instance, &config).await;
        assert_eq!(
            status, 200,
            "case \"put on {instance}\": expected HTTP 200, got {status} {body}"
        );
    }

    let (status, body) = delete_integration(&router, onboarded, &identifier).await;
    assert_eq!(
        status, 200,
        "case \"delete on one instance\": expected HTTP 200, got {status} {body}"
    );
    let remaining = held(&router, onboarded, "delete on one instance").await;
    assert_eq!(
        remaining,
        Vec::<Value>::new(),
        "case \"delete on one instance\": expected the instance to hold nothing, \
         got {remaining:?}"
    );
    let untouched = held(&router, other, "delete on one instance").await;
    assert_eq!(
        untouched,
        vec![config.clone()],
        "case \"delete on one instance\": one instance's delete is not another's, \
         got {untouched:?}"
    );

    // An instance that was onboarded, then had its onboarding job deleted while it is
    // still in progress, is not onboarded again: the delete there is not found.
    let in_progress = "restarted-instance";
    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{in_progress}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"start a second onboarding job\": expected HTTP 200, got {status} {body}"
    );
    let (status, body) = delete_integration(&router, in_progress, &identifier).await;
    assert_eq!(
        status, 404,
        "case \"delete while onboarding is in progress\": expected HTTP 404, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ResourceNotFoundException"),
        "case \"delete while onboarding is in progress\": expected ResourceNotFoundException, \
         got {}",
        error_code(&body)
    );

    // Once that job is due, the instance is onboarded and the same delete reports that
    // the instance simply holds no such integration — still a 404, now for the
    // integration rather than the instance.
    std::thread::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150));
    let (status, body) = delete_integration(&router, in_progress, &identifier).await;
    assert_eq!(
        status, 404,
        "case \"delete on an onboarded instance holding nothing\": expected HTTP 404, \
         got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ResourceNotFoundException"),
        "case \"delete on an onboarded instance holding nothing\": expected \
         ResourceNotFoundException, got {}",
        error_code(&body)
    );
}
