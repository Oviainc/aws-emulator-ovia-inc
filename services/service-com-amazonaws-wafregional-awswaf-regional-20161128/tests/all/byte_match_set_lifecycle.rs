// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) ByteMatchSet lifecycle, end to end over the wire:
//! `CreateByteMatchSet`, `UpdateByteMatchSet`, `GetByteMatchSet`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`, awsJson1_1), so what these tests
//! assert is what an SDK client sees: each response is built from the persisted record,
//! not from the request.
//!
//! The rule under test is the round trip: a `ByteMatchTuple` a caller INSERTs is stored
//! whole, and `GetByteMatchSet` reports every one of its members unchanged, for every
//! `MatchFieldType`, `TextTransformation` and `PositionalConstraint` the model declares
//! and for target bytes of any size up to the documented bound. The table below spans
//! those value lists rather than one recorded tuple.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    MATCH_FIELD_TYPE_VALUES, POSITIONAL_CONSTRAINT_VALUES, TEXT_TRANSFORMATION_VALUES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_BYTE_MATCH_SETS, MAX_BYTE_MATCH_UPDATES_PER_REQUEST, MAX_TARGET_STRING_BYTES,
    MESSAGE_NONEXISTENT_CONTAINER, MESSAGE_NONEXISTENT_ITEM, MESSAGE_STALE_DATA,
};

// The wire harness is shared with the other suites in this crate.
use crate::wire::{
    call, change_token, error_code, error_message, expect_error, ok_call, router,
};

/// Create a ByteMatchSet named `name` and return `(ByteMatchSetId, response body)`.
async fn create_byte_match_set(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateByteMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    let id = body["ByteMatchSet"]["ByteMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateByteMatchSet returned no ByteMatchSetId: {body}"))
        .to_string();
    (id, body)
}

/// Apply `updates` to `id` with a fresh change token, and return `(status, body)`.
async fn update(router: &MultiServiceRouter, id: &str, updates: Vec<Value>) -> (u16, Value) {
    let token = change_token(router).await;
    call(
        router,
        "UpdateByteMatchSet",
        &json!({"ByteMatchSetId": id, "ChangeToken": token, "Updates": updates}),
    )
    .await
}

/// The stored ByteMatchSet, read back through `GetByteMatchSet`.
async fn get_byte_match_set(router: &MultiServiceRouter, id: &str) -> Value {
    ok_call(router, "GetByteMatchSet", &json!({"ByteMatchSetId": id})).await
}

/// One row of the tuple table: the members a `ByteMatchTuple` carries. The enum values
/// come from the model's own value lists, not from a hand-written catalogue.
#[derive(Debug, Clone)]
struct TupleCase {
    label: &'static str,
    field_type: &'static str,
    data: Option<&'static str>,
    target: Vec<u8>,
    transformation: &'static str,
    constraint: &'static str,
}

impl TupleCase {
    /// This row as the wire `ByteMatchTuple`: `TargetString` is a blob, which awsJson1_1
    /// carries as base64 (the codec the serializer itself uses).
    fn tuple(&self) -> Value {
        let mut field_to_match = json!({"Type": self.field_type});
        if let Some(data) = self.data {
            field_to_match["Data"] = json!(data);
        }
        json!({
            "FieldToMatch": field_to_match,
            "TargetString": aws_smithy_types::base64::encode(&self.target),
            "TextTransformation": self.transformation,
            "PositionalConstraint": self.constraint,
        })
    }

    /// This row as one `ByteMatchSetUpdate`.
    fn update(&self, action: &str) -> Value {
        json!({"Action": action, "ByteMatchTuple": self.tuple()})
    }
}

