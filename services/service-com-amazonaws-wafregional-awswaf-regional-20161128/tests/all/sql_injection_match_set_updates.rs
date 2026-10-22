// SPDX-License-Identifier: CC0-1.0

//! `UpdateSqlInjectionMatchSet`, end to end over the wire.
//!
//! The rule under test: an update inserts or deletes the filters of ONE stored match
//! set, so an accepted request consumes its change token exactly once and leaves the
//! named set holding exactly the filters that survive the updates it carried, in
//! insertion order, readable through `GetSqlInjectionMatchSet`; an update naming a set
//! the store does not hold, an enum value outside the modelled set, a `FieldToMatch`
//! whose `Data` does not go with its `Type`, an insert of a filter the set already
//! holds, a delete of one it does not, or an insert past the per-set filter quota is
//! refused with the modelled error for that class and persists nothing.
//!
//! Each row of the tables below is one input along what that rule generalises over
//! (which part of a request is inspected, which text transformation, whether a `Data`
//! name is carried and how it is spelled, how many updates one request carries, how
//! many filters the set already holds, which id is named), so no assertion depends on
//! the one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The two value catalogs below are the model's own, generated from it — never
// hand-written: `MATCH_FIELD_TYPE_VALUES` is every `MatchFieldType` and
// `TEXT_TRANSFORMATION_VALUES` every `TextTransformation` the wire accepts.
use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    MATCH_FIELD_TYPE_VALUES, TEXT_TRANSFORMATION_VALUES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    match_field_takes_data, MAX_SQL_INJECTION_MATCH_SET_UPDATES_PER_REQUEST,
    MAX_SQL_INJECTION_MATCH_TUPLES,
};

// The wire harness, and the create/read helpers, are shared with the other suites in
// this crate.
use crate::sql_injection_match_set::{create_set, get_set};
use crate::wire::{call, change_token, expect_error, ok_call, router, token_status};

/// One `SqlInjectionMatchSetUpdate` on the wire.
fn update(action: &str, field_type: &str, data: Option<&str>, transformation: &str) -> Value {
    let field_to_match = match data {
        Some(data) => json!({"Type": field_type, "Data": data}),
        None => json!({"Type": field_type}),
    };
    json!({
        "Action": action,
        "SqlInjectionMatchTuple": {
            "FieldToMatch": field_to_match,
            "TextTransformation": transformation,
        }
    })
}

/// A filter as the assertions below compare them: `(Type, Data, TextTransformation)`.
type Filter = (String, Option<String>, String);

