// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `XssMatchSet` lifecycle, end to end over the wire:
//! `CreateXssMatchSet`, `GetXssMatchSet`, `UpdateXssMatchSet`, `ListXssMatchSets` and
//! `DeleteXssMatchSet`, each driven with the change token `GetChangeToken` mints.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`, awsJson1_1), so what these tests
//! assert is what an SDK client sees: each response is built from the persisted record,
//! not from the request.
//!
//! The tables below span the model's own value sets (`MatchFieldType`,
//! `TextTransformation`) rather than one recorded pair, so the rules under test are
//! checked on entries no recording cites.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    MATCH_FIELD_TYPE_VALUES, TEXT_TRANSFORMATION_VALUES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_PAGE_SIZE, MAX_XSS_MATCH_SET_UPDATES_PER_REQUEST, MAX_XSS_MATCH_TUPLES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::EmulatedService;

/// The `MatchFieldType` values whose `FieldToMatch.Data` names the part to inspect.
///
/// Source: `FieldToMatch`'s `Data` documentation — "When the value of `Type` is
/// `HEADER`, enter the name of the header that you want AWS WAF to search […] When the
/// value of `Type` is `SINGLE_QUERY_ARG`, enter the name of the parameter that you want
/// AWS WAF to search […] If the value of `Type` is any other value, omit `Data`." Stated
/// here independently of the handler so the test does not check the code against itself.
const TYPES_TAKING_DATA: &[&str] = &["HEADER", "SINGLE_QUERY_ARG"];

/// A router serving this one service over a fresh engine, as `build_router` does in
/// production.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope names
        // this service's signing name (`waf-regional`). Request authentication is off
        // for this router, so the signature itself is never checked.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250818/us-east-1/waf-regional/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000",
        )
        .header("host", "waf-regional.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSWAF_Regional_20161128.{operation}"),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} expected 200, got {status}: {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of an error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert `operation` refused `request` with HTTP 400 and the named modelled error.
async fn expect_error(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    expected: &str,
    label: &str,
) -> Value {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {label}: {operation} must answer 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains(expected),
        "case {label}: expected {expected}, got {} ({body})",
        error_code(&body)
    );
    body
}

/// The change token a mutating call must carry. Each one is consumed by exactly one
/// mutating call, so every caller below asks for its own.
async fn change_token(router: &MultiServiceRouter) -> String {
    let body = ok_call(router, "GetChangeToken", &json!({})).await;
    body["ChangeToken"]
        .as_str()
        .unwrap_or_else(|| panic!("GetChangeToken returned no ChangeToken: {body}"))
        .to_string()
}

/// Create an XssMatchSet named `name` and return its `XssMatchSetId`.
async fn create_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateXssMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["XssMatchSet"]["XssMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateXssMatchSet returned no XssMatchSetId: {body}"))
        .to_string()
}

/// A `Data` value for `field_type`: the name of the header or query-string parameter
/// when the type takes one, and none otherwise.
fn data_for(field_type: &str) -> Option<&'static str> {
    match field_type {
        "HEADER" => Some("User-Agent"),
        "SINGLE_QUERY_ARG" => Some("UserName"),
        _ => None,
    }
}

/// One `XssMatchTuple` on the wire.
fn tuple(field_type: &str, data: Option<&str>, transformation: &str) -> Value {
    let mut field_to_match = json!({"Type": field_type});
    if let Some(data) = data {
        field_to_match["Data"] = json!(data);
    }
    json!({"FieldToMatch": field_to_match, "TextTransformation": transformation})
}

/// One `XssMatchSetUpdate` on the wire.
fn update(action: &str, filter: Value) -> Value {
    json!({"Action": action, "XssMatchTuple": filter})
}

/// Apply `updates` to `xss_match_set_id` with a fresh change token.
async fn apply_updates(
    router: &MultiServiceRouter,
    xss_match_set_id: &str,
    updates: Vec<Value>,
) -> (u16, Value) {
    let token = change_token(router).await;
    call(
        router,
        "UpdateXssMatchSet",
        &json!({"XssMatchSetId": xss_match_set_id, "ChangeToken": token, "Updates": updates}),
    )
    .await
}