/// The table: every `MatchFieldType`, every `TextTransformation` and every
/// `PositionalConstraint` the model declares appears in at least one row, and the rows
/// differ in the size and content of the target bytes (one byte, printable ASCII, bytes
/// that are not UTF-8, and the documented 50-byte maximum) and in whether
/// `FieldToMatch.Data` is present.
fn tuple_table() -> Vec<TupleCase> {
    vec![
        TupleCase {
            label: "URI, starts with a path",
            field_type: "URI",
            data: None,
            target: b"/images/daily-ad.jpg".to_vec(),
            transformation: "NONE",
            constraint: "STARTS_WITH",
        },
        TupleCase {
            label: "QUERY_STRING, lowercased, contains",
            field_type: "QUERY_STRING",
            data: None,
            target: b"BadBot".to_vec(),
            transformation: "LOWERCASE",
            constraint: "CONTAINS",
        },
        TupleCase {
            label: "HEADER names its Data",
            field_type: "HEADER",
            data: Some("User-Agent"),
            target: b"BadBot".to_vec(),
            transformation: "COMPRESS_WHITE_SPACE",
            constraint: "CONTAINS_WORD",
        },
        TupleCase {
            label: "METHOD, exactly",
            field_type: "METHOD",
            data: None,
            target: b"POST".to_vec(),
            transformation: "NONE",
            constraint: "EXACTLY",
        },
        TupleCase {
            label: "BODY, the 50-byte maximum",
            field_type: "BODY",
            data: None,
            target: vec![b'b'; MAX_TARGET_STRING_BYTES],
            transformation: "HTML_ENTITY_DECODE",
            constraint: "ENDS_WITH",
        },
        TupleCase {
            label: "SINGLE_QUERY_ARG names its Data",
            field_type: "SINGLE_QUERY_ARG",
            data: Some("UserName"),
            target: b"admin".to_vec(),
            transformation: "URL_DECODE",
            constraint: "EXACTLY",
        },
        TupleCase {
            label: "ALL_QUERY_ARGS, bytes that are not UTF-8",
            field_type: "ALL_QUERY_ARGS",
            data: None,
            target: vec![0x00, 0x01, 0xff],
            transformation: "CMD_LINE",
            constraint: "CONTAINS",
        },
        TupleCase {
            label: "a second HEADER row, one byte, different Data",
            field_type: "HEADER",
            data: Some("Referer"),
            target: b"x".to_vec(),
            transformation: "NONE",
            constraint: "CONTAINS",
        },
    ]
}

/// Assert the stored tuple at `index` of `set` is `case`.
fn assert_stored(set: &Value, index: usize, case: &TupleCase) {
    let tuples = set["ByteMatchSet"]["ByteMatchTuples"]
        .as_array()
        .unwrap_or_else(|| panic!("GetByteMatchSet returned no ByteMatchTuples: {set}"));
    let stored = tuples.get(index).unwrap_or_else(|| {
        panic!(
            "case {:?}: expected a tuple at index {index}, got {} tuples: {set}",
            case.label,
            tuples.len()
        )
    });
    assert_eq!(
        stored["FieldToMatch"]["Type"].as_str(),
        Some(case.field_type),
        "case {:?}: expected FieldToMatch.Type {}, got {stored}",
        case.label,
        case.field_type
    );
    assert_eq!(
        stored["FieldToMatch"]["Data"].as_str(),
        case.data,
        "case {:?}: expected FieldToMatch.Data {:?}, got {stored}",
        case.label,
        case.data
    );
    let encoded = stored["TargetString"].as_str().unwrap_or_else(|| {
        panic!(
            "case {:?}: expected a base64 TargetString, got {stored}",
            case.label
        )
    });
    let decoded = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
        panic!(
            "case {:?}: TargetString must be base64 ({error}): {encoded}",
            case.label
        )
    });
    assert_eq!(
        decoded, case.target,
        "case {:?}: expected TargetString bytes {:?}, got {decoded:?}",
        case.label, case.target
    );
    assert_eq!(
        stored["TextTransformation"].as_str(),
        Some(case.transformation),
        "case {:?}: expected TextTransformation {}, got {stored}",
        case.label,
        case.transformation
    );
    assert_eq!(
        stored["PositionalConstraint"].as_str(),
        Some(case.constraint),
        "case {:?}: expected PositionalConstraint {}, got {stored}",
        case.label,
        case.constraint
    );
}

