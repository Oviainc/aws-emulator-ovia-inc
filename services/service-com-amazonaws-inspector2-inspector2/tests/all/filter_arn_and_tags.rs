// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two rules of the Amazon Inspector filter surface, driven through
//! `CreateFilter`, `ListFilters`, `UpdateFilter` and `DeleteFilter`.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.
//!
//! The rules under test:
//!
//! * an operation that takes a filter ARN naming a filter checks that ARN's SHAPE before
//!   it consults the store, refusing a value that is not a well-formed filter ARN of this
//!   service with HTTP 400 `ValidationException`; only a well-formed ARN reaches the
//!   store, where naming no stored filter is HTTP 404 `ResourceNotFoundException`;
//! * a filter's tag set is reported unconditionally, as an empty map when the filter has
//!   no tags, so a response's shape never depends on whether tags were set.

use serde_json::{json, Value};

use crate::filters_and_enablement::{post_in, router};

/// `UpdateFilter`'s route and the member it takes the filter's ARN in.
/// Source: `smithy_query(query="operation", operation="UpdateFilter")` —
/// `POST /filters/update`, `filterArn` required.
const UPDATE: (&str, &str) = ("/filters/update", "filterArn");

/// `DeleteFilter`'s route and the member it takes the filter's ARN in.
/// Source: `smithy_query(query="operation", operation="DeleteFilter")` —
/// `POST /filters/delete`, `arn` required.
const DELETE: (&str, &str) = ("/filters/delete", "arn");

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A `CreateFilter` body, optionally carrying `tags`.
fn create_body(name: &str, tags: Option<&Value>) -> Value {
    let mut body = json!({
        "action": "NONE",
        "name": name,
        "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": name }] }
    });
    if let Some(tags) = tags {
        body["tags"] = tags.clone();
    }
    body
}

/// Create one filter and answer its minted ARN.
async fn create_filter(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
    let (status, _, created) =
        post_in(router, "us-east-1", "/filters/create", &create_body(name, None)).await;
    assert_eq!(
        status, 200,
        "creating {name}: expected 200, got {status} / {created}"
    );
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {name}: expected a minted ARN, got {created}"))
        .to_string()
}

/// A value that is not a well-formed filter ARN of this service is refused by BOTH
/// operations that take one, with HTTP 400 `ValidationException` and the service's
/// invalid-ARN wording quoting the value exactly as sent — and the refusal happens before
/// the store is touched, so the filter that does exist is left alone.
///
/// The rows differ along WHICH part of the ARN shape is wrong: no ARN at all, a bare name,
/// the prefix alone, too few / too many fields, the wrong service namespace, an empty
/// field, another resource type of this service, and a filter ARN missing its id — none of
/// them a value the issue's recorded case used alone.
#[tokio::test]
async fn malformed_filter_arns_are_refused_as_invalid() {
    let malformed: [(&str, &str); 11] = [
        ("not an arn at all", "not-an-arn-at-all"),
        ("a bare name", "my-filter"),
        ("the arn prefix only", "arn"),
        ("too few fields", "arn:aws:inspector2:us-east-1:000000000000"),
        (
            "a seventh field",
            "arn:aws:inspector2:us-east-1:000000000000:owner/000000000000:filter/1",
        ),
        (
            "another service's namespace",
            "arn:aws:inspector:us-east-1:000000000000:owner/000000000000/filter/1",
        ),
        (
            "an empty region",
            "arn:aws:inspector2::000000000000:owner/000000000000/filter/1",
        ),
        (
            "an empty account",
            "arn:aws:inspector2:us-east-1::owner/000000000000/filter/1",
        ),
        (
            "an empty partition",
            "arn::inspector2:us-east-1:000000000000:owner/000000000000/filter/1",
        ),
        (
            "another resource type of this service",
            "arn:aws:inspector2:us-east-1:000000000000:owner/000000000000/cis-configuration/\
             11111111-2222-3333-4444-555555555555",
        ),
        (
            "a filter ARN with no id",
            "arn:aws:inspector2:us-east-1:000000000000:owner/000000000000/filter/",
        ),
    ];

    for (case, candidate) in malformed {
        for (uri, member) in [UPDATE, DELETE] {
            let router = router();
            let existing = create_filter(&router, "survivor").await;

            let mut body = json!({ member: candidate });
            if uri == UPDATE.0 {
                body["description"] = Value::String("tuned".to_string());
            }
            let (status, error_type, response) = post_in(&router, "us-east-1", uri, &body).await;
            assert_eq!(
                status, 400,
                "case {case} / {uri}: expected 400, got {status} / {response}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case} / {uri}: expected ValidationException, got {error_type:?} / \
                 {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some(format!("Invalid Filter Arn {candidate}").as_str()),
                "case {case} / {uri}: unexpected refusal wording, got {response}"
            );

            // The refused request never reached the store: the filter that does exist is
            // still listed, tags and all.
            let (status, _, listed) =
                post_in(&router, "us-east-1", "/filters/list", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case} / {uri}: ListFilters: expected 200, got {status} / {listed}"
            );
            assert_eq!(
                listed["filters"][0]["arn"].as_str(),
                Some(existing.as_str()),
                "case {case} / {uri}: a refused request must leave the store alone, got \
                 {listed}"
            );
        }
    }
}

