// SPDX-License-Identifier: CC0-1.0

//! The wire form of the members this service's shapes always report.
//!
//! Two rules are under test, both read off the Smithy model rather than off any one
//! recorded response:
//!
//! 1. A structure member the model marks `@timestampFormat("date-time")` appears on the
//!    wire as an RFC 3339 / ISO-8601 date-time STRING — never as restJson1's default
//!    epoch-seconds number. `smithy_query(query="shape",shape="Component,Theme,
//!    CodegenJob,CodegenJobSummary")` reports that trait on `Component$createdAt`,
//!    `Component$modifiedAt`, `Theme$createdAt`, `Theme$modifiedAt`,
//!    `CodegenJob$createdAt`, `CodegenJob$modifiedAt`, `CodegenJobSummary$createdAt` and
//!    `CodegenJobSummary$modifiedAt` — every `@timestampFormat` member this service
//!    models, and the catalog the schema case below walks.
//! 2. A component reports its last-modified instant and its tag map for every component,
//!    whatever the create supplied: `modifiedAt` is the instant the record last changed
//!    (its creation, until something updates it) and `tags` is a map — empty when the
//!    create named none — rather than an omitted member.
//!
//! Case 1 is checked both end to end over HTTP — every operation this service implements
//! that reports one of those four shapes: the `Component` payload of
//! `CreateComponent`/`GetComponent`/`UpdateComponent` and the `entities` of
//! `ExportComponents`, the `Theme` payload of `CreateTheme`/`GetTheme`/`UpdateTheme` and
//! the `entities` of `ExportThemes`, the `CodegenJob` payload of
//! `StartCodegenJob`/`GetCodegenJob` and the `CodegenJobSummary` `entities` of
//! `ListCodegenJobs` — and directly on the member schemas, which is the reach that
//! survives when a shape has no operation to drive it through.

use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::ops::types::{
    CodegenJob, CodegenJobSummary, Component, Theme,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The longest `TagKey` the model admits (`Tags` key `@length(min: 1, max: 128)`, from
/// `smithy_query(query="shape", shape="Tags")`).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// The longest `TagValue` the model admits (`Tags` value `@length(min: 1, max: 256)`).
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

fn components_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/components")
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    call(router, "POST", &components_uri(), Some(data)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", components_uri());
    call(router, "GET", &uri, None).await
}

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    let uri = format!("{}/{id}", components_uri());
    call(router, "PATCH", &uri, Some(patch)).await
}

fn themes_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/themes")
}

fn codegen_jobs_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/codegen-jobs")
}

/// The `@http` URI the export operations are bound to, for the collection named — from
/// `smithy_query(query="operation", operation="ExportComponents,ExportThemes")`:
/// `/export/app/{appId}/environment/{environmentName}/components` and `.../themes`.
fn export_uri(collection: &str) -> String {
    format!("/export/app/{APP_ID}/environment/{ENVIRONMENT}/{collection}")
}

async fn create_theme(router: &MultiServiceRouter, data: Value) -> Answer {
    call(router, "POST", &themes_uri(), Some(data)).await
}

async fn get_theme(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("{}/{id}", themes_uri()), None).await
}

async fn update_theme(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    call(router, "PATCH", &format!("{}/{id}", themes_uri()), Some(patch)).await
}

async fn start_codegen_job(router: &MultiServiceRouter, data: Value) -> Answer {
    call(router, "POST", &codegen_jobs_uri(), Some(data)).await
}

async fn get_codegen_job(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("{}/{id}", codegen_jobs_uri()), None).await
}

async fn list_codegen_jobs(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", &codegen_jobs_uri(), None).await
}

async fn export(router: &MultiServiceRouter, collection: &str) -> Answer {
    call(router, "GET", &export_uri(collection), None).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The `entities` a paginated answer carries — `@required` on every one of this service's
/// list and export outputs — panicking when the answer is not a page.
fn entities(answer: &Answer, operation: &str) -> Vec<Value> {
    answer.body["entities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "{operation} must answer a page carrying the required entities list, got {} \
                 with {:?}",
                answer.status, answer.body
            )
        })
        .clone()
}