/// The table spans the model's value lists: a row exists for every `MatchFieldType`,
/// `TextTransformation` and `PositionalConstraint` the service declares, so the
/// round-trip test below is not a test of one recorded tuple.
#[test]
fn the_tuple_table_spans_every_modelled_value() {
    let table = tuple_table();
    for (member, declared, used) in [
        (
            "MatchFieldType",
            MATCH_FIELD_TYPE_VALUES,
            table
                .iter()
                .map(|case| case.field_type)
                .collect::<BTreeSet<_>>(),
        ),
        (
            "TextTransformation",
            TEXT_TRANSFORMATION_VALUES,
            table
                .iter()
                .map(|case| case.transformation)
                .collect::<BTreeSet<_>>(),
        ),
        (
            "PositionalConstraint",
            POSITIONAL_CONSTRAINT_VALUES,
            table
                .iter()
                .map(|case| case.constraint)
                .collect::<BTreeSet<_>>(),
        ),
    ] {
        let declared: BTreeSet<&str> = declared.iter().copied().collect();
        assert_eq!(
            used, declared,
            "case {member}: the table must cover every modelled value, missing {:?}",
            declared.difference(&used).collect::<Vec<_>>()
        );
    }
}

/// Create stores a record the reader sees, and every member of every INSERTed tuple
/// round-trips through `GetByteMatchSet` unchanged, in insertion order.
#[tokio::test]
async fn tuple_fields_round_trip_through_update_and_get() {
    let router = router();
    let table = tuple_table();
    let (id, created) = create_byte_match_set(&router, "round-trip").await;

    // "A ByteMatchSet that contains no ByteMatchTuple objects."
    assert_eq!(
        created["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case create: a new ByteMatchSet holds no tuples, got {created}"
    );
    assert_eq!(
        created["ByteMatchSet"]["Name"].as_str(),
        Some("round-trip"),
        "case create: the response carries the stored Name, got {created}"
    );

    // The first rows arrive in ONE request, the rest one request at a time: the stored
    // order is the order the tuples were inserted in either way.
    let (status, body) = update(
        &router,
        &id,
        table[..3].iter().map(|case| case.update("INSERT")).collect(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case batch insert: UpdateByteMatchSet must answer 200, got {status}: {body}"
    );
    for case in &table[3..] {
        let (status, body) = update(&router, &id, vec![case.update("INSERT")]).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateByteMatchSet must answer 200, got {status}: {body}",
            case.label
        );
    }

    let read = get_byte_match_set(&router, &id).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchSetId"].as_str(),
        Some(id.as_str()),
        "case read: the reader answers the requested set, got {read}"
    );
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(table.len()),
        "case read: expected {} stored tuples, got {read}",
        table.len()
    );
    for (index, case) in table.iter().enumerate() {
        assert_stored(&read, index, case);
    }

    // A read changes nothing a later caller sees.
    let again = get_byte_match_set(&router, &id).await;
    assert_eq!(
        again, read,
        "case repeat read: a read must not change the stored set, got {again}"
    );

    // Each tuple is deletable by its own values, and only that tuple goes.
    for (deleted, case) in table.iter().enumerate() {
        let (status, body) = update(&router, &id, vec![case.update("DELETE")]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the DELETE of a stored tuple must answer 200, got {status}: {body}",
            case.label
        );
        let read = get_byte_match_set(&router, &id).await;
        let remaining = &table[deleted + 1..];
        assert_eq!(
            read["ByteMatchSet"]["ByteMatchTuples"]
                .as_array()
                .map(Vec::len),
            Some(remaining.len()),
            "case {:?}: expected {} tuples left, got {read}",
            case.label,
            remaining.len()
        );
        for (index, left) in remaining.iter().enumerate() {
            assert_stored(&read, index, left);
        }
    }
}

