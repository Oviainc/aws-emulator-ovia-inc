// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ADVISORY REPORT `ImportApi` and `ReimportApi` answer an
//! OpenAPI definition with: the `ImportInfo` list ("particular properties of your OpenAPI
//! definition which are ignored during import") and the `Warnings` list, both
//! `__listOf__string` — lists, so the ORDER and the TEXT of their entries are part of the
//! response.
//!
//! The rules these cases assert:
//!
//! * **The ignored-extension-key report is a function of the key SET, in API Gateway's own
//!   order**: an `x-amazon-apigateway-integration` key an HTTP api's integration has no
//!   member for is reported in the order a REST api's integration declares its members
//!   (`requestTemplates`, `passthroughBehavior`, `contentHandling`, the cache members,
//!   `responses`), so the same keys laid out in ANY order in the submitted document report
//!   the same entries in the same order; a key that configures no integration at all
//!   follows them in its own key order.
//! * **An integration `type` outside the modelled `IntegrationType` catalog names no
//!   integration type at all**: such an extension is reported exactly as one that writes no
//!   `type` is — `IntegrationType is a required parameter for creating integration.` — and
//!   the submitted value is never named, while a `type` the catalog DOES carry is reported
//!   on its own terms (or creates the integration).
//! * **`FailOnWarnings` refuses the import with every warning it found**: the refusal
//!   carries API Gateway's `Warnings found during import:` sentence followed by each
//!   warning on its own indented line, and persists nothing.
//! * **`ReimportApi` reports what `ImportApi` reports**, since the two read one definition
//!   reader: every rule above is asserted through both operations.
//!
//! The rows span the number of ignored keys (none, one, two, three, five), the order the
//! document writes them in (ascending, descending, scrambled, and — in the permutation
//! case — every ordering of five members), several methods (including the catch-all one)
//! and several paths; and, for the type rule, the spellings a `type` outside the catalog
//! can arrive as (an unknown name, a plausible misspelling, an empty string, a number, an
//! object, a null, and no `type` member at all). The recorded cases that filed the issue —
//! one path carrying both of two ignored keys, and a `GET` operation declaring an
//! unrecognised type — are ONE row each.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_import::{import, stored_integrations, stored_routes};
use crate::api_lifecycle::{as_str, call, emulator, error_code};
use crate::api_reimport::reimport;

/// The extension an OpenAPI operation carries its API Gateway integration in.
const INTEGRATION_EXTENSION: &str = "x-amazon-apigateway-integration";

/// The reason API Gateway gives for an integration that names no `IntegrationType`.
const INTEGRATION_TYPE_REQUIRED: &str =
    "IntegrationType is a required parameter for creating integration.";

/// The members of an extension that DO create an integration an HTTP api can hold, as the
/// raw JSON the document writes them as.
const CARRIED_MEMBERS: &[(&str, &str)] = &[
    ("type", "\"http_proxy\""),
    ("uri", "\"https://upstream.example.com\""),
];

/// One member of an integration extension: its key, and the raw JSON the document writes
/// as its value.
type Member<'a> = (&'a str, &'a str);

/// One operation of a document: the operation key as written, the path it is declared
/// under, and the extension's members in the order the document writes them.
type Operation<'a> = (&'a str, &'a str, Vec<Member<'a>>);

