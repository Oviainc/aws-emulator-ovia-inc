// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two component-surface reads a Greengrass core device and a
//! deployment client depend on: `ListComponentVersions` and `ResolveComponentCandidates`.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body. Nothing reaches into the state struct, so a handler that
//! answered from a canned body instead of from what `CreateComponentVersion` persisted
//! fails here.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::state::{
    COMPONENT_VALIDATION_SPAN, MAX_RESOLVE_CANDIDATES, MAX_RESOLVE_REQUIREMENTS,
};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one signed request and return its status and parsed JSON body.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Vec<u8>) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("a serializable request body");
    call(router, "POST", path, bytes).await
}

/// `ResolveComponentCandidates` as the ONE caller class AWS authorizes for it reaches it:
/// a Greengrass core device on the data-plane endpoint, whose identity is its IoT device
/// certificate, so the request carries no IAM credential in either SigV4 carrier.
///
/// A request that DOES carry one is refused 403 before any component is looked at, which
/// is the rule `core_device_data_plane_authorization.rs` pins.
async fn resolve(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://greengrass.{REGION}.amazonaws.com/greengrass/v2/resolveComponentCandidates"
        ))
        .header("content-type", "application/json")
        .body(Bytes::from(
            serde_json::to_vec(body).expect("a serializable request body"),
        ))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

async fn get(router: &MultiServiceRouter, path: &str) -> (u16, Value) {
    call(router, "GET", path, Vec::new()).await
}

/// Publish one component version from a recipe document.
async fn publish(router: &MultiServiceRouter, recipe: &str) -> (u16, Value) {
    post(
        router,
        "/greengrass/v2/createComponentVersion",
        &json!({ "inlineRecipe": base64::encode(recipe.as_bytes()) }),
    )
    .await
}

/// A minimal YAML recipe for `name`@`version`. The three keys are the ones the IoT
/// Greengrass component recipe reference requires.
fn recipe(name: &str, version: &str) -> String {
    format!(
        "RecipeFormatVersion: '2020-01-25'\nComponentName: {name}\nComponentVersion: {version}\n"
    )
}

/// A recipe for `name`@`version` whose single manifest declares `attributes` as its
/// platform (the recipe reference's `Manifests[].Platform`).
fn platform_recipe(name: &str, version: &str, attributes: &[(&str, &str)]) -> String {
    let mut document = recipe(name, version);
    document.push_str("Manifests:\n  - Platform:\n");
    for (key, value) in attributes {
        document.push_str(&format!("      {key}: {value}\n"));
    }
    document.push_str("    Lifecycle:\n      Run: echo hello\n");
    document
}

/// The name-level component ARN this emulator mints for `name`.
fn component_arn(name: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}")
}

/// The component-version ARN this emulator mints for `name`@`version`.
fn version_arn(name: &str, version: &str) -> String {
    format!("{}:versions:{version}", component_arn(name))
}

/// Percent-encode the colons in an ARN, the way an AWS SDK encodes an `@httpLabel`.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A")
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The array member `key` of a JSON object, or an empty vector when absent.
fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `componentVersion` of every entry of a `componentVersions` list, in the order the
/// response carries them.
fn listed_versions(body: &Value) -> Vec<String> {
    array(body, "componentVersions")
        .iter()
        .map(|entry| text(entry, "componentVersion"))
        .collect()
}

// ---------------------------------------------------------------------------
// ListComponentVersions
// ---------------------------------------------------------------------------

/// The versions one component is published with below, and the order
/// `ListComponentVersions` must report them in.
///
/// The rows differ along what semantic-version precedence generalises over: patch,
/// minor and major steps, a two-digit major (which byte order would sort before `2.…`),
/// and a prerelease (which precedes its own release).
const VERSIONS: &[&str] = &[
    "1.0.0", "1.0.1", "1.2.0", "2.0.0", "2.0.0-alpha.1", "10.0.0",
];