/// `Name` is not an identity: two creates with the same name are two resources, each
/// read back under its own id.
#[tokio::test]
async fn each_create_mints_its_own_set() {
    let router = router();
    let mut ids = BTreeSet::new();
    for case in ["same-name", "same-name", "other-name"] {
        let (id, created) = create_byte_match_set(&router, case).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: every create mints a fresh ByteMatchSetId, {id} repeated"
        );
        assert_eq!(
            created["ByteMatchSet"]["Name"].as_str(),
            Some(case),
            "case {case:?}: the created set carries its own Name, got {created}"
        );
        let read = get_byte_match_set(&router, &id).await;
        assert_eq!(
            read["ByteMatchSet"]["Name"].as_str(),
            Some(case),
            "case {case:?}: the stored set carries its own Name, got {read}"
        );
        assert_eq!(
            read["ByteMatchSet"]["ByteMatchSetId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the stored set carries its own id, got {read}"
        );
    }
}

/// An update that leaves the set unchanged is refused, in both directions: an INSERT of
/// a tuple the set already holds and a DELETE of one it does not.
#[tokio::test]
async fn an_update_with_nothing_to_do_is_refused() {
    let router = router();
    let (id, _) = create_byte_match_set(&router, "nothing-to-do").await;

    for case in tuple_table() {
        // The DELETE of a tuple that was never inserted.
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateByteMatchSet",
            &json!({
                "ByteMatchSetId": id,
                "ChangeToken": token,
                "Updates": [case.update("DELETE")],
            }),
            "WAFInvalidOperationException",
            case.label,
        )
        .await;

        let (status, body) = update(&router, &id, vec![case.update("INSERT")]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first INSERT must answer 200, got {status}: {body}",
            case.label
        );

        // The INSERT of a tuple the set already holds.
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateByteMatchSet",
            &json!({
                "ByteMatchSetId": id,
                "ChangeToken": token,
                "Updates": [case.update("INSERT")],
            }),
            "WAFInvalidOperationException",
            case.label,
        )
        .await;

        let read = get_byte_match_set(&router, &id).await;
        let stored = read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            stored, 1,
            "case {:?}: the refused INSERT must not duplicate the tuple, got {read}",
            case.label
        );

        let (status, body) = update(&router, &id, vec![case.update("DELETE")]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the DELETE of the stored tuple must answer 200, got {status}: {body}",
            case.label
        );
    }
}

/// A request refused part-way through applies none of its updates, and the change token
/// it carried is still usable.
#[tokio::test]
async fn a_refused_update_applies_nothing() {
    let router = router();
    let (id, _) = create_byte_match_set(&router, "all-or-nothing").await;
    let table = tuple_table();

    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateByteMatchSet",
        &json!({
            "ByteMatchSetId": id,
            "ChangeToken": token,
            "Updates": [
                table[0].update("INSERT"),
                // The second update is refused: nothing to do.
                table[1].update("DELETE"),
            ],
        }),
        "WAFInvalidOperationException",
        "a valid insert followed by a refused delete",
    )
    .await;

    let read = get_byte_match_set(&router, &id).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case partial update: none of the updates may be applied, got {read}"
    );

    // The refused request consumed nothing, so its token still works.
    let body = ok_call(
        &router,
        "UpdateByteMatchSet",
        &json!({
            "ByteMatchSetId": id,
            "ChangeToken": token,
            "Updates": [table[0].update("INSERT")],
        }),
    )
    .await;
    assert_eq!(
        body["ChangeToken"].as_str(),
        Some(token.as_str()),
        "case reused token: the response echoes the token it consumed, got {body}"
    );
    assert_stored(&get_byte_match_set(&router, &id).await, 0, &table[0]);
}

