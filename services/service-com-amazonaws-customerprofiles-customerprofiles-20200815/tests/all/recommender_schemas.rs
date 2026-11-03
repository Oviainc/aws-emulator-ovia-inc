// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateRecommenderSchema` and the reader that serves what it
//! stored (`GetRecommenderSchema`).
//!
//! The rule under test: a recommender schema is a resource of its domain, identified by
//! the (domain, `RecommenderSchemaName`) pair, and it selects COLUMNS OF the domain's data
//! tables. A create whose dataset type the domain holds no data table for is refused
//! `BadRequestException` (400); otherwise it persists the dataset-type -> column-definitions
//! map the request carried, mints the schema's ARN once, and answers from the record it
//! stored, so the reader reports the same schema. A `Fields` key that names a dataset type
//! the service does not support, a column list outside the length the model declares for
//! one, a column defined twice, a feature type on a numeric column, a tag key outside the
//! bounds `TagKey` declares, and a name the domain already holds are each refused
//! `BadRequestException` (400) with nothing persisted; a domain, or a schema in it, that is
//! not stored is `ResourceNotFoundException` (404).
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::{
    ARN_SERVICE, MAX_RECOMMENDER_SCHEMA_COLUMNS, SUPPORTED_RECOMMENDER_DATASET_TYPES,
};

use crate::domain_profile_lifecycle::{error_code, Emulator};
use crate::profile_object_types::error_message;

/// The region the test harness signs for, which is the region a minted ARN carries.
const REGION: &str = "us-east-1";

/// The longest a `RecommenderSchemaFieldList` may be: the model's `@length(max: 9)` on
/// `RecommenderSchemaFieldList`.
const MAX_FIELDS_PER_DATASET: usize = MAX_RECOMMENDER_SCHEMA_COLUMNS;

/// Define the domain's data table for `dataset_type` — the object type the domain ingests
/// that data through — asserting it was stored.
async fn define_data_table(emulator: &Emulator, domain: &str, dataset_type: &str) {
    let (status, body) = emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/object-types/{dataset_type}"),
            Some(json!({ "Description": format!("the {dataset_type} data table") })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {dataset_type:?}: expected PutProfileObjectType 200, got {status} {body:?}"
    );
}

/// A domain holding a data table for every dataset type the service supports, so a create
/// over any of them has columns to select.
async fn domain_with_data_tables(emulator: &Emulator, domain: &str, expiration_days: u32) {
    emulator.create_domain(domain, expiration_days).await;
    for dataset_type in SUPPORTED_RECOMMENDER_DATASET_TYPES {
        define_data_table(emulator, domain, dataset_type).await;
    }
}

/// `CreateRecommenderSchema` over the wire:
/// `POST /domains/{DomainName}/recommender-schemas/{RecommenderSchemaName}`.
async fn create(emulator: &Emulator, domain: &str, name: &str, body: Value) -> (u16, Value) {
    emulator
        .post(
            &format!("/domains/{domain}/recommender-schemas/{name}"),
            body,
        )
        .await
}

/// `GetRecommenderSchema` over the wire:
/// `GET /domains/{DomainName}/recommender-schemas/{RecommenderSchemaName}`.
async fn get(emulator: &Emulator, domain: &str, name: &str) -> (u16, Value) {
    emulator
        .send(
            "GET",
            &format!("/domains/{domain}/recommender-schemas/{name}"),
            None,
        )
        .await
}

/// A column definition: the required target field name, plus whichever of the two
/// optional settings the case carries.
fn field(target: &str, content_type: Option<&str>, feature_type: Option<&str>) -> Value {
    let mut column = json!({ "TargetFieldName": target });
    if let Some(content_type) = content_type {
        column["ContentType"] = json!(content_type);
    }
    if let Some(feature_type) = feature_type {
        column["FeatureType"] = json!(feature_type);
    }
    column
}

/// `n` column definitions, each naming a distinct target field.
fn fields(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| field(&format!("Attributes.Column{i}"), None, None))
            .collect(),
    )
}