/// The `(Type, Data, TextTransformation)` triples `GetXssMatchSet` reports for
/// `xss_match_set_id`, in stored order.
async fn filters(
    router: &MultiServiceRouter,
    xss_match_set_id: &str,
) -> Vec<(String, Option<String>, String)> {
    let body = ok_call(
        router,
        "GetXssMatchSet",
        &json!({"XssMatchSetId": xss_match_set_id}),
    )
    .await;
    assert_eq!(
        body["XssMatchSet"]["XssMatchSetId"].as_str(),
        Some(xss_match_set_id),
        "GetXssMatchSet must report the set that was asked for: {body}"
    );
    body["XssMatchSet"]["XssMatchTuples"]
        .as_array()
        .unwrap_or_else(|| panic!("GetXssMatchSet returned no XssMatchTuples: {body}"))
        .iter()
        .map(|filter| {
            (
                filter["FieldToMatch"]["Type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                filter["FieldToMatch"]["Data"].as_str().map(str::to_string),
                filter["TextTransformation"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// `count` distinct `(MatchFieldType, TextTransformation)` pairs, drawn from the model's
/// own two value sets so a table spans both rather than repeating one row.
fn distinct_rows(count: usize) -> Vec<(&'static str, &'static str)> {
    let mut rows: Vec<(&'static str, &'static str)> = Vec::new();
    for field_type in MATCH_FIELD_TYPE_VALUES {
        for transformation in TEXT_TRANSFORMATION_VALUES {
            if rows.len() == count {
                return rows;
            }
            rows.push((field_type, transformation));
        }
    }
    assert_eq!(
        rows.len(),
        count,
        "the model's value sets yield {} distinct pairs, {count} asked for",
        rows.len()
    );
    rows
}

/// `ResourceId`/`ChangeToken` values AWS WAF Classic hands out are 36-character
/// UUID-shaped strings, and the model bounds both at `len 1..128, pattern \S`.
fn is_resource_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(groups.iter())
            .all(|(len, group)| {
                group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit())
            })
        && (1..=128).contains(&id.len())
        && !id.chars().any(char::is_whitespace)
}

/// `CreateXssMatchSet` mints an id, stores an empty set under it and answers the stored
/// record; `GetXssMatchSet` reports that same record back.
///
/// The table's rows differ along what `ResourceName` allows (`len 1..128, pattern \S`):
/// both modelled length bounds, punctuation, non-ASCII text and an embedded space.
#[tokio::test]
async fn create_and_get_round_trip_the_stored_record_for_every_name_in_the_table() {
    let router = router();

    let names = [
        ("minimum modelled length", "a".to_string()),
        ("maximum modelled length", "n".repeat(128)),
        ("punctuation", "xss.match-set_01".to_string()),
        ("non-ascii", "規則セット".to_string()),
        ("embedded space", "front door filters".to_string()),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (label, name) in &names {
        let token = change_token(&router).await;
        let created = ok_call(
            &router,
            "CreateXssMatchSet",
            &json!({"Name": name, "ChangeToken": token.clone()}),
        )
        .await;
        assert_eq!(
            created["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: CreateXssMatchSet must report the change token it consumed, got {created}"
        );
        let id = created["XssMatchSet"]["XssMatchSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: no XssMatchSetId in {created}"))
            .to_string();
        assert!(
            is_resource_id(&id),
            "case {label}: expected a minted ResourceId, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {label}: CreateXssMatchSet handed out {id} twice"
        );
        assert_eq!(
            created["XssMatchSet"]["Name"].as_str(),
            Some(name.as_str()),
            "case {label}: the stored Name must be the one created, got {created}"
        );
        assert_eq!(
            created["XssMatchSet"]["XssMatchTuples"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {label}: a new XssMatchSet holds no filters, got {created}"
        );

        let fetched = ok_call(&router, "GetXssMatchSet", &json!({"XssMatchSetId": id})).await;
        assert_eq!(
            fetched["XssMatchSet"], created["XssMatchSet"],
            "case {label}: GetXssMatchSet must report the record CreateXssMatchSet stored, \
             got {fetched}"
        );
    }

    // `ResourceName` carries no uniqueness constraint, so two sets created with one name
    // are two distinct resources.
    let first = create_set(&router, "same-name").await;
    let second = create_set(&router, "same-name").await;
    assert_ne!(
        first, second,
        "case duplicate name: two XssMatchSets with one Name must be two resources, \
         both answered {first}"
    );

    // An id nothing was ever created for is refused, not answered with an empty set.
    expect_error(
        &router,
        "GetXssMatchSet",
        &json!({"XssMatchSetId": "00000000-0000-0000-0000-000000000000"}),
        "WAFNonexistentItemException",
        "never created",
    )
    .await;
}

/// `UpdateXssMatchSet` inserts and deletes filters, and `GetXssMatchSet` reports exactly
/// the filters the set holds, in insertion order.
///
/// The table spans every `MatchFieldType` the model declares, each paired with a
/// different `TextTransformation` from the model's own catalog.
#[tokio::test]
async fn every_modelled_match_field_type_round_trips_as_a_filter() {
    let router = router();
    let id = create_set(&router, "field-types").await;

    assert!(
        MATCH_FIELD_TYPE_VALUES.len() <= MAX_XSS_MATCH_TUPLES,
        "the table must fit one set's filter quota: {} types, quota {MAX_XSS_MATCH_TUPLES}",
        MATCH_FIELD_TYPE_VALUES.len()
    );

    let mut expected: Vec<(String, Option<String>, String)> = Vec::new();
    for (index, field_type) in MATCH_FIELD_TYPE_VALUES.iter().enumerate() {
        let transformation = TEXT_TRANSFORMATION_VALUES[index % TEXT_TRANSFORMATION_VALUES.len()];
        let data = data_for(field_type);
        let (status, body) = apply_updates(
            &router,
            &id,
            vec![update("INSERT", tuple(field_type, data, transformation))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {field_type}/{transformation}: INSERT expected 200, got {status}: {body}"
        );
        expected.push((
            (*field_type).to_string(),
            data.map(str::to_string),
            transformation.to_string(),
        ));
        let stored = filters(&router, &id).await;
        assert_eq!(
            stored, expected,
            "case {field_type}/{transformation}: GetXssMatchSet must report every inserted \
             filter in insertion order, got {stored:?}"
        );
    }

    // Deleting the filters one at a time leaves the rest exactly as they were.
    while let Some((field_type, data, transformation)) = expected.pop() {
        let (status, body) = apply_updates(
            &router,
            &id,
            vec![update(
                "DELETE",
                tuple(&field_type, data.as_deref(), &transformation),
            )],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {field_type}/{transformation}: DELETE expected 200, got {status}: {body}"
        );
        let stored = filters(&router, &id).await;
        assert_eq!(
            stored, expected,
            "case {field_type}/{transformation}: the remaining filters must be untouched, \
             got {stored:?}"
        );
    }
}

/// A filter's identity is the part of the request it inspects and the transformation
/// applied to it — not the spelling of the header or parameter name, which `FieldToMatch`
/// documents as "not case sensitive". An update that would leave the set as it was is
/// refused: "the operation failed because there was nothing to do".
#[tokio::test]
async fn a_filter_is_identified_by_its_field_and_transformation_not_by_a_names_spelling() {
    let router = router();
    let id = create_set(&router, "identity").await;

    let (status, body) = apply_updates(
        &router,
        &id,
        vec![update("INSERT", tuple("HEADER", Some("User-Agent"), "NONE"))],
    )
    .await;
    assert_eq!(
        status, 200,
        "case first insert: expected 200, got {status}: {body}"
    );

    // Every spelling of that header name is the same filter, so inserting it again has
    // nothing to do.
    for (label, data) in [
        ("upper case", "USER-AGENT"),
        ("lower case", "user-agent"),
        ("mixed case", "User-agent"),
        ("as written", "User-Agent"),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateXssMatchSet",
            &json!({
                "XssMatchSetId": id,
                "ChangeToken": token,
                "Updates": [update("INSERT", tuple("HEADER", Some(data), "NONE"))],
            }),
            "WAFInvalidOperationException",
            label,
        )
        .await;
    }

    // A different transformation, a different name, or a different field is a different
    // filter.
    for (label, filter) in [
        (
            "same name, other transformation",
            tuple("HEADER", Some("User-Agent"), "LOWERCASE"),
        ),
        ("other name", tuple("HEADER", Some("Referer"), "NONE")),
        (
            "other field taking a name",
            tuple("SINGLE_QUERY_ARG", Some("UserName"), "NONE"),
        ),
    ] {
        let (status, body) = apply_updates(&router, &id, vec![update("INSERT", filter)]).await;
        assert_eq!(
            status, 200,
            "case {label}: a distinct filter must be accepted, got {status}: {body}"
        );
    }

    // The parameter name of a SINGLE_QUERY_ARG filter is case-insensitive in the same way.
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": id,
            "ChangeToken": token,
            "Updates": [update("INSERT", tuple("SINGLE_QUERY_ARG", Some("username"), "NONE"))],
        }),
        "WAFInvalidOperationException",
        "query-arg name in another case",
    )
    .await;

    // A DELETE matches under any spelling of the name, and deleting it twice has nothing
    // to do.
    let (status, body) = apply_updates(
        &router,
        &id,
        vec![update("DELETE", tuple("HEADER", Some("USER-AGENT"), "NONE"))],
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete under another spelling: expected 200, got {status}: {body}"
    );
    let stored = filters(&router, &id).await;
    assert!(
        !stored.contains(&(
            "HEADER".to_string(),
            Some("User-Agent".to_string()),
            "NONE".to_string()
        )),
        "case delete under another spelling: the filter must be gone, got {stored:?}"
    );
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": id,
            "ChangeToken": token,
            "Updates": [update("DELETE", tuple("HEADER", Some("user-agent"), "NONE"))],
        }),
        "WAFInvalidOperationException",
        "delete a filter the set no longer holds",
    )
    .await;
}

