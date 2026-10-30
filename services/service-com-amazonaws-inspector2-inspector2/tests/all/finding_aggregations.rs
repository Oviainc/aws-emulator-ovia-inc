// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFindingAggregations`.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.
//!
//! The rule under test: `aggregationType` names the aggregation and
//! `aggregationRequest` carries THAT aggregation's filters, so the two must name the
//! same aggregation; a pair that does not is refused with the modelled
//! `ValidationException`, and a pair that does is answered with the roll-up of the
//! findings this service has recorded.

use serde_json::{json, Value};

use crate::filters_and_enablement::{post_in, router};

/// `ListFindingAggregations`'s route.
/// Source: `smithy_query(query="operation", operation="ListFindingAggregations")` —
/// `POST /findings/aggregation/list`.
const URI: &str = "/findings/aggregation/list";

/// Every `AggregationType` token, with the `AggregationRequest` union member that
/// carries that aggregation's filters.
///
/// Source: `smithy_query(query="shape", shape="AggregationType,AggregationRequest")`.
const AGGREGATIONS: [(&str, &str); 15] = [
    ("ACCOUNT", "accountAggregation"),
    ("AMI", "amiAggregation"),
    ("AWS_EC2_INSTANCE", "ec2InstanceAggregation"),
    ("AWS_ECR_CONTAINER", "awsEcrContainerAggregation"),
    ("AWS_LAMBDA_FUNCTION", "lambdaFunctionAggregation"),
    ("CODE_REPOSITORY", "codeRepositoryAggregation"),
    ("CONTAINER_IMAGE", "containerImageAggregation"),
    ("FINDING_TYPE", "findingTypeAggregation"),
    ("IMAGE_LAYER", "imageLayerAggregation"),
    ("LAMBDA_LAYER", "lambdaLayerAggregation"),
    ("PACKAGE", "packageAggregation"),
    ("REPOSITORY", "repositoryAggregation"),
    ("SERVERLESS_FUNCTION", "serverlessFunctionAggregation"),
    ("TITLE", "titleAggregation"),
    ("VM_INSTANCE", "vmInstanceAggregation"),
];

/// A request naming `aggregation_type`, optionally carrying one aggregation request
/// member. `sortOrder` is the one member EVERY `AggregationRequest` variant declares, so
/// the same body shape fits all fifteen.
/// Source: `smithy_query(query="shape", shape="SortOrder")` — `ASC | DESC`.
fn body(aggregation_type: &str, member: Option<&str>, sort_order: &str) -> Value {
    let mut request = json!({ "aggregationType": aggregation_type });
    if let Some(member) = member {
        request["aggregationRequest"] = json!({ member: { "sortOrder": sort_order } });
    }
    request
}

/// A 12-digit account id, so a row can name several distinct accounts.
/// Source: `smithy_query(query="shape", shape="AccountId")` — `^\d{12}$`.
fn account_id(index: usize) -> String {
    format!("{:012}", 100_000_000_000_u64 + index as u64)
}

/// An aggregation whose request is the one its type names is answered: the type is
/// echoed back, and the roll-up of a service that has recorded no finding carries no
/// rows and no cursor.
#[tokio::test]
async fn every_aggregation_type_answers_the_aggregation_request_it_names() {
    let router = router();

    // Rows: all fifteen aggregation types, each with its own request member, and the
    // two `SortOrder` tokens alternating across them.
    for (index, (aggregation_type, member)) in AGGREGATIONS.iter().enumerate() {
        let sort_order = if index % 2 == 0 { "ASC" } else { "DESC" };
        let request = body(aggregation_type, Some(member), sort_order);
        let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;

        assert_eq!(
            status, 200,
            "case {aggregation_type}/{member}: expected 200, got {status} / {answer} \
             ({error_type:?})"
        );
        assert_eq!(
            answer["aggregationType"], *aggregation_type,
            "case {aggregation_type}/{member}: the answer must name the aggregation type the \
             request asked for, got {answer}"
        );
        assert_eq!(
            answer["responses"],
            json!([]),
            "case {aggregation_type}/{member}: nothing has recorded a finding, so the roll-up \
             must carry no rows, got {answer}"
        );
        assert!(
            answer.get("nextToken").is_none(),
            "case {aggregation_type}/{member}: an answer carrying its whole roll-up must hand \
             out no cursor, got {answer}"
        );
    }
}