/// A WELL-FORMED filter ARN reaches the store, so the answer is about the resource rather
/// than the request: the ARN of a stored filter succeeds, and one that names no stored
/// filter is a 404 `ResourceNotFoundException`.
///
/// The rows differ along which part of a well-formed ARN is unfamiliar: the partition, the
/// region, the account, the owner and the id — every one of them still a value the shape
/// check must accept.
#[tokio::test]
async fn well_formed_filter_arns_reach_the_store() {
    let unknown: [(&str, String); 6] = [
        (
            "this emulator's own shape, never minted",
            format!("arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/{ACCOUNT}/filter/deadbeefdeadbeef"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:inspector2:us-gov-west-1:{ACCOUNT}:owner/{ACCOUNT}/filter/1"),
        ),
        (
            "another region",
            format!("arn:aws:inspector2:ap-southeast-2:{ACCOUNT}:owner/{ACCOUNT}/filter/abc123"),
        ),
        (
            "another account",
            "arn:aws:inspector2:us-east-1:111122223333:owner/111122223333/filter/abc123".to_string(),
        ),
        (
            "an organization as the owner",
            format!("arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/o-abcdefghij/filter/abc123"),
        ),
        (
            "a long opaque id",
            format!("arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/{ACCOUNT}/filter/{}", "f".repeat(32)),
        ),
    ];

    for (case, candidate) in unknown {
        for (uri, member) in [UPDATE, DELETE] {
            let router = router();
            let mut body = json!({ member: candidate });
            if uri == UPDATE.0 {
                body["description"] = Value::String("tuned".to_string());
            }
            let (status, error_type, response) = post_in(&router, "us-east-1", uri, &body).await;
            assert_eq!(
                status, 404,
                "case {case} / {uri}: expected 404, got {status} / {response}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case} / {uri}: expected ResourceNotFoundException, got {error_type:?} / \
                 {response}"
            );
        }
    }

    // And the ARN of a filter that IS stored is served, so the shape check refuses nothing
    // the store can answer.
    for (uri, member) in [UPDATE, DELETE] {
        let router = router();
        let arn = create_filter(&router, "reachable").await;
        let mut body = json!({ member: arn.clone() });
        if uri == UPDATE.0 {
            body["description"] = Value::String("tuned".to_string());
        }
        let (status, error_type, response) = post_in(&router, "us-east-1", uri, &body).await;
        assert_eq!(
            status, 200,
            "case a stored ARN / {uri}: expected 200, got {status} / {response} \
             (errortype {error_type:?})"
        );
        assert_eq!(
            response["arn"].as_str(),
            Some(arn.as_str()),
            "case a stored ARN / {uri}: expected the filter's ARN back, got {response}"
        );
    }
}

/// A listed filter ALWAYS carries `tags`: the entries it holds, and an empty object when
/// it holds none — so the shape of the listing never depends on whether tags were set.
///
/// The rows differ along the tag set's presence, size and content, including the modelled
/// key and value length bounds.
/// Source: `smithy_query(query="shape", shape="TagMap")` — keys length 1..128, values
/// length ..256.
#[tokio::test]
async fn filter_tags_are_always_reported() {
    // Built by hand rather than with `json!`, so the key and the value can be the
    // modelled bound lengths.
    let at_key_bound = Value::Object(
        [("k".repeat(128), Value::String("at-the-bound".to_string()))]
            .into_iter()
            .collect(),
    );
    let at_value_bound = Value::Object(
        [("big".to_string(), Value::String("v".repeat(256)))]
            .into_iter()
            .collect(),
    );
    let cases: [(&str, Option<Value>, Value); 6] = [
        ("no tags member at all", None, json!({})),
        ("an empty tags map", Some(json!({})), json!({})),
        (
            "one entry",
            Some(json!({ "team": "appsec" })),
            json!({ "team": "appsec" }),
        ),
        (
            "three entries",
            Some(json!({ "team": "appsec", "env": "prod", "owner": "platform" })),
            json!({ "team": "appsec", "env": "prod", "owner": "platform" }),
        ),
        (
            "a key at the modelled length bound",
            Some(at_key_bound.clone()),
            at_key_bound,
        ),
        (
            "a value at the modelled length bound",
            Some(at_value_bound.clone()),
            at_value_bound,
        ),
    ];

    for (case, tags, expected) in cases {
        let router = router();
        let body = create_body("tagged", tags.as_ref());
        let (status, _, created) = post_in(&router, "us-east-1", "/filters/create", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateFilter: expected 200, got {status} / {created}"
        );

        let (status, _, listed) = post_in(&router, "us-east-1", "/filters/list", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: ListFilters: expected 200, got {status} / {listed}"
        );
        let filter = &listed["filters"][0];
        let got = filter.get("tags").cloned().unwrap_or(Value::Null);
        assert!(
            got.is_object(),
            "case {case}: tags must be a present object, got {got}"
        );
        assert_eq!(
            got, expected,
            "case {case}: expected tags {expected}, got {got}"
        );

        // The same filter after an UpdateFilter that carries no tags member: the tag set
        // is untouched and still reported.
        let arn = created["arn"].as_str().expect("a minted ARN").to_string();
        let (status, _, updated) = post_in(
            &router,
            "us-east-1",
            UPDATE.0,
            &json!({ "filterArn": arn, "description": "tuned" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateFilter: expected 200, got {status} / {updated}"
        );
        let (status, _, listed) = post_in(&router, "us-east-1", "/filters/list", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: ListFilters after the update: expected 200, got {status} / {listed}"
        );
        let got = listed["filters"][0].get("tags").cloned().unwrap_or(Value::Null);
        assert_eq!(
            got, expected,
            "case {case}: an update must leave the reported tags alone, expected {expected}, \
             got {got}"
        );
    }
}