/// The id an answer whose `@httpPayload` IS the resource reports, or the empty string.
fn reported_id(answer: &Answer) -> String {
    at(&answer.body, "/id")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The instant a `date-time` member names, in seconds since the epoch, or the reason the
/// value is not one: a number, a missing member and a malformed string are all failures
/// the caller reports with its own case name.
fn date_time_seconds(value: &Value) -> Result<f64, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("expected an ISO-8601 date-time string, got {value:?}"))?;
    let parsed = DateTime::from_str(text, Format::DateTime)
        .map_err(|error| format!("{text:?} is not an ISO-8601 date-time ({error})"))?;
    // The re-rendered instant must be the same text, so a string that merely starts with
    // something parseable cannot pass.
    let rendered = parsed
        .fmt(Format::DateTime)
        .map_err(|error| format!("{text:?} does not re-render ({error})"))?;
    if rendered != text {
        return Err(format!("{text:?} is not the canonical form of {rendered:?}"));
    }
    Ok(parsed.as_secs_f64())
}

/// The six required members of `CreateComponentData`, plus whatever a case adds.
fn create_data(name: &str, tags: Option<Value>) -> Value {
    let mut data = json!({
        "name": name,
        "componentType": "View",
        "properties": {"label": {"value": "hello"}},
        "variants": [{"variantValues": {"state": "default"}}],
        "overrides": {"Text": {"color": "#000000"}},
        "bindingProperties": {"image": {"type": "Data"}},
    });
    if let Some(tags) = tags {
        data.as_object_mut()
            .expect("the create payload is an object")
            .insert("tags".to_string(), tags);
    }
    data
}

/// A one-entry JSON object whose key is computed rather than written out.
fn one_entry(key: String, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(key, value);
    Value::Object(map)
}

/// The tag maps the cases drive, differing along what the rule generalises over: how many
/// entries the create supplied (none, one, several) and how long the key and value are
/// (one character, and the published `@length` maxima of `TagKey` and `TagValue`).
fn tag_cases() -> Vec<(&'static str, Option<Value>, Value)> {
    vec![
        ("no tags member at all", None, json!({})),
        ("an explicitly empty tag map", Some(json!({})), json!({})),
        (
            "one tag",
            Some(json!({"env": "test"})),
            json!({"env": "test"}),
        ),
        (
            "three tags",
            Some(json!({"env": "test", "team": "studio", "owner": "ui"})),
            json!({"env": "test", "team": "studio", "owner": "ui"}),
        ),
        (
            "a one-character key and value",
            Some(json!({"e": "t"})),
            json!({"e": "t"}),
        ),
        (
            "a key and value at the published length maxima",
            Some(one_entry(
                "k".repeat(TAG_KEY_MAX_LENGTH),
                json!("v".repeat(TAG_VALUE_MAX_LENGTH)),
            )),
            one_entry(
                "k".repeat(TAG_KEY_MAX_LENGTH),
                json!("v".repeat(TAG_VALUE_MAX_LENGTH)),
            ),
        ),
    ]
}

