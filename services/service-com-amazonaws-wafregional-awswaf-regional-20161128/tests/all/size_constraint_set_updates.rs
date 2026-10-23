// SPDX-License-Identifier: CC0-1.0

//! `UpdateSizeConstraintSet`, end to end over the wire.
//!
//! The rule under test: an update inserts or deletes the `SizeConstraint`s of ONE
//! stored size constraint set, a constraint being identified by every member of the
//! modelled shape (which part of the request is measured, the `Data` name for the types
//! that carry one, the text transformation, the comparison operator and the size), so an
//! accepted request consumes its change token exactly once and leaves the named set
//! holding exactly the constraints that survive the updates it carried, in insertion
//! order, readable through `GetSizeConstraintSet`; an update naming a set the store does
//! not hold, a value outside a modelled enum or range, a `FieldToMatch` whose `Data`
//! does not go with its `Type`, an insert of a constraint the set already holds, a
//! delete of one it does not, or a request past a quota is refused with the modelled
//! error for that class and persists nothing.
//!
//! Each row of the tables below is one input along what that rule generalises over
//! (which part of a request is measured, which transformation, which comparison
//! operator, which size, whether a `Data` name is carried and how it is spelled, how
//! many updates one request carries, how many constraints the set already holds, which
//! id is named), so no assertion depends on the one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The three value catalogs below are the model's own, generated from it — never
// hand-written: `MATCH_FIELD_TYPE_VALUES` is every `MatchFieldType`,
// `TEXT_TRANSFORMATION_VALUES` every `TextTransformation` and
// `COMPARISON_OPERATOR_VALUES` every `ComparisonOperator` the wire accepts.
use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    COMPARISON_OPERATOR_VALUES, MATCH_FIELD_TYPE_VALUES, TEXT_TRANSFORMATION_VALUES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    match_field_takes_data, MAX_SIZE_CONSTRAINTS, MAX_SIZE_CONSTRAINT_SET_UPDATES_PER_REQUEST,
    MAX_SIZE_CONSTRAINT_SIZE,
};

// The wire harness and the create helper are shared with the other suites in this crate.
use crate::size_constraint_set::create_size_constraint_set;
use crate::wire::{call, change_token, expect_error, ok_call, router, token_status};

/// One `SizeConstraintSetUpdate` on the wire.
fn update(
    action: &str,
    field_type: &str,
    data: Option<&str>,
    transformation: &str,
    operator: &str,
    size: i64,
) -> Value {
    let field_to_match = match data {
        Some(data) => json!({"Type": field_type, "Data": data}),
        None => json!({"Type": field_type}),
    };
    json!({
        "Action": action,
        "SizeConstraint": {
            "FieldToMatch": field_to_match,
            "TextTransformation": transformation,
            "ComparisonOperator": operator,
            "Size": size,
        }
    })
}

/// A constraint as the assertions below compare them:
/// `(Type, Data, TextTransformation, ComparisonOperator, Size)`.
type Constraint = (String, Option<String>, String, String, i64);

