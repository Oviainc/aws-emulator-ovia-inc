// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the documentation set a `GetExport` carries: the
//! `x-amazon-apigateway-documentation` extension an export asks for with
//! `parameters={"extensions": "documentation"}`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the export helper, the
//! documentation-part helpers and the snapshot helper are this crate's existing harness,
//! reused from `stage_export`, `documentation_parts`, `documentation_versions`,
//! `patch_updates` and `execute_api_data_plane`.
//!
//! The RULE the rows assert: an export carries each vendor extension set its `parameters`
//! ask for, rendered from the stored records that set describes — and the documentation set
//! of an export is the `DocumentationVersion` SNAPSHOT the exported stage pins, never the
//! API's live documentation parts. The rows differ along what that rule generalises over:
//! which of several snapshots a stage pins, how the pin was made, how many parts the
//! snapshot froze and of which entity types, whether the live parts were since edited,
//! added to or removed, both dialects, and every `parameters` spelling that asks for the
//! set or does not.

use serde_json::{json, Value};

use crate::documentation_parts::{content_map, created_part_id, get_part, import_parts};
use crate::documentation_versions::create_version;
use crate::patch_updates::{patch, patch_ok, replace};
use crate::rest_api_lifecycle::{create_api, string_member, Answer, Gateway};
use crate::stage_export::{api_with_paths, export};

/// The two dialects `exportType` accepts, from `GetExportRequest$exportType`: "'oas30' for
/// OpenAPI 3.0.x and 'swagger' for Swagger/OpenAPI 2.0". The documentation set is part of
/// the definition whichever dialect renders it.
const DIALECTS: &[&str] = &["swagger", "oas30"];

/// The `parameters` that ask for the documentation set.
const ASKS_FOR_DOCUMENTATION: &[(&str, &str)] = &[("extensions", "documentation")];

/// The document key API Gateway carries a documentation set under, on the export side and
/// on the import side alike.
const DOCUMENTATION_EXTENSION: &str = "x-amazon-apigateway-documentation";

/// The query string `parameters` of an export request.
type Parameters<'a> = &'a [(&'a str, &'a str)];

/// One row of the asked-for table: the case, the request's `parameters`, and whether the
/// documentation set and the integration extension must be carried.
type AskedForCase<'a> = (&'a str, Parameters<'a>, bool, bool);

/// How the stage being exported came to pin a documentation version — the three paths a
/// stage's `documentationVersion` is written through.
#[derive(Debug, Clone, Copy)]
enum Pin {
    /// `CreateStage`'s own `documentationVersion` member.
    AtCreate,
    /// `CreateDocumentationVersion`'s `stageName`, "the stage name to be associated with
    /// the new documentation snapshot".
    ByAssociation,
    /// An `UpdateStage` patch of `/documentationVersion`, which repins a stage.
    ByPatch,
}

