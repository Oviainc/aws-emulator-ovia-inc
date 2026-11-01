// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the constraint this service holds a caller-supplied
//! resource NAME to.
//!
//! RULE: a name a caller supplies for a resource of this service — the `RegistryName`
//! and `SchemaName` labels — is checked against the service's own published constraint
//! for that name before the request is acted on, and a name outside it is refused with
//! HTTP 400 `BadRequestException` naming the parameter and the rule it broke, with
//! nothing stored. The constraint is the SERVICE's: both members are modelled as a
//! plain `__string` with no `@length` and no `@pattern`, so the boundary validator
//! enforces nothing.
//!
//! The rows below differ along what that rule generalises over: WHICH character outside
//! the accepted set a name carries and where it falls, which accepted character class
//! it is built from, its LENGTH either side of the published bound, which of the two
//! names it is, and which OPERATION carries it — a reporting one as much as a mutating
//! one. The recorded case (a registry name holding a space, and a 65-character one, on
//! `CreateRegistry`) is one row of each table.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs`.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    NameConstraint, AWS_MANAGED_REGISTRY, REGISTRY_NAME, SCHEMA_NAME,
};

use crate::registry_schema_lifecycle::{
    create_registry, registry_names, registry_uri, schema_names, schema_uri, versions_uri,
    wait_for_keyword_index, walk, Answer, Emulator,
};

/// The listing of one registry's schemas.
fn listing_uri(registry: &str) -> String {
    format!("/v1/registries/name/{registry}/schemas")
}

/// A `CreateSchema`/`UpdateSchema` body: the smallest document of a modelled type.
fn schema_body() -> serde_json::Value {
    json!({"Content": "{}", "Type": "OpenApi3"})
}

/// Which of the two rules a name breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Broken {
    /// It is built from characters the service does not accept.
    Pattern,
    /// It is longer than the service accepts.
    Length,
}

impl Broken {
    /// The sentence the service refuses a name breaking this rule with: the parameter
    /// it checked and the rule that parameter broke. Spelled out here rather than taken
    /// from the handler, so a message that changed would fail the case.
    fn refusal(self, constraint: &NameConstraint) -> String {
        match self {
            Broken::Pattern => format!(
                "Parameter {} doesn't match pattern {}",
                constraint.parameter,
                constraint.pattern()
            ),
            Broken::Length => format!(
                "Parameter {} must have length less than or equal to {}",
                constraint.parameter, constraint.max_length
            ),
        }
    }
}

/// One name a case drives, as the URI carries it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    name: String,
    /// The rule it breaks, or `None` when the service accepts it.
    broken: Option<Broken>,
}

impl Row {
    fn accepted(case: &'static str, name: impl Into<String>) -> Self {
        Self {
            case,
            name: name.into(),
            broken: None,
        }
    }

    fn refused(case: &'static str, name: impl Into<String>, broken: Broken) -> Self {
        Self {
            case,
            name: name.into(),
            broken: Some(broken),
        }
    }
}

/// Assert `answer` is the service's refusal of a name that broke `broken`.
fn assert_refused(case: &str, answer: &Answer, constraint: &NameConstraint, broken: Broken) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "BadRequestException",
        "case {case:?}: expected a BadRequestException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        "MalformedRequest",
        "case {case:?}: expected the malformed-request code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        broken.refusal(constraint),
        "case {case:?}: the refusal must name the parameter and the rule it broke, got {:?}",
        answer.body
    );
}