/// A value outside a member's modelled enum is refused, and the refusal names that
/// member and the value it did not recognise.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused() {
    let router = router();
    let (id, _) = create_byte_match_set(&router, "enums").await;
    let base = tuple_table()[0].clone();

    // (label, the update, the ParameterExceptionField the refusal must name)
    let mut insert = base.update("INSERT");
    insert["Action"] = json!("UPSERT");
    let bad_action = insert;

    let mut tuple = base.tuple();
    tuple["FieldToMatch"]["Type"] = json!("COOKIE");
    let bad_field_type = json!({"Action": "INSERT", "ByteMatchTuple": tuple});

    let mut tuple = base.tuple();
    tuple["TextTransformation"] = json!("ROT13");
    let bad_transformation = json!({"Action": "INSERT", "ByteMatchTuple": tuple});

    let mut tuple = base.tuple();
    tuple["PositionalConstraint"] = json!("ANYWHERE");
    let bad_constraint = json!({"Action": "INSERT", "ByteMatchTuple": tuple});

    for (label, update, value) in [
        ("an Action outside ChangeAction", bad_action, "UPSERT"),
        (
            "a FieldToMatch.Type outside MatchFieldType",
            bad_field_type,
            "COOKIE",
        ),
        (
            "a TextTransformation outside its enum",
            bad_transformation,
            "ROT13",
        ),
        (
            "a PositionalConstraint outside its enum",
            bad_constraint,
            "ANYWHERE",
        ),
    ] {
        let token = change_token(&router).await;
        // Each of these members carries a modelled enum, so the refusal may come from
        // the centralized walk (`ValidationException`, before any handler runs) or from
        // the handler's own guard (`WAFInvalidParameterException`); what this asserts is
        // that the value is refused as invalid input with HTTP 400 and that the refusal
        // names the value, not which of the two worded it.
        let body = expect_error(
            &router,
            "UpdateByteMatchSet",
            &json!({"ByteMatchSetId": id, "ChangeToken": token, "Updates": [update]}),
            "Exception",
            label,
        )
        .await;
        let code = error_code(&body);
        assert!(
            code.contains("WAFInvalidParameterException") || code.contains("ValidationException"),
            "case {label:?}: expected the request to be refused as invalid input, got {code}"
        );
        assert!(
            body.to_string().contains(value),
            "case {label:?}: the refusal must name the value it refused ({value}), got {body}"
        );
    }

    let read = get_byte_match_set(&router, &id).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case refused enums: a refused update stores nothing, got {read}"
    );
}

/// `FieldToMatch.Data` names the part of the request to search for the two field types
/// that have one, so those are refused without it; every other field type is accepted
/// without it.
#[tokio::test]
async fn a_field_type_that_names_its_data_is_refused_without_it() {
    let router = router();
    let (id, _) = create_byte_match_set(&router, "field-data").await;

    for field_type in MATCH_FIELD_TYPE_VALUES {
        let case = TupleCase {
            label: "no Data",
            field_type,
            data: None,
            target: b"BadBot".to_vec(),
            transformation: "NONE",
            constraint: "CONTAINS",
        };
        let needs_data = *field_type == "HEADER" || *field_type == "SINGLE_QUERY_ARG";
        if needs_data {
            let token = change_token(&router).await;
            let body = expect_error(
                &router,
                "UpdateByteMatchSet",
                &json!({
                    "ByteMatchSetId": id,
                    "ChangeToken": token,
                    "Updates": [case.update("INSERT")],
                }),
                "WAFInvalidParameterException",
                field_type,
            )
            .await;
            assert_eq!(
                body["reason"].as_str().or_else(|| body["Reason"].as_str()),
                Some("ILLEGAL_COMBINATION"),
                "case {field_type}: expected reason ILLEGAL_COMBINATION, got {body}"
            );
        } else {
            let (status, body) = update(&router, &id, vec![case.update("INSERT")]).await;
            assert_eq!(
                status, 200,
                "case {field_type}: a type that names no Data is accepted without it, \
                 got {status}: {body}"
            );
        }
    }
}