/// `FieldToMatch.Data` must be present exactly for the types that name a header or a
/// query-string parameter, and absent for every other type; the mismatch is refused with
/// the modelled `WAFInvalidParameterException`.
///
/// The table is every `MatchFieldType` the model declares, each tried with and without
/// `Data`.
#[tokio::test]
async fn data_is_required_exactly_for_the_types_that_name_one() {
    let router = router();

    for field_type in MATCH_FIELD_TYPE_VALUES {
        for (label, data) in [("with Data", Some("User-Agent")), ("without Data", None)] {
            let id = create_set(&router, &format!("{field_type}-{label}")).await;
            let accepted = TYPES_TAKING_DATA.contains(field_type) == data.is_some();
            let updates = vec![update("INSERT", tuple(field_type, data, "NONE"))];
            if accepted {
                let (status, body) = apply_updates(&router, &id, updates).await;
                assert_eq!(
                    status, 200,
                    "case {field_type} {label}: expected 200, got {status}: {body}"
                );
                let stored = filters(&router, &id).await;
                assert_eq!(
                    stored,
                    vec![(
                        (*field_type).to_string(),
                        data.map(str::to_string),
                        "NONE".to_string()
                    )],
                    "case {field_type} {label}: the stored filter must be the one inserted, \
                     got {stored:?}"
                );
            } else {
                let token = change_token(&router).await;
                let body = expect_error(
                    &router,
                    "UpdateXssMatchSet",
                    &json!({
                        "XssMatchSetId": id,
                        "ChangeToken": token,
                        "Updates": updates,
                    }),
                    "WAFInvalidParameterException",
                    &format!("{field_type} {label}"),
                )
                .await;
                assert_eq!(
                    body["reason"].as_str(),
                    Some("ILLEGAL_COMBINATION"),
                    "case {field_type} {label}: expected the ILLEGAL_COMBINATION reason, \
                     got {body}"
                );
                assert_eq!(
                    body["parameter"].as_str(),
                    Some(*field_type),
                    "case {field_type} {label}: the refusal must name the offending value, \
                     got {body}"
                );
                let stored = filters(&router, &id).await;
                assert!(
                    stored.is_empty(),
                    "case {field_type} {label}: a refused update must store nothing, \
                     got {stored:?}"
                );
            }
        }
    }
}