/// The same versions greatest-first, which is the order the operation documents
/// ("Greater versions are listed first").
const VERSIONS_GREATEST_FIRST: &[&str] = &[
    "10.0.0", "2.0.0", "2.0.0-alpha.1", "1.2.0", "1.0.1", "1.0.0",
];

#[tokio::test]
async fn list_component_versions_reports_every_published_version_greatest_first() {
    let router = emulator();
    let name = "com.example.Versions";
    for version in VERSIONS {
        let (status, body) = publish(&router, &recipe(name, version)).await;
        assert_eq!(
            status, 201,
            "case {version:?}: publishing must answer 201, got {status} {body}"
        );
    }

    let (status, body) = get(
        &router,
        &format!(
            "/greengrass/v2/components/{}/versions",
            label(&component_arn(name))
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListComponentVersions must answer 200, got {status} {body}"
    );
    assert_eq!(
        listed_versions(&body),
        VERSIONS_GREATEST_FIRST,
        "the listing must be greatest-version-first, got {body}"
    );
    for entry in array(&body, "componentVersions") {
        let version = text(&entry, "componentVersion");
        assert_eq!(
            text(&entry, "componentName"),
            name,
            "case {version:?}: every entry names its component, got {entry}"
        );
        assert_eq!(
            text(&entry, "arn"),
            version_arn(name, &version),
            "case {version:?}: every entry carries the minted version ARN, got {entry}"
        );
    }
}

#[tokio::test]
async fn list_component_versions_pages_in_that_order_without_losing_or_repeating_one() {
    let router = emulator();
    let name = "com.example.Paged";
    for version in VERSIONS {
        publish(&router, &recipe(name, version)).await;
    }
    let path = format!(
        "/greengrass/v2/components/{}/versions",
        label(&component_arn(name))
    );

    // Page sizes spanning one entry per page, a partial last page, exactly the whole
    // collection, and more than it holds.
    for page_size in [1_usize, 2, 4, VERSIONS.len(), VERSIONS.len() + 3] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match &token {
                Some(token) => format!("{path}?maxResults={page_size}&nextToken={token}"),
                None => format!("{path}?maxResults={page_size}"),
            };
            let (status, body) = get(&router, &uri).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: page {pages} must answer 200, got {status} {body}"
            );
            let page = listed_versions(&body);
            assert!(
                page.len() <= page_size,
                "case {page_size:?}: page {pages} must hold at most {page_size} entries, \
                 got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= VERSIONS.len() + 2,
                "case {page_size:?}: paging must terminate, got {seen:?}"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, VERSIONS_GREATEST_FIRST,
            "case {page_size:?}: paging must yield every version once, in order"
        );
    }
}

