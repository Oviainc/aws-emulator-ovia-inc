// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a table created with `VectorIndexes`.
//!
//! The rules pinned here:
//!
//! 1. an `AttributeDefinitions` entry is USED — and so never refused with "Some
//!    AttributeDefinitions are not used in any KeySchema" — when ANY schema of the table
//!    consumes it: the table's own `KeySchema`, the `KeySchema` of a global or local
//!    secondary index, or the `VectorAttribute`/`SearchSchema` of a vector index. Only a
//!    definition no schema consumes is refused, by name;
//! 2. a vector index the create declared is stored and reported back in the table's
//!    `TableDescription` — by `CreateTable` and by the `DescribeTable` that reads the
//!    table afterwards — with the configuration the request gave it, its own `IndexArn`,
//!    and `CREATING` until the table's clock-owed transition makes it `ACTIVE`;
//! 3. an index name already taken on the table is refused, whichever index kind took it.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the element type the search schema names, the type and number of the
//! attributes it consumes, the distance function, the projection — rather than one
//! example.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so the table's `CREATING` -> `ACTIVE` transition and its
/// indexes' are due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One on-demand `CreateTable` body with a `pk` partition key, the attribute definitions
/// and the vector indexes given.
fn create_request(name: &str, definitions: Value, vector_indexes: Value) -> Value {
    json!({
        "TableName": name,
        "BillingMode": "PAY_PER_REQUEST",
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": definitions,
        "VectorIndexes": vector_indexes,
    })
}

/// One vector index body: the embedding attribute, its dimensions, the distance function,
/// the projection and the search schema.
fn vector_index(
    name: &str,
    dimensions: i64,
    distance_function: &str,
    projection: Value,
    search_schema: Value,
) -> Value {
    let mut index = json!({
        "IndexName": name,
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": dimensions,
        "DistanceFunction": distance_function,
        "Projection": projection,
    });
    if !search_schema.is_null() {
        index["SearchSchema"] = search_schema;
    }
    index
}

