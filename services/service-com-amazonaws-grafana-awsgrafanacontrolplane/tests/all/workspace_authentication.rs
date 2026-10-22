// SPDX-License-Identifier: CC0-1.0

//! A workspace's authentication methods, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes.
//!
//! The rule under test: `DescribeWorkspaceAuthentication` reports the authentication the
//! workspace record holds — `providers` is the stored authentication-provider list,
//! `saml` is reported exactly when SAML is one of those providers (carrying the stored
//! SAML configuration, with `status` `CONFIGURED` only once one has been stored and
//! `NOT_CONFIGURED` otherwise) and `awsSso` exactly when `AWS_SSO` is one of them; a
//! workspace id that names no workspace is refused `ResourceNotFoundException` (404).
//! `UpdateWorkspaceAuthentication` is the only writer of that record, so each row below
//! reads back through the describe what a create, or a later authentication update,
//! stored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The two values `AuthenticationProviderTypes` admits, from the Smithy model
/// (`smithy_query(query="shape",shape="AuthenticationProviderTypes")`): every provider
/// set a case below uses is drawn from this catalog, and the table covers each value
/// alone and both together.
const AUTHENTICATION_PROVIDER_TYPES: [&str; 2] = ["AWS_SSO", "SAML"];

/// The two values `SamlConfigurationStatus` admits, from the Smithy model: `CONFIGURED`
/// ("SAML on an AMG workspace is enabled and has been configured") and `NOT_CONFIGURED`
/// ("enabled but has not been configured").
const SAML_CONFIGURATION_STATUSES: [&str; 2] = ["CONFIGURED", "NOT_CONFIGURED"];

/// A SigV4 credential scope naming this service; it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

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

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
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

/// `CreateWorkspace` with the three required members and the given provider list.
async fn create_with(router: &MultiServiceRouter, providers: &[&str]) -> String {
    let answer = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "CUSTOMER_MANAGED",
            "authenticationProviders": providers,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "CreateWorkspace with providers {providers:?} must answer 202, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkspace must answer an id: {:?}", answer.body))
        .to_string()
}

async fn describe_authentication(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/workspaces/{id}/authentication"),
        None,
    )
    .await
}

async fn update_authentication(
    router: &MultiServiceRouter,
    id: &str,
    request: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/workspaces/{id}/authentication"),
        Some(request),
    )
    .await
}

async fn describe_workspace(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/workspaces/{id}"), None).await
}