/// One row of the pinned-snapshot table: the case, the parts frozen into the snapshot as
/// `(location as the request sets it, location as it is stored, description)` triples, and
/// how the stage pins it.
type SnapshotCase<'a> = (&'a str, Vec<(Value, Value, &'a str)>, Pin);

/// `CreateStage` for `api`, sent as given and asserting the modelled 201.
async fn create_stage(gateway: &Gateway, api: &str, case: &str, request: Value) -> Value {
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateStage, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `CreateDeployment` for `api` carrying no stage, answering the deployment id: a stage
/// that pins a documentation version at CREATE time needs the deployment first.
async fn create_deployment(gateway: &Gateway, api: &str, case: &str) -> String {
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/deployments"), Some(json!({})))
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", case)
}

/// `CreateDocumentationVersion`, asserting the modelled 201.
async fn create_snapshot(gateway: &Gateway, api: &str, case: &str, request: Value) {
    let answer = create_version(gateway, api, request).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateDocumentationVersion, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `UpdateDocumentationPart`, replacing the part's content map with `description`.
async fn edit_part(gateway: &Gateway, api: &str, part: &str, case: &str, description: &str) {
    patch_ok(
        gateway,
        case,
        &format!("/restapis/{api}/documentation/parts/{part}"),
        vec![replace("/properties", &content_map(description))],
    )
    .await;
}

/// The documentation set an export carries, or `Value::Null` for an export that carries
/// none.
async fn exported_documentation(
    gateway: &Gateway,
    api: &str,
    stage: &str,
    dialect: &str,
    parameters: Parameters<'_>,
    case: &str,
) -> Value {
    let document = export(gateway, api, stage, dialect, None, parameters)
        .await
        .json(case);
    document
        .get(DOCUMENTATION_EXTENSION)
        .cloned()
        .unwrap_or(Value::Null)
}

/// One entry of a documentation set as the export must write it: the location as it is
/// STORED (every member the entity type makes valid, at its documented default where the
/// request set none) and the content map as the JSON object an OpenAPI document carries.
fn expected_entry(location: &Value, description: &str) -> Value {
    json!({ "location": location, "properties": { "description": description } })
}

/// The `documentationParts` of a documentation set, with the case named on failure.
fn parts_of(documentation: &Value, case: &str) -> Vec<Value> {
    documentation["documentationParts"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected a documentationParts list, got {documentation}")
        })
        .clone()
}

#[tokio::test]
async fn an_export_carries_the_snapshot_the_stage_pins_not_the_live_documentation_parts() {
    // Rows differ along what the rule generalises over: how many parts the snapshot froze,
    // which entity types they target, whether a location was spelled out or left to the
    // members its type defaults, and which of the three paths wrote the stage's pin.
    //
    // The location catalog each row draws on is `DocumentationPartType`'s value set with
    // each `DocumentationPartLocation` member's own valid-for list (source:
    // `smithy_query(query="shape", shape="DocumentationPartLocation")`, the same catalog
    // the `documentation_parts` suite tabulates) — the rows below target API, METHOD,
    // RESOURCE, QUERY_PARAMETER and RESPONSE entities, including a METHOD location left
    // entirely to its documented defaults (`path` "/" and `method` "*").
    let cases: Vec<SnapshotCase<'_>> = vec![
        (
            "one API-level part, pinned by CreateStage",
            vec![(
                json!({ "type": "API" }),
                json!({ "type": "API" }),
                "the pet store, v1",
            )],
            Pin::AtCreate,
        ),
        (
            "an API and a method part, pinned by the snapshot's own stageName",
            vec![
                (
                    json!({ "type": "API" }),
                    json!({ "type": "API" }),
                    "the pet store, v1",
                ),
                (
                    json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                    json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                    "list every pet, v1",
                ),
            ],
            Pin::ByAssociation,
        ),
        (
            "four parts across four entity types, pinned by an UpdateStage patch",
            vec![
                (
                    json!({ "type": "API" }),
                    json!({ "type": "API" }),
                    "the pet store, v1",
                ),
                (
                    json!({ "type": "RESOURCE", "path": "/pets" }),
                    json!({ "type": "RESOURCE", "path": "/pets" }),
                    "the pets collection, v1",
                ),
                (
                    json!({ "type": "QUERY_PARAMETER", "path": "/pets", "method": "GET", "name": "limit" }),
                    json!({ "type": "QUERY_PARAMETER", "path": "/pets", "method": "GET", "name": "limit" }),
                    "how many pets, v1",
                ),
                (
                    json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
                    json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
                    "the pets answered, v1",
                ),
            ],
            Pin::ByPatch,
        ),
        (
            // A location the request leaves to its documented defaults is STORED completed
            // (`path` "/" for the root resource, `method` "*" for any method), so that is
            // the location the export carries.
            "a method part left to the defaults its type documents",
            vec![(
                json!({ "type": "METHOD" }),
                json!({ "type": "METHOD", "path": "/", "method": "*" }),
                "every method, v1",
            )],
            Pin::AtCreate,
        ),
    ];

    for (case, frozen, pin) in cases {
        let gateway = Gateway::new();
        let (api, _) = api_with_paths(&gateway, "pinned", &[("/pets", &["GET"])]).await;
        let stage = "prod";
        let version = "1.0.0";

        // The parts as they stand when the snapshot is taken.
        let mut ids: Vec<String> = Vec::new();
        for (location, _, description) in &frozen {
            ids.push(
                created_part_id(
                    &gateway,
                    &api,
                    location.clone(),
                    &content_map(description),
                    case,
                )
                .await,
            );
        }

        // The snapshot, and the stage that pins it — by each of the three paths.
        match pin {
            Pin::AtCreate => {
                create_snapshot(
                    &gateway,
                    &api,
                    case,
                    json!({ "documentationVersion": version, "description": "the first cut" }),
                )
                .await;
                let deployment = create_deployment(&gateway, &api, case).await;
                create_stage(
                    &gateway,
                    &api,
                    case,
                    json!({
                        "stageName": stage,
                        "deploymentId": deployment,
                        "documentationVersion": version,
                    }),
                )
                .await;
            }
            Pin::ByAssociation => {
                let deployment = create_deployment(&gateway, &api, case).await;
                create_stage(
                    &gateway,
                    &api,
                    case,
                    json!({ "stageName": stage, "deploymentId": deployment }),
                )
                .await;
                create_snapshot(
                    &gateway,
                    &api,
                    case,
                    json!({ "documentationVersion": version, "stageName": stage }),
                )
                .await;
            }
            Pin::ByPatch => {
                create_snapshot(&gateway, &api, case, json!({ "documentationVersion": version }))
                    .await;
                let deployment = create_deployment(&gateway, &api, case).await;
                create_stage(
                    &gateway,
                    &api,
                    case,
                    json!({ "stageName": stage, "deploymentId": deployment }),
                )
                .await;
                patch_ok(
                    &gateway,
                    case,
                    &format!("/restapis/{api}/stages/{stage}"),
                    vec![replace("/documentationVersion", version)],
                )
                .await;
            }
        }

        // Everything the API's LIVE documentation does after the snapshot: every frozen
        // part is edited, and a part the snapshot never saw is created.
        for (index, part) in ids.iter().enumerate() {
            edit_part(&gateway, &api, part, case, &format!("edited part {index}")).await;
        }
        let added = created_part_id(
            &gateway,
            &api,
            json!({ "type": "REQUEST_HEADER", "path": "/pets", "method": "GET", "name": "X-Trace" }),
            &content_map("added after the snapshot"),
            case,
        )
        .await;

        let expected: Vec<Value> = frozen
            .iter()
            .map(|(_, stored, description)| expected_entry(stored, description))
            .collect();

        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let documentation =
                exported_documentation(&gateway, &api, stage, dialect, ASKS_FOR_DOCUMENTATION, &case)
                    .await;
            assert_eq!(
                documentation["version"].as_str(),
                Some(version),
                "case {case}: expected the pinned documentation version, got {documentation}"
            );
            let carried = parts_of(&documentation, &case);
            assert_eq!(
                carried, expected,
                "case {case}: expected the parts the snapshot froze, got {carried:?}"
            );
            assert!(
                !carried
                    .iter()
                    .any(|entry| entry["location"]["name"] == json!("X-Trace")),
                "case {case}: a part created after the snapshot is not part of it, got {carried:?}"
            );
        }

        // The positive control: the LIVE parts did change, and the readers of a part report
        // the edit the export does not carry.
        for (index, part) in ids.iter().enumerate() {
            let answer = get_part(&gateway, &api, part).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 from GetDocumentationPart, got {} with {:?}",
                answer.status, answer.body
            );
            let properties = string_member(&answer.body, "properties", case);
            assert_eq!(
                properties,
                content_map(&format!("edited part {index}")),
                "case {case}: expected the live part to report the edit, got {properties}"
            );
        }
        let answer = get_part(&gateway, &api, &added).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the part added after the snapshot to be readable, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn each_stage_exports_the_version_it_pins_and_a_stage_pinning_none_carries_no_set() {
    // One API, two snapshots of the same documentation at two different instants, and
    // three stages: each stage's export follows ITS pin, not the API's latest snapshot.
    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "two-snapshots", &[("/pets", &["GET"])]).await;
    let case = "two snapshots of one API";

    let api_part = created_part_id(
        &gateway,
        &api,
        json!({ "type": "API" }),
        &content_map("v1-description"),
        case,
    )
    .await;
    create_snapshot(&gateway, &api, case, json!({ "documentationVersion": "1.0.0" })).await;

    // The documentation moves on: the API part is edited and a method part is documented.
    edit_part(&gateway, &api, &api_part, case, "v2-description").await;
    created_part_id(
        &gateway,
        &api,
        json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
        &content_map("list every pet"),
        case,
    )
    .await;
    create_snapshot(&gateway, &api, case, json!({ "documentationVersion": "2.0.0" })).await;

    let deployment = create_deployment(&gateway, &api, case).await;
    for (stage, pinned) in [("old", Some("1.0.0")), ("new", Some("2.0.0")), ("unpinned", None)] {
        let mut request = json!({ "stageName": stage, "deploymentId": deployment });
        if let Some(pinned) = pinned {
            request["documentationVersion"] = json!(pinned);
        }
        create_stage(&gateway, &api, case, request).await;
    }

    // Rows: the stage, the version its export must name, and the entries that version
    // froze — the older snapshot holds ONE part at its pre-edit value, the newer one holds
    // two at their post-edit values, and the unpinned stage pins nothing to render.
    let expectations: Vec<(&str, Option<&str>, Vec<Value>)> = vec![
        (
            "old",
            Some("1.0.0"),
            vec![expected_entry(&json!({ "type": "API" }), "v1-description")],
        ),
        (
            "new",
            Some("2.0.0"),
            vec![
                expected_entry(&json!({ "type": "API" }), "v2-description"),
                expected_entry(
                    &json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                    "list every pet",
                ),
            ],
        ),
        ("unpinned", None, Vec::new()),
    ];

    for (stage, version, expected) in expectations {
        for dialect in DIALECTS {
            let case = format!("stage {stage} as {dialect}");
            let documentation =
                exported_documentation(&gateway, &api, stage, dialect, ASKS_FOR_DOCUMENTATION, &case)
                    .await;
            let Some(version) = version else {
                assert!(
                    documentation.is_null(),
                    "case {case}: a stage that pins no documentation version has no snapshot \
                     to export, got {documentation}"
                );
                continue;
            };
            assert_eq!(
                documentation["version"].as_str(),
                Some(version),
                "case {case}: expected the version this stage pins, got {documentation}"
            );
            let carried = parts_of(&documentation, &case);
            assert_eq!(
                carried, expected,
                "case {case}: expected the entries this version froze, got {carried:?}"
            );
        }
    }

    // A pin whose snapshot the API no longer holds leaves nothing to render either: the
    // live parts are not a snapshot.
    let answer = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/documentation/versions/1.0.0"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202 from DeleteDocumentationVersion, got {} with {:?}",
        answer.status, answer.body
    );
    for dialect in DIALECTS {
        let case = format!("a deleted snapshot as {dialect}");
        let documentation =
            exported_documentation(&gateway, &api, "old", dialect, ASKS_FOR_DOCUMENTATION, &case)
                .await;
        assert!(
            documentation.is_null(),
            "case {case}: a pin whose snapshot is gone exports no documentation set, got \
             {documentation}"
        );
    }
}