/// A document declaring ONE operation at `path` under `method`, whose
/// [`INTEGRATION_EXTENSION`] carries `members` IN THE ORDER GIVEN.
///
/// The body is formatted as TEXT rather than built through a JSON map, so the order the
/// submitted document writes the extension's keys in is exactly the order given here —
/// whatever a map type in the build would otherwise do with it.
fn definition(name: &str, method: &str, path: &str, members: &[Member<'_>]) -> String {
    definition_of(name, &[(method, path, members.to_vec())])
}

/// A document declaring one operation per entry of `operations`, each as [`definition`]
/// declares its one.
fn definition_of(name: &str, operations: &[Operation<'_>]) -> String {
    let paths = operations
        .iter()
        .map(|(method, path, members)| {
            let written = members
                .iter()
                .map(|(key, value)| format!("\"{key}\": {value}"))
                .collect::<Vec<String>>()
                .join(", ");
            let operation = format!("{{\"{INTEGRATION_EXTENSION}\": {{{written}}}}}");
            format!("\"{path}\": {{\"{method}\": {operation}}}")
        })
        .collect::<Vec<String>>()
        .join(", ");
    let info = format!("{{\"title\": \"{name}\", \"version\": \"1.0\"}}");
    let body = format!("{{\"openapi\": \"3.0.1\", \"info\": {info}, \"paths\": {{{paths}}}}}");
    assert!(
        ::serde_json::from_str::<Value>(&body).is_ok(),
        "the case's own definition must be JSON, got {body}"
    );
    body
}

/// The `ImportInfo` entry an ignored extension key is reported with, at the location the
/// document wrote it at.
fn ignored_key_advisory(path: &str, method: &str, key: &str) -> String {
    format!("paths.{path}.{method}.{INTEGRATION_EXTENSION}: key [{key}] will be ignored by API Gateway")
}

/// The whole warning an import reports for an integration it could not create.
fn unable_to_create(method: &str, path: &str, reason: &str) -> String {
    format!("Unable to create integration for resource at path '{method} {path}': {reason}. Ignoring.")
}

/// Every `ImportInfo` entry an answer reports, in order.
fn import_info(answer: &Value) -> Vec<String> {
    strings(answer, "importInfo")
}

/// Every `Warnings` entry an answer reports, in order.
fn warnings(answer: &Value) -> Vec<String> {
    strings(answer, "warnings")
}

/// One `__listOf__string` member of an answer, in order.
fn strings(answer: &Value, member: &str) -> Vec<String> {
    answer[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.as_str().map(ToString::to_string))
        .collect()
}

/// An api to reimport over: one imported from a definition that reports nothing, so every
/// entry a later reimport reports is the reimported document's own.
async fn api_to_reimport_over(router: &MultiServiceRouter, name: &str) -> String {
    let clean = definition(name, "get", "/seed", CARRIED_MEMBERS);
    let (status, imported) = import(router, "", &clean).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected 200 from ImportApi for the api to reimport over, got \
         {status} {imported}"
    );
    assert!(
        import_info(&imported).is_empty() && warnings(&imported).is_empty(),
        "case {name:?}: the seed definition must report nothing, got {imported}"
    );
    as_str(&imported, "apiId")
}

/// One row of the ignored-key order table: the extension the document writes, and the keys
/// the report must list, in the order it must list them.
struct OrderRow {
    case: &'static str,
    /// The operation key the document declares, as written.
    declared_method: &'static str,
    path: &'static str,
    /// The extension's members, in the order the document writes them.
    members: Vec<(&'static str, &'static str)>,
    /// The ignored keys, in the order the report must list them.
    expect_keys: Vec<&'static str>,
}

/// The ignored keys the rows below use are REST-api integration fields —
/// `requestTemplates`, `passthroughBehavior`, `contentHandling`, `cacheNamespace`,
/// `cacheKeyParameters`, `responses` — none of which the modelled `Integration` of an HTTP
/// api carries a member for
/// (`smithy_query(query="shape",shape="Integration")`), plus vendor extensions of the
/// document's own, which configure no integration at all.
///
/// The order the report lists them in is that REST integration's member order, which is
/// the order of the six keys as written in the previous paragraph.
fn order_rows() -> Vec<OrderRow> {
    vec![
        OrderRow {
            // The recorded case: one operation whose extension carries BOTH of two ignored
            // keys, written with `responses` first.
            case: "the recorded pair, written responses first",
            declared_method: "get",
            path: "/probe",
            members: vec![
                ("type", "\"http_proxy\""),
                ("uri", "\"https://probe.example.com/probe\""),
                ("responses", "{\"default\": {\"statusCode\": \"200\"}}"),
                ("passthroughBehavior", "\"when_no_match\""),
            ],
            expect_keys: vec!["passthroughBehavior", "responses"],
        },
        OrderRow {
            case: "the same pair, written passthroughBehavior first",
            declared_method: "post",
            path: "/probe",
            members: vec![
                ("passthroughBehavior", "\"when_no_match\""),
                ("type", "\"http_proxy\""),
                ("responses", "{\"default\": {\"statusCode\": \"200\"}}"),
                ("uri", "\"https://probe.example.com/probe\""),
            ],
            expect_keys: vec!["passthroughBehavior", "responses"],
        },
        OrderRow {
            case: "three keys written in reverse report order",
            declared_method: "put",
            path: "/images/{id}",
            members: vec![
                ("type", "\"http_proxy\""),
                ("uri", "\"https://upstream.example.com\""),
                ("requestTemplates", "{\"application/json\": \"{}\"}"),
                ("contentHandling", "\"CONVERT_TO_BINARY\""),
                ("cacheNamespace", "\"items\""),
            ],
            expect_keys: vec!["requestTemplates", "contentHandling", "cacheNamespace"],
        },
        OrderRow {
            case: "five keys written scrambled, under the catch-all method",
            declared_method: "x-amazon-apigateway-any-method",
            path: "/legacy/{proxy+}",
            members: vec![
                ("responses", "{\"default\": {\"statusCode\": \"200\"}}"),
                ("cacheKeyParameters", "[\"method.request.path.id\"]"),
                ("type", "\"http_proxy\""),
                ("passthroughBehavior", "\"never\""),
                ("uri", "\"https://legacy.example.com\""),
                ("requestTemplates", "{\"application/json\": \"{}\"}"),
                ("contentHandling", "\"CONVERT_TO_TEXT\""),
            ],
            expect_keys: vec![
                "requestTemplates",
                "passthroughBehavior",
                "contentHandling",
                "cacheKeyParameters",
                "responses",
            ],
        },
        OrderRow {
            case: "an ignored key beside two vendor extensions of the document's own",
            declared_method: "delete",
            path: "/vendor",
            members: vec![
                ("x-probe-note", "\"kept out of the integration\""),
                ("type", "\"http_proxy\""),
                ("responses", "{\"default\": {\"statusCode\": \"200\"}}"),
                ("x-another-note", "\"also kept out\""),
                ("uri", "\"https://vendor.example.com\""),
            ],
            expect_keys: vec!["responses", "x-another-note", "x-probe-note"],
        },
        OrderRow {
            case: "one ignored key",
            declared_method: "head",
            path: "/cached",
            members: vec![
                ("type", "\"http_proxy\""),
                ("cacheNamespace", "\"items\""),
                ("uri", "\"https://cached.example.com\""),
            ],
            expect_keys: vec!["cacheNamespace"],
        },
        OrderRow {
            case: "no ignored key",
            declared_method: "options",
            path: "/carried",
            members: vec![
                ("type", "\"http_proxy\""),
                ("uri", "\"https://carried.example.com\""),
                ("payloadFormatVersion", "\"1.0\""),
                ("timeoutInMillis", "12000"),
            ],
            expect_keys: vec![],
        },
    ]
}

#[tokio::test]
async fn ignored_extension_keys_are_reported_in_key_order_whatever_the_document_writes() {
    let router = emulator();

    for row in order_rows() {
        let case = row.case;
        let body = definition(case, row.declared_method, row.path, &row.members);
        let expected: Vec<String> = row
            .expect_keys
            .iter()
            .map(|key| ignored_key_advisory(row.path, row.declared_method, key))
            .collect();

        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ignored key must not fail the import, got {status} {imported}"
        );
        assert_eq!(
            import_info(&imported),
            expected,
            "case {case:?}: expected the ignored keys reported in API Gateway's order, got {:?}",
            import_info(&imported)
        );
        // An ignored key is no warning: the integration the rest of the extension
        // describes is created, and the route is wired to it. Read back through the
        // service's own readers, so nothing here is echoed from the request.
        assert!(
            warnings(&imported).is_empty(),
            "case {case:?}: an ignored key is no warning, got {:?}",
            warnings(&imported)
        );
        let api_id = as_str(&imported, "apiId");
        let integrations = stored_integrations(&router, &api_id).await;
        assert_eq!(
            integrations.len(),
            1,
            "case {case:?}: the integration must still be created, got {integrations:?}"
        );
        let integration_id = integrations.keys().next().cloned().unwrap_or_default();
        assert_eq!(
            stored_routes(&router, &api_id)
                .await
                .into_values()
                .collect::<Vec<Option<String>>>(),
            vec![Some(format!("integrations/{integration_id}"))],
            "case {case:?}: the route must be wired to the created integration"
        );

        // ReimportApi reads the same definition, so it reports the same list.
        let reimported_over = api_to_reimport_over(&router, &format!("{case} (reimported)")).await;
        let (status, reimported) = reimport(&router, &reimported_over, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ReimportApi, got {status} {reimported}"
        );
        assert_eq!(
            import_info(&reimported),
            expected,
            "case {case:?}: ReimportApi must report what ImportApi reports, got {:?}",
            import_info(&reimported)
        );
    }
}