#[tokio::test]
async fn list_component_versions_of_a_component_with_no_versions_is_an_empty_list() {
    let router = emulator();
    let published = "com.example.Present";
    publish(&router, &recipe(published, "1.0.0")).await;
    let deleted = "com.example.Deleted";
    publish(&router, &recipe(deleted, "1.0.0")).await;
    let (status, _) = call(
        &router,
        "DELETE",
        &format!(
            "/greengrass/v2/components/{}",
            label(&version_arn(deleted, "1.0.0"))
        ),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 204, "the delete must answer 204, got {status}");

    // (case, the component ARN listed)
    //
    // The rows span the ways a well-formed component ARN can name nothing this account
    // published: a component never published, one whose last version was deleted, and
    // the same component under another region and another account.
    let rows = [
        ("never published", component_arn("com.example.Absent")),
        ("every version deleted", component_arn(deleted)),
        (
            "another region's ARN for a published component",
            format!("arn:aws:greengrass:eu-west-1:{ACCOUNT}:components:{published}"),
        ),
        (
            "another account's ARN for a published component",
            format!("arn:aws:greengrass:{REGION}:210987654321:components:{published}"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = get(
            &router,
            &format!("/greengrass/v2/components/{}/versions", label(&arn)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            listed_versions(&body),
            Vec::<String>::new(),
            "case {case:?}: expected an empty componentVersions, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: an empty listing carries no cursor, got {body}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    let name = "com.example.Cursor";
    publish(&router, &recipe(name, "1.0.0")).await;
    let path = format!(
        "/greengrass/v2/components/{}/versions",
        label(&component_arn(name))
    );

    // Tokens that are not the hex-encoded resume key this service issues.
    for case in ["not-a-token", "zz", "%2B0001"] {
        let (status, body) = get(&router, &format!("{path}?nextToken={case}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

// ---------------------------------------------------------------------------
// How the component surface words the component it did not find
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_component_read_names_the_component_and_the_account_it_looked_in() {
    let router = emulator();
    publish(&router, &recipe("com.example.Worded", "1.0.0")).await;

    // (case, the component name and version addressed)
    //
    // The rows differ along what the message templates fill: a component that was
    // never published, one that exists at another version, and a name and version
    // whose characters a template must carry through unchanged.
    let rows = [
        ("never published", "com.example.Missing", "1.0.0"),
        ("another version of a published component", "com.example.Worded", "2.0.0"),
        ("a name and version with punctuation", "com.example.Odd-Name_1", "0.1.0-beta.2+build.7"),
    ];

    for (case, name, version) in rows {
        let encoded = label(&version_arn(name, version));

        // Each operation words its own answer, as Greengrass does: the component and
        // the account it was looked for in.
        let (status, body) = get(
            &router,
            &format!("/greengrass/v2/components/{encoded}/metadata"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeComponent expected 404, got {status} {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Component ({name}:{version}) does not exist in account {ACCOUNT}."),
            "case {case:?}: DescribeComponent must name the component and the account, \
             got {body}"
        );

        let (status, body) = get(&router, &format!("/greengrass/v2/components/{encoded}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetComponent expected 404, got {status} {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!(
                "PRIVATE component ({name}:{version}) under account {ACCOUNT} does not exist"
            ),
            "case {case:?}: GetComponent must name the scope it searched, got {body}"
        );

        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/greengrass/v2/components/{encoded}"),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: DeleteComponent expected 404, got {status} {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Component [{name} : {version}] in the accountId [{ACCOUNT}] does not exist"),
            "case {case:?}: DeleteComponent must name the component and the account, got \
             {body}"
        );
    }
}

#[tokio::test]
async fn republishing_a_version_names_the_state_the_stored_one_is_in() {
    // The fixed text below is the recorded ConflictException body's, which brackets the
    // component, the account AND the state:
    //   Component [<name> : <version>] for account [<account>] already exists with
    //   state: [<state>]
    // What this test varies is the one part the emulator computes: the state, which the
    // clock moves from REQUESTED to DEPLOYABLE over the validation span.
    let router = emulator();
    let name = "com.example.Duplicate";
    let document = recipe(name, "1.0.0");
    let (status, _) = publish(&router, &document).await;
    assert_eq!(status, 201, "the first publish must answer 201");

    // Straight after the publish the stored version is still in the non-terminal state
    // the create reported, and the conflict names that state.
    let (status, body) = publish(&router, &document).await;
    assert_eq!(status, 409, "expected 409, got {status} {body}");
    assert_eq!(
        text(&body, "message"),
        format!(
            "Component [{name} : 1.0.0] for account [{ACCOUNT}] already exists with state: \
             [REQUESTED]"
        ),
        "the conflict must name the stored version's state, got {body}"
    );

    // Once the validation window the clock owes has elapsed, the same conflict names
    // the state the stored version has moved to.
    std::thread::sleep(COMPONENT_VALIDATION_SPAN * 2);
    let (status, body) = publish(&router, &document).await;
    assert_eq!(status, 409, "expected 409, got {status} {body}");
    assert_eq!(
        text(&body, "message"),
        format!(
            "Component [{name} : 1.0.0] for account [{ACCOUNT}] already exists with state: \
             [DEPLOYABLE]"
        ),
        "the conflict must name the state the clock moved the version to, got {body}"
    );
}


/// One `ResolveComponentCandidates` row: the version requirements a candidate carries
/// and the version they must resolve to.
struct CandidateCase {
    label: &'static str,
    /// The version the core device already holds, if the candidate names one.
    candidate_version: Option<&'static str>,
    /// `requester -> semantic-version requirement`, as a dependent component's recipe
    /// states it.
    requirements: &'static [(&'static str, &'static str)],
    /// The version that must resolve, or `None` when the requirements resolve to
    /// nothing.
    resolved: Option<&'static str>,
}

/// Requirement forms from the semantic-version requirement grammar IoT Greengrass uses
/// for `versionRequirements`, each stated over the versions in [`VERSIONS`].
const CANDIDATES: &[CandidateCase] = &[
    CandidateCase {
        label: "no requirement at all resolves the greatest version",
        candidate_version: None,
        requirements: &[],
        resolved: Some("10.0.0"),
    },
    CandidateCase {
        label: "the version the device names, with no requirement",
        candidate_version: Some("1.0.1"),
        requirements: &[],
        resolved: Some("1.0.1"),
    },
    CandidateCase {
        label: "an exact requirement",
        candidate_version: None,
        requirements: &[("com.example.Nucleus", "=1.2.0")],
        resolved: Some("1.2.0"),
    },
    CandidateCase {
        label: "a lower bound resolves the greatest version above it",
        candidate_version: None,
        requirements: &[("com.example.Nucleus", ">=1.0.1")],
        resolved: Some("10.0.0"),
    },
    CandidateCase {
        label: "an upper bound",
        candidate_version: None,
        requirements: &[("com.example.Nucleus", "<2.0.0")],
        resolved: Some("1.2.0"),
    },
    CandidateCase {
        label: "two requirements that overlap in one version",
        candidate_version: None,
        requirements: &[
            ("com.example.Nucleus", ">=1.0.1"),
            ("com.example.Logger", "<1.2.0"),
        ],
        resolved: Some("1.0.1"),
    },
    CandidateCase {
        label: "a caret requirement keeps the major",
        candidate_version: None,
        requirements: &[("com.example.Nucleus", "^1.0.0")],
        resolved: Some("1.2.0"),
    },
    CandidateCase {
        label: "a requirement no published version satisfies",
        candidate_version: None,
        requirements: &[("com.example.Nucleus", ">=99.0.0")],
        resolved: None,
    },
    CandidateCase {
        label: "requirements that conflict with each other",
        candidate_version: None,
        requirements: &[
            ("com.example.Nucleus", ">2.0.0"),
            ("com.example.Logger", "<2.0.0"),
        ],
        resolved: None,
    },
];

/// The `componentCandidates` member for one case.
fn candidate_member(name: &str, case: &CandidateCase) -> Value {
    let mut candidate = serde_json::Map::new();
    candidate.insert("componentName".to_string(), json!(name));
    if let Some(version) = case.candidate_version {
        candidate.insert("componentVersion".to_string(), json!(version));
    }
    if !case.requirements.is_empty() {
        let mut requirements = serde_json::Map::new();
        for (requester, requirement) in case.requirements {
            requirements.insert((*requester).to_string(), json!(requirement));
        }
        candidate.insert("versionRequirements".to_string(), Value::Object(requirements));
    }
    json!([Value::Object(candidate)])
}

#[tokio::test]
async fn resolve_component_candidates_picks_the_greatest_version_every_requirement_admits() {
    let router = emulator();
    let name = "com.example.Resolved";
    for version in VERSIONS {
        publish(&router, &recipe(name, version)).await;
    }

    for case in CANDIDATES {
        let (status, body) = resolve(
            &router,
            &json!({ "componentCandidates": candidate_member(name, case) }),
        )
        .await;
        match case.resolved {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected 200, got {status} {body}",
                    case.label
                );
                let resolved = array(&body, "resolvedComponentVersions");
                assert_eq!(
                    resolved.len(),
                    1,
                    "case {:?}: expected one resolved version, got {body}",
                    case.label
                );
                let entry = &resolved[0];
                assert_eq!(
                    text(entry, "componentVersion"),
                    expected,
                    "case {:?}: expected version {expected}, got {entry}",
                    case.label
                );
                assert_eq!(
                    text(entry, "componentName"),
                    name,
                    "case {:?}: expected the candidate's name, got {entry}",
                    case.label
                );
                assert_eq!(
                    text(entry, "arn"),
                    version_arn(name, expected),
                    "case {:?}: expected the minted version ARN, got {entry}",
                    case.label
                );
                // The recipe the core device installs is the one that was published,
                // byte for byte.
                let blob = base64::decode(text(entry, "recipe")).expect("a base64 recipe blob");
                assert_eq!(
                    blob,
                    recipe(name, expected).as_bytes(),
                    "case {:?}: expected the stored recipe, got {:?}",
                    case.label,
                    String::from_utf8_lossy(&blob)
                );
                assert_eq!(
                    text(entry, "vendorGuidance"),
                    "ACTIVE",
                    "case {:?}: a published version is current, got {entry}",
                    case.label
                );
            }
            None => {
                assert!(
                    (400..500).contains(&status),
                    "case {:?}: expected a client error, got {status} {body}",
                    case.label
                );
                assert!(
                    !error_code(&body).contains("InternalServerException"),
                    "case {:?}: expected a modelled client error, got {}",
                    case.label,
                    error_code(&body)
                );
            }
        }
    }
}

#[tokio::test]
async fn resolve_component_candidates_resolves_every_candidate_the_request_names() {
    let router = emulator();
    for (name, version) in [
        ("com.example.First", "1.0.0"),
        ("com.example.Second", "2.5.0"),
        ("com.example.Third", "0.1.0"),
    ] {
        publish(&router, &recipe(name, version)).await;
    }

    let (status, body) = resolve(
        &router,
        &json!({
            "componentCandidates": [
                { "componentName": "com.example.First" },
                { "componentName": "com.example.Second", "versionRequirements": { "com.example.First": ">=2.0.0" } },
                { "componentName": "com.example.Third", "componentVersion": "0.1.0" },
            ]
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    let resolved: Vec<(String, String)> = array(&body, "resolvedComponentVersions")
        .iter()
        .map(|entry| (text(entry, "componentName"), text(entry, "componentVersion")))
        .collect();
    assert_eq!(
        resolved,
        vec![
            ("com.example.First".to_string(), "1.0.0".to_string()),
            ("com.example.Second".to_string(), "2.5.0".to_string()),
            ("com.example.Third".to_string(), "0.1.0".to_string()),
        ],
        "every candidate must resolve, in the order the request named them, got {body}"
    );

    // An empty candidate list resolves nothing, and is not an error.
    let (status, body) = resolve(&router, &json!({ "componentCandidates": [] })).await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    assert_eq!(
        array(&body, "resolvedComponentVersions").len(),
        0,
        "no candidates resolve to no versions, got {body}"
    );
}

#[tokio::test]
async fn resolve_component_candidates_only_resolves_a_version_the_platform_supports() {
    let router = emulator();
    let name = "com.example.Platform";
    // One component published three times: a platform-independent version, a Linux
    // aarch64 one and a Windows one. Each manifest's platform attributes come from the
    // recipe reference's `Manifests[].Platform` (`os` and `architecture`).
    publish(&router, &recipe(name, "1.0.0")).await;
    publish(
        &router,
        &platform_recipe(name, "2.0.0", &[("os", "linux"), ("architecture", "aarch64")]),
    )
    .await;
    publish(&router, &platform_recipe(name, "3.0.0", &[("os", "windows")])).await;

    // (case, the platform attributes the core device reports, the version that resolves)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a Linux aarch64 device",
            json!({ "attributes": { "os": "linux", "architecture": "aarch64" } }),
            Some("2.0.0"),
        ),
        (
            "a Linux x86 device, which the aarch64 manifest does not support",
            json!({ "attributes": { "os": "linux", "architecture": "x86_64" } }),
            Some("1.0.0"),
        ),
        (
            "a Windows device",
            json!({ "attributes": { "os": "windows", "architecture": "x86_64" } }),
            Some("3.0.0"),
        ),
        (
            "a device whose platform names an attribute no manifest does",
            json!({ "attributes": { "os": "darwin" } }),
            Some("1.0.0"),
        ),
        (
            "no platform at all, which constrains nothing",
            Value::Null,
            Some("3.0.0"),
        ),
    ];

    for (case, platform, expected) in rows {
        let mut request = serde_json::Map::new();
        request.insert(
            "componentCandidates".to_string(),
            json!([{ "componentName": name }]),
        );
        if !platform.is_null() {
            request.insert("platform".to_string(), platform.clone());
        }
        let (status, body) = resolve(&router, &Value::Object(request)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let resolved = array(&body, "resolvedComponentVersions");
        let actual = resolved.first().map(|entry| text(entry, "componentVersion"));
        assert_eq!(
            actual.as_deref(),
            expected,
            "case {case:?}: expected version {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_candidate_no_published_component_answers_is_reported_as_missing() {
    let router = emulator();
    publish(&router, &recipe("com.example.Known", "1.0.0")).await;

    let (status, body) = resolve(
        &router,
        &json!({ "componentCandidates": [{ "componentName": "com.example.Unknown" }] }),
    )
    .await;
    assert_eq!(status, 404, "expected 404, got {status} {body}");
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {}",
        error_code(&body)
    );

    // More candidates than one request may carry is bad input too: resolving each one
    // walks the versions of the component it names.
    let too_many: Vec<Value> = (0..=MAX_RESOLVE_CANDIDATES)
        .map(|index| json!({ "componentName": format!("com.example.Bulk{index}") }))
        .collect();
    let (status, body) = resolve(&router, &json!({ "componentCandidates": too_many })).await;
    assert_eq!(
        status, 400,
        "case \"one candidate past the bound\": expected 400, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case \"one candidate past the bound\": expected ValidationException, got {}",
        error_code(&body)
    );

    // Exactly the bound is still served, and the first candidate decides the answer:
    // none of these components is published, so it is reported as missing.
    let at_the_bound: Vec<Value> = (0..MAX_RESOLVE_CANDIDATES)
        .map(|index| json!({ "componentName": format!("com.example.Bulk{index}") }))
        .collect();
    let (status, body) = resolve(&router, &json!({ "componentCandidates": at_the_bound })).await;
    assert_eq!(
        status, 404,
        "case \"exactly the bound\": expected 404, got {status} {body}"
    );

    // More version requirements in total than one request may carry is bad input for
    // the same reason: each one is checked against every stored version.
    let mut requirements = serde_json::Map::new();
    for index in 0..=MAX_RESOLVE_REQUIREMENTS {
        requirements.insert(format!("com.example.Requester{index}"), json!(">=1.0.0"));
    }
    let (status, body) = resolve(
        &router,
        &json!({
            "componentCandidates": [{
                "componentName": "com.example.Known",
                "versionRequirements": Value::Object(requirements),
            }]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one requirement past the bound\": expected 400, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case \"one requirement past the bound\": expected ValidationException, got {}",
        error_code(&body)
    );

    // A requirement that is not a semantic-version requirement is bad input, not a
    // missing resource.
    for case in ["not-a-requirement", ">=", "1.0.0 || 2.0.0"] {
        let (status, body) = resolve(
            &router,
            &json!({
                "componentCandidates": [{
                    "componentName": "com.example.Known",
                    "versionRequirements": { "com.example.Nucleus": case },
                }]
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}