/// The `VectorIndexes` entry of a table description by index name, or `Null` when the
/// description carries no vector index by that name.
fn vector_index_named(description: &Value, name: &str) -> Value {
    description["VectorIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|index| index["IndexName"] == json!(name))
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn an_attribute_definition_a_vector_index_consumes_is_not_refused_as_unused() {
    let router = emulator();

    // Rows differ along what the rule generalises over: which member of the vector index
    // consumes the attribute (its `SearchSchema`, its `VectorAttribute`), the element type
    // the search schema gives it (`HASH`, `INLINE_FILTER`, both — the two values
    // `smithy_query(query="shape", shape="SearchSchemaElementType")` declares), the
    // attribute's own type (S, N, B), how many attributes the index consumes, and the
    // projection the index carries. None of these attributes is in any KeySchema, which is
    // exactly what used to be refused. The last column is the attribute names the created
    // table must report as its `AttributeDefinitions`, in the order a description gives
    // them (by name).
    let cases: [(&str, Value, Value, &[&str]); 6] = [
        (
            "one HASH search-schema attribute",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "tenant", "AttributeType": "S"}]),
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
            &["pk", "tenant"],
        ),
        (
            "one INLINE_FILTER attribute, numeric",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "price", "AttributeType": "N"}]),
            json!([{"AttributeName": "price", "SearchSchemaElementType": "INLINE_FILTER"}]),
            &["pk", "price"],
        ),
        (
            "one INLINE_FILTER attribute, binary",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "fingerprint", "AttributeType": "B"}]),
            json!([{"AttributeName": "fingerprint", "SearchSchemaElementType": "INLINE_FILTER"}]),
            &["fingerprint", "pk"],
        ),
        (
            "a HASH partition and two inline filters",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "tenant", "AttributeType": "S"},
                   {"AttributeName": "tier", "AttributeType": "S"},
                   {"AttributeName": "price", "AttributeType": "N"}]),
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"},
                   {"AttributeName": "tier", "SearchSchemaElementType": "INLINE_FILTER"},
                   {"AttributeName": "price", "SearchSchemaElementType": "INLINE_FILTER"}]),
            &["pk", "price", "tenant", "tier"],
        ),
        (
            "no search schema, the vector attribute declared",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "emb", "AttributeType": "B"}]),
            Value::Null,
            &["emb", "pk"],
        ),
        (
            "a search schema attribute that is also the table key",
            json!([{"AttributeName": "pk", "AttributeType": "S"}]),
            json!([{"AttributeName": "pk", "SearchSchemaElementType": "HASH"}]),
            &["pk"],
        ),
    ];

    for (position, (case, definitions, search_schema, expected)) in
        cases.into_iter().enumerate()
    {
        let table = format!("VectorUsed{position}");
        let (status, body) = call(
            &router,
            "CreateTable",
            create_request(
                &table,
                definitions,
                json!([vector_index(
                    "idx.search",
                    4,
                    "EUCLIDEAN",
                    json!({"ProjectionType": "ALL"}),
                    search_schema,
                )]),
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case {case}: a definition a vector index consumes is accepted, got {status} with \
             {body}"
        );
        // The definition really was kept: the created table reports every attribute the
        // request declared, so nothing was accepted and then dropped.
        let reported: Vec<String> = body["TableDescription"]["AttributeDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|definition| {
                definition["AttributeName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: every declared definition is kept, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_request_past_a_vector_index_bound_is_refused_before_it_is_walked() {
    let router = emulator();
    let definitions = json!([
        {"AttributeName": "pk", "AttributeType": "S"},
        {"AttributeName": "tenant", "AttributeType": "S"},
    ]);
    let element = json!({"AttributeName": "tenant", "SearchSchemaElementType": "INLINE_FILTER"});

    // Rows are the two caller-sized lists a vector-index create carries, each asserted at
    // its published bound and one element past it: the bounds are
    // `MAX_VECTOR_INDEXES` and `MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS`, which this crate
    // exports, so the rows move with them.
    use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
        MAX_VECTOR_INDEXES, MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS,
    };
    let indexes = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|ordinal| {
                    vector_index(
                        &format!("idx.search{ordinal}"),
                        4,
                        "EUCLIDEAN",
                        json!({"ProjectionType": "ALL"}),
                        json!([element]),
                    )
                })
                .collect(),
        )
    };
    let wide_schema = |count: usize| -> Value {
        json!([vector_index(
            "idx.search",
            4,
            "EUCLIDEAN",
            json!({"ProjectionType": "ALL"}),
            Value::Array((0..count).map(|_| element.clone()).collect()),
        )])
    };

    let cases: [(&str, Value, Option<String>); 4] = [
        ("as many vector indexes as a table may carry", indexes(MAX_VECTOR_INDEXES), None),
        (
            "one vector index too many",
            indexes(MAX_VECTOR_INDEXES + 1),
            Some(format!(
                "Invalid VectorIndexes: a table may carry at most {MAX_VECTOR_INDEXES} vector \
                 indexes, got {}",
                MAX_VECTOR_INDEXES + 1
            )),
        ),
        (
            "as many search-schema elements as one index may carry",
            wide_schema(MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS),
            None,
        ),
        (
            "one search-schema element too many",
            wide_schema(MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS + 1),
            Some(format!(
                "Invalid SearchSchema for index idx.search: a vector index's search schema may \
                 carry at most {MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS} elements, got {}",
                MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS + 1
            )),
        ),
    ];

    for (position, (case, vector_indexes, refusal)) in cases.into_iter().enumerate() {
        let table = format!("VectorBound{position}");
        let (status, body) = call(
            &router,
            "CreateTable",
            create_request(&table, definitions.clone(), vector_indexes),
        )
        .await;
        match refusal {
            Some(expected) => assert_eq!(
                (status, error_code(&body), message(&body)),
                (400, "ValidationException".to_string(), expected),
                "case {case}: the request is refused at the bound, got {status} with {body}"
            ),
            None => assert_eq!(
                (status, error_code(&body).as_str()),
                (200, "<no __type>"),
                "case {case}: a request at the bound is served, got {status} with {body}"
            ),
        }
    }
}

