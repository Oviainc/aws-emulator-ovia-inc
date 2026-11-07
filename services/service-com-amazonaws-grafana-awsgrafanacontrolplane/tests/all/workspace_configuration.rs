// SPDX-License-Identifier: CC0-1.0

//! The workspace configuration document, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: a workspace's `configuration` is the
//! `OverridableConfigurationJson` document the create, or the last
//! `UpdateWorkspaceConfiguration`, stored, and `DescribeWorkspaceConfiguration` reports
//! that record verbatim together with the Grafana version the record runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The version `CreateWorkspaceRequest$grafanaVersion` documents as the default
/// ("defaults to the latest version (for example, 10.4)"). Spelled out here,
/// independently of the constant the implementation carries.
const LATEST_GRAFANA_VERSION: &str = "10.4";

/// The document a workspace created without a `configuration` carries: the JSON
/// document that overrides nothing, and the shortest one `OverridableConfigurationJson`'s
/// `@length(min: 2, max: 65536)` admits.
const DEFAULT_CONFIGURATION: &str = "{}";

/// `OverridableConfigurationJson`'s published upper length bound, from
/// `smithy_query(query="shape", shape="OverridableConfigurationJson")`:
/// `@length(min: 2, max: 65536)`.
const CONFIGURATION_MAX_LEN: usize = 65536;

/// A SigV4 credential scope naming this service; it is what routes the request.
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

    /// The `configuration` member, or the empty string when the answer carries none.
    fn configuration(&self) -> &str {
        self.body["configuration"].as_str().unwrap_or_default()
    }

    /// The `grafanaVersion` member, or the empty string when the answer carries none.
    fn grafana_version(&self) -> &str {
        self.body["grafanaVersion"].as_str().unwrap_or_default()
    }
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

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/workspaces", Some(request)).await
}

async fn describe_workspace(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/workspaces/{id}"), None).await
}

async fn describe_configuration(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/workspaces/{id}/configuration"),
        None,
    )
    .await
}

async fn update_configuration(
    router: &MultiServiceRouter,
    id: &str,
    request: Value,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/workspaces/{id}/configuration"),
        Some(request),
    )
    .await
}

/// A create request carrying only the three required members, plus whatever `extra`
/// members a case needs.
fn create_request(extra: Value) -> Value {
    let mut request = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    });
    let object = request.as_object_mut().expect("the request is an object");
    for (key, value) in extra.as_object().expect("the extra members are an object") {
        object.insert(key.clone(), value.clone());
    }
    request
}

/// Wait out the span the clock owes a workspace its next status in, so the workspace has
/// SETTLED (`ACTIVE`, or gone when a delete was accepted) before the next call.
///
/// A workspace whose accepted change is still in flight refuses another one with
/// `ConflictException`, so a case that is not about that gate waits for the record to
/// settle first — exactly as a client driving two changes in a row has to. The transition
/// is owed by the clock, not by a call: nothing is done here but wait.
async fn settled() {
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;
}

/// Create a workspace and answer its id.
async fn new_workspace(router: &MultiServiceRouter, extra: Value) -> String {
    let created = create(router, create_request(extra)).await;
    assert_eq!(
        created.status, 202,
        "the fixture create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkspace must answer an id: {:?}", created.body))
        .to_string();
    // A workspace is `CREATING` when the create returns, and a change to a workspace
    // whose create is still in flight is refused: the fixture is handed over settled.
    settled().await;
    id
}

/// A JSON object document of exactly `len` bytes, for the length-bound cases.
fn configuration_of_len(len: usize) -> String {
    let envelope = "{\"k\":\"\"}";
    assert!(
        len >= envelope.len(),
        "case: a {len}-byte document cannot hold the envelope"
    );
    format!(
        "{{\"k\":\"{}\"}}",
        "a".repeat(len - envelope.len())
    )
}

/// Documents spanning what the rule generalises over: form (object, array, scalar,
/// string), nesting depth, member count, encoding (ASCII, escapes, non-ASCII) and size.
/// Every one of them is a JSON document, which is all `@mediaType("application/json")`
/// asks of the member.
fn round_trip_documents() -> Vec<(&'static str, String)> {
    vec![
        ("the empty object, at the minimum length", DEFAULT_CONFIGURATION.to_string()),
        (
            "a flat object",
            "{\"plugins\":{\"pluginAdminEnabled\":true}}".to_string(),
        ),
        (
            "several members, deeply nested",
            json!({
                "plugins": {"pluginAdminEnabled": false},
                "unified_alerting": {"enabled": true},
                "panels": {"disable_sanitize_html": false, "nested": {"deeper": [1, 2, 3]}},
            })
            .to_string(),
        ),
        ("a JSON array", "[1,2,3]".to_string()),
        ("a JSON scalar", "42".to_string()),
        ("a JSON string", "\"plain\"".to_string()),
        ("a JSON null", "null".to_string()),
        (
            "non-ASCII text and escapes",
            json!({"note": "日本語 — \"quoted\", back\\slash, tab\there"}).to_string(),
        ),
        (
            "insignificant whitespace, which is kept verbatim",
            "{ \"a\" : 1 ,\n  \"b\" : [ ] }".to_string(),
        ),
        (
            "a document at the published maximum length",
            configuration_of_len(CONFIGURATION_MAX_LEN),
        ),
    ]
}

