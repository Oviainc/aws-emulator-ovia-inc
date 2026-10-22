// SPDX-License-Identifier: CC0-1.0

//! The schema -> directory -> object slice of Amazon Cloud Directory, exercised end to
//! end over the emulator's own HTTP front door.
//!
//! Every case drives `MultiServiceRouter::dispatch`, so the routes, the
//! `x-amz-data-partition` header bindings, the restJson1 bodies and the modelled error
//! codes are all the ones a real client would see. Nothing reads the state struct
//! directly: a value is only accepted as stored once a later call reports it back.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints is scoped to (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The base path every route in this service hangs off (`http.uri` in the Smithy model).
const BASE: &str = "/amazonclouddirectory/2017-01-11";

/// The `FacetAttributeType` values the model declares, copied from
/// `src/ops/types/shared_types.rs`'s `FACET_ATTRIBUTE_TYPE_VALUES` (itself rendered
/// from the Smithy model's `FacetAttributeType` enum).
const FACET_ATTRIBUTE_TYPES: &[&str] = &[
    "BINARY", "BOOLEAN", "DATETIME", "NUMBER", "STRING", "VARIANT",
];

/// A SigV4 `Authorization` header scoped to `region`: the emulator's router reads the
/// target service and the Region out of the credential scope.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDCLOUDDIRTEST/20260101/{region}/clouddirectory/\
         aws4_request, SignedHeaders=host, Signature=00"
    )
}

/// What one call answered with: the HTTP status, the restJson1 `x-amzn-errortype` error
/// code when it failed, and the parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// A required string member of the response body.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "expected a string {member} in the response body, got {:?}",
                    self.body
                )
            })
            .to_string()
    }
}

