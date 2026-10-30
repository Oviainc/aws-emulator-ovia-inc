// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the configuration bundle lifecycle
//! (`CreateConfigurationBundle`, `GetConfigurationBundle`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The branch a version belongs to when the request names none, which the model
/// documents on both `CreateConfigurationBundleRequest$branchName` ("Defaults to
/// mainline if not specified") and `GetConfigurationBundleRequest$branchName`.
const DEFAULT_BRANCH: &str = "mainline";

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query
/// operation=CreateConfigurationBundle`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
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
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(REGION))
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

/// `CreateConfigurationBundle`: `POST /configuration-bundles/create`
/// (`smithy_query operation=CreateConfigurationBundle`).
async fn create_bundle(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "POST", "/configuration-bundles/create", Some(body)).await
}

/// `GetConfigurationBundle`: `GET /configuration-bundles/{bundleId}`, with the
/// optional `branchName` as an `@httpQuery`.
async fn get_bundle(
    router: &MultiServiceRouter,
    bundle_id: &str,
    branch_name: Option<&str>,
) -> (u16, Value) {
    let uri = match branch_name {
        Some(branch) => format!(
            "/configuration-bundles/{bundle_id}?branchName={}",
            branch.replace('/', "%2F")
        ),
        None => format!("/configuration-bundles/{bundle_id}"),
    };
    send(router, "GET", &uri, None).await
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

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `ConfigurationBundleId`'s pattern, `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`
/// (`smithy_query operation=CreateConfigurationBundle`), checked by hand because this
/// crate carries no regex engine.
fn is_modelled_bundle_id(id: &str) -> bool {
    let Some(first) = id.chars().next() else {
        return false;
    };
    let (head, suffix) = match id.len().checked_sub(11) {
        Some(split) => id.split_at(split),
        None => return false,
    };
    first.is_ascii_alphabetic()
        && (1..=100).contains(&head.len())
        && head
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && suffix.starts_with('-')
        && suffix[1..].len() == 10
        && suffix[1..].bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `ConfigurationBundleVersion`'s pattern,
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` (`smithy_query
/// shape=ConfigurationBundleVersion`), checked by hand for the same reason.
fn is_modelled_version_id(version_id: &str) -> bool {
    let groups: Vec<&str> = version_id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| group.len() == *width)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
}

/// Assert one timestamp member's wire form. `createdAt` and `updatedAt` target the
/// plain `Timestamp` shape with no `@timestampFormat` (`smithy_query
/// operation=CreateConfigurationBundle,GetConfigurationBundle`), so `restJson1`
/// renders it as epoch seconds: a JSON NUMBER naming a real instant.
fn assert_timestamp(case: &str, member: &str, value: &Value) {
    let seconds = value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an epoch-seconds number, got {value:?}")
    });
    assert!(
        seconds > 0.0,
        "case {case:?}: {member} must name a real instant, got {seconds}"
    );
}