/// The constraint one reported `SizeConstraint` names.
fn constraint_of(reported: &Value) -> Constraint {
    (
        reported["FieldToMatch"]["Type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        reported["FieldToMatch"]["Data"].as_str().map(str::to_string),
        reported["TextTransformation"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        reported["ComparisonOperator"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        reported["Size"].as_i64().unwrap_or(-1),
    )
}

/// One expected constraint, in the same shape [`constraint_of`] reports.
fn constraint(
    field_type: &str,
    data: Option<&str>,
    transformation: &str,
    operator: &str,
    size: i64,
) -> Constraint {
    (
        field_type.to_string(),
        data.map(str::to_string),
        transformation.to_string(),
        operator.to_string(),
        size,
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
        "UpdateSizeConstraintSet",
        &json!({
            "SizeConstraintSetId": set_id,
            "ChangeToken": token,
            "Updates": updates,
        }),
    )
    .await
}

/// The constraints `GetSizeConstraintSet` reports for `set_id`, in stored order.
async fn constraints(router: &MultiServiceRouter, set_id: &str) -> Vec<Constraint> {
    let body = ok_call(
        router,
        "GetSizeConstraintSet",
        &json!({"SizeConstraintSetId": set_id}),
    )
    .await;
    body["SizeConstraintSet"]["SizeConstraints"]
        .as_array()
        .unwrap_or_else(|| panic!("GetSizeConstraintSet reported no constraint list: {body}"))
        .iter()
        .map(constraint_of)
        .collect()
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
        "UpdateSizeConstraintSet",
        &json!({
            "SizeConstraintSetId": set_id,
            "ChangeToken": token,
            "Updates": updates,
        }),
        expected,
        label,
    )
    .await;
}

/// A `Data` name for `field_type`: one for the types that take a name, none for the
/// types whose documentation says to omit it.
fn data_for(field_type: &str, index: usize) -> Option<String> {
    match_field_takes_data(field_type).then(|| format!("Probe-Name-{index}"))
}

/// An INSERT of every `(MatchFieldType, ComparisonOperator)` pair the model declares is
/// persisted and read back verbatim, each with a text transformation drawn from the
/// model's catalog too.
///
/// The catalogs are the model's own (`MATCH_FIELD_TYPE_VALUES`,
/// `COMPARISON_OPERATOR_VALUES`, `TEXT_TRANSFORMATION_VALUES`), so this covers every
/// published value — including the ones the issue's recorded request never names — and
/// each pair goes into its own freshly created set, so what comes back is that set's own
/// stored constraint.
#[tokio::test]
async fn an_insert_of_every_modelled_field_type_and_operator_round_trips() {
    let router = router();

    for (field_index, field_type) in MATCH_FIELD_TYPE_VALUES.iter().enumerate() {
        for (operator_index, operator) in COMPARISON_OPERATOR_VALUES.iter().enumerate() {
            // The transformation and the size vary with the row too, so no row repeats
            // another's whole constraint.
            let transformation =
                TEXT_TRANSFORMATION_VALUES[(field_index + operator_index) % TEXT_TRANSFORMATION_VALUES.len()];
            let size = (operator_index as i64) * 1_024;
            let label = format!("{field_type}/{operator}/{transformation}/{size}");
            let data = data_for(field_type, field_index);
            let set_id = create_size_constraint_set(
                &router,
                &format!("catalog-{field_index}-{operator_index}"),
            )
            .await;

            let (status, body) = apply_updates(
                &router,
                &set_id,
                vec![update(
                    "INSERT",
                    field_type,
                    data.as_deref(),
                    transformation,
                    operator,
                    size,
                )],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label}: the insert must be accepted, got {status}: {body}"
            );

            let stored = constraints(&router, &set_id).await;
            assert_eq!(
                stored,
                vec![constraint(
                    field_type,
                    data.as_deref(),
                    transformation,
                    operator,
                    size
                )],
                "case {label}: the set must hold exactly the inserted constraint, got \
                 {stored:?}"
            );
        }
    }
}

/// Every published bound of `Size` round-trips, and a value past either bound is
/// refused and persists nothing.
///
/// Source: `Size`'s modelled `@range` of 0..21474836480 (`MAX_SIZE_CONSTRAINT_SIZE`),
/// which its documentation repeats as "0 - 21474836480 bytes (0 - 20 GB)".
#[tokio::test]
async fn the_published_size_bounds_round_trip_and_a_value_past_them_is_refused() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "size-bounds").await;

    let inside: &[(&str, i64)] = &[
        ("the minimum Size", 0),
        ("one byte", 1),
        ("the body-inspection limit", 8_192),
        ("the maximum Size", MAX_SIZE_CONSTRAINT_SIZE),
        ("one byte below the maximum", MAX_SIZE_CONSTRAINT_SIZE - 1),
    ];
    let mut expected: Vec<Constraint> = Vec::new();
    for (label, size) in inside {
        let (status, body) = apply_updates(
            &router,
            &set_id,
            vec![update("INSERT", "URI", None, "NONE", "GT", *size)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: a Size inside the published range must be accepted, got \
             {status}: {body}"
        );
        expected.push(constraint("URI", None, "NONE", "GT", *size));
    }
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored, expected,
        "case every bound inside the range: each must be stored once, got {stored:?}"
    );

    let outside: &[(&str, i64)] = &[
        ("one byte past the maximum", MAX_SIZE_CONSTRAINT_SIZE + 1),
        ("far past the maximum", MAX_SIZE_CONSTRAINT_SIZE * 2),
        ("one byte below the minimum", -1),
    ];
    for (label, size) in outside {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateSizeConstraintSet",
            &json!({
                "SizeConstraintSetId": set_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", "URI", None, "NONE", "GT", *size)],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: a Size outside the published range must be refused, got \
             {status}: {body}"
        );
        let after = constraints(&router, &set_id).await;
        assert_eq!(
            after, expected,
            "case {label}: a refused update must persist nothing, got {after:?}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused update must leave its change token unused"
        );
    }
}