/// `UpdateWorkspaceConfiguration` stores the document it was given and
/// `DescribeWorkspaceConfiguration` reports that record: the document comes back
/// byte-identical, for every form of document the member admits.
#[tokio::test]
async fn a_configuration_round_trips_through_update_and_describe() {
    let router = emulator();

    for (case, document) in round_trip_documents() {
        let id = new_workspace(&router, json!({})).await;

        let updated = update_configuration(&router, &id, json!({"configuration": document})).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: the update must be accepted, got {} with {:?}",
            updated.status, updated.body
        );

        let described = describe_configuration(&router, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the describe must succeed, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.configuration(),
            document.as_str(),
            "case {case:?}: expected the stored document back, got {:?}",
            described.configuration()
        );
        assert_eq!(
            serde_json::from_str::<Value>(described.configuration()).ok(),
            serde_json::from_str::<Value>(&document).ok(),
            "case {case:?}: the reported document must parse to the same JSON value, got {:?}",
            described.configuration()
        );
        assert_eq!(
            described.grafana_version(),
            LATEST_GRAFANA_VERSION,
            "case {case:?}: an update naming no version keeps the current one, got {:?}",
            described.grafana_version()
        );
    }
}

/// The reported document is whatever the record carries, so a workspace that was never
/// updated reports what the CREATE stored — and the document that overrides nothing when
/// the create named none.
#[tokio::test]
async fn a_created_workspace_reports_the_configuration_the_create_stored() {
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "no configuration and no version",
            json!({}),
            DEFAULT_CONFIGURATION,
            LATEST_GRAFANA_VERSION,
        ),
        (
            "a configuration and no version",
            json!({"configuration": "{\"plugins\":{\"pluginAdminEnabled\":true}}"}),
            "{\"plugins\":{\"pluginAdminEnabled\":true}}",
            LATEST_GRAFANA_VERSION,
        ),
        (
            "a configuration and a pinned version",
            json!({"configuration": "[\"a\",\"b\"]", "grafanaVersion": "9.4"}),
            "[\"a\",\"b\"]",
            "9.4",
        ),
        (
            "no configuration and a pinned version",
            json!({"grafanaVersion": "8.4"}),
            DEFAULT_CONFIGURATION,
            "8.4",
        ),
    ];

    let router = emulator();
    for (case, extra, configuration, version) in cases {
        let id = new_workspace(&router, extra).await;
        let described = describe_configuration(&router, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the describe must succeed, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.configuration(),
            configuration,
            "case {case:?}: expected the created document, got {:?}",
            described.configuration()
        );
        assert_eq!(
            described.grafana_version(),
            version,
            "case {case:?}: expected version {version}, got {:?}",
            described.grafana_version()
        );
    }
}