/// The registry names the cases drive.
///
/// Source for the accepted set and the bound: the constraint the service publishes for
/// a registry's name (`AWS::EventSchemas::Registry`'s `RegistryName`) — letters,
/// digits, `.`, `-` and `_`, up to 64 characters — which `REGISTRY_NAME` carries and
/// `the_constraints_are_the_ones_the_service_publishes` pins.
fn registry_rows() -> Vec<Row> {
    vec![
        // Accepted: one row per accepted character class, each class at the start, in
        // the middle and at the end of a name, and both ends of the length bound.
        Row::accepted("a single letter", "r"),
        Row::accepted("a single digit", "7"),
        Row::accepted("dots", "svc.registry.one"),
        Row::accepted("hyphens", "svc-registry-two"),
        Row::accepted("underscores", "svc_registry_three"),
        Row::accepted("every accepted class at once", "Svc.4-registry_X"),
        Row::accepted("a leading separator", "_leading"),
        Row::accepted("a trailing separator", "trailing."),
        Row::accepted("the longest accepted name", "r".repeat(REGISTRY_NAME.max_length)),
        // Refused on the character set: a space (the recorded case), the separator the
        // SCHEMA name constraint adds, and the other punctuation a URI path segment can
        // carry unencoded.
        Row::refused("a space", "my%20registry", Broken::Pattern),
        Row::refused("an at sign", "my@registry", Broken::Pattern),
        Row::refused("an exclamation mark", "my!registry", Broken::Pattern),
        Row::refused("an asterisk", "my*registry", Broken::Pattern),
        Row::refused("a colon", "my:registry", Broken::Pattern),
        Row::refused("a dollar sign", "my$registry", Broken::Pattern),
        Row::refused("parentheses", "my(registry)", Broken::Pattern),
        Row::refused("a tilde", "my~registry", Broken::Pattern),
        Row::refused("a comma", "my,registry", Broken::Pattern),
        Row::refused("a semicolon", "my;registry", Broken::Pattern),
        Row::refused("an equals sign", "my=registry", Broken::Pattern),
        Row::refused("a plus sign", "my+registry", Broken::Pattern),
        Row::refused("a non-ASCII letter", "caf%C3%A9", Broken::Pattern),
        Row::refused("a leading space", "%20leading", Broken::Pattern),
        // Refused on the length: one character past the bound (the recorded case), and
        // far past it.
        Row::refused(
            "one character past the longest accepted name",
            "r".repeat(REGISTRY_NAME.max_length + 1),
            Broken::Length,
        ),
        Row::refused(
            "far past the longest accepted name",
            "r".repeat(REGISTRY_NAME.max_length * 4),
            Broken::Length,
        ),
        // Both rules at once: reported against its length, which is the order the
        // refusals are decided in.
        Row::refused(
            "too long AND built from characters the service does not accept",
            format!("{}%20", "r".repeat(REGISTRY_NAME.max_length)),
            Broken::Length,
        ),
    ]
}

/// The schema names the cases drive.
///
/// Source for the accepted set and the bound: the constraint the service publishes for
/// a schema's name (`AWS::EventSchemas::Schema`'s `SchemaName`) — the registry name's
/// set plus `@`, up to 385 characters.
fn schema_rows() -> Vec<Row> {
    vec![
        Row::accepted("a single letter", "s"),
        Row::accepted("an at sign", "source@EventType"),
        Row::accepted(
            "the form the published catalogue's own names take",
            "svc.events@ScheduledEvent",
        ),
        Row::accepted("dots, hyphens, underscores and digits", "svc.9-schema_name"),
        Row::accepted("a leading and a trailing at sign", "@svc.events@"),
        // The two names carry DIFFERENT bounds, so a name a registry's bound would
        // refuse is accepted here.
        Row::accepted(
            "longer than a registry name may be",
            "s".repeat(REGISTRY_NAME.max_length + 1),
        ),
        Row::accepted("the longest accepted name", "s".repeat(SCHEMA_NAME.max_length)),
        Row::refused("a space", "my%20schema", Broken::Pattern),
        Row::refused("a number sign", "my%23schema", Broken::Pattern),
        Row::refused("a dollar sign", "my$schema", Broken::Pattern),
        Row::refused("an encoded slash", "my%2Fschema", Broken::Pattern),
        Row::refused("an exclamation mark", "my!schema", Broken::Pattern),
        Row::refused("an asterisk", "my*schema", Broken::Pattern),
        Row::refused("a plus sign", "my+schema", Broken::Pattern),
        Row::refused("an equals sign", "my=schema", Broken::Pattern),
        Row::refused("parentheses", "my(schema)", Broken::Pattern),
        Row::refused("a colon", "my:schema", Broken::Pattern),
        Row::refused("a tilde", "my~schema", Broken::Pattern),
        Row::refused("a comma", "my,schema", Broken::Pattern),
        Row::refused("a non-ASCII letter", "caf%C3%A9", Broken::Pattern),
        Row::refused("a leading space", "%20leading", Broken::Pattern),
        Row::refused(
            "one character past the longest accepted name",
            "s".repeat(SCHEMA_NAME.max_length + 1),
            Broken::Length,
        ),
        Row::refused(
            "far past the longest accepted name",
            "s".repeat(SCHEMA_NAME.max_length * 4),
            Broken::Length,
        ),
        Row::refused(
            "too long AND built from characters the service does not accept",
            format!("{}%20", "s".repeat(SCHEMA_NAME.max_length)),
            Broken::Length,
        ),
    ]
}