/// Every ordering of `members`, so the report can be asserted to be independent of the
/// order the document writes the extension's keys in rather than of the two orders a table
/// can spell out.
fn permutations(members: &[(&'static str, &'static str)]) -> Vec<Vec<(&'static str, &'static str)>> {
    if members.len() <= 1 {
        return vec![members.to_vec()];
    }
    let mut orderings = Vec::new();
    for (index, member) in members.iter().enumerate() {
        let mut rest = members.to_vec();
        rest.remove(index);
        for tail in permutations(&rest) {
            let mut ordering = vec![*member];
            ordering.extend(tail);
            orderings.push(ordering);
        }
    }
    orderings
}

#[tokio::test]
async fn the_ignored_key_report_is_the_same_for_every_order_the_document_writes() {
    let router = emulator();

    // Five members — three of them ignored — give 120 orderings of one key set: the report
    // must be identical for all of them.
    let members: Vec<(&str, &str)> = vec![
        ("type", "\"http_proxy\""),
        ("uri", "\"https://upstream.example.com\""),
        ("responses", "{\"default\": {\"statusCode\": \"200\"}}"),
        ("cacheNamespace", "\"items\""),
        ("passthroughBehavior", "\"when_no_match\""),
    ];
    let path = "/permuted";
    let method = "get";
    let expected: Vec<String> = ["passthroughBehavior", "cacheNamespace", "responses"]
        .iter()
        .map(|key| ignored_key_advisory(path, method, key))
        .collect();

    for (ordering, written) in permutations(&members).into_iter().enumerate() {
        let case = format!("ordering {ordering}: {:?}", keys_of(&written));
        let body = definition("permuted", method, path, &written);
        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ImportApi, got {status} {imported}"
        );
        assert_eq!(
            import_info(&imported),
            expected,
            "case {case:?}: the report must not depend on the document's key order, got {:?}",
            import_info(&imported)
        );
    }
}