/// One row of the table: a workspace created with `create_providers`, then zero or more
/// `UpdateWorkspaceAuthentication` requests, and what the authentication must then be.
struct Case {
    name: &'static str,
    create_providers: Vec<&'static str>,
    updates: Vec<Value>,
    expected_providers: Vec<&'static str>,
    /// The `saml.status` expected, or `None` when no `saml` structure may be reported.
    expected_saml_status: Option<&'static str>,
    /// Pointers into `saml.configuration` and the values expected there; empty means no
    /// `configuration` may be reported.
    expected_saml_configuration: Vec<(&'static str, Value)>,
    expect_aws_sso: bool,
}

/// A SAML configuration whose IdP metadata is a URL, with every optional member set.
fn saml_configuration_by_url() -> Value {
    json!({
        "idpMetadata": {"url": "https://idp.example.com/metadata.xml"},
        "assertionAttributes": {
            "login": "login",
            "email": "mail",
            "name": "displayName",
            "groups": "groups",
            "role": "role",
            "org": "org",
        },
        "roleValues": {"admin": ["grafana-admins"], "editor": ["grafana-editors", "sre"]},
        "allowedOrganizations": ["example-org", "example-org-2"],
        "loginValidityDuration": 120,
    })
}

/// A SAML configuration whose IdP metadata is inline XML, with no optional member set.
fn saml_configuration_by_xml() -> Value {
    json!({
        "idpMetadata": {
            "xml": "<EntityDescriptor entityID=\"https://idp.example.com\"/>",
        },
    })
}

/// `DescribeWorkspaceAuthentication` reports what the create, or a later
/// `UpdateWorkspaceAuthentication`, stored for the workspace.
///
/// The rows differ along what the rule generalises over: which providers, how many of
/// them, whether a SAML configuration has been stored at all, which `idpMetadata`
/// variant it carries, how many optional members it sets, and whether a later update
/// adds or drops a provider.
#[tokio::test]
async fn the_described_authentication_is_the_one_that_was_stored() {
    let cases = vec![
        Case {
            name: "SAML only, created and never configured",
            create_providers: vec!["SAML"],
            updates: vec![],
            expected_providers: vec!["SAML"],
            expected_saml_status: Some("NOT_CONFIGURED"),
            expected_saml_configuration: vec![],
            expect_aws_sso: false,
        },
        Case {
            name: "IAM Identity Center only",
            create_providers: vec!["AWS_SSO"],
            updates: vec![],
            expected_providers: vec!["AWS_SSO"],
            expected_saml_status: None,
            expected_saml_configuration: vec![],
            expect_aws_sso: true,
        },
        Case {
            name: "both providers, SAML not configured yet",
            create_providers: vec!["AWS_SSO", "SAML"],
            updates: vec![],
            expected_providers: vec!["AWS_SSO", "SAML"],
            expected_saml_status: Some("NOT_CONFIGURED"),
            expected_saml_configuration: vec![],
            expect_aws_sso: true,
        },
        Case {
            name: "SAML configured from IdP metadata at a URL",
            create_providers: vec!["SAML"],
            updates: vec![json!({
                "authenticationProviders": ["SAML"],
                "samlConfiguration": saml_configuration_by_url(),
            })],
            expected_providers: vec!["SAML"],
            expected_saml_status: Some("CONFIGURED"),
            expected_saml_configuration: vec![
                (
                    "/idpMetadata/url",
                    json!("https://idp.example.com/metadata.xml"),
                ),
                ("/assertionAttributes/login", json!("login")),
                ("/assertionAttributes/org", json!("org")),
                ("/roleValues/admin", json!(["grafana-admins"])),
                ("/roleValues/editor", json!(["grafana-editors", "sre"])),
                (
                    "/allowedOrganizations",
                    json!(["example-org", "example-org-2"]),
                ),
                ("/loginValidityDuration", json!(120)),
            ],
            expect_aws_sso: false,
        },
        Case {
            name: "SAML configured from inline IdP metadata XML, replacing IAM Identity Center",
            create_providers: vec!["AWS_SSO"],
            updates: vec![json!({
                "authenticationProviders": ["SAML"],
                "samlConfiguration": saml_configuration_by_xml(),
            })],
            expected_providers: vec!["SAML"],
            expected_saml_status: Some("CONFIGURED"),
            expected_saml_configuration: vec![(
                "/idpMetadata/xml",
                json!("<EntityDescriptor entityID=\"https://idp.example.com\"/>"),
            )],
            expect_aws_sso: false,
        },
        Case {
            name: "a second update adds IAM Identity Center and keeps the stored SAML config",
            create_providers: vec!["SAML"],
            updates: vec![
                json!({
                    "authenticationProviders": ["SAML"],
                    "samlConfiguration": saml_configuration_by_url(),
                }),
                json!({"authenticationProviders": ["SAML", "AWS_SSO"]}),
            ],
            expected_providers: vec!["SAML", "AWS_SSO"],
            expected_saml_status: Some("CONFIGURED"),
            expected_saml_configuration: vec![(
                "/idpMetadata/url",
                json!("https://idp.example.com/metadata.xml"),
            )],
            expect_aws_sso: true,
        },
        Case {
            name: "an update that drops SAML stops reporting the SAML structure",
            create_providers: vec!["SAML"],
            updates: vec![
                json!({
                    "authenticationProviders": ["SAML"],
                    "samlConfiguration": saml_configuration_by_xml(),
                }),
                json!({"authenticationProviders": ["AWS_SSO"]}),
            ],
            expected_providers: vec!["AWS_SSO"],
            expected_saml_status: None,
            expected_saml_configuration: vec![],
            expect_aws_sso: true,
        },
    ];

    let router = emulator();
    for case in cases {
        let name = case.name;
        for provider in case
            .create_providers
            .iter()
            .chain(case.expected_providers.iter())
        {
            assert!(
                AUTHENTICATION_PROVIDER_TYPES.contains(provider),
                "case {name:?}: the table must only use modelled providers, got {provider:?}"
            );
        }
        if let Some(status) = case.expected_saml_status {
            assert!(
                SAML_CONFIGURATION_STATUSES.contains(&status),
                "case {name:?}: the table must only expect modelled statuses, got {status:?}"
            );
        }

        let id = create_with(&router, &case.create_providers).await;

        let mut last_update: Option<Value> = None;
        for (index, request) in case.updates.iter().enumerate() {
            let updated = update_authentication(&router, &id, request.clone()).await;
            assert_eq!(
                updated.status, 200,
                "case {name:?}: update {index} must answer HTTP 200, got {} with {:?}",
                updated.status, updated.body
            );
            last_update = Some(updated.body["authentication"].clone());
        }

        let described = describe_authentication(&router, &id).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected HTTP 200, got {} with {:?}",
            described.status, described.body
        );
        let authentication = described.body["authentication"].clone();

        assert_eq!(
            at(&authentication, "/providers"),
            json!(case.expected_providers),
            "case {name:?}: expected providers {:?}, got {:?}",
            case.expected_providers,
            at(&authentication, "/providers")
        );

        match case.expected_saml_status {
            Some(status) => {
                assert_eq!(
                    at(&authentication, "/saml/status"),
                    json!(status),
                    "case {name:?}: expected saml.status {status:?}, got {:?}",
                    at(&authentication, "/saml/status")
                );
            }
            None => assert_eq!(
                at(&authentication, "/saml"),
                Value::Null,
                "case {name:?}: a workspace that does not authenticate with SAML reports no \
                 saml structure, got {:?}",
                at(&authentication, "/saml")
            ),
        }

        if case.expected_saml_configuration.is_empty() {
            assert_eq!(
                at(&authentication, "/saml/configuration"),
                Value::Null,
                "case {name:?}: no SAML configuration was stored, so none may be reported, \
                 got {:?}",
                at(&authentication, "/saml/configuration")
            );
        }
        for (pointer, expected) in &case.expected_saml_configuration {
            let full = format!("/saml/configuration{pointer}");
            assert_eq!(
                &at(&authentication, &full),
                expected,
                "case {name:?}: expected {full} to be {expected:?}, got {:?}",
                at(&authentication, &full)
            );
        }

        let aws_sso = at(&authentication, "/awsSso");
        assert_eq!(
            aws_sso.is_object(),
            case.expect_aws_sso,
            "case {name:?}: expected an awsSso structure: {}, got {aws_sso:?}",
            case.expect_aws_sso
        );

        // The update answers "the user authentication for this workspace after the
        // update is made", so the last one must agree with what the store now reports.
        if let Some(answered) = last_update {
            assert_eq!(
                answered, authentication,
                "case {name:?}: the update's answer and the describe's answer must be the \
                 same record, got {answered:?} and {authentication:?}"
            );
        }

        // The same record drives the `AuthenticationSummary` the workspace operations
        // report, so the two projections cannot disagree.
        let summary = describe_workspace(&router, &id).await;
        let workspace = summary.body["workspace"].clone();
        assert_eq!(
            at(&workspace, "/authentication/providers"),
            json!(case.expected_providers),
            "case {name:?}: DescribeWorkspace must report the same providers, got {:?}",
            at(&workspace, "/authentication/providers")
        );
        assert_eq!(
            at(&workspace, "/authentication/samlConfigurationStatus"),
            case.expected_saml_status
                .map(|status| json!(status))
                .unwrap_or(Value::Null),
            "case {name:?}: DescribeWorkspace must report the same SAML status, got {:?}",
            at(&workspace, "/authentication/samlConfigurationStatus")
        );
    }
}