/// The constraints the cases above read their accepted set and bound from are the ones
/// the service publishes for these two names, and the names the service's OWN catalogue
/// carries satisfy them.
#[tokio::test]
async fn the_constraints_are_the_ones_the_service_publishes() {
    assert_eq!(
        (
            REGISTRY_NAME.parameter,
            REGISTRY_NAME.pattern().as_str(),
            REGISTRY_NAME.max_length
        ),
        ("RegistryName", r"^[\.\-_A-Za-z0-9]+$", 64),
        "the registry-name constraint must be the published one"
    );
    assert_eq!(
        (
            SCHEMA_NAME.parameter,
            SCHEMA_NAME.pattern().as_str(),
            SCHEMA_NAME.max_length
        ),
        ("SchemaName", r"^[\.\-_A-Za-z0-9@]+$", 385),
        "the schema-name constraint must be the published one"
    );

    // A constraint the service's own content could not satisfy would be the wrong
    // constraint: the service-owned registry and one of its published schemas are read
    // through the same checked path every other request takes.
    let emulator = Emulator::new();
    let described = emulator
        .call("GET", &registry_uri(AWS_MANAGED_REGISTRY), None)
        .await;
    assert_eq!(
        described.status, 200,
        "the service-owned registry's own name must satisfy the constraint, got {} {:?}",
        described.status, described.body
    );
    let published = emulator.call("GET", &listing_uri(AWS_MANAGED_REGISTRY), None).await;
    let first = schema_names(&published)
        .first()
        .cloned()
        .expect("the service-owned registry publishes schemas");
    let read = emulator
        .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &first), None)
        .await;
    assert_eq!(
        read.status, 200,
        "case {first:?}: a published schema's own name must satisfy the constraint, got {} {:?}",
        read.status, read.body
    );
}

/// RULE: a registry name the service accepts is created and reads back under exactly
/// that name — the constraint admits every name built from the accepted set, at any
/// length up to the bound.
#[tokio::test]
async fn a_registry_name_the_service_accepts_round_trips() {
    let emulator = Emulator::new();
    for row in registry_rows().iter().filter(|row| row.broken.is_none()) {
        let case = row.case;
        let created = emulator
            .call("POST", &registry_uri(&row.name), Some(json!({})))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateRegistry, got {} {:?}",
            created.status, created.body
        );

        let described = emulator.call("GET", &registry_uri(&row.name), None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeRegistry, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("RegistryName"),
            row.name,
            "case {case:?}: the read must answer the name the create stored"
        );
        let arn = described.str_at("RegistryArn");
        assert!(
            arn.ends_with(&format!("registry/{}", row.name)),
            "case {case:?}: the ARN must name the stored registry, got {arn:?}"
        );
    }
}

/// RULE: a registry name outside the service's constraint is refused with the
/// parameter-validation refusal, and NOTHING is created — the store is what a later
/// listing reports, so a refused create must leave it as it was.
#[tokio::test]
async fn a_registry_name_outside_the_constraint_is_refused_and_creates_nothing() {
    let emulator = Emulator::new();
    create_registry(&emulator, "already-here").await;
    let before = walk(&emulator, "/v1/registries", None, registry_names).await;

    for row in registry_rows() {
        let Some(broken) = row.broken else { continue };
        let refused = emulator
            .call("POST", &registry_uri(&row.name), Some(json!({})))
            .await;
        assert_refused(row.case, &refused, &REGISTRY_NAME, broken);

        let after = walk(&emulator, "/v1/registries", None, registry_names).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused create must leave the registries as they were",
            row.case
        );
    }
}

