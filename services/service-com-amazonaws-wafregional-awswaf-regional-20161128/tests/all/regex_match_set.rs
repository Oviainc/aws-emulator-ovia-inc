// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `RegexMatchSet` lifecycle, end to end over the wire:
//! `CreateRegexMatchSet`, `UpdateRegexMatchSet`, `GetRegexMatchSet` — together with the
//! `RegexPatternSet` half each filter points at (`CreateRegexPatternSet`,
//! `UpdateRegexPatternSet`, `GetRegexPatternSet`).
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`, awsJson1_1), so what these tests
//! assert is what an SDK client sees: each response is built from the persisted record,
//! not from the request.
//!
//! The rule under test, from the operations' own documentation: `CreateRegexMatchSet`
//! stores "a RegexMatchSet that contains no RegexMatchTuple objects", each
//! `RegexMatchSetUpdate` applies its `ChangeAction` to that set's filter collection —
//! `INSERT` adds the update's `RegexMatchTuple`, `DELETE` removes it — and every later
//! `GetRegexMatchSet` of the same id reports the collection those updates left behind,
//! whole: the part of the request to inspect, the transformation to apply first and the
//! `RegexPatternSetId` the filter searches for.
//!
//! The tables below are the class that rule covers: their rows span every
//! `MatchFieldType` and every `TextTransformation` the model declares, the presence of
//! `FieldToMatch.Data`, the number of updates one request carries and the mix of actions
//! in it. The refusals the same rule implies are tables too — an update that would leave
//! the set as it was, a set id nothing is stored under, a filter naming a pattern set
//! nothing is stored under, a reused change token, and an insert past the set's own
//! bound.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    MATCH_FIELD_TYPE_VALUES, TEXT_TRANSFORMATION_VALUES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_REGEX_MATCH_TUPLES, MAX_REGEX_MATCH_UPDATES_PER_REQUEST,
};

// The wire harness (router, `call`, `ok_call`, the error helpers, the change-token
// protocol) is shared with every other suite in this crate; see `tests/all/wire.rs`.
use crate::wire::{
    call, change_token, error_code, error_message, expect_error, is_resource_id, ok_call, router,
    token_status,
};

/// One row of the filter table: the members a `RegexMatchTuple` carries besides the
/// `RegexPatternSetId` (which names a pattern set created by the test).
#[derive(Debug, Clone)]
struct FilterCase {
    label: &'static str,
    field_type: &'static str,
    data: Option<&'static str>,
    transformation: &'static str,
}

impl FilterCase {
    /// This row as the wire `RegexMatchTuple`, pointing at `pattern_set_id`.
    fn tuple(&self, pattern_set_id: &str) -> Value {
        let mut field_to_match = json!({"Type": self.field_type});
        if let Some(data) = self.data {
            field_to_match["Data"] = json!(data);
        }
        json!({
            "FieldToMatch": field_to_match,
            "TextTransformation": self.transformation,
            "RegexPatternSetId": pattern_set_id,
        })
    }

    /// This row as one `RegexMatchSetUpdate`.
    fn update(&self, action: &str, pattern_set_id: &str) -> Value {
        update(action, &self.tuple(pattern_set_id))
    }
}

/// One `RegexMatchSetUpdate` carrying `tuple`.
fn update(action: &str, tuple: &Value) -> Value {
    json!({"Action": action, "RegexMatchTuple": tuple})
}

/// The table: every `MatchFieldType` and every `TextTransformation` the model declares
/// appears in at least one row (the lists come from the model's own
/// `MATCH_FIELD_TYPE_VALUES` / `TEXT_TRANSFORMATION_VALUES`, not from a hand-written
/// catalogue — see the coverage assertion in
/// [`an_inserted_filter_is_read_back_and_a_deleted_one_is_gone`]), and the rows differ
/// in whether `FieldToMatch.Data` names the part of the request to inspect.
fn filter_table() -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "URI, untransformed",
            field_type: "URI",
            data: None,
            transformation: "NONE",
        },
        FilterCase {
            label: "QUERY_STRING, lowercased",
            field_type: "QUERY_STRING",
            data: None,
            transformation: "LOWERCASE",
        },
        FilterCase {
            label: "HEADER names its Data",
            field_type: "HEADER",
            data: Some("User-Agent"),
            transformation: "COMPRESS_WHITE_SPACE",
        },
        FilterCase {
            label: "SINGLE_QUERY_ARG names its Data",
            field_type: "SINGLE_QUERY_ARG",
            data: Some("UserName"),
            transformation: "URL_DECODE",
        },
        FilterCase {
            label: "ALL_QUERY_ARGS, html-entity decoded",
            field_type: "ALL_QUERY_ARGS",
            data: None,
            transformation: "HTML_ENTITY_DECODE",
        },
        FilterCase {
            label: "BODY, command-line normalised",
            field_type: "BODY",
            data: None,
            transformation: "CMD_LINE",
        },
        FilterCase {
            label: "METHOD, untransformed",
            field_type: "METHOD",
            data: None,
            transformation: "NONE",
        },
    ]
}

/// Create a `RegexPatternSet` holding `patterns` and return its `RegexPatternSetId` —
/// the resource a `RegexMatchTuple` points at.
async fn create_pattern_set(
    router: &MultiServiceRouter,
    name: &str,
    patterns: &[&str],
) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRegexPatternSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    let id = body["RegexPatternSet"]["RegexPatternSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRegexPatternSet returned no RegexPatternSetId: {body}"))
        .to_string();
    if !patterns.is_empty() {
        let token = change_token(router).await;
        let updates: Vec<Value> = patterns
            .iter()
            .map(|pattern| json!({"Action": "INSERT", "RegexPatternString": pattern}))
            .collect();
        ok_call(
            router,
            "UpdateRegexPatternSet",
            &json!({
                "RegexPatternSetId": id,
                "ChangeToken": token,
                "Updates": updates,
            }),
        )
        .await;
    }
    id
}

/// Create a `RegexMatchSet` named `name` and return `(RegexMatchSetId, response body)`.
async fn create_match_set(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRegexMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    let id = body["RegexMatchSet"]["RegexMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRegexMatchSet returned no RegexMatchSetId: {body}"))
        .to_string();
    (id, body)
}

/// `UpdateRegexMatchSet` with a fresh change token: `(status, body, token)`.
async fn apply(
    router: &MultiServiceRouter,
    set_id: &str,
    updates: &[Value],
) -> (u16, Value, String) {
    let token = change_token(router).await;
    let (status, body) = call(
        router,
        "UpdateRegexMatchSet",
        &json!({"RegexMatchSetId": set_id, "ChangeToken": token, "Updates": updates}),
    )
    .await;
    (status, body, token)
}

/// `UpdateRegexMatchSet` that must answer 200, and the token it consumed.
async fn apply_ok(
    router: &MultiServiceRouter,
    set_id: &str,
    updates: &[Value],
) -> (Value, String) {
    let (status, body, token) = apply(router, set_id, updates).await;
    assert_eq!(
        status, 200,
        "UpdateRegexMatchSet expected 200, got {status}: {body}"
    );
    (body, token)
}

/// The `RegexMatchTuples` of the stored set, read back through `GetRegexMatchSet`.
async fn stored_filters(router: &MultiServiceRouter, set_id: &str) -> Vec<Value> {
    let body = ok_call(
        router,
        "GetRegexMatchSet",
        &json!({"RegexMatchSetId": set_id}),
    )
    .await;
    assert_eq!(
        body["RegexMatchSet"]["RegexMatchSetId"].as_str(),
        Some(set_id),
        "GetRegexMatchSet must report the set it was asked for, got {body}"
    );
    body["RegexMatchSet"]["RegexMatchTuples"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("GetRegexMatchSet must carry the filter collection, even empty: {body}")
        })
        .clone()
}

/// An `INSERT` of each filter in the table is reported whole by a later
/// `GetRegexMatchSet`, and a `DELETE` of it is reported as gone.
#[tokio::test]
async fn an_inserted_filter_is_read_back_and_a_deleted_one_is_gone() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["B[a@]dB[o0]t"]).await;

    // The table is the class, so it must span the model's own value lists.
    let covered_fields: BTreeSet<&str> = filter_table()
        .iter()
        .map(|case| case.field_type)
        .collect();
    let covered_transformations: BTreeSet<&str> = filter_table()
        .iter()
        .map(|case| case.transformation)
        .collect();
    for field_type in MATCH_FIELD_TYPE_VALUES {
        assert!(
            covered_fields.contains(field_type),
            "the filter table must cover MatchFieldType {field_type}, covers {covered_fields:?}"
        );
    }
    for transformation in TEXT_TRANSFORMATION_VALUES {
        assert!(
            covered_transformations.contains(transformation),
            "the filter table must cover TextTransformation {transformation}, \
             covers {covered_transformations:?}"
        );
    }

    for case in filter_table() {
        let label = case.label;
        let (set_id, created) = create_match_set(&router, &format!("insert-{label}")).await;
        assert!(
            is_resource_id(&set_id),
            "case {label:?}: a minted RegexMatchSetId is UUID-shaped, got {set_id:?}"
        );
        assert_eq!(
            created["RegexMatchSet"]["RegexMatchTuples"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {label:?}: a new RegexMatchSet contains no RegexMatchTuple objects, \
             got {created}"
        );
        assert!(
            stored_filters(&router, &set_id).await.is_empty(),
            "case {label:?}: a new set holds no filters"
        );

        let tuple = case.tuple(&pattern_set);
        let (body, token) = apply_ok(&router, &set_id, &[update("INSERT", &tuple)]).await;
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label:?}: UpdateRegexMatchSet must echo the change token it consumed, \
             got {body}"
        );

        let after_insert = stored_filters(&router, &set_id).await;
        assert_eq!(
            after_insert,
            vec![tuple.clone()],
            "case {label:?}: expected the inserted filter to be read back whole, \
             got {after_insert:?}"
        );
        // A read changes nothing a later caller sees.
        let again = stored_filters(&router, &set_id).await;
        assert_eq!(
            again, after_insert,
            "case {label:?}: a second read must report the same filters, got {again:?}"
        );

        apply_ok(&router, &set_id, &[update("DELETE", &tuple)]).await;
        let after_delete = stored_filters(&router, &set_id).await;
        assert!(
            after_delete.is_empty(),
            "case {label:?}: expected the deleted filter to be gone, got {after_delete:?}"
        );
    }
}

/// One request carries any number of updates, applied in the order given: the set a
/// later read reports is the one those updates leave behind. Each row starts from the set
/// the previous one left, so the rows differ in count and in their mix of actions.
#[tokio::test]
async fn a_batch_of_updates_applies_in_the_order_it_is_given() {
    let router = router();
    let first_patterns = create_pattern_set(&router, "first", &["^admin-[0-9]+$"]).await;
    let second_patterns = create_pattern_set(&router, "second", &[r"\p{L}+"]).await;
    let table = filter_table();
    let uri = table[0].tuple(&first_patterns);
    let query = table[1].tuple(&first_patterns);
    let header = table[2].tuple(&second_patterns);

    let rows: Vec<(&str, Vec<Value>, Vec<&Value>)> = vec![
        (
            "three inserts in one request",
            vec![
                update("INSERT", &uri),
                update("INSERT", &query),
                update("INSERT", &header),
            ],
            vec![&uri, &query, &header],
        ),
        (
            "one delete",
            vec![update("DELETE", &query)],
            vec![&uri, &header],
        ),
        (
            "a delete and an insert in one request",
            vec![update("DELETE", &uri), update("INSERT", &query)],
            vec![&header, &query],
        ),
        (
            "delete and re-insert the same filter",
            vec![update("DELETE", &header), update("INSERT", &header)],
            vec![&query, &header],
        ),
        (
            "empty the set",
            vec![update("DELETE", &query), update("DELETE", &header)],
            vec![],
        ),
    ];

    let (set_id, _) = create_match_set(&router, "batched").await;
    for (label, updates, expected) in rows {
        apply_ok(&router, &set_id, &updates).await;
        let actual = stored_filters(&router, &set_id).await;
        let expected: Vec<Value> = expected.into_iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A filter's identity is the part of the request it inspects, the transformation applied
/// first and the pattern set it points at: two filters differing in any of those are two
/// filters, and `FieldToMatch.Data` — a header or query-argument name, which "is not case
/// sensitive" — is one name however it is spelled.
#[tokio::test]
async fn a_filter_is_identified_by_its_field_transformation_and_pattern_set() {
    let router = router();
    let first_patterns = create_pattern_set(&router, "first", &["^a$"]).await;
    let second_patterns = create_pattern_set(&router, "second", &["^b$"]).await;
    let header = FilterCase {
        label: "HEADER/User-Agent/NONE",
        field_type: "HEADER",
        data: Some("User-Agent"),
        transformation: "NONE",
    };
    let (set_id, _) = create_match_set(&router, "identity").await;
    apply_ok(
        &router,
        &set_id,
        &[header.update("INSERT", &first_patterns)],
    )
    .await;

    // Rows: a filter differing from the stored one in exactly one member, each of which
    // is therefore a DIFFERENT filter an INSERT must store.
    let distinct: Vec<(&str, Value)> = vec![
        (
            "another Data value",
            FilterCase {
                data: Some("Referer"),
                ..header.clone()
            }
            .tuple(&first_patterns),
        ),
        (
            "another TextTransformation",
            FilterCase {
                transformation: "LOWERCASE",
                ..header.clone()
            }
            .tuple(&first_patterns),
        ),
        (
            "another FieldToMatch Type",
            FilterCase {
                field_type: "URI",
                data: None,
                ..header.clone()
            }
            .tuple(&first_patterns),
        ),
        (
            "another RegexPatternSetId",
            header.tuple(&second_patterns),
        ),
    ];
    let mut expected = vec![header.tuple(&first_patterns)];
    for (label, tuple) in distinct {
        apply_ok(&router, &set_id, &[update("INSERT", &tuple)]).await;
        expected.push(tuple);
        let actual = stored_filters(&router, &set_id).await;
        assert_eq!(
            actual, expected,
            "case {label:?}: a filter differing in one member is another filter, \
             got {actual:?}"
        );
    }

    // ... while a `Data` that differs only in case names the same header, so inserting it
    // again is the update that changes nothing.
    let same_header = FilterCase {
        data: Some("user-agent"),
        ..header.clone()
    }
    .tuple(&first_patterns);
    let (status, body, _) = apply(&router, &set_id, &[update("INSERT", &same_header)]).await;
    assert_eq!(
        status, 400,
        "case \"Data in another case\": a header name is not case sensitive, so this \
         filter is already stored, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("WAFInvalidOperationException"),
        "case \"Data in another case\": expected WAFInvalidOperationException, got {} ({body})",
        error_code(&body)
    );
    // And a DELETE naming it in that other case removes the stored filter.
    apply_ok(&router, &set_id, &[update("DELETE", &same_header)]).await;
    let actual = stored_filters(&router, &set_id).await;
    assert_eq!(
        actual,
        expected[1..].to_vec(),
        "case \"Data in another case\": the DELETE must remove the filter stored under \
         the other spelling, got {actual:?}"
    );
}

/// "The operation failed because there was nothing to do": an `INSERT` of a filter the
/// set already holds and a `DELETE` of one it does not are each refused, and neither
/// changes the set or consumes the token it carried.
#[tokio::test]
async fn an_update_that_would_change_nothing_is_refused() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["^held$"]).await;
    let table = filter_table();
    let held = table[0].tuple(&pattern_set);
    let absent = table[1].tuple(&pattern_set);
    let (set_id, _) = create_match_set(&router, "nothing-to-do").await;
    apply_ok(&router, &set_id, &[update("INSERT", &held)]).await;

    for (label, updates) in [
        ("insert a filter the set holds", vec![update("INSERT", &held)]),
        (
            "delete a filter the set does not hold",
            vec![update("DELETE", &absent)],
        ),
        (
            "a batch whose last update changes nothing",
            vec![update("INSERT", &absent), update("INSERT", &held)],
        ),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateRegexMatchSet",
            &json!({"RegexMatchSetId": set_id, "ChangeToken": token, "Updates": updates}),
            "WAFInvalidOperationException",
            label,
        )
        .await;

        let actual = stored_filters(&router, &set_id).await;
        assert_eq!(
            actual,
            vec![held.clone()],
            "case {label:?}: a refused update must leave the set as it was, got {actual:?}"
        );
        let reported = token_status(&router, &token).await;
        assert_eq!(
            reported, "PROVISIONED",
            "case {label:?}: a refused update must leave its change token unconsumed, \
             got {reported}"
        );
    }
}

/// The filters go INTO a set, so a `RegexMatchSetId` nothing is stored under is the
/// modelled `WAFNonexistentContainerException` — including the id of another resource
/// type, since each type is its own keyspace. `GetRegexMatchSet` of such an id is the
/// modelled `WAFNonexistentItemException`: there is no referenced object to report.
#[tokio::test]
async fn an_id_no_regex_match_set_is_stored_under_is_refused() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["^any$"]).await;
    // One stored match set, so the refusals below are about the id, not an empty store.
    let (stored, _) = create_match_set(&router, "stored").await;
    let tuple = filter_table()[0].tuple(&pattern_set);

    for (label, set_id) in [
        (
            "a uuid-shaped id nothing was created under",
            "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
        ),
        ("another resource type's id", pattern_set.clone()),
        ("an opaque non-uuid id", "not-an-id".to_string()),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateRegexMatchSet",
            &json!({
                "RegexMatchSetId": set_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", &tuple)],
            }),
            "WAFNonexistentContainerException",
            label,
        )
        .await;
        expect_error(
            &router,
            "GetRegexMatchSet",
            &json!({"RegexMatchSetId": set_id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
    }

    assert!(
        stored_filters(&router, &stored).await.is_empty(),
        "the refusals must have stored nothing"
    );
}

/// A filter names the `RegexPatternSet` it searches for, which this operation documents
/// as created first ("Create a RegexPatternSet […] Submit an UpdateRegexMatchSet request
/// to specify […] the identifier of the RegexPatternSet"), so an `INSERT` naming an id
/// nothing is stored under is the modelled `WAFNonexistentItemException` — the referenced
/// object does not exist — and stores nothing.
#[tokio::test]
async fn an_insert_naming_an_unknown_pattern_set_is_refused() {
    let router = router();
    let (set_id, _) = create_match_set(&router, "dangling-reference").await;
    let case = &filter_table()[0];

    for (label, pattern_set_id) in [
        (
            "a uuid-shaped id nothing was created under",
            "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
        ),
        ("the match set's own id", set_id.clone()),
        ("an opaque non-uuid id", "not-an-id".to_string()),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateRegexMatchSet",
            &json!({
                "RegexMatchSetId": set_id,
                "ChangeToken": token,
                "Updates": [case.update("INSERT", &pattern_set_id)],
            }),
            "WAFNonexistentItemException",
            label,
        )
        .await;

        let actual = stored_filters(&router, &set_id).await;
        assert!(
            actual.is_empty(),
            "case {label:?}: a refused insert must store nothing, got {actual:?}"
        );
        let reported = token_status(&router, &token).await;
        assert_eq!(
            reported, "PROVISIONED",
            "case {label:?}: a refused update must leave its change token unconsumed, \
             got {reported}"
        );
    }
}

/// "Each create, update, or delete request must use a unique change token", so a token an
/// update already consumed — and one this service never minted — is stale data, and
/// neither request changes the set.
#[tokio::test]
async fn an_update_consumes_its_change_token_exactly_once() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["^held$"]).await;
    let table = filter_table();
    let held = table[0].tuple(&pattern_set);
    let never = table[1].tuple(&pattern_set);
    let (set_id, _) = create_match_set(&router, "token-protocol").await;
    let (_, token) = apply_ok(&router, &set_id, &[update("INSERT", &held)]).await;

    for (label, stale) in [
        ("the token the update consumed", token.clone()),
        (
            "a token never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "UpdateRegexMatchSet",
            &json!({
                "RegexMatchSetId": set_id,
                "ChangeToken": stale,
                "Updates": [update("INSERT", &never)],
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
    }

    let actual = stored_filters(&router, &set_id).await;
    assert_eq!(
        actual,
        vec![held],
        "a refused update must leave the set as it was, got {actual:?}"
    );
}

/// A set holds at most `MAX_REGEX_MATCH_TUPLES` filters and one request carries at most
/// `MAX_REGEX_MATCH_UPDATES_PER_REQUEST` updates: past either, the modelled
/// `WAFLimitsExceededException`, with nothing further stored.
#[tokio::test]
async fn updates_past_the_modelled_bounds_are_refused() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["^bounded$"]).await;
    let (set_id, _) = create_match_set(&router, "bounded").await;

    // `MAX_REGEX_MATCH_TUPLES` distinct filters: one per header name.
    let filling: Vec<Value> = (0..MAX_REGEX_MATCH_TUPLES)
        .map(|index| {
            update(
                "INSERT",
                &json!({
                    "FieldToMatch": {"Type": "HEADER", "Data": format!("x-header-{index}")},
                    "TextTransformation": "NONE",
                    "RegexPatternSetId": pattern_set,
                }),
            )
        })
        .collect();
    apply_ok(&router, &set_id, &filling).await;
    let full = stored_filters(&router, &set_id).await;
    assert_eq!(
        full.len(),
        MAX_REGEX_MATCH_TUPLES,
        "a set must hold every filter inserted up to its bound, got {full:?}"
    );

    let rows: Vec<(String, Vec<Value>)> = vec![
        (
            format!("one past the {MAX_REGEX_MATCH_TUPLES}-filter bound"),
            vec![update(
                "INSERT",
                &json!({
                    "FieldToMatch": {"Type": "URI"},
                    "TextTransformation": "NONE",
                    "RegexPatternSetId": pattern_set,
                }),
            )],
        ),
        (
            format!(
                "one past the {MAX_REGEX_MATCH_UPDATES_PER_REQUEST} updates a request \
                 may carry"
            ),
            (0..=MAX_REGEX_MATCH_UPDATES_PER_REQUEST)
                .map(|index| {
                    update(
                        "DELETE",
                        &json!({
                            "FieldToMatch": {
                                "Type": "HEADER",
                                "Data": format!("x-header-{index}"),
                            },
                            "TextTransformation": "NONE",
                            "RegexPatternSetId": pattern_set,
                        }),
                    )
                })
                .collect(),
        ),
    ];

    for (label, updates) in rows {
        let (status, body, token) = apply(&router, &set_id, &updates).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("WAFLimitsExceededException"),
            "case {label:?}: expected WAFLimitsExceededException, got {} ({body})",
            error_code(&body)
        );

        let actual = stored_filters(&router, &set_id).await;
        assert_eq!(
            actual, full,
            "case {label:?}: a refused update must leave the set as it was, got {actual:?}"
        );
        let reported = token_status(&router, &token).await;
        assert_eq!(
            reported, "PROVISIONED",
            "case {label:?}: a refused update must leave its change token unconsumed, \
             got {reported}"
        );
    }
}

/// `FieldToMatch.Data` "names the header […] the parameter" for `HEADER` and
/// `SINGLE_QUERY_ARG`, and "if the value of Type is any other value, omit Data": a filter
/// whose `Type` and `Data` cannot hold together is refused, with a message naming the
/// member and the constraint, and stores nothing.
#[tokio::test]
async fn a_field_type_and_data_that_cannot_hold_together_is_refused() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["^any$"]).await;
    let (set_id, _) = create_match_set(&router, "field-and-data").await;

    // Rows: every `MatchFieldType` that takes a `Data`, with none; and types that take
    // none, with one.
    let rows: Vec<(&str, Value)> = vec![
        (
            "HEADER without Data",
            json!({
                "FieldToMatch": {"Type": "HEADER"},
                "TextTransformation": "NONE",
                "RegexPatternSetId": pattern_set,
            }),
        ),
        (
            "SINGLE_QUERY_ARG without Data",
            json!({
                "FieldToMatch": {"Type": "SINGLE_QUERY_ARG"},
                "TextTransformation": "LOWERCASE",
                "RegexPatternSetId": pattern_set,
            }),
        ),
        (
            "URI with Data",
            json!({
                "FieldToMatch": {"Type": "URI", "Data": "User-Agent"},
                "TextTransformation": "NONE",
                "RegexPatternSetId": pattern_set,
            }),
        ),
        (
            "ALL_QUERY_ARGS with Data",
            json!({
                "FieldToMatch": {"Type": "ALL_QUERY_ARGS", "Data": "UserName"},
                "TextTransformation": "NONE",
                "RegexPatternSetId": pattern_set,
            }),
        ),
    ];

    for (label, tuple) in rows {
        let (status, body, token) = apply(&router, &set_id, &[update("INSERT", &tuple)]).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status}: {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("FieldToMatch") && message.contains("Data"),
            "case {label:?}: the refusal must name the member at fault, got {message:?}"
        );

        let actual = stored_filters(&router, &set_id).await;
        assert!(
            actual.is_empty(),
            "case {label:?}: a refused update must store nothing, got {actual:?}"
        );
        let reported = token_status(&router, &token).await;
        assert_eq!(
            reported, "PROVISIONED",
            "case {label:?}: a refused update must leave its change token unconsumed, \
             got {reported}"
        );
    }
}

/// `Name` carries no uniqueness constraint, so each `CreateRegexMatchSet` is its own
/// resource under its own minted id, and the filters written to one are not reported by
/// another.
#[tokio::test]
async fn each_created_set_is_its_own_resource() {
    let router = router();
    let pattern_set = create_pattern_set(&router, "patterns", &["^shared$"]).await;
    let tuple = filter_table()[0].tuple(&pattern_set);

    let (first, first_body) = create_match_set(&router, "same-name").await;
    let (second, second_body) = create_match_set(&router, "same-name").await;
    assert_ne!(
        first, second,
        "two creates with one Name are two resources, got {first} twice"
    );
    for (label, body, id) in [
        ("the first create", &first_body, &first),
        ("the second create", &second_body, &second),
    ] {
        assert_eq!(
            body["RegexMatchSet"]["Name"].as_str(),
            Some("same-name"),
            "case {label:?}: the response reports the stored Name, got {body}"
        );
        assert_eq!(
            body["RegexMatchSet"]["RegexMatchSetId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the response reports the id it stored the set under, got {body}"
        );
    }

    apply_ok(&router, &first, &[update("INSERT", &tuple)]).await;
    let written = stored_filters(&router, &first).await;
    assert_eq!(
        written,
        vec![tuple],
        "the set the update named must report the filter, got {written:?}"
    );
    let untouched = stored_filters(&router, &second).await;
    assert!(
        untouched.is_empty(),
        "the other set must be untouched, got {untouched:?}"
    );
}
