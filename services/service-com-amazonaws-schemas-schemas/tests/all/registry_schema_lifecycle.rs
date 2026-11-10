// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the registry → schema lifecycle.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! instead of from the store would still have to make a later read agree.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs with, unless the case varies it.
pub(crate) const REGION: &str = "us-west-2";

/// The account segment every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator with its own store, reached over HTTP.
///
/// Shared with the sibling suites in `tests/all/`: this is the crate's only test
/// harness, so a new suite imports it from here rather than standing up a second one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the signature check is the
        // framework's and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.call_signed_for(REGION, method, uri, body).await
    }

    /// This emulator's router, for a suite that needs to serve it on a SOCKET rather
    /// than drive it in process — the framing headers a response carries are written
    /// by the HTTP layer, which an in-process dispatch never reaches.
    pub(crate) fn router(&self) -> MultiServiceRouter {
        self.router.clone()
    }

    pub(crate) async fn call_signed_for(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Answer {
        self.dispatch(region, method, uri, body, true).await
    }

    /// Dispatch one request and answer with the response body left UNPARSED where it is
    /// not JSON.
    ///
    /// For the one operation whose output is an `httpPayload` blob
    /// (`GetCodeBindingSource`): its success answer is an archive, which is no JSON
    /// document, so [`Self::call`] would fail the case on it. A refusal still arrives as
    /// this service's JSON error envelope, so [`Answer::body`] carries one when there is
    /// one and [`Answer::raw`] carries the bytes either way.
    pub(crate) async fn call_payload(&self, method: &str, uri: &str) -> Answer {
        self.dispatch(REGION, method, uri, None, false).await
    }

    /// The one dispatch every call above goes through. `json_body` says whether a body
    /// that is not JSON fails the case or is left to [`Answer::raw`].
    async fn dispatch(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
        json_body: bool,
    ) -> Answer {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `schemas`
            // service and what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20240101/{region}/schemas/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        // Every response header, the framing ones an answer with no body is asserted on
        // and `x-amzn-errortype` (where the exception SHAPE name travels, which a suite
        // asserts against the error body's own `Code` member) among them.
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else if json_body {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} answered {status} with a body that is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer {
            status,
            headers,
            body,
            raw,
        }
    }
}

/// A response, reduced to what these tests assert on.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// Every response header, by lower-case name.
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Value,
    /// The response body as it came off the wire, before any parsing: an answer with
    /// no body has none, which is not the same thing as a body that parsed to null.
    pub(crate) raw: bytes::Bytes,
}

impl Answer {
    /// The value of one response header, by name (case-insensitive).
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
    /// The wire error code a client branches on, as restJson1 renders it.
    pub(crate) fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The exception shape name the `x-amzn-errortype` header carries, with any
    /// `#`-qualified namespace and any `:`-suffixed URI dropped, as a client reads it.
    pub(crate) fn error_type(&self) -> String {
        self.header("x-amzn-errortype")
            .unwrap_or("<no x-amzn-errortype>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    pub(crate) fn str_at(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a string `{member}` in {:?}", self.body))
            .to_string()
    }

    /// A member that must be present and non-null, whatever its JSON type.
    ///
    /// Timestamps are read through this rather than [`Self::str_at`] because what these
    /// cases own is the VALUE — fixed at write time and stable across reads — not its
    /// rendering. The WIRE FORM of a `date-time` member is pinned on its own, over every
    /// operation that answers one, in `tests/all/iso8601_timestamps.rs`.
    pub(crate) fn value_at(&self, member: &str) -> &Value {
        let value = self
            .body
            .get(member)
            .unwrap_or_else(|| panic!("expected a `{member}` in {:?}", self.body));
        assert!(
            !value.is_null(),
            "expected `{member}` to carry a value in {:?}",
            self.body
        );
        value
    }
}

pub(crate) fn registry_uri(registry: &str) -> String {
    format!("/v1/registries/name/{registry}")
}

pub(crate) fn schema_uri(registry: &str, schema: &str) -> String {
    format!("/v1/registries/name/{registry}/schemas/name/{schema}")
}

/// The `ExportSchema` URI of one schema, in one schema language.
pub(crate) fn export_uri(registry: &str, schema: &str, export_type: &str) -> String {
    format!(
        "{}/export?type={export_type}",
        schema_uri(registry, schema)
    )
}

/// The keywords OpenAPI 3.0 adds to the JSON Schema vocabulary, which draft-04 does not
/// define — so neither a schema keyword of one of these names nor a `properties` member
/// declared under one survives a conversion to draft-04.
///
/// Source: the fixed fields the OpenAPI 3.0 Schema Object adds for its own purposes.
/// Shared with the sibling suites: every case that asserts what a draft-04 export does
/// NOT carry reads the set from here, so two suites cannot disagree about it.
pub(crate) const OPENAPI_ONLY_KEYWORDS: &[&str] = &[
    "nullable",
    "readOnly",
    "writeOnly",
    "deprecated",
    "discriminator",
    "xml",
    "example",
    "externalDocs",
];