/// Inserts accumulate in insertion order, across one request and across several, and a
/// delete removes only the constraint it names.
#[tokio::test]
async fn inserts_accumulate_in_order_and_a_delete_removes_only_its_own_constraint() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "accumulate").await;

    // Three constraints in ONE request: a type that takes no Data, a type that takes
    // one, and a third comparison on a third part of the request.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("INSERT", "QUERY_STRING", None, "URL_DECODE", "GT", 100),
            update(
                "INSERT",
                "HEADER",
                Some("User-Agent"),
                "LOWERCASE",
                "EQ",
                0,
            ),
            update("INSERT", "BODY", None, "NONE", "LE", 8_192),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case three inserts in one request: expected 200, got {status}: {body}"
    );
    let expected = vec![
        constraint("QUERY_STRING", None, "URL_DECODE", "GT", 100),
        constraint("HEADER", Some("User-Agent"), "LOWERCASE", "EQ", 0),
        constraint("BODY", None, "NONE", "LE", 8_192),
    ];
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored, expected,
        "case three inserts in one request: the constraints must be stored in insertion \
         order, got {stored:?}"
    );

    // Two more in a SECOND request: appended after the ones already stored.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("INSERT", "URI", None, "CMD_LINE", "LT", 7),
            update(
                "INSERT",
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
                "GE",
                42,
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
            constraint("URI", None, "CMD_LINE", "LT", 7),
            constraint(
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
                "GE",
                42,
            ),
        ],
    ]
    .concat();
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored, expected,
        "case two more inserts: a second request appends to what is stored, got \
         {stored:?}"
    );

    // One DELETE: only the named constraint goes, and the rest keep their order.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update(
            "DELETE",
            "HEADER",
            Some("User-Agent"),
            "LOWERCASE",
            "EQ",
            0,
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "case one delete: expected 200, got {status}: {body}"
    );
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![
            constraint("QUERY_STRING", None, "URL_DECODE", "GT", 100),
            constraint("BODY", None, "NONE", "LE", 8_192),
            constraint("URI", None, "CMD_LINE", "LT", 7),
            constraint(
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
                "GE",
                42,
            ),
        ],
        "case one delete: only the named constraint may go, got {stored:?}"
    );

    // An INSERT and a DELETE in the SAME request, applied in order.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("DELETE", "URI", None, "CMD_LINE", "LT", 7),
            update("INSERT", "METHOD", None, "NONE", "NE", 3),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case a delete and an insert together: expected 200, got {status}: {body}"
    );
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![
            constraint("QUERY_STRING", None, "URL_DECODE", "GT", 100),
            constraint("BODY", None, "NONE", "LE", 8_192),
            constraint(
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
                "GE",
                42,
            ),
            constraint("METHOD", None, "NONE", "NE", 3),
        ],
        "case a delete and an insert together: both must be applied, got {stored:?}"
    );

    // Deleting everything leaves the set itself, holding nothing.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![
            update("DELETE", "QUERY_STRING", None, "URL_DECODE", "GT", 100),
            update("DELETE", "BODY", None, "NONE", "LE", 8_192),
            update(
                "DELETE",
                "SINGLE_QUERY_ARG",
                Some("SalesRegion"),
                "COMPRESS_WHITE_SPACE",
                "GE",
                42,
            ),
            update("DELETE", "METHOD", None, "NONE", "NE", 3),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete every constraint: expected 200, got {status}: {body}"
    );
    let stored = constraints(&router, &set_id).await;
    assert!(
        stored.is_empty(),
        "case delete every constraint: the set must hold nothing, got {stored:?}"
    );
}

/// One row of the identity table: a constraint, and what it varies from the first row.
struct IdentityRow {
    /// What this row varies, named in every assertion failure.
    label: &'static str,
    /// `FieldToMatch.Type`, a `MatchFieldType` value.
    field_type: &'static str,
    /// `FieldToMatch.Data`, which only HEADER and SINGLE_QUERY_ARG carry.
    data: Option<&'static str>,
    /// A `TextTransformation` value.
    transformation: &'static str,
    /// A `ComparisonOperator` value.
    operator: &'static str,
    /// `Size`, within the modelled `@range`.
    size: i64,
}