/// One emulator, one service, one shared engine — exactly what the router mounts.
struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    fn new() -> Self {
        Self::in_region("us-east-1")
    }

    fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_clouddirectory_amazonclouddirectory_20170111::EmulatedService::new(
                engine,
            ),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// The SAME emulator (same router, same engine, so the same state) addressed as
    /// another Region: `MultiServiceRouter` is `Clone` and shares its service set.
    fn as_region(&self, region: &str) -> Self {
        Self {
            router: self.router.clone(),
            region: region.to_string(),
        }
    }

    /// Send one request. `partition` is the ARN the operation binds to the
    /// `x-amz-data-partition` header.
    async fn call(
        &self,
        method: &str,
        path: &str,
        partition: Option<&str>,
        body: Value,
    ) -> Answer {
        self.call_with_headers(method, path, partition, &[], body)
            .await
    }

    async fn call_with_headers(
        &self,
        method: &str,
        path: &str,
        partition: Option<&str>,
        extra: &[(&str, String)],
        body: Value,
    ) -> Answer {
        let host = format!("clouddirectory.{}.amazonaws.com", self.region);
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{BASE}{path}"))
            .header("host", host.as_str())
            .header("authorization", authorization(&self.region))
            .header("content-type", "application/json");
        if let Some(arn) = partition {
            builder = builder.header("x-amz-data-partition", arn);
        }
        for (name, value) in extra {
            builder = builder.header(*name, value.as_str());
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    async fn create_schema(&self, name: &str) -> Answer {
        self.call("PUT", "/schema/create", None, json!({ "Name": name }))
            .await
    }

    async fn create_facet(&self, schema_arn: &str, facet: Value) -> Answer {
        self.call("PUT", "/facet/create", Some(schema_arn), facet)
            .await
    }

    async fn publish_schema(&self, dev_arn: &str, body: Value) -> Answer {
        self.call("PUT", "/schema/publish", Some(dev_arn), body).await
    }

    async fn create_directory(&self, name: &str, published_arn: &str) -> Answer {
        self.call(
            "PUT",
            "/directory/create",
            Some(published_arn),
            json!({ "Name": name }),
        )
        .await
    }

    async fn create_object(&self, directory_arn: &str, body: Value) -> Answer {
        self.call("PUT", "/object", Some(directory_arn), body).await
    }

    async fn list_object_children(&self, directory_arn: &str, body: Value) -> Answer {
        self.call("POST", "/object/children", Some(directory_arn), body)
            .await
    }

    async fn get_directory(&self, directory_arn: &str) -> Answer {
        self.call("POST", "/directory/get", Some(directory_arn), json!({}))
            .await
    }

    async fn disable_directory(&self, directory_arn: &str) -> Answer {
        self.call("PUT", "/directory/disable", Some(directory_arn), json!({}))
            .await
    }

    async fn delete_directory(&self, directory_arn: &str) -> Answer {
        self.call("PUT", "/directory", Some(directory_arn), json!({}))
            .await
    }
}

/// The ARNs one provisioned schema-and-directory chain produced.
#[derive(Debug)]
struct Chain {
    development_schema_arn: String,
    published_schema_arn: String,
    directory_arn: String,
    applied_schema_arn: String,
    root_object_identifier: String,
}

/// Walk the whole happy path once: CreateSchema, CreateFacet for each facet given,
/// PublishSchema, CreateDirectory. Every ARN handed on is the one the previous call
/// answered with.
async fn provision(emulator: &Emulator, slug: &str, facets: &[Value]) -> Chain {
    let created = emulator.create_schema(slug).await;
    assert_eq!(
        created.status, 200,
        "CreateSchema {slug:?}: expected 200, got {} {:?}",
        created.status, created.body
    );
    let development_schema_arn = created.text("SchemaArn");

    for facet in facets {
        let answer = emulator.create_facet(&development_schema_arn, facet.clone()).await;
        assert_eq!(
            answer.status, 200,
            "CreateFacet {facet:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let published = emulator
        .publish_schema(&development_schema_arn, json!({ "Version": "1" }))
        .await;
    assert_eq!(
        published.status, 200,
        "PublishSchema {slug:?}: expected 200, got {} {:?}",
        published.status, published.body
    );
    let published_schema_arn = published.text("PublishedSchemaArn");

    let directory = emulator
        .create_directory(&format!("{slug}-dir"), &published_schema_arn)
        .await;
    assert_eq!(
        directory.status, 200,
        "CreateDirectory {slug:?}: expected 200, got {} {:?}",
        directory.status, directory.body
    );

    Chain {
        development_schema_arn,
        published_schema_arn,
        directory_arn: directory.text("DirectoryArn"),
        applied_schema_arn: directory.text("AppliedSchemaArn"),
        root_object_identifier: directory.text("ObjectIdentifier"),
    }
}

/// A `NODE` facet with one `STRING` attribute named `label`.
fn node_facet(name: &str) -> Value {
    json!({
        "Name": name,
        "ObjectType": "NODE",
        "Attributes": [{
            "Name": "label",
            "AttributeDefinition": { "Type": "STRING" },
            "RequiredBehavior": "NOT_REQUIRED"
        }]
    })
}

/// A value of each `FacetAttributeType`, in the `TypedAttributeValue` member that type
/// selects. `VARIANT` accepts a value of any other type, so it is given a string.
fn value_of_type(attribute_type: &str) -> Value {
    match attribute_type {
        // A blob renders as base64 on restJson1; this is "hi".
        "BINARY" => json!({ "BinaryValue": "aGk=" }),
        "BOOLEAN" => json!({ "BooleanValue": true }),
        // A timestamp with no timestampFormat renders as epoch seconds on restJson1.
        "DATETIME" => json!({ "DatetimeValue": 1_735_689_600 }),
        "NUMBER" => json!({ "NumberValue": "42" }),
        "STRING" | "VARIANT" => json!({ "StringValue": "hello" }),
        other => panic!("no sample value for FacetAttributeType {other:?}"),
    }
}

/// A value that is NOT of `attribute_type`, for the refusal half of the type rule.
/// `VARIANT` has no such value, hence the `Option`.
fn value_of_other_type(attribute_type: &str) -> Option<Value> {
    match attribute_type {
        "VARIANT" => None,
        "BOOLEAN" => Some(json!({ "StringValue": "not-a-bool" })),
        _ => Some(json!({ "BooleanValue": false })),
    }
}

/// Create an object carrying `facets` and link it under `selector` as `link`.
async fn attach_child(
    emulator: &Emulator,
    directory_arn: &str,
    facets: &Value,
    selector: &str,
    link: &str,
) -> Answer {
    emulator
        .create_object(
            directory_arn,
            json!({
                "SchemaFacets": facets,
                "ParentReference": { "Selector": selector },
                "LinkName": link
            }),
        )
        .await
}

/// The `Children` map of a ListObjectChildren answer.
fn children_of(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .body
        .get("Children")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("expected a Children map, got {:?}", answer.body))
        .iter()
        .map(|(link, identifier)| {
            (
                link.clone(),
                identifier
                    .as_str()
                    .unwrap_or_else(|| panic!("Children[{link}] must be a string"))
                    .to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CreateSchema
// ---------------------------------------------------------------------------

/// RULE: CreateSchema mints `schema/development/<Name>` in the caller's own account and
/// Region and records it, so the same name conflicts and a different name does not.
///
/// The rows differ along the shape of `Name` the model's `^[a-zA-Z0-9._-]*$` pattern
/// admits — including the 32-character upper bound of its length constraint.
#[tokio::test]
async fn create_schema_mints_and_records_a_development_arn_for_every_legal_name() {
    let emulator = Emulator::new();
    let cases = [
        "a",
        "demo",
        "with.dots",
        "with-dashes",
        "with_underscores",
        "MiXeDcAsE123",
        // The upper bound of SchemaName's length constraint: exactly 32 characters.
        "abcdefghij.klmnopqrst-uvwxyz0123",
    ];

    for case in cases {
        let answer = emulator.create_schema(case).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let expected =
            format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:schema/development/{case}");
        assert_eq!(
            answer.text("SchemaArn"),
            expected,
            "case {case:?}: expected SchemaArn {expected}, got {:?}",
            answer.text("SchemaArn")
        );

        let again = emulator.create_schema(case).await;
        assert_eq!(
            (again.status, again.error.as_deref()),
            (400, Some("SchemaAlreadyExistsException")),
            "case {case:?}: a repeated CreateSchema must conflict, got {} {:?}",
            again.status,
            again.error
        );
    }
}

/// RULE: the ARNs a call mints are scoped to the Region of the request's own SigV4
/// credential scope, not to a fixed one.
#[tokio::test]
async fn minted_arns_carry_the_requests_own_region() {
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let emulator = Emulator::in_region(region);
        let answer = emulator.create_schema("regional").await;
        let expected =
            format!("arn:aws:clouddirectory:{region}:{ACCOUNT}:schema/development/regional");
        assert_eq!(
            answer.text("SchemaArn"),
            expected,
            "case {region:?}: expected SchemaArn {expected}, got {:?}",
            answer.text("SchemaArn")
        );
    }
}

/// RULE: a `Name` outside `SchemaName`'s declared 1..32 length is refused at the
/// boundary, one character past the published bound included.
#[tokio::test]
async fn create_schema_refuses_a_name_past_the_declared_length_bound() {
    let emulator = Emulator::new();
    // 33 characters: one past SchemaName's max of 32.
    let too_long = "abcdefghij.klmnopqrst-uvwxyz01234";
    assert_eq!(too_long.len(), 33, "the over-long case must be 33 characters");
    let answer = emulator.create_schema(too_long).await;
    assert_eq!(
        answer.status, 400,
        "case {too_long:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
}

// ---------------------------------------------------------------------------
// CreateFacet and PublishSchema
// ---------------------------------------------------------------------------

/// RULE: a facet is recorded on the schema the `x-amz-data-partition` ARN names; an
/// unknown schema is not found, a name already taken on that schema conflicts, and a
/// published schema is immutable.
#[tokio::test]
async fn create_facet_records_on_the_named_schema_and_refuses_otherwise() {
    let emulator = Emulator::new();
    let created = emulator.create_schema("facets").await;
    let development = created.text("SchemaArn");

    // A facet of each ObjectType the model declares.
    for object_type in ["INDEX", "LEAF_NODE", "NODE", "POLICY"] {
        let facet = json!({ "Name": format!("f{object_type}"), "ObjectType": object_type });
        let answer = emulator.create_facet(&development, facet).await;
        assert_eq!(
            answer.status, 200,
            "case {object_type:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let again = emulator
            .create_facet(
                &development,
                json!({ "Name": format!("f{object_type}"), "ObjectType": object_type }),
            )
            .await;
        assert_eq!(
            (again.status, again.error.as_deref()),
            (400, Some("FacetAlreadyExistsException")),
            "case {object_type:?}: a repeated CreateFacet must conflict, got {} {:?}",
            again.status,
            again.error
        );
    }

    let unknown = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:schema/development/absent");
    let answer = emulator.create_facet(&unknown, node_facet("n")).await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case unknown-schema: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.error
    );

    let published = emulator
        .publish_schema(&development, json!({ "Version": "1" }))
        .await
        .text("PublishedSchemaArn");
    let answer = emulator.create_facet(&published, node_facet("late")).await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("ValidationException")),
        "case published-schema: a published schema is immutable, got {} {:?}",
        answer.status,
        answer.error
    );

    let answer = emulator
        .create_facet(
            &development,
            json!({ "Name": "bad", "ObjectType": "NOT_A_TYPE" }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("ValidationException")),
        "case bad-object-type: expected 400 ValidationException, got {} {:?}",
        answer.status,
        answer.error
    );

    let answer = emulator
        .create_facet(
            &development,
            json!({ "Name": "neither", "Attributes": [{ "Name": "x" }] }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("FacetValidationException")),
        "case attribute-without-definition: expected FacetValidationException, got {} {:?}",
        answer.status,
        answer.error
    );
}

/// RULE: PublishSchema mints `schema/published/<Name>/<Version>[/<MinorVersion>]` from
/// the development schema it is given — taking `Name` when one is supplied and the
/// development schema's own name otherwise — and the same name-and-version cannot be
/// published twice.
#[tokio::test]
async fn publish_schema_versions_a_frozen_copy_once_per_name_and_version() {
    let emulator = Emulator::new();
    let development = emulator.create_schema("pub").await.text("SchemaArn");
    emulator.create_facet(&development, node_facet("Node")).await;

    // (body, expected published resource path) — the rows vary the version, the minor
    // version and the name override independently.
    let cases: [(Value, &str); 4] = [
        (json!({ "Version": "1" }), "schema/published/pub/1"),
        (json!({ "Version": "2" }), "schema/published/pub/2"),
        (
            json!({ "Version": "2", "MinorVersion": "7" }),
            "schema/published/pub/2/7",
        ),
        (
            json!({ "Version": "1", "Name": "renamed" }),
            "schema/published/renamed/1",
        ),
    ];

    for (body, resource) in cases {
        let answer = emulator.publish_schema(&development, body.clone()).await;
        let expected = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:{resource}");
        assert_eq!(
            answer.text("PublishedSchemaArn"),
            expected,
            "case {body:?}: expected PublishedSchemaArn {expected}, got {:?}",
            answer.text("PublishedSchemaArn")
        );

        let again = emulator.publish_schema(&development, body.clone()).await;
        assert_eq!(
            (again.status, again.error.as_deref()),
            (400, Some("SchemaAlreadyPublishedException")),
            "case {body:?}: republishing must conflict, got {} {:?}",
            again.status,
            again.error
        );
    }

    let unknown = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:schema/development/absent");
    let answer = emulator
        .publish_schema(&unknown, json!({ "Version": "1" }))
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case unknown-development-schema: expected 404, got {} {:?}",
        answer.status,
        answer.error
    );
}

// ---------------------------------------------------------------------------
// CreateDirectory, read back through GetDirectory
// ---------------------------------------------------------------------------

/// RULE: CreateDirectory mints a directory ARN, applies a copy of the published schema
/// beneath it and creates the root object; every response member is read back out of
/// the stored record, and GetDirectory reports the same record afterwards.
#[tokio::test]
async fn create_directory_is_readable_through_get_directory() {
    let emulator = Emulator::new();
    for name in ["d", "demo-dir", "Dir.1_2-3"] {
        let development = emulator
            .create_schema(&format!("s{}", name.len()))
            .await
            .text("SchemaArn");
        emulator.create_facet(&development, node_facet("Node")).await;
        let published = emulator
            .publish_schema(&development, json!({ "Version": "1" }))
            .await
            .text("PublishedSchemaArn");

        let created = emulator.create_directory(name, &published).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            created.status, created.body
        );
        let directory_arn = created.text("DirectoryArn");
        let applied = created.text("AppliedSchemaArn");
        assert_eq!(
            created.text("Name"),
            name,
            "case {name:?}: Name must echo the stored name, got {:?}",
            created.text("Name")
        );
        let prefix = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:directory/");
        assert!(
            directory_arn.starts_with(&prefix),
            "case {name:?}: expected a DirectoryArn under {prefix}, got {directory_arn}"
        );
        let expected_applied = format!("{directory_arn}/schema/s{}/1", name.len());
        assert_eq!(
            applied, expected_applied,
            "case {name:?}: expected AppliedSchemaArn {expected_applied}, got {applied}"
        );
        assert!(
            !created.text("ObjectIdentifier").is_empty(),
            "case {name:?}: the root ObjectIdentifier must be non-empty"
        );

        let fetched = emulator.get_directory(&directory_arn).await;
        let directory = fetched
            .body
            .get("Directory")
            .cloned()
            .unwrap_or_else(|| panic!("case {name:?}: expected a Directory, got {:?}", fetched.body));
        assert_eq!(
            directory.get("Name").and_then(Value::as_str),
            Some(name),
            "case {name:?}: GetDirectory must report the stored name, got {directory:?}"
        );
        assert_eq!(
            directory.get("DirectoryArn").and_then(Value::as_str),
            Some(directory_arn.as_str()),
            "case {name:?}: GetDirectory must report the stored ARN, got {directory:?}"
        );
        assert_eq!(
            directory.get("State").and_then(Value::as_str),
            Some("ENABLED"),
            "case {name:?}: a new directory is ENABLED, got {directory:?}"
        );
        let first_creation = directory.get("CreationDateTime").cloned();
        assert!(
            first_creation.is_some(),
            "case {name:?}: CreationDateTime must be reported, got {directory:?}"
        );

        // A read must not change what the next caller sees.
        let again = emulator.get_directory(&directory_arn).await;
        assert_eq!(
            again.body.get("Directory").cloned(),
            Some(directory),
            "case {name:?}: a second GetDirectory must report the same record, got {:?}",
            again.body
        );

        // A second directory of the same name conflicts; the ARN is minted once.
        let duplicate = emulator.create_directory(name, &published).await;
        assert_eq!(
            (duplicate.status, duplicate.error.as_deref()),
            (400, Some("DirectoryAlreadyExistsException")),
            "case {name:?}: a repeated name must conflict, got {} {:?}",
            duplicate.status,
            duplicate.error
        );
    }
}

/// RULE: a directory name is unique per account and REGION, so the same name is free in
/// every other Region — the uniqueness scope is the one the minted ARN is scoped to.
#[tokio::test]
async fn a_directory_name_is_unique_only_within_its_own_region() {
    let home = Emulator::in_region("us-east-1");
    let away = home.as_region("eu-west-2");
    let name = "shared-name";

    let mut arns = Vec::new();
    for (case, emulator) in [("us-east-1", &home), ("eu-west-2", &away)] {
        let development = emulator.create_schema("regional").await.text("SchemaArn");
        emulator.create_facet(&development, node_facet("Node")).await;
        let published = emulator
            .publish_schema(&development, json!({ "Version": "1" }))
            .await
            .text("PublishedSchemaArn");
        let answer = emulator.create_directory(name, &published).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the name must be free in this Region, got {} {:?}",
            answer.status, answer.body
        );
        let arn = answer.text("DirectoryArn");
        assert!(
            arn.starts_with(&format!("arn:aws:clouddirectory:{case}:{ACCOUNT}:directory/")),
            "case {case:?}: expected a DirectoryArn in {case}, got {arn}"
        );
        arns.push(arn);

        // Within the Region it IS taken.
        let again = emulator.create_directory(name, &published).await;
        assert_eq!(
            (again.status, again.error.as_deref()),
            (400, Some("DirectoryAlreadyExistsException")),
            "case {case:?}: the same name in the same Region must conflict, got {} {:?}",
            again.status,
            again.error
        );
    }
    assert_ne!(
        arns[0], arns[1],
        "the two Regions must hold distinct directories, got {arns:?}"
    );
}

/// RULE: only a PUBLISHED schema can be applied to a directory, and a schema ARN that
/// names nothing is not found.
#[tokio::test]
async fn create_directory_refuses_a_schema_that_cannot_be_applied() {
    let emulator = Emulator::new();
    let development = emulator.create_schema("stages").await.text("SchemaArn");
    let published = emulator
        .publish_schema(&development, json!({ "Version": "1" }))
        .await
        .text("PublishedSchemaArn");
    let applied = emulator
        .create_directory("staged-dir", &published)
        .await
        .text("AppliedSchemaArn");
    let unknown = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:schema/published/absent/1");

    let cases: [(&str, &str, u16, &str); 3] = [
        ("development", development.as_str(), 400, "ValidationException"),
        ("applied", applied.as_str(), 400, "ValidationException"),
        ("absent", unknown.as_str(), 404, "ResourceNotFoundException"),
    ];
    for (case, arn, status, code) in cases {
        let answer = emulator.create_directory(&format!("dir-{case}"), arn).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.error
        );
    }
}

