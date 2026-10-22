// SPDX-License-Identifier: CC0-1.0

//! `ListVersions`, end to end over HTTP.
//!
//! The rule under test: `ListVersions` reports the service's catalogue of supported
//! Grafana versions, in one stable order, paginated by `maxResults` / `nextToken`; when
//! the request names a workspace it reports instead the catalogue entries STRICTLY NEWER
//! — compared as numbers, not as bytes — than the version that workspace persists, and a
//! workspace id that names nothing is a 404.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Grafana versions Amazon Managed Grafana supports, oldest first.
///
/// SOURCE: this service's own model docs, which name the supported majors —
/// `CreateWorkspaceRequest$grafanaVersion` ("defaults to the latest version (for example,
/// 10.4)") and `UpdateWorkspaceConfigurationRequest$grafanaVersion` ("Can only be used to
/// upgrade (for example, from 8.4 to 9.4)"). Spelled out here rather than imported from
/// `src/state.rs`, so a wrong catalogue in the implementation fails instead of agreeing
/// with itself.
const SUPPORTED_VERSIONS: [&str; 3] = ["8.4", "9.4", "10.4"];

/// The largest page `ListVersionsRequest$maxResults` admits (`@range(min: 1, max: 100)`),
/// spelled out here for the same reason.
const MAX_RESULTS_CEILING: i32 = 100;

/// The smallest page that same `@range` admits.
const MAX_RESULTS_FLOOR: i32 = 1;

/// A SigV4 credential scope naming this service: it is what routes a request here.
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

    /// The `grafanaVersions` the answer carries, as owned strings.
    fn versions(&self) -> Vec<String> {
        self.body["grafanaVersions"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .unwrap_or_else(|| {
                                panic!("grafanaVersions must carry strings, got {item:?}")
                            })
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The pagination token, if the answer carries one.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
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

async fn list_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/versions{query}"), None).await
}

/// A workspace running `version` (or the service default when `None`), as
/// `CreateWorkspace` stores it. Returns its id.
async fn workspace_running(router: &MultiServiceRouter, version: Option<&str>) -> String {
    let mut request = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    });
    if let Some(version) = version {
        request["grafanaVersion"] = json!(version);
    }
    let created = call(router, "POST", "/workspaces", Some(request)).await;
    // `CreateWorkspace` is modelled with a 202 success status: the workspace is minted
    // CREATING and the clock owes it ACTIVE.
    assert_eq!(
        created.status, 202,
        "case {version:?}: the create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {version:?}: the create must report an id"))
        .to_string()
}

/// The version `DescribeWorkspace` reports for `id` — the round trip that proves what the
/// catalogue filter is computed from.
async fn stored_version(router: &MultiServiceRouter, id: &str) -> String {
    let described = call(router, "GET", &format!("/workspaces/{id}"), None).await;
    assert_eq!(
        described.status, 200,
        "describing {id} must succeed, got {} with {:?}",
        described.status, described.body
    );
    described.body["workspace"]["grafanaVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("the description of {id} must carry grafanaVersion"))
        .to_string()
}

/// With no workspace named, `ListVersions` publishes the whole supported catalogue, and
/// reporting it changes nothing a later caller sees.
#[tokio::test]
async fn list_versions_publishes_the_supported_catalog() {
    let router = emulator();

    let answer = list_versions(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "an unpaged ListVersions must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.versions(),
        SUPPORTED_VERSIONS,
        "an unpaged ListVersions must report every supported version, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.next_token(),
        None,
        "a page that served the whole catalogue must carry no nextToken, got {:?}",
        answer.body
    );

    // The same question asked twice gets the same answer: this is a report, not a draw.
    let again = list_versions(&router, "").await;
    assert_eq!(
        again.versions(),
        answer.versions(),
        "a second ListVersions must report the same catalogue, got {:?}",
        again.body
    );

    // And the catalogue's newest entry is the version a create with no `grafanaVersion`
    // runs ("defaults to the latest version"), read back through DescribeWorkspace.
    let id = workspace_running(&router, None).await;
    let latest = SUPPORTED_VERSIONS
        .last()
        .expect("the catalogue is not empty");
    let stored = stored_version(&router, &id).await;
    assert_eq!(
        &stored.as_str(),
        latest,
        "a create naming no version must run the newest supported version"
    );
}

/// `@paginated(items: grafanaVersions)`: whatever `maxResults` it is given, following
/// `nextToken` to the end enumerates the served set exactly once in one stable order, no
/// page is longer than was asked for, and a token this operation never issued is refused.
#[tokio::test]
async fn list_versions_pages_over_one_stable_order() {
    let router = emulator();
    // A workspace on the oldest supported version, so the paginated set differs from the
    // catalogue in both length and content.
    let oldest = SUPPORTED_VERSIONS
        .first()
        .expect("the catalogue is not empty");
    let id = workspace_running(&router, Some(oldest)).await;
    let upgrades: Vec<String> = SUPPORTED_VERSIONS[1..]
        .iter()
        .map(|version| (*version).to_string())
        .collect();
    let catalog: Vec<String> = SUPPORTED_VERSIONS
        .iter()
        .map(|version| (*version).to_string())
        .collect();

    let scopes: [(&str, String, Vec<String>); 2] = [
        ("the whole catalogue", String::new(), catalog),
        (
            "one workspace's upgrade targets",
            format!("&workspace-id={id}"),
            upgrades,
        ),
    ];

    for (case, scope, expected) in scopes {
        // Both published bounds of `@range(min: 1, max: 100)`, plus the sizes between
        // that cut the served set into a different number of pages.
        for page_size in [MAX_RESULTS_FLOOR, 2, 3, MAX_RESULTS_CEILING] {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let query = match &token {
                    Some(token) => format!("?maxResults={page_size}&nextToken={token}{scope}"),
                    None => format!("?maxResults={page_size}{scope}"),
                };
                let answer = list_versions(&router, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} maxResults={page_size}: expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let page = answer.versions();
                assert!(
                    page.len() <= page_size as usize,
                    "case {case:?} maxResults={page_size}: a page carried {} versions: {page:?}",
                    page.len()
                );
                walked.extend(page);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?} maxResults={page_size}: pagination did not terminate, \
                     walked {walked:?}"
                );
                match answer.next_token() {
                    Some(next) => token = Some(next),
                    None => break,
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?} maxResults={page_size}: walking every page must enumerate the \
                 served versions once, in the same order"
            );
        }

        // A token this operation never issued is a bad request, not an empty page.
        for bad in ["not-a-version", "0.1", "99.9"] {
            let refused = list_versions(&router, &format!("?nextToken={bad}{scope}")).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} nextToken={bad:?}: an unissued token must be refused, got {} \
                 with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?} nextToken={bad:?}: expected ValidationException, got {:?}",
                refused.body
            );
        }

        // One past each published bound of `maxResults` is refused at the boundary.
        for bad in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
            let refused = list_versions(&router, &format!("?maxResults={bad}{scope}")).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} maxResults={bad}: a value outside @range(1, 100) must be \
                 refused, got {} with {:?}",
                refused.status, refused.body
            );
        }
    }
}