/// RULE: the same constraint governs EVERY operation that takes a `RegistryName` — a
/// reporting one as much as a mutating one. A name the service does not accept names no
/// registry anywhere, so such a request is refused on its form rather than answered as
/// if the registry merely did not exist yet.
#[tokio::test]
async fn every_operation_taking_a_registry_name_refuses_one_outside_the_constraint() {
    let emulator = Emulator::new();

    // Rows: a name holding a character the service does not accept, and one past the
    // length bound — the two ways the constraint can be broken.
    for row in registry_rows() {
        let Some(broken) = row.broken else { continue };
        let name = &row.name;
        let schema = schema_uri(name, "any-schema");
        let bindings = format!("{}/language/Java8", schema_uri(name, "any-schema"));
        let policy = format!("/v1/policy?registryName={name}");
        for (operation, method, uri, body) in [
            ("CreateRegistry", "POST", registry_uri(name), Some(json!({}))),
            ("DescribeRegistry", "GET", registry_uri(name), None),
            ("UpdateRegistry", "PUT", registry_uri(name), Some(json!({}))),
            ("DeleteRegistry", "DELETE", registry_uri(name), None),
            ("ListSchemas", "GET", listing_uri(name), None),
            (
                "SearchSchemas",
                "GET",
                format!("{}/search?keywords=svc", listing_uri(name)),
                None,
            ),
            ("CreateSchema", "POST", schema.clone(), Some(schema_body())),
            ("DescribeSchema", "GET", schema.clone(), None),
            ("UpdateSchema", "PUT", schema.clone(), Some(schema_body())),
            ("DeleteSchema", "DELETE", schema.clone(), None),
            (
                "DeleteSchemaVersion",
                "DELETE",
                format!("{schema}/version/1"),
                None,
            ),
            (
                "ListSchemaVersions",
                "GET",
                versions_uri(name, "any-schema"),
                None,
            ),
            (
                "ExportSchema",
                "GET",
                format!("{schema}/export?type=JSONSchemaDraft4"),
                None,
            ),
            ("PutCodeBinding", "POST", bindings.clone(), None),
            ("DescribeCodeBinding", "GET", bindings.clone(), None),
            ("GetResourcePolicy", "GET", policy.clone(), None),
            (
                "PutResourcePolicy",
                "PUT",
                policy.clone(),
                Some(json!({"Policy": "{}"})),
            ),
            ("DeleteResourcePolicy", "DELETE", policy.clone(), None),
        ] {
            let case = format!("{}/{operation}", row.case);
            let answer = emulator.call(method, &uri, body).await;
            assert_refused(&case, &answer, &REGISTRY_NAME, broken);
        }
    }
}

/// RULE: a schema name the service accepts is created in a registry and reads back
/// under exactly that name — including the `@` the registry name's constraint does not
/// admit, which is the separator the published catalogue's own names carry.
#[tokio::test]
async fn a_schema_name_the_service_accepts_round_trips() {
    let emulator = Emulator::new();
    let registry = "accepted-schema-names";
    create_registry(&emulator, registry).await;

    for row in schema_rows().iter().filter(|row| row.broken.is_none()) {
        let case = row.case;
        let created = emulator
            .call(
                "POST",
                &schema_uri(registry, &row.name),
                Some(schema_body()),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );

        let described = emulator
            .call("GET", &schema_uri(registry, &row.name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("SchemaName"),
            row.name,
            "case {case:?}: the read must answer the name the create stored"
        );
    }
}

/// RULE: a schema name outside the service's constraint is refused the same way, and
/// nothing is created in the registry it named.
#[tokio::test]
async fn a_schema_name_outside_the_constraint_is_refused_and_creates_nothing() {
    let emulator = Emulator::new();
    let registry = "refused-schema-names";
    create_registry(&emulator, registry).await;
    let before = walk(&emulator, &listing_uri(registry), None, schema_names).await;

    for row in schema_rows() {
        let Some(broken) = row.broken else { continue };
        let refused = emulator
            .call(
                "POST",
                &schema_uri(registry, &row.name),
                Some(schema_body()),
            )
            .await;
        assert_refused(row.case, &refused, &SCHEMA_NAME, broken);

        let after = walk(&emulator, &listing_uri(registry), None, schema_names).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused create must leave the registry's schemas as they were",
            row.case
        );
    }
}

/// The control schema's document, which declares the event source a search selects it
/// by (an EventBridge document's `x-amazon-events-source` extension, which is what this
/// service's keyword index is built from — see `tests/all/search_keyword_index.rs`).
const CONTROL_CONTENT: &str = r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Control"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"svc.control","x-amazon-events-detail-type":"Control Event","properties":{}}}}}"#;