/// `aggregationRequest` is optional: a request that carries none still aggregates the
/// type it names.
#[tokio::test]
async fn an_aggregation_type_without_an_aggregation_request_is_answered() {
    let router = router();

    for (aggregation_type, _) in AGGREGATIONS {
        let request = body(aggregation_type, None, "ASC");
        let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;

        assert_eq!(
            status, 200,
            "case {aggregation_type}: expected 200, got {status} / {answer} ({error_type:?})"
        );
        assert_eq!(
            answer["aggregationType"], aggregation_type,
            "case {aggregation_type}: the answer must name the requested aggregation type, got \
             {answer}"
        );
        assert_eq!(
            answer["responses"],
            json!([]),
            "case {aggregation_type}: nothing has recorded a finding, so the roll-up must carry \
             no rows, got {answer}"
        );
    }
}

/// An `aggregationRequest` that is not the aggregation `aggregationType` names describes
/// two aggregations at once, so it is refused — never answered with either one.
#[tokio::test]
async fn an_aggregation_request_of_another_aggregation_is_refused() {
    let router = router();

    // Rows: every aggregation type against the request member of another type, twice
    // over with different offsets, so each type meets two foreign request members and
    // each member is refused for fourteen types across the table.
    for offset in [1_usize, 7] {
        for (index, (aggregation_type, own_member)) in AGGREGATIONS.iter().enumerate() {
            let (_, foreign_member) = AGGREGATIONS[(index + offset) % AGGREGATIONS.len()];
            let request = body(aggregation_type, Some(foreign_member), "DESC");
            let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;
            let case = format!("{aggregation_type} with {foreign_member}");

            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} / {answer}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {error_type:?} / {answer}"
            );
            assert_eq!(
                answer["reason"], "FIELD_VALIDATION_FAILED",
                "case {case}: expected the FIELD_VALIDATION_FAILED reason, got {answer}"
            );
            assert_eq!(
                answer["fields"][0]["name"], "aggregationRequest",
                "case {case}: the refusal must name the member that failed, got {answer}"
            );
            let message = answer["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(aggregation_type)
                    && message.contains(own_member)
                    && message.contains(foreign_member),
                "case {case}: the message must name the aggregation type, the request it takes \
                 ({own_member}) and the one it was given ({foreign_member}), got {message:?}"
            );
        }
    }
}

/// A `nextToken` no answer handed out names no position in a roll-up, so it is refused
/// rather than read as the start of the list.
#[tokio::test]
async fn a_next_token_no_answer_handed_out_is_refused() {
    let router = router();

    // Rows: the empty token (inside `NextToken`'s modelled length), a non-hex token,
    // well-formed hex, and hex that decodes to an ARN of another service — none of them
    // a cursor this operation ever handed out.
    let tokens = [
        "",
        "not-a-token",
        "4141",
        "61726e3a6177733a73333a3a3a6d792d6275636b6574",
    ];
    for token in tokens {
        for (aggregation_type, member) in [AGGREGATIONS[0], AGGREGATIONS[8]] {
            let mut request = body(aggregation_type, Some(member), "ASC");
            request["nextToken"] = json!(token);
            let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;
            let case = format!("{aggregation_type} with nextToken {token:?}");

            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} / {answer}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {error_type:?} / {answer}"
            );
            assert_eq!(
                answer["reason"], "CANNOT_PARSE",
                "case {case}: expected the CANNOT_PARSE reason, got {answer}"
            );
            assert_eq!(
                answer["fields"][0]["name"], "nextToken",
                "case {case}: the refusal must name the member that failed, got {answer}"
            );
        }
    }
}