/// The filter one reported `SqlInjectionMatchTuple` names.
fn filter_of(tuple: &Value) -> Filter {
    (
        tuple["FieldToMatch"]["Type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        tuple["FieldToMatch"]["Data"].as_str().map(str::to_string),
        tuple["TextTransformation"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// One expected filter, in the same shape [`filter_of`] reports.
fn filter(field_type: &str, data: Option<&str>, transformation: &str) -> Filter {
    (
        field_type.to_string(),
        data.map(str::to_string),
        transformation.to_string(),
    )
}

/// Apply `updates` to `set_id` with a fresh change token, and answer
/// `(http status, body)`.
async fn apply_updates(
    router: &MultiServiceRouter,
    set_id: &str,
    updates: Vec<Value>,
) -> (u16, Value) {
    let token = change_token(router).await;
    call(
        router,
        "UpdateSqlInjectionMatchSet",
        &json!({
            "SqlInjectionMatchSetId": set_id,
            "ChangeToken": token,
            "Updates": updates,
        }),
    )
    .await
}

/// The filters `GetSqlInjectionMatchSet` reports for `set_id`, in stored order.
async fn filters(router: &MultiServiceRouter, set_id: &str) -> Vec<Filter> {
    let set = get_set(router, set_id).await;
    set["SqlInjectionMatchTuples"]
        .as_array()
        .unwrap_or_else(|| panic!("GetSqlInjectionMatchSet reported no tuple list: {set}"))
        .iter()
        .map(filter_of)
        .collect()
}

/// A `Data` name for `field_type`: one for the types that take a name, none for the
/// types whose documentation says to omit it.
fn data_for(field_type: &str, index: usize) -> Option<String> {
    match_field_takes_data(field_type).then(|| format!("Probe-Name-{index}"))
}

/// An INSERT of every `(MatchFieldType, TextTransformation)` pair the model declares is
/// persisted and read back verbatim.
///
/// The catalogs are the model's own (`MATCH_FIELD_TYPE_VALUES`,
/// `TEXT_TRANSFORMATION_VALUES`), so this covers every published value — including the
/// ones the issue's recorded request never names — and each pair goes into its own
/// freshly created set, so what comes back is that set's own stored filter.
#[tokio::test]
async fn an_insert_of_every_modelled_field_type_and_transformation_round_trips() {
    let router = router();

    for (index, field_type) in MATCH_FIELD_TYPE_VALUES.iter().enumerate() {
        for transformation in TEXT_TRANSFORMATION_VALUES {
            let label = format!("{field_type}/{transformation}");
            let data = data_for(field_type, index);
            let (set_id, _) = create_set(&router, &format!("catalog-{index}-{transformation}")).await;

            let (status, body) = apply_updates(
                &router,
                &set_id,
                vec![update("INSERT", field_type, data.as_deref(), transformation)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label}: the insert must be accepted, got {status}: {body}"
            );

            let stored = filters(&router, &set_id).await;
            assert_eq!(
                stored,
                vec![filter(field_type, data.as_deref(), transformation)],
                "case {label}: the set must hold exactly the inserted filter, got \
                 {stored:?}"
            );
        }
    }
}

/// Inserts accumulate in insertion order, across one request and across several, and a
/// delete removes only the filter it names.
#[tokio::test]
async fn inserts_accumulate_in_order_and_a_delete_removes_only_its_own_filter() {
    let router = router();
    let (set_id, _) = create_set(&router, "accumulate").await;

    // Three filters in ONE request: a type that takes no Data, a type that takes one,
    // and a third transformation on a third part of the request.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("INSERT", "QUERY_STRING", None, "URL_DECODE"),
            update("INSERT", "HEADER", Some("User-Agent"), "LOWERCASE"),
            update("INSERT", "BODY", None, "HTML_ENTITY_DECODE"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case three inserts in one request: expected 200, got {status}: {body}"
    );
    let expected = vec![
        filter("QUERY_STRING", None, "URL_DECODE"),
        filter("HEADER", Some("User-Agent"), "LOWERCASE"),
        filter("BODY", None, "HTML_ENTITY_DECODE"),
    ];
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored, expected,
        "case three inserts in one request: the filters must be stored in insertion \
         order, got {stored:?}"
    );

    // Two more in a SECOND request: appended after the ones already stored.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("INSERT", "URI", None, "CMD_LINE"),
            update(
                "INSERT",
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case two more inserts: expected 200, got {status}: {body}"
    );
    let expected = [
        expected,
        vec![
            filter("URI", None, "CMD_LINE"),
            filter("SINGLE_QUERY_ARG", Some("SalesRegion"), "COMPRESS_WHITE_SPACE"),
        ],
    ]
    .concat();
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored, expected,
        "case two more inserts: a second request appends to what is stored, got \
         {stored:?}"
    );

    // One DELETE: only the named filter goes, and the rest keep their order.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update("DELETE", "HEADER", Some("User-Agent"), "LOWERCASE")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case one delete: expected 200, got {status}: {body}"
    );
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![
            filter("QUERY_STRING", None, "URL_DECODE"),
            filter("BODY", None, "HTML_ENTITY_DECODE"),
            filter("URI", None, "CMD_LINE"),
            filter("SINGLE_QUERY_ARG", Some("SalesRegion"), "COMPRESS_WHITE_SPACE"),
        ],
        "case one delete: only the named filter may go, got {stored:?}"
    );

    // An INSERT and a DELETE in the SAME request, applied in order.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("DELETE", "URI", None, "CMD_LINE"),
            update("INSERT", "METHOD", None, "NONE"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case a delete and an insert together: expected 200, got {status}: {body}"
    );
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![
            filter("QUERY_STRING", None, "URL_DECODE"),
            filter("BODY", None, "HTML_ENTITY_DECODE"),
            filter("SINGLE_QUERY_ARG", Some("SalesRegion"), "COMPRESS_WHITE_SPACE"),
            filter("METHOD", None, "NONE"),
        ],
        "case a delete and an insert together: both must be applied, got {stored:?}"
    );

    // Deleting everything leaves the set itself, holding nothing.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("DELETE", "QUERY_STRING", None, "URL_DECODE"),
            update("DELETE", "BODY", None, "HTML_ENTITY_DECODE"),
            update(
                "DELETE",
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
            ),
            update("DELETE", "METHOD", None, "NONE"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete every filter: expected 200, got {status}: {body}"
    );
    let stored = filters(&router, &set_id).await;
    assert!(
        stored.is_empty(),
        "case delete every filter: the set must hold nothing, got {stored:?}"
    );
}