/// A constraint is identified by EVERY member of the modelled shape together: changing
/// the field type, the `Data` name, the transformation, the comparison operator or the
/// size each makes it a DIFFERENT constraint — since "if you want to change a
/// SizeConstraintSetUpdate object, you delete the existing object and add a new one" —
/// and the `Data` name is matched case-insensitively, since the member's documentation
/// states the header and query-argument names it carries are "not case sensitive".
#[tokio::test]
async fn a_constraints_identity_is_every_member_of_the_modelled_shape() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "identity").await;

    // Five constraints that differ from the first in exactly one component each.
    let rows: &[IdentityRow] = &[
        IdentityRow {
            label: "the constraint itself",
            field_type: "HEADER",
            data: Some("Referer"),
            transformation: "NONE",
            operator: "GT",
            size: 100,
        },
        IdentityRow {
            label: "another transformation on the same field",
            field_type: "HEADER",
            data: Some("Referer"),
            transformation: "URL_DECODE",
            operator: "GT",
            size: 100,
        },
        IdentityRow {
            label: "another Data name on the same field",
            field_type: "HEADER",
            data: Some("Cookie"),
            transformation: "NONE",
            operator: "GT",
            size: 100,
        },
        IdentityRow {
            label: "another comparison operator",
            field_type: "HEADER",
            data: Some("Referer"),
            transformation: "NONE",
            operator: "LT",
            size: 100,
        },
        IdentityRow {
            label: "another size",
            field_type: "HEADER",
            data: Some("Referer"),
            transformation: "NONE",
            operator: "GT",
            size: 101,
        },
        IdentityRow {
            label: "another field type",
            field_type: "QUERY_STRING",
            data: None,
            transformation: "NONE",
            operator: "GT",
            size: 100,
        },
    ];
    for row in rows {
        let label = row.label;
        let (status, body) = apply_updates(
            &router,
            &set_id,
            vec![update(
                "INSERT",
                row.field_type,
                row.data,
                row.transformation,
                row.operator,
                row.size,
            )],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: a constraint differing in any component is a new constraint, \
             got {status}: {body}"
        );
    }
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored.len(),
        rows.len(),
        "case six distinct constraints: each must be stored once, got {stored:?}"
    );

    // Spelled in another case, the Data name denotes the SAME constraint — so a delete
    // that spells it differently removes it, and the rest stay.
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update(
            "DELETE",
            "HEADER",
            Some("rEfErEr"),
            "NONE",
            "GT",
            100,
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete a Data name spelled in another case: the name is not case \
         sensitive, got {status}: {body}"
    );
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![
            constraint("HEADER", Some("Referer"), "URL_DECODE", "GT", 100),
            constraint("HEADER", Some("Cookie"), "NONE", "GT", 100),
            constraint("HEADER", Some("Referer"), "NONE", "LT", 100),
            constraint("HEADER", Some("Referer"), "NONE", "GT", 101),
            constraint("QUERY_STRING", None, "NONE", "GT", 100),
        ],
        "case delete a Data name spelled in another case: only that constraint may go, \
         got {stored:?}"
    );

    // And an insert of a stored constraint under another spelling is the same
    // constraint too, so there is nothing to do.
    expect_error_update(
        &router,
        &set_id,
        vec![update(
            "INSERT",
            "HEADER",
            Some("COOKIE"),
            "NONE",
            "GT",
            100,
        )],
        "WAFInvalidOperationException",
        "insert a Data name spelled in another case",
    )
    .await;
}