/// With a workspace named, `ListVersions` reports the versions that workspace "can be
/// upgraded" to: the supported versions strictly newer than the one it persists, compared
/// as numbers rather than as bytes.
#[tokio::test]
async fn list_versions_for_a_workspace_reports_only_newer_versions() {
    // (case, the version the workspace is created with, the versions it can upgrade to)
    let cases: [(&str, Option<&str>, &[&str]); 6] = [
        ("the oldest supported version", Some("8.4"), &["9.4", "10.4"]),
        // A byte comparison would order "10.4" before "9.4" and report no upgrade here.
        ("a middle supported version", Some("9.4"), &["10.4"]),
        ("the newest supported version", Some("10.4"), &[]),
        ("no version named, so the newest", None, &[]),
        (
            "older than every supported version",
            Some("7.5"),
            &["8.4", "9.4", "10.4"],
        ),
        ("newer than every supported version", Some("11.4"), &[]),
    ];

    let router = emulator();
    for (case, created_with, expected) in cases {
        let id = workspace_running(&router, created_with).await;

        // The filter is computed from the persisted record, so pin that record first.
        let stored = stored_version(&router, &id).await;
        if let Some(version) = created_with {
            assert_eq!(
                stored, version,
                "case {case:?}: the create must persist the version it was given, got \
                 {stored:?}"
            );
        }

        let answer = list_versions(&router, &format!("?workspace-id={id}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.versions(),
            expected,
            "case {case:?}: a workspace on {stored:?} must be upgradable to exactly \
             {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            !answer.versions().iter().any(|version| version == &stored),
            "case {case:?}: the version a workspace already runs is not an upgrade, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {case:?}: a page that served the whole set must carry no nextToken, got {:?}",
            answer.body
        );
    }
}

/// A workspace id the model's `^g-[0-9a-f]{10}$` admits but no workspace has is a 404; one
/// the pattern does not admit is a 400.
#[tokio::test]
async fn list_versions_for_an_unknown_workspace_is_refused_by_kind() {
    let cases = [
        (
            "a well-formed id nothing has",
            "g-0123456789",
            404,
            "ResourceNotFoundException",
        ),
        (
            "another well-formed id nothing has",
            "g-ffffffffff",
            404,
            "ResourceNotFoundException",
        ),
        (
            "not a workspace id at all",
            "not-a-workspace",
            400,
            "ValidationException",
        ),
        ("too few hex digits", "g-0123", 400, "ValidationException"),
        (
            "too many hex digits",
            "g-0123456789a",
            400,
            "ValidationException",
        ),
    ];

    let router = emulator();
    // One real workspace, so a refusal cannot be an artefact of an empty store.
    let stored = workspace_running(&router, Some("8.4")).await;

    for (case, id, status, code) in cases {
        let answer = list_versions(&router, &format!("?workspace-id={id}")).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
    }

    // The workspace that does exist is still answered.
    let answer = list_versions(&router, &format!("?workspace-id={stored}")).await;
    assert_eq!(
        answer.status, 200,
        "the workspace that exists must still be answered, got {} with {:?}",
        answer.status, answer.body
    );
}