/// A filter is identified by its part of the request and its text transformation
/// together: changing either one makes it a DIFFERENT filter, and the `Data` name is
/// matched case-insensitively, since the member's documentation states the header and
/// query-argument names it carries are "not case sensitive".
#[tokio::test]
async fn a_filters_identity_is_its_field_to_match_and_transformation() {
    let router = router();
    let (set_id, _) = create_set(&router, "identity").await;

    // Four filters that differ from the first in exactly one component each.
    let rows: &[(&str, &str, Option<&str>, &str)] = &[
        ("the filter itself", "HEADER", Some("Referer"), "NONE"),
        (
            "another transformation on the same field",
            "HEADER",
            Some("Referer"),
            "URL_DECODE",
        ),
        (
            "another Data name on the same field",
            "HEADER",
            Some("Cookie"),
            "NONE",
        ),
        ("another field type", "QUERY_STRING", None, "NONE"),
    ];
    for (label, field_type, data, transformation) in rows {
        let (status, body) = apply_updates(
            &router,
            &set_id,
            vec![update("INSERT", field_type, *data, transformation)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: a filter differing in any component is a new filter, got \
             {status}: {body}"
        );
    }
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored.len(),
        rows.len(),
        "case four distinct filters: each must be stored once, got {stored:?}"
    );

    // Spelled in another case, the Data name denotes the SAME filter — so a delete
    // that spells it differently removes it, and the rest stay.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update("DELETE", "HEADER", Some("rEfErEr"), "NONE")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete a Data name spelled in another case: the name is not case \
         sensitive, got {status}: {body}"
    );
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![
            filter("HEADER", Some("Referer"), "URL_DECODE"),
            filter("HEADER", Some("Cookie"), "NONE"),
            filter("QUERY_STRING", None, "NONE"),
        ],
        "case delete a Data name spelled in another case: only that filter may go, got \
         {stored:?}"
    );

    // And an insert of a stored filter under another spelling is the same filter too,
    // so there is nothing to do.
    expect_error_update(
        &router,
        &set_id,
        vec![update("INSERT", "HEADER", Some("COOKIE"), "NONE")],
        "WAFInvalidOperationException",
        "insert a Data name spelled in another case",
    )
    .await;
}