/// An insert of a constraint the set already holds, and a delete of one it does not, are
/// both refused with the modelled `WAFInvalidOperationException` — "the operation failed
/// because there was nothing to do" — and the stored set is left as it was, together
/// with the change token the refused request carried.
#[tokio::test]
async fn an_update_with_nothing_to_do_is_refused_and_persists_nothing() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "nothing-to-do").await;
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update("INSERT", "BODY", None, "NONE", "GT", 100)],
    )
    .await;
    assert_eq!(status, 200, "the setup insert must be accepted: {body}");
    let before = constraints(&router, &set_id).await;

    let cases: &[(&str, Vec<Value>)] = &[
        (
            "insert a constraint the set already holds",
            vec![update("INSERT", "BODY", None, "NONE", "GT", 100)],
        ),
        (
            "delete a constraint the set does not hold",
            vec![update("DELETE", "URI", None, "NONE", "GT", 100)],
        ),
        (
            "delete a constraint whose size differs",
            vec![update("DELETE", "BODY", None, "NONE", "GT", 101)],
        ),
        (
            "delete a constraint whose operator differs",
            vec![update("DELETE", "BODY", None, "NONE", "GE", 100)],
        ),
        (
            "delete a constraint whose transformation differs",
            vec![update("DELETE", "BODY", None, "CMD_LINE", "GT", 100)],
        ),
        (
            "delete the same constraint twice in one request",
            vec![
                update("DELETE", "BODY", None, "NONE", "GT", 100),
                update("DELETE", "BODY", None, "NONE", "GT", 100),
            ],
        ),
        (
            "an accepted insert followed by a refused one",
            vec![
                update("INSERT", "METHOD", None, "NONE", "EQ", 3),
                update("INSERT", "BODY", None, "NONE", "GT", 100),
            ],
        ),
    ];
    for (label, updates) in cases {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateSizeConstraintSet",
            &json!({
                "SizeConstraintSetId": set_id,
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

        let after = constraints(&router, &set_id).await;
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
    let set_id = create_size_constraint_set(&router, "invalid-parameters").await;

    let cases: &[(&str, Value)] = &[
        // "You tried to update a ByteMatchSet with a Field of HEADER but no value for
        // Data."
        (
            "HEADER with no Data",
            update("INSERT", "HEADER", None, "NONE", "GT", 100),
        ),
        (
            "SINGLE_QUERY_ARG with no Data",
            update("INSERT", "SINGLE_QUERY_ARG", None, "NONE", "GT", 100),
        ),
        // "If the value of Type is any other value, omit Data."
        (
            "a field type that takes no Data, carrying one",
            update(
                "INSERT",
                "QUERY_STRING",
                Some("User-Agent"),
                "NONE",
                "GT",
                100,
            ),
        ),
        (
            "ALL_QUERY_ARGS carrying a Data name",
            update(
                "INSERT",
                "ALL_QUERY_ARGS",
                Some("UserName"),
                "NONE",
                "GT",
                100,
            ),
        ),
        (
            "URI carrying a Data name",
            update("INSERT", "URI", Some("Referer"), "NONE", "GT", 100),
        ),
        (
            "a DELETE whose Data does not go with its Type",
            update("DELETE", "METHOD", Some("GET"), "NONE", "GT", 100),
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

    let stored = constraints(&router, &set_id).await;
    assert!(
        stored.is_empty(),
        "case after every refusal: nothing may have been persisted, got {stored:?}"
    );
}

/// A value outside the set an enum member declares never reaches the store: the request
/// is refused with HTTP 400 and the set keeps the constraints it had.
///
/// The modelled value sets (`ChangeAction`, `MatchFieldType`, `TextTransformation`,
/// `ComparisonOperator`) are enforced on the way in as well as in the handler, so what
/// this pins is the refusal and the absence of any effect — not which of the two 400s
/// answers it.
#[tokio::test]
async fn an_enum_value_outside_the_modelled_set_is_refused() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "invalid-enum-values").await;
    let (status, body) = apply_updates(
        &router,
        &set_id,
        vec![update("INSERT", "BODY", None, "NONE", "GT", 100)],
    )
    .await;
    assert_eq!(status, 200, "the setup insert must be accepted: {body}");
    let before = constraints(&router, &set_id).await;

    let cases: &[(&str, Value)] = &[
        // An action other than INSERT or DELETE (`ChangeAction` is DELETE | INSERT).
        (
            "an action that is not INSERT or DELETE",
            update("MODIFY", "URI", None, "NONE", "GT", 100),
        ),
        (
            "an action in the wrong case",
            update("insert", "URI", None, "NONE", "GT", 100),
        ),
        // A field type outside `MatchFieldType`.
        (
            "a field type the model does not declare",
            update("INSERT", "COOKIE", None, "NONE", "GT", 100),
        ),
        (
            "a field type in the wrong case",
            update("INSERT", "uri", None, "NONE", "GT", 100),
        ),
        // A transformation outside `TextTransformation`.
        (
            "a text transformation the model does not declare",
            update("INSERT", "URI", None, "BASE64_DECODE", "GT", 100),
        ),
        (
            "a text transformation in the wrong case",
            update("INSERT", "URI", None, "none", "GT", 100),
        ),
        // An operator outside `ComparisonOperator`.
        (
            "a comparison operator the model does not declare",
            update("INSERT", "URI", None, "NONE", "GREATER_THAN", 100),
        ),
        (
            "a comparison operator in the wrong case",
            update("INSERT", "URI", None, "NONE", "gt", 100),
        ),
    ];
    for (label, bad_update) in cases {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateSizeConstraintSet",
            &json!({
                "SizeConstraintSetId": set_id,
                "ChangeToken": token,
                "Updates": [bad_update],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: the update must be refused, got {status}: {body}"
        );

        let after = constraints(&router, &set_id).await;
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
async fn an_unknown_set_id_is_a_modelled_nonexistent_container() {
    let router = router();

    // A real id of a different resource type: minted by this service, but not a
    // SizeConstraintSetId.
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
            "UpdateSizeConstraintSet",
            &json!({
                "SizeConstraintSetId": id,
                "ChangeToken": token,
                "Updates": [update("INSERT", "URI", None, "NONE", "GT", 100)],
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
    let set_id = create_size_constraint_set(&router, "token-protocol").await;

    let token = change_token(&router).await;
    let updated = ok_call(
        &router,
        "UpdateSizeConstraintSet",
        &json!({
            "SizeConstraintSetId": set_id,
            "ChangeToken": token,
            "Updates": [update("INSERT", "URI", None, "NONE", "GT", 100)],
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
            "UpdateSizeConstraintSet",
            &json!({
                "SizeConstraintSetId": set_id,
                "ChangeToken": candidate,
                "Updates": [update("INSERT", "BODY", None, "NONE", "LE", 8_192)],
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
    }

    // The refused updates persisted nothing: only the accepted insert is stored.
    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored,
        vec![constraint("URI", None, "NONE", "GT", 100)],
        "case after the stale-token refusals: only the accepted insert may be stored, \
         got {stored:?}"
    );
}

/// The number of constraints one set can hold is bounded, and the bound is reported as
/// the modelled `WAFLimitsExceededException` — at the quota, not before it.
#[tokio::test]
async fn an_insert_past_the_constraint_quota_is_refused() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "constraint-quota").await;

    // Distinct constraints: one size per constraint, so each is its own constraint.
    for index in 0..MAX_SIZE_CONSTRAINTS {
        let (status, body) = apply_updates(
            &router,
            &set_id,
            vec![update("INSERT", "URI", None, "NONE", "GT", index as i64)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case constraint {index} of {MAX_SIZE_CONSTRAINTS}: an insert inside the \
             quota must be accepted, got {status}: {body}"
        );
    }

    expect_error_update(
        &router,
        &set_id,
        vec![update(
            "INSERT",
            "URI",
            None,
            "NONE",
            "GT",
            MAX_SIZE_CONSTRAINTS as i64,
        )],
        "WAFLimitsExceededException",
        "one constraint past the quota",
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
                "LT",
                512,
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

    let stored = constraints(&router, &set_id).await;
    assert_eq!(
        stored.len(),
        MAX_SIZE_CONSTRAINTS,
        "case after the quota refusals: the set must hold exactly the quota, got \
         {stored:?}"
    );
}

/// The number of updates ONE request may carry is bounded too: a request past that bound
/// is refused with the modelled `WAFLimitsExceededException`, and a request at it is
/// not.
#[tokio::test]
async fn a_request_carrying_more_updates_than_the_bound_is_refused() {
    let router = router();
    let set_id = create_size_constraint_set(&router, "updates-per-request").await;

    // At the bound: fill the set to its constraint quota and empty it again in one
    // request — the largest request that can leave the set in a state a smaller one
    // cannot.
    let sizes: Vec<i64> = (0..MAX_SIZE_CONSTRAINTS as i64).collect();
    let mut at_the_bound: Vec<Value> = sizes
        .iter()
        .map(|size| update("INSERT", "HEADER", Some("Referer"), "NONE", "GE", *size))
        .collect();
    at_the_bound.extend(
        sizes
            .iter()
            .map(|size| update("DELETE", "HEADER", Some("Referer"), "NONE", "GE", *size)),
    );
    assert_eq!(
        at_the_bound.len(),
        MAX_SIZE_CONSTRAINT_SET_UPDATES_PER_REQUEST,
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
    past_the_bound.push(update("INSERT", "URI", None, "NONE", "GT", 1));
    expect_error_update(
        &router,
        &set_id,
        past_the_bound,
        "WAFLimitsExceededException",
        "a request one update past the per-request bound",
    )
    .await;

    let stored = constraints(&router, &set_id).await;
    assert!(
        stored.is_empty(),
        "case after the refusal: the accepted request emptied the set again and the \
         refused one persisted nothing, got {stored:?}"
    );
}