#[tokio::test]
async fn a_definition_no_schema_consumes_is_still_refused_by_name() {
    let router = emulator();

    // The orphan-definition refusal still fires, and names the definition no schema
    // consumes. Rows differ in what the table DOES carry beside the orphan — nothing, a
    // vector index, a global secondary index — and in the orphan's own name and type, so
    // the message's trailing value is read from the request rather than fixed.
    let cases: [(&str, Value, Value, &str); 3] = [
        (
            "no index at all",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "spare", "AttributeType": "S"}]),
            json!([]),
            "spare",
        ),
        (
            "a vector index that consumes another attribute",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "tenant", "AttributeType": "S"},
                   {"AttributeName": "unused", "AttributeType": "N"}]),
            json!([vector_index(
                "idx.search",
                8,
                "COSINE",
                json!({"ProjectionType": "KEYS_ONLY"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
            )]),
            "unused",
        ),
        (
            "a vector index whose search schema is empty",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "orphan", "AttributeType": "B"}]),
            json!([vector_index(
                "idx.search",
                2,
                "DOT_PRODUCT",
                json!({"ProjectionType": "ALL"}),
                Value::Null,
            )]),
            "orphan",
        ),
    ];

    for (position, (case, definitions, vector_indexes, orphan)) in cases.into_iter().enumerate() {
        let table = format!("VectorOrphan{position}");
        let (status, body) = call(
            &router,
            "CreateTable",
            create_request(&table, definitions, vector_indexes),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                format!("Some AttributeDefinitions are not used in any KeySchema: {orphan}")
            ),
            "case {case}: the orphan definition is refused by name, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_search_schema_attribute_the_request_declares_no_definition_for_is_refused() {
    let router = emulator();

    // The converse of the usage rule, and the same rule an index key schema is held to: a
    // search schema partitions and filters by an attribute's VALUE, so that attribute
    // needs an `AttributeDefinitions` entry. Rows differ in the element type naming the
    // undeclared attribute and in which position of the schema it sits in.
    let cases: [(&str, Value, &str); 3] = [
        (
            "a HASH element on an undeclared attribute",
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
            "tenant",
        ),
        (
            "an INLINE_FILTER element on an undeclared attribute",
            json!([{"AttributeName": "price", "SearchSchemaElementType": "INLINE_FILTER"}]),
            "price",
        ),
        (
            "the second element of the schema undeclared",
            json!([{"AttributeName": "pk", "SearchSchemaElementType": "HASH"},
                   {"AttributeName": "tier", "SearchSchemaElementType": "INLINE_FILTER"}]),
            "tier",
        ),
    ];

    for (position, (case, search_schema, undeclared)) in cases.into_iter().enumerate() {
        let table = format!("VectorUndeclared{position}");
        let (status, body) = call(
            &router,
            "CreateTable",
            create_request(
                &table,
                json!([{"AttributeName": "pk", "AttributeType": "S"}]),
                json!([vector_index(
                    "idx.search",
                    4,
                    "EUCLIDEAN",
                    json!({"ProjectionType": "ALL"}),
                    search_schema,
                )]),
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                format!(
                    "Invalid SearchSchema for index idx.search: no AttributeDefinition was \
                     provided for search schema attribute {undeclared}"
                )
            ),
            "case {case}: the undeclared search schema attribute is refused by name, got \
             {status} with {body}"
        );
    }

    // The vector attribute itself needs no definition — no schema is built over the
    // vector — so a create that declares only the table key and the search schema's
    // attribute is served.
    ok(
        &router,
        "CreateTable",
        create_request(
            "VectorDeclared",
            json!([{"AttributeName": "pk", "AttributeType": "S"},
                   {"AttributeName": "tenant", "AttributeType": "S"}]),
            json!([vector_index(
                "idx.search",
                4,
                "EUCLIDEAN",
                json!({"ProjectionType": "ALL"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
            )]),
        ),
    )
    .await;
}

#[tokio::test]
async fn a_restore_carries_the_source_table_vector_indexes_over() {
    let router = emulator();
    let definitions = json!([
        {"AttributeName": "pk", "AttributeType": "S"},
        {"AttributeName": "tenant", "AttributeType": "S"},
    ]);
    let requested = vector_index(
        "idx.search",
        4,
        "COSINE",
        json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["title"]}),
        json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
    );
    ok(
        &router,
        "CreateTable",
        create_request("VectorSource", definitions, json!([requested.clone()])),
    )
    .await;
    settle().await;

    // Both restores say the same thing: "If not specified, all vector indexes from the
    // source table / from the backup are restored". Rows are the two of them, each with
    // the call that prepares it.
    let backup = ok(
        &router,
        "CreateBackup",
        json!({"TableName": "VectorSource", "BackupName": "VectorBackup"}),
    )
    .await;
    let backup_arn = backup["BackupDetails"]["BackupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // A backup "is created asynchronously": it has to be AVAILABLE before it can be
    // restored from.
    settle().await;
    // The backup RECORDS the configuration, so a reader of the backup reports it too.
    let described_backup = ok(
        &router,
        "DescribeBackup",
        json!({"BackupArn": backup_arn.clone()}),
    )
    .await;
    assert_eq!(
        described_backup["BackupDescription"]["SourceTableFeatureDetails"]["VectorIndexes"]
            .clone(),
        json!([{
            "IndexName": "idx.search",
            "VectorAttribute": {"AttributeName": "emb"},
            "SearchSchema": [{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}],
            "Projection": {"ProjectionType": "INCLUDE", "NonKeyAttributes": ["title"]},
            "Dimensions": 4,
            "DistanceFunction": "COSINE",
        }]),
        "case the backup: it records the source table's vector indexes, got {described_backup}"
    );
    ok(
        &router,
        "UpdateContinuousBackups",
        json!({
            "TableName": "VectorSource",
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
        }),
    )
    .await;

    let restores: [(&str, &str, &str, Value); 2] = [
        (
            "from the backup",
            "RestoreTableFromBackup",
            "VectorFromBackup",
            json!({"BackupArn": backup_arn, "TargetTableName": "VectorFromBackup"}),
        ),
        (
            "to the latest restorable point",
            "RestoreTableToPointInTime",
            "VectorFromPitr",
            json!({
                "SourceTableName": "VectorSource",
                "TargetTableName": "VectorFromPitr",
                "UseLatestRestorableTime": true,
            }),
        ),
    ];

    for (case, operation, target, request) in restores {
        let restored = ok(&router, operation, request).await;
        settle().await;
        let read_back = ok(&router, "DescribeTable", json!({"TableName": target})).await;
        let reported = vector_index_named(&read_back["Table"], "idx.search");
        let table_arn = read_back["Table"]["TableArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (
                vector_index_named(&restored["TableDescription"], "idx.search")["IndexStatus"]
                    .clone(),
                reported["IndexStatus"].clone(),
                reported["Dimensions"].clone(),
                reported["DistanceFunction"].clone(),
                reported["VectorAttribute"].clone(),
                reported["SearchSchema"].clone(),
                reported["Projection"].clone(),
                reported["IndexArn"].clone(),
            ),
            (
                json!("CREATING"),
                json!("ACTIVE"),
                json!(4),
                json!("COSINE"),
                json!({"AttributeName": "emb"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
                json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["title"]}),
                json!(format!("{table_arn}/index/idx.search")),
            ),
            "case {case}: the restored table carries the vector index under its own ARN, got \
             {restored} then {read_back}"
        );
    }
}