/// `createdAt` and `modifiedAt` come back as date-time strings, `tags` comes back as a
/// map for every component, and a read does not change either.
#[tokio::test]
async fn a_component_reports_date_time_timestamps_and_a_tag_map() {
    let router = emulator();

    for (index, (case, supplied, expected_tags)) in tag_cases().into_iter().enumerate() {
        // A name per row: a component name identifies a component within its app
        // environment, so two rows may not share one.
        let data = create_data(&format!("Hero{index}"), supplied.clone());
        let created = create(&router, data).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateComponent, got {} with {:?}",
            created.status, created.body
        );
        let id = at(&created.body, "/id")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetComponent, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case:?}: GetComponent must report exactly what CreateComponent persisted"
        );

        for (operation, body) in [("CreateComponent", &created.body), ("GetComponent", &read.body)]
        {
            for member in ["/createdAt", "/modifiedAt"] {
                let observed = at(body, member);
                let seconds = date_time_seconds(&observed).unwrap_or_else(|reason| {
                    panic!("case {case:?}: {operation}{member}: {reason}");
                });
                assert!(
                    seconds > 0.0,
                    "case {case:?}: {operation}{member} must name a real instant, got \
                     {observed:?}"
                );
            }
            assert_eq!(
                at(body, "/createdAt"),
                at(body, "/modifiedAt"),
                "case {case:?}: {operation} on a component nothing has updated reports \
                 modifiedAt at its createdAt, got {:?} and {:?}",
                at(body, "/createdAt"),
                at(body, "/modifiedAt")
            );
            assert_eq!(
                at(body, "/tags"),
                expected_tags,
                "case {case:?}: {operation} must report the tag map, got {:?}",
                at(body, "/tags")
            );
        }

        // A read reports the same instants however often it is made.
        let again = get(&router, &id).await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: two reads of one component must report the same record"
        );
    }
}

/// An update moves `modifiedAt` — still a date-time string, still at or after
/// `createdAt`, which does not move — and leaves the tag map the create stored.
#[tokio::test]
async fn an_update_moves_modified_at_in_the_same_wire_form() {
    // Rows differ in which member the patch names and in how many entries it carries.
    let patches = vec![
        ("the name only", json!({"name": "HeroV2"})),
        ("the component type only", json!({"componentType": "Flex"})),
        (
            "replaced properties and variants",
            json!({
                "properties": {"label": {"value": "after"}, "extra": {"value": "x"}},
                "variants": [],
            }),
        ),
        ("an emptied override map", json!({"overrides": {}})),
    ];

    let router = emulator();
    for (index, (case, patch)) in patches.into_iter().enumerate() {
        // A name per row, for the same reason as above.
        let data = create_data(&format!("Baseline{index}"), Some(json!({"env": "test"})));
        let created = create(&router, data).await;
        let id = at(&created.body, "/id")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let before = get(&router, &id).await.body;

        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateComponent, got {} with {:?}",
            updated.status, updated.body
        );

        let after = get(&router, &id).await.body;
        assert_eq!(
            after, updated.body,
            "case {case:?}: UpdateComponent's answer must match the stored record"
        );
        assert_eq!(
            at(&after, "/createdAt"),
            at(&before, "/createdAt"),
            "case {case:?}: an update must not move createdAt"
        );
        assert_eq!(
            at(&after, "/tags"),
            json!({"env": "test"}),
            "case {case:?}: an update naming no tags must leave the stored tag map, got {:?}",
            at(&after, "/tags")
        );

        let created_at = date_time_seconds(&at(&after, "/createdAt"))
            .unwrap_or_else(|reason| panic!("case {case:?}: createdAt: {reason}"));
        let modified_at = date_time_seconds(&at(&after, "/modifiedAt"))
            .unwrap_or_else(|reason| panic!("case {case:?}: modifiedAt: {reason}"));
        assert!(
            modified_at >= created_at,
            "case {case:?}: an updated component reports modifiedAt at or after createdAt, \
             got {modified_at} against {created_at}"
        );
    }
}