#[tokio::test]
async fn the_documentation_set_is_carried_only_when_the_parameters_ask_for_it() {
    // Rows differ along the `parameters` spelling: no parameters at all, each other
    // `extensions` value the member documents, the value that asks for the documentation
    // set, and the comma-separated combinations of it with the others.
    let cases: Vec<AskedForCase<'_>> = vec![
        ("no parameters at all", &[], false, false),
        (
            "extensions=integrations",
            &[("extensions", "integrations")],
            false,
            true,
        ),
        (
            "extensions=apigateway",
            &[("extensions", "apigateway")],
            false,
            true,
        ),
        (
            "extensions=authorizers",
            &[("extensions", "authorizers")],
            false,
            false,
        ),
        (
            "extensions=documentation",
            &[("extensions", "documentation")],
            true,
            false,
        ),
        (
            "documentation combined with the integration extensions",
            &[("extensions", "documentation,integrations")],
            true,
            true,
        ),
        (
            "documentation combined with API Gateway's own extension set",
            &[("extensions", "apigateway,documentation")],
            true,
            true,
        ),
        (
            "every extension set this export can carry at once",
            &[("extensions", "integrations,documentation,authorizers")],
            true,
            true,
        ),
    ];

    let gateway = Gateway::new();
    let (api, by_path) = api_with_paths(&gateway, "asked-for", &[("/pets", &["GET"])]).await;
    let case = "the documentation extension";
    let answer = crate::method_integration::put_integration(
        &gateway,
        &api,
        &by_path["/pets"],
        "GET",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
        }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from PutIntegration, got {} with {:?}",
        answer.status, answer.body
    );
    created_part_id(
        &gateway,
        &api,
        json!({ "type": "API" }),
        &content_map("the pet store"),
        case,
    )
    .await;
    create_snapshot(&gateway, &api, case, json!({ "documentationVersion": "1.0.0" })).await;
    let deployment = create_deployment(&gateway, &api, case).await;
    create_stage(
        &gateway,
        &api,
        case,
        json!({
            "stageName": "prod",
            "deploymentId": deployment,
            "documentationVersion": "1.0.0",
        }),
    )
    .await;

    for (case, parameters, documentation_expected, integration_expected) in cases {
        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "prod", dialect, None, parameters)
                .await
                .json(&case);
            let documentation = &document[DOCUMENTATION_EXTENSION];
            assert_eq!(
                !documentation.is_null(),
                documentation_expected,
                "case {case}: expected the documentation set present={documentation_expected}, \
                 got {documentation}"
            );
            if documentation_expected {
                assert_eq!(
                    parts_of(documentation, &case),
                    vec![expected_entry(&json!({ "type": "API" }), "the pet store")],
                    "case {case}: expected the snapshot's one entry, got {documentation}"
                );
            }
            // A combination asks for every set it names: carrying the documentation set
            // never costs the caller one of the others.
            let integration = &document["paths"]["/pets"]["get"]["x-amazon-apigateway-integration"];
            assert_eq!(
                !integration.is_null(),
                integration_expected,
                "case {case}: expected the integration extension present={integration_expected}, \
                 got {integration}"
            );
        }
    }
}