#[tokio::test]
async fn a_created_vector_index_is_reported_back_and_becomes_active() {
    let router = emulator();

    // Rows differ in the distance function (every value
    // `smithy_query(query="shape", shape="VectorDistanceFunction")` declares: COSINE,
    // DOT_PRODUCT, EUCLIDEAN), the dimensions, the projection and the search schema, so
    // what is reported back is read from the request rather than fixed.
    let cases: [(&str, i64, &str, Value, Value); 3] = [
        (
            "cosine, projecting everything",
            4,
            "COSINE",
            json!({"ProjectionType": "ALL"}),
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
        ),
        (
            "dot product, keys only",
            1,
            "DOT_PRODUCT",
            json!({"ProjectionType": "KEYS_ONLY"}),
            json!([{"AttributeName": "tier", "SearchSchemaElementType": "INLINE_FILTER"}]),
        ),
        (
            "euclidean, including one attribute",
            1536,
            "EUCLIDEAN",
            json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["title"]}),
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"},
                   {"AttributeName": "tier", "SearchSchemaElementType": "INLINE_FILTER"}]),
        ),
    ];

    for (position, (case, dimensions, distance_function, projection, search_schema)) in
        cases.into_iter().enumerate()
    {
        let table = format!("VectorReported{position}");
        let requested = vector_index(
            "idx.search",
            dimensions,
            distance_function,
            projection.clone(),
            search_schema.clone(),
        );
        // The table declares its own key and exactly the attributes this row's search
        // schema consumes — every definition is used by some schema, which is the rule the
        // suite's first test pins.
        let mut definitions = vec![json!({"AttributeName": "pk", "AttributeType": "S"})];
        for element in search_schema.as_array().cloned().unwrap_or_default() {
            definitions.push(json!({
                "AttributeName": element["AttributeName"].clone(),
                "AttributeType": "S",
            }));
        }
        let created = ok(
            &router,
            "CreateTable",
            create_request(&table, Value::Array(definitions), json!([requested])),
        )
        .await;

        // The create reports the index CREATING, with the configuration it was given and
        // an ARN of its own under the table's.
        let reported = vector_index_named(&created["TableDescription"], "idx.search");
        let table_arn = created["TableDescription"]["TableArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (
                reported["IndexStatus"].clone(),
                reported["Dimensions"].clone(),
                reported["DistanceFunction"].clone(),
                reported["VectorAttribute"].clone(),
                reported["SearchSchema"].clone(),
                reported["Projection"].clone(),
                reported["IndexArn"].clone(),
            ),
            (
                json!("CREATING"),
                json!(dimensions),
                json!(distance_function),
                json!({"AttributeName": "emb"}),
                search_schema.clone(),
                projection.clone(),
                json!(format!("{table_arn}/index/idx.search")),
            ),
            "case {case}: the create reports the index it stored, got {created}"
        );

        // Reading the table back through DescribeTable reports the same stored index, now
        // ACTIVE because the clock has made the table's transition due. Its metered
        // figures are those of a table that holds no items.
        settle().await;
        let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
        let read_back = vector_index_named(&described["Table"], "idx.search");
        assert_eq!(
            (
                read_back["IndexStatus"].clone(),
                read_back["Dimensions"].clone(),
                read_back["DistanceFunction"].clone(),
                read_back["SearchSchema"].clone(),
                read_back["ItemCount"].clone(),
                read_back["IndexSizeBytes"].clone(),
            ),
            (
                json!("ACTIVE"),
                json!(dimensions),
                json!(distance_function),
                search_schema,
                json!(0),
                json!(0),
            ),
            "case {case}: the stored index is read back and has settled, got {described}"
        );
    }

    // A table created with no vector index reports no list at all, so the member above is
    // the stored index rather than something every description carries.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "VectorNone",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        }),
    )
    .await;
    let described = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "VectorNone"}),
    )
    .await;
    assert_eq!(
        described["Table"]["VectorIndexes"].clone(),
        Value::Null,
        "case no vector index: the table reports no vector index list, got {described}"
    );
}