/// The theme payloads the end-to-end rows drive, each with the patch that updates it.
/// Rows differ along what the rule generalises over: how many `values` entries the create
/// supplies, how deeply they nest, and whether the optional `overrides` and `tags` are
/// present at all. A theme name identifies a theme within its app environment, so every
/// row carries its own.
fn theme_rows() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "only the required members, with an empty values list",
            json!({"name": "WireBare", "values": []}),
            json!({"values": [{"key": "primaryColor", "value": {"value": "#047d95"}}]}),
        ),
        (
            "flat values and tags",
            json!({
                "name": "WireFlat",
                "values": [
                    {"key": "primaryColor", "value": {"value": "#047d95"}},
                    {"key": "borderRadius", "value": {"value": "4px"}},
                ],
                "tags": {"env": "test"},
            }),
            json!({"name": "WireFlatV2", "values": []}),
        ),
        (
            "values nested two levels deep, plus overrides",
            json!({
                "name": "WireNested",
                "values": [
                    {
                        "key": "tokens",
                        "value": {
                            "children": [
                                {"key": "space", "value": {"value": "8px"}},
                            ],
                        },
                    },
                ],
                "overrides": [
                    {"key": "dark", "value": {"value": "#ffffff"}},
                ],
            }),
            json!({
                "values": [{"key": "space", "value": {"value": "16px"}}],
                "overrides": [],
            }),
        ),
    ]
}

/// The codegen job payloads the end-to-end rows drive. `renderConfig` is
/// `StartCodegenJobData`'s only `@required` member
/// (`smithy_query(query="shape", shape="StartCodegenJobData")`), so the rows differ in
/// which of the optional members they add.
fn codegen_job_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required renderConfig",
            json!({"renderConfig": {"react": {}}}),
        ),
        (
            "a configured render, autogenerated forms and tags",
            json!({
                "renderConfig": {"react": {"module": "es2020", "target": "es2020", "script": "tsx"}},
                "autoGenerateForms": true,
                "tags": {"env": "test"},
            }),
        ),
    ]
}