/// An `UpdateXssMatchSet` request is all-or-nothing: a request refused part-way through
/// changes no filter and consumes no change token, so the caller can resubmit with the
/// same token.
#[tokio::test]
async fn a_refused_update_changes_nothing_and_leaves_the_change_token_unused() {
    let router = router();
    let id = create_set(&router, "atomic").await;
    let token = change_token(&router).await;

    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": id,
            "ChangeToken": token.clone(),
            "Updates": [
                update("INSERT", tuple("URI", None, "NONE")),
                update("INSERT", tuple("HEADER", None, "NONE")),
            ],
        }),
        "WAFInvalidParameterException",
        "second update invalid",
    )
    .await;
    let stored = filters(&router, &id).await;
    assert!(
        stored.is_empty(),
        "case second update invalid: the first update must not have been applied, got {stored:?}"
    );

    let applied = ok_call(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": id,
            "ChangeToken": token.clone(),
            "Updates": [update("INSERT", tuple("URI", None, "NONE"))],
        }),
    )
    .await;
    assert_eq!(
        applied["ChangeToken"].as_str(),
        Some(token.as_str()),
        "case resubmit: the token the refused request left unused must still work, got {applied}"
    );
    let stored = filters(&router, &id).await;
    assert_eq!(
        stored,
        vec![("URI".to_string(), None, "NONE".to_string())],
        "case resubmit: the accepted update must be stored, got {stored:?}"
    );

    // "Each create, update, or delete request must use a unique change token."
    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": id,
            "ChangeToken": token,
            "Updates": [update("INSERT", tuple("BODY", None, "NONE"))],
        }),
        "WAFStaleDataException",
        "token reused after it was consumed",
    )
    .await;

    // An update against a container that does not exist is refused as such.
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": "00000000-0000-0000-0000-000000000000",
            "ChangeToken": token,
            "Updates": [update("INSERT", tuple("URI", None, "NONE"))],
        }),
        "WAFNonexistentContainerException",
        "update a set that does not exist",
    )
    .await;
}