// ---------------------------------------------------------------------------
// CreateObject
// ---------------------------------------------------------------------------

/// RULE: an object's attribute value must be of the `FacetAttributeType` its facet
/// declares, and a value of any other type is refused as a facet violation.
///
/// The rows are every value of `FacetAttributeType` the model declares — including
/// `VARIANT`, which accepts a value of any other type and therefore has no refusal row.
#[tokio::test]
async fn create_object_stores_a_value_of_each_declared_attribute_type() {
    for attribute_type in FACET_ATTRIBUTE_TYPES {
        let emulator = Emulator::new();
        let facet = json!({
            "Name": "Typed",
            "ObjectType": "NODE",
            "Attributes": [{
                "Name": "value",
                "AttributeDefinition": { "Type": attribute_type },
                "RequiredBehavior": "REQUIRED_ALWAYS"
            }]
        });
        let chain = provision(&emulator, "typed", &[facet]).await;

        let key = json!({
            "SchemaArn": chain.applied_schema_arn,
            "FacetName": "Typed",
            "Name": "value"
        });
        let answer = emulator
            .create_object(
                &chain.directory_arn,
                json!({
                    "SchemaFacets": [{
                        "SchemaArn": chain.applied_schema_arn,
                        "FacetName": "Typed"
                    }],
                    "ObjectAttributeList": [{
                        "Key": key,
                        "Value": value_of_type(attribute_type)
                    }],
                    "ParentReference": { "Selector": "/" },
                    "LinkName": "typed"
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {attribute_type:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let identifier = answer.text("ObjectIdentifier");

        // The object is stored: the root's children name it under its link name.
        let listed = emulator
            .list_object_children(
                &chain.directory_arn,
                json!({ "ObjectReference": { "Selector": "/" } }),
            )
            .await;
        assert_eq!(
            children_of(&listed),
            BTreeMap::from([("typed".to_string(), identifier.clone())]),
            "case {attribute_type:?}: the root must list the object just created"
        );

        if let Some(mismatched) = value_of_other_type(attribute_type) {
            let answer = emulator
                .create_object(
                    &chain.directory_arn,
                    json!({
                        "SchemaFacets": [{
                            "SchemaArn": chain.applied_schema_arn,
                            "FacetName": "Typed"
                        }],
                        "ObjectAttributeList": [{ "Key": key, "Value": mismatched }],
                        "ParentReference": { "Selector": "/" },
                        "LinkName": "mismatched"
                    }),
                )
                .await;
            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (400, Some("FacetValidationException")),
                "case {attribute_type:?}: a value of another type must be refused, got {} {:?}",
                answer.status,
                answer.error
            );
        }

        // REQUIRED_ALWAYS with no default cannot be omitted.
        let answer = emulator
            .create_object(
                &chain.directory_arn,
                json!({
                    "SchemaFacets": [{
                        "SchemaArn": chain.applied_schema_arn,
                        "FacetName": "Typed"
                    }],
                    "ParentReference": { "Selector": "/" },
                    "LinkName": "missing"
                }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("FacetValidationException")),
            "case {attribute_type:?}: a missing required attribute must be refused, got {} {:?}",
            answer.status,
            answer.error
        );
    }
}

/// RULE: a `SchemaFacets` entry must name a facet of a schema APPLIED to the directory;
/// anything else is a facet violation, and an unknown directory is not found.
#[tokio::test]
async fn create_object_refuses_a_facet_the_directory_has_not_applied() {
    let emulator = Emulator::new();
    let chain = provision(&emulator, "facetval", &[node_facet("Node")]).await;

    let cases: [(&str, Value); 4] = [
        (
            "facet-name-absent-from-the-applied-schema",
            json!([{ "SchemaArn": chain.applied_schema_arn, "FacetName": "Nope" }]),
        ),
        (
            "schema-not-applied-to-this-directory",
            json!([{ "SchemaArn": chain.published_schema_arn, "FacetName": "Node" }]),
        ),
        (
            "development-schema-not-applied",
            json!([{ "SchemaArn": chain.development_schema_arn, "FacetName": "Node" }]),
        ),
        ("no-facets-at-all", json!([])),
    ];
    for (case, schema_facets) in cases {
        let answer = emulator
            .create_object(
                &chain.directory_arn,
                json!({ "SchemaFacets": schema_facets }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("FacetValidationException")),
            "case {case:?}: expected 400 FacetValidationException, got {} {:?}",
            answer.status,
            answer.error
        );
    }

    let unknown = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:directory/absent");
    let answer = emulator
        .create_object(
            &unknown,
            json!({ "SchemaFacets": [{ "SchemaArn": chain.applied_schema_arn, "FacetName": "Node" }] }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case unknown-directory: expected 404, got {} {:?}",
        answer.status,
        answer.error
    );
}

/// RULE: a link name is unique among one parent's children, so the same name under the
/// same parent is refused while the same name under a different parent is not; and
/// `ParentReference` and `LinkName` are only meaningful together.
#[tokio::test]
async fn create_object_links_are_unique_per_parent() {
    let emulator = Emulator::new();
    let chain = provision(&emulator, "links", &[node_facet("Node")]).await;
    let facets = json!([{ "SchemaArn": chain.applied_schema_arn, "FacetName": "Node" }]);

    let first = attach_child(&emulator, &chain.directory_arn, &facets, "/", "shared").await;
    assert_eq!(
        first.status, 200,
        "case first-link: expected 200, got {} {:?}",
        first.status, first.body
    );
    let child = first.text("ObjectIdentifier");

    let repeat = attach_child(&emulator, &chain.directory_arn, &facets, "/", "shared").await;
    assert_eq!(
        (repeat.status, repeat.error.as_deref()),
        (400, Some("LinkNameAlreadyInUseException")),
        "case repeated-link-under-same-parent: expected LinkNameAlreadyInUseException, \
         got {} {:?}",
        repeat.status,
        repeat.error
    );

    // The same link name under a DIFFERENT parent is fine: uniqueness is per parent.
    let nested =
        attach_child(&emulator, &chain.directory_arn, &facets, "/shared", "shared").await;
    assert_eq!(
        nested.status, 200,
        "case same-link-under-different-parent: expected 200, got {} {:?}",
        nested.status, nested.body
    );
    let grandchild = nested.text("ObjectIdentifier");
    assert_ne!(
        grandchild, child,
        "case same-link-under-different-parent: each object gets its own identifier"
    );

    let listed = emulator
        .list_object_children(
            &chain.directory_arn,
            json!({ "ObjectReference": { "Selector": format!("${child}") } }),
        )
        .await;
    assert_eq!(
        children_of(&listed),
        BTreeMap::from([("shared".to_string(), grandchild)]),
        "case nested-child: the child's own children must list the grandchild"
    );

    for (case, body) in [
        (
            "parent-without-link",
            json!({ "SchemaFacets": facets, "ParentReference": { "Selector": "/" } }),
        ),
        (
            "link-without-parent",
            json!({ "SchemaFacets": facets, "LinkName": "orphan" }),
        ),
    ] {
        let answer = emulator.create_object(&chain.directory_arn, body).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            answer.status,
            answer.error
        );
    }
}

// ---------------------------------------------------------------------------
// ListObjectChildren
// ---------------------------------------------------------------------------

/// RULE: every documented `ObjectReference.Selector` form resolves to the object it
/// names — a path of link names from the root, and an `$ObjectIdentifier` — while a
/// well-formed selector naming nothing is not found and a selector of no documented
/// form is refused.
#[tokio::test]
async fn list_object_children_resolves_every_documented_selector_form() {
    let emulator = Emulator::new();
    let chain = provision(&emulator, "selectors", &[node_facet("Node")]).await;
    let facets = json!([{ "SchemaArn": chain.applied_schema_arn, "FacetName": "Node" }]);

    let alpha = emulator
        .create_object(
            &chain.directory_arn,
            json!({
                "SchemaFacets": facets,
                "ParentReference": { "Selector": "/" },
                "LinkName": "alpha"
            }),
        )
        .await
        .text("ObjectIdentifier");
    let beta = emulator
        .create_object(
            &chain.directory_arn,
            json!({
                "SchemaFacets": facets,
                "ParentReference": { "Selector": "/alpha" },
                "LinkName": "beta"
            }),
        )
        .await
        .text("ObjectIdentifier");

    // (selector, the children it must report)
    let resolves: [(String, BTreeMap<String, String>); 6] = [
        (
            "/".to_string(),
            BTreeMap::from([("alpha".to_string(), alpha.clone())]),
        ),
        (
            format!("${}", chain.root_object_identifier),
            BTreeMap::from([("alpha".to_string(), alpha.clone())]),
        ),
        (
            "/alpha".to_string(),
            BTreeMap::from([("beta".to_string(), beta.clone())]),
        ),
        (
            format!("${alpha}"),
            BTreeMap::from([("beta".to_string(), beta.clone())]),
        ),
        // The docs wrap an ObjectIdentifier selector in double quotes.
        (
            format!("$\"{alpha}\""),
            BTreeMap::from([("beta".to_string(), beta.clone())]),
        ),
        ("/alpha/beta".to_string(), BTreeMap::new()),
    ];
    for (selector, expected) in resolves {
        let answer = emulator
            .list_object_children(
                &chain.directory_arn,
                json!({ "ObjectReference": { "Selector": selector } }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {selector:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            children_of(&answer),
            expected,
            "case {selector:?}: unexpected Children, got {:?}",
            answer.body
        );
    }

    let refusals: [(&str, u16, &str); 6] = [
        ("/alpha/absent", 404, "ResourceNotFoundException"),
        ("$notanidentifier", 404, "ResourceNotFoundException"),
        // Only ONE matched pair of the documented quotes is stripped.
        ("$\"\"doubly-quoted\"\"", 404, "ResourceNotFoundException"),
        // A batch reference only names an object inside a BatchRead/BatchWrite call.
        ("#someBatchReference", 400, "ValidationException"),
        ("alpha", 400, "ValidationException"),
        ("", 400, "ValidationException"),
    ];
    for (selector, status, code) in refusals {
        let answer = emulator
            .list_object_children(
                &chain.directory_arn,
                json!({ "ObjectReference": { "Selector": selector } }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (status, Some(code)),
            "case {selector:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.error
        );
    }
}

/// RULE: `MaxResults` bounds one page and `NextToken` resumes the listing, so paging a
/// listing to exhaustion yields exactly the links recorded, each once; a token this
/// service never issued is invalid.
///
/// The rows vary the number of children and the page size independently.
#[tokio::test]
async fn list_object_children_pages_exactly_the_recorded_links() {
    for child_count in [0_usize, 1, 3, 7] {
        for page_size in [1_usize, 2, 3, 10] {
            let emulator = Emulator::new();
            let chain = provision(&emulator, "paging", &[node_facet("Node")]).await;
            let facets =
                json!([{ "SchemaArn": chain.applied_schema_arn, "FacetName": "Node" }]);

            let mut expected = BTreeMap::new();
            for index in 0..child_count {
                let link = format!("child-{index:02}");
                let identifier = emulator
                    .create_object(
                        &chain.directory_arn,
                        json!({
                            "SchemaFacets": facets,
                            "ParentReference": { "Selector": "/" },
                            "LinkName": link
                        }),
                    )
                    .await
                    .text("ObjectIdentifier");
                expected.insert(link, identifier);
            }

            let case = format!("{child_count} children, MaxResults {page_size}");
            let mut seen: BTreeMap<String, String> = BTreeMap::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut body = json!({
                    "ObjectReference": { "Selector": "/" },
                    "MaxResults": page_size
                });
                if let Some(next) = token.as_deref() {
                    body["NextToken"] = json!(next);
                }
                let answer = emulator.list_object_children(&chain.directory_arn, body).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: page {pages} expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                let page = children_of(&answer);
                assert!(
                    page.len() <= page_size,
                    "case {case}: page {pages} holds {} entries, more than MaxResults",
                    page.len()
                );
                for (link, identifier) in page {
                    assert!(
                        seen.insert(link.clone(), identifier).is_none(),
                        "case {case}: link {link} was reported on more than one page"
                    );
                }
                pages += 1;
                token = answer
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= child_count + 1,
                    "case {case}: paging did not terminate after {pages} pages"
                );
            }
            assert_eq!(
                seen, expected,
                "case {case}: paging to exhaustion must yield exactly the recorded links"
            );

            let answer = emulator
                .list_object_children(
                    &chain.directory_arn,
                    json!({
                        "ObjectReference": { "Selector": "/" },
                        "NextToken": "not-a-token-this-service-issued"
                    }),
                )
                .await;
            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (400, Some("InvalidNextTokenException")),
                "case {case}: a token this service never issued must be invalid, got {} {:?}",
                answer.status,
                answer.error
            );
        }
    }
}

/// RULE: `MaxResults` has a declared minimum of 1, so 1 is accepted and 0 — one past
/// the published bound — is refused.
#[tokio::test]
async fn list_object_children_honours_the_declared_max_results_bound() {
    let emulator = Emulator::new();
    let chain = provision(&emulator, "bounds", &[node_facet("Node")]).await;

    for (case, max_results, status) in [("at-the-bound", 1, 200), ("past-the-bound", 0, 400)] {
        let answer = emulator
            .list_object_children(
                &chain.directory_arn,
                json!({
                    "ObjectReference": { "Selector": "/" },
                    "MaxResults": max_results
                }),
            )
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?} (MaxResults {max_results}): expected {status}, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: only a NODE holds children, so asking an object whose facets declare another
/// object type for its children is refused as not-a-node.
#[tokio::test]
async fn list_object_children_refuses_an_object_that_is_not_a_node() {
    // (ObjectType, whether an object carrying only that facet holds children)
    let cases: [(&str, bool); 3] = [("NODE", true), ("LEAF_NODE", false), ("POLICY", false)];
    for (object_type, is_node) in cases {
        let emulator = Emulator::new();
        let facet = json!({ "Name": "F", "ObjectType": object_type });
        let chain = provision(&emulator, "nodes", &[facet]).await;
        let identifier = emulator
            .create_object(
                &chain.directory_arn,
                json!({
                    "SchemaFacets": [{
                        "SchemaArn": chain.applied_schema_arn,
                        "FacetName": "F"
                    }],
                    "ParentReference": { "Selector": "/" },
                    "LinkName": "only"
                }),
            )
            .await
            .text("ObjectIdentifier");

        let answer = emulator
            .list_object_children(
                &chain.directory_arn,
                json!({ "ObjectReference": { "Selector": format!("${identifier}") } }),
            )
            .await;
        if is_node {
            assert_eq!(
                answer.status, 200,
                "case {object_type:?}: a NODE must list children, got {} {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (400, Some("NotNodeException")),
                "case {object_type:?}: expected 400 NotNodeException, got {} {:?}",
                answer.status,
                answer.error
            );
        }

        // The same rule binds the writer: an object that holds no children cannot be a
        // parent either, so no graph with children under a non-node is ever recorded.
        let answer = attach_child(
            &emulator,
            &chain.directory_arn,
            &json!([{ "SchemaArn": chain.applied_schema_arn, "FacetName": "F" }]),
            &format!("${identifier}"),
            "below",
        )
        .await;
        if is_node {
            assert_eq!(
                answer.status, 200,
                "case {object_type:?}: a NODE must accept a child, got {} {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (400, Some("ValidationException")),
                "case {object_type:?}: a non-NODE parent must be refused, got {} {:?}",
                answer.status,
                answer.error
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DisableDirectory / DeleteDirectory
// ---------------------------------------------------------------------------

/// RULE: a directory is deletable only from DISABLED, disabling stops it serving data
/// operations, and deleting is terminal — the state each call reports is the one stored.
#[tokio::test]
async fn the_directory_lifecycle_gates_data_operations_and_deletion() {
    let emulator = Emulator::new();
    let chain = provision(&emulator, "lifecycle", &[node_facet("Node")]).await;
    let facets = json!([{ "SchemaArn": chain.applied_schema_arn, "FacetName": "Node" }]);
    let list_root = json!({ "ObjectReference": { "Selector": "/" } });

    // ENABLED: data operations work, deletion does not.
    let answer = emulator.delete_directory(&chain.directory_arn).await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("DirectoryNotDisabledException")),
        "case delete-while-enabled: expected DirectoryNotDisabledException, got {} {:?}",
        answer.status,
        answer.error
    );
    let created = emulator
        .create_object(
            &chain.directory_arn,
            json!({
                "SchemaFacets": facets,
                "ParentReference": { "Selector": "/" },
                "LinkName": "before"
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case create-while-enabled: expected 200, got {} {:?}",
        created.status, created.body
    );

    // DISABLED: the stored state flips and data operations stop.
    let disabled = emulator.disable_directory(&chain.directory_arn).await;
    assert_eq!(
        disabled.text("DirectoryArn"),
        chain.directory_arn,
        "case disable: DirectoryArn must name the directory disabled"
    );
    let state = |answer: &Answer| -> Option<String> {
        answer
            .body
            .get("Directory")
            .and_then(|directory| directory.get("State"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let fetched = emulator.get_directory(&chain.directory_arn).await;
    assert_eq!(
        state(&fetched).as_deref(),
        Some("DISABLED"),
        "case disable: GetDirectory must report DISABLED, got {:?}",
        fetched.body
    );
    for (case, answer) in [
        (
            "create-while-disabled",
            emulator
                .create_object(
                    &chain.directory_arn,
                    json!({
                        "SchemaFacets": facets,
                        "ParentReference": { "Selector": "/" },
                        "LinkName": "during"
                    }),
                )
                .await,
        ),
        (
            "list-while-disabled",
            emulator
                .list_object_children(&chain.directory_arn, list_root.clone())
                .await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("DirectoryNotEnabledException")),
            "case {case:?}: expected DirectoryNotEnabledException, got {} {:?}",
            answer.status,
            answer.error
        );
    }

    // DELETED: terminal, and still what GetDirectory reports.
    let deleted = emulator.delete_directory(&chain.directory_arn).await;
    assert_eq!(
        deleted.text("DirectoryArn"),
        chain.directory_arn,
        "case delete: DirectoryArn must name the directory deleted"
    );
    let fetched = emulator.get_directory(&chain.directory_arn).await;
    assert_eq!(
        state(&fetched).as_deref(),
        Some("DELETED"),
        "case delete: GetDirectory must report DELETED, got {:?}",
        fetched.body
    );
    for (case, answer) in [
        (
            "delete-twice",
            emulator.delete_directory(&chain.directory_arn).await,
        ),
        (
            "disable-after-delete",
            emulator.disable_directory(&chain.directory_arn).await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("DirectoryDeletedException")),
            "case {case:?}: expected DirectoryDeletedException, got {} {:?}",
            answer.status,
            answer.error
        );
    }
    let answer = emulator
        .list_object_children(&chain.directory_arn, list_root)
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("DirectoryNotEnabledException")),
        "case list-after-delete: expected DirectoryNotEnabledException, got {} {:?}",
        answer.status,
        answer.error
    );

    // The name a deleted directory held is free again.
    let reused = emulator
        .create_directory("lifecycle-dir", &chain.published_schema_arn)
        .await;
    assert_eq!(
        reused.status, 200,
        "case reuse-name-after-delete: expected 200, got {} {:?}",
        reused.status, reused.body
    );

    // Deleting releases the directory's per-directory applied schema copy, so the
    // applied ARN names nothing afterwards.
    let answer = emulator
        .create_facet(&chain.applied_schema_arn, node_facet("Late"))
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case applied-schema-after-delete: the applied copy must be released, got {} {:?}",
        answer.status,
        answer.error
    );
}

/// RULE: the directory store is bounded, so once it holds the emulator's own maximum a
/// further CreateDirectory is refused with the modelled `LimitExceededException` rather
/// than growing the store without end.
#[tokio::test]
async fn the_directory_store_is_bounded_by_its_declared_quota() {
    use service_com_amazonaws_clouddirectory_amazonclouddirectory_20170111::state::DIRECTORY_QUOTA;

    let emulator = Emulator::new();
    let development = emulator.create_schema("quota").await.text("SchemaArn");
    emulator.create_facet(&development, node_facet("Node")).await;
    let published = emulator
        .publish_schema(&development, json!({ "Version": "1" }))
        .await
        .text("PublishedSchemaArn");

    let mut first_directory = String::new();
    for index in 0..DIRECTORY_QUOTA {
        let answer = emulator
            .create_directory(&format!("dir-{index:04}"), &published)
            .await;
        assert_eq!(
            answer.status, 200,
            "case directory {index} of {DIRECTORY_QUOTA}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        if index == 0 {
            first_directory = answer.text("DirectoryArn");
        }
    }

    let answer = emulator.create_directory("one-too-many", &published).await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("LimitExceededException")),
        "case past-the-quota: expected 400 LimitExceededException, got {} {:?}",
        answer.status,
        answer.error
    );

    // The bound counts LIVE directories, so deleting one frees capacity at once.
    emulator.disable_directory(&first_directory).await;
    emulator.delete_directory(&first_directory).await;
    let answer = emulator.create_directory("after-a-delete", &published).await;
    assert_eq!(
        answer.status, 200,
        "case after-a-delete: deleting must free capacity, got {} {:?}",
        answer.status, answer.body
    );
}

/// RULE: the per-request work a caller sizes is bounded too — a `CreateObject` naming
/// more facets or carrying more attribute values than the emulator's own maximum, and a
/// `CreateFacet` defining more attributes than its maximum, are refused with the
/// modelled `LimitExceededException` instead of being served.
#[tokio::test]
async fn caller_sized_request_lists_are_bounded_by_their_declared_quotas() {
    use service_com_amazonaws_clouddirectory_amazonclouddirectory_20170111::state::{
        ATTRIBUTES_PER_FACET_QUOTA, ATTRIBUTES_PER_OBJECT_QUOTA, SCHEMA_FACETS_PER_OBJECT_QUOTA,
    };

    let emulator = Emulator::new();
    let chain = provision(&emulator, "reqbound", &[node_facet("Node")]).await;
    let one_facet = json!({ "SchemaArn": chain.applied_schema_arn, "FacetName": "Node" });

    // One facet named over and over is deduplicated, so it is served, not refused.
    let duplicated: Vec<Value> = (0..SCHEMA_FACETS_PER_OBJECT_QUOTA)
        .map(|_| one_facet.clone())
        .collect();
    let answer = emulator
        .create_object(
            &chain.directory_arn,
            json!({ "SchemaFacets": duplicated, "ParentReference": { "Selector": "/" },
                    "LinkName": "deduped" }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case at-the-facet-bound: expected 200, got {} {:?}",
        answer.status, answer.body
    );

    // One past the bound is refused, before any state is touched.
    let too_many: Vec<Value> = (0..=SCHEMA_FACETS_PER_OBJECT_QUOTA)
        .map(|index| json!({ "SchemaArn": chain.applied_schema_arn, "FacetName": format!("F{index}") }))
        .collect();
    let answer = emulator
        .create_object(&chain.directory_arn, json!({ "SchemaFacets": too_many }))
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("LimitExceededException")),
        "case past-the-facet-bound: expected LimitExceededException, got {} {:?}",
        answer.status,
        answer.error
    );

    let attribute_values: Vec<Value> = (0..=ATTRIBUTES_PER_OBJECT_QUOTA)
        .map(|index| {
            json!({
                "Key": {
                    "SchemaArn": chain.applied_schema_arn,
                    "FacetName": "Node",
                    "Name": format!("a{index}")
                },
                "Value": { "StringValue": "x" }
            })
        })
        .collect();
    let answer = emulator
        .create_object(
            &chain.directory_arn,
            json!({ "SchemaFacets": [one_facet], "ObjectAttributeList": attribute_values }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("LimitExceededException")),
        "case past-the-attribute-bound: expected LimitExceededException, got {} {:?}",
        answer.status,
        answer.error
    );

    let facet_attributes: Vec<Value> = (0..=ATTRIBUTES_PER_FACET_QUOTA)
        .map(|index| {
            json!({
                "Name": format!("a{index}"),
                "AttributeDefinition": { "Type": "STRING" }
            })
        })
        .collect();
    let answer = emulator
        .create_facet(
            &chain.development_schema_arn,
            json!({ "Name": "Wide", "ObjectType": "NODE", "Attributes": facet_attributes }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some("LimitExceededException")),
        "case past-the-facet-attribute-bound: expected LimitExceededException, got {} {:?}",
        answer.status,
        answer.error
    );
}

/// RULE: a directory ARN that names nothing is answered as not found by the operations
/// that model it, and as access-denied by GetDirectory, whose `AccessDeniedException`
/// is the error its model documents for a directory that does not exist.
#[tokio::test]
async fn an_unknown_directory_arn_is_refused_by_every_directory_operation() {
    let emulator = Emulator::new();
    let unknown = format!("arn:aws:clouddirectory:us-east-1:{ACCOUNT}:directory/absent");

    let cases: [(&str, Answer); 4] = [
        ("GetDirectory", emulator.get_directory(&unknown).await),
        (
            "DisableDirectory",
            emulator.disable_directory(&unknown).await,
        ),
        ("DeleteDirectory", emulator.delete_directory(&unknown).await),
        (
            "ListObjectChildren",
            emulator
                .list_object_children(
                    &unknown,
                    json!({ "ObjectReference": { "Selector": "/" } }),
                )
                .await,
        ),
    ];
    for (case, answer) in cases {
        let expected = if case == "GetDirectory" {
            (403, Some("AccessDeniedException"))
        } else {
            (404, Some("ResourceNotFoundException"))
        };
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            expected,
            "case {case:?}: expected {expected:?}, got {} {:?}",
            answer.status,
            answer.error
        );
    }
}