#[tokio::test]
async fn an_exported_documentation_set_imports_back_into_another_api() {
    // The round trip the extension exists for: what `GetExport` writes under the
    // documentation key is what `ImportDocumentationParts` reads back out of it, so an
    // exported snapshot can be imported into another API unchanged.
    let case = "export then import";
    let gateway = Gateway::new();
    let (source, _) = api_with_paths(&gateway, "round-trip", &[("/pets", &["GET"])]).await;
    let entries: Vec<(Value, &str)> = vec![
        (json!({ "type": "API" }), "the pet store"),
        (
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
            "list every pet",
        ),
        (
            json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
            "the pets answered",
        ),
    ];
    for (location, description) in &entries {
        created_part_id(
            &gateway,
            &source,
            location.clone(),
            &content_map(description),
            case,
        )
        .await;
    }
    create_snapshot(&gateway, &source, case, json!({ "documentationVersion": "1.0.0" })).await;
    let deployment = create_deployment(&gateway, &source, case).await;
    create_stage(
        &gateway,
        &source,
        case,
        json!({
            "stageName": "prod",
            "deploymentId": deployment,
            "documentationVersion": "1.0.0",
        }),
    )
    .await;

    for dialect in DIALECTS {
        let case = format!("{case} as {dialect}");
        let body = export(
            &gateway,
            &source,
            "prod",
            dialect,
            None,
            ASKS_FOR_DOCUMENTATION,
        )
        .await;
        let document = String::from_utf8(body.body.clone()).unwrap_or_else(|error| {
            panic!("case {case}: expected a text export, got {error}")
        });

        let (target, _) = create_api(&gateway, &format!("imported-{dialect}")).await;
        let answer = import_parts(&gateway, &target, Some("overwrite"), true, &document).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from ImportDocumentationParts of the exported set, got \
             {} with {:?}",
            answer.status, answer.body
        );
        let imported = crate::documentation_parts::listed_entities(&gateway, &target, &case).await;
        let expected: Vec<(Value, Value)> = entries
            .iter()
            .map(|(location, description)| {
                (location.clone(), json!({ "description": description }))
            })
            .collect();
        assert_eq!(
            imported, expected,
            "case {case}: expected the exported set to import unchanged, got {imported:?}"
        );
    }
}