/// One set holds at most the published number of filters, and a set that still holds any
/// cannot be deleted: "You can't delete an XssMatchSet if it's still used in any Rules or
/// if it still contains any XssMatchTuple objects."
#[tokio::test]
async fn a_set_is_bounded_and_must_be_emptied_before_it_is_deleted() {
    let router = router();
    let id = create_set(&router, "bounded").await;

    // Distinct filters drawn from the model's two value sets, one more than fits.
    let rows = distinct_rows(MAX_XSS_MATCH_TUPLES + 1);
    assert_eq!(
        rows.len(),
        MAX_XSS_MATCH_TUPLES + 1,
        "the table must hold one filter more than the quota, got {}",
        rows.len()
    );

    let accepted: Vec<Value> = rows[..MAX_XSS_MATCH_TUPLES]
        .iter()
        .map(|(field_type, transformation)| {
            update(
                "INSERT",
                tuple(field_type, data_for(field_type), transformation),
            )
        })
        .collect();
    let (status, body) = apply_updates(&router, &id, accepted.clone()).await;
    assert_eq!(
        status, 200,
        "case filling to the quota: expected 200, got {status}: {body}"
    );
    assert_eq!(
        filters(&router, &id).await.len(),
        MAX_XSS_MATCH_TUPLES,
        "case filling to the quota: the set must hold every accepted filter"
    );

    let (over_type, over_transformation) = rows[MAX_XSS_MATCH_TUPLES];
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({
            "XssMatchSetId": id,
            "ChangeToken": token,
            "Updates": [update(
                "INSERT",
                tuple(over_type, data_for(over_type), over_transformation),
            )],
        }),
        "WAFLimitsExceededException",
        "one filter past the quota",
    )
    .await;
    assert_eq!(
        filters(&router, &id).await.len(),
        MAX_XSS_MATCH_TUPLES,
        "case one filter past the quota: the refused insert must change nothing"
    );

    let token = change_token(&router).await;
    expect_error(
        &router,
        "DeleteXssMatchSet",
        &json!({"XssMatchSetId": id, "ChangeToken": token}),
        "WAFNonEmptyEntityException",
        "delete a set that still holds filters",
    )
    .await;

    // Emptied in one request, the set deletes, and then neither a read nor a second
    // delete finds it.
    let removals: Vec<Value> = rows[..MAX_XSS_MATCH_TUPLES]
        .iter()
        .map(|(field_type, transformation)| {
            update(
                "DELETE",
                tuple(field_type, data_for(field_type), transformation),
            )
        })
        .collect();
    let (status, body) = apply_updates(&router, &id, removals).await;
    assert_eq!(
        status, 200,
        "case emptying the set: expected 200, got {status}: {body}"
    );
    assert!(
        filters(&router, &id).await.is_empty(),
        "case emptying the set: every filter must be gone"
    );

    let token = change_token(&router).await;
    let deleted = ok_call(
        &router,
        "DeleteXssMatchSet",
        &json!({"XssMatchSetId": id, "ChangeToken": token.clone()}),
    )
    .await;
    assert_eq!(
        deleted["ChangeToken"].as_str(),
        Some(token.as_str()),
        "DeleteXssMatchSet must report the change token it consumed, got {deleted}"
    );

    expect_error(
        &router,
        "GetXssMatchSet",
        &json!({"XssMatchSetId": id}),
        "WAFNonexistentItemException",
        "read a deleted set",
    )
    .await;
    let token = change_token(&router).await;
    expect_error(
        &router,
        "DeleteXssMatchSet",
        &json!({"XssMatchSetId": id, "ChangeToken": token}),
        "WAFNonexistentItemException",
        "delete a deleted set",
    )
    .await;
}