#[tokio::test]
async fn an_index_name_already_taken_on_the_table_is_refused() {
    let router = emulator();
    let definitions = json!([
        {"AttributeName": "pk", "AttributeType": "S"},
        {"AttributeName": "tenant", "AttributeType": "S"},
    ]);
    let searched = |name: &str| {
        vector_index(
            name,
            4,
            "EUCLIDEAN",
            json!({"ProjectionType": "ALL"}),
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
        )
    };
    let secondary_index = json!({
        "IndexName": "idx.taken",
        "KeySchema": [{"AttributeName": "tenant", "KeyType": "HASH"}],
        "Projection": {"ProjectionType": "ALL"},
    });

    // Rows differ in which index kind already holds the name: another vector index, and a
    // global secondary index declared in the same create.
    let mut taken_by_global = create_request(
        "VectorNameTakenByIndex",
        definitions.clone(),
        json!([searched("idx.taken")]),
    );
    taken_by_global["GlobalSecondaryIndexes"] = json!([secondary_index]);
    let cases: [(&str, Value); 2] = [
        (
            "another vector index",
            create_request(
                "VectorNameTakenTwice",
                definitions.clone(),
                json!([searched("idx.taken"), searched("idx.taken")]),
            ),
        ),
        ("a global secondary index", taken_by_global),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, "CreateTable", request).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                "Invalid VectorIndex: duplicate index name idx.taken".to_string()
            ),
            "case {case}: a name already taken is refused, got {status} with {body}"
        );
    }

    // A name no other index holds is accepted, so the refusals above are about the name.
    let created = ok(
        &router,
        "CreateTable",
        create_request(
            "VectorNameFree",
            definitions,
            json!([searched("idx.free")]),
        ),
    )
    .await;
    assert_ne!(
        vector_index_named(&created["TableDescription"], "idx.free"),
        Value::Null,
        "case a free name: the index is stored, got {created}"
    );
}
