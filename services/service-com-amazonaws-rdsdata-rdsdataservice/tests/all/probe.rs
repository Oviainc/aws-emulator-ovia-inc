// SPDX-License-Identifier: CC0-1.0

//! Probes of the PostgreSQL data plane the Data API answers from: the statement
//! forms it accepts, the catalog its `columnMetadata` reports, the `Field` member
//! each PostgreSQL type's values come back in, and what it refuses.
//!
//! Every call goes through the router, so routing, restJson1 deserialization, the
//! handler and serialization are all exercised. The client harness is the one
//! `sql_data_api` already builds.

use serde_json::{json, Value};

use crate::sql_data_api::{error_code, list, number, parameter, DataApi};

/// RULE: `columnMetadata` describes a column with the PostgreSQL type its
/// declaration resolves to: `typeName` is that type's catalog name, `type` its
/// `java.sql.Types` code, `precision` and `scale` the digits it carries,
/// `isSigned` whether it is a signed number, `isCaseSensitive` whether its values
/// compare as text, and `schemaName` the schema the column is in.
///
/// The catalog is PostgreSQL's: the accepted spellings and the type each resolves
/// to are the ones its "Data Types" chapter documents (`integer`, `int` and
/// `int4` are one type, and `serial` is "an autoincrementing integer column"),
/// the codes are `java.sql.Types`' constants, and a precision is the digits the
/// type's documented range holds (`smallint` spans ±32767, so 5; `integer`
/// ±2147483647, so 10; `bigint` 19; `double precision` 17 significant digits;
/// `boolean` 1), with a length-parameterised type reporting the length its
/// declaration gave and an unlimited one the largest length the member carries.
///
/// The rows span the catalog's families — boolean, the three integer widths, both
/// floating-point widths, exact numbers with and without a declared scale, the
/// character types, binary, the date and time types, the two document types, and
/// the three sequence types — each under a spelling and under an alias.
#[tokio::test]
async fn column_metadata_reports_the_postgresql_catalog() {
    /// The largest length a character member carries, which is what PostgreSQL
    /// reports for a type with no length limit.
    const UNLIMITED: i64 = i32::MAX as i64;

    struct Case {
        /// The column declaration, as a client writes it.
        declared: &'static str,
        /// The value bound into the column, and the `Field` member it must come
        /// back in.
        bound: Value,
        expected: Value,
        type_name: &'static str,
        jdbc_type: i64,
        precision: i64,
        scale: i64,
        signed: bool,
        case_sensitive: bool,
    }

    let cases = [
        Case {
            declared: "BOOLEAN",
            bound: json!({"booleanValue": true}),
            expected: json!({"booleanValue": true}),
            type_name: "bool",
            jdbc_type: -7,
            precision: 1,
            scale: 0,
            signed: false,
            case_sensitive: false,
        },
        Case {
            declared: "SMALLINT",
            bound: json!({"longValue": 32_767}),
            expected: json!({"longValue": 32_767}),
            type_name: "int2",
            jdbc_type: 5,
            precision: 5,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "INT2",
            bound: json!({"longValue": -32_768}),
            expected: json!({"longValue": -32_768}),
            type_name: "int2",
            jdbc_type: 5,
            precision: 5,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "INTEGER",
            bound: json!({"longValue": 2_147_483_647}),
            expected: json!({"longValue": 2_147_483_647}),
            type_name: "int4",
            jdbc_type: 4,
            precision: 10,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "BIGINT",
            bound: json!({"longValue": 9_007_199_254_740_993_i64}),
            expected: json!({"longValue": 9_007_199_254_740_993_i64}),
            type_name: "int8",
            jdbc_type: -5,
            precision: 19,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "REAL",
            bound: json!({"doubleValue": 0.5}),
            expected: json!({"doubleValue": 0.5}),
            type_name: "float4",
            jdbc_type: 7,
            precision: 8,
            scale: 8,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "DOUBLE PRECISION",
            bound: json!({"doubleValue": -1.25}),
            expected: json!({"doubleValue": -1.25}),
            type_name: "float8",
            jdbc_type: 8,
            precision: 17,
            scale: 17,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "NUMERIC(12,4)",
            bound: json!({"stringValue": "1.5000"}),
            expected: json!({"stringValue": "1.5000"}),
            type_name: "numeric",
            jdbc_type: 2,
            precision: 12,
            scale: 4,
            signed: true,
            case_sensitive: false,
        },
        // An exact number with no declared precision carries each value's own.
        Case {
            declared: "NUMERIC",
            bound: json!({"stringValue": "1.5"}),
            expected: json!({"stringValue": "1.5"}),
            type_name: "numeric",
            jdbc_type: 2,
            precision: 0,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "DECIMAL(5,2)",
            bound: json!({"stringValue": "1.25"}),
            expected: json!({"stringValue": "1.25"}),
            type_name: "numeric",
            jdbc_type: 2,
            precision: 5,
            scale: 2,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "TEXT",
            bound: json!({"stringValue": "a text value"}),
            expected: json!({"stringValue": "a text value"}),
            type_name: "text",
            jdbc_type: 12,
            precision: UNLIMITED,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            declared: "VARCHAR(32)",
            bound: json!({"stringValue": "a varchar value"}),
            expected: json!({"stringValue": "a varchar value"}),
            type_name: "varchar",
            jdbc_type: 12,
            precision: 32,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            declared: "CHARACTER VARYING(8)",
            bound: json!({"stringValue": "short"}),
            expected: json!({"stringValue": "short"}),
            type_name: "varchar",
            jdbc_type: 12,
            precision: 8,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            declared: "CHAR(4)",
            bound: json!({"stringValue": "abcd"}),
            expected: json!({"stringValue": "abcd"}),
            type_name: "bpchar",
            jdbc_type: 1,
            precision: 4,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            // "AAAA" is the base64 of three zero bytes.
            declared: "BYTEA",
            bound: json!({"blobValue": "AAAA"}),
            expected: json!({"blobValue": "AAAA"}),
            type_name: "bytea",
            jdbc_type: -2,
            precision: UNLIMITED,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            declared: "DATE",
            bound: json!({"stringValue": "2024-02-29"}),
            expected: json!({"stringValue": "2024-02-29"}),
            type_name: "date",
            jdbc_type: 91,
            precision: 13,
            scale: 0,
            signed: false,
            case_sensitive: false,
        },
        Case {
            declared: "TIME",
            bound: json!({"stringValue": "23:59:59"}),
            expected: json!({"stringValue": "23:59:59"}),
            type_name: "time",
            jdbc_type: 92,
            precision: 15,
            // Every date-and-time type that carries a TIME resolves to one
            // microsecond, which is the scale it reports.
            scale: 6,
            signed: false,
            case_sensitive: false,
        },
        Case {
            declared: "TIMESTAMP",
            bound: json!({"stringValue": "2024-02-29 23:59:59"}),
            expected: json!({"stringValue": "2024-02-29 23:59:59"}),
            type_name: "timestamp",
            jdbc_type: 93,
            precision: 29,
            scale: 6,
            signed: false,
            case_sensitive: false,
        },
        Case {
            declared: "TIMESTAMP WITH TIME ZONE",
            bound: json!({"stringValue": "2024-02-29 23:59:59+00"}),
            // A zone-aware timestamp is displayed in the session's zone and with no
            // zone designator, which is the zone a Data API caller does not get back.
            expected: json!({"stringValue": "2024-02-29 23:59:59"}),
            type_name: "timestamptz",
            // `java.sql.Types` has no constant for a timestamp carrying a time zone,
            // so the catalog reports the code for a type it does not name, and the
            // microsecond resolution PostgreSQL documents for the type as its scale.
            jdbc_type: 1111,
            precision: 35,
            scale: 6,
            signed: false,
            case_sensitive: false,
        },
        Case {
            declared: "UUID",
            bound: json!({"stringValue": "4bcd3f0e-7a25-4f8c-9c3a-1b2c3d4e5f60"}),
            expected: json!({"stringValue": "4bcd3f0e-7a25-4f8c-9c3a-1b2c3d4e5f60"}),
            type_name: "uuid",
            jdbc_type: 1111,
            precision: UNLIMITED,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            declared: "JSON",
            bound: json!({"stringValue": r#"{"a":1}"#}),
            expected: json!({"stringValue": r#"{"a":1}"#}),
            type_name: "json",
            jdbc_type: 1111,
            precision: UNLIMITED,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        Case {
            declared: "JSONB",
            bound: json!({"stringValue": r#"["b"]"#}),
            expected: json!({"stringValue": r#"["b"]"#}),
            type_name: "jsonb",
            jdbc_type: 1111,
            precision: UNLIMITED,
            scale: 0,
            signed: false,
            case_sensitive: true,
        },
        // The sequence types are the integer they are built on, under their own
        // name: a value bound explicitly is stored like any other.
        Case {
            declared: "SMALLSERIAL",
            bound: json!({"longValue": 7}),
            expected: json!({"longValue": 7}),
            type_name: "smallserial",
            jdbc_type: 5,
            precision: 5,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "SERIAL",
            bound: json!({"longValue": 8}),
            expected: json!({"longValue": 8}),
            type_name: "serial",
            jdbc_type: 4,
            precision: 10,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
        Case {
            declared: "BIGSERIAL",
            bound: json!({"longValue": 9}),
            expected: json!({"longValue": 9}),
            type_name: "bigserial",
            jdbc_type: -5,
            precision: 19,
            scale: 0,
            signed: true,
            case_sensitive: false,
        },
    ];

    for case in cases {
        let api = DataApi::new();
        api.run(json!({
            "sql": format!("CREATE TABLE probe (value {})", case.declared),
        }))
        .await;
        api.run(json!({
            // A value written as TEXT reaches a column of another type through a CAST
            // the statement writes (a `typeHint` is the other way of saying it), so
            // every case binds through one and what the case is about stays the
            // column's own declared type.
            "sql": format!(
                "INSERT INTO probe (value) VALUES (CAST(:value AS {}))",
                case.declared
            ),
            "parameters": [parameter("value", case.bound.clone())],
        }))
        .await;

        let selected = api
            .run(json!({
                "sql": "SELECT value FROM probe",
                "includeResultMetadata": true,
            }))
            .await;
        let label = case.declared;
        assert_eq!(
            selected["records"],
            json!([[case.expected.clone()]]),
            "case {label}: expected {}, got {selected}",
            case.expected
        );
        let metadata = list(&selected, "columnMetadata");
        let column = metadata
            .first()
            .unwrap_or_else(|| panic!("case {label}: no columnMetadata in {selected}"));
        for (member, expected, actual) in [
            ("typeName", json!(case.type_name), column["typeName"].clone()),
            ("type", json!(case.jdbc_type), column["type"].clone()),
            (
                "precision",
                json!(case.precision),
                json!(number(column, "precision")),
            ),
            ("scale", json!(case.scale), json!(number(column, "scale"))),
            ("isSigned", json!(case.signed), column["isSigned"].clone()),
            (
                "isCaseSensitive",
                json!(case.case_sensitive),
                column["isCaseSensitive"].clone(),
            ),
            // The database name alone selects the state a statement runs
            // against, so a column is in no further namespace.
            ("schemaName", json!(""), column["schemaName"].clone()),
            ("tableName", json!("probe"), column["tableName"].clone()),
            ("name", json!("value"), column["name"].clone()),
        ] {
            assert_eq!(
                actual, expected,
                "case {label}: expected {member} {expected}, got {actual} in {column}"
            );
        }
    }
}

/// RULE: a computed result column has the PostgreSQL type the statement itself
/// gives it — a cast names its target type and a literal has the type PostgreSQL
/// gives that literal — and that type decides both what `columnMetadata` reports
/// and which `Field` member carries the value.
///
/// The rows differ along the FORM that types the column (a cast of a literal, a
/// cast of an expression, a bare literal of each kind) and along the type it
/// names, including the alias spellings.
#[tokio::test]
async fn a_cast_or_a_literal_types_the_column_it_computes() {
    let cases = [
        ("1::int4", "int4", json!({"longValue": 1})),
        ("'abc'::text", "text", json!({"stringValue": "abc"})),
        ("true", "bool", json!({"booleanValue": true})),
        ("false", "bool", json!({"booleanValue": false})),
        ("2.5::numeric", "numeric", json!({"stringValue": "2.5"})),
        ("1.5::float8", "float8", json!({"doubleValue": 1.5})),
        ("NULL::text", "text", json!({"isNull": true})),
        ("1", "int4", json!({"longValue": 1})),
        ("'a literal'", "text", json!({"stringValue": "a literal"})),
        (
            "9223372036854775807",
            "int8",
            json!({"longValue": 9_223_372_036_854_775_807_i64}),
        ),
        ("(1 + 1)::int8", "int8", json!({"longValue": 2})),
        ("'7'::integer", "int4", json!({"longValue": 7})),
        (
            "'2024-02-29'::date",
            "date",
            json!({"stringValue": "2024-02-29"}),
        ),
        ("CAST(3 AS bigint)", "int8", json!({"longValue": 3})),
        ("1::bool", "bool", json!({"booleanValue": true})),
        // A typed literal is the same value as the cast of its own string.
        (
            "DATE '2024-02-29'",
            "date",
            json!({"stringValue": "2024-02-29"}),
        ),
        (
            "TIMESTAMP '2024-02-29 23:59:59'",
            "timestamp",
            json!({"stringValue": "2024-02-29 23:59:59"}),
        ),
    ];

    let api = DataApi::new();
    for (expression, type_name, expected) in cases {
        let selected = api
            .run(json!({
                "sql": format!("SELECT {expression} AS computed"),
                "includeResultMetadata": true,
            }))
            .await;
        assert_eq!(
            selected["records"],
            json!([[expected.clone()]]),
            "case {expression}: expected {expected}, got {selected}"
        );
        let metadata = list(&selected, "columnMetadata");
        assert_eq!(
            metadata.first().map(|column| column["typeName"].clone()),
            Some(json!(type_name)),
            "case {expression}: expected typeName {type_name}, got {selected}"
        );
        // A computed column is in no table, so no declaration forbids a NULL in
        // it: it is reported as a column that accepts one, and as stored in no
        // table.
        assert_eq!(
            metadata.first().map_or(-1, |column| number(column, "nullable")),
            1,
            "case {expression}: a computed column accepts a NULL, got {selected}"
        );
        assert_eq!(
            metadata.first().map(|column| column["tableName"].clone()),
            Some(json!("")),
            "case {expression}: a computed column is stored in no table, got {selected}"
        );
    }

    // The whole recorded shape of a typed SELECT in one statement: six columns,
    // one record, each value in the member its own column's type names.
    let selected = api
        .run(json!({
            "sql": "SELECT 1::int4 AS a, 'abc'::text AS b, true AS c, 2.5::numeric AS d, \
                    1.5::float8 AS e, NULL::text AS f",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(
        selected["records"],
        json!([[
            {"longValue": 1},
            {"stringValue": "abc"},
            {"booleanValue": true},
            {"stringValue": "2.5"},
            {"doubleValue": 1.5},
            {"isNull": true},
        ]]),
        "case a typed SELECT: got {selected}"
    );
    assert_eq!(
        list(&selected, "columnMetadata")
            .iter()
            .map(|column| column["typeName"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("int4"),
            json!("text"),
            json!("bool"),
            json!("numeric"),
            json!("float8"),
            json!("text")
        ],
        "case a typed SELECT: got {selected}"
    );
}

/// RULE: the statement forms PostgreSQL accepts are run, not refused: the `::`
/// cast, the standard spelling of every type, a sequence-carrying column
/// declaration, a `RETURNING` clause, an upsert, and the clauses a client builds
/// a statement from.
///
/// The rows differ along the FORM under test; each one is a statement the real
/// service runs, so each must answer 200 and leave the database it describes.
#[tokio::test]
async fn the_postgresql_statement_forms_are_run() {
    let api = DataApi::new();
    let statements = [
        (
            "a sequence-carrying declaration",
            "CREATE TABLE orders (id serial PRIMARY KEY, label text NOT NULL, \
             total numeric(12,2), placed_at timestamptz, note varchar(16))",
        ),
        (
            "an insert that leaves the sequence to assign the key",
            "INSERT INTO orders (label, total) VALUES ('first', 1.25)",
        ),
        (
            "a cast in the values of an insert",
            "INSERT INTO orders (label, total, placed_at) \
             VALUES ('second'::text, '2.50'::numeric, '2024-01-01 00:00:00+00'::timestamptz)",
        ),
        (
            "a typed literal and a clock reading in the values of an insert",
            "INSERT INTO orders (label, total, placed_at) \
             VALUES ('clocked', 3.00, now())",
        ),
        (
            "a typed literal in a predicate",
            "SELECT id FROM orders WHERE placed_at > TIMESTAMP '2000-01-01 00:00:00'",
        ),
        (
            "a cast in a predicate",
            "SELECT id FROM orders WHERE total > '1.00'::numeric",
        ),
        (
            "a cast of a column",
            "SELECT id::text AS id_text FROM orders",
        ),
        (
            "an upsert",
            "INSERT INTO orders (id, label) VALUES (1, 'again') \
             ON CONFLICT (id) DO UPDATE SET label = 'updated'",
        ),
        (
            "a returning clause",
            "INSERT INTO orders (label) VALUES ('third') RETURNING id, label",
        ),
        (
            "a concatenation, a coalesce and an order with a limit",
            "SELECT label || '!' AS shouted, COALESCE(note, 'none') AS note FROM orders \
             ORDER BY id DESC LIMIT 2 OFFSET 1",
        ),
        (
            "a common table expression",
            "WITH totals AS (SELECT id, total FROM orders) \
             SELECT count(*) AS counted FROM totals",
        ),
        (
            "a column added by an alter",
            "ALTER TABLE orders ADD COLUMN shipped boolean",
        ),
        (
            "an update through a cast",
            "UPDATE orders SET total = total + 1::numeric WHERE id = 1",
        ),
        (
            "a case-insensitive match",
            "SELECT id FROM orders WHERE label ILIKE 'UPDA%'",
        ),
        (
            "a locking read",
            "SELECT id FROM orders WHERE id = 1 FOR UPDATE",
        ),
        ("a table to empty", "CREATE TABLE counted (id int4)"),
        ("rows to empty", "INSERT INTO counted (id) VALUES (1), (2)"),
        ("a delete", "DELETE FROM orders WHERE label = 'again'"),
    ];
    for (case, sql) in statements {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 200, "case {case}: {sql} answered {status} {body}");
    }

    // A case-insensitive match finds the row whose label differs only in case,
    // which is what makes it the ILIKE and not the LIKE.
    let matched = api
        .run(json!({"sql": "SELECT label FROM orders WHERE label ILIKE 'UPDA%'"}))
        .await;
    assert_eq!(
        matched["records"],
        json!([[{"stringValue": "updated"}]]),
        "case a case-insensitive match: got {matched}"
    );

    // Emptying a table empties it, and reports no row count, which is what
    // PostgreSQL's own command tag for it carries. Every table it names is
    // emptied, because the engine runs one statement at a time.
    api.run(json!({"sql": "CREATE TABLE counted_too (id int4)"}))
        .await;
    api.run(json!({"sql": "INSERT INTO counted_too (id) VALUES (3)"}))
        .await;
    let emptied = api
        .run(json!({"sql": "TRUNCATE TABLE counted, counted_too"}))
        .await;
    assert_eq!(
        emptied["numberOfRecordsUpdated"],
        json!(0),
        "case emptying a table: it reports no row count, got {emptied}"
    );
    for table in ["counted", "counted_too"] {
        let counted = api
            .run(json!({"sql": format!("SELECT count(*) AS left_over FROM {table}")}))
            .await;
        assert_eq!(
            counted["records"],
            json!([[{"longValue": 0}]]),
            "case emptying {table}: every row is gone, got {counted}"
        );
    }

    // The statements above left a table whose sequence assigned every key, which
    // is what a read of it reports.
    let selected = api
        .run(json!({
            "sql": "SELECT id, label FROM orders ORDER BY id",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(
        selected["records"],
        json!([
            [{"longValue": 1}, {"stringValue": "updated"}],
            [{"longValue": 2}, {"stringValue": "second"}],
            [{"longValue": 3}, {"stringValue": "clocked"}],
            [{"longValue": 4}, {"stringValue": "third"}],
        ]),
        "the keys the sequence assigned are the stored ids, got {selected}"
    );
    let metadata = list(&selected, "columnMetadata");
    assert_eq!(
        metadata[0]["isAutoIncrement"],
        json!(true),
        "case id: a sequence-carrying column reports its sequence, got {metadata:?}"
    );
    assert_eq!(
        number(&metadata[0], "nullable"),
        0,
        "case id: a primary key is NOT NULL, got {metadata:?}"
    );
}

/// RULE: a sequence assigns the next value above the largest the column holds
/// whenever an INSERT leaves the column out, whether or not the column is its
/// table's key, and the row count the call reports is the rows the STATEMENT
/// changed.
///
/// The rows differ along where the sequence-carrying column sits: the table's
/// primary key, and a column that is not the key.
#[tokio::test]
async fn a_sequence_assigns_every_column_that_declares_one() {
    for (case, declaration) in [
        (
            "the primary key",
            "CREATE TABLE tickets (id serial PRIMARY KEY, label text)",
        ),
        (
            "a column that is not the key",
            "CREATE TABLE tickets (label text PRIMARY KEY, id bigserial)",
        ),
    ] {
        let api = DataApi::new();
        api.run(json!({"sql": declaration})).await;
        for label in ["first", "second", "third"] {
            let inserted = api
                .run(json!({
                    "sql": "INSERT INTO tickets (label) VALUES (:label)",
                    "parameters": [parameter("label", json!({"stringValue": label}))],
                }))
                .await;
            assert_eq!(
                number(&inserted, "numberOfRecordsUpdated"),
                1,
                "case {case}: one row was inserted, got {inserted}"
            );
            assert_eq!(
                list(&inserted, "generatedFields"),
                Vec::<Value>::new(),
                "case {case}: generatedFields is not supported by this engine, got {inserted}"
            );
        }
        let selected = api
            .run(json!({"sql": "SELECT id FROM tickets ORDER BY id"}))
            .await;
        assert_eq!(
            selected["records"],
            json!([[{"longValue": 1}], [{"longValue": 2}], [{"longValue": 3}]]),
            "case {case}: the sequence assigned each row the next value, got {selected}"
        );
        // The JSON rendering of the same result set carries the same keys.
        let formatted = api
            .run(json!({
                "sql": "SELECT id, label FROM tickets ORDER BY id",
                "formatRecordsAs": "JSON",
            }))
            .await;
        assert_eq!(
            formatted["formattedRecords"],
            json!(
                "[{\"id\":1,\"label\":\"first\"},{\"id\":2,\"label\":\"second\"},\
                 {\"id\":3,\"label\":\"third\"}]"
            ),
            "case {case}: got {formatted}"
        );
    }
}

/// RULE: a result column whose PostgreSQL type has no `Field` member to come back
/// in is refused with `UnsupportedResultException` (HTTP 400) naming that type,
/// and an array of arrays is refused as the multidimensional array the model
/// names — the statement is valid and runs, it is the RESULT that cannot be
/// returned. A ONE-dimensional array HAS a member, `arrayValue`, so it is
/// returned, with its elements in the list its element type names.
///
/// The rows differ along WHERE the type comes from (a cast, a stored column's
/// declaration, a column of a wider result set, an array constructor) and along
/// the type itself.
#[tokio::test]
async fn an_array_is_returned_and_a_result_with_no_field_member_is_refused() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE spans (label text, span interval, tags text[])"}))
        .await;

    for (case, sql, message) in [
        (
            "a cast to interval",
            "SELECT '1 day'::interval AS iv".to_string(),
            "The result contains the unsupported data type INTERVAL.",
        ),
        (
            "an interval column",
            "SELECT span FROM spans".to_string(),
            "The result contains the unsupported data type INTERVAL.",
        ),
        (
            "an interval beside a type that is supported",
            "SELECT label, span FROM spans".to_string(),
            "The result contains the unsupported data type INTERVAL.",
        ),
        (
            "an array of arrays",
            "SELECT ARRAY[[1, 2], [3, 4]] AS grid".to_string(),
            "The result contains a multidimensional array.",
        ),
        (
            "a braced literal of arrays",
            "SELECT '{{1,2},{3,4}}'::int4[] AS grid".to_string(),
            "The result contains a multidimensional array.",
        ),
        (
            "a type nested in brackets far past the point it means anything",
            format!("SELECT '{{}}'::int4{} AS deep", "[]".repeat(500)),
            "The result contains a multidimensional array.",
        ),
    ] {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 400, "case {case}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "UnsupportedResultException",
            "case {case}: got {body}"
        );
        assert_eq!(
            body["message"],
            json!(message),
            "case {case}: got {body}"
        );
    }

    // A one-dimensional array comes back as one `arrayValue`, its elements in the
    // list its element type names.
    for (case, sql, expected, type_name) in [
        (
            "a constructed array of integers",
            "SELECT ARRAY[1, 2, 3] AS numbers",
            json!({"arrayValue": {"longValues": [1, 2, 3]}}),
            "_int4",
        ),
        (
            "a constructed array of strings",
            "SELECT ARRAY['a', 'b'] AS words",
            json!({"arrayValue": {"stringValues": ["a", "b"]}}),
            "_text",
        ),
        (
            "a braced literal of integers",
            "SELECT '{1,2}'::int4[] AS numbers",
            json!({"arrayValue": {"longValues": [1, 2]}}),
            "_int4",
        ),
        (
            "a braced literal of strings",
            "SELECT '{\"a\",b}'::text[] AS words",
            json!({"arrayValue": {"stringValues": ["a", "b"]}}),
            "_text",
        ),
        (
            "a braced literal of booleans",
            "SELECT '{t,f}'::bool[] AS flags",
            json!({"arrayValue": {"booleanValues": [true, false]}}),
            "_bool",
        ),
        (
            "a braced literal of floating-point numbers",
            "SELECT '{1.5,-0.25}'::float8[] AS scores",
            json!({"arrayValue": {"doubleValues": [1.5, -0.25]}}),
            "_float8",
        ),
        (
            "an empty array",
            "SELECT ARRAY[]::text[] AS none",
            json!({"arrayValue": {"stringValues": []}}),
            "_text",
        ),
    ] {
        let selected = api
            .run(json!({"sql": sql, "includeResultMetadata": true}))
            .await;
        assert_eq!(
            selected["records"],
            json!([[expected.clone()]]),
            "case {case}: expected {expected}, got {selected}"
        );
        let metadata = list(&selected, "columnMetadata");
        assert_eq!(
            metadata.first().map(|column| column["typeName"].clone()),
            Some(json!(type_name)),
            "case {case}: expected typeName {type_name}, got {selected}"
        );
        // Every array type carries one `java.sql.Types` code.
        assert_eq!(
            metadata.first().map_or(0, |column| number(column, "type")),
            2003,
            "case {case}: got {selected}"
        );
    }

    // An array column holds what was stored in it, and comes back the same way.
    api.run(json!({
        "sql": "INSERT INTO spans (label, tags) VALUES ('tagged', ARRAY['red', 'blue'])",
    }))
    .await;
    // A span of time is stored like any other value the engine has no class of
    // its own for; it is only the RESULT that cannot be returned.
    api.run(json!({
        "sql": "INSERT INTO spans (label, span) VALUES ('timed', interval '1 day')",
    }))
    .await;
    let (status, refused) = api
        .execute(json!({"sql": "SELECT span FROM spans WHERE label = 'timed'"}))
        .await;
    assert_eq!(
        status, 400,
        "case a stored span: the stored value is still a result that cannot be \
         returned, got {refused}"
    );
    let stored = api
        .run(json!({
            "sql": "SELECT tags FROM spans WHERE label = 'tagged'",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(
        stored["records"],
        json!([[{"arrayValue": {"stringValues": ["red", "blue"]}}]]),
        "case a stored array column: got {stored}"
    );
    assert_eq!(
        list(&stored, "columnMetadata")
            .first()
            .map(|column| column["typeName"].clone()),
        Some(json!("_text")),
        "case a stored array column: got {stored}"
    );

    // The control: a result of a type that HAS a member is served, so the
    // refusals above are about the type and not about the statement.
    for (case, sql) in [
        ("a cast to a supported type", "SELECT '1'::int4 AS supported"),
        ("a supported column", "SELECT label FROM spans"),
    ] {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 200, "case {case}: got {status} {body}");
    }
}

/// RULE: a call runs ONE statement (the model's `sql` member is "The SQL
/// statement"), and a statement nests only so deeply before the parse refuses it,
/// so neither a second statement nor an unbounded nesting rides in on one call.
#[tokio::test]
async fn one_call_runs_one_bounded_statement() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE counted (id int4)"}))
        .await;

    for (case, sql, expected) in [
        (
            "two statements",
            "INSERT INTO counted (id) VALUES (1); INSERT INTO counted (id) VALUES (2)".to_string(),
            // More than one call runs: a fault of the CALL, refused before the engine
            // saw it.
            "ValidationException",
        ),
        (
            "a statement nested past the bound",
            format!("SELECT {}1{}", "abs(".repeat(64), ")".repeat(64)),
            // The ENGINE's own depth limit refused it, so it is the database's error.
            "DatabaseErrorException",
        ),
    ] {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 400, "case {case}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            expected,
            "case {case}: expected {expected}, got {body}"
        );
    }

    // Nothing the refused statements described was stored, and a statement
    // within the bound is still run.
    let selected = api
        .run(json!({"sql": "SELECT abs(abs(abs(-1))) AS nested, count(*) AS rows FROM counted"}))
        .await;
    assert_eq!(
        selected["records"],
        json!([[{"longValue": 1}, {"longValue": 0}]]),
        "a statement within the bound runs, and the refused ones stored nothing, got {selected}"
    );
}

/// RULE: the text an array value reaches the engine as is read in the two forms
/// an array value has — the JSON document this data plane builds for an array
/// constructor, and PostgreSQL's own braced literal — and a text that is neither
/// is refused, never guessed at. The braced form has no crate in this tree that
/// reads it, so the reader here is BOUNDED: it nests only as deeply as it takes
/// to see that an array holds arrays, it recurses no further on whatever depth a
/// caller sends, and the value it reads is already bounded by the size one value
/// may have.
///
/// The rows are the inputs that break a reader: malformed, truncated, nested past
/// the bound, empty, and far longer than any example — each answered, none
/// hanging and none guessed at.
#[tokio::test]
async fn the_array_reader_refuses_what_is_not_an_array() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE stored (label text, tags text[])"}))
        .await;

    // Each case is stored as the text it is, and read back as an array.
    let long = format!("{{{}}}", (0..2_000).map(|n| n.to_string()).collect::<Vec<_>>().join(","));
    let cases = [
        ("the empty array", "{}".to_string(), Some(0)),
        ("one element", "{a}".to_string(), Some(1)),
        ("a quoted element holding a brace", "{\"a}b\"}".to_string(), Some(1)),
        ("an element that is NULL", "{NULL}".to_string(), None),
        ("truncated", "{1,2".to_string(), None),
        ("not an array at all", "plain text".to_string(), None),
        ("nested", "{{1,2},{3,4}}".to_string(), None),
        ("nested past the bound", "{{{{1}}}}".to_string(), None),
        ("empty text", String::new(), None),
        ("two thousand elements", long, Some(2_000)),
    ];
    for (case, stored, expected) in cases {
        api.run(json!({
            // The array column takes the text through the cast the statement writes:
            // a character value reaches a column of another type that way.
            "sql": "INSERT INTO stored (label, tags) VALUES (:label, CAST(:tags AS text[]))",
            "parameters": [
                parameter("label", json!({"stringValue": case})),
                parameter("tags", json!({"stringValue": stored})),
            ],
        }))
        .await;

        let (status, body) = api
            .execute(json!({
                "sql": "SELECT tags FROM stored WHERE label = :label",
                "parameters": [parameter("label", json!({"stringValue": case}))],
            }))
            .await;
        match expected {
            Some(elements) => {
                assert_eq!(status, 200, "case {case}: got {status} {body}");
                assert_eq!(
                    body["records"][0][0]["arrayValue"]["stringValues"]
                        .as_array()
                        .map(Vec::len),
                    Some(elements),
                    "case {case}: expected {elements} elements, got {body}"
                );
            }
            None => {
                assert_eq!(status, 400, "case {case}: got {status} {body}");
                assert_eq!(
                    error_code(&body),
                    "UnsupportedResultException",
                    "case {case}: got {body}"
                );
            }
        }
    }
}

/// RULE: a statement is bounded by how many levels it asks the data plane to
/// chain, as well as by how deeply it nests: a combined query and an operator
/// chain are parsed iteratively, so their depth grows with the request text,
/// while the tree they build is walked and dropped by recursion. The bound is therefore
/// decided on the statement's TOKENS, before it is parsed at all; a statement past
/// either bound is refused with HTTP 400 on every operation that takes one, and the
/// data plane serves the next request.
///
/// The rows differ along WHICH chain the statement grows — the queries it
/// combines, the conditions it ands, the values it concatenates — each one far
/// past what a client writes, and each paired with a statement of the same shape
/// that is inside the bound and runs; the last row fills the largest `sql` the model
/// carries (65,536 bytes), which is the most tree one call could ask for.
#[tokio::test]
async fn a_statement_is_bounded_by_the_levels_it_chains() {
    let api = DataApi::new();

    for (case, inside, past) in [
        (
            "queries combined",
            format!("SELECT 1 AS x{}", " UNION ALL SELECT 1".repeat(8)),
            format!("SELECT 1 AS x{}", " UNION ALL SELECT 1".repeat(2_000)),
        ),
        (
            "conditions anded",
            format!("SELECT 1 AS x WHERE 1 = 1{}", " AND 1 = 1".repeat(8)),
            format!("SELECT 1 AS x WHERE 1 = 1{}", " AND 1 = 1".repeat(2_000)),
        ),
        (
            "values concatenated",
            format!("SELECT 'a'{} AS joined", " || 'a'".repeat(8)),
            format!("SELECT 'a'{} AS joined", " || 'a'".repeat(2_000)),
        ),
        (
            "numbers added",
            format!("SELECT 1{} AS total", " + 1".repeat(8)),
            format!("SELECT 1{} AS total", " + 1".repeat(5_000)),
        ),
        (
            "casts chained",
            format!("SELECT 'a'{} AS cast_over", "::text".repeat(8)),
            format!("SELECT 'a'{} AS cast_over", "::text".repeat(2_000)),
        ),
        (
            // 4 bytes per level, so this fills the model's own `sql` ceiling.
            "numbers added to the largest sql the model carries",
            format!("SELECT 1{} AS total", " + 1".repeat(8)),
            format!("SELECT 1{} AS total", " + 1".repeat(16_000)),
        ),
    ] {
        let (status, body) = api.execute(json!({"sql": inside})).await;
        assert_eq!(
            status, 200,
            "case {case} inside the bound: got {status} {body}"
        );

        // On both operations that take a statement: the bound is the statement's, not
        // one operation's.
        let (status, body) = api.execute(json!({"sql": past.clone()})).await;
        assert_eq!(status, 400, "case {case} past the bound: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} past the bound: got {body}"
        );
        let (status, body) = api
            .batch(json!({"sql": past, "parameterSets": [[]]}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case} past the bound as a batch: got {status} {body}"
        );

        // The data plane is still there, which is the point of the bound.
        let served = api.run(json!({"sql": "SELECT 1 AS still_here"})).await;
        assert_eq!(
            served["records"],
            json!([[{"longValue": 1}]]),
            "case {case}: the data plane serves the next request, got {served}"
        );
    }
}

/// RULE: the sequence of a table's KEY is not part of the transaction that takes
/// values out of it — PostgreSQL documents that a value `nextval` handed out is
/// not given back when the transaction rolls back — and it never hands the same
/// value out twice, so a value a deleted row was given is not given again either.
/// (A sequence on a column that is NOT the key takes the next value above the
/// largest the column holds, which `src/state/postgres.rs` states, so it does
/// re-use a deleted row's value; only the key's counter is one the engine keeps.)
///
/// The rows differ along WHAT consumed the value and did not keep it: a
/// transaction that rolled back, and a row that was deleted.
#[tokio::test]
async fn a_sequence_never_hands_out_a_value_twice() {
    for (case, consume) in [
        ("a transaction that rolled back", "rollback"),
        ("a row that was deleted", "delete"),
    ] {
        let api = DataApi::new();
        api.run(json!({"sql": "CREATE TABLE tickets (id serial PRIMARY KEY, label text)"}))
            .await;
        api.run(json!({"sql": "INSERT INTO tickets (label) VALUES ('first')"}))
            .await;

        if consume == "rollback" {
            let transaction = api.begin_transaction(None).await;
            api.run(json!({
                "sql": "INSERT INTO tickets (label) VALUES ('rolled back')",
                "transactionId": transaction,
            }))
            .await;
            let (status, body) = api.rollback(&transaction).await;
            assert_eq!(status, 200, "case {case}: rollback failed with {body}");
        } else {
            api.run(json!({"sql": "INSERT INTO tickets (label) VALUES ('deleted')"}))
                .await;
            api.run(json!({"sql": "DELETE FROM tickets WHERE label = 'deleted'"}))
                .await;
        }

        // The value the lost row was given is gone with it: the next row gets
        // the one after it.
        let inserted = api
            .run(json!({
                "sql": "INSERT INTO tickets (label) VALUES ('after') RETURNING id",
            }))
            .await;
        assert_eq!(
            inserted["records"],
            json!([[{"longValue": 3}]]),
            "case {case}: the sequence does not hand out a value it already gave, got {inserted}"
        );
        let selected = api
            .run(json!({"sql": "SELECT id, label FROM tickets ORDER BY id"}))
            .await;
        assert_eq!(
            selected["records"],
            json!([
                [{"longValue": 1}, {"stringValue": "first"}],
                [{"longValue": 3}, {"stringValue": "after"}],
            ]),
            "case {case}: the stored keys skip the value nothing kept, got {selected}"
        );
    }
}

/// RULE: `RollbackTransaction` reports the status of the rollback operation,
/// which is the status the service reports for a rollback that finished.
#[tokio::test]
async fn a_rollback_reports_the_status_of_the_rollback() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE ledger (id int4)"}))
        .await;

    for case in ["a transaction that wrote nothing", "one that wrote a row"] {
        let transaction = api.begin_transaction(None).await;
        if case == "one that wrote a row" {
            api.run(json!({
                "sql": "INSERT INTO ledger (id) VALUES (1)",
                "transactionId": transaction,
            }))
            .await;
        }
        let (status, body) = api.rollback(&transaction).await;
        assert_eq!(status, 200, "case {case}: got {status} {body}");
        assert_eq!(
            body["transactionStatus"],
            json!("Rollback complete"),
            "case {case}: got {body}"
        );
    }

    let selected = api.run(json!({"sql": "SELECT id FROM ledger"})).await;
    assert!(
        list(&selected, "records").is_empty(),
        "a rolled-back write reaches no later reader, got {selected}"
    );
}
