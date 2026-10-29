// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` already spent on a DIFFERENT `CreateComputationModel`
//! request means, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rule these rows pin: a `clientToken` is spent by ONE call of an operation.
//! A repeat of the SAME request under that token is a RETRY of that call —
//! answered from the computation model it minted, writing nothing — while a
//! request differing in any member it is recognised by belongs to no served call
//! and is refused `ConflictingOperationException` (409) naming the model the
//! token's own call minted. A refused request mints nothing, so the model a second
//! request asked for is never silently dropped on the floor and the caller is
//! never handed an id for a configuration it did not send; the refusal spends
//! nothing of its own, so a retry of the first request is still answered from its
//! model.
//!
//! The rows span what the rule generalises over — the member the second request
//! differs in: the name, a description the first call did not carry, another
//! description, the configuration (the binding variable the computation reads),
//! the value one variable is bound to, the FORM a binding value takes, how many
//! variables the binding defines, tags the first call did not carry, and a tag's
//! value.
//!
//! The HTTP harness (`call`, `error_code`, `router`) is `asset_lifecycle`'s, the
//! token helpers (`client_token`, `with_token`) are `client_token_idempotency`'s,
//! the tag reader (`list_tags`) is `resource_tags`'s, the listing reader
//! (`listed_ids`) is `computation_model_delete`'s, and the computation-model
//! fixtures (`create`, `describe`, `expected_arn`, `anomaly_detection`,
//! `asset_model_property`, `asset_property`, `binding_list`, `id_of`) are
//! `computation_model_lifecycle`'s; all are reused rather than copied.

use serde_json::{json, Value};

use crate::asset_lifecycle::{error_code, router};
use crate::client_token_idempotency::{client_token, with_token};
use crate::computation_model_delete::listed_ids;
use crate::computation_model_lifecycle::{
    anomaly_detection, asset_model_property, asset_property, binding_list, create, describe,
    expected_arn, id_of,
};
use crate::resource_tags::list_tags;

/// A `CreateComputationModel` body, with every absent optional member OMITTED
/// rather than sent as null, carrying `token` when one is given.
fn create_body(
    name: &str,
    description: Option<&str>,
    configuration: &Value,
    binding: &Value,
    tags: Option<&Value>,
    token: Option<&str>,
) -> Value {
    let mut body = json!({
        "computationModelName": name,
        "computationModelConfiguration": configuration.clone(),
        "computationModelDataBinding": binding.clone(),
    });
    let object = body
        .as_object_mut()
        .expect("a CreateComputationModel body is an object");
    if let Some(description) = description {
        object.insert(
            "computationModelDescription".to_string(),
            json!(description),
        );
    }
    if let Some(tags) = tags {
        object.insert("tags".to_string(), tags.clone());
    }
    if let Some(token) = token {
        object.insert("clientToken".to_string(), json!(token));
    }
    body
}

/// The binding three rows share: two variables a computation may read plus one it
/// does not, so a row can differ in the variable its configuration names without
/// differing in the binding as well.
fn three_variable_binding() -> Value {
    json!({
        "input": asset_model_property(&id_of('1'), &id_of('2')),
        "result": asset_model_property(&id_of('1'), &id_of('3')),
        "spare": asset_model_property(&id_of('1'), &id_of('4')),
    })
}

/// The binding most rows carry: the two variables the configuration references.
fn two_variable_binding() -> Value {
    json!({
        "input": asset_model_property(&id_of('1'), &id_of('2')),
        "result": asset_model_property(&id_of('1'), &id_of('3')),
    })
}

/// One row: the request a token is spent on, and a DIFFERENT request of this same
/// operation sent under that same, now spent, token.
struct Row {
    case: &'static str,
    /// The request the token is spent on, WITHOUT the token.
    first: Value,
    /// The same operation with ONE member different, which is another call.
    second: Value,
    /// The tags the FIRST request carried, which the model must still report
    /// after the refusal — an empty map when it carried none.
    first_tags: Value,
}

/// The rows: the same operation sent twice under one token, differing in exactly
/// one of the members a create is recognised by.
fn rows() -> Vec<Row> {
    let configuration = anomaly_detection("input", "result");
    vec![
        Row {
            case: "another computationModelName",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "pump-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "a description the first request did not carry",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                Some("Detects anomalies in turbine 4"),
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "another computationModelDescription",
            first: create_body(
                "turbine-anomaly",
                Some("Detects anomalies in turbine 4"),
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                Some("Detects anomalies in turbine 5"),
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "another computationModelConfiguration",
            first: create_body(
                "turbine-anomaly",
                None,
                &anomaly_detection("input", "result"),
                &three_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                None,
                &anomaly_detection("spare", "result"),
                &three_variable_binding(),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "another property bound to one variable",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &json!({
                    "input": asset_model_property(&id_of('1'), &id_of('5')),
                    "result": asset_model_property(&id_of('1'), &id_of('3')),
                }),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "a binding value in another form",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &json!({
                    "input": asset_property(&id_of('6'), &id_of('2')),
                    "result": asset_model_property(&id_of('1'), &id_of('3')),
                }),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "one more variable bound, by a list of values",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &json!({
                    "input": asset_model_property(&id_of('1'), &id_of('2')),
                    "result": asset_model_property(&id_of('1'), &id_of('3')),
                    "spare": binding_list(vec![
                        asset_property(&id_of('7'), &id_of('8')),
                        asset_model_property(&id_of('9'), &id_of('a')),
                    ]),
                }),
                None,
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "tags the first request did not carry",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                None,
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                Some(&json!({ "line": "packaging" })),
                None,
            ),
            first_tags: json!({}),
        },
        Row {
            case: "another value under the same tag key",
            first: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                Some(&json!({ "line": "packaging" })),
                None,
            ),
            second: create_body(
                "turbine-anomaly",
                None,
                &configuration,
                &two_variable_binding(),
                Some(&json!({ "line": "bottling" })),
                None,
            ),
            first_tags: json!({ "line": "packaging" }),
        },
    ]
}