/// The requests the success table creates, spanning what the rule generalises over: which
/// supported dataset types the schema covers, how many columns each one carries, which
/// optional column settings are set, and whether the request carries tags.
fn success_rows() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "one dataset type with one column",
            "WebOnly",
            json!({
                "Fields": {
                    "_webAnalytics": [field("Attributes.PageUrl", None, None)],
                },
            }),
        ),
        (
            "the other dataset type, with both optional column settings set",
            "catalog_only-1",
            json!({
                "Fields": {
                    "_catalogItem": [
                        field("Attributes.MealTime", Some("STRING"), Some("CATEGORICAL")),
                        field("Attributes.Price", Some("NUMBER"), None),
                        field("Location.City", None, Some("TEXTUAL")),
                    ],
                },
            }),
        ),
        (
            "both dataset types at once, and tags",
            "BothDatasets",
            json!({
                "Fields": {
                    "_webAnalytics": [field("Attributes.PageUrl", Some("STRING"), None)],
                    "_catalogItem": [field("Attributes.Sku", Some("STRING"), None)],
                },
                "Tags": { "env": "dev", "team": "recs" },
            }),
        ),
        (
            "the longest column list the model admits",
            "MaxColumns",
            json!({ "Fields": { "_catalogItem": fields(MAX_FIELDS_PER_DATASET) } }),
        ),
    ]
}

/// Every stored schema round trips: the create answers from the record it stored and the
/// reader reports that same record, over a table of requests spanning the dataset types,
/// the column counts and the optional members.
#[tokio::test]
async fn a_created_recommender_schema_round_trips_through_the_reader() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "recdom", 90).await;

    for (case, name, request) in success_rows() {
        let (status, created) = create(&emulator, "recdom", name, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRecommenderSchema 200, got {status} {created:?}"
        );
        assert_eq!(
            created.get("RecommenderSchemaName").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected RecommenderSchemaName {name:?}, got {:?}",
            created.get("RecommenderSchemaName")
        );
        assert_eq!(
            created.get("Status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected Status ACTIVE, got {:?}",
            created.get("Status")
        );
        assert_eq!(
            created.get("Fields"),
            request.get("Fields"),
            "case {case:?}: expected the Fields the request carried, got {:?}",
            created.get("Fields")
        );
        assert_eq!(
            created.get("Tags"),
            request.get("Tags"),
            "case {case:?}: expected the Tags the request carried, got {:?}",
            created.get("Tags")
        );
        let expected_arn = engine::emulator_arn(
            ARN_SERVICE,
            REGION,
            &format!("domains/recdom/recommender-schemas/{name}"),
        );
        assert_eq!(
            created.get("RecommenderSchemaArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected RecommenderSchemaArn {expected_arn:?}, got {:?}",
            created.get("RecommenderSchemaArn")
        );
        let created_at = created.get("CreatedAt").cloned().unwrap_or(Value::Null);
        assert!(
            created_at.is_number(),
            "case {case:?}: expected a CreatedAt timestamp, got {created_at:?}"
        );

        let (status, read) = get(&emulator, "recdom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecommenderSchema 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("RecommenderSchemaName").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected the stored name {name:?}, got {:?}",
            read.get("RecommenderSchemaName")
        );
        assert_eq!(
            read.get("Fields"),
            request.get("Fields"),
            "case {case:?}: expected the stored Fields, got {:?}",
            read.get("Fields")
        );
        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected Status ACTIVE on the read, got {:?}",
            read.get("Status")
        );
        assert_eq!(
            read.get("CreatedAt"),
            Some(&created_at),
            "case {case:?}: expected the CreatedAt the create reported, got {:?}",
            read.get("CreatedAt")
        );

        // A read changes nothing a later caller sees: the second read answers exactly
        // what the first one did.
        let (status, again) = get(&emulator, "recdom", name).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: expected the read to be repeatable, got {status} {again:?}"
        );
    }
}