/// An insert of a filter the set already holds, and a delete of one it does not, are
/// both refused with the modelled `WAFInvalidOperationException` — "the operation
/// failed because there was nothing to do" — and the stored set is left as it was,
/// together with the change token the refused request carried.
#[tokio::test]
async fn an_update_with_nothing_to_do_is_refused_and_persists_nothing() {
    let router = router();
    let (set_id, _) = create_set(&router, "nothing-to-do").await;
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update("INSERT", "BODY", None, "NONE")],
    )
    .await;
    assert_eq!(status, 200, "the setup insert must be accepted: {body}");
    let before = filters(&router, &set_id).await;

    let cases: &[(&str, Vec<Value>)] = &[
        (
            "insert a filter the set already holds",
            vec![update("INSERT", "BODY", None, "NONE")],
        ),
        (
            "delete a filter the set does not hold",
            vec![update("DELETE", "URI", None, "NONE")],
        ),
        (
            "delete a filter whose transformation differs",
            vec![update("DELETE", "BODY", None, "CMD_LINE")],
        ),
        (
            "delete the same filter twice in one request",
            vec![
                update("DELETE", "BODY", None, "NONE"),
                update("DELETE", "BODY", None, "NONE"),
            ],
        ),
        (
            "an accepted insert followed by a refused one",
            vec![
                update("INSERT", "METHOD", None, "NONE"),
                update("INSERT", "BODY", None, "NONE"),
            ],
        ),
    ];
    for (label, updates) in cases {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateSqlInjectionMatchSet",
            &json!({
                "SqlInjectionMatchSetId": set_id,
                "ChangeToken": token,
                "Updates": updates,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: the update must be refused, got {status}: {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("WAFInvalidOperationException"),
            "case {label}: expected WAFInvalidOperationException, got {body}"
        );

        let after = filters(&router, &set_id).await;
        assert_eq!(
            after, before,
            "case {label}: a refused update must persist nothing, got {after:?}"
        );
        // A request that was not applied did not consume its change token either.
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused update must leave its change token unused"
        );
    }
}

/// A `FieldToMatch` whose `Data` does not go with its `Type` is refused with the
/// modelled `WAFInvalidParameterException`.
#[tokio::test]
async fn an_unrecognised_parameter_is_a_modelled_invalid_parameter() {
    let router = router();
    let (set_id, _) = create_set(&router, "invalid-parameters").await;

    let cases: &[(&str, Value)] = &[
        // "You tried to update a ByteMatchSet with a Field of HEADER but no value for
        // Data."
        (
            "HEADER with no Data",
            update("INSERT", "HEADER", None, "NONE"),
        ),
        (
            "SINGLE_QUERY_ARG with no Data",
            update("INSERT", "SINGLE_QUERY_ARG", None, "NONE"),
        ),
        // "If the value of Type is any other value, omit Data."
        (
            "a field type that takes no Data, carrying one",
            update("INSERT", "QUERY_STRING", Some("User-Agent"), "NONE"),
        ),
        (
            "ALL_QUERY_ARGS carrying a Data name",
            update("INSERT", "ALL_QUERY_ARGS", Some("UserName"), "NONE"),
        ),
        (
            "URI carrying a Data name",
            update("INSERT", "URI", Some("Referer"), "NONE"),
        ),
        (
            "a DELETE whose Data does not go with its Type",
            update("DELETE", "METHOD", Some("GET"), "NONE"),
        ),
    ];
    for (label, bad_update) in cases {
        expect_error_update(
            &router,
            &set_id,
            vec![bad_update.clone()],
            "WAFInvalidParameterException",
            label,
        )
        .await;
    }

    let stored = filters(&router, &set_id).await;
    assert!(
        stored.is_empty(),
        "case after every refusal: nothing may have been persisted, got {stored:?}"
    );
}

/// A value outside the set an enum member declares never reaches the store: the request
/// is refused with HTTP 400 and the set keeps the filters it had.
///
/// The modelled value sets (`ChangeAction`, `MatchFieldType`, `TextTransformation`) are
/// enforced on the way in, before the handler runs, so what this pins is the refusal
/// and the absence of any effect — not which of the two 400s answers it.
#[tokio::test]
async fn an_enum_value_outside_the_modelled_set_is_refused() {
    let router = router();
    let (set_id, _) = create_set(&router, "invalid-enum-values").await;
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update("INSERT", "BODY", None, "NONE")],
    )
    .await;
    assert_eq!(status, 200, "the setup insert must be accepted: {body}");
    let before = filters(&router, &set_id).await;

    let cases: &[(&str, Value)] = &[
        // An action other than INSERT or DELETE (`ChangeAction` is DELETE | INSERT).
        (
            "an action that is not INSERT or DELETE",
            update("MODIFY", "URI", None, "NONE"),
        ),
        (
            "an action in the wrong case",
            update("insert", "URI", None, "NONE"),
        ),
        // A field type outside `MatchFieldType`.
        (
            "a field type the model does not declare",
            update("INSERT", "COOKIE", None, "NONE"),
        ),
        (
            "a field type in the wrong case",
            update("INSERT", "uri", None, "NONE"),
        ),
        // A transformation outside `TextTransformation`.
        (
            "a text transformation the model does not declare",
            update("INSERT", "URI", None, "BASE64_DECODE"),
        ),
        (
            "a text transformation in the wrong case",
            update("INSERT", "URI", None, "none"),
        ),
    ];
    for (label, bad_update) in cases {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateSqlInjectionMatchSet",
            &json!({
                "SqlInjectionMatchSetId": set_id,
                "ChangeToken": token,
                "Updates": [bad_update],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: the update must be refused, got {status}: {body}"
        );

        let after = filters(&router, &set_id).await;
        assert_eq!(
            after, before,
            "case {label}: a refused update must persist nothing, got {after:?}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused update must leave its change token unused"
        );
    }
}