#[tokio::test]
async fn a_token_one_create_spent_refuses_a_different_create_and_mints_nothing() {
    for row in rows() {
        // A router per row, so the listing below reports exactly what this row's
        // own calls left behind.
        let router = router();
        let Row {
            case,
            first,
            second,
            first_tags,
        } = row;
        // The token's seed is the row's own case, with its spaces replaced:
        // `ClientToken` is `^\S{36,64}$`, so no whitespace may reach the wire.
        let token = client_token(&case.replace(' ', "-"));

        let (status, created) = create(&router, with_token(&first, &token)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the first create expected 202, got {status} {created}"
        );
        let model_id = created["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no computationModelId in {created}"))
            .to_string();

        // The SAME request under that token is a retry of the call above: it is
        // answered from the model that call minted and mints none of its own.
        let (status, retried) = create(&router, with_token(&first, &token)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the retry expected 202, got {status} {retried}"
        );
        assert_eq!(
            retried["computationModelId"].as_str().unwrap_or(""),
            model_id,
            "case {case:?}: a retry answers from the model its token minted, got {retried}"
        );
        assert_eq!(
            retried["computationModelArn"].as_str().unwrap_or(""),
            expected_arn(&model_id),
            "case {case:?}: a retry reports that model's own ARN, got {retried}"
        );

        // The DIFFERING request reusing that spent token repeats no call, so it is
        // the modelled conflict, naming the model the token's own call minted.
        let (status, refused) = create(&router, with_token(&second, &token)).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {case:?}: a spent token expected the modelled conflict, got {status} {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            model_id,
            "case {case:?}: the conflict names the model the first call minted, got {refused}"
        );
        assert_eq!(
            refused["resourceArn"].as_str().unwrap_or(""),
            expected_arn(&model_id),
            "case {case:?}: the conflict carries that model's ARN, got {refused}"
        );

        // The refused request minted nothing and rewrote nothing: the stored model
        // still reports what the FIRST request asked for, and the service holds no
        // second model.
        let (status, described) = describe(&router, &model_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read of the first model got {status} {described}"
        );
        for member in [
            "computationModelName",
            "computationModelDescription",
            "computationModelConfiguration",
            "computationModelDataBinding",
        ] {
            // An optional member the first request omitted reads back as null,
            // which is exactly what the body carries for it.
            let expected = first.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                described[member], expected,
                "case {case:?}: the refused request must leave {member} as the first create \
                 set it, expected {expected}, got {described}"
            );
        }
        let (status, tags) = list_tags(&router, &expected_arn(&model_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource got {status} {tags}"
        );
        assert_eq!(
            tags["tags"], first_tags,
            "case {case:?}: the refused request must leave the model's tags as the first \
             create set them, expected {first_tags}, got {tags}"
        );
        let held = listed_ids(&router).await;
        assert_eq!(
            held,
            vec![model_id.clone()],
            "case {case:?}: only the model the first call minted may be held, holds {held:?}"
        );

        // The refusal spent nothing of its own: the token still names the first
        // call, so a retry of THAT request is still answered from its model.
        let (status, again) = create(&router, with_token(&first, &token)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the retry after the refusal expected 202, got {status} {again}"
        );
        assert_eq!(
            again["computationModelId"].as_str().unwrap_or(""),
            model_id,
            "case {case:?}: a refused request must not unspend the token, got {again}"
        );
        let held = listed_ids(&router).await;
        assert_eq!(
            held,
            vec![model_id.clone()],
            "case {case:?}: that retry must mint nothing either, holds {held:?}"
        );
    }
}

#[tokio::test]
async fn a_token_no_create_has_spent_mints_a_model_of_its_own() {
    let router = router();

    // Rows: fresh tokens at each published `ClientToken` length bound and one in
    // between, plus two requests carrying no token at all. None of them repeats a
    // served call, so each must mint a model of its own.
    let mut tokens: Vec<Option<String>> = vec![None, None];
    // Source: `ClientToken` is `@length(min: 36, max: 64)` —
    // `smithy_query(query="operation", operation="CreateComputationModel")`.
    for length in [36_usize, 50, 64] {
        let mut token = client_token(&format!("fresh-token-of-{length}-characters"));
        while token.len() < length {
            token.push('f');
        }
        assert_eq!(
            token.len(),
            length,
            "case {length}: this row needs a token of {length} characters, got {token:?}"
        );
        tokens.push(Some(token));
    }

    let mut minted: Vec<String> = Vec::new();
    for token in &tokens {
        let case = token.as_deref().unwrap_or("no token");
        let body = create_body(
            "turbine-anomaly",
            None,
            &anomaly_detection("input", "result"),
            &two_variable_binding(),
            None,
            token.as_deref(),
        );
        let (status, created) = create(&router, body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {created}"
        );
        let model_id = created["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no computationModelId in {created}"))
            .to_string();
        assert!(
            !minted.contains(&model_id),
            "case {case:?}: a call that repeats none must mint a model of its own, \
             got {model_id} again"
        );
        minted.push(model_id);
    }

    let mut expected = minted.clone();
    expected.sort();
    let held = listed_ids(&router).await;
    assert_eq!(
        held, expected,
        "every row must have minted a model of its own: {expected:?} minted, {held:?} held"
    );
}
