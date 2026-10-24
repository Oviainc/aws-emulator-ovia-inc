// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ImportRestApi`: creating a NEW API from an external API
//! definition, read back through `GetRestApi`, `GetRestApis`, `GetResources`,
//! `GetResource` and `GetMethod`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The tests assert the RULES over sets of inputs, not one recorded call:
//!
//! * every path of the posted definition (and every ancestor a path implies) becomes a
//!   `Resource` of the created API, parented to the resource of its parent path, and
//!   every operation on a path becomes a `Method` on that path's resource — the rows
//!   differ along the number of paths, their nesting, shared ancestors, the path forms
//!   API Gateway accepts, the verbs put on a path and the form the document is written
//!   in (JSON and the YAML of the same document);
//! * the created API's own members come from the document's `info`;
//! * the created API's endpoint type is the one the `parameters` map asks for, over the
//!   whole catalog of documented values, their default and a value outside it;
//! * the document's base path reaches the imported paths as the `basepath` parameter
//!   asks, over that key's documented values and the two ways a document declares a base
//!   path;
//! * a refused import — a malformed definition, an unsupported parameter value, a
//!   definition that names no API, or `failOnWarnings` with a warning to report — creates
//!   NO API.

use serde_json::Value;

use crate::rest_api_lifecycle::{
    assert_failure, page_items, resource_paths, string_member, Answer, Gateway,
};

/// The HTTP verbs an imported operation can become, as the keys an OpenAPI Path Item
/// Object uses for them (`x-amazon-apigateway-any-method` is API Gateway's own extension
/// for the `ANY` catch-all).
const OPERATION_KEYS: &[(&str, &str)] = &[
    ("get", "GET"),
    ("put", "PUT"),
    ("post", "POST"),
    ("delete", "DELETE"),
    ("options", "OPTIONS"),
    ("head", "HEAD"),
    ("patch", "PATCH"),
    ("x-amazon-apigateway-any-method", "ANY"),
];

/// The endpoint types the `endpointConfigurationTypes` parameter documents, and the
/// default the member names ("The default endpoint type is `EDGE`").
const ENDPOINT_TYPES: &[&str] = &["EDGE", "REGIONAL", "PRIVATE"];
const DEFAULT_ENDPOINT_TYPE: &str = "EDGE";

/// One path of a definition: the path the document declares, and the operation keys the
/// document puts on it.
type PathRow<'a> = (&'a str, &'a [&'a str]);