/// `grafanaVersion` "Can only be used to upgrade (for example, from 8.4 to 9.4), not
/// downgrade (for example, from 9.4 to 8.4)", and "If not specified, keeps the current
/// version of the workspace". The rows differ along the ordering relation between the two
/// versions, the number of components and whether the string is a numbered version at
/// all.
#[tokio::test]
async fn the_version_member_upgrades_only() {
    // (case, created version, requested version, accepted, version afterwards)
    let cases: Vec<(&str, &str, Option<&str>, bool, &str)> = vec![
        ("no version keeps the current one", "9.4", None, true, "9.4"),
        ("the same version is not a downgrade", "9.4", Some("9.4"), true, "9.4"),
        ("a minor upgrade", "8.4", Some("9.4"), true, "9.4"),
        ("a major upgrade", "9.4", Some("10.4"), true, "10.4"),
        (
            "components are ordered numerically, so 9.10 is later than 9.4",
            "9.4",
            Some("9.10"),
            true,
            "9.10",
        ),
        (
            "a longer version with the same prefix is later",
            "10.4",
            Some("10.4.1"),
            true,
            "10.4.1",
        ),
        ("a minor downgrade is refused", "9.4", Some("8.4"), false, "9.4"),
        ("a major downgrade is refused", "10.4", Some("9.4"), false, "10.4"),
        (
            "numeric ordering again: 9.4 is earlier than 9.10",
            "9.10",
            Some("9.4"),
            false,
            "9.10",
        ),
        (
            "dropping a component is a downgrade",
            "10.4.1",
            Some("10.4"),
            false,
            "10.4.1",
        ),
        (
            "an unnumbered version sorts below every numbered one, so it is refused",
            "9.4",
            Some("preview"),
            false,
            "9.4",
        ),
        (
            "and a numbered version is an upgrade from an unnumbered one",
            "preview",
            Some("9.4"),
            true,
            "9.4",
        ),
    ];

    let router = emulator();
    for (case, created, requested, accepted, expected) in cases {
        let id = new_workspace(&router, json!({"grafanaVersion": created})).await;
        let document = json!({"was": case}).to_string();

        let mut request = json!({"configuration": document});
        if let Some(version) = requested {
            request["grafanaVersion"] = json!(version);
        }
        let answer = update_configuration(&router, &id, request).await;

        let described = describe_configuration(&router, &id).await;
        if accepted {
            assert_eq!(
                answer.status, 202,
                "case {case:?}: the update must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                described.configuration(),
                document.as_str(),
                "case {case:?}: an accepted update stores the document, got {:?}",
                described.configuration()
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a downgrade must be refused, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                described.configuration(),
                DEFAULT_CONFIGURATION,
                "case {case:?}: a refused update must store nothing, got {:?}",
                described.configuration()
            );
        }
        assert_eq!(
            described.grafana_version(),
            expected,
            "case {case:?}: expected version {expected}, got {:?}",
            described.grafana_version()
        );
    }
}

/// `OverridableConfigurationJson` is `@mediaType("application/json")`: a `configuration`
/// that is not a JSON document is refused, by whichever operation accepts the member,
/// and nothing is stored. The rows span the ways a string fails to be JSON.
#[tokio::test]
async fn a_configuration_that_is_not_a_json_document_is_refused() {
    let cases: Vec<(&str, &str)> = vec![
        ("plain text", "not json at all"),
        ("an unterminated object", "{\"a\": 1"),
        ("an unterminated array", "[1, 2"),
        ("single-quoted keys, which JSON does not admit", "{'a': 1}"),
        ("a trailing comma", "{\"a\": 1,}"),
        ("two documents in one string", "{} {}"),
        ("a bare key with no value", "{\"a\"}"),
        ("an XML document", "<configuration/>"),
    ];

    let router = emulator();
    let id = new_workspace(&router, json!({})).await;

    for (case, document) in cases {
        let refused = update_configuration(&router, &id, json!({"configuration": document})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the update must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            describe_configuration(&router, &id).await.configuration(),
            DEFAULT_CONFIGURATION,
            "case {case:?}: a refused update must leave the stored document alone"
        );

        // The same member on CreateWorkspace carries the same shape, so it is refused
        // there too — and no workspace is minted for it.
        let refused = create(&router, create_request(json!({"configuration": document}))).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the create must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException from the create, got {:?}",
            refused.body
        );
    }
}

/// The published length bounds of `OverridableConfigurationJson` (`@length(min: 2, max:
/// 65536)`): each bound is admitted and a document one byte past the maximum is refused.
#[tokio::test]
async fn the_published_length_bounds_are_enforced() {
    let router = emulator();
    let id = new_workspace(&router, json!({})).await;

    // (case, document, accepted)
    let cases: Vec<(&str, String, bool)> = vec![
        ("the minimum length", DEFAULT_CONFIGURATION.to_string(), true),
        (
            "the maximum length",
            configuration_of_len(CONFIGURATION_MAX_LEN),
            true,
        ),
        (
            "one byte past the maximum",
            configuration_of_len(CONFIGURATION_MAX_LEN + 1),
            false,
        ),
    ];

    for (case, document, accepted) in cases {
        // Each accepted row leaves the workspace `UPDATING`, and the next change is
        // refused until the clock settles it.
        settled().await;
        let answer = update_configuration(&router, &id, json!({"configuration": document})).await;
        if accepted {
            assert_eq!(
                answer.status, 202,
                "case {case:?}: expected the update to be accepted, got {} with {:?}",
                answer.status, answer.error_code()
            );
            assert_eq!(
                describe_configuration(&router, &id).await.configuration(),
                document.as_str(),
                "case {case:?}: the document at a published bound must round-trip"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected the update to be refused, got {} with {:?}",
                answer.status, answer.error_code()
            );
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?}: `configuration` travels in the request body, so the \
                 service's request validator refuses it as a bad request, got {:?}",
                answer.body
            );
        }
    }
}