/// The keys of an extension as written, for naming a permutation case.
fn keys_of(members: &[(&'static str, &'static str)]) -> Vec<&'static str> {
    members.iter().map(|(key, _)| *key).collect()
}

/// One row of the integration-type table: the `type` the extension declares (as raw JSON,
/// or nothing at all), and the resource the warning must name.
struct TypeRow {
    case: &'static str,
    /// The raw JSON the extension writes as its `type`; `None` for an extension that
    /// writes no `type` member.
    declared_type: Option<&'static str>,
    /// The operation key the document declares, as written.
    declared_method: &'static str,
    /// The method the warning — and the route key — names for that operation key.
    route_method: &'static str,
    path: &'static str,
    /// Text the warning must NOT carry: the submitted spelling, which AWS does not report.
    must_not_name: Option<&'static str>,
}

/// The modelled `IntegrationType` catalog is `AWS | HTTP | MOCK | HTTP_PROXY | AWS_PROXY`
/// (`smithy_query(query="shape",shape="IntegrationType")`); every `type` below is outside
/// it, so none of them names an integration type.
fn type_rows() -> Vec<TypeRow> {
    vec![
        TypeRow {
            // The recorded case.
            case: "an unknown type name",
            declared_type: Some("\"BOGUS\""),
            declared_method: "get",
            route_method: "GET",
            path: "/pets",
            must_not_name: Some("BOGUS"),
        },
        TypeRow {
            case: "a plausible but unmodelled spelling",
            declared_type: Some("\"lambda\""),
            declared_method: "post",
            route_method: "POST",
            path: "/orders",
            must_not_name: Some("lambda"),
        },
        TypeRow {
            case: "a modelled value spelled with a hyphen",
            declared_type: Some("\"HTTP-PROXY\""),
            declared_method: "put",
            route_method: "PUT",
            path: "/items/{id}",
            must_not_name: Some("HTTP-PROXY"),
        },
        TypeRow {
            case: "an empty type",
            declared_type: Some("\"\""),
            declared_method: "delete",
            route_method: "DELETE",
            path: "/empty",
            must_not_name: ::std::option::Option::None,
        },
        TypeRow {
            case: "a type written as a number",
            declared_type: Some("7"),
            declared_method: "patch",
            route_method: "PATCH",
            path: "/numeric",
            must_not_name: Some("7"),
        },
        TypeRow {
            case: "a type written as an object",
            declared_type: Some("{\"name\": \"http_proxy\"}"),
            declared_method: "head",
            route_method: "HEAD",
            path: "/object",
            must_not_name: Some("http_proxy"),
        },
        TypeRow {
            case: "a null type",
            declared_type: Some("null"),
            declared_method: "options",
            route_method: "OPTIONS",
            path: "/voided",
            must_not_name: Some("null"),
        },
        TypeRow {
            case: "no type member at all, under the catch-all method",
            declared_type: ::std::option::Option::None,
            declared_method: "x-amazon-apigateway-any-method",
            route_method: "ANY",
            path: "/anything/{proxy+}",
            must_not_name: ::std::option::Option::None,
        },
    ]
}