#[tokio::test]
async fn exporting_a_documentation_set_twice_answers_the_same_set_and_changes_no_record() {
    // `GetExport` is a reporting operation: reading a stage's documentation set must not
    // change what a later caller sees — of the export or of the parts.
    let case = "two exports of one stage";
    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "reporting-docs", &[("/pets", &["GET"])]).await;
    let part = created_part_id(
        &gateway,
        &api,
        json!({ "type": "API" }),
        &content_map("the pet store"),
        case,
    )
    .await;
    create_snapshot(&gateway, &api, case, json!({ "documentationVersion": "1.0.0" })).await;
    let deployment = create_deployment(&gateway, &api, case).await;
    create_stage(
        &gateway,
        &api,
        case,
        json!({
            "stageName": "prod",
            "deploymentId": deployment,
            "documentationVersion": "1.0.0",
        }),
    )
    .await;

    for dialect in DIALECTS {
        let case = format!("{case} as {dialect}");
        let first =
            exported_documentation(&gateway, &api, "prod", dialect, ASKS_FOR_DOCUMENTATION, &case)
                .await;
        let second =
            exported_documentation(&gateway, &api, "prod", dialect, ASKS_FOR_DOCUMENTATION, &case)
                .await;
        assert_eq!(
            first, second,
            "case {case}: two exports of one stage answer the same documentation set, got \
             {first} then {second}"
        );
        let answer = get_part(&gateway, &api, &part).await;
        assert_eq!(
            string_member(&answer.body, "properties", &case),
            content_map("the pet store"),
            "case {case}: an export leaves the documentation parts as they were, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_extensions_value_outside_the_accepted_set_is_refused_naming_that_set() {
    // The refusal names the offending value, taken from the request, and the values the
    // parameter accepts — which now include the documentation set. Rows differ along the
    // shape of the bad value: a misspelling of an accepted one, a value in the wrong case
    // (the documented values are lower case), an unrelated word, and a bad value carried
    // beside a good one in a combination.
    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "bad-extensions", &[("/pets", &["GET"])]).await;
    crate::execute_api_data_plane::deploy(&gateway, &api, "prod").await;

    let cases: Vec<(&str, &str, &str)> = vec![
        ("a misspelling of an accepted value", "documentaton", "documentaton"),
        ("an accepted value in the wrong case", "Documentation", "Documentation"),
        ("an unrelated word", "schemas", "schemas"),
        (
            "a bad value beside a good one",
            "documentation,sdk",
            "sdk",
        ),
    ];

    for (case, requested, offending) in cases {
        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let answer = export(
                &gateway,
                &api,
                "prod",
                dialect,
                None,
                &[("extensions", requested)],
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected the modelled 400, got {} with {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("BadRequestException"),
                "case {case}: expected BadRequestException, got {:?}",
                answer.error
            );
            let message = String::from_utf8_lossy(&answer.body).to_string();
            assert!(
                message.contains(&format!("Invalid extensions specified: {offending}")),
                "case {case}: expected the offending value named, got {message}"
            );
            assert!(
                message.contains(
                    "extensions must be one of integrations, apigateway, authorizers, \
                     documentation, postman"
                ),
                "case {case}: expected the accepted values named, got {message}"
            );
        }
    }
}