/// Both configuration operations answer `ResourceNotFoundException` for an id that names
/// no workspace — before and after the workspace existed — and name the id they could not
/// find.
#[tokio::test]
async fn an_id_that_names_no_workspace_is_a_404() {
    let router = emulator();
    let live = new_workspace(&router, json!({})).await;
    let deleted = new_workspace(&router, json!({})).await;
    let gone = call(&router, "DELETE", &format!("/workspaces/{deleted}"), None).await;
    assert_eq!(
        gone.status, 202,
        "the fixture delete must succeed, got {} with {:?}",
        gone.status, gone.body
    );
    // The delete is ACCEPTED, not finished: the workspace reports `DELETING` until the
    // removal the clock owes the store falls due, and only from then on does its id name
    // nothing.
    settled().await;

    // Ids matching `WorkspaceId`'s `^g-[0-9a-f]{10}$` that no workspace carries.
    let cases: Vec<(&str, String)> = vec![
        ("an id nothing ever minted", "g-0123456789".to_string()),
        ("the all-zero id", "g-0000000000".to_string()),
        ("the all-f id", "g-ffffffffff".to_string()),
        ("a deleted workspace", deleted.clone()),
    ];

    for (case, id) in cases {
        for (operation, answer) in [
            (
                "DescribeWorkspaceConfiguration",
                describe_configuration(&router, &id).await,
            ),
            (
                "UpdateWorkspaceConfiguration",
                update_configuration(&router, &id, json!({"configuration": "{\"a\":1}"})).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: {operation} must answer 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: {operation} expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceId"].as_str().unwrap_or_default(),
                id.as_str(),
                "case {case:?}: {operation} must name the id it could not find, got {:?}",
                answer.body
            );
        }
    }

    // The workspace that does exist still answers, so the 404s above are about the id.
    assert_eq!(
        describe_configuration(&router, &live).await.status,
        200,
        "the live workspace must still report its configuration"
    );
}

/// The update is ACCEPTED (202), not finished: the workspace reports `UPDATING` to a
/// caller acting straight away and the clock owes it `ACTIVE`, while the new document is
/// readable at once.
#[tokio::test]
async fn an_accepted_update_leaves_the_workspace_updating_until_the_clock_owes_it_active() {
    let router = emulator();
    let id = new_workspace(&router, json!({})).await;
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200));
    assert_eq!(
        describe_workspace(&router, &id).await.body["workspace"]["status"],
        json!("ACTIVE"),
        "the fixture workspace must settle to ACTIVE before the update"
    );

    let document = "{\"unified_alerting\":{\"enabled\":true}}";
    let started = Instant::now();
    let updated = update_configuration(&router, &id, json!({"configuration": document})).await;
    assert_eq!(
        updated.status, 202,
        "the update must be accepted, got {} with {:?}",
        updated.status, updated.body
    );

    // The status read is the FIRST call after the 202, so the transient state is checked
    // on the shortest possible interval; the guard is only there to keep the test honest
    // on a machine slow enough to have let the whole span pass between the two calls.
    let status_now = describe_workspace(&router, &id).await.body["workspace"]["status"].clone();
    let configuration_now = describe_configuration(&router, &id).await;
    if started.elapsed() < LIFECYCLE_TRANSITION {
        assert_eq!(
            status_now,
            json!("UPDATING"),
            "a caller acting inside the transition span still sees UPDATING, got {status_now:?}"
        );
    }
    assert_eq!(
        configuration_now.configuration(),
        document,
        "the new document is readable at once, got {:?}",
        configuration_now.configuration()
    );

    // The transition is owed by the clock, not by a call: nothing is done here but wait.
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200));
    assert_eq!(
        describe_workspace(&router, &id).await.body["workspace"]["status"],
        json!("ACTIVE"),
        "once the transition span has passed the workspace is ACTIVE again"
    );
}

/// `DescribeWorkspaceConfiguration` reports: repeating it changes nothing a later caller
/// sees, and it never mints, moves or modifies the workspace.
#[tokio::test]
async fn describing_the_configuration_changes_nothing() {
    let router = emulator();
    let id = new_workspace(&router, json!({"configuration": "{\"a\":1}"})).await;
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200));

    let before = describe_workspace(&router, &id).await.body["workspace"].clone();
    let first = describe_configuration(&router, &id).await;

    for round in 1..=3 {
        let again = describe_configuration(&router, &id).await;
        assert_eq!(
            again.body, first.body,
            "round {round}: every describe must answer the same, got {:?}",
            again.body
        );
    }

    let after = describe_workspace(&router, &id).await.body["workspace"].clone();
    assert_eq!(
        before, after,
        "describing the configuration must leave the workspace record untouched"
    );
    assert_eq!(
        call(&router, "GET", "/workspaces", None).await.body["workspaces"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "describing the configuration must not mint a workspace"
    );
}