/// A `Fields` key naming a dataset type the service does not define is refused, and the
/// refusal stores nothing. The rows differ in HOW the key misses the supported set:
/// unknown word, wrong case, missing or extra characters, and the empty key the map's
/// unconstrained `String` key admits.
#[tokio::test]
async fn an_unsupported_dataset_type_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "recdom", 90).await;

    let rows = [
        ("an unknown dataset type", "_clickstream"),
        ("the supported name without its leading underscore", "webAnalytics"),
        ("the supported name in another case", "_WebAnalytics"),
        ("the supported name with a trailing character", "_catalogItems"),
        ("the empty key the map admits", ""),
    ];

    for (index, (case, dataset_type)) in rows.into_iter().enumerate() {
        let name = format!("Unsupported{index}");
        let (status, body) = create(
            &emulator,
            "recdom",
            &name,
            json!({
                "Fields": { dataset_type: [field("Attributes.Column", None, None)] },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for dataset type {dataset_type:?}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            error_code(&body)
        );
        let expected = format!(
            "Fields contains an unsupported dataset type {dataset_type}. \
             The supported dataset types are {}",
            SUPPORTED_RECOMMENDER_DATASET_TYPES.join(", ")
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&body)
        );

        let (status, read) = get(&emulator, "recdom", &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused schema to be absent, got {status} {read:?}"
        );
    }

    // A request that mixes a supported dataset type with an unsupported one is refused
    // for the unsupported one: the whole schema is rejected, not the half of it that was
    // well formed.
    let (status, body) = create(
        &emulator,
        "recdom",
        "Mixed",
        json!({
            "Fields": {
                "_webAnalytics": [field("Attributes.PageUrl", None, None)],
                "_clickstream": [field("Attributes.Column", None, None)],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one supported and one unsupported dataset type\": expected 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case \"one supported and one unsupported dataset type\": expected BadRequestException, got {:?}",
        error_code(&body)
    );
    let (status, read) = get(&emulator, "recdom", "Mixed").await;
    assert_eq!(
        status, 404,
        "case \"one supported and one unsupported dataset type\": expected nothing stored, got {status} {read:?}"
    );
}

/// Every dataset type the service documents as supported is accepted, each on its own
/// schema — the catalog the refusal above is the complement of. The source of the list is
/// `CreateRecommenderSchemaRequest$Fields`, which names `_webAnalytics` and `_catalogItem`.
#[tokio::test]
async fn every_supported_dataset_type_is_accepted_on_its_own() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "recdom", 90).await;

    for (index, dataset_type) in SUPPORTED_RECOMMENDER_DATASET_TYPES.into_iter().enumerate() {
        let name = format!("Supported{index}");
        let request = json!({
            "Fields": { dataset_type: [field("Attributes.Column", None, None)] },
        });
        let (status, created) = create(&emulator, "recdom", &name, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {dataset_type:?}: expected 200, got {status} {created:?}"
        );
        let (status, read) = get(&emulator, "recdom", &name).await;
        assert_eq!(
            status, 200,
            "case {dataset_type:?}: expected the schema to be readable, got {status} {read:?}"
        );
        assert_eq!(
            read.get("Fields"),
            request.get("Fields"),
            "case {dataset_type:?}: expected the stored Fields, got {:?}",
            read.get("Fields")
        );
    }
}

/// The name identifies the schema within its domain: the first create of a name wins and
/// every later create of it is refused without changing what the winner stored, while the
/// same name in another domain is a different schema.
#[tokio::test]
async fn the_name_identifies_the_schema_within_its_domain() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "first", 90).await;
    domain_with_data_tables(&emulator, "second", 120).await;

    let winner = json!({ "Fields": { "_webAnalytics": [field("Attributes.PageUrl", None, None)] } });
    let (status, created) = create(&emulator, "first", "Shared", winner.clone()).await;
    assert_eq!(
        status, 200,
        "case \"the first create of a name\": expected 200, got {status} {created:?}"
    );

    for (case, loser) in [
        (
            "a later create carrying other columns",
            json!({ "Fields": { "_catalogItem": [field("Attributes.Sku", None, None)] } }),
        ),
        (
            "a later create carrying the same columns",
            winner.clone(),
        ),
    ] {
        let (status, body) = create(&emulator, "first", "Shared", loser).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an existing name, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            "Recommender schema with name Shared already exists",
            "case {case:?}: expected the already-exists sentence, got {:?}",
            error_message(&body)
        );
        let (status, read) = get(&emulator, "first", "Shared").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the winner to still be readable, got {status} {read:?}"
        );
        assert_eq!(
            read.get("Fields"),
            winner.get("Fields"),
            "case {case:?}: expected the winner's Fields to be unchanged, got {:?}",
            read.get("Fields")
        );
    }

    // The identity is the (domain, name) PAIR, so the same name in another domain is a
    // different schema, with its own columns and its own ARN.
    let other = json!({ "Fields": { "_catalogItem": [field("Attributes.Sku", None, None)] } });
    let (status, created) = create(&emulator, "second", "Shared", other.clone()).await;
    assert_eq!(
        status, 200,
        "case \"the same name in another domain\": expected 200, got {status} {created:?}"
    );
    let expected_arn = engine::emulator_arn(
        ARN_SERVICE,
        REGION,
        "domains/second/recommender-schemas/Shared",
    );
    assert_eq!(
        created.get("RecommenderSchemaArn").and_then(Value::as_str),
        Some(expected_arn.as_str()),
        "case \"the same name in another domain\": expected {expected_arn:?}, got {:?}",
        created.get("RecommenderSchemaArn")
    );
    let (_, read) = get(&emulator, "second", "Shared").await;
    assert_eq!(
        read.get("Fields"),
        other.get("Fields"),
        "case \"the same name in another domain\": expected its own Fields, got {:?}",
        read.get("Fields")
    );
    let (_, first) = get(&emulator, "first", "Shared").await;
    assert_eq!(
        first.get("Fields"),
        winner.get("Fields"),
        "case \"the same name in another domain\": expected the first domain's schema untouched, got {:?}",
        first.get("Fields")
    );
}