/// A configuration bundle is created from the request and its initial version is read
/// back through `GetConfigurationBundle`, for every shape of request the operation
/// accepts.
///
/// The rows differ along what the rule generalises over: how many components the
/// bundle carries and what kind of document each configuration is, whether the
/// description, commit message, creation source, KMS key and tags are supplied, and
/// which branch the initial version is committed on (none, `mainline` by name, and a
/// path-shaped branch). Sources for the value lists: the members and their constraints
/// in `smithy_query operation=CreateConfigurationBundle,GetConfigurationBundle`,
/// `ComponentConfiguration$configuration` (a flexible JSON document) and `BranchName`.
#[tokio::test]
async fn a_configuration_bundle_is_created_and_its_initial_version_reads_back() {
    let router = router();

    // (case, bundleName, the members beyond `bundleName` and `components`, components,
    //  the branch the version lands on)
    let rows: [(&str, &str, Value, Value, &str); 6] = [
        (
            "only the required members, one object component",
            "minimal",
            json!({}),
            json!({"agent": {"configuration": {"model": "a-model", "temperature": 0}}}),
            DEFAULT_BRANCH,
        ),
        (
            "a described bundle with two components and a commit message",
            "described_bundle",
            json!({"description": "the evaluated agent", "commitMessage": "initial import"}),
            json!({
                "agent": {"configuration": {"model": "a-model"}},
                "tools/search": {"configuration": {"enabled": true, "topK": 5}}
            }),
            DEFAULT_BRANCH,
        ),
        (
            "the default branch named explicitly, a scalar component document",
            "scalarBundle",
            json!({"branchName": DEFAULT_BRANCH}),
            json!({"prompt": {"configuration": "answer briefly"}}),
            DEFAULT_BRANCH,
        ),
        (
            "a path-shaped branch and a list component document",
            "branched",
            json!({"branchName": "team/experiment-1", "commitMessage": "try a new order"}),
            json!({"pipeline": {"configuration": ["retrieve", "rerank", "generate"]}}),
            "team/experiment-1",
        ),
        (
            "a bundle created by a named source, with tags",
            "withSource",
            json!({
                "createdBy": {"name": "optimization-job", "arn": "arn:aws:bedrock-agentcore:us-east-1:000000000000:job/one"},
                "tags": {"owner": "team-a", "env": "test"}
            }),
            json!({"agent": {"configuration": {"nested": {"depth": 2}, "flags": [true, false]}}}),
            DEFAULT_BRANCH,
        ),
        (
            "a customer-managed key and an empty component map",
            "encrypted",
            json!({
                "kmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012",
                "branchName": "release-2024"
            }),
            json!({}),
            "release-2024",
        ),
    ];

    for (case, bundle_name, extra, components, branch) in rows {
        let mut body = json!({"bundleName": bundle_name, "components": components});
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }

        let (status, created) = create_bundle(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateConfigurationBundle, got {status} \
             ({created})"
        );

        let bundle_id = text(&created, "bundleId");
        assert!(
            is_modelled_bundle_id(&bundle_id),
            "case {case:?}: expected a bundleId matching \
             ^[a-zA-Z][a-zA-Z0-9-_]{{0,99}}-[a-zA-Z0-9]{{10}}$, got {bundle_id:?}"
        );
        assert!(
            bundle_id.starts_with(&format!("{bundle_name}-")),
            "case {case:?}: expected the bundleId to carry the bundleName, got {bundle_id:?}"
        );
        assert_eq!(
            text(&created, "bundleArn"),
            format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:configuration-bundle/{bundle_id}"
            ),
            "case {case:?}: expected the modelled ConfigurationBundleArn for {bundle_id}, got {}",
            text(&created, "bundleArn")
        );
        let version_id = text(&created, "versionId");
        assert!(
            is_modelled_version_id(&version_id),
            "case {case:?}: expected a versionId in the modelled UUID form, got {version_id:?}"
        );
        assert_timestamp(case, "createdAt", &created["createdAt"]);

        // The version the create minted is the latest one on its own branch, so a read
        // of that branch reports it. A row that named no branch is read with none.
        let read_branch = (branch != DEFAULT_BRANCH).then_some(branch);
        let (status, fetched) = get_bundle(&router, &bundle_id, read_branch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetConfigurationBundle, got {status} \
             ({fetched})"
        );
        assert_eq!(
            text(&fetched, "bundleName"),
            bundle_name,
            "case {case:?}: the bundleName reads back as sent, got {}",
            text(&fetched, "bundleName")
        );
        assert_eq!(
            text(&fetched, "bundleId"),
            bundle_id,
            "case {case:?}: the read reports the id the create minted, got {}",
            text(&fetched, "bundleId")
        );
        assert_eq!(
            text(&fetched, "bundleArn"),
            text(&created, "bundleArn"),
            "case {case:?}: the read reports the ARN the create handed back, got {}",
            text(&fetched, "bundleArn")
        );
        assert_eq!(
            text(&fetched, "versionId"),
            version_id,
            "case {case:?}: the latest version on the branch is the one the create \
             minted, got {}",
            text(&fetched, "versionId")
        );
        assert_eq!(
            fetched["components"], body["components"],
            "case {case:?}: the components read back as sent, got {}",
            fetched["components"]
        );
        assert_eq!(
            fetched["description"],
            body.get("description").cloned().unwrap_or(Value::Null),
            "case {case:?}: the description reads back as sent, got {}",
            fetched["description"]
        );
        assert_eq!(
            fetched["kmsKeyArn"],
            body.get("kmsKeyArn").cloned().unwrap_or(Value::Null),
            "case {case:?}: the kmsKeyArn reads back as sent, got {}",
            fetched["kmsKeyArn"]
        );
        assert_eq!(
            fetched["lineageMetadata"]["branchName"],
            json!(branch),
            "case {case:?}: expected the version on branch {branch:?}, got {}",
            fetched["lineageMetadata"]["branchName"]
        );
        assert_eq!(
            fetched["lineageMetadata"]["parentVersionIds"],
            json!([]),
            "case {case:?}: the version a create mints has no parent, got {}",
            fetched["lineageMetadata"]["parentVersionIds"]
        );
        assert_eq!(
            fetched["lineageMetadata"]["commitMessage"],
            body.get("commitMessage").cloned().unwrap_or(Value::Null),
            "case {case:?}: the commitMessage reads back as sent, got {}",
            fetched["lineageMetadata"]["commitMessage"]
        );
        assert_eq!(
            fetched["lineageMetadata"]["createdBy"],
            body.get("createdBy").cloned().unwrap_or(Value::Null),
            "case {case:?}: the creation source reads back as sent, got {}",
            fetched["lineageMetadata"]["createdBy"]
        );
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {case:?}: the read reports the create instant, got {} against {}",
            fetched["createdAt"], created["createdAt"]
        );
        assert_eq!(
            fetched["updatedAt"], fetched["createdAt"],
            "case {case:?}: a bundle with one version was last updated when it was \
             created, got {} and {}",
            fetched["updatedAt"], fetched["createdAt"]
        );
        for member in ["createdAt", "updatedAt"] {
            assert_timestamp(case, member, &fetched[member]);
        }

        // A read changes nothing a later caller sees.
        let (status, again) = get_bundle(&router, &bundle_id, read_branch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading the bundle again, got {status} ({again})"
        );
        assert_eq!(
            again, fetched,
            "case {case:?}: a read does not change what the next read reports, got {again}"
        );

        // A branch this bundle holds no commit on has no version to report, including
        // the default branch for a row that committed elsewhere.
        let empty_branch = if branch == DEFAULT_BRANCH {
            Some("no/such-branch")
        } else {
            None
        };
        let (status, missing) = get_bundle(&router, &bundle_id, empty_branch).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for a branch holding no version, got {status} \
             ({missing})"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {} ({missing})",
            error_code(&missing)
        );
    }
}