/// `TargetString` is bounded at the documented 50 bytes: every size up to it is stored,
/// and the first size past it is refused.
#[tokio::test]
async fn a_target_string_is_bounded_at_the_documented_maximum() {
    let router = router();
    let (id, _) = create_byte_match_set(&router, "target-bound").await;

    // 1 byte, one below the bound, exactly the bound, one past it, and far past it.
    for length in [
        1,
        MAX_TARGET_STRING_BYTES - 1,
        MAX_TARGET_STRING_BYTES,
        MAX_TARGET_STRING_BYTES + 1,
        MAX_TARGET_STRING_BYTES * 4,
    ] {
        let case = TupleCase {
            label: "sized target",
            field_type: "URI",
            data: None,
            target: vec![b'a'; length],
            transformation: "NONE",
            constraint: "CONTAINS",
        };
        if length <= MAX_TARGET_STRING_BYTES {
            let (status, body) = update(&router, &id, vec![case.update("INSERT")]).await;
            assert_eq!(
                status, 200,
                "case {length} bytes: a target within the bound is stored, got {status}: {body}"
            );
            let read = get_byte_match_set(&router, &id).await;
            let stored = read["ByteMatchSet"]["ByteMatchTuples"]
                .as_array()
                .and_then(|tuples| tuples.last())
                .cloned()
                .unwrap_or(Value::Null);
            let decoded = aws_smithy_types::base64::decode(
                stored["TargetString"].as_str().unwrap_or_default(),
            )
            .unwrap_or_default();
            assert_eq!(
                decoded.len(),
                length,
                "case {length} bytes: expected {length} stored bytes, got {decoded:?}"
            );
        } else {
            let token = change_token(&router).await;
            expect_error(
                &router,
                "UpdateByteMatchSet",
                &json!({
                    "ByteMatchSetId": id,
                    "ChangeToken": token,
                    "Updates": [case.update("INSERT")],
                }),
                "WAFLimitsExceededException",
                &format!("{length} bytes"),
            )
            .await;
        }
    }
}

/// One request carries at most `MAX_BYTE_MATCH_UPDATES_PER_REQUEST` updates: a request
/// at that bound is applied and one past it is `WAFLimitsExceededException`, so no
/// request can make the service walk a list only the caller bounds.
#[tokio::test]
async fn an_update_carries_at_most_the_batch_cap() {
    let router = router();
    let (id, _) = create_byte_match_set(&router, "batch-cap").await;

    // Distinct targets, so every INSERT has something to do.
    let updates = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|n| {
                TupleCase {
                    label: "batched",
                    field_type: "URI",
                    data: None,
                    target: format!("/{n}").into_bytes(),
                    transformation: "NONE",
                    constraint: "CONTAINS",
                }
                .update("INSERT")
            })
            .collect()
    };

    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateByteMatchSet",
        &json!({
            "ByteMatchSetId": id,
            "ChangeToken": token,
            "Updates": updates(MAX_BYTE_MATCH_UPDATES_PER_REQUEST + 1),
        }),
        "WAFLimitsExceededException",
        "one update past the batch cap",
    )
    .await;
    let read = get_byte_match_set(&router, &id).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case one past the batch cap: the refused request stores nothing, got {read}"
    );

    let (status, body) = update(
        &router,
        &id,
        updates(MAX_BYTE_MATCH_UPDATES_PER_REQUEST),
    )
    .await;
    assert_eq!(
        status, 200,
        "case at the batch cap: the request must be applied, got {status}: {body}"
    );
    let read = get_byte_match_set(&router, &id).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(MAX_BYTE_MATCH_UPDATES_PER_REQUEST),
        "case at the batch cap: every update is applied, got {}",
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    );
}

/// The account holds at most `MAX_BYTE_MATCH_SETS` sets: the one past that bound is
/// refused with the modelled `WAFLimitsExceededException`, and the store is unchanged.
#[tokio::test]
async fn the_account_holds_at_most_the_published_number_of_sets() {
    let router = router();
    for n in 0..MAX_BYTE_MATCH_SETS {
        let token = change_token(&router).await;
        let body = ok_call(
            &router,
            "CreateByteMatchSet",
            &json!({"Name": format!("set-{n}"), "ChangeToken": token}),
        )
        .await;
        assert!(
            body["ByteMatchSet"]["ByteMatchSetId"].is_string(),
            "case set {n}: a create within the bound must answer a set, got {body}"
        );
    }

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateByteMatchSet",
        &json!({"Name": "one-too-many", "ChangeToken": token}),
        "WAFLimitsExceededException",
        "one set past the account bound",
    )
    .await;
}