/// The `ListSchemaVersions` URI of one schema.
pub(crate) fn versions_uri(registry: &str, schema: &str) -> String {
    format!("{}/versions", schema_uri(registry, schema))
}

/// The version labels a `ListSchemaVersions` answer reports, as a set.
///
/// Shared with the sibling suites: every case that asks which versions a schema
/// holds reads them through this one reducer, so two suites cannot disagree about
/// what the listing said.
pub(crate) fn listed_versions(answer: &Answer) -> BTreeSet<String> {
    answer.body["SchemaVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a SchemaVersions array in {:?}", answer.body))
        .iter()
        .map(|item| {
            item["SchemaVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Create a registry, asserting it succeeded; the lifecycle cases start here.
pub(crate) async fn create_registry(emulator: &Emulator, registry: &str) {
    let created = emulator
        .call("POST", &registry_uri(registry), Some(json!({})))
        .await;
    assert_eq!(
        created.status, 201,
        "case {registry:?}: expected 201 from CreateRegistry, got {} {:?}",
        created.status, created.body
    );
}

/// Wait until the registry's keyword index has caught up with every definition written
/// so far, so a `SearchSchemas` case can assert what the index holds rather than when it
/// caught up.
///
/// The span is the service's own
/// (`service_com_amazonaws_schemas_schemas::state::SCHEMA_INDEXING_DELAY`), read rather
/// than written out here, plus a small margin for the clock readings either side of it.
/// This blocks the calling test's thread, which is what a suite wants: what a search
/// answers BEFORE the catch-up is asserted by the cases that drive the transient absence.
pub(crate) fn wait_for_keyword_index() {
    std::thread::sleep(
        service_com_amazonaws_schemas_schemas::state::SCHEMA_INDEXING_DELAY
            + std::time::Duration::from_millis(250),
    );
}

/// One schema the table below creates and reads back.
#[derive(Debug)]
struct SchemaCase {
    case: &'static str,
    name: String,
    /// A member of [`TYPE_VALUES`].
    schema_type: &'static str,
    content: String,
    description: Option<&'static str>,
    tags: BTreeMap<&'static str, &'static str>,
}

/// The table: rows differ along what the round-trip rule generalises over — the
/// schema `Type`, the form, size and alphabet of `Content`, the presence of
/// `Description` and `Tags`, and the shape of the schema name.
fn schema_cases() -> Vec<SchemaCase> {
    vec![
        SchemaCase {
            case: "minimal-openapi-no-description-no-tags",
            name: "demo-schema".to_string(),
            schema_type: "OpenApi3",
            content: r#"{"openapi":"3.0.0"}"#.to_string(),
            description: None,
            tags: BTreeMap::new(),
        },
        SchemaCase {
            case: "jsonschema-draft4-with-description-and-tags",
            name: "order.placed_v1".to_string(),
            schema_type: "JSONSchemaDraft4",
            content: r#"{"$schema":"http://json-schema.org/draft-04/schema#","type":"object"}"#
                .to_string(),
            description: Some("the order-placed event"),
            tags: BTreeMap::from([("team", "payments"), ("env", "test")]),
        },
        SchemaCase {
            case: "content-with-newlines-quotes-and-non-ascii",
            name: "unicode-schema".to_string(),
            schema_type: "OpenApi3",
            content: "{\n  \"openapi\": \"3.0.0\",\n  \"info\": {\"title\": \"caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}\", \"version\": \"1\\\"0\"}\n}"
                .to_string(),
            description: Some(""),
            tags: BTreeMap::from([("owner", "caf\u{e9}")]),
        },
        SchemaCase {
            case: "content-near-the-modelled-upper-length-bound",
            name: "big-schema".to_string(),
            schema_type: "JSONSchemaDraft4",
            // `Content` is modelled `len 1..100000`; this row sits just under the top.
            content: format!(
                r#"{{"type":"object","description":"{}"}}"#,
                "x".repeat(99_900)
            ),
            description: None,
            tags: BTreeMap::new(),
        },
        SchemaCase {
            case: "content-at-the-smallest-document-there-is",
            name: "two-byte-schema".to_string(),
            schema_type: "OpenApi3",
            // The smallest `Content` that is a schema document: `{}`. The modelled
            // lower bound is `len 1`, but a one-byte body cannot be a JSON object, so
            // that bound is exercised by the refusal table in
            // `malformed_content_and_policy_deletion.rs` instead.
            content: "{}".to_string(),
            description: None,
            tags: BTreeMap::new(),
        },
    ]
}

fn create_schema_body(case: &SchemaCase) -> Value {
    let mut body = json!({ "Content": case.content, "Type": case.schema_type });
    if let Some(description) = case.description {
        body["Description"] = json!(description);
    }
    if !case.tags.is_empty() {
        body["tags"] = json!(case.tags);
    }
    body
}

/// RULE: a create persists the resource, and every later read of it reports exactly
/// what was stored — the content verbatim, version `1`, the declared type, the
/// description and tags, and timestamps fixed at write time.
#[tokio::test]
async fn a_created_schema_reads_back_exactly_as_stored() {
    let emulator = Emulator::new();
    let registry = "demo-registry";
    create_registry(&emulator, registry).await;

    for case in schema_cases() {
        let created = emulator
            .call(
                "POST",
                &schema_uri(registry, &case.name),
                Some(create_schema_body(&case)),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateSchema, got {} {:?}",
            case.case, created.status, created.body
        );
        assert_eq!(
            created.str_at("SchemaVersion"),
            "1",
            "case {:?}: a schema's first version is 1, got {:?}",
            case.case,
            created.body["SchemaVersion"]
        );
        let expected_arn = format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/{registry}/{}", case.name);
        assert_eq!(
            created.str_at("SchemaArn"),
            expected_arn,
            "case {:?}: SchemaArn must name the registry and schema",
            case.case
        );

        let described = emulator
            .call("GET", &schema_uri(registry, &case.name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeSchema, got {} {:?}",
            case.case, described.status, described.body
        );
        assert_eq!(
            described.str_at("Content"),
            case.content,
            "case {:?}: DescribeSchema must return the stored Content verbatim",
            case.case
        );
        assert_eq!(
            described.str_at("Type"),
            case.schema_type,
            "case {:?}: the stored Type is reported back",
            case.case
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            "1",
            "case {:?}: the only stored version is 1",
            case.case
        );
        assert_eq!(
            described.str_at("SchemaArn"),
            expected_arn,
            "case {:?}: the ARN is the one minted at create time",
            case.case
        );
        assert_eq!(
            described.body.get("Description"),
            created.body.get("Description"),
            "case {:?}: the Description a read reports is the one the create stored",
            case.case
        );
        assert_eq!(
            described.body.get("Description").and_then(Value::as_str),
            case.description,
            "case {:?}: the stored Description is reported back",
            case.case
        );
        let expected_tags: Value = json!(case.tags);
        assert_eq!(
            described.body["tags"], expected_tags,
            "case {:?}: the stored tags are reported back, got {:?}",
            case.case, described.body["tags"]
        );

        // Timestamps are set at WRITE time: the create and every read agree, and two
        // reads of an unchanged schema agree with each other.
        let again = emulator
            .call("GET", &schema_uri(registry, &case.name), None)
            .await;
        for member in ["LastModified", "VersionCreatedDate"] {
            assert_eq!(
                created.value_at(member),
                described.value_at(member),
                "case {:?}: {member} must come from the stored write time",
                case.case
            );
            assert_eq!(
                described.value_at(member),
                again.value_at(member),
                "case {:?}: {member} must not change between reads",
                case.case
            );
        }

        // Asking for version "1" explicitly is the same answer as asking for the latest.
        let pinned = emulator
            .call(
                "GET",
                &format!("{}?schemaVersion=1", schema_uri(registry, &case.name)),
                None,
            )
            .await;
        assert_eq!(
            pinned.status, 200,
            "case {:?}: expected 200 for ?schemaVersion=1, got {} {:?}",
            case.case, pinned.status, pinned.body
        );
        assert_eq!(
            pinned.body, described.body,
            "case {:?}: version 1 is the latest version",
            case.case
        );
    }
}

/// The `Type` table above must cover every value the model admits, so a value added
/// to the model is not silently left untested.
#[test]
fn the_schema_type_table_covers_every_modelled_type() {
    // Source: `com.amazonaws.schemas#Type`, re-exported as `TYPE_VALUES`.
    let cases = schema_cases();
    for modelled in TYPE_VALUES {
        assert!(
            cases.iter().any(|case| case.schema_type == *modelled),
            "the table must exercise Type {modelled:?}, covered: {:?}",
            cases
                .iter()
                .map(|case| case.schema_type)
                .collect::<Vec<_>>()
        );
    }
}

/// RULE: an ARN a resource is created with names the region the request was signed
/// for, and keeps that value for the resource's life.
#[tokio::test]
async fn arns_name_the_region_the_request_was_signed_for() {
    // Rows: regions from three partitions' worth of naming shapes; the rule is
    // "whatever the credential scope said", not a fixed list.
    for region in ["us-east-1", "eu-west-3", "ap-southeast-2", "us-gov-west-1"] {
        let emulator = Emulator::new();
        let created = emulator
            .call_signed_for(region, "POST", &registry_uri("regional"), Some(json!({})))
            .await;
        assert_eq!(
            created.status, 201,
            "case {region:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.str_at("RegistryArn"),
            format!("arn:aws:schemas:{region}:{ACCOUNT}:registry/regional"),
            "case {region:?}: the ARN must carry the signed region"
        );

        // And a read reports the stored ARN, even when the reader signs elsewhere.
        let described = emulator
            .call_signed_for("eu-central-1", "GET", &registry_uri("regional"), None)
            .await;
        assert_eq!(
            described.str_at("RegistryArn"),
            format!("arn:aws:schemas:{region}:{ACCOUNT}:registry/regional"),
            "case {region:?}: the stored ARN is reported, not re-derived per reader"
        );
    }
}

/// RULE: creating a resource under a name that already exists is refused as a
/// conflict with the stored resource (409 `ConflictException`, for a registry and for
/// a schema alike) and leaves the stored resource untouched.
#[tokio::test]
async fn creating_an_existing_name_is_refused() {
    let emulator = Emulator::new();
    let registry = "dup-registry";

    let first = emulator
        .call(
            "POST",
            &registry_uri(registry),
            Some(json!({"Description": "the original"})),
        )
        .await;
    assert_eq!(first.status, 201, "case registry-first: {:?}", first.body);

    let again = emulator
        .call(
            "POST",
            &registry_uri(registry),
            Some(json!({"Description": "the impostor"})),
        )
        .await;
    assert_eq!(
        again.status, 409,
        "case registry-again: expected 409, got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "ConflictException",
        "case registry-again: expected ConflictException, got {:?}",
        again.body
    );

    let described = emulator.call("GET", &registry_uri(registry), None).await;
    assert_eq!(
        described.str_at("Description"),
        "the original",
        "case registry-again: the refused create must not have overwritten anything"
    );

    let schema = schema_uri(registry, "dup-schema");
    let created = emulator
        .call(
            "POST",
            &schema,
            Some(json!({"Content": "{\"openapi\":\"3.0.0\"}", "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(created.status, 201, "case schema-first: {:?}", created.body);

    let recreated = emulator
        .call(
            "POST",
            &schema,
            Some(json!({"Content": "{\"openapi\":\"3.1.0\"}", "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        recreated.status, 409,
        "case schema-again: expected 409, got {} {:?}",
        recreated.status, recreated.body
    );
    assert_eq!(
        recreated.error_code(),
        "ConflictException",
        "case schema-again: expected ConflictException, got {:?}",
        recreated.body
    );
    let described = emulator.call("GET", &schema, None).await;
    assert_eq!(
        described.str_at("Content"),
        "{\"openapi\":\"3.0.0\"}",
        "case schema-again: the refused create must not have replaced the content"
    );
    assert_eq!(
        described.str_at("SchemaVersion"),
        "1",
        "case schema-again: a refused create must not mint a version"
    );
}

/// RULE: an operation that names a resource the store does not hold answers 404
/// `NotFoundException`, whatever the operation.
#[tokio::test]
async fn naming_an_absent_resource_is_a_404() {
    let emulator = Emulator::new();
    create_registry(&emulator, "present").await;
    emulator
        .call(
            "POST",
            &schema_uri("present", "present-schema"),
            Some(json!({"Content": "{}", "Type": "OpenApi3"})),
        )
        .await;

    // Rows: every registry-scoped operation in this slice, against a missing
    // registry, a missing schema and a missing version.
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("absent registry / DescribeRegistry", "GET", registry_uri("ghost"), None),
        ("absent registry / DeleteRegistry", "DELETE", registry_uri("ghost"), None),
        ("absent registry / ListSchemas", "GET", format!("{}/schemas", registry_uri("ghost")), None),
        ("absent registry / DescribeSchema", "GET", schema_uri("ghost", "any"), None),
        ("absent registry / DeleteSchema", "DELETE", schema_uri("ghost", "any"), None),
        (
            "absent registry / CreateSchema",
            "POST",
            schema_uri("ghost", "any"),
            Some(json!({"Content": "{}", "Type": "OpenApi3"})),
        ),
        ("absent schema / DescribeSchema", "GET", schema_uri("present", "ghost"), None),
        ("absent schema / DeleteSchema", "DELETE", schema_uri("present", "ghost"), None),
        (
            "absent version / DescribeSchema",
            "GET",
            format!("{}?schemaVersion=7", schema_uri("present", "present-schema")),
            None,
        ),
        (
            "non-numeric version / DescribeSchema",
            "GET",
            format!("{}?schemaVersion=latest", schema_uri("present", "present-schema")),
            None,
        ),
        (
            "space-padded version / DescribeSchema",
            "GET",
            format!("{}?schemaVersion=%201", schema_uri("present", "present-schema")),
            None,
        ),
        (
            "trailing-space version / DescribeSchema",
            "GET",
            format!("{}?schemaVersion=1%20", schema_uri("present", "present-schema")),
            None,
        ),
    ];

    for (case, method, uri, body) in cases {
        let answer = emulator.call(method, &uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            answer.body
        );
    }
}

/// RULE: a delete removes the resource, so every later read of it 404s and no
/// listing reports it; deleting the parent takes its children with it.
#[tokio::test]
async fn a_delete_removes_the_resource_from_every_read() {
    let emulator = Emulator::new();
    let registry = "lifecycle";
    create_registry(&emulator, registry).await;
    for name in ["alpha", "beta"] {
        let created = emulator
            .call(
                "POST",
                &schema_uri(registry, name),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(created.status, 201, "case {name:?}: {:?}", created.body);
    }

    let deleted = emulator
        .call("DELETE", &schema_uri(registry, "alpha"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "case delete-schema: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body,
        Value::Null,
        "case delete-schema: a 204 carries no body"
    );
    assert_eq!(
        emulator
            .call("GET", &schema_uri(registry, "alpha"), None)
            .await
            .status,
        404,
        "case delete-schema: DescribeSchema must 404 after the delete"
    );
    let listed = emulator
        .call("GET", &format!("{}/schemas", registry_uri(registry)), None)
        .await;
    assert_eq!(
        schema_names(&listed),
        vec!["beta".to_string()],
        "case delete-schema: the deleted schema is gone from ListSchemas, the other is not"
    );

    // Deleting the registry is refused while it still holds a schema (the rule is
    // asserted over a table in `aws_managed_and_versions.rs`), so the last schema
    // goes first.
    let refused = emulator.call("DELETE", &registry_uri(registry), None).await;
    assert_eq!(
        refused.status, 409,
        "case delete-registry: a registry that still holds a schema is refused, got {} {:?}",
        refused.status, refused.body
    );
    let deleted = emulator
        .call("DELETE", &schema_uri(registry, "beta"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "case delete-schema: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );

    let deleted = emulator.call("DELETE", &registry_uri(registry), None).await;
    assert_eq!(
        deleted.status, 204,
        "case delete-registry: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    for (case, uri) in [
        ("DescribeRegistry", registry_uri(registry)),
        ("ListSchemas", format!("{}/schemas", registry_uri(registry))),
        ("DescribeSchema", schema_uri(registry, "beta")),
    ] {
        assert_eq!(
            emulator.call("GET", &uri, None).await.status,
            404,
            "case delete-registry: {case} must 404 once the registry is gone"
        );
    }
    let registries = emulator
        .call("GET", "/v1/registries?scope=Local", None)
        .await;
    assert_eq!(
        registry_names(&registries),
        Vec::<String>::new(),
        "case delete-registry: ListRegistries no longer reports it"
    );
}

pub(crate) fn schema_names(answer: &Answer) -> Vec<String> {
    answer.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Schemas array in {:?}", answer.body))
        .iter()
        .map(|item| item["SchemaName"].as_str().unwrap_or_default().to_string())
        .collect()
}

pub(crate) fn registry_names(answer: &Answer) -> Vec<String> {
    answer.body["Registries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Registries array in {:?}", answer.body))
        .iter()
        .map(|item| item["RegistryName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE (an invariant): following `NextToken` walks a listing exactly once — the
/// concatenated pages are the full set in key order, with no repeat and no gap — and
/// the last page carries no token, for every page size.
#[tokio::test]
async fn following_next_token_walks_each_listing_exactly_once() {
    // Rows: the listing's size crossed with the page size, including a page size of
    // 1, one that divides the total exactly, one larger than the total, and the
    // unpaged case.
    for total in [0_usize, 1, 2, 5, 7] {
        let emulator = Emulator::new();
        let names: Vec<String> = (0..total).map(|i| format!("reg-{i:02}")).collect();
        for name in &names {
            create_registry(&emulator, name).await;
        }
        create_registry(&emulator, "holder").await;
        for name in &names {
            let created = emulator
                .call(
                    "POST",
                    &schema_uri("holder", name),
                    Some(json!({"Content": "{}", "Type": "OpenApi3"})),
                )
                .await;
            assert_eq!(created.status, 201, "case {name:?}: {:?}", created.body);
        }

        for limit in [None, Some(1), Some(2), Some(total.max(1)), Some(total + 3)] {
            let case = format!("total={total} limit={limit:?}");

            let walked = walk(
                &emulator,
                "/v1/registries?scope=Local",
                limit,
                registry_names,
            )
            .await;
            let mut expected = names.clone();
            expected.push("holder".to_string());
            expected.sort();
            assert_eq!(
                walked, expected,
                "case {case}: ListRegistries must walk every registry once, in key order"
            );

            let walked = walk(
                &emulator,
                "/v1/registries/name/holder/schemas",
                limit,
                schema_names,
            )
            .await;
            let mut expected = names.clone();
            expected.sort();
            assert_eq!(
                walked, expected,
                "case {case}: ListSchemas must walk every schema once, in key order"
            );
        }
    }
}

/// Follow a listing to exhaustion, returning every item in order. Panics if the
/// listing does not terminate within one page per item plus one.
pub(crate) async fn walk(
    emulator: &Emulator,
    path: &str,
    limit: Option<usize>,
    names: fn(&Answer) -> Vec<String>,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        // `path` may already carry the members that SELECT the listing (a search's
        // keywords, say), so the pagination members are appended to whatever query
        // string it has rather than starting a second one.
        let separator = if path.contains('?') { '&' } else { '?' };
        let uri = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}{separator}{}", query.join("&"))
        };
        let answer = emulator.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "walking {uri}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let page = names(&answer);
        if let Some(limit) = limit {
            assert!(
                page.len() <= limit,
                "walking {uri}: a page must hold at most {limit} items, got {}",
                page.len()
            );
        }
        seen.extend(page);
        match answer.body.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return seen,
        }
    }
    panic!("walking {path} did not terminate: NextToken never went absent");
}

/// RULE: a listing's name-prefix member selects exactly the stored names that start
/// with it — including the empty prefix (everything) and a prefix nothing matches.
#[tokio::test]
async fn a_name_prefix_selects_exactly_the_names_that_start_with_it() {
    let emulator = Emulator::new();
    let names = ["aa", "ab", "abc", "b", "ba"];
    for name in names {
        create_registry(&emulator, name).await;
        let created = emulator
            .call(
                "POST",
                &schema_uri(name, name),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(created.status, 201, "case {name:?}: {:?}", created.body);
    }

    // Rows: a prefix matching several, one matching one, one matching none, the
    // empty prefix, and a full name.
    for (prefix, expected) in [
        ("a", vec!["aa", "ab", "abc"]),
        ("ab", vec!["ab", "abc"]),
        ("abc", vec!["abc"]),
        ("abcd", vec![]),
        ("", vec!["aa", "ab", "abc", "b", "ba"]),
        ("b", vec!["b", "ba"]),
    ] {
        let answer = emulator
            .call(
                "GET",
                &format!("/v1/registries?scope=Local&registryNamePrefix={prefix}"),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case prefix={prefix:?}: got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            registry_names(&answer),
            expected,
            "case prefix={prefix:?}: ListRegistries must select exactly the matching names"
        );
    }

    // The same rule on ListSchemas, inside one registry holding several names.
    create_registry(&emulator, "many").await;
    for name in names {
        emulator
            .call(
                "POST",
                &schema_uri("many", name),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
    }
    for (prefix, expected) in [
        ("a", vec!["aa", "ab", "abc"]),
        ("ba", vec!["ba"]),
        ("zz", vec![]),
    ] {
        let answer = emulator
            .call(
                "GET",
                &format!("/v1/registries/name/many/schemas?schemaNamePrefix={prefix}"),
                None,
            )
            .await;
        assert_eq!(
            schema_names(&answer),
            expected,
            "case prefix={prefix:?}: ListSchemas must select exactly the matching names"
        );
    }
}

/// RULE: `Scope` LIMITS a listing that otherwise reports every registry — `Local`
/// selects the ones a caller created, `AWS` the service-owned ones, and a value that
/// is neither is refused.
#[tokio::test]
async fn scope_selects_between_custom_and_aws_managed_registries() {
    use service_com_amazonaws_schemas_schemas::state::AWS_MANAGED_REGISTRIES;

    let emulator = Emulator::new();
    create_registry(&emulator, "mine").await;

    // Source: `crate::state::AWS_MANAGED_REGISTRIES`, the service-owned registries
    // every account sees.
    let mut managed: Vec<String> = AWS_MANAGED_REGISTRIES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    managed.sort();
    let mut everything = managed.clone();
    everything.push("mine".to_string());
    everything.sort();

    // Rows: the two documented values in both casings, absent, and a value that is
    // neither.
    for (scope, expected) in [
        (Some("Local"), Some(vec!["mine".to_string()])),
        (Some("local"), Some(vec!["mine".to_string()])),
        (Some("LOCAL"), Some(vec!["mine".to_string()])),
        (Some("AWS"), Some(managed.clone())),
        (Some("aws"), Some(managed.clone())),
        (None, Some(everything.clone())),
        (Some("Everything"), None),
    ] {
        let uri = match scope {
            Some(scope) => format!("/v1/registries?scope={scope}"),
            None => "/v1/registries".to_string(),
        };
        let answer = emulator.call("GET", &uri, None).await;
        match expected {
            Some(expected) => {
                assert_eq!(
                    answer.status, 200,
                    "case scope={scope:?}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    registry_names(&answer),
                    expected,
                    "case scope={scope:?}: wrong selection"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 400,
                    "case scope={scope:?}: a value that is not a scope is refused, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "BadRequestException",
                    "case scope={scope:?}: expected BadRequestException, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE: a pagination member outside what it can mean is refused 400 — a page of
/// fewer than one item, and a `NextToken` this service never issued.
#[tokio::test]
async fn an_unusable_pagination_member_is_refused() {
    let emulator = Emulator::new();
    create_registry(&emulator, "paged").await;

    // Rows: both listings, crossed with a zero limit, a negative limit, and two
    // tokens the service did not mint (one non-hex, one hex of the wrong thing is
    // indistinguishable from a real token, so only foreign *forms* are asserted).
    for path in ["/v1/registries", "/v1/registries/name/paged/schemas"] {
        for query in ["limit=0", "limit=-1", "nextToken=not-hex", "nextToken=zz"] {
            let answer = emulator
                .call("GET", &format!("{path}?{query}"), None)
                .await;
            assert_eq!(
                answer.status, 400,
                "case {path} {query}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {path} {query}: expected BadRequestException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: no single response materialises a whole listing — a page holds at most
/// `MAX_PAGE_SIZE` items whatever `Limit` asks for, and the rest is reachable only
/// through `NextToken`.
#[tokio::test]
async fn a_page_is_capped_server_side_however_large_the_limit() {
    use service_com_amazonaws_schemas_schemas::state::MAX_PAGE_SIZE;

    let emulator = Emulator::new();
    let over = MAX_PAGE_SIZE + 5;
    let names: Vec<String> = (0..over).map(|i| format!("reg-{i:04}")).collect();
    create_registry(&emulator, "holder").await;
    for name in &names {
        create_registry(&emulator, name).await;
        let created = emulator
            .call(
                "POST",
                &schema_uri("holder", name),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(created.status, 201, "case {name:?}: {:?}", created.body);
    }

    // Rows: `Limit` absent, exactly the ceiling, one past it, and absurdly large.
    let ceiling = i64::try_from(MAX_PAGE_SIZE).expect("the ceiling fits an i64");
    for query in [
        String::new(),
        format!("?limit={ceiling}"),
        format!("?limit={}", ceiling + 1),
        format!("?limit={}", i32::MAX),
    ] {
        for (path, names_of) in [
            ("/v1/registries", registry_names as fn(&Answer) -> Vec<String>),
            ("/v1/registries/name/holder/schemas", schema_names),
        ] {
            let case = format!("{path}{query}");
            let answer = emulator.call("GET", &format!("{path}{query}"), None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = names_of(&answer);
            assert_eq!(
                page.len(),
                MAX_PAGE_SIZE,
                "case {case}: a page must hold at most {MAX_PAGE_SIZE} items, got {}",
                page.len()
            );
            assert!(
                answer.body.get("NextToken").and_then(Value::as_str).is_some(),
                "case {case}: a capped page that has more to give must hand back a token: {:?}",
                answer.body
            );
        }
    }

    // And the rest is still reachable: the walk returns every schema.
    let walked = walk(
        &emulator,
        "/v1/registries/name/holder/schemas",
        None,
        schema_names,
    )
    .await;
    let mut expected = names.clone();
    expected.sort();
    assert_eq!(
        walked, expected,
        "capping a page must not hide items: the walk still returns all {over}"
    );
}

/// RULE: the store a caller fills is bounded — a create past a cap is refused 400,
/// and the refusal leaves the store exactly at the cap.
#[tokio::test]
async fn a_create_past_the_store_cap_is_refused() {
    use service_com_amazonaws_schemas_schemas::state::{
        MAX_REGISTRIES, MAX_SCHEMAS_PER_REGISTRY,
    };

    let emulator = Emulator::new();
    for i in 0..MAX_REGISTRIES {
        let created = emulator
            .call("POST", &registry_uri(&format!("reg-{i:06}")), Some(json!({})))
            .await;
        assert_eq!(
            created.status, 201,
            "case registry {i}: a create below the cap must succeed, got {} {:?}",
            created.status, created.body
        );
    }
    let refused = emulator
        .call("POST", &registry_uri("one-too-many"), Some(json!({})))
        .await;
    assert_eq!(
        refused.status, 400,
        "case registries at cap: expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "BadRequestException",
        "case registries at cap: expected BadRequestException, got {:?}",
        refused.body
    );
    assert_eq!(
        emulator
            .call("GET", &registry_uri("one-too-many"), None)
            .await
            .status,
        404,
        "case registries at cap: the refused create must have stored nothing"
    );

    // The same rule one level down, on a fresh store so the registry cap is not in
    // play.
    let emulator = Emulator::new();
    create_registry(&emulator, "full").await;
    for i in 0..MAX_SCHEMAS_PER_REGISTRY {
        let created = emulator
            .call(
                "POST",
                &schema_uri("full", &format!("schema-{i:06}")),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case schema {i}: a create below the cap must succeed, got {} {:?}",
            created.status, created.body
        );
    }
    let refused = emulator
        .call(
            "POST",
            &schema_uri("full", "one-too-many"),
            Some(json!({"Content": "{}", "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case schemas at cap: expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        emulator
            .call("GET", &schema_uri("full", "one-too-many"), None)
            .await
            .status,
        404,
        "case schemas at cap: the refused create must have stored nothing"
    );

    // A different registry is unaffected: the cap is per registry.
    create_registry(&emulator, "roomy").await;
    let created = emulator
        .call(
            "POST",
            &schema_uri("roomy", "first"),
            Some(json!({"Content": "{}", "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case another registry: the per-registry cap must not be global, got {} {:?}",
        created.status, created.body
    );
}

/// RULE: the store's retained schema content is bounded too — the entry caps alone
/// would still admit `MAX_SCHEMAS_PER_REGISTRY` payloads of the modelled maximum
/// size, so a create that would take the store past its byte budget is refused 400.
#[tokio::test]
async fn a_create_past_the_content_budget_is_refused() {
    use service_com_amazonaws_schemas_schemas::state::MAX_STORED_CONTENT_BYTES;

    // `Content` is modelled `len 1..100000`; each row below stores the largest
    // payload the model admits, so the budget is reached in the fewest requests.
    // Every payload is a JSON object document, which is what a create will store at
    // all — `{"x":"…"}` frames the padding in 8 bytes.
    const PAYLOAD: usize = 100_000;
    /// A schema document of exactly `size` bytes (`size` must leave room for the
    /// 8-byte frame).
    fn document_of(size: usize) -> String {
        format!(r#"{{"x":"{}"}}"#, "x".repeat(size - 8))
    }
    let content = document_of(PAYLOAD);
    assert_eq!(
        content.len(),
        PAYLOAD,
        "the payload is exactly the modelled maximum"
    );

    let emulator = Emulator::new();
    let mut stored = 0usize;
    let mut refused_at = None;
    for i in 0..(MAX_STORED_CONTENT_BYTES / PAYLOAD + 2) {
        // Spread the schemas over registries so the per-registry count cap is not
        // what refuses first.
        let registry = format!("bulk-{:03}", i / 100);
        if i % 100 == 0 {
            create_registry(&emulator, &registry).await;
        }
        let answer = emulator
            .call(
                "POST",
                &schema_uri(&registry, &format!("schema-{i:06}")),
                Some(json!({"Content": content, "Type": "OpenApi3"})),
            )
            .await;
        if answer.status == 201 {
            stored += PAYLOAD;
            assert!(
                stored <= MAX_STORED_CONTENT_BYTES,
                "case {i}: a create was accepted past the {MAX_STORED_CONTENT_BYTES}-byte \
                 budget (store now holds {stored})"
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {i}: a create past the budget must be refused 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {i}: expected BadRequestException, got {:?}",
            answer.body
        );
        refused_at = Some(stored);
        break;
    }

    let refused_at = refused_at.unwrap_or_else(|| {
        panic!("the content budget of {MAX_STORED_CONTENT_BYTES} bytes was never reached")
    });
    assert!(
        refused_at + PAYLOAD > MAX_STORED_CONTENT_BYTES,
        "the refusal must come only when the next payload would not fit: refused with \
         {refused_at} of {MAX_STORED_CONTENT_BYTES} bytes stored"
    );

    // A payload that still fits is accepted after the refusal, so the budget bounds
    // the total rather than latching the store closed.
    let headroom = MAX_STORED_CONTENT_BYTES - refused_at;
    assert!(
        headroom >= 8,
        "there is room for at least one document: {headroom} bytes"
    );
    let answer = emulator
        .call(
            "POST",
            &schema_uri("bulk-000", "fits-in-the-headroom"),
            Some(json!({"Content": document_of(headroom), "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "a payload that fits the remaining {headroom} bytes must be accepted, got {} {:?}",
        answer.status, answer.body
    );
}

/// RULE: a `NextToken` names a position within ONE listing, so following it from a
/// listing selected differently is refused rather than silently skipping or
/// repeating items.
#[tokio::test]
async fn a_next_token_does_not_resume_a_differently_selected_listing() {
    let emulator = Emulator::new();
    for name in ["aa", "ab", "ba", "bb"] {
        create_registry(&emulator, name).await;
        let created = emulator
            .call(
                "POST",
                &schema_uri("aa", name),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(created.status, 201, "case {name:?}: {:?}", created.body);
    }

    // Rows: a token taken from one selection, re-presented under each OTHER
    // selection of the same listing.
    let registries = "/v1/registries";
    let schemas = "/v1/registries/name/aa/schemas";
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "ListRegistries, prefix changed",
            format!("{registries}?limit=1&registryNamePrefix=a"),
            vec![
                format!("{registries}?limit=1&registryNamePrefix=b"),
                format!("{registries}?limit=1"),
                format!("{registries}?limit=1&scope=AWS"),
            ],
        ),
        (
            "ListRegistries, prefix dropped",
            format!("{registries}?limit=1"),
            vec![format!("{registries}?limit=1&registryNamePrefix=a")],
        ),
        (
            "ListSchemas, prefix changed",
            format!("{schemas}?limit=1&schemaNamePrefix=a"),
            vec![
                format!("{schemas}?limit=1&schemaNamePrefix=b"),
                format!("{schemas}?limit=1"),
            ],
        ),
    ];

    for (case, first_page, re_presented_to) in cases {
        let first = emulator.call("GET", &first_page, None).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200 for {first_page}, got {} {:?}",
            first.status, first.body
        );
        let token = first
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {first_page} must hand back a token: {:?}", first.body)
            })
            .to_string();

        // Re-presenting it to its OWN listing is honoured.
        let resumed = emulator
            .call("GET", &format!("{first_page}&nextToken={token}"), None)
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: its own listing must honour the token, got {} {:?}",
            resumed.status, resumed.body
        );

        for other in re_presented_to {
            let answer = emulator
                .call("GET", &format!("{other}&nextToken={token}"), None)
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} -> {other}: a token from another selection must be \
                 refused, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?} -> {other}: expected BadRequestException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: state is per-emulator and survives between requests — two independent
/// emulators never see each other's resources.
#[tokio::test]
async fn each_emulator_keeps_its_own_store() {
    let first = Emulator::new();
    let second = Emulator::new();
    create_registry(&first, "only-in-first").await;

    assert_eq!(
        registry_names(&first.call("GET", "/v1/registries?scope=Local", None).await),
        vec!["only-in-first".to_string()],
        "case first: the registry it created is in its own listing"
    );
    assert_eq!(
        second.call("GET", &registry_uri("only-in-first"), None).await.status,
        404,
        "case second: another emulator's registry is not visible"
    );
}