/// What `CreateConfigurationBundle` refuses, and what a `clientToken` replayed against
/// it answers with: "Names must be unique within your account", and "If this token
/// matches a previous request, the service ignores the request, but doesn't return an
/// error."
#[tokio::test]
async fn create_configuration_bundle_refuses_a_held_name_and_replays_one_token() {
    let router = router();

    let token = client_token("idem");
    let body = json!({
        "bundleName": "taken",
        "components": {"agent": {"configuration": {"model": "a-model"}}},
        "clientToken": token,
    });
    let (status, first) = create_bundle(&router, &body).await;
    assert_eq!(
        status, 201,
        "the first create of a name succeeds, got {status} ({first})"
    );

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 4] = [
        (
            "the same name again, with a fresh token",
            json!({
                "bundleName": "taken",
                "components": {"agent": {"configuration": {}}},
                "clientToken": client_token("other"),
            }),
            409,
            "ConflictException",
        ),
        (
            "the same name with different components",
            json!({
                "bundleName": "taken",
                "components": {"tools": {"configuration": [1, 2, 3]}},
            }),
            409,
            "ConflictException",
        ),
        (
            "a name that differs only in case is its own bundle",
            json!({
                "bundleName": "Taken",
                "components": {"agent": {"configuration": {}}},
            }),
            201,
            "",
        ),
        (
            "a name no bundle holds",
            json!({
                "bundleName": "free_name",
                "components": {"agent": {"configuration": {}}},
            }),
            201,
            "",
        ),
    ];

    for (case, request, expected_status, expected_code) in rows {
        let (status, answered) = create_bundle(&router, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );
        if expected_status == 201 {
            continue;
        }
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
    }

    // (case, the replayed body)
    let replays: [(&str, Value); 2] = [
        ("the same request again", body.clone()),
        (
            "the same token with other members changed",
            json!({
                "bundleName": "taken",
                "components": {"agent": {"configuration": {"model": "another-model"}}},
                "description": "changed",
                "clientToken": token,
            }),
        ),
    ];

    for (case, replayed) in replays {
        let (status, answered) = create_bundle(&router, &replayed).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a replayed token is not an error, got {status} ({answered})"
        );
        assert_eq!(
            text(&answered, "bundleId"),
            text(&first, "bundleId"),
            "case {case:?}: a replayed token answers with the first bundle, got {}",
            text(&answered, "bundleId")
        );
        assert_eq!(
            text(&answered, "versionId"),
            text(&first, "versionId"),
            "case {case:?}: a replayed token answers with the first version, got {}",
            text(&answered, "versionId")
        );
    }

    // The refused creates and the replays persisted nothing on the held name: the one
    // bundle it addresses is still the one the first create minted.
    let bundle_id = text(&first, "bundleId");
    let (status, fetched) = get_bundle(&router, &bundle_id, None).await;
    assert_eq!(
        status, 200,
        "the bundle the first create minted is still readable, got {status} ({fetched})"
    );
    assert_eq!(
        fetched["description"],
        Value::Null,
        "a refused or replayed create does not overwrite the stored version, got {}",
        fetched["description"]
    );
    assert_eq!(
        fetched["components"],
        json!({"agent": {"configuration": {"model": "a-model"}}}),
        "a refused or replayed create does not overwrite the stored components, got {}",
        fetched["components"]
    );

    // (case, the bundleId read, expected status)
    let reads: [(&str, &str, u16); 3] = [
        ("the minted id", bundle_id.as_str(), 200),
        ("an id no bundle holds", "absent-aaaaaaaaaa", 404),
        ("an id whose name is held but suffix is not", "taken-0000000000", 404),
    ];

    for (case, identifier, expected_status) in reads {
        let (status, answered) = get_bundle(&router, identifier, None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetConfigurationBundle, got \
             {status} ({answered})"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} ({answered})",
                error_code(&answered)
            );
        }
    }
}