/// A set this service holds nothing for: the reader answers the modelled
/// `WAFNonexistentItemException`, the update the modelled
/// `WAFNonexistentContainerException`, and neither message carries anything from the
/// request.
#[tokio::test]
async fn a_set_that_does_not_exist_is_refused() {
    let router = router();
    // One id never minted, and one whose set exists, so the two refusals below differ
    // only in the id that was asked for.
    let (real, _) = create_byte_match_set(&router, "exists").await;
    let absent = [
        "00000000-0000-0000-0000-000000000000",
        "11111111-2222-4333-8444-555555555555",
    ];

    let mut messages = BTreeSet::new();
    for id in absent {
        let body = expect_error(
            &router,
            "GetByteMatchSet",
            &json!({"ByteMatchSetId": id}),
            "WAFNonexistentItemException",
            id,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {id}: the message is the fixed sentence, got {body}"
        );
        messages.insert(error_message(&body));

        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateByteMatchSet",
            &json!({
                "ByteMatchSetId": id,
                "ChangeToken": token,
                "Updates": [tuple_table()[0].update("INSERT")],
            }),
            "WAFNonexistentContainerException",
            id,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_CONTAINER,
            "case {id}: the message is the fixed sentence, got {body}"
        );
        assert!(
            !error_message(&body).contains(id),
            "case {id}: the message carries nothing from the request, got {body}"
        );
    }
    assert_eq!(
        messages.len(),
        1,
        "case not-found messages: one constant sentence per exception, got {messages:?}"
    );

    // The set that does exist is still readable: a refused call left it alone.
    let read = get_byte_match_set(&router, &real).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchSetId"].as_str(),
        Some(real.as_str()),
        "case existing set: still readable after the refusals, got {read}"
    );
}

/// Each change token is consumed by exactly one mutating call: the second use of one is
/// refused, whichever operation uses it.
#[tokio::test]
async fn a_change_token_is_consumed_once() {
    let router = router();
    let table = tuple_table();

    // A token consumed by CreateByteMatchSet cannot create a second set.
    let token = change_token(&router).await;
    let body = ok_call(
        &router,
        "CreateByteMatchSet",
        &json!({"Name": "first", "ChangeToken": token}),
    )
    .await;
    let id = body["ByteMatchSet"]["ByteMatchSetId"]
        .as_str()
        .expect("CreateByteMatchSet returned an id")
        .to_string();
    assert_eq!(
        body["ChangeToken"].as_str(),
        Some(token.as_str()),
        "case create: the response echoes the token it consumed, got {body}"
    );
    let reused_on_create = expect_error(
        &router,
        "CreateByteMatchSet",
        &json!({"Name": "second", "ChangeToken": token}),
        "WAFStaleDataException",
        "a token already used by a create",
    )
    .await;

    // And a token consumed by UpdateByteMatchSet cannot be used again either.
    let token = change_token(&router).await;
    ok_call(
        &router,
        "UpdateByteMatchSet",
        &json!({
            "ByteMatchSetId": id,
            "ChangeToken": token,
            "Updates": [table[0].update("INSERT")],
        }),
    )
    .await;
    let reused_on_update = expect_error(
        &router,
        "UpdateByteMatchSet",
        &json!({
            "ByteMatchSetId": id,
            "ChangeToken": token,
            "Updates": [table[1].update("INSERT")],
        }),
        "WAFStaleDataException",
        "a token already used by an update",
    )
    .await;

    // The refusal is one fixed sentence: it does not vary with the token or the
    // operation that was refused.
    for (label, body) in [
        ("create", &reused_on_create),
        ("update", &reused_on_update),
    ] {
        assert_eq!(
            error_message(body),
            MESSAGE_STALE_DATA,
            "case {label}: the stale-token message is the fixed sentence, got {body}"
        );
    }

    // The update that DID run is the only one stored.
    let read = get_byte_match_set(&router, &id).await;
    assert_eq!(
        read["ByteMatchSet"]["ByteMatchTuples"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "case stale update: the refused update stored nothing, got {read}"
    );
    assert_stored(&read, 0, &table[0]);
}