#[tokio::test]
async fn a_stage_that_does_not_exist_is_not_found_whether_or_not_documentation_is_asked_for() {
    // The documentation set is rendered from the stage's pin, so a request naming no stage
    // is answered the same way with and without the parameter that asks for it.
    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "missing-stage", &[("/pets", &["GET"])]).await;
    crate::execute_api_data_plane::deploy(&gateway, &api, "prod").await;

    for parameters in [ASKS_FOR_DOCUMENTATION, &[][..]] {
        let case = format!("no such stage with parameters {parameters:?}");
        let answer = export(&gateway, &api, "nope", "swagger", None, parameters).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected the modelled 404, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {case}: expected NotFoundException, got {:?}",
            answer.error
        );
    }
}

/// A patch of a stage that names no such stage still fails, so the repin path the first
/// case uses cannot pass by accident: the helper asserts the modelled 200, and this is the
/// negative control for it.
#[tokio::test]
async fn repinning_a_stage_that_does_not_exist_is_refused() {
    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "repin-refusal", &[("/pets", &["GET"])]).await;
    let answer: Answer = patch(
        &gateway,
        &format!("/restapis/{api}/stages/nope"),
        vec![replace("/documentationVersion", "1.0.0")],
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case repin a missing stage: expected the modelled 404, got {} with {:?}",
        answer.status, answer.body
    );
}
