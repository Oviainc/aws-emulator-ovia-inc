// SPDX-License-Identifier: CC0-1.0

//! What a successful `ExecuteStatement` result set CARRIES: the JSON form every
//! value takes in `formattedRecords`, and the 200 envelope a PostgreSQL-typed
//! projection is answered with.
//!
//! Every call goes through the router, so routing, restJson1 deserialization, the
//! handler and serialization are all exercised. The client harness is the one
//! `sql_data_api` already builds.

use serde_json::{json, Value};

use crate::sql_data_api::{error_code, list, number, parameter, DataApi};

/// The JSON document `formattedRecords` carries, parsed.
fn formatted(body: &Value) -> Value {
    let text = body["formattedRecords"]
        .as_str()
        .unwrap_or_else(|| panic!("formattedRecords must be a string: {body}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("formattedRecords must parse as JSON ({error}): {text}"))
}

/// RULE: in a `formatRecordsAs=JSON` result set a value is rendered in the JSON
/// form of the `Field` member its COLUMN's type names, so both forms of one
/// result set report the same value as the same JSON type: the union models
/// `booleanValue` as a boolean and `blobValue` as a blob — which this protocol
/// carries base64-encoded — so a boolean column is a JSON `true`/`false` and a
/// binary column is its base64 text, a number is a JSON number, text and a date
/// or time are JSON strings, a NULL is `null`, and an array column is a JSON
/// array whose elements take the form their element type names.
///
/// The rows span the type families the rule generalises over (boolean, binary,
/// both integer widths, floating point, exact numeric, character, temporal, the
/// NULL of any type, and an array of each element family), and along the VALUE
/// inside a family — the bytes of a binary value, both boolean literals, an
/// array element that is NULL. Each case asserts BOTH forms of the same column,
/// so the JSON form is pinned against the `Field` member it must agree with.
#[tokio::test]
async fn the_json_record_format_renders_a_value_as_its_field_member_does() {
    let api = DataApi::new();
    // (case, the column's declared type, the inserted value, its parameters, the
    //  JSON form, the `Field` the records form carries — `null` where that form
    //  has no member for the value and refuses the whole result instead)
    let cases: [(&str, &str, &str, Value, Value, Value); 20] = [
        (
            "a boolean that is true",
            "BOOLEAN",
            "true",
            json!([]),
            json!(true),
            json!({"booleanValue": true}),
        ),
        (
            "a boolean that is false",
            "BOOLEAN",
            "false",
            json!([]),
            json!(false),
            json!({"booleanValue": false}),
        ),
        (
            "binary data",
            "BYTEA",
            ":v",
            json!([parameter("v", json!({"blobValue": "AQL/"}))]),
            json!("AQL/"),
            json!({"blobValue": "AQL/"}),
        ),
        (
            "binary data whose bytes are zero",
            "BYTEA",
            ":v",
            json!([parameter("v", json!({"blobValue": "AAAA"}))]),
            json!("AAAA"),
            json!({"blobValue": "AAAA"}),
        ),
        (
            "binary data of a length base64 pads",
            "BYTEA",
            ":v",
            json!([parameter("v", json!({"blobValue": "/w=="}))]),
            json!("/w=="),
            json!({"blobValue": "/w=="}),
        ),
        (
            "an integer",
            "INT",
            "42",
            json!([]),
            json!(42),
            json!({"longValue": 42}),
        ),
        (
            "an integer wider than a float holds exactly",
            "BIGINT",
            "9007199254740993",
            json!([]),
            json!(9_007_199_254_740_993_i64),
            json!({"longValue": 9_007_199_254_740_993_i64}),
        ),
        (
            "a floating-point number",
            "DOUBLE PRECISION",
            "0.25",
            json!([]),
            json!(0.25),
            json!({"doubleValue": 0.25}),
        ),
        (
            "an exact number",
            "NUMERIC(10,2)",
            "2.5",
            json!([]),
            json!(2.5),
            json!({"stringValue": "2.50"}),
        ),
        (
            "text",
            "TEXT",
            "'rdsd-first'",
            json!([]),
            json!("rdsd-first"),
            json!({"stringValue": "rdsd-first"}),
        ),
        (
            "text carrying a quote",
            "VARCHAR(16)",
            "'a \"quoted\" word'",
            json!([]),
            json!("a \"quoted\" word"),
            json!({"stringValue": "a \"quoted\" word"}),
        ),
        (
            "a date",
            "DATE",
            "'2024-02-29'",
            json!([]),
            json!("2024-02-29"),
            json!({"stringValue": "2024-02-29"}),
        ),
        (
            "a NULL in a text column",
            "TEXT",
            "NULL",
            json!([]),
            json!(null),
            json!({"isNull": true}),
        ),
        (
            "a NULL in a binary column",
            "BYTEA",
            "NULL",
            json!([]),
            json!(null),
            json!({"isNull": true}),
        ),
        (
            "an array of booleans",
            "BOOL[]",
            "ARRAY[true, false]",
            json!([]),
            json!([true, false]),
            json!({"arrayValue": {"booleanValues": [true, false]}}),
        ),
        (
            "an array of integers",
            "INT4[]",
            "ARRAY[10, 20, 30]",
            json!([]),
            json!([10, 20, 30]),
            json!({"arrayValue": {"longValues": [10, 20, 30]}}),
        ),
        (
            "an array of floating-point numbers",
            "FLOAT8[]",
            "ARRAY[0.5, 1.25]",
            json!([]),
            json!([0.5, 1.25]),
            json!({"arrayValue": {"doubleValues": [0.5, 1.25]}}),
        ),
        (
            // The one class where the two forms differ: JSON has a number, and
            // `records` follows `decimalReturnType`, whose documented default
            // returns an exact number as text.
            "an array of exact numbers",
            "NUMERIC[]",
            "'{2.50,3}'::numeric[]",
            json!([]),
            json!([2.5, 3.0]),
            json!({"arrayValue": {"stringValues": ["2.50", "3"]}}),
        ),
        (
            "an array of strings",
            "TEXT[]",
            "ARRAY['red', 'blue']",
            json!([]),
            json!(["red", "blue"]),
            json!({"arrayValue": {"stringValues": ["red", "blue"]}}),
        ),
        (
            // The `ArrayValue` lists hold a value per element and nothing else, so
            // the records form refuses this one; JSON has a form for a missing
            // value, so the JSON form reports it.
            "an array with a NULL element",
            "TEXT[]",
            "'{red,NULL}'::text[]",
            json!([]),
            json!(["red", null]),
            Value::Null,
        ),
    ];

    for (index, (case, declared, inserted, parameters, as_json, as_field)) in
        cases.into_iter().enumerate()
    {
        let table = format!("values_{index}");
        api.run(json!({"sql": format!("CREATE TABLE {table} (v {declared})")}))
            .await;
        api.run(json!({
            "sql": format!("INSERT INTO {table} (v) VALUES ({inserted})"),
            "parameters": parameters,
        }))
        .await;

        let json_form = api
            .run(json!({
                "sql": format!("SELECT v FROM {table}"),
                "formatRecordsAs": "JSON",
            }))
            .await;
        let rendered = formatted(&json_form);
        assert_eq!(
            rendered,
            json!([{"v": as_json}]),
            "case {case:?}: expected the JSON form [{{\"v\":{as_json}}}], got {rendered:?}"
        );

        let (status, records_form) = api
            .execute(json!({"sql": format!("SELECT v FROM {table}")}))
            .await;
        if as_field.is_null() {
            // The records form has no member for this value, so the whole result
            // is refused there — which is the asymmetry the JSON form above
            // resolves, asserted rather than assumed.
            assert_eq!(
                (status, error_code(&records_form)),
                (400, "UnsupportedResultException"),
                "case {case:?}: expected the records form to refuse the result, \
                 got {status} {records_form}"
            );
            continue;
        }
        assert_eq!(status, 200, "case {case:?}: got {status} {records_form}");
        assert_eq!(
            records_form["records"],
            json!([[as_field]]),
            "case {case:?}: expected the records form [[{as_field}]], got {records_form:?}"
        );
    }

    // The one value NEITHER form can carry — the multidimensional array the model
    // names — is refused in the model's own words whichever form was asked for.
    for format in ["JSON", "NONE"] {
        let (status, body) = api
            .execute(json!({
                "sql": "SELECT '{{1,2},{3,4}}'::int4[] AS grid",
                "formatRecordsAs": format,
            }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "UnsupportedResultException"),
            "case a multidimensional array as {format}: got {status} {body}"
        );
        assert_eq!(
            body["message"],
            json!("The result contains a multidimensional array."),
            "case a multidimensional array as {format}: got {body}"
        );
    }
}

/// RULE: a SELECT of PostgreSQL-typed expressions is EXECUTED and answered 200
/// with the modelled result envelope — `records` carrying one `Field` per value
/// in the member its column's PostgreSQL type names, `numberOfRecordsUpdated` 0
/// because a SELECT changes no rows, and (when `includeResultMetadata` is set)
/// one `columnMetadata` per projected column carrying that type's catalog
/// `typeName`, its `java.sql.Types` code, its digits and its flags.
///
/// The rows span the typed forms a projection is written in — a cast, a bare
/// literal, a typed NULL — over the type families the Data API carries, each with
/// the member and the catalog entry its values must come back under.
#[tokio::test]
async fn a_postgresql_typed_projection_is_executed_and_returns_its_result_envelope() {
    let api = DataApi::new();
    // (case, the projected expression, typeName, java.sql.Types code, the Field,
    //  isSigned, isCaseSensitive)
    let cases = [
        (
            "a cast to int4",
            "1::int4",
            "int4",
            4,
            json!({"longValue": 1}),
            true,
            false,
        ),
        (
            "a cast to int8",
            "7::int8",
            "int8",
            -5,
            json!({"longValue": 7}),
            true,
            false,
        ),
        (
            "a cast to text",
            "'abc'::text",
            "text",
            12,
            json!({"stringValue": "abc"}),
            false,
            true,
        ),
        (
            "a boolean literal",
            "true",
            "bool",
            -7,
            json!({"booleanValue": true}),
            false,
            false,
        ),
        (
            "a cast to numeric",
            "2.5::numeric",
            "numeric",
            2,
            json!({"stringValue": "2.5"}),
            true,
            false,
        ),
        (
            "a cast to float8",
            "1.5::float8",
            "float8",
            8,
            json!({"doubleValue": 1.5}),
            true,
            false,
        ),
        (
            "a typed NULL",
            "NULL::text",
            "text",
            12,
            json!({"isNull": true}),
            false,
            true,
        ),
        (
            "a cast to a date",
            "'2024-01-31'::date",
            "date",
            91,
            json!({"stringValue": "2024-01-31"}),
            false,
            false,
        ),
    ];

    for (case, expression, type_name, jdbc_type, field, signed, case_sensitive) in cases {
        let (status, body) = api
            .execute(json!({
                "sql": format!("SELECT {expression} AS projected"),
                "includeResultMetadata": true,
            }))
            .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["records"],
            json!([[field]]),
            "case {case:?}: expected records [[{field}]], got {body}"
        );
        assert_eq!(
            number(&body, "numberOfRecordsUpdated"),
            0,
            "case {case:?}: a SELECT changes no rows, got {body}"
        );
        let metadata = list(&body, "columnMetadata");
        assert_eq!(
            metadata.len(),
            1,
            "case {case:?}: one ColumnMetadata per projected column, got {body}"
        );
        let column = &metadata[0];
        assert_eq!(
            (
                column["typeName"].clone(),
                column["type"].clone(),
                column["name"].clone(),
                column["label"].clone(),
                column["nullable"].clone(),
                column["isSigned"].clone(),
                column["isCaseSensitive"].clone(),
            ),
            (
                json!(type_name),
                json!(jdbc_type),
                json!("projected"),
                json!("projected"),
                json!(1),
                json!(signed),
                json!(case_sensitive),
            ),
            "case {case:?}: expected the catalog entry for {type_name}, got {column}"
        );
    }

    // The whole projection at once: every column of one row is described and
    // returned, in the order the statement lists them.
    let (status, body) = api
        .execute(json!({
            "sql": "SELECT 1::int4 AS i, 'abc'::text AS t, true AS b, \
                    2.5::numeric AS n, 1.5::float8 AS d, NULL::text AS missing",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(status, 200, "case the whole projection: got {status} {body}");
    assert_eq!(
        body["records"],
        json!([[
            {"longValue": 1},
            {"stringValue": "abc"},
            {"booleanValue": true},
            {"stringValue": "2.5"},
            {"doubleValue": 1.5},
            {"isNull": true},
        ]]),
        "case the whole projection: got {body}"
    );
    let type_names = list(&body, "columnMetadata")
        .iter()
        .map(|column| column["typeName"].clone())
        .collect::<Vec<Value>>();
    assert_eq!(
        type_names,
        vec![
            json!("int4"),
            json!("text"),
            json!("bool"),
            json!("numeric"),
            json!("float8"),
            json!("text"),
        ],
        "case the whole projection: expected one typeName per column, got {body}"
    );
}

/// RULE: the base64 text a binary column is rendered as — in `formattedRecords`
/// and in `records`' `blobValue` alike — is the base64 of the BYTES the column
/// holds, so a client that decodes it gets what was stored.
///
/// Each row pairs a byte string, written in hexadecimal, with the base64 text
/// that is its encoding; both are written here rather than computed, so what the
/// case asserts is the encoding and not the implementation's own codec. The bytes
/// really stored are read back through the engine's own `hex` and `length`, which
/// is independent of how the response renders them.
///
/// The rows differ along the byte string: three arbitrary bytes, bytes that are
/// zero, one byte whose length makes base64 pad, and none at all.
#[tokio::test]
async fn a_binary_column_is_the_base64_of_the_bytes_it_holds() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE payloads (id INT, payload BYTEA)"}))
        .await;
    let cases = [
        ("three arbitrary bytes", "0102ff", "AQL/"),
        ("bytes that are zero", "000000", "AAAA"),
        ("one byte, a length base64 pads", "ff", "/w=="),
        ("no bytes at all", "", ""),
    ];

    for (index, (case, bytes_in_hex, as_base64)) in cases.into_iter().enumerate() {
        let bytes = hex::decode(bytes_in_hex).expect("each case writes whole bytes in hex");
        api.run(json!({
            "sql": "INSERT INTO payloads (id, payload) VALUES (:id, :payload)",
            "parameters": [
                parameter("id", json!({"longValue": index})),
                parameter("payload", json!({"blobValue": as_base64})),
            ],
        }))
        .await;

        // What the engine holds, read back through the engine itself.
        let stored = api
            .run(json!({
                "sql": "SELECT hex(payload) AS written, length(payload) AS bytes \
                        FROM payloads WHERE id = :id",
                "parameters": [parameter("id", json!({"longValue": index}))],
            }))
            .await;
        assert_eq!(
            stored["records"],
            json!([[
                {"stringValue": hex::encode_upper(&bytes)},
                {"longValue": bytes.len()},
            ]]),
            "case {case:?}: expected the {} byte(s) of {bytes_in_hex} to be stored, got {stored}",
            bytes.len()
        );

        // And what each form of the result set renders them as.
        let records_form = api
            .run(json!({"sql": "SELECT payload FROM payloads WHERE id = :id",
                        "parameters": [parameter("id", json!({"longValue": index}))]}))
            .await;
        assert_eq!(
            records_form["records"],
            json!([[{"blobValue": as_base64}]]),
            "case {case:?}: expected blobValue {as_base64:?}, got {records_form}"
        );
        let json_form = api
            .run(json!({"sql": "SELECT payload FROM payloads WHERE id = :id",
                        "parameters": [parameter("id", json!({"longValue": index}))],
                        "formatRecordsAs": "JSON"}))
            .await;
        assert_eq!(
            formatted(&json_form),
            json!([{"payload": as_base64}]),
            "case {case:?}: expected the JSON form {as_base64:?}, got {json_form}"
        );
    }
}
