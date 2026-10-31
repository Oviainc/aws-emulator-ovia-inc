// SPDX-License-Identifier: CC0-1.0

//! `ListSchemas` over the wire, as a table rather than a single call.
//!
//! RULE: a `List*` of this service REPORTS what the account holds — it answers HTTP 200
//! with the summary of every resource of its kind, writes nothing, and never refuses
//! because the account holds none. `maxResults` bounds ONE page, the `nextToken` a page
//! hands back resumes after the item that page ended on, the last page carries no token,
//! and a `nextToken` the listing did not itself issue is `InvalidNextTokenException`
//! (400).
//!
//! The rows vary what the rule generalises over: how many schemas exist (none through
//! several), the page size (absent, each published bound, and sizes between), the shape of
//! each schema (its name, its Avro document, whether it belongs to a domain), and the
//! form of the `nextToken` offered. The bundled issue's own step — `ListSchemas` with no
//! parameters at all — is one row among them.

use serde_json::{json, Value};

use crate::dataset_group_lifecycle::{
    avro_schema, create_active_group, create_schema, error_code, list_all, Emulator, ACCOUNT,
    REGION,
};

/// One schema a row creates: its name, its Avro document, and the `Domain` it is created
/// with (`None` for a schema outside a Domain dataset group).
///
/// The domains are the model's own `Domain` enum values (`DOMAIN_VALUES` in
/// `src/ops/types/shared_types.rs`), and `ECOMMERCE` is the entry the issue never cited.
const SCHEMA_CATALOG: &[(&str, &str, Option<&str>)] = &[
    ("A", "Interactions", None),
    ("items9", "Items", Some("ECOMMERCE")),
    ("vod_users", "Users", Some("VIDEO_ON_DEMAND")),
    ("my-actions-schema", "Actions", None),
    ("action_interactions-1", "Action_Interactions", None),
];

/// The ARN this service mints for the schema `name`, as `src/state.rs::schema_arn` builds
/// it — so a row can predict the key the listing orders by.
fn expected_schema_arn(name: &str) -> String {
    format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/{name}")
}

/// Create the catalog's schemas and return their ARNs in the order the listing must
/// report them (the store is keyed by ARN, so that order is the ARNs' own).
async fn create_catalog(emulator: &Emulator) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    for (name, dataset_type, domain) in SCHEMA_CATALOG {
        let arn = match domain {
            // `create_schema` covers the domain-less form; a domain schema is created
            // here, with the `domain` the row names.
            None => create_schema(emulator, name, &avro_schema(dataset_type), name).await,
            Some(domain) => {
                let (status, body) = emulator
                    .call(
                        "CreateSchema",
                        json!({
                            "name": name,
                            "schema": avro_schema(dataset_type),
                            "domain": domain,
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected CreateSchema to answer 200, got {status} {body:?}"
                );
                body["schemaArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: expected a schemaArn, got {body:?}"))
                    .to_string()
            }
        };
        assert_eq!(
            arn,
            expected_schema_arn(name),
            "case {name:?}: expected the ARN this service mints for the name"
        );
        arns.push(arn);
    }
    arns.sort();
    arns
}