/// How many updates one `UpdateXssMatchSet` request may carry is bounded by a constant
/// the request cannot choose, so a caller cannot pick the amount of work the service
/// does: a request at the bound is applied, one past it is refused with the modelled
/// `WAFLimitsExceededException` and stores nothing.
#[tokio::test]
async fn one_request_carries_at_most_the_bounded_number_of_updates() {
    let router = router();
    let id = create_set(&router, "update-bound").await;

    let rows = distinct_rows(MAX_XSS_MATCH_TUPLES);
    let inserts = rows.iter().map(|(field_type, transformation)| {
        update(
            "INSERT",
            tuple(field_type, data_for(field_type), transformation),
        )
    });
    let deletes = rows.iter().map(|(field_type, transformation)| {
        update(
            "DELETE",
            tuple(field_type, data_for(field_type), transformation),
        )
    });
    // At the bound: a request may fill a set and empty it again, which is the most any
    // request can usefully do.
    let mut updates: Vec<Value> = inserts.chain(deletes).collect();
    assert_eq!(
        updates.len(),
        MAX_XSS_MATCH_SET_UPDATES_PER_REQUEST,
        "the table must sit exactly at the per-request bound, got {}",
        updates.len()
    );
    let (status, body) = apply_updates(&router, &id, updates.clone()).await;
    assert_eq!(
        status, 200,
        "case at the per-request bound: expected 200, got {status}: {body}"
    );
    let stored = filters(&router, &id).await;
    assert!(
        stored.is_empty(),
        "case at the per-request bound: the request filled and emptied the set, got {stored:?}"
    );

    // One past the bound: refused before anything is applied.
    let (field_type, transformation) = rows[0];
    updates.push(update(
        "INSERT",
        tuple(field_type, data_for(field_type), transformation),
    ));
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateXssMatchSet",
        &json!({"XssMatchSetId": id, "ChangeToken": token, "Updates": updates}),
        "WAFLimitsExceededException",
        "one update past the per-request bound",
    )
    .await;
    let stored = filters(&router, &id).await;
    assert!(
        stored.is_empty(),
        "case one update past the per-request bound: nothing may be stored, got {stored:?}"
    );
}

