// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for importing an API definition with `PutRestApi`, read back through
//! `GetResources`, `GetResource`, `GetMethod` and `GetRestApi`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` helper are this crate's existing harness, reused from
//! `rest_api_lifecycle`.
//!
//! The tests assert the RULE over sets of inputs, not one recorded call: that every path
//! of the imported definition (and every ancestor a path implies) becomes a `Resource`
//! parented to the resource of its parent path, and every operation on a path becomes a
//! `Method` on that path's resource. The rows differ along what the rule generalises
//! over — the number of paths, their nesting depth, shared ancestors, the path forms
//! API Gateway accepts, the verbs put on a path, the document's form (JSON and the YAML
//! the same definition is written in), and both `PutMode` values.

use serde_json::Value;

use crate::rest_api_lifecycle::{create_api, page_items, resource_paths, string_member, Answer, Gateway};

/// The HTTP verbs an imported operation can become, as the keys an OpenAPI Path Item
/// Object uses for them. `x-amazon-apigateway-any-method` is API Gateway's own extension
/// for the `ANY` catch-all; the others are the OpenAPI operation fields whose verbs are
/// in the service's method catalog (`ANY`, `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`,
/// `POST`, `PUT`).
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

/// One path of a table row's definition: the path the document declares, and the
/// operation keys the document puts on it.
type PathRow<'a> = (&'a str, &'a [&'a str]);

/// One row of the import table: the case's name, the definition's paths, and the full
/// resource paths the import must leave behind.
type ImportCase<'a> = (&'a str, &'a [PathRow<'a>], &'a [&'a str]);