/// Over HTTP, every shape this service models with a `@timestampFormat("date-time")`
/// member reports those members as date-time STRINGS, through each operation that reports
/// the shape — the create, the read, the update, the list and the export alike — and an
/// update never moves `createdAt`.
///
/// This is the catalog of the schema case below, driven end to end instead of read off the
/// member schemas.
#[tokio::test]
async fn every_date_time_shape_reports_date_time_strings_over_http() {
    let router = emulator();
    // Each observation: the shape, the operation and case that reported it, and the JSON
    // object carrying its timestamp members.
    let mut observed: Vec<(&str, String, Value)> = Vec::new();

    // `Component`: the `@httpPayload` of the create, the read and the update, and an
    // `entities` entry of the export.
    for (index, (case, tags)) in [
        ("no tags member at all", None),
        ("one tag", Some(json!({"env": "test"}))),
    ]
    .into_iter()
    .enumerate()
    {
        let created = create(&router, create_data(&format!("Wire{index}"), tags)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateComponent, got {} with {:?}",
            created.status, created.body
        );
        let id = reported_id(&created);

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetComponent, got {} with {:?}",
            read.status, read.body
        );

        let updated = update(&router, &id, json!({"name": format!("Wire{index}V2")})).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateComponent, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            at(&updated.body, "/createdAt"),
            at(&created.body, "/createdAt"),
            "case {case:?}: UpdateComponent must not move createdAt, got {:?} against {:?}",
            at(&updated.body, "/createdAt"),
            at(&created.body, "/createdAt")
        );

        for (operation, body) in [
            ("CreateComponent", created.body),
            ("GetComponent", read.body),
            ("UpdateComponent", updated.body),
        ] {
            observed.push(("Component", format!("{operation}, {case}"), body));
        }
    }

    let exported_components = export(&router, "components").await;
    for (position, entity) in entities(&exported_components, "ExportComponents")
        .into_iter()
        .enumerate()
    {
        observed.push((
            "Component",
            format!("ExportComponents entity {position}"),
            entity,
        ));
    }

    // `Theme`: the same reach, through the theme operations.
    for (case, data, patch) in theme_rows() {
        let created = create_theme(&router, data).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
            created.status, created.body
        );
        let id = reported_id(&created);

        let read = get_theme(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetTheme, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case:?}: GetTheme must report exactly what CreateTheme persisted"
        );

        let updated = update_theme(&router, &id, patch).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateTheme, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            at(&updated.body, "/createdAt"),
            at(&created.body, "/createdAt"),
            "case {case:?}: UpdateTheme must not move createdAt, got {:?} against {:?}",
            at(&updated.body, "/createdAt"),
            at(&created.body, "/createdAt")
        );

        for (operation, body) in [
            ("CreateTheme", created.body),
            ("GetTheme", read.body),
            ("UpdateTheme", updated.body),
        ] {
            observed.push(("Theme", format!("{operation}, {case}"), body));
        }
    }

    let exported_themes = export(&router, "themes").await;
    for (position, entity) in entities(&exported_themes, "ExportThemes")
        .into_iter()
        .enumerate()
    {
        observed.push(("Theme", format!("ExportThemes entity {position}"), entity));
    }

    // `CodegenJob`, and the `CodegenJobSummary` the list reports for each one.
    for (case, data) in codegen_job_rows() {
        let started = start_codegen_job(&router, data).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartCodegenJob, got {} with {:?}",
            started.status, started.body
        );
        let id = reported_id(&started);

        let read = get_codegen_job(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetCodegenJob, got {} with {:?}",
            read.status, read.body
        );

        for (operation, body) in [
            ("StartCodegenJob", started.body),
            ("GetCodegenJob", read.body),
        ] {
            observed.push(("CodegenJob", format!("{operation}, {case}"), body));
        }
    }

    let listed_jobs = list_codegen_jobs(&router).await;
    for (position, entity) in entities(&listed_jobs, "ListCodegenJobs")
        .into_iter()
        .enumerate()
    {
        observed.push((
            "CodegenJobSummary",
            format!("ListCodegenJobs entity {position}"),
            entity,
        ));
    }

    // Every shape of the catalog was reached over the wire, so no row of the schema case
    // below is schema-only by accident.
    for shape in ["Component", "Theme", "CodegenJob", "CodegenJobSummary"] {
        assert!(
            observed.iter().any(|(reported, _, _)| *reported == shape),
            "case {shape:?}: no operation reported this shape over HTTP, so its \
             @timestampFormat members went unchecked on the wire"
        );
    }

    for (shape, case, body) in &observed {
        for member in ["createdAt", "modifiedAt"] {
            let value = at(body, &format!("/{member}"));
            let seconds = date_time_seconds(&value)
                .unwrap_or_else(|reason| panic!("case {case:?}: {shape}${member}: {reason}"));
            assert!(
                seconds > 0.0,
                "case {case:?}: {shape}${member} must name a real instant, got {value:?}"
            );
        }
        let created_at = date_time_seconds(&at(body, "/createdAt"))
            .unwrap_or_else(|reason| panic!("case {case:?}: {shape}$createdAt: {reason}"));
        let modified_at = date_time_seconds(&at(body, "/modifiedAt"))
            .unwrap_or_else(|reason| panic!("case {case:?}: {shape}$modifiedAt: {reason}"));
        assert!(
            modified_at >= created_at,
            "case {case:?}: {shape} reports modifiedAt at or after createdAt, got \
             {modified_at} against {created_at}"
        );
    }
}