/// An update naming a container the store does not hold is refused with the modelled
/// `WAFNonexistentContainerException` — "you tried to add an object to or delete an
/// object from another object that doesn't exist".
#[tokio::test]
async fn an_unknown_match_set_id_is_a_modelled_nonexistent_container() {
    let router = router();

    // A real id of a different resource type: minted by this service, but not a
    // SqlInjectionMatchSetId.
    let token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "other-type", "ChangeToken": token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .expect("CreateIPSet returned an IPSetId")
        .to_string();

    let long_id = "i".repeat(128);
    let cases: &[(&str, &str)] = &[
        (
            "a UUID-shaped id nothing was created under",
            "11111111-2222-4333-8444-555555555555",
        ),
        ("an id of a different resource type", ip_set_id.as_str()),
        ("a one-character id", "x"),
        ("a 128-character id (the maximum length)", long_id.as_str()),
    ];
    for (label, id) in cases {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateSqlInjectionMatchSet",
            &json!({
                "SqlInjectionMatchSetId": id,
                "ChangeToken": token,
                "Updates": [update("INSERT", "URI", None, "NONE")],
            }),
            "WAFNonexistentContainerException",
            label,
        )
        .await;
        // The refused request did not consume the token it carried.
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused update must leave its change token unused"
        );
    }
}