/// A domain, or a schema in it, that is not stored is reported absent rather than
/// invented, on both the create and the read path.
#[tokio::test]
async fn an_absent_domain_or_schema_is_reported_absent() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "recdom", 90).await;

    let request = json!({ "Fields": { "_webAnalytics": [field("Attributes.PageUrl", None, None)] } });
    for (case, domain) in [
        ("a domain that was never created", "nosuchdomain"),
        ("a domain whose name only resembles a stored one", "recdom2"),
    ] {
        let (status, body) = create(&emulator, domain, "Schema", request.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for domain {domain:?}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            error_code(&body)
        );
        // The wording the service answers an absent domain with, shared by every
        // domain-scoped operation (`crate::state::domain_not_found`).
        let expected = format!("Domain {domain} is not found");
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&body)
        );

        let (status, body) = get(&emulator, domain, "Schema").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 reading in domain {domain:?}, got {status} {body:?}"
        );
    }

    create(&emulator, "recdom", "Stored", request).await;
    for (case, name) in [
        ("a schema that was never created", "NeverCreated"),
        ("a name that only resembles a stored one", "Stored1"),
    ] {
        let (status, body) = get(&emulator, "recdom", name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for schema {name:?}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            error_code(&body)
        );
        let expected = format!("The recommender schema {name} does not exist in domain recdom");
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// A schema selects columns OF a data table, so a dataset type the domain holds no data
/// table for is refused and nothing is stored — and the same request is admitted once the
/// domain defines that table. The rows span which of the supported dataset types is
/// missing, including a request naming both when only one is defined.
#[tokio::test]
async fn a_dataset_type_with_no_data_table_is_refused_until_the_domain_defines_one() {
    let emulator = Emulator::new();
    emulator.create_domain("tableless", 90).await;
    emulator.create_domain("tabled", 90).await;

    let rows: [(&str, &str, Value); 3] = [
        (
            "the web-analytics table",
            "_webAnalytics",
            json!({ "Fields": { "_webAnalytics": [field("Attributes.PageUrl", None, None)] } }),
        ),
        (
            "the catalog-item table",
            "_catalogItem",
            json!({ "Fields": { "_catalogItem": [field("Attributes.Sku", None, None)] } }),
        ),
        (
            "both tables, with neither defined",
            "_catalogItem",
            json!({
                "Fields": {
                    "_catalogItem": [field("Attributes.Sku", None, None)],
                    "_webAnalytics": [field("Attributes.PageUrl", None, None)],
                },
            }),
        ),
    ];

    for (index, (case, missing, request)) in rows.into_iter().enumerate() {
        let name = format!("NeedsTable{index}");
        let (status, body) = create(&emulator, "tableless", &name, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 while the domain holds no data table, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            error_code(&body)
        );
        // The sentence is written out here rather than taken from the handler's own
        // formatter, so a change to that wording has to be made in both places
        // deliberately: the kind of resource, the name the request carried, and
        // "not found" — the form this service's `ObjectType {name} not found` takes.
        let expected = format!("DataTable {missing} not found");
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&body)
        );
        let (status, read) = get(&emulator, "tableless", &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused schema to be absent, got {status} {read:?}"
        );

        // The very same request, in a domain that holds a data table for every dataset
        // type it names.
        for dataset_type in SUPPORTED_RECOMMENDER_DATASET_TYPES {
            define_data_table(&emulator, "tabled", dataset_type).await;
        }
        let (status, created) = create(&emulator, "tabled", &name, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once the data tables are defined, got {status} {created:?}"
        );
    }
}