/// RULE: because a name outside the constraint is refused before anything is stored, no
/// reader ever reports it and no ARN is ever minted for it — the refused create leaves
/// the registry holding exactly the schemas it held, whichever reader is asked.
#[tokio::test]
async fn a_refused_schema_name_reaches_no_reader_and_mints_no_arn() {
    let emulator = Emulator::new();
    let registry = "no-reader-reports-it";
    create_registry(&emulator, registry).await;

    // A control schema, so the readers below are asserted to report the registry's real
    // contents rather than merely to report nothing at all. Its document DECLARES an
    // event, which is what the search below selects it by.
    let control = "svc.events@Control";
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, control),
            Some(json!({"Content": CONTROL_CONTENT, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the control schema must be created, got {} {:?}",
        created.status, created.body
    );
    let expected = vec![control.to_string()];
    // The keyword the control schema's declared event source indexes it under, so what
    // the search reports turns on the registry's contents rather than on how a search
    // naming no keyword behaves. The index catches up with a write a short span after it,
    // so the rows below start from a search that does report the control schema.
    let search = format!("{}/search?keywords=svc.control", listing_uri(registry));
    wait_for_keyword_index();

    for row in schema_rows() {
        let Some(broken) = row.broken else { continue };
        let case = row.case;
        let refused = emulator
            .call(
                "POST",
                &schema_uri(registry, &row.name),
                Some(schema_body()),
            )
            .await;
        assert_refused(case, &refused, &SCHEMA_NAME, broken);
        assert!(
            refused.body.get("SchemaArn").is_none(),
            "case {case:?}: a refused create must mint no ARN, got {:?}",
            refused.body
        );

        for (reader, uri) in [
            ("ListSchemas", listing_uri(registry)),
            ("SearchSchemas", search.clone()),
        ] {
            let reported = walk(&emulator, &uri, None, schema_names).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: {reader} must report only the schemas the registry holds"
            );
        }

        let read = emulator
            .call("GET", &schema_uri(registry, &row.name), None)
            .await;
        assert_refused(case, &read, &SCHEMA_NAME, broken);
        assert!(
            read.body.get("SchemaArn").is_none(),
            "case {case:?}: no read may answer an ARN for a name the service refuses, \
             got {:?}",
            read.body
        );
    }
}

/// RULE: the same constraint governs every operation that takes a `SchemaName`, under a
/// registry that DOES exist — so what the refusal reports is the name's form and not a
/// missing registry.
#[tokio::test]
async fn every_operation_taking_a_schema_name_refuses_one_outside_the_constraint() {
    let emulator = Emulator::new();
    let registry = "holder";
    create_registry(&emulator, registry).await;

    for row in schema_rows() {
        let Some(broken) = row.broken else { continue };
        let schema = schema_uri(registry, &row.name);
        let bindings = format!("{schema}/language/Java8");
        for (operation, method, uri, body) in [
            ("CreateSchema", "POST", schema.clone(), Some(schema_body())),
            ("DescribeSchema", "GET", schema.clone(), None),
            ("UpdateSchema", "PUT", schema.clone(), Some(schema_body())),
            ("DeleteSchema", "DELETE", schema.clone(), None),
            (
                "DeleteSchemaVersion",
                "DELETE",
                format!("{schema}/version/1"),
                None,
            ),
            (
                "ListSchemaVersions",
                "GET",
                versions_uri(registry, &row.name),
                None,
            ),
            (
                "ExportSchema",
                "GET",
                format!("{schema}/export?type=JSONSchemaDraft4"),
                None,
            ),
            ("PutCodeBinding", "POST", bindings.clone(), None),
            ("DescribeCodeBinding", "GET", bindings.clone(), None),
        ] {
            let case = format!("{}/{operation}", row.case);
            let answer = emulator.call(method, &uri, body).await;
            assert_refused(&case, &answer, &SCHEMA_NAME, broken);
        }
    }
}

/// RULE: a request breaking the constraint on BOTH names is refused against the first
/// name the URI states — the names are checked in the order the request carries them,
/// so a caller fixing them works through the request from the front.
#[tokio::test]
async fn a_request_breaking_both_constraints_is_refused_against_the_first_name() {
    let emulator = Emulator::new();
    create_registry(&emulator, "present").await;

    let both = emulator
        .call(
            "POST",
            &schema_uri("my%20registry", "my%20schema"),
            Some(schema_body()),
        )
        .await;
    assert_refused("both names", &both, &REGISTRY_NAME, Broken::Pattern);

    let schema_only = emulator
        .call(
            "POST",
            &schema_uri("present", "my%20schema"),
            Some(schema_body()),
        )
        .await;
    assert_refused("the schema name alone", &schema_only, &SCHEMA_NAME, Broken::Pattern);
}