/// The `schemaArn` of each listed summary.
fn listed_arns(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|summary| summary["schemaArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn list_schemas_reports_every_schema_at_every_page_size() {
    let emulator = Emulator::new();

    // An account holding no schema at all: a 200 with an empty list, never an error —
    // and that is true of the issue's own step, `ListSchemas` with no parameters.
    for max_results in [None, Some(1), Some(100)] {
        let case = format!("an empty account at maxResults {max_results:?}");
        let listed = list_all(&emulator, "ListSchemas", "schemas", None, max_results).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected no schema, got {listed:?}"
        );
    }

    let expected = create_catalog(&emulator).await;

    // `maxResults` is modelled 1..100: every size inside the range reports exactly the
    // catalog, as does an absent `maxResults`, and both published bounds are accepted.
    for max_results in [None, Some(1), Some(2), Some(3), Some(5), Some(100)] {
        let case = format!("maxResults {max_results:?}");
        let listed = list_all(&emulator, "ListSchemas", "schemas", None, max_results).await;
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case:?}: expected every created schema exactly once, in ARN order"
        );

        for summary in &listed {
            let arn = summary["schemaArn"].as_str().unwrap_or_default();
            let name = summary["name"].as_str().unwrap_or_default();
            let created_with = SCHEMA_CATALOG
                .iter()
                .find(|(candidate, _, _)| *candidate == name)
                .unwrap_or_else(|| panic!("case {case:?}: unexpected schema {summary:?}"));
            assert_eq!(
                arn,
                expected_schema_arn(name),
                "case {case:?}: expected {name:?}'s own ARN, got {summary:?}"
            );
            assert!(
                summary["creationDateTime"].is_number()
                    && summary["lastUpdatedDateTime"].is_number(),
                "case {case:?}: expected {name:?} to carry both timestamps, got {summary:?}"
            );
            assert_eq!(
                summary["domain"].as_str(),
                created_with.2,
                "case {case:?}: expected {name:?}'s domain as created, got {summary:?}"
            );
            // "Provides a SUMMARY of the properties of a dataset schema. For a complete
            // listing, call the DescribeSchema API": the Avro document is the describe's
            // answer, not the listing's.
            assert!(
                summary["schema"].is_null(),
                "case {case:?}: expected {name:?}'s summary to carry no schema document, got \
                 {summary:?}"
            );
        }
    }

    // A listing REPORTS: two listings a call apart agree, and the document each schema was
    // created with is still served by the describe the summary points at.
    let again = list_all(&emulator, "ListSchemas", "schemas", None, None).await;
    assert_eq!(
        listed_arns(&again),
        expected,
        "case \"a second listing\": expected a read to change nothing"
    );
    for (name, dataset_type, _) in SCHEMA_CATALOG {
        let (status, body) = emulator
            .call_with_arn("DescribeSchema", "schemaArn", &expected_schema_arn(name))
            .await;
        assert_eq!(
            (status, body["schema"]["schema"].as_str()),
            (200, Some(avro_schema(dataset_type).as_str())),
            "case {name:?}: expected DescribeSchema to still serve the document it was created \
             with, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn list_schemas_refuses_a_page_size_outside_the_modelled_range() {
    let emulator = Emulator::new();
    create_catalog(&emulator).await;

    // The modelled range is 1..100, so each value just outside it — and a negative one —
    // is refused before the handler runs.
    for max_results in [0, 101, -1] {
        let case = format!("maxResults {max_results}");
        let (status, body) = emulator
            .call("ListSchemas", json!({"maxResults": max_results}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a maxResults outside 1..100, got {status} \
             {body:?}"
        );
    }
}

#[tokio::test]
async fn list_schemas_resumes_only_from_a_token_it_issued() {
    let emulator = Emulator::new();
    let expected = create_catalog(&emulator).await;

    // A real page of this listing, and a real page of a SIBLING listing: the first is the
    // only thing `ListSchemas` resumes from.
    let (status, body) = emulator
        .call("ListSchemas", json!({"maxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a real page\": expected HTTP 200, got {status} {body:?}"
    );
    let own_token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"a real page\": expected a nextToken, got {body:?}"))
        .to_string();

    create_active_group(&emulator, "schema-token-dsg", "tokens").await;
    let (status, body) = emulator
        .call("ListDatasetGroups", json!({"maxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a sibling listing's page\": expected HTTP 200, got {status} {body:?}"
    );
    let sibling_token = body["nextToken"].as_str().map(ToString::to_string);

    // Every form of token this listing did not issue.
    let mut refused: Vec<(String, String)> = vec![
        ("an arbitrary string".to_string(), "bogus".to_string()),
        (
            "an ARN pasted in raw".to_string(),
            expected[0].clone(),
        ),
        (
            "hex that decodes to no tagged key".to_string(),
            "6e6f742d612d746f6b656e".to_string(),
        ),
        ("the empty string".to_string(), String::new()),
    ];
    if let Some(sibling_token) = sibling_token {
        refused.push(("another listing's token".to_string(), sibling_token));
    }

    for (case, token) in refused {
        let (status, body) = emulator
            .call("ListSchemas", json!({"nextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("InvalidNextTokenException"),
            "case {case:?}: expected InvalidNextTokenException, got {body:?}"
        );
    }

    // The token this listing issued resumes it, after the item its page ended on.
    let (status, body) = emulator
        .call("ListSchemas", json!({"nextToken": own_token}))
        .await;
    assert_eq!(
        status, 200,
        "case \"its own token\": expected HTTP 200, got {status} {body:?}"
    );
    let resumed = listed_arns(
        body["schemas"]
            .as_array()
            .unwrap_or_else(|| panic!("case \"its own token\": expected a schemas list, got {body:?}")),
    );
    assert_eq!(
        resumed,
        expected[1..].to_vec(),
        "case \"its own token\": expected the listing to resume after the first schema"
    );
}