/// The OpenAPI definition text for `paths`, as JSON.
///
/// Written out by hand rather than through `serde_json::Map`, which SORTS its keys: the
/// document's own key order is what the import walks, so a document-order assertion
/// built on a sorted map would be vacuous.
fn json_definition(title: &str, paths: &[(&str, &[&str])]) -> String {
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
fn yaml_definition(title: &str, paths: &[(&str, &[&str])]) -> String {
    let mut text = format!("openapi: 3.0.1\ninfo:\n  title: {title}\n  version: '2026-01-01'\npaths:\n");
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

/// `PutRestApi` with `body` sent verbatim as the definition payload.
async fn put_definition(
    gateway: &Gateway,
    api: &str,
    mode: Option<&str>,
    fail_on_warnings: bool,
    body: &str,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(mode) = mode {
        query.push(format!("mode={mode}"));
    }
    if fail_on_warnings {
        query.push("failonwarnings=true".to_string());
    }
    let uri = if query.is_empty() {
        format!("/restapis/{api}")
    } else {
        format!("/restapis/{api}?{}", query.join("&"))
    };
    gateway.call_raw("PUT", &uri, body).await
}

/// `PutRestApi` with `body`, asserting the modelled 200 and returning the `RestApi` body.
async fn import_definition(
    gateway: &Gateway,
    case: &str,
    api: &str,
    mode: Option<&str>,
    body: &str,
) -> Value {
    let answer = put_definition(gateway, api, mode, false, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: PutRestApi expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
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

#[test]
fn every_verb_an_import_can_create_is_in_the_services_method_catalog() {
    use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_method::HTTP_METHODS;

    // The import's table of operation keys is written against the catalog `PutMethod`
    // accepts, so a verb it could create that the catalog does not carry would be a
    // method no other operation would admit. This pins the two together.
    for (key, verb) in OPERATION_KEYS {
        assert!(
            HTTP_METHODS.contains(verb),
            "case {key:?}: the verb {verb:?} an import creates must be in the service's method catalog {HTTP_METHODS:?}"
        );
    }
    // And the operation fields the import reports as unsupported are exactly the ones
    // whose verb that catalog does NOT carry.
    for key in ["trace", "connect"] {
        let verb = key.to_ascii_uppercase();
        assert!(
            !HTTP_METHODS.contains(&verb.as_str()),
            "case {key:?}: {verb:?} is in the method catalog {HTTP_METHODS:?}, so the import must create it rather than warn about it"
        );
    }
}

#[tokio::test]
async fn every_path_of_an_imported_definition_becomes_a_resource() {
    // Rows differ along what the rule generalises over: how many paths the document
    // declares, how deeply they nest, whether they share an ancestor the document never
    // declares on its own, and the path forms API Gateway accepts (a path parameter, a
    // greedy proxy, a path that is only the root, a trailing separator).
    let cases: &[ImportCase] = &[
        ("one flat path", &[("/pets", &["get"])], &["/", "/pets"]),
        (
            "two flat paths in document order",
            &[("/pets", &["get"]), ("/items", &["get"])],
            &["/", "/pets", "/items"],
        ),
        (
            "a path parameter under an undeclared ancestor",
            &[("/pets/{petId}", &["get"])],
            &["/", "/pets", "/pets/{petId}"],
        ),
        (
            "three levels of nesting",
            &[("/a/b/c", &["get"])],
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
            &[("/pets/", &["get"])],
            &["/", "/pets"],
        ),
        (
            "a path with no operations is still a resource",
            &[("/pets", &[])],
            &["/", "/pets"],
        ),
    ];

    let gateway = Gateway::new();
    for (case, paths, expected) in cases {
        let (api, root) = create_api(&gateway, &format!("import-{case}")).await;
        import_definition(&gateway, case, &api, None, &json_definition(case, paths)).await;

        let listed = resource_paths(&gateway, &api).await;
        assert_eq!(
            listed,
            expected.iter().map(|path| (*path).to_string()).collect::<Vec<String>>(),
            "case {case}: expected the imported paths {expected:?}, got {listed:?}"
        );

        // Each imported resource is parented to the resource of its parent path and
        // carries its own last segment — the same record `CreateResource` would build.
        let resources = resources_by_path(&gateway, &api, case).await;
        for (path, item) in &resources {
            if path == "/" {
                assert_eq!(
                    item["id"].as_str(),
                    Some(root.as_str()),
                    "case {case}: the root resource keeps its id {root:?}, got {item:?}"
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

            // Round trip: the reader reports the persisted record, member for member.
            let id = string_member(item, "id", case);
            let read = gateway
                .call("GET", &format!("/restapis/{api}/resources/{id}"), None)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case} at {path:?}: GetResource expected 200, got {} with {:?}",
                read.status, read.body
            );
            for member in ["id", "parentId", "pathPart", "path"] {
                assert_eq!(
                    read.body[member], item[member],
                    "case {case} at {path:?}: GetResource must report the persisted {member}: expected {:?}, got {:?}",
                    item[member], read.body[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn every_operation_of_an_imported_path_becomes_a_method_on_its_resource() {
    // The catalog row: every operation key the import understands, on one path. The
    // other rows vary the count and the mix, including a key that is not an operation
    // at all (`summary`, `parameters`) and so imports nothing.
    let all_keys: Vec<&str> = OPERATION_KEYS.iter().map(|(key, _)| *key).collect();
    let cases: &[&[&str]] = &[
        &["get"],
        &["get", "post"],
        &["x-amazon-apigateway-any-method"],
        &["patch", "head", "options"],
        &all_keys,
    ];

    let gateway = Gateway::new();
    for keys in cases {
        let case = format!("{keys:?}");
        let (api, _) = create_api(&gateway, &format!("methods-{}", keys.join("-"))).await;
        import_definition(
            &gateway,
            &case,
            &api,
            None,
            &json_definition("methods", &[("/pets", keys)]),
        )
        .await;

        let resources = resources_by_path(&gateway, &api, &case).await;
        let (_, pets) = resources
            .iter()
            .find(|(path, _)| path == "/pets")
            .unwrap_or_else(|| panic!("case {case}: expected a /pets resource, got {resources:?}"));
        let pets_id = string_member(pets, "id", &case);

        let expected: Vec<String> = keys
            .iter()
            .map(|key| {
                OPERATION_KEYS
                    .iter()
                    .find(|(candidate, _)| candidate == key)
                    .map(|(_, verb)| (*verb).to_string())
                    .unwrap_or_else(|| panic!("case {case}: {key:?} is not an operation key"))
            })
            .collect();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        let reported = method_keys(pets);
        assert_eq!(
            reported, expected_sorted,
            "case {case}: expected exactly the imported methods {expected_sorted:?}, got {reported:?}"
        );
        for verb in &expected {
            // Round trip: each imported method is a readable record, open because the
            // operation declares no security requirement.
            let read = gateway
                .call(
                    "GET",
                    &format!("/restapis/{api}/resources/{pets_id}/methods/{verb}"),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case}: GetMethod({verb}) expected 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["httpMethod"].as_str(),
                Some(verb.as_str()),
                "case {case}: GetMethod({verb}) expected httpMethod {verb:?}, got {:?}",
                read.body["httpMethod"]
            );
            assert_eq!(
                read.body["authorizationType"].as_str(),
                Some("NONE"),
                "case {case}: GetMethod({verb}) expected authorizationType NONE, got {:?}",
                read.body["authorizationType"]
            );
        }

        // A path item key that names no operation imports no method.
        let described = import_definition(
            &gateway,
            &case,
            &api,
            None,
            "{\"openapi\": \"3.0.1\", \"paths\": {\"/described\": {\"summary\": \"a path\", \
             \"parameters\": []}}}",
        )
        .await;
        assert!(
            described["warnings"].is_null(),
            "case {case}: a path item that declares no operation warns about nothing, got {:?}",
            described["warnings"]
        );
        let resources = resources_by_path(&gateway, &api, &case).await;
        let (_, described) = resources
            .iter()
            .find(|(path, _)| path == "/described")
            .unwrap_or_else(|| panic!("case {case}: expected a /described resource"));
        assert!(
            described["resourceMethods"].is_null(),
            "case {case}: /described carries no method, got {:?}",
            described["resourceMethods"]
        );
    }
}

#[tokio::test]
async fn the_same_definition_in_yaml_imports_the_same_tree_as_in_json() {
    // The rule is about the definition, not the form it is written in: each row is
    // imported twice, once from JSON and once from the YAML of the same document, and
    // the two APIs must end up with the same tree.
    let cases: &[&[(&str, &[&str])]] = &[
        &[("/pets", &["get"])],
        &[("/pets", &["get", "post"]), ("/items", &["delete"])],
        &[("/pets/{petId}/toys", &["get"])],
        &[("/{proxy+}", &["x-amazon-apigateway-any-method"])],
    ];

    let gateway = Gateway::new();
    for paths in cases {
        let case = format!("{paths:?}");
        let (json_api, _) = create_api(&gateway, &format!("json-{}", paths.len())).await;
        let (yaml_api, _) = create_api(&gateway, &format!("yaml-{}", paths.len())).await;
        import_definition(
            &gateway,
            &case,
            &json_api,
            None,
            &json_definition("same", paths),
        )
        .await;
        import_definition(
            &gateway,
            &case,
            &yaml_api,
            None,
            &yaml_definition("same", paths),
        )
        .await;

        let from_json = resources_by_path(&gateway, &json_api, &case).await;
        let from_yaml = resources_by_path(&gateway, &yaml_api, &case).await;
        let json_paths: Vec<String> = from_json.iter().map(|(path, _)| path.clone()).collect();
        let yaml_paths: Vec<String> = from_yaml.iter().map(|(path, _)| path.clone()).collect();
        assert_eq!(
            json_paths, yaml_paths,
            "case {case}: the YAML form must import the same paths as the JSON form: expected {json_paths:?}, got {yaml_paths:?}"
        );
        for ((path, json_item), (_, yaml_item)) in from_json.iter().zip(from_yaml.iter()) {
            let json_methods = method_keys(json_item);
            let yaml_methods = method_keys(yaml_item);
            assert_eq!(
                json_methods, yaml_methods,
                "case {case} at {path:?}: expected the methods {json_methods:?}, got {yaml_methods:?}"
            );
        }
    }
}

#[tokio::test]
async fn merge_adds_to_the_definition_and_overwrite_replaces_it() {
    // The first definition, then a second one that overlaps it on `/pets` (a new verb on
    // a path that exists) and adds `/orders`.
    let first = json_definition("first", &[("/pets", &["get"]), ("/items", &["get"])]);
    let second = json_definition("second", &[("/pets", &["post"]), ("/orders", &["get"])]);

    // Each row is the mode of the SECOND import and the tree it must leave behind: a
    // merge is the union of both definitions, an overwrite is the second one alone, and
    // the default mode is `merge`.
    let cases: &[(&str, Option<&str>, &[&str])] = &[
        ("explicit merge", Some("merge"), &["/", "/pets", "/items", "/orders"]),
        ("default mode", None, &["/", "/pets", "/items", "/orders"]),
        ("overwrite", Some("overwrite"), &["/", "/pets", "/orders"]),
    ];

    let gateway = Gateway::new();
    for (case, mode, expected) in cases {
        let (api, root) = create_api(&gateway, &format!("mode-{case}")).await;
        import_definition(&gateway, case, &api, Some("merge"), &first).await;
        let body = import_definition(&gateway, case, &api, *mode, &second).await;

        let listed = resource_paths(&gateway, &api).await;
        let mut sorted = listed.clone();
        sorted.sort();
        let mut wanted: Vec<String> = expected.iter().map(|path| (*path).to_string()).collect();
        wanted.sort();
        assert_eq!(
            sorted, wanted,
            "case {case}: expected the resources {wanted:?}, got {listed:?}"
        );

        // The API's root resource is an attribute of the API: it survives either mode
        // under its own id, and the response still reports it.
        assert_eq!(
            body["rootResourceId"].as_str(),
            Some(root.as_str()),
            "case {case}: expected the rootResourceId {root:?}, got {:?}",
            body["rootResourceId"]
        );

        // `/pets` exists in both definitions: the second import's verb is on it either
        // way, and the first import's verb only survives a merge.
        let resources = resources_by_path(&gateway, &api, case).await;
        let (_, pets) = resources
            .iter()
            .find(|(path, _)| path == "/pets")
            .unwrap_or_else(|| panic!("case {case}: expected a /pets resource, got {resources:?}"));
        let methods = method_keys(pets);
        let expected_methods: Vec<String> = if *mode == Some("overwrite") {
            vec!["POST".to_string()]
        } else {
            vec!["GET".to_string(), "POST".to_string()]
        };
        assert_eq!(
            methods, expected_methods,
            "case {case}: expected exactly {expected_methods:?} on /pets, got {methods:?}"
        );
    }
}

#[tokio::test]
async fn an_import_takes_the_apis_metadata_from_the_definitions_info() {
    let gateway = Gateway::new();
    // `info` is where an OpenAPI document carries the API's name, description and
    // version, so an import takes each of them from the document.
    let full = "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"renamed\", \
                \"description\": \"from the definition\", \"version\": \"2026-02-02\"}, \
                \"x-amazon-apigateway-binary-media-types\": [\"image/png\"], \
                \"paths\": {\"/pets\": {\"get\": {}}}}";
    // The same definition with everything but the title left out.
    let quiet = "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"renamed\"}, \
                 \"paths\": {\"/pets\": {\"get\": {}}}}";

    // The rule holds for either mode and for the default: `info` describes the
    // definition being imported, not the paths under it.
    for mode in [Some("merge"), Some("overwrite"), None] {
        let case = format!("mode {mode:?}");
        let (api, _) = create_api(&gateway, "metadata").await;
        let body = import_definition(&gateway, &case, &api, mode, full).await;
        for (member, expected) in [
            ("name", "renamed"),
            ("description", "from the definition"),
            ("version", "2026-02-02"),
        ] {
            assert_eq!(
                body[member].as_str(),
                Some(expected),
                "case {case}: expected {member} {expected:?}, got {:?}",
                body[member]
            );
        }
        assert_eq!(
            body["binaryMediaTypes"],
            serde_json::json!(["image/png"]),
            "case {case}: the definition's binary media types are the API's, got {:?}",
            body["binaryMediaTypes"]
        );

        // Round trip: the reader reports the same record the import answered.
        let read = gateway.call("GET", &format!("/restapis/{api}"), None).await;
        for member in [
            "id",
            "name",
            "description",
            "version",
            "rootResourceId",
            "binaryMediaTypes",
        ] {
            assert_eq!(
                read.body[member], body[member],
                "case {case}: GetRestApi must report the persisted {member}: expected {:?}, got {:?}",
                body[member], read.body[member]
            );
        }

        // A member the document leaves out leaves the stored one alone: a definition
        // that says nothing about the API's version does not clear it.
        let silent = import_definition(&gateway, &case, &api, mode, quiet).await;
        for (member, expected) in [
            ("name", "renamed"),
            ("description", "from the definition"),
            ("version", "2026-02-02"),
        ] {
            assert_eq!(
                silent[member].as_str(),
                Some(expected),
                "case {case}: a definition that declares no {member} keeps the stored {expected:?}, got {:?}",
                silent[member]
            );
        }
        assert_eq!(
            silent["binaryMediaTypes"],
            serde_json::json!(["image/png"]),
            "case {case}: a definition that declares no binary media types keeps the stored ones, got {:?}",
            silent["binaryMediaTypes"]
        );
    }
}

#[tokio::test]
async fn an_operation_api_gateway_has_no_method_for_is_reported_as_a_warning() {
    let gateway = Gateway::new();
    let definition = json_definition("warned", &[("/pets", &["get", "trace"])]);

    // Without `failonwarnings` the import is served, the method API Gateway has no verb
    // for is skipped, and the response says so.
    let (api, _) = create_api(&gateway, "warnings").await;
    let body = import_definition(&gateway, "warning reported", &api, None, &definition).await;
    let warnings = body["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("case warning reported: expected warnings, got {body:?}"));
    assert_eq!(
        warnings.len(),
        1,
        "case warning reported: expected one warning, got {warnings:?}"
    );
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|warning| warning.contains("trace") && warning.contains("/pets")),
        "case warning reported: the warning names the operation and its path, got {:?}",
        warnings[0]
    );
    let resources = resources_by_path(&gateway, &api, "warning reported").await;
    let (_, pets) = resources
        .iter()
        .find(|(path, _)| path == "/pets")
        .unwrap_or_else(|| panic!("case warning reported: expected a /pets resource"));
    let methods = method_keys(pets);
    assert_eq!(
        methods,
        vec!["GET".to_string()],
        "case warning reported: only the supported operation is imported, got {methods:?}"
    );

    // With `failonwarnings=true` the update is rolled back: the request is refused and
    // the API is left exactly as it was.
    let (rolled_back, _) = create_api(&gateway, "rollback").await;
    let before = resource_paths(&gateway, &rolled_back).await;
    let answer = put_definition(&gateway, &rolled_back, None, true, &definition).await;
    assert_eq!(
        answer.status, 400,
        "case failonwarnings: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("BadRequestException"),
        "case failonwarnings: expected BadRequestException, got {:?}",
        answer.error
    );
    let after = resource_paths(&gateway, &rolled_back).await;
    assert_eq!(
        after, before,
        "case failonwarnings: a refused import must change nothing, expected {before:?}, got {after:?}"
    );
}

#[tokio::test]
async fn a_body_that_is_not_an_api_definition_is_refused_and_changes_nothing() {
    // Rows differ along HOW the request is wrong: the body is not a document, not an
    // object, not an API definition, or its `paths` are not paths.
    let cases: &[(&str, &str)] = &[
        ("an empty body", ""),
        ("an unparsable document", "{\"openapi\": \"3.0.1\", \"paths\": {"),
        ("a scalar document", "\"just a string\""),
        (
            "no openapi or swagger version",
            "{\"info\": {\"title\": \"x\"}, \"paths\": {\"/pets\": {\"get\": {}}}}",
        ),
        (
            "paths that are not an object",
            "{\"openapi\": \"3.0.1\", \"paths\": [\"/pets\"]}",
        ),
        (
            "a path that does not start with a separator",
            "{\"openapi\": \"3.0.1\", \"paths\": {\"pets\": {\"get\": {}}}}",
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "refusals").await;
    let before = resource_paths(&gateway, &api).await;
    for (case, body) in cases {
        let answer = put_definition(&gateway, &api, None, false, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            answer.error
        );
        let after = resource_paths(&gateway, &api).await;
        assert_eq!(
            after, before,
            "case {case}: a refused import must change nothing, expected {before:?}, got {after:?}"
        );
    }

    // A `mode` outside the model's `PutMode` enum is refused the same way.
    let answer = put_definition(
        &gateway,
        &api,
        Some("replace"),
        false,
        &json_definition("bad mode", &[("/pets", &["get"])]),
    )
    .await;
    assert!(
        answer.status == 400,
        "case an unknown mode: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    let after = resource_paths(&gateway, &api).await;
    assert_eq!(
        after, before,
        "case an unknown mode: a refused import must change nothing, expected {before:?}, got {after:?}"
    );

    // An unknown `restApiId` is the modelled 404, and nothing is created for it.
    let answer = put_definition(
        &gateway,
        "nosuchapi0",
        None,
        false,
        &json_definition("unknown api", &[("/pets", &["get"])]),
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case an unknown restApiId: expected 404, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("NotFoundException"),
        "case an unknown restApiId: expected NotFoundException, got {:?}",
        answer.error
    );
}

/// Emulator resource guard (`put_rest_api.rs`): the largest definition the import reads.
const MAX_DEFINITION_BYTES: usize = 1024 * 1024;

#[tokio::test]
async fn a_definition_built_to_explode_the_parse_is_refused_and_changes_nothing() {
    // Rows differ along the SHAPE of the amplification a body can carry: an alias that
    // copies an anchored node (the YAML-bomb shape, which expands multiplicatively per
    // level), nesting deep enough to drive a recursive loader off the stack in either
    // spelling, block nesting by indentation and by compact indicators, more nodes than
    // the reader admits, and a body larger than it reads.
    let mut alias_bomb = String::from("openapi: 3.0.1\nlaugh0: &laugh0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..10 {
        let copies = (0..10)
            .map(|_| format!("*laugh{}", level - 1))
            .collect::<Vec<String>>()
            .join(", ");
        alias_bomb.push_str(&format!("laugh{level}: &laugh{level} [{copies}]\n"));
    }
    alias_bomb.push_str("paths: {}\n");

    let yaml_nesting = format!("openapi: 3.0.1\npaths: {}", "[".repeat(4_096));
    let json_nesting = format!(
        "{{\"openapi\": \"3.0.1\", \"paths\": {}",
        "[".repeat(4_096)
    );
    let indent_bomb = format!("openapi: 3.0.1\npaths:\n{}deep: 1\n", " ".repeat(4_096));
    let compact_bomb = format!("openapi: 3.0.1\npaths:\n  {}x\n", "- ".repeat(4_096));
    let mut node_bomb = String::from("openapi: 3.0.1\nitems:\n");
    for _ in 0..60_000 {
        node_bomb.push_str("- 1\n");
    }
    let oversized = format!(
        "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"{}\"}}, \"paths\": {{}}}}",
        "x".repeat(MAX_DEFINITION_BYTES + 1)
    );

    let cases: &[(&str, &str)] = &[
        ("an anchor/alias expansion", &alias_bomb),
        ("deeply nested YAML", &yaml_nesting),
        ("deeply nested JSON", &json_nesting),
        ("runaway indentation", &indent_bomb),
        ("runaway compact block indicators", &compact_bomb),
        ("more nodes than the reader admits", &node_bomb),
        ("a body larger than the reader admits", &oversized),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "bombs").await;
    let before = resource_paths(&gateway, &api).await;
    for (case, body) in cases {
        let answer = put_definition(&gateway, &api, None, false, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            answer.error
        );
        let after = resource_paths(&gateway, &api).await;
        assert_eq!(
            after, before,
            "case {case}: a refused import must change nothing, expected {before:?}, got {after:?}"
        );
    }

    // The emulator is still serving, and a well-formed definition still imports: the
    // refusals above cost the caller a 400 and cost everyone else nothing.
    import_definition(
        &gateway,
        "after the bombs",
        &api,
        None,
        &json_definition("after", &[("/pets", &["get"])]),
    )
    .await;
    let after = resource_paths(&gateway, &api).await;
    assert_eq!(
        after,
        vec!["/".to_string(), "/pets".to_string()],
        "case after the bombs: expected the imported path, got {after:?}"
    );
}
