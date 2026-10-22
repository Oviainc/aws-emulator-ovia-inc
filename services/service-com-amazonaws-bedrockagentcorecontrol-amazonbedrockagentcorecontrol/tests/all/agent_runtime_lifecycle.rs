// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the agent-runtime lifecycle
//! (`CreateAgentRuntime`, `GetAgentRuntime`, `ListAgentRuntimes`,
//! `UpdateAgentRuntime`, `DeleteAgentRuntime`).
//!
//! Every test drives the service the way the router does: an `http::Request`
//! through [`MultiServiceRouter::dispatch`], so the wire protocol
//! (`restJson1`), the boundary constraint validation and the handlers are all
//! exercised. The store is the one shared engine, so what one request writes is
//! what the next request reads.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for unless a case names another.
const REGION: &str = "us-east-1";

/// `LifecycleConfiguration`'s documented defaults: "Default: 900 seconds (15
/// minutes)" for `idleRuntimeSessionTimeout` and "Default: 28800 seconds (8
/// hours)" for `maxLifetime` (`smithy_query doc shape=LifecycleConfiguration`).
const DEFAULT_LIFECYCLE: fn() -> Value =
    || json!({"idleRuntimeSessionTimeout": 900, "maxLifetime": 28_800});

/// The settle span `src/state.rs` gives the `CREATING`/`UPDATING` -> `READY`
/// transition, plus slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // `default()` registers no authenticator, so the tests exercise routing and
    // the handlers rather than this process's SigV4 keys.
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the
/// router routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(region))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "PUT", "/runtimes/", REGION, Some(body)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/runtimes/{id}/"), REGION, None).await
}

async fn get_version(router: &MultiServiceRouter, id: &str, version: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/runtimes/{id}/?version={version}"),
        REGION,
        None,
    )
    .await
}