/// The extension one type row submits: its `type`, when it declares one, beside the
/// endpoint member every row carries.
fn type_row_members(row: &TypeRow) -> Vec<(&'static str, &'static str)> {
    let mut members: Vec<(&'static str, &'static str)> =
        vec![("uri", "\"https://upstream.example.com\"")];
    if let Some(declared) = row.declared_type {
        members.insert(0, ("type", declared));
    }
    members
}

#[tokio::test]
async fn a_type_outside_the_catalog_is_reported_as_a_missing_integration_type() {
    let router = emulator();

    for row in type_rows() {
        let case = row.case;
        let members = type_row_members(&row);
        let body = definition(case, row.declared_method, row.path, &members);
        let expected = vec![unable_to_create(
            row.route_method,
            row.path,
            INTEGRATION_TYPE_REQUIRED,
        )];

        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the api must still be created, got {status} {imported}"
        );
        assert_eq!(
            warnings(&imported),
            expected,
            "case {case:?}: expected the missing-integration-type warning, got {:?}",
            warnings(&imported)
        );
        assert!(
            import_info(&imported).is_empty(),
            "case {case:?}: no key of this extension is ignored, got {:?}",
            import_info(&imported)
        );
        if let Some(named) = row.must_not_name {
            let reported = warnings(&imported).join(" ");
            assert!(
                !reported.contains(named),
                "case {case:?}: the warning must not name the submitted type {named:?}, got \
                 {reported:?}"
            );
        }

        // The api, the route with no target and the absence of the integration, read back
        // through the service's own readers.
        let api_id = as_str(&imported, "apiId");
        let (status, read_back) = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApi, got {status} {read_back}"
        );
        assert_eq!(
            stored_routes(&router, &api_id).await.into_iter().collect::<Vec<_>>(),
            vec![(
                format!("{} {}", row.route_method, row.path),
                ::std::option::Option::None
            )],
            "case {case:?}: the route must be imported with no target"
        );
        let integrations = stored_integrations(&router, &api_id).await;
        assert!(
            integrations.is_empty(),
            "case {case:?}: the skipped integration must not be created, got {integrations:?}"
        );

        // ReimportApi reads the same definition, so it warns with the same sentence.
        let reimported_over = api_to_reimport_over(&router, &format!("{case} (reimported)")).await;
        let (status, reimported) = reimport(&router, &reimported_over, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ReimportApi, got {status} {reimported}"
        );
        assert_eq!(
            warnings(&reimported),
            expected,
            "case {case:?}: ReimportApi must warn as ImportApi does, got {:?}",
            warnings(&reimported)
        );
    }
}