/// Reading the authentication changes nothing a later caller sees: repeated describes
/// answer the same record, and the workspace's `modified` instant does not move.
#[tokio::test]
async fn describing_the_authentication_changes_nothing() {
    let router = emulator();
    let id = create_with(&router, &["SAML", "AWS_SSO"]).await;
    let modified_before = at(&describe_workspace(&router, &id).await.body["workspace"], "/modified");

    let first = describe_authentication(&router, &id).await;
    for round in 0..3 {
        let again = describe_authentication(&router, &id).await;
        assert_eq!(
            again.status, 200,
            "round {round}: expected HTTP 200, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "round {round}: a read must answer the same record every time, got {:?}",
            again.body
        );
    }

    let modified_after = at(&describe_workspace(&router, &id).await.body["workspace"], "/modified");
    assert_eq!(
        modified_before, modified_after,
        "a read must not bump the workspace's modified instant, got {modified_before:?} \
         then {modified_after:?}"
    );
}

/// An authentication update is persisted for the workspace it names and for no other.
#[tokio::test]
async fn an_authentication_update_reaches_only_the_workspace_it_names() {
    let router = emulator();
    let target = create_with(&router, &["SAML"]).await;
    let bystander = create_with(&router, &["SAML"]).await;

    let updated = update_authentication(
        &router,
        &target,
        json!({
            "authenticationProviders": ["SAML"],
            "samlConfiguration": saml_configuration_by_xml(),
        }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "expected HTTP 200, got {} with {:?}",
        updated.status, updated.body
    );

    for (case, id, expected) in [
        ("the updated workspace", &target, "CONFIGURED"),
        ("the bystander", &bystander, "NOT_CONFIGURED"),
    ] {
        let described = describe_authentication(&router, id).await;
        assert_eq!(
            at(&described.body["authentication"], "/saml/status"),
            json!(expected),
            "case {case:?}: expected saml.status {expected:?}, got {:?}",
            at(&described.body["authentication"], "/saml/status")
        );
    }
}

/// A workspace id the model's `^g-[0-9a-f]{10}$` admits but no workspace has is a 404 on
/// both authentication operations; one the pattern does not admit is a 400.
#[tokio::test]
async fn an_id_that_names_no_workspace_is_refused_by_kind() {
    let cases = [
        ("the lowest well-formed id", "g-0000000000", 404, "ResourceNotFoundException"),
        ("the highest well-formed id", "g-ffffffffff", 404, "ResourceNotFoundException"),
        ("another well-formed id nothing has", "g-0123456789", 404, "ResourceNotFoundException"),
        ("not a workspace id at all", "not-a-workspace", 400, "ValidationException"),
        ("one hex digit too few", "g-012345678", 400, "ValidationException"),
        ("one hex digit too many", "g-0123456789a", 400, "ValidationException"),
        ("hex digits out of range", "g-00000000zz", 400, "ValidationException"),
    ];

    let router = emulator();
    // One real workspace, so a refusal cannot be an artefact of an empty store.
    let stored = create_with(&router, &["SAML"]).await;

    for (case, id, status, code) in cases {
        for (operation, answer) in [
            ("DescribeWorkspaceAuthentication", describe_authentication(&router, id).await),
            (
                "UpdateWorkspaceAuthentication",
                update_authentication(&router, id, json!({"authenticationProviders": ["SAML"]}))
                    .await,
            ),
        ] {
            assert_eq!(
                answer.status, status,
                "case {case:?}: {operation} on {id:?} must answer HTTP {status}, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: {operation} on {id:?} must answer {code}, got {:?}",
                answer.body
            );
            if status == 404 {
                assert_eq!(
                    answer.body["resourceId"].as_str().unwrap_or_default(),
                    id,
                    "case {case:?}: {operation} must name the id it could not find, got {:?}",
                    answer.body
                );
            }
        }
    }

    // None of those refusals touched the workspace that does exist.
    let described = describe_authentication(&router, &stored).await;
    assert_eq!(
        at(&described.body["authentication"], "/saml/status"),
        json!("NOT_CONFIGURED"),
        "a refused call must leave the stored workspace's authentication alone, got {:?}",
        described.body
    );
}