/// Two bundles created in the same instant are two resources: distinct ids, distinct
/// version ids, and each read reports only its own components.
#[tokio::test]
async fn bundles_created_together_are_distinct_resources() {
    let router = router();

    let mut minted: Vec<(String, String, Value)> = Vec::new();
    for index in 0..4 {
        let components = json!({"agent": {"configuration": {"index": index}}});
        let (status, created) = create_bundle(
            &router,
            &json!({"bundleName": format!("bundle_{index}"), "components": components}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {index}: expected HTTP 201 from CreateConfigurationBundle, got {status} \
             ({created})"
        );
        minted.push((
            text(&created, "bundleId"),
            text(&created, "versionId"),
            components,
        ));
    }

    for (index, (bundle_id, version_id, components)) in minted.iter().enumerate() {
        assert_eq!(
            minted
                .iter()
                .filter(|(other, _, _)| other == bundle_id)
                .count(),
            1,
            "case {index}: expected a bundleId no other bundle holds, got {bundle_id:?}"
        );
        assert_eq!(
            minted
                .iter()
                .filter(|(_, other, _)| other == version_id)
                .count(),
            1,
            "case {index}: expected a versionId no other version holds, got {version_id:?}"
        );

        let (status, fetched) = get_bundle(&router, bundle_id, None).await;
        assert_eq!(
            status, 200,
            "case {index}: expected HTTP 200 from GetConfigurationBundle, got {status} \
             ({fetched})"
        );
        assert_eq!(
            fetched["components"], *components,
            "case {index}: each bundle reports its own components, got {}",
            fetched["components"]
        );
        assert_eq!(
            text(&fetched, "versionId"),
            *version_id,
            "case {index}: each bundle reports its own version, got {}",
            text(&fetched, "versionId")
        );
    }
}