#[tokio::test]
async fn a_type_inside_the_catalog_is_not_reported_as_a_missing_integration_type() {
    let router = emulator();

    // A value the catalog DOES carry names an integration type: either the integration is
    // created, or it is reported on its own terms — never as a missing `IntegrationType`.
    let cases: Vec<(&str, &str, &str, &str, bool)> = vec![
        (
            "the proxy type an HTTP api carries",
            "\"http_proxy\"",
            "get",
            "/proxied",
            true,
        ),
        ("an AWS service integration", "\"aws\"", "post", "/aws", false),
        (
            "a non-proxy HTTP integration",
            "\"http\"",
            "put",
            "/http",
            false,
        ),
        (
            "a loopback integration",
            "\"mock\"",
            "delete",
            "/mock",
            false,
        ),
    ];

    for (case, declared, method, path, creates) in cases {
        let members: Vec<(&str, &str)> = vec![
            ("type", declared),
            ("uri", "\"https://upstream.example.com\""),
        ];
        let body = definition(case, method, path, &members);
        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the api must be created, got {status} {imported}"
        );
        let reported = warnings(&imported);
        if creates {
            assert!(
                reported.is_empty(),
                "case {case:?}: a carried type warns about nothing, got {reported:?}"
            );
            let api_id = as_str(&imported, "apiId");
            assert_eq!(
                stored_integrations(&router, &api_id).await.len(),
                1,
                "case {case:?}: the integration must be created"
            );
        } else {
            assert!(
                reported.iter().all(|warning| !warning.contains(INTEGRATION_TYPE_REQUIRED)),
                "case {case:?}: a type the catalog carries is not a missing type, got \
                 {reported:?}"
            );
            assert!(
                reported
                    .iter()
                    .all(|warning| warning.starts_with("Unable to create integration for resource")
                        && warning.ends_with(". Ignoring.")),
                "case {case:?}: the warning must keep the import's own frame, got {reported:?}"
            );
        }
    }
}

/// How many apis the store holds, through `GetApis`.
async fn stored_api_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "expected 200 from GetApis, got {status} {body}");
    body["items"].as_array().map(Vec::len).unwrap_or_default()
}

/// The refusal `FailOnWarnings` turns an import's warnings into: the fixed sentence, then
/// every warning on its own indented line.
fn warnings_found(reported: &[String]) -> String {
    let mut message = String::from("Warnings found during import:");
    for warning in reported {
        message.push_str("\n\t");
        message.push_str(warning);
    }
    message
}

#[tokio::test]
async fn fail_on_warnings_refuses_the_import_with_every_warning_it_found() {
    let router = emulator();
    let api_id = api_to_reimport_over(&router, "fail-on-warnings").await;
    let seed_routes = stored_routes(&router, &api_id).await;

    // The rows differ in HOW MANY warnings one definition produces, since the refusal
    // carries one line per warning: one operation whose extension names no type, then that
    // operation plus a second whose type is outside the catalog.
    let one: Vec<Operation<'static>> = vec![(
        "get",
        "/pets",
        vec![("uri", "\"https://pets.example.com\"")],
    )];
    let two: Vec<Operation<'static>> = vec![
        (
            "get",
            "/pets",
            vec![("uri", "\"https://pets.example.com\"")],
        ),
        (
            "post",
            "/orders",
            vec![
                ("type", "\"BOGUS\""),
                ("uri", "\"https://orders.example.com\""),
            ],
        ),
    ];

    for (case, operations) in [("one warning", one), ("two warnings", two)] {
        let body = definition_of(case, &operations);

        // The warnings the SAME body reports when nothing asks for a refusal are the lines
        // the refusal has to carry.
        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ImportApi, got {status} {imported}"
        );
        let reported = warnings(&imported);
        assert_eq!(
            reported.len(),
            operations.len(),
            "case {case:?}: expected one warning per operation, got {reported:?}"
        );
        let expected = warnings_found(&reported);
        assert_eq!(
            expected.matches("\n\t").count(),
            operations.len(),
            "case {case:?}: the refusal must carry one indented line per warning, got \
             {expected:?}"
        );

        let before = stored_api_count(&router).await;
        let (status, refused) = import(&router, "?failOnWarnings=true", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: FailOnWarnings must refuse the import, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            expected,
            "case {case:?}: expected the warnings-found refusal, got {refused}"
        );
        assert_eq!(
            stored_api_count(&router).await,
            before,
            "case {case:?}: a refused import must persist nothing"
        );

        // ReimportApi refuses the same body with the same message, and the api it was
        // called on keeps the routes it had.
        let (status, refused) = reimport(&router, &api_id, "?failOnWarnings=true", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: FailOnWarnings must refuse the reimport, got {status} {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            expected,
            "case {case:?}: ReimportApi must refuse as ImportApi does, got {refused}"
        );
        assert_eq!(
            stored_routes(&router, &api_id).await,
            seed_routes,
            "case {case:?}: a refused reimport must leave the api's routes as they were"
        );
    }
}