/// One row of the metadata table: the case's name, the definition, and the members the
/// created API must report for it.
type MetadataCase<'a> = (&'a str, &'a str, &'a [(&'a str, Value)]);

/// The OpenAPI definition text for `paths`, as JSON.
///
/// Written out by hand rather than through `serde_json::Map`, which SORTS its keys: the
/// document's own key order is what the import walks.
fn json_definition(title: &str, paths: &[PathRow]) -> String {
    let rendered: Vec<String> = paths
        .iter()
        .map(|(path, methods)| {
            let operations: Vec<String> = methods
                .iter()
                .map(|method| {
                    format!(
                        "\"{method}\": {{\"responses\": {{\"200\": {{\"description\": \"ok\"}}}}}}"
                    )
                })
                .collect();
            format!("    \"{path}\": {{{}}}", operations.join(", "))
        })
        .collect();
    format!(
        "{{\n  \"openapi\": \"3.0.1\",\n  \"info\": {{\"title\": \"{title}\", \"version\": \"2026-01-01\"}},\n  \"paths\": {{\n{}\n  }}\n}}\n",
        rendered.join(",\n")
    )
}

/// The same definition as [`json_definition`], written in YAML block style — the other
/// form the `body` member documents ("only OpenAPI definition JSON/YAML files are
/// supported").
fn yaml_definition(title: &str, paths: &[PathRow]) -> String {
    let mut text =
        format!("openapi: 3.0.1\ninfo:\n  title: {title}\n  version: '2026-01-01'\npaths:\n");
    for (path, methods) in paths {
        if methods.is_empty() {
            text.push_str(&format!("  \"{path}\": {{}}\n"));
            continue;
        }
        text.push_str(&format!("  \"{path}\":\n"));
        for method in *methods {
            text.push_str(&format!(
                "    {method}:\n      responses:\n        '200':\n          description: ok\n"
            ));
        }
    }
    text
}

/// `ImportRestApi`: the operation's own route (`POST /restapis?mode=import`), with `body`
/// sent verbatim and the request's `parameters` carried in the query string.
async fn import(
    gateway: &Gateway,
    body: &str,
    fail_on_warnings: bool,
    parameters: &[(&str, &str)],
) -> Answer {
    let mut query = vec!["mode=import".to_string()];
    if fail_on_warnings {
        query.push("failonwarnings=true".to_string());
    }
    for (key, value) in parameters {
        query.push(format!("{key}={value}"));
    }
    gateway
        .call_raw("POST", &format!("/restapis?{}", query.join("&")), body)
        .await
}

/// `ImportRestApi`, asserting the modelled 201 and returning the created `RestApi` body.
async fn import_created(gateway: &Gateway, case: &str, body: &str) -> Value {
    let answer = import(gateway, body, false, &[]).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: ImportRestApi expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// How many APIs the account holds.
async fn api_count(gateway: &Gateway, case: &str) -> usize {
    let answer = gateway.call("GET", "/restapis", None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetRestApis expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case).len()
}

/// Every listed resource of `api`, keyed by its full path.
async fn resources_by_path(gateway: &Gateway, api: &str, case: &str) -> Vec<(String, Value)> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/resources"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetResources expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .into_iter()
        .map(|item| (string_member(&item, "path", case), item))
        .collect()
}

/// The verbs of a resource's `resourceMethods`, SORTED: the map is a JSON object, so the
/// order its members are serialised in is not part of what the service reports.
fn method_keys(resource: &Value) -> Vec<String> {
    let mut keys: Vec<String> = resource["resourceMethods"]
        .as_object()
        .map(|methods| methods.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

#[tokio::test]
async fn an_import_creates_an_api_whose_tree_is_the_definitions_paths() {
    // Rows differ along what the rule generalises over: how many paths the document
    // declares, how deeply they nest, whether they share an ancestor the document never
    // declares on its own, the path forms API Gateway accepts (a path parameter, a greedy
    // proxy, a path that is only the root, a trailing separator), and the verbs put on a
    // path.
    let all_keys: Vec<&str> = OPERATION_KEYS.iter().map(|(key, _)| *key).collect();
    let cases: &[(&str, &[PathRow], &[&str])] = &[
        ("one flat path", &[("/pets", &["get"])], &["/", "/pets"]),
        (
            "two flat paths in document order",
            &[("/pets", &["get"]), ("/items", &["post"])],
            &["/", "/pets", "/items"],
        ),
        (
            "a path parameter under an undeclared ancestor",
            &[("/pets/{petId}", &["get"])],
            &["/", "/pets", "/pets/{petId}"],
        ),
        (
            "three levels of nesting",
            &[("/a/b/c", &["delete"])],
            &["/", "/a", "/a/b", "/a/b/c"],
        ),
        (
            "a shared ancestor is created once",
            &[
                ("/pets", &["get"]),
                ("/pets/{petId}", &["get"]),
                ("/pets/{petId}/toys", &["get"]),
            ],
            &["/", "/pets", "/pets/{petId}", "/pets/{petId}/toys"],
        ),
        (
            "the root path adds no resource",
            &[("/", &["get"])],
            &["/"],
        ),
        (
            "a greedy proxy path",
            &[("/{proxy+}", &["x-amazon-apigateway-any-method"])],
            &["/", "/{proxy+}"],
        ),
        (
            "a trailing separator names the same resource",
            &[("/pets/", &["head"])],
            &["/", "/pets"],
        ),
        (
            "a path with no operations is still a resource",
            &[("/pets", &[])],
            &["/", "/pets"],
        ),
        (
            "every operation key on one path",
            &[("/pets", &all_keys)],
            &["/", "/pets"],
        ),
    ];

    let gateway = Gateway::new();
    for (case, paths, expected) in cases {
        let created = import_created(&gateway, case, &json_definition(case, paths)).await;
        let api = string_member(&created, "id", case);
        let root = string_member(&created, "rootResourceId", case);

        let listed = resource_paths(&gateway, &api).await;
        assert_eq!(
            listed,
            expected
                .iter()
                .map(|path| (*path).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected the imported paths {expected:?}, got {listed:?}"
        );

        // Round trip: the created API is a readable record, and it reports the root
        // resource the import answered with.
        let read = gateway.call("GET", &format!("/restapis/{api}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case}: GetRestApi expected 200, got {} with {:?}",
            read.status, read.body
        );
        for member in ["id", "name", "version", "rootResourceId", "apiStatus"] {
            assert_eq!(
                read.body[member], created[member],
                "case {case}: GetRestApi must report the persisted {member}: expected {:?}, got {:?}",
                created[member], read.body[member]
            );
        }

        // Each imported resource is parented to the resource of its parent path and
        // carries its own last segment — the same record `CreateResource` would build.
        let resources = resources_by_path(&gateway, &api, case).await;
        for (path, item) in &resources {
            if path == "/" {
                assert_eq!(
                    item["id"].as_str(),
                    Some(root.as_str()),
                    "case {case}: the root resource keeps the created id {root:?}, got {item:?}"
                );
                continue;
            }
            let (parent_path, path_part) = path
                .rsplit_once('/')
                .map(|(head, tail)| (if head.is_empty() { "/" } else { head }, tail))
                .unwrap_or(("/", path.as_str()));
            let parent_id = resources
                .iter()
                .find(|(candidate, _)| candidate == parent_path)
                .map(|(_, parent)| string_member(parent, "id", case))
                .unwrap_or_else(|| panic!("case {case}: no resource for parent {parent_path:?}"));
            assert_eq!(
                item["parentId"].as_str(),
                Some(parent_id.as_str()),
                "case {case} at {path:?}: expected parentId {parent_id:?}, got {:?}",
                item["parentId"]
            );
            assert_eq!(
                item["pathPart"].as_str(),
                Some(path_part),
                "case {case} at {path:?}: expected pathPart {path_part:?}, got {:?}",
                item["pathPart"]
            );
        }

        // Every operation the definition declares on a path is a `Method` on that path's
        // resource, readable through `GetMethod` and open, because an imported operation
        // that declares no security requirement is.
        for (path, keys) in *paths {
            let normalized = {
                let mut normalized = String::new();
                for segment in path.split('/').filter(|segment| !segment.is_empty()) {
                    normalized.push('/');
                    normalized.push_str(segment);
                }
                if normalized.is_empty() {
                    "/".to_string()
                } else {
                    normalized
                }
            };
            let (_, resource) = resources
                .iter()
                .find(|(candidate, _)| *candidate == normalized)
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a resource for {normalized:?}, got {resources:?}")
                });
            let resource_id = string_member(resource, "id", case);
            let mut expected_verbs: Vec<String> = keys
                .iter()
                .map(|key| {
                    OPERATION_KEYS
                        .iter()
                        .find(|(candidate, _)| candidate == key)
                        .map(|(_, verb)| (*verb).to_string())
                        .unwrap_or_else(|| panic!("case {case}: {key:?} is not an operation key"))
                })
                .collect();
            expected_verbs.sort();
            let reported = method_keys(resource);
            assert_eq!(
                reported, expected_verbs,
                "case {case} at {normalized:?}: expected exactly the imported methods {expected_verbs:?}, got {reported:?}"
            );
            for verb in &expected_verbs {
                let method = gateway
                    .call(
                        "GET",
                        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
                        None,
                    )
                    .await;
                assert_eq!(
                    method.status, 200,
                    "case {case}: GetMethod({verb}) expected 200, got {} with {:?}",
                    method.status, method.body
                );
                assert_eq!(
                    method.body["authorizationType"].as_str(),
                    Some("NONE"),
                    "case {case}: GetMethod({verb}) expected authorizationType NONE, got {:?}",
                    method.body["authorizationType"]
                );
            }
        }
    }
}

#[tokio::test]
async fn the_same_definition_in_yaml_creates_the_same_api_as_in_json() {
    // The rule is about the definition, not the form it is written in: each row is
    // imported twice, once from JSON and once from the YAML of the same document, and the
    // two created APIs must carry the same tree and the same metadata.
    let cases: &[&[PathRow]] = &[
        &[("/pets", &["get"])],
        &[("/pets", &["get", "post"]), ("/items", &["delete"])],
        &[("/pets/{petId}/toys", &["get"])],
        &[("/{proxy+}", &["x-amazon-apigateway-any-method"])],
    ];

    let gateway = Gateway::new();
    for paths in cases {
        let case = format!("{paths:?}");
        let from_json = import_created(&gateway, &case, &json_definition("same", paths)).await;
        let from_yaml = import_created(&gateway, &case, &yaml_definition("same", paths)).await;
        for member in ["name", "version"] {
            assert_eq!(
                from_json[member], from_yaml[member],
                "case {case}: the YAML form must report the same {member}: expected {:?}, got {:?}",
                from_json[member], from_yaml[member]
            );
        }

        let json_api = string_member(&from_json, "id", &case);
        let yaml_api = string_member(&from_yaml, "id", &case);
        assert_ne!(
            json_api, yaml_api,
            "case {case}: each import creates its own API, got {json_api:?} twice"
        );

        let json_tree = resources_by_path(&gateway, &json_api, &case).await;
        let yaml_tree = resources_by_path(&gateway, &yaml_api, &case).await;
        let json_paths: Vec<String> = json_tree.iter().map(|(path, _)| path.clone()).collect();
        let yaml_paths: Vec<String> = yaml_tree.iter().map(|(path, _)| path.clone()).collect();
        assert_eq!(
            json_paths, yaml_paths,
            "case {case}: the YAML form must import the same paths: expected {json_paths:?}, got {yaml_paths:?}"
        );
        for ((path, json_item), (_, yaml_item)) in json_tree.iter().zip(yaml_tree.iter()) {
            let expected = method_keys(json_item);
            let reported = method_keys(yaml_item);
            assert_eq!(
                expected, reported,
                "case {case} at {path:?}: expected the methods {expected:?}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_created_api_takes_its_metadata_from_the_definitions_info() {
    // Rows differ along which of the document's describing members are present: `info` is
    // where an OpenAPI document carries the API's name, description and version, and
    // `x-amazon-apigateway-binary-media-types` is API Gateway's own extension for the
    // API's binary media types. A member the document leaves out leaves the created API
    // without one rather than with some other API's value.
    let cases: &[MetadataCase] = &[
        (
            "every describing member",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"full\", \
             \"description\": \"from the definition\", \"version\": \"2026-02-02\"}, \
             \"x-amazon-apigateway-binary-media-types\": [\"image/png\", \"image/jpeg\"], \
             \"paths\": {\"/pets\": {\"get\": {}}}}",
            &[
                ("name", Value::String("full".to_string())),
                (
                    "description",
                    Value::String("from the definition".to_string()),
                ),
                ("version", Value::String("2026-02-02".to_string())),
                (
                    "binaryMediaTypes",
                    Value::Array(vec![
                        Value::String("image/png".to_string()),
                        Value::String("image/jpeg".to_string()),
                    ]),
                ),
            ],
        ),
        (
            "a title only",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"titled\"}, \"paths\": {}}",
            &[
                ("name", Value::String("titled".to_string())),
                ("description", Value::Null),
                ("version", Value::Null),
                ("binaryMediaTypes", Value::Null),
            ],
        ),
        (
            "a swagger 2.0 document",
            "{\"swagger\": \"2.0\", \"info\": {\"title\": \"legacy\", \"version\": \"1\"}, \
             \"paths\": {\"/pets\": {\"get\": {}}}}",
            &[
                ("name", Value::String("legacy".to_string())),
                ("version", Value::String("1".to_string())),
            ],
        ),
    ];

    let gateway = Gateway::new();
    for (case, body, expected) in cases {
        let created = import_created(&gateway, case, body).await;
        let api = string_member(&created, "id", case);
        for (member, value) in *expected {
            assert_eq!(
                created[*member], *value,
                "case {case}: expected {member} {value:?}, got {:?}",
                created[*member]
            );
        }
        // Round trip: the reader reports the same record the import answered.
        let read = gateway.call("GET", &format!("/restapis/{api}"), None).await;
        for (member, _) in *expected {
            assert_eq!(
                read.body[*member], created[*member],
                "case {case}: GetRestApi must report the persisted {member}: expected {:?}, got {:?}",
                created[*member], read.body[*member]
            );
        }
        // A created API is immediately usable and carries no warning of its own.
        assert_eq!(
            created["apiStatus"].as_str(),
            Some("AVAILABLE"),
            "case {case}: expected apiStatus AVAILABLE, got {:?}",
            created["apiStatus"]
        );
        assert!(
            created["warnings"].is_null(),
            "case {case}: a definition the import carried over in full reports no warnings, got {:?}",
            created["warnings"]
        );
    }
}

#[tokio::test]
async fn the_created_apis_endpoint_type_is_the_one_the_parameters_ask_for() {
    // The catalog row per documented value, their default, several types at once, and a
    // value outside the catalog. `ENDPOINT_TYPES` is the member's own list ("set
    // `parameters` as `endpointConfigurationTypes=EDGE`, `=REGIONAL`, or `=PRIVATE`"),
    // tested here on every entry, `PRIVATE` included, which the issue cited none of.
    let gateway = Gateway::new();
    let definition = json_definition("endpoints", &[("/pets", &["get"])]);

    for requested in ENDPOINT_TYPES {
        let case = format!("endpointConfigurationTypes={requested}");
        let answer = import(
            &gateway,
            &definition,
            false,
            &[("endpointConfigurationTypes", requested)],
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["endpointConfiguration"]["types"],
            serde_json::json!([requested]),
            "case {case}: expected the endpoint types [{requested:?}], got {:?}",
            answer.body["endpointConfiguration"]
        );
        // Round trip: the stored configuration is what a later read reports.
        let api = string_member(&answer.body, "id", &case);
        let read = gateway.call("GET", &format!("/restapis/{api}"), None).await;
        assert_eq!(
            read.body["endpointConfiguration"], answer.body["endpointConfiguration"],
            "case {case}: GetRestApi must report the persisted endpointConfiguration: expected {:?}, got {:?}",
            answer.body["endpointConfiguration"], read.body["endpointConfiguration"]
        );
    }

    // No `endpointConfigurationTypes` parameter: the documented default endpoint type.
    let defaulted = import_created(&gateway, "no endpoint parameter", &definition).await;
    assert_eq!(
        defaulted["endpointConfiguration"]["types"],
        serde_json::json!([DEFAULT_ENDPOINT_TYPE]),
        "case no endpoint parameter: expected the default endpoint type, got {:?}",
        defaulted["endpointConfiguration"]
    );

    // Several endpoint types at once, as a repeated parameter reaches the member.
    let several = import(
        &gateway,
        &definition,
        false,
        &[("endpointConfigurationTypes", "EDGE,REGIONAL")],
    )
    .await;
    assert_eq!(
        several.status, 201,
        "case two endpoint types: expected 201, got {} with {:?}",
        several.status, several.body
    );
    assert_eq!(
        several.body["endpointConfiguration"]["types"],
        serde_json::json!(["EDGE", "REGIONAL"]),
        "case two endpoint types: expected both types, got {:?}",
        several.body["endpointConfiguration"]
    );

    // A value outside the documented catalog is refused, and no API is created for it.
    let before = api_count(&gateway, "unsupported endpoint type").await;
    for value in ["edge", "LOCAL", ""] {
        let case = format!("endpointConfigurationTypes={value:?}");
        let answer = import(
            &gateway,
            &definition,
            false,
            &[("endpointConfigurationTypes", value)],
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let after = api_count(&gateway, &case).await;
        assert_eq!(
            after, before,
            "case {case}: a refused import must create no API, expected {before} APIs, got {after}"
        );
    }
}

#[tokio::test]
async fn the_definitions_base_path_reaches_the_imported_paths_as_basepath_asks() {
    // A base path is declared by Swagger 2.0's own `basePath` member and, from OpenAPI 3
    // on, by the path of the first `servers` entry's URL — absolute or relative. Each row
    // declares `/v1/beta` one of those three ways and is imported under each documented
    // value of the `basepath` parameter, plus the absent key.
    let declarations: &[(&str, &str)] = &[
        (
            "swagger basePath",
            "{\"swagger\": \"2.0\", \"info\": {\"title\": \"based\"}, \"basePath\": \"/v1/beta\", \
             \"paths\": {\"/pets\": {\"get\": {}}}}",
        ),
        (
            "an absolute server url",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"based\"}, \
             \"servers\": [{\"url\": \"https://example.test/v1/beta\"}], \
             \"paths\": {\"/pets\": {\"get\": {}}}}",
        ),
        (
            "a relative server url",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"based\"}, \
             \"servers\": [{\"url\": \"/v1/beta\"}], \
             \"paths\": {\"/pets\": {\"get\": {}}}}",
        ),
    ];
    // The paths each documented value of the key must leave behind: `ignore` (and the
    // absent key, which defaults to it) drops the base path, `prepend` carries all of it,
    // and `split` carries everything after its leading segment.
    let modes: &[(Option<&str>, &[&str])] = &[
        (None, &["/", "/pets"]),
        (Some("ignore"), &["/", "/pets"]),
        (Some("prepend"), &["/", "/v1", "/v1/beta", "/v1/beta/pets"]),
        (Some("split"), &["/", "/beta", "/beta/pets"]),
    ];

    let gateway = Gateway::new();
    for (declaration, body) in declarations {
        for (mode, expected) in modes {
            let case = format!("{declaration} with basepath {mode:?}");
            let parameters: Vec<(&str, &str)> = mode
                .map(|mode| vec![("basepath", mode)])
                .unwrap_or_default();
            let answer = import(&gateway, body, false, &parameters).await;
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201, got {} with {:?}",
                answer.status, answer.body
            );
            let api = string_member(&answer.body, "id", &case);
            let listed = resource_paths(&gateway, &api).await;
            let mut sorted = listed.clone();
            sorted.sort();
            let mut wanted: Vec<String> =
                expected.iter().map(|path| (*path).to_string()).collect();
            wanted.sort();
            assert_eq!(
                sorted, wanted,
                "case {case}: expected the resources {wanted:?}, got {listed:?}"
            );
        }
    }

    // A document that declares NO base path imports the same tree under every value of
    // the key: there is nothing for it to apply.
    let unbased = json_definition("unbased", &[("/pets", &["get"])]);
    for mode in ["ignore", "prepend", "split"] {
        let case = format!("no base path with basepath {mode}");
        let answer = import(&gateway, &unbased, false, &[("basepath", mode)]).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        let api = string_member(&answer.body, "id", &case);
        let listed = resource_paths(&gateway, &api).await;
        assert_eq!(
            listed,
            vec!["/".to_string(), "/pets".to_string()],
            "case {case}: expected the declared paths, got {listed:?}"
        );
    }

    // A value outside the documented catalog is refused, and creates no API.
    let before = api_count(&gateway, "unsupported basepath").await;
    for value in ["Ignore", "keep", ""] {
        let case = format!("basepath={value:?}");
        let answer = import(&gateway, &unbased, false, &[("basepath", value)]).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let after = api_count(&gateway, &case).await;
        assert_eq!(
            after, before,
            "case {case}: a refused import must create no API, expected {before} APIs, got {after}"
        );
    }
}

#[tokio::test]
async fn fail_on_warnings_refuses_the_import_and_leaves_no_api_behind() {
    // The operation keys whose verb API Gateway has no method for: each is reported as a
    // warning and the operation is skipped.
    let cases: &[(&str, &[PathRow])] = &[
        ("one unsupported operation", &[("/pets", &["get", "trace"])]),
        (
            "an unsupported operation on each of two paths",
            &[("/pets", &["trace"]), ("/items", &["get", "connect"])],
        ),
    ];

    let gateway = Gateway::new();
    for (case, paths) in cases {
        let definition = json_definition(case, paths);

        // Without `failonwarnings` the import is served, the unsupported operations are
        // skipped, and the created API reports what was not carried over.
        let created = import_created(&gateway, case, &definition).await;
        let warnings = created["warnings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected warnings, got {created:?}"));
        assert!(
            !warnings.is_empty(),
            "case {case}: expected a warning per skipped operation, got {warnings:?}"
        );
        let api = string_member(&created, "id", case);
        let resources = resources_by_path(&gateway, &api, case).await;
        for (path, keys) in *paths {
            let (_, resource) = resources
                .iter()
                .find(|(candidate, _)| candidate == path)
                .unwrap_or_else(|| panic!("case {case}: expected a resource for {path:?}"));
            let expected: Vec<String> = keys
                .iter()
                .filter_map(|key| {
                    OPERATION_KEYS
                        .iter()
                        .find(|(candidate, _)| candidate == key)
                        .map(|(_, verb)| (*verb).to_string())
                })
                .collect();
            let reported = method_keys(resource);
            assert_eq!(
                reported, expected,
                "case {case} at {path:?}: only the supported operations are imported: expected {expected:?}, got {reported:?}"
            );
        }

        // With `failonwarnings=true` the creation is rolled back: the request is refused
        // and NO API is left behind.
        let before = api_count(&gateway, case).await;
        let answer = import(&gateway, &definition, true, &[]).await;
        assert_failure(
            &answer,
            400,
            "BadRequestException",
            &format!("{case} with failonwarnings"),
        );
        let after = api_count(&gateway, case).await;
        assert_eq!(
            after, before,
            "case {case} with failonwarnings: a refused import must create no API, expected {before} APIs, got {after}"
        );
    }

    // A definition the import carries over in full is served with `failonwarnings=true`:
    // the parameter refuses a warning, not an import.
    let clean = json_definition("clean", &[("/pets", &["get"])]);
    let answer = import(&gateway, &clean, true, &[]).await;
    assert_eq!(
        answer.status, 201,
        "case failonwarnings with no warning: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body["warnings"].is_null(),
        "case failonwarnings with no warning: expected no warnings, got {:?}",
        answer.body["warnings"]
    );
}

#[tokio::test]
async fn an_operations_integration_extension_becomes_the_methods_integration() {
    // The rule: everything of an operation that a `Method` has a member for is carried
    // over — its `operationId`, its declared `parameters`, its `responses`, and API
    // Gateway's integration extension (with the pseudo parameters of its ARNs resolved
    // against the account and Region the API is imported into). The rows differ along the
    // integration types the extension declares and what each one carries.
    let region = "us-east-1";
    let account = engine::emulator_account_id();
    let lambda = "arn:aws:apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/\
                  arn:${AWS::Partition}:lambda:${AWS::Region}:${AWS::AccountId}:function:handler/invocations"
        .to_string();
    let resolved_lambda = format!(
        "arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:{region}:{account}:function:handler/invocations"
    );

    let definition = format!(
        "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"integrated\"}}, \"paths\": {{\
           \"/mock\": {{\"get\": {{\
             \"operationId\": \"readMock\", \
             \"parameters\": [{{\"name\": \"petId\", \"in\": \"query\", \"required\": true}}, \
                              {{\"name\": \"x-trace\", \"in\": \"header\"}}], \
             \"responses\": {{\"200\": {{\"description\": \"ok\", \
                 \"headers\": {{\"x-count\": {{\"schema\": {{\"type\": \"string\"}}}}}}}}}}, \
             \"x-amazon-apigateway-integration\": {{\"type\": \"mock\", \
                 \"requestTemplates\": {{\"application/json\": \"{{\\\"region\\\":\\\"${{AWS::Region}}\\\"}}\"}}, \
                 \"responses\": {{\"default\": {{\"statusCode\": \"200\", \
                     \"responseTemplates\": {{\"application/json\": \"{{\\\"in\\\":\\\"${{AWS::Region}}\\\"}}\"}}}}, \
                     \"4\\\\d{{2}}\": {{\"statusCode\": \"400\"}}}}}}}}}}, \
           \"/lambda\": {{\"post\": {{\
             \"responses\": {{\"200\": {{\"description\": \"ok\"}}}}, \
             \"x-amazon-apigateway-integration\": {{\"type\": \"aws_proxy\", \
                 \"httpMethod\": \"post\", \"uri\": \"{lambda}\", \
                 \"credentials\": \"arn:${{AWS::Partition}}:iam::${{AWS::AccountId}}:role/invoke\", \
                 \"timeoutInMillis\": 5000, \"passthroughBehavior\": \"when_no_match\"}}}}}}\
         }}}}"
    );

    let gateway = Gateway::new();
    let created = import_created(&gateway, "integrations", &definition).await;
    let api = string_member(&created, "id", "integrations");
    let resources = resources_by_path(&gateway, &api, "integrations").await;

    let resource_id = |path: &str| {
        let (_, resource) = resources
            .iter()
            .find(|(candidate, _)| candidate == path)
            .unwrap_or_else(|| panic!("case integrations: expected a resource for {path:?}"));
        string_member(resource, "id", "integrations")
    };

    // The MOCK row: the integration, its request template with the Region resolved, and
    // its integration responses keyed by the status code each one declares — the entry
    // the extension keys `default` carrying no selection pattern of its own.
    let mock = gateway
        .call(
            "GET",
            &format!(
                "/restapis/{api}/resources/{}/methods/GET/integration",
                resource_id("/mock")
            ),
            None,
        )
        .await;
    assert_eq!(
        mock.status, 200,
        "case mock: GetIntegration expected 200, got {} with {:?}",
        mock.status, mock.body
    );
    assert_eq!(
        mock.body["type"].as_str(),
        Some("MOCK"),
        "case mock: expected the modelled spelling of the declared type, got {:?}",
        mock.body["type"]
    );
    assert_eq!(
        mock.body["requestTemplates"]["application/json"].as_str(),
        Some(format!("{{\"region\":\"{region}\"}}").as_str()),
        "case mock: expected the request template's pseudo parameter resolved, got {:?}",
        mock.body["requestTemplates"]
    );
    assert_eq!(
        mock.body["integrationResponses"]["200"]["responseTemplates"]["application/json"].as_str(),
        Some(format!("{{\"in\":\"{region}\"}}").as_str()),
        "case mock: expected the response template's pseudo parameter resolved, got {:?}",
        mock.body["integrationResponses"]["200"]
    );
    assert!(
        mock.body["integrationResponses"]["200"]["selectionPattern"].is_null(),
        "case mock: the default integration response matches everything the others do not, got {:?}",
        mock.body["integrationResponses"]["200"]["selectionPattern"]
    );
    assert_eq!(
        mock.body["integrationResponses"]["400"]["selectionPattern"].as_str(),
        Some("4\\d{2}"),
        "case mock: expected the declared selection pattern, got {:?}",
        mock.body["integrationResponses"]["400"]
    );
    // The method the integration hangs off carries the operation's own members.
    let method = gateway
        .call(
            "GET",
            &format!(
                "/restapis/{api}/resources/{}/methods/GET",
                resource_id("/mock")
            ),
            None,
        )
        .await;
    assert_eq!(
        method.body["operationName"].as_str(),
        Some("readMock"),
        "case mock: expected the operationId as the method's operationName, got {:?}",
        method.body["operationName"]
    );
    assert_eq!(
        method.body["requestParameters"],
        serde_json::json!({
            "method.request.querystring.petId": true,
            "method.request.header.x-trace": false,
        }),
        "case mock: expected the declared parameters as request parameters, got {:?}",
        method.body["requestParameters"]
    );
    assert_eq!(
        method.body["methodResponses"]["200"]["statusCode"].as_str(),
        Some("200"),
        "case mock: expected the declared response as a method response, got {:?}",
        method.body["methodResponses"]
    );
    assert_eq!(
        method.body["methodResponses"]["200"]["responseParameters"],
        serde_json::json!({ "method.response.header.x-count": false }),
        "case mock: expected the declared response header as a response parameter, got {:?}",
        method.body["methodResponses"]["200"]
    );

    // The AWS_PROXY row: the ARN members with every pseudo parameter resolved, and the
    // members the extension set kept as given.
    let proxy = gateway
        .call(
            "GET",
            &format!(
                "/restapis/{api}/resources/{}/methods/POST/integration",
                resource_id("/lambda")
            ),
            None,
        )
        .await;
    assert_eq!(
        proxy.status, 200,
        "case aws_proxy: GetIntegration expected 200, got {} with {:?}",
        proxy.status, proxy.body
    );
    for (member, expected) in [
        ("type", "AWS_PROXY".to_string()),
        ("httpMethod", "POST".to_string()),
        ("uri", resolved_lambda.clone()),
        (
            "credentials",
            format!("arn:aws:iam::{account}:role/invoke"),
        ),
        ("passthroughBehavior", "WHEN_NO_MATCH".to_string()),
    ] {
        assert_eq!(
            proxy.body[member].as_str(),
            Some(expected.as_str()),
            "case aws_proxy: expected {member} {expected:?}, got {:?}",
            proxy.body[member]
        );
    }
    assert_eq!(
        proxy.body["timeoutInMillis"].as_i64(),
        Some(5_000),
        "case aws_proxy: expected the declared timeout, got {:?}",
        proxy.body["timeoutInMillis"]
    );
    // "By default, API Gateway uses the resource ID as the cacheNamespace", which is the
    // default the operation that puts an integration applies too.
    assert_eq!(
        proxy.body["cacheNamespace"].as_str(),
        Some(resource_id("/lambda").as_str()),
        "case aws_proxy: expected the resource id as the cacheNamespace, got {:?}",
        proxy.body["cacheNamespace"]
    );
}

#[tokio::test]
async fn a_definition_the_import_cannot_carry_over_in_full_reports_what_it_dropped() {
    // Rows differ along WHAT the import could not carry over, and each row's warning names
    // the part of the document it is about. The security rows cover both spellings of a
    // declared scheme set — Swagger 2.0's `securityDefinitions` and OpenAPI 3's
    // `components.securitySchemes` — and both places a security requirement is declared:
    // on the operation, and on the document, which every operation that declares none of
    // its own is imported under.
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "a security scheme type no authorizer covers",
            "{\"swagger\": \"2.0\", \"info\": {\"title\": \"w\"}, \
             \"securityDefinitions\": {\"myAuth\": {\"type\": \"basic\"}}, \
             \"paths\": {\"/thing\": {\"get\": {\"responses\": {\"200\": {\"description\": \"ok\"}}}}}}",
            &["myAuth", "basic"],
        ),
        (
            "an operation that specifies security",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
             \"components\": {\"securitySchemes\": {\"myAuth\": {\"type\": \"apiKey\", \
                 \"name\": \"x-key\", \"in\": \"header\"}}}, \
             \"paths\": {\"/thing\": {\"get\": {\"security\": [{\"myAuth\": []}], \
                 \"responses\": {\"200\": {\"description\": \"ok\"}}}}}}",
            &["GET", "/thing", "security"],
        ),
        (
            "a document that specifies security across its API",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
             \"components\": {\"securitySchemes\": {\"myAuth\": {\"type\": \"oauth2\", \
                 \"flows\": {}}}}, \
             \"security\": [{\"myAuth\": []}], \
             \"paths\": {\"/thing\": {\"get\": {\"responses\": {\"200\": {\"description\": \"ok\"}}}}}}",
            &["GET", "/thing", "security"],
        ),
        (
            "an operation API Gateway has no method for",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
             \"paths\": {\"/thing\": {\"trace\": {\"responses\": {}}}}}",
            &["trace", "/thing"],
        ),
        (
            "an integration the service would refuse",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
             \"paths\": {\"/thing\": {\"get\": {\"responses\": {}, \
                 \"x-amazon-apigateway-integration\": {\"type\": \"http\", \
                     \"httpMethod\": \"get\", \"uri\": \"not-a-url\"}}}}}",
            &["GET", "/thing"],
        ),
        (
            "a reference the document does not declare",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
             \"components\": {\"schemas\": {\"Thing\": {\"type\": \"object\"}}}, \
             \"paths\": {\"/thing\": {\"get\": {\"responses\": {\"200\": {\"description\": \"ok\", \
                 \"content\": {\"application/json\": {\"schema\": \
                     {\"$ref\": \"#/components/schemas/Missing\"}}}}}}}}}",
            &["#/components/schemas/Missing"],
        ),
        (
            "a reference to another document",
            "{\"swagger\": \"2.0\", \"info\": {\"title\": \"w\"}, \
             \"paths\": {\"/thing\": {\"get\": {\"responses\": {\"200\": {\"description\": \"ok\", \
                 \"schema\": {\"$ref\": \"shared.json#/definitions/Thing\"}}}}}}}",
            &["shared.json#/definitions/Thing"],
        ),
    ];

    let gateway = Gateway::new();
    for (case, body, named) in cases {
        // Without `failonwarnings` the import is served and reports what it dropped.
        let created = import_created(&gateway, case, body).await;
        let warnings = created["warnings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected warnings, got {created:?}"));
        assert_eq!(
            warnings.len(),
            1,
            "case {case}: expected exactly one warning, got {warnings:?}"
        );
        let warning = warnings[0].as_str().unwrap_or_default();
        for part in *named {
            assert!(
                warning.contains(part),
                "case {case}: the warning must name {part:?}, got {warning:?}"
            );
        }

        // And with it the import is refused, leaving no API behind.
        let before = api_count(&gateway, case).await;
        let answer = import(&gateway, body, true, &[]).await;
        assert_failure(
            &answer,
            400,
            "BadRequestException",
            &format!("{case} with failonwarnings"),
        );
        let after = api_count(&gateway, case).await;
        assert_eq!(
            after, before,
            "case {case} with failonwarnings: a refused import must create no API, expected {before} APIs, got {after}"
        );
    }

    // An operation that overrides the document's requirement with an EMPTY one specifies
    // no security, so there is nothing to report: that is the OpenAPI rule for `security`.
    let overridden = import_created(
        &gateway,
        "an operation that overrides the document's security",
        "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
         \"components\": {\"securitySchemes\": {\"myAuth\": {\"type\": \"apiKey\", \
             \"name\": \"x-key\", \"in\": \"header\"}}}, \
         \"security\": [{\"myAuth\": []}], \
         \"paths\": {\"/thing\": {\"get\": {\"security\": [], \
             \"responses\": {\"200\": {\"description\": \"ok\"}}}}}}",
    )
    .await;
    assert!(
        overridden["warnings"].is_null(),
        "case an operation that overrides the document's security: expected no warnings, got {:?}",
        overridden["warnings"]
    );

    // A reference the document DOES declare resolves, so there is nothing to report
    // either — including one whose path tokens spell a separator (`~1`) the way a JSON
    // pointer does.
    let resolved = import_created(
        &gateway,
        "a reference the document declares",
        "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"w\"}, \
         \"components\": {\"schemas\": {\"Thing\": {\"type\": \"object\"}, \
             \"a/b\": {\"type\": \"object\"}}}, \
         \"paths\": {\"/thing\": {\"get\": {\"responses\": {\"200\": {\"description\": \"ok\", \
             \"content\": {\"application/json\": {\"schema\": \
                 {\"$ref\": \"#/components/schemas/Thing\"}}, \
                 \"application/xml\": {\"schema\": \
                 {\"$ref\": \"#/components/schemas/a~1b\"}}}}}}}}}",
    )
    .await;
    assert!(
        resolved["warnings"].is_null(),
        "case a reference the document declares: expected no warnings, got {:?}",
        resolved["warnings"]
    );
}

/// Emulator resource guard (`put_rest_api.rs`): the most nodes one parsed definition may
/// hold, whichever spelling it was written in.
const MAX_DEFINITION_NODES: usize = 50_000;

#[tokio::test]
async fn a_definition_built_to_explode_the_reference_walk_is_served_or_refused_under_a_bound() {
    // A document whose every entry carries a distinct unresolvable reference: the work of
    // checking them is bounded by a count rather than by what the body asks for, so this
    // is answered as quickly as any other import — and it reports no more warnings than
    // any other response carries.
    let references: Vec<String> = (0..2_000)
        .map(|index| format!("\"k{index}\": {{\"$ref\": \"#/missing{index}\"}}"))
        .collect();
    let many_references = format!(
        "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"many\"}}, \"paths\": {{}}, {}}}",
        references.join(", ")
    );

    let gateway = Gateway::new();
    let started = std::time::Instant::now();
    let answer = import(&gateway, &many_references, false, &[]).await;
    let elapsed = started.elapsed();
    assert_eq!(
        answer.status, 201,
        "case many references: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let warnings = answer.body["warnings"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert!(
        warnings <= 50,
        "case many references: a response carries at most the warnings this emulator \
         reports for a collection, got {warnings}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "case many references: the reference walk must be bounded by a count, took {elapsed:?}"
    );

    // And a document carrying more nodes than the reader admits is refused under that
    // bound in BOTH spellings — the JSON one here, the YAML one in the `rest_api_import`
    // suite — leaving no API behind.
    let nodes: Vec<String> = (0..MAX_DEFINITION_NODES + 1)
        .map(|index| format!("\"k{index}\": 1"))
        .collect();
    let before = api_count(&gateway, "more nodes than the reader admits").await;
    let answer = import(
        &gateway,
        &format!(
            "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"nodes\"}}, \"paths\": {{}}, {}}}",
            nodes.join(", ")
        ),
        false,
        &[],
    )
    .await;
    assert_failure(
        &answer,
        400,
        "BadRequestException",
        "more nodes than the reader admits",
    );
    let after = api_count(&gateway, "more nodes than the reader admits").await;
    assert_eq!(
        after, before,
        "case more nodes than the reader admits: a refused import must create no API, \
         expected {before} APIs, got {after}"
    );
}

#[tokio::test]
async fn a_body_that_is_not_an_api_definition_is_refused_and_creates_no_api() {
    // Rows differ along HOW the request is wrong: the body is not a document, not an
    // object, not an API definition, its `paths` are not paths, or it names no API.
    let cases: &[(&str, &str)] = &[
        ("an empty body", ""),
        (
            "an unparsable document",
            "{\"openapi\": \"3.0.1\", \"paths\": {",
        ),
        ("a scalar document", "\"just a string\""),
        (
            "no openapi or swagger version",
            "{\"info\": {\"title\": \"x\"}, \"paths\": {\"/pets\": {\"get\": {}}}}",
        ),
        (
            "paths that are not an object",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"x\"}, \"paths\": [\"/pets\"]}",
        ),
        (
            "a path that does not start with a separator",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"x\"}, \"paths\": {\"pets\": {}}}",
        ),
        (
            "a definition that names no API",
            "{\"openapi\": \"3.0.1\", \"paths\": {\"/pets\": {\"get\": {}}}}",
        ),
        (
            "a definition whose info names no API",
            "{\"openapi\": \"3.0.1\", \"info\": {\"version\": \"1\"}, \"paths\": {}}",
        ),
        (
            "a definition whose title is empty",
            "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"\"}, \"paths\": {}}",
        ),
    ];

    let gateway = Gateway::new();
    // One served import, so the refusals below are checked against an account that
    // already holds an API: a refusal must leave THAT api alone too.
    let kept = import_created(
        &gateway,
        "a served import",
        &json_definition("kept", &[("/pets", &["get"])]),
    )
    .await;
    let kept_api = string_member(&kept, "id", "a served import");
    let before = api_count(&gateway, "refusals").await;

    for (case, body) in cases {
        let answer = import(&gateway, body, false, &[]).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let after = api_count(&gateway, case).await;
        assert_eq!(
            after, before,
            "case {case}: a refused import must create no API, expected {before} APIs, got {after}"
        );
        let kept_paths = resource_paths(&gateway, &kept_api).await;
        assert_eq!(
            kept_paths,
            vec!["/".to_string(), "/pets".to_string()],
            "case {case}: a refused import must leave the account's other APIs alone, got {kept_paths:?}"
        );
    }

    // The emulator is still serving, and a well-formed definition still imports.
    let after_refusals = import_created(
        &gateway,
        "after the refusals",
        &json_definition("after", &[("/orders", &["post"])]),
    )
    .await;
    let api = string_member(&after_refusals, "id", "after the refusals");
    let listed = resource_paths(&gateway, &api).await;
    assert_eq!(
        listed,
        vec!["/".to_string(), "/orders".to_string()],
        "case after the refusals: expected the imported path, got {listed:?}"
    );
}