/// `ListXssMatchSets` reports every stored set exactly once, in one stable order, under
/// every page size the model's `PaginationLimit` allows (`@range` 0..100) — and a page
/// size past that range is refused at the boundary.
#[tokio::test]
async fn list_pages_every_stored_set_exactly_once() {
    let router = router();

    let mut expected: Vec<(String, String)> = Vec::new();
    for index in 0..5 {
        let name = format!("listed-{index}");
        let id = create_set(&router, &name).await;
        expected.push((id, name));
    }
    expected.sort();

    // `Limit` omitted, the modelled default of 0, and the range's maximum all ask for
    // the service's own page size, which holds every set created here.
    for (label, request) in [
        ("omitted", json!({})),
        ("modelled default 0", json!({"Limit": 0})),
        ("range maximum", json!({"Limit": MAX_PAGE_SIZE})),
    ] {
        let body = ok_call(&router, "ListXssMatchSets", &request).await;
        let page = body["XssMatchSets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no XssMatchSets in {body}"));
        assert_eq!(
            page.len(),
            expected.len(),
            "case {label}: expected every set on one page, got {body}"
        );
        assert!(
            body["NextMarker"].is_null(),
            "case {label}: a complete page must carry no NextMarker, got {body}"
        );
    }

    // One past the modelled range is a boundary refusal, not a page.
    let (status, body) = call(
        &router,
        "ListXssMatchSets",
        &json!({"Limit": MAX_PAGE_SIZE + 1}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the range maximum: expected 400, got {status}: {body}"
    );

    for limit in 1..=expected.len() + 1 {
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        for _ in 0..expected.len() + 2 {
            let mut request = json!({"Limit": limit});
            if let Some(marker) = &marker {
                request["NextMarker"] = json!(marker);
            }
            let body = ok_call(&router, "ListXssMatchSets", &request).await;
            let page = body["XssMatchSets"]
                .as_array()
                .unwrap_or_else(|| panic!("case limit {limit}: no XssMatchSets in {body}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case limit {limit}: a page must hold at most Limit sets, got {body}"
            );
            for summary in &page {
                seen.push((
                    summary["XssMatchSetId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    summary["Name"].as_str().unwrap_or_default().to_string(),
                ));
            }
            marker = body["NextMarker"].as_str().map(str::to_string);
            if marker.is_none() {
                break;
            }
        }
        assert!(
            marker.is_none(),
            "case limit {limit}: paging did not terminate, last marker {marker:?}"
        );
        assert_eq!(
            seen, expected,
            "case limit {limit}: paging must report every stored set exactly once, got {seen:?}"
        );
    }

    // A deleted set leaves the listing.
    let (gone_id, _) = expected.remove(0);
    let token = change_token(&router).await;
    ok_call(
        &router,
        "DeleteXssMatchSet",
        &json!({"XssMatchSetId": gone_id, "ChangeToken": token}),
    )
    .await;
    let body = ok_call(&router, "ListXssMatchSets", &json!({})).await;
    let listed: Vec<String> = body["XssMatchSets"]
        .as_array()
        .unwrap_or_else(|| panic!("no XssMatchSets in {body}"))
        .iter()
        .map(|summary| {
            summary["XssMatchSetId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        !listed.contains(&gone_id),
        "case after delete: {gone_id} must be gone from the listing, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        expected.len(),
        "case after delete: every other set must still be listed, got {listed:?}"
    );
}