async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    send(router, "POST", &format!("/runtimes/{query}"), REGION, None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `AgentRuntimeId`'s pattern, `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`
/// (`smithy_query shape=AgentRuntimeId`), checked by hand because this crate
/// carries no regex engine.
fn is_modelled_runtime_id(id: &str) -> bool {
    let Some((head, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        && !head.is_empty()
        && head.len() <= 100
        && head.starts_with(|c: char| c.is_ascii_alphabetic())
        && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True when `later` is at or after `earlier`, whichever form the protocol
/// rendered the timestamp in: epoch seconds compare numerically, and an RFC 3339
/// instant compares lexically (that encoding sorts in chronological order).
fn timestamp_at_or_after(later: &Value, earlier: &Value) -> bool {
    match (later, earlier) {
        (Value::Number(later), Value::Number(earlier)) => {
            later.as_f64().unwrap_or_default() >= earlier.as_f64().unwrap_or_default()
        }
        (Value::String(later), Value::String(earlier)) => later >= earlier,
        _ => false,
    }
}

/// A container artifact whose URI satisfies `RuntimeContainerUri`'s ECR pattern.
fn container_artifact(tag: &str) -> Value {
    json!({"containerConfiguration": {
        "containerUri": format!("public.ecr.aws/agentcore/demo:{tag}")
    }})
}

/// A code artifact: `CodeConfiguration` needs `code`, a `runtime` from
/// `AgentManagedRuntimeType` and 1..2 entry points.
fn code_artifact(entry_point: &str) -> Value {
    json!({"codeConfiguration": {
        "code": {"s3": {"bucket": "agentcore-artifacts", "prefix": "agents/demo.zip"}},
        "runtime": "PYTHON_3_12",
        "entryPoint": [entry_point],
    }})
}

/// One row's request body is compared member by member against what
/// `GetAgentRuntime` reports, so the assertion is the rule itself: a read
/// returns what was stored.
///
/// An empty map or list carries nothing to store, so it may be reported as
/// absent.
fn echoed(expected: &Value, actual: &Value) -> bool {
    match expected {
        Value::Object(members) if members.is_empty() => actual.is_null() || actual == expected,
        Value::Array(items) if items.is_empty() => actual.is_null() || actual == expected,
        _ => actual == expected,
    }
}

/// The create requests the echo rule is asserted over. The rows differ along
/// what the rule generalises over: the artifact union variant, which optional
/// configuration members are present, the size of the maps and lists, and the
/// length of the name.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "required members only, container artifact",
            json!({
                "agentRuntimeName": "minimal_agent",
                "agentRuntimeArtifact": container_artifact("v1"),
                "roleArn": "arn:aws:iam::000000000000:role/AgentRole",
            }),
        ),
        (
            "code artifact, description, three environment variables",
            json!({
                "agentRuntimeName": "code_agent",
                "agentRuntimeArtifact": code_artifact("app.handler"),
                "roleArn": "arn:aws:iam::000000000000:role/CodeAgentRole",
                "description": "an agent built from code in S3",
                "environmentVariables": {"LOG_LEVEL": "debug", "REGION_HINT": "x", "TIER": "2"},
                "protocolConfiguration": {"serverProtocol": "MCP"},
                "platformVersion": "V2",
            }),
        ),
        (
            "vpc network mode, one environment variable, filesystem mount",
            json!({
                "agentRuntimeName": "vpc_agent",
                "agentRuntimeArtifact": container_artifact("vpc"),
                "roleArn": "arn:aws:iam::000000000000:role/VpcAgentRole",
                "description": "runs inside a VPC",
                "networkConfiguration": {
                    "networkMode": "VPC",
                    "networkModeConfig": {
                        "securityGroups": ["sg-0123456789abcdef0"],
                        "subnets": ["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                    },
                },
                "environmentVariables": {"ONLY": "one"},
                "filesystemConfigurations": [{"sessionStorage": {"mountPath": "/mnt/data"}}],
                "lifecycleConfiguration": {"idleRuntimeSessionTimeout": 60, "maxLifetime": 3600},
                "requestHeaderConfiguration": {"requestHeaderAllowlist": ["X-Amzn-Trace-Id"]},
            }),
        ),
        (
            "empty collections and a client token",
            json!({
                "agentRuntimeName": "empty_collections_agent",
                "agentRuntimeArtifact": code_artifact("main.run"),
                "roleArn": "arn:aws:iam::000000000000:role/EmptyAgentRole",
                "environmentVariables": {},
                "filesystemConfigurations": [],
                "tags": {"team": "agents"},
                "clientToken": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            }),
        ),
        (
            "name at the 48-character bound of AgentRuntimeName",
            json!({
                // `AgentRuntimeName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`: 48 chars.
                "agentRuntimeName": "a234567890123456789012345678901234567890_2345678",
                "agentRuntimeArtifact": container_artifact("bound"),
                "roleArn": "arn:aws:iam::000000000000:role/BoundAgentRole",
                "lifecycleConfiguration": {"idleRuntimeSessionTimeout": 900},
            }),
        ),
    ]
}

#[tokio::test]
async fn create_persists_a_runtime_that_get_reports_back() {
    let router = router();
    for (case, request) in create_cases() {
        let (status, created) = create(&router, &request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from CreateAgentRuntime, got {status} / {created}"
        );

        let id = created["agentRuntimeId"].as_str().unwrap_or_default();
        let name = request["agentRuntimeName"].as_str().expect("a name");
        assert!(
            is_modelled_runtime_id(id),
            "case {case:?}: expected an agentRuntimeId matching \
             ^[a-zA-Z][a-zA-Z0-9_]{{0,99}}-[a-zA-Z0-9]{{10}}$, got {id:?}"
        );
        assert!(
            id.starts_with(&format!("{name}-")),
            "case {case:?}: expected the id to be derived from the name {name:?}, got {id:?}"
        );
        assert_eq!(
            created["agentRuntimeArn"],
            json!(format!(
                "arn:aws:bedrock-agentcore:{REGION}:{}:runtime/{id}",
                engine::EMULATOR_ACCOUNT_ID
            )),
            "case {case:?}: expected the ARN to name the minted runtime, got {}",
            created["agentRuntimeArn"]
        );
        assert_eq!(
            created["agentRuntimeVersion"],
            json!("1"),
            "case {case:?}: expected the first version to be \"1\", got {}",
            created["agentRuntimeVersion"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: expected a freshly created runtime to report CREATING, got {}",
            created["status"]
        );
        assert!(
            !created["createdAt"].is_null(),
            "case {case:?}: expected a createdAt timestamp, got {}",
            created["createdAt"]
        );
        assert!(
            created["workloadIdentityDetails"]["workloadIdentityArn"]
                .as_str()
                .is_some_and(|arn| arn.contains("workload-identity")),
            "case {case:?}: expected a workload identity ARN, got {}",
            created["workloadIdentityDetails"]
        );

        let (status, got) = get(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAgentRuntime, got {status} / {got}"
        );
        for (member, expected) in request.as_object().expect("an object body") {
            // `tags` and `clientToken` are not members of the Get response.
            if matches!(member.as_str(), "tags" | "clientToken") {
                continue;
            }
            let expected = if member == "lifecycleConfiguration" {
                // Each member the caller left out comes back at the default the
                // model documents.
                let mut merged = DEFAULT_LIFECYCLE();
                for (key, value) in expected.as_object().expect("a lifecycle object") {
                    merged[key] = value.clone();
                }
                merged
            } else {
                expected.clone()
            };
            assert!(
                echoed(&expected, &got[member]),
                "case {case:?}: expected GetAgentRuntime to report the stored {member} \
                 {expected}, got {}",
                got[member]
            );
        }
        if request.get("lifecycleConfiguration").is_none() {
            assert_eq!(
                got["lifecycleConfiguration"],
                DEFAULT_LIFECYCLE(),
                "case {case:?}: expected the documented lifecycle defaults, got {}",
                got["lifecycleConfiguration"]
            );
        }
        assert_eq!(
            got["agentRuntimeId"], created["agentRuntimeId"],
            "case {case:?}: expected the stored id, got {}",
            got["agentRuntimeId"]
        );
        assert_eq!(
            got["agentRuntimeArn"], created["agentRuntimeArn"],
            "case {case:?}: expected the stored ARN, got {}",
            got["agentRuntimeArn"]
        );
        assert_eq!(
            got["createdAt"], created["createdAt"],
            "case {case:?}: expected the stored createdAt, got {}",
            got["createdAt"]
        );
        assert_eq!(
            got["lastUpdatedAt"], created["createdAt"],
            "case {case:?}: expected lastUpdatedAt to start at createdAt, got {}",
            got["lastUpdatedAt"]
        );
    }
}

#[tokio::test]
async fn the_arn_carries_the_region_the_request_was_signed_for() {
    // The ARN region segment is the SigV4 credential scope's region; these are
    // three regions from different partitions of the public grammar `[a-z0-9-]+`.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = router();
        let request = json!({
            "agentRuntimeName": "region_agent",
            "agentRuntimeArtifact": container_artifact("region"),
            "roleArn": "arn:aws:iam::000000000000:role/RegionAgentRole",
        });
        let (status, created) =
            send(&router, "PUT", "/runtimes/", region, Some(&request)).await;
        assert_eq!(
            status, 202,
            "case {region:?}: expected 202 from CreateAgentRuntime, got {status} / {created}"
        );
        let id = created["agentRuntimeId"].as_str().unwrap_or_default();
        assert_eq!(
            created["agentRuntimeArn"],
            json!(format!(
                "arn:aws:bedrock-agentcore:{region}:{}:runtime/{id}",
                engine::EMULATOR_ACCOUNT_ID
            )),
            "case {region:?}: expected the ARN to carry the signed region, got {}",
            created["agentRuntimeArn"]
        );
    }
}

#[tokio::test]
async fn list_pages_over_every_persisted_runtime() {
    let router = router();
    let mut expected = BTreeSet::new();
    for index in 0..5 {
        let request = json!({
            "agentRuntimeName": format!("listed_agent_{index}"),
            "agentRuntimeArtifact": container_artifact("list"),
            "roleArn": "arn:aws:iam::000000000000:role/ListedAgentRole",
            "description": format!("runtime number {index}"),
        });
        let (status, created) = create(&router, &request).await;
        assert_eq!(status, 202, "creating runtime {index}: got {status} / {created}");
        expected.insert(created["agentRuntimeId"].as_str().unwrap_or_default().to_string());
    }

    // `maxResults` has the modelled range 1..100; absent means one page holds
    // everything the store has (there are fewer than 100 runtimes here).
    for page_size in [None, Some(1), Some(2), Some(3), Some(5), Some(100)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen = Vec::new();
        let mut query = page_size.map_or_else(String::new, |size| format!("?maxResults={size}"));
        let mut pages = 0;
        loop {
            let (status, body) = list(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ListAgentRuntimes, got {status} / {body}"
            );
            let page = body["agentRuntimes"].as_array().expect("agentRuntimes is a list");
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: expected at most {size} summaries on a page, got {}",
                    page.len()
                );
            }
            for summary in page {
                for member in [
                    "agentRuntimeArn",
                    "agentRuntimeId",
                    "agentRuntimeVersion",
                    "agentRuntimeName",
                    "description",
                    "lastUpdatedAt",
                    "status",
                ] {
                    assert!(
                        !summary[member].is_null(),
                        "case {case:?}: expected the required summary member {member}, got {summary}"
                    );
                }
                seen.push(summary["agentRuntimeId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => {
                    let size = page_size.unwrap_or(100);
                    query = format!("?maxResults={size}&nextToken={token}");
                }
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: a runtime was reported twice across pages: {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case:?}: expected every persisted runtime exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_store_lists_no_runtimes() {
    let router = router();
    let (status, body) = list(&router, "").await;
    assert_eq!(status, 200, "expected 200 from ListAgentRuntimes, got {status} / {body}");
    assert_eq!(
        body["agentRuntimes"],
        json!([]),
        "expected an empty list from an empty store, got {}",
        body["agentRuntimes"]
    );
    assert!(
        body["nextToken"].is_null(),
        "expected no nextToken on the only page, got {}",
        body["nextToken"]
    );
}

#[tokio::test]
async fn update_publishes_the_next_version_and_keeps_the_earlier_ones() {
    // The rule is "the next integer version, previous versions still readable",
    // so the rows differ in how many updates have been applied.
    for updates in [1_u32, 2, 3] {
        let case = format!("{updates} update(s)");
        let router = router();
        let create_request = json!({
            "agentRuntimeName": "versioned_agent",
            "agentRuntimeArtifact": container_artifact("v1"),
            "roleArn": "arn:aws:iam::000000000000:role/Version1Role",
            "description": "version 1",
        });
        let (status, created) = create(&router, &create_request).await;
        assert_eq!(status, 202, "case {case:?}: create got {status} / {created}");
        let id = created["agentRuntimeId"].as_str().expect("an id").to_string();

        for revision in 1..=updates {
            let next = revision + 1;
            let update_request = json!({
                "agentRuntimeArtifact": container_artifact(&format!("v{next}")),
                "roleArn": format!("arn:aws:iam::000000000000:role/Version{next}Role"),
                "description": format!("version {next}"),
            });
            let (status, updated) = send(
                &router,
                "PUT",
                &format!("/runtimes/{id}/"),
                REGION,
                Some(&update_request),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected 202 from UpdateAgentRuntime, got {status} / {updated}"
            );
            assert_eq!(
                updated["agentRuntimeVersion"],
                json!(next.to_string()),
                "case {case:?}: expected version {next} after update {revision}, got {}",
                updated["agentRuntimeVersion"]
            );
            assert_eq!(
                updated["status"],
                json!("UPDATING"),
                "case {case:?}: expected a just-updated runtime to report UPDATING, got {}",
                updated["status"]
            );
            assert_eq!(
                updated["createdAt"], created["createdAt"],
                "case {case:?}: expected createdAt to survive the update, got {}",
                updated["createdAt"]
            );
            assert_eq!(
                updated["agentRuntimeArn"], created["agentRuntimeArn"],
                "case {case:?}: expected the ARN to survive the update, got {}",
                updated["agentRuntimeArn"]
            );
        }

        let latest = updates + 1;
        let (status, got) = get(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: get got {status} / {got}");
        assert_eq!(
            got["agentRuntimeVersion"],
            json!(latest.to_string()),
            "case {case:?}: expected the latest version {latest}, got {}",
            got["agentRuntimeVersion"]
        );
        assert_eq!(
            got["roleArn"],
            json!(format!("arn:aws:iam::000000000000:role/Version{latest}Role")),
            "case {case:?}: expected the updated roleArn, got {}",
            got["roleArn"]
        );
        assert_eq!(
            got["agentRuntimeArtifact"],
            container_artifact(&format!("v{latest}")),
            "case {case:?}: expected the updated artifact, got {}",
            got["agentRuntimeArtifact"]
        );
        assert!(
            timestamp_at_or_after(&got["lastUpdatedAt"], &got["createdAt"]),
            "case {case:?}: expected lastUpdatedAt at or after createdAt, got {} / {}",
            got["lastUpdatedAt"],
            got["createdAt"]
        );

        // Every earlier version is still readable, with the values it was
        // published with.
        for version in 1..=latest {
            let (status, old) = get_version(&router, &id, &version.to_string()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected version {version} to still resolve, got {status} / {old}"
            );
            assert_eq!(
                old["description"],
                json!(format!("version {version}")),
                "case {case:?}: expected version {version}'s own description, got {}",
                old["description"]
            );
            assert_eq!(
                old["agentRuntimeArtifact"],
                container_artifact(&format!("v{version}")),
                "case {case:?}: expected version {version}'s own artifact, got {}",
                old["agentRuntimeArtifact"]
            );
        }

        // The list reports one summary per runtime, at its latest version.
        let (_, listed) = list(&router, "").await;
        let summaries = listed["agentRuntimes"].as_array().expect("a list");
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected one summary per runtime, got {listed}"
        );
        assert_eq!(
            summaries[0]["agentRuntimeVersion"],
            json!(latest.to_string()),
            "case {case:?}: expected the summary at the latest version, got {}",
            summaries[0]
        );
    }
}

#[tokio::test]
async fn delete_removes_the_runtime_or_just_the_named_version() {
    let router = router();
    let create_request = json!({
        "agentRuntimeName": "deleted_agent",
        "agentRuntimeArtifact": container_artifact("v1"),
        "roleArn": "arn:aws:iam::000000000000:role/DeletedAgentRole",
        "description": "version 1",
    });
    let (_, created) = create(&router, &create_request).await;
    let id = created["agentRuntimeId"].as_str().expect("an id").to_string();
    let (status, _) = send(
        &router,
        "PUT",
        &format!("/runtimes/{id}/"),
        REGION,
        Some(&json!({
            "agentRuntimeArtifact": container_artifact("v2"),
            "roleArn": "arn:aws:iam::000000000000:role/DeletedAgentRole",
            "description": "version 2",
        })),
    )
    .await;
    assert_eq!(status, 202, "expected the update to be accepted, got {status}");

    // Deleting one version leaves the runtime and its other versions.
    let (status, deleted) = send(
        &router,
        "DELETE",
        &format!("/runtimes/{id}/?version=1"),
        REGION,
        None,
    )
    .await;
    assert_eq!(status, 202, "expected 202 from DeleteAgentRuntime, got {status} / {deleted}");
    assert_eq!(
        deleted["status"],
        json!("DELETING"),
        "expected DELETING from DeleteAgentRuntime, got {}",
        deleted["status"]
    );
    assert_eq!(
        deleted["agentRuntimeVersion"],
        json!("1"),
        "expected the deleted version to be reported, got {}",
        deleted["agentRuntimeVersion"]
    );
    let (status, gone) = get_version(&router, &id, "1").await;
    assert_eq!(
        status, 404,
        "expected the deleted version to be absent, got {status} / {gone}"
    );
    let (status, still_there) = get(&router, &id).await;
    assert_eq!(
        status, 200,
        "expected the runtime's other versions to survive, got {status} / {still_there}"
    );
    assert_eq!(
        still_there["agentRuntimeVersion"],
        json!("2"),
        "expected version 2 to be the latest, got {}",
        still_there["agentRuntimeVersion"]
    );

    // Deleting without a version removes the runtime entirely.
    let (status, deleted) =
        send(&router, "DELETE", &format!("/runtimes/{id}/"), REGION, None).await;
    assert_eq!(status, 202, "expected 202 from DeleteAgentRuntime, got {status} / {deleted}");
    assert_eq!(
        deleted["agentRuntimeId"],
        json!(id),
        "expected the deleted id to be reported, got {}",
        deleted["agentRuntimeId"]
    );
    let (status, gone) = get(&router, &id).await;
    assert_eq!(status, 404, "expected the runtime to be gone, got {status} / {gone}");
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {gone}"
    );
    let (_, listed) = list(&router, "").await;
    assert_eq!(
        listed["agentRuntimes"],
        json!([]),
        "expected the deleted runtime to be absent from the list, got {listed}"
    );

    // And the freed name can be used again.
    let (status, recreated) = create(&router, &create_request).await;
    assert_eq!(
        status, 202,
        "expected the name to be free after the delete, got {status} / {recreated}"
    );
}

#[tokio::test]
async fn deleting_the_latest_version_falls_back_without_re_using_its_number() {
    let router = router();
    let (_, created) = create(
        &router,
        &json!({
            "agentRuntimeName": "rollback_agent",
            "agentRuntimeArtifact": container_artifact("v1"),
            "roleArn": "arn:aws:iam::000000000000:role/RollbackAgentRole",
            "description": "version 1",
        }),
    )
    .await;
    let id = created["agentRuntimeId"].as_str().expect("an id").to_string();
    let update = |description: &str, tag: &str| {
        json!({
            "agentRuntimeArtifact": container_artifact(tag),
            "roleArn": "arn:aws:iam::000000000000:role/RollbackAgentRole",
            "description": description,
        })
    };
    let (_, second) = send(
        &router,
        "PUT",
        &format!("/runtimes/{id}/"),
        REGION,
        Some(&update("version 2", "v2")),
    )
    .await;
    assert_eq!(
        second["agentRuntimeVersion"],
        json!("2"),
        "expected the update to publish version 2, got {second}"
    );

    // Deleting the latest version leaves the earlier one as the latest.
    let (status, deleted) = send(
        &router,
        "DELETE",
        &format!("/runtimes/{id}/?version=2"),
        REGION,
        None,
    )
    .await;
    assert_eq!(status, 202, "expected 202 from DeleteAgentRuntime, got {status} / {deleted}");
    let (_, got) = get(&router, &id).await;
    assert_eq!(
        got["agentRuntimeVersion"],
        json!("1"),
        "expected version 1 to become the latest again, got {got}"
    );

    // And the next update publishes version 3: a version number is never
    // re-used for different configuration.
    let (_, third) = send(
        &router,
        "PUT",
        &format!("/runtimes/{id}/"),
        REGION,
        Some(&update("version 3", "v3")),
    )
    .await;
    assert_eq!(
        third["agentRuntimeVersion"],
        json!("3"),
        "expected the next update to publish version 3, got {third}"
    );
    let (status, reused) = get_version(&router, &id, "2").await;
    assert_eq!(
        status, 404,
        "expected the deleted version 2 to stay deleted, got {status} / {reused}"
    );
}

#[tokio::test]
async fn a_created_runtime_becomes_ready_once_the_clock_owes_it() {
    let router = router();
    let (_, created) = create(
        &router,
        &json!({
            "agentRuntimeName": "settling_agent",
            "agentRuntimeArtifact": container_artifact("settle"),
            "roleArn": "arn:aws:iam::000000000000:role/SettlingAgentRole",
        }),
    )
    .await;
    let id = created["agentRuntimeId"].as_str().expect("an id").to_string();

    let (_, before) = get(&router, &id).await;
    assert_eq!(
        before["status"],
        json!("CREATING"),
        "expected a caller acting straight after create to see CREATING, got {}",
        before["status"]
    );
    // Two reads in a row report the same thing: a read changes nothing.
    let (_, again) = get(&router, &id).await;
    assert_eq!(
        again, before,
        "expected a second read to report the same runtime, got {again}"
    );

    tokio::time::sleep(PAST_SETTLE).await;

    let (_, after) = get(&router, &id).await;
    assert_eq!(
        after["status"],
        json!("READY"),
        "expected the runtime to be READY once the transition came due, got {}",
        after["status"]
    );
    let (_, listed) = list(&router, "").await;
    assert_eq!(
        listed["agentRuntimes"][0]["status"],
        json!("READY"),
        "expected the list to report the same status as the read, got {listed}"
    );
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_first_result() {
    let router = router();
    let request = json!({
        "agentRuntimeName": "idempotent_agent",
        "agentRuntimeArtifact": container_artifact("idem"),
        "roleArn": "arn:aws:iam::000000000000:role/IdempotentAgentRole",
        "clientToken": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    });
    let (status, first) = create(&router, &request).await;
    assert_eq!(status, 202, "expected 202 from the first create, got {status} / {first}");
    let (status, replay) = create(&router, &request).await;
    assert_eq!(status, 202, "expected 202 from the replay, got {status} / {replay}");
    assert_eq!(
        replay["agentRuntimeId"], first["agentRuntimeId"],
        "expected the replay to return the first runtime, got {replay}"
    );
    let (_, listed) = list(&router, "").await;
    assert_eq!(
        listed["agentRuntimes"].as_array().map(Vec::len),
        Some(1),
        "expected the replay to create no second runtime, got {listed}"
    );
}

#[tokio::test]
async fn a_client_token_record_does_not_outlive_the_runtime_it_named() {
    let router = router();
    let request = json!({
        "agentRuntimeName": "recycled_agent",
        "agentRuntimeArtifact": container_artifact("recycled"),
        "roleArn": "arn:aws:iam::000000000000:role/RecycledAgentRole",
        "clientToken": "cccccccccccccccccccccccccccccccccc",
    });
    let (_, first) = create(&router, &request).await;
    let id = first["agentRuntimeId"].as_str().expect("an id").to_string();
    let (status, deleted) =
        send(&router, "DELETE", &format!("/runtimes/{id}/"), REGION, None).await;
    assert_eq!(status, 202, "expected the delete to be accepted, got {status} / {deleted}");

    // The token's record went with the runtime, so the same token now reads as
    // a fresh request rather than replaying a runtime that no longer exists.
    let (status, second) = create(&router, &request).await;
    assert_eq!(status, 202, "expected 202 after the delete, got {status} / {second}");
    assert_ne!(
        second["agentRuntimeId"], first["agentRuntimeId"],
        "expected a new runtime rather than a replay of the deleted one, got {second}"
    );
}

#[tokio::test]
async fn a_client_token_is_scoped_to_the_operation_it_was_presented_to() {
    let router = router();
    let token = "dddddddddddddddddddddddddddddddddd";
    let (_, created) = create(
        &router,
        &json!({
            "agentRuntimeName": "scoped_token_agent",
            "agentRuntimeArtifact": container_artifact("v1"),
            "roleArn": "arn:aws:iam::000000000000:role/ScopedTokenRole",
            "clientToken": token,
        }),
    )
    .await;
    let id = created["agentRuntimeId"].as_str().expect("an id").to_string();

    // The SAME token presented to a different operation is a different request:
    // the update must publish version 2, not replay the create's version 1.
    let update = json!({
        "agentRuntimeArtifact": container_artifact("v2"),
        "roleArn": "arn:aws:iam::000000000000:role/ScopedTokenUpdatedRole",
        "clientToken": token,
    });
    let (status, updated) = send(
        &router,
        "PUT",
        &format!("/runtimes/{id}/"),
        REGION,
        Some(&update),
    )
    .await;
    assert_eq!(status, 202, "expected 202 from UpdateAgentRuntime, got {status} / {updated}");
    assert_eq!(
        updated["agentRuntimeVersion"],
        json!("2"),
        "expected the update to publish version 2 rather than replay the create, got {updated}"
    );
    let (_, got) = get(&router, &id).await;
    assert_eq!(
        got["roleArn"],
        json!("arn:aws:iam::000000000000:role/ScopedTokenUpdatedRole"),
        "expected the update's configuration to be stored, got {got}"
    );

    // Replaying the update itself does short-circuit: still version 2.
    let (status, replayed) = send(
        &router,
        "PUT",
        &format!("/runtimes/{id}/"),
        REGION,
        Some(&update),
    )
    .await;
    assert_eq!(status, 202, "expected 202 from the update replay, got {status} / {replayed}");
    assert_eq!(
        replayed["agentRuntimeVersion"],
        json!("2"),
        "expected the update replay to publish no third version, got {replayed}"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_declared_ceilings() {
    use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::state::{
        MAX_AGENT_RUNTIMES, MAX_VERSIONS_PER_RUNTIME,
    };

    // Growth site 1: runtimes per account.
    let crowded = router();
    for index in 0..MAX_AGENT_RUNTIMES {
        let (status, created) = create(
            &crowded,
            &json!({
                "agentRuntimeName": format!("ceiling_agent_{index}"),
                "agentRuntimeArtifact": container_artifact("ceiling"),
                "roleArn": "arn:aws:iam::000000000000:role/CeilingAgentRole",
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case runtime {index}: expected 202 below the ceiling of {MAX_AGENT_RUNTIMES}, \
             got {status} / {created}"
        );
    }
    let (status, refused) = create(
        &crowded,
        &json!({
            "agentRuntimeName": "one_too_many_agent",
            "agentRuntimeArtifact": container_artifact("ceiling"),
            "roleArn": "arn:aws:iam::000000000000:role/CeilingAgentRole",
        }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one past the runtime ceiling: expected HTTP 402, got {status} / {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case one past the runtime ceiling: expected ServiceQuotaExceededException, got {refused}"
    );

    // Growth site 2: versions per runtime.
    let versioned = router();
    let (_, created) = create(
        &versioned,
        &json!({
            "agentRuntimeName": "versions_ceiling_agent",
            "agentRuntimeArtifact": container_artifact("v1"),
            "roleArn": "arn:aws:iam::000000000000:role/VersionsCeilingRole",
        }),
    )
    .await;
    let id = created["agentRuntimeId"].as_str().expect("an id").to_string();
    let update = json!({
        "agentRuntimeArtifact": container_artifact("next"),
        "roleArn": "arn:aws:iam::000000000000:role/VersionsCeilingRole",
    });
    for version in 2..=MAX_VERSIONS_PER_RUNTIME {
        let (status, updated) = send(
            &versioned,
            "PUT",
            &format!("/runtimes/{id}/"),
            REGION,
            Some(&update),
        )
        .await;
        assert_eq!(
            status, 202,
            "case version {version}: expected 202 below the ceiling of \
             {MAX_VERSIONS_PER_RUNTIME}, got {status} / {updated}"
        );
    }
    let (status, refused) = send(
        &versioned,
        "PUT",
        &format!("/runtimes/{id}/"),
        REGION,
        Some(&update),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one past the version ceiling: expected HTTP 402, got {status} / {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case one past the version ceiling: expected ServiceQuotaExceededException, got {refused}"
    );
}

/// One refusal row: a request that must be refused, and the modelled answer it
/// must be refused with.
struct RefusalCase {
    case: &'static str,
    status: u16,
    code: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

#[tokio::test]
async fn bad_requests_are_refused_with_the_modelled_error() {
    let router = router();
    let valid = json!({
        "agentRuntimeName": "refusal_agent",
        "agentRuntimeArtifact": container_artifact("refusal"),
        "roleArn": "arn:aws:iam::000000000000:role/RefusalAgentRole",
    });
    let (status, created) = create(&router, &valid).await;
    assert_eq!(status, 202, "expected the setup create to succeed, got {status} / {created}");

    let unknown_id = "no_such_agent-0123456789";
    let cases = vec![
        RefusalCase {
            case: "a second runtime with the same name",
            status: 409,
            code: "ConflictException",
            method: "PUT",
            uri: "/runtimes/".to_string(),
            body: Some(valid.clone()),
        },
        RefusalCase {
            case: "a name outside AgentRuntimeName's pattern",
            status: 400,
            code: "ValidationException",
            method: "PUT",
            uri: "/runtimes/".to_string(),
            body: Some(json!({
                "agentRuntimeName": "9_starts_with_a_digit",
                "agentRuntimeArtifact": container_artifact("bad"),
                "roleArn": "arn:aws:iam::000000000000:role/BadNameRole",
            })),
        },
        RefusalCase {
            case: "a roleArn outside RoleArn's pattern",
            status: 400,
            code: "ValidationException",
            method: "PUT",
            uri: "/runtimes/".to_string(),
            body: Some(json!({
                "agentRuntimeName": "bad_role_agent",
                "agentRuntimeArtifact": container_artifact("bad"),
                "roleArn": "not-an-arn",
            })),
        },
        RefusalCase {
            case: "getting an unknown runtime",
            status: 404,
            code: "ResourceNotFoundException",
            method: "GET",
            uri: format!("/runtimes/{unknown_id}/"),
            body: None,
        },
        RefusalCase {
            case: "getting a version the runtime never published",
            status: 404,
            code: "ResourceNotFoundException",
            method: "GET",
            uri: format!(
                "/runtimes/{}/?version=7",
                created["agentRuntimeId"].as_str().unwrap_or_default()
            ),
            body: None,
        },
        RefusalCase {
            case: "updating an unknown runtime",
            status: 404,
            code: "ResourceNotFoundException",
            method: "PUT",
            uri: format!("/runtimes/{unknown_id}/"),
            body: Some(json!({
                "agentRuntimeArtifact": container_artifact("missing"),
                "roleArn": "arn:aws:iam::000000000000:role/MissingAgentRole",
            })),
        },
        RefusalCase {
            case: "deleting an unknown runtime",
            status: 404,
            code: "ResourceNotFoundException",
            method: "DELETE",
            uri: format!("/runtimes/{unknown_id}/"),
            body: None,
        },
        RefusalCase {
            case: "a nextToken this service never issued",
            status: 400,
            code: "ValidationException",
            method: "POST",
            uri: "/runtimes/?nextToken=zzzz".to_string(),
            body: None,
        },
        RefusalCase {
            case: "maxResults above the modelled range",
            status: 400,
            code: "ValidationException",
            method: "POST",
            uri: "/runtimes/?maxResults=101".to_string(),
            body: None,
        },
        RefusalCase {
            case: "maxResults below the modelled range",
            status: 400,
            code: "ValidationException",
            method: "POST",
            uri: "/runtimes/?maxResults=0".to_string(),
            body: None,
        },
    ];

    for RefusalCase {
        case,
        status: expected_status,
        code: expected_code,
        method,
        uri,
        body,
    } in cases
    {
        let (status, response) = send(&router, method, &uri, REGION, body.as_ref()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} / {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected {expected_code}, got {response}"
        );
    }

    // The refused requests changed nothing: the one runtime is still the one
    // runtime.
    let (_, listed) = list(&router, "").await;
    assert_eq!(
        listed["agentRuntimes"].as_array().map(Vec::len),
        Some(1),
        "expected the refusals to leave the store alone, got {listed}"
    );
}