/// The member schema the codec reads the wire form off: every member this service models
/// `@timestampFormat("date-time")` reports that format, and a member the model leaves
/// alone reports none.
///
/// This is the same catalog the end-to-end case above drives over HTTP, read here off the
/// schemas instead, and it is what would fail if the schemas went back to the vendored
/// codegen's output.
#[test]
fn every_modelled_date_time_member_carries_the_date_time_format() {
    // Source: `smithy_query(query="shape", shape="Component,Theme,CodegenJob,
    // CodegenJobSummary")` — the only members of this service carrying
    // `"timestampFormat": "date-time"`.
    let date_time_members: Vec<(&str, &Schema)> = vec![
        ("Component$createdAt", &Component::M_CREATED_AT),
        ("Component$modifiedAt", &Component::M_MODIFIED_AT),
        ("Theme$createdAt", &Theme::M_CREATED_AT),
        ("Theme$modifiedAt", &Theme::M_MODIFIED_AT),
        ("CodegenJob$createdAt", &CodegenJob::M_CREATED_AT),
        ("CodegenJob$modifiedAt", &CodegenJob::M_MODIFIED_AT),
        (
            "CodegenJobSummary$createdAt",
            &CodegenJobSummary::M_CREATED_AT,
        ),
        (
            "CodegenJobSummary$modifiedAt",
            &CodegenJobSummary::M_MODIFIED_AT,
        ),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model does not annotate keeps the protocol default, so the fix did not
    // put a format where the model names none.
    for (case, schema) in [
        ("Component$appId", &Component::M_APP_ID),
        ("Component$tags", &Component::M_TAGS),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}

/// A request body whose nesting is chosen by the caller is REFUSED, not fatal: the
/// recursive readers (`children`, and the self-referential property shapes under
/// `properties`) are bounded by the depth guard the vendored JSON deserializer takes at
/// the entry of `read_struct`/`read_list`/`read_map`
/// (`vendor/smithy-schema-server/src/json.rs`, `MAX_DEPTH` = 127), so the service answers
/// a client error and keeps serving.
///
/// Rows differ in WHICH self-referential member carries the nesting and how deep it goes
/// — one within the limit, which must be accepted and read back, and three far past it.
#[tokio::test]
async fn a_deeply_nested_request_body_is_refused_not_fatal() {
    /// `ComponentChild` nested `depth` levels deep through its own `children` member.
    fn nested_children(depth: usize) -> Value {
        let mut child = json!({"componentType": "Text", "name": "Leaf", "properties": {}});
        for _ in 0..depth {
            child = json!({
                "componentType": "Flex",
                "name": "Branch",
                "properties": {},
                "children": [child],
            });
        }
        json!([child])
    }

    /// `ComponentProperty` nested `depth` levels deep through its own `concat` member.
    fn nested_concat(depth: usize) -> Value {
        let mut property = json!({"value": "leaf"});
        for _ in 0..depth {
            property = json!({"concat": [property]});
        }
        json!({"label": property})
    }

    let router = emulator();
    let accepted_depth = 16;

    let shallow = create(&router, {
        let mut data = create_data("Shallow", None);
        data.as_object_mut()
            .expect("the create payload is an object")
            .insert("children".to_string(), nested_children(accepted_depth));
        data
    })
    .await;
    assert_eq!(
        shallow.status, 200,
        "case \"nesting within the codec's limit\": expected HTTP 200, got {} with {:?}",
        shallow.status, shallow.body
    );
    let id = at(&shallow.body, "/id")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        get(&router, &id).await.body,
        shallow.body,
        "case \"nesting within the codec's limit\": the nested children must read back as stored"
    );

    for (case, member, value) in [
        ("children nested 130 deep", "children", nested_children(130)),
        ("children nested 400 deep", "children", nested_children(400)),
        (
            "a property concatenated 400 deep",
            "properties",
            nested_concat(400),
        ),
    ] {
        let mut data = create_data("TooDeep", None);
        data.as_object_mut()
            .expect("the create payload is an object")
            .insert(member.to_string(), value);
        let answer = create(&router, data).await;
        assert!(
            (400..500).contains(&answer.status),
            "case {case:?}: a body nesting past the codec's limit must be refused as a \
             client error, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // The service is still serving afterwards, on the same state.
    let after = create(&router, create_data("StillServing", None)).await;
    assert_eq!(
        after.status, 200,
        "after a refused body the service must keep serving, got {} with {:?}",
        after.status, after.body
    );
    assert_eq!(
        get(&router, &id).await.status,
        200,
        "the component stored before the refused bodies must still be readable"
    );
}