/// The bounds the model declares on one dataset type's column list, and the two rules a
/// column list has to satisfy that no modelled constraint states.
///
/// The list's own `@length` (1..9, from `RecommenderSchemaFieldList`) is checked by the
/// handler because the boundary walk cannot reach it: the list is a map value, and a
/// collection that is itself a collection element is walked against its enclosing member's
/// schema. Each published bound is exercised, and one count past it.
#[tokio::test]
async fn a_column_list_outside_its_bounds_or_rules_is_refused() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "recdom", 90).await;

    let refused: [(&str, Value); 4] = [
        (
            "no column at all, one under the published minimum",
            json!({ "Fields": { "_webAnalytics": [] } }),
        ),
        (
            "one column past the published maximum",
            json!({ "Fields": { "_catalogItem": fields(MAX_FIELDS_PER_DATASET + 1) } }),
        ),
        (
            "the same column defined twice",
            json!({
                "Fields": {
                    "_webAnalytics": [
                        field("Attributes.PageUrl", None, None),
                        field("Attributes.PageUrl", Some("STRING"), None),
                    ],
                },
            }),
        ),
        (
            "a feature type on a column holding numbers",
            json!({
                "Fields": {
                    "_catalogItem": [field("Attributes.Price", Some("NUMBER"), Some("CATEGORICAL"))],
                },
            }),
        ),
    ];

    for (index, (case, request)) in refused.into_iter().enumerate() {
        let name = format!("Refused{index}");
        let (status, body) = create(&emulator, "recdom", &name, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            error_code(&body)
        );
        let (status, read) = get(&emulator, "recdom", &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused schema to be absent, got {status} {read:?}"
        );
    }

    // Each published bound itself is admitted, and so is a numeric column that declares no
    // feature type.
    let admitted: [(&str, Value); 3] = [
        (
            "exactly the published minimum",
            json!({ "Fields": { "_webAnalytics": fields(1) } }),
        ),
        (
            "exactly the published maximum",
            json!({ "Fields": { "_catalogItem": fields(MAX_FIELDS_PER_DATASET) } }),
        ),
        (
            "a numeric column with no feature type",
            json!({
                "Fields": { "_catalogItem": [field("Attributes.Price", Some("NUMBER"), None)] },
            }),
        ),
    ];

    for (index, (case, request)) in admitted.into_iter().enumerate() {
        let name = format!("Admitted{index}");
        let (status, created) = create(&emulator, "recdom", &name, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
    }
}

/// A tag key is the one half of a tag map the boundary's constraint walk does not reach,
/// so the bounds `TagKey` declares are applied by the handler: the rows span the reserved
/// prefix, a character the pattern excludes, the empty key and a key past the declared
/// maximum, with a key at that maximum admitted.
#[tokio::test]
async fn a_tag_key_outside_the_modelled_bounds_is_refused() {
    let emulator = Emulator::new();
    domain_with_data_tables(&emulator, "recdom", 90).await;

    let request = |key: &str| {
        json!({
            "Fields": { "_webAnalytics": [field("Attributes.PageUrl", None, None)] },
            "Tags": { key: "value" },
        })
    };

    let refused = [
        ("the prefix AWS reserves", "aws:cost-centre".to_string()),
        ("a character the pattern excludes", "team space".to_string()),
        ("the empty key", String::new()),
        ("one character past the declared maximum", "k".repeat(129)),
    ];
    for (index, (case, key)) in refused.into_iter().enumerate() {
        let name = format!("Tagged{index}");
        let (status, body) = create(&emulator, "recdom", &name, request(&key)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for tag key {key:?}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            error_code(&body)
        );
        let (status, read) = get(&emulator, "recdom", &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused schema to be absent, got {status} {read:?}"
        );
    }

    for (index, (case, key)) in [
        ("a key at the declared maximum", "k".repeat(128)),
        (
            "a key of the characters the pattern admits",
            "team-1_a.b:c/d".to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("TaggedOk{index}");
        let (status, created) = create(&emulator, "recdom", &name, request(&key)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for tag key {key:?}, got {status} {created:?}"
        );
    }
}