/// The change-token protocol on this operation: an accepted update reports the token it
/// consumed, that token is then propagating (`PENDING`), and no later mutating call may
/// use it again, since "each create, update, or delete request must use a unique change
/// token".
#[tokio::test]
async fn an_accepted_update_consumes_its_change_token_exactly_once() {
    let router = router();
    let (set_id, _) = create_set(&router, "token-protocol").await;

    let token = change_token(&router).await;
    let updated = ok_call(
        &router,
        "UpdateSqlInjectionMatchSet",
        &json!({
            "SqlInjectionMatchSetId": set_id,
            "ChangeToken": token,
            "Updates": [update("INSERT", "URI", None, "NONE")],
        }),
    )
    .await;
    assert_eq!(
        updated["ChangeToken"].as_str(),
        Some(token.as_str()),
        "the update must report the change token it consumed: {updated}"
    );
    assert_eq!(
        token_status(&router, &token).await,
        "PENDING",
        "a token a mutating call just consumed is propagating"
    );

    for (label, candidate) in [
        ("already consumed", token.clone()),
        (
            "never minted",
            "99999999-8888-4777-8666-555555555555".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "UpdateSqlInjectionMatchSet",
            &json!({
                "SqlInjectionMatchSetId": set_id,
                "ChangeToken": candidate,
                "Updates": [update("INSERT", "BODY", None, "NONE")],
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
    }

    // The refused updates persisted nothing: only the accepted insert is stored.
    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![filter("URI", None, "NONE")],
        "case after the stale-token refusals: only the accepted insert may be stored, \
         got {stored:?}"
    );
}

/// The number of filters one set can hold is bounded, and the bound is reported as the
/// modelled `WAFLimitsExceededException` — at the quota, not before it.
#[tokio::test]
async fn an_insert_past_the_filter_quota_is_refused() {
    let router = router();
    let (set_id, _) = create_set(&router, "filter-quota").await;

    // Distinct filters: one HEADER name per filter, so each is its own filter.
    for index in 0..MAX_SQL_INJECTION_MATCH_TUPLES {
        let name = format!("Probe-{index}");
        let (status, body) = apply_updates(
            &router,
            &set_id,
            vec![update("INSERT", "HEADER", Some(&name), "NONE")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case filter {index} of {MAX_SQL_INJECTION_MATCH_TUPLES}: an insert inside \
             the quota must be accepted, got {status}: {body}"
        );
    }

    let name = format!("Probe-{MAX_SQL_INJECTION_MATCH_TUPLES}");
    expect_error_update(
        &router,
        &set_id,
        vec![update("INSERT", "HEADER", Some(&name), "NONE")],
        "WAFLimitsExceededException",
        "one filter past the quota",
    )
    .await;

    // A single request that would take the set past the quota is refused the same way,
    // and leaves the set exactly at the quota.
    let overflowing: Vec<Value> = (0..3)
        .map(|index| {
            update(
                "INSERT",
                "HEADER",
                Some(&format!("Batch-{index}")),
                "NONE",
            )
        })
        .collect();
    expect_error_update(
        &router,
        &set_id,
        overflowing,
        "WAFLimitsExceededException",
        "a batch of inserts past the quota",
    )
    .await;

    let stored = filters(&router, &set_id).await;
    assert_eq!(
        stored.len(),
        MAX_SQL_INJECTION_MATCH_TUPLES,
        "case after the quota refusals: the set must hold exactly the quota, got \
         {stored:?}"
    );
}

/// The number of updates ONE request may carry is bounded too: a request past that
/// bound is refused with the modelled `WAFLimitsExceededException`, and a request at it
/// is not.
#[tokio::test]
async fn a_request_carrying_more_updates_than_the_bound_is_refused() {
    let router = router();
    let (set_id, _) = create_set(&router, "updates-per-request").await;

    // At the bound: fill the set to its filter quota and empty it again in one request
    // — the largest request that can leave the set in a state a smaller one cannot.
    let names: Vec<String> = (0..MAX_SQL_INJECTION_MATCH_TUPLES)
        .map(|index| format!("Batch-{index}"))
        .collect();
    let mut at_the_bound: Vec<Value> = names
        .iter()
        .map(|name| update("INSERT", "HEADER", Some(name), "NONE"))
        .collect();
    at_the_bound.extend(
        names
            .iter()
            .map(|name| update("DELETE", "HEADER", Some(name), "NONE")),
    );
    assert_eq!(
        at_the_bound.len(),
        MAX_SQL_INJECTION_MATCH_SET_UPDATES_PER_REQUEST,
        "the table must sit exactly at the per-request bound, got {}",
        at_the_bound.len()
    );
    let (status, body) = apply_updates(&router, &set_id, at_the_bound.clone()).await;
    assert_eq!(
        status, 200,
        "case a request at the per-request bound: expected 200, got {status}: {body}"
    );

    // One past it.
    let mut past_the_bound = at_the_bound;
    past_the_bound.push(update("INSERT", "URI", None, "NONE"));
    expect_error_update(
        &router,
        &set_id,
        past_the_bound,
        "WAFLimitsExceededException",
        "a request one update past the per-request bound",
    )
    .await;

    let stored = filters(&router, &set_id).await;
    assert!(
        stored.is_empty(),
        "case after the refusal: the accepted request emptied the set again and the \
         refused one persisted nothing, got {stored:?}"
    );
}

/// Refuse-or-explain: the update must answer 400 with the named modelled error.
async fn expect_error_update(
    router: &MultiServiceRouter,
    set_id: &str,
    updates: Vec<Value>,
    expected: &str,
    label: &str,
) {
    let token = change_token(router).await;
    expect_error(
        router,
        "UpdateSqlInjectionMatchSet",
        &json!({
            "SqlInjectionMatchSetId": set_id,
            "ChangeToken": token,
            "Updates": updates,
        }),
        expected,
        label,
    )
    .await;
}