/// An `aggregationType` outside the modelled enum is refused at the boundary — 400
/// `ValidationException` naming the member and the enum constraint it broke — so the
/// handler never answers it with some other aggregation's roll-up.
///
/// The constraint CLAUSE is asserted, not the whole sentence: the rejected value the
/// emulator still interpolates into it, and the order it lists the value set in, are an
/// open divergence of this service's shared constraint-violation message builder, which
/// this operation neither owns nor reaches.
#[tokio::test]
async fn an_aggregation_type_outside_the_enum_is_refused() {
    let router = router();

    for value in ["NOT_A_REAL_AGGREGATION_TYPE", "account", ""] {
        let (status, error_type, answer) =
            post_in(&router, "us-east-1", URI, &body(value, None, "ASC")).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected 400, got {status} / {answer} ({error_type:?})"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {value:?}: expected ValidationException, got {error_type:?} / {answer}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("at 'aggregationType'")
                && message.contains("Member must satisfy enum value set:"),
            "case {value:?}: the refusal must name the member and the enum constraint it broke, \
             got {message:?}"
        );
        assert!(
            answer.get("responses").is_none(),
            "case {value:?}: a refused request must carry no roll-up, got {answer}"
        );
    }
}

/// The scope and page members are honoured on their own terms: `accountIds` filters and
/// a `maxResults` inside the modelled range are accepted, and the modelled bounds
/// themselves are enforced before the handler runs.
#[tokio::test]
async fn account_id_filters_and_max_results_are_accepted_inside_their_modelled_bounds() {
    let router = router();

    // Rows: the `StringComparison` tokens, the `accountIds` list bounds (1 and 10) and
    // the `maxResults` range bounds (1 and 100).
    // Source: `smithy_query(query="shape", shape="StringComparison")` and
    // `smithy_query(query="operation", operation="ListFindingAggregations")`.
    let cases: [(&str, usize, i64); 4] = [
        ("EQUALS", 1, 1),
        ("PREFIX", 1, 100),
        ("NOT_EQUALS", 10, 50),
        ("EQUALS", 10, 100),
    ];

    for (comparison, ids, max_results) in cases {
        let filters: Vec<Value> = (0..ids)
            .map(|index| json!({ "comparison": comparison, "value": account_id(index) }))
            .collect();
        let mut request = body("ACCOUNT", Some("accountAggregation"), "DESC");
        request["accountIds"] = json!(filters);
        request["maxResults"] = json!(max_results);
        let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;
        let case = format!("{comparison} x{ids} maxResults={max_results}");

        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} / {answer} ({error_type:?})"
        );
        assert_eq!(
            answer["responses"],
            json!([]),
            "case {case}: no account has a recorded finding, so no account rolls up to a row, \
             got {answer}"
        );
    }

    // One past each published bound is refused at the boundary, so the handler never
    // sees it: `maxResults` outside 1..100, and an `accountIds` list outside 1..10.
    let refused: [(&str, Value); 4] = [
        ("maxResults=0", json!(0)),
        ("maxResults=101", json!(101)),
        ("accountIds=[]", Value::Null),
        ("accountIds x11", Value::Null),
    ];
    for (case, max_results) in refused {
        let mut request = body("ACCOUNT", Some("accountAggregation"), "DESC");
        if max_results.is_null() {
            let ids = if case == "accountIds=[]" { 0 } else { 11 };
            let filters: Vec<Value> = (0..ids)
                .map(|index| json!({ "comparison": "EQUALS", "value": account_id(index) }))
                .collect();
            request["accountIds"] = json!(filters);
        } else {
            request["maxResults"] = max_results;
        }
        let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;

        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} / {answer}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {error_type:?} / {answer}"
        );
    }
}
