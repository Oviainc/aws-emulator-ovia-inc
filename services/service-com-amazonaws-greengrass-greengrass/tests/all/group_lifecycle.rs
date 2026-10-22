// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Greengrass V1 group lifecycle over the wire:
//! `CreateGroup`, `CreateGroupVersion`, `GetGroup`, `GetGroupVersion`, `ListGroups`,
//! `CreateDeployment`, `GetDeploymentStatus` and `DeleteGroup`.
//!
//! Every test drives real HTTP through the same `MultiServiceRouter::dispatch` the
//! emulator serves sockets with, so what is asserted is the wire behaviour and not an
//! internal call.

use std::sync::Arc;

use service_com_amazonaws_greengrass_greengrass::ops::types::DEPLOYMENT_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// The seven component version ARN members a group version carries, from
/// `smithy_query(query="shape", shape="GroupVersion")` — the whole member set, not a
/// sample of it.
const GROUP_VERSION_MEMBERS: &[&str] = &[
    "ConnectorDefinitionVersionArn",
    "CoreDefinitionVersionArn",
    "DeviceDefinitionVersionArn",
    "FunctionDefinitionVersionArn",
    "LoggerDefinitionVersionArn",
    "ResourceDefinitionVersionArn",
    "SubscriptionDefinitionVersionArn",
];

/// The deployment statuses `GetDeploymentStatus` may report, from the model's doc for
/// `GetDeploymentStatusResponse$DeploymentStatus`: "The status of the deployment:
/// ''InProgress'', ''Building'', ''Success'', or ''Failure''."
const DEPLOYMENT_STATUSES: &[&str] = &["InProgress", "Building", "Success", "Failure"];

/// One emulator: a single shared engine with the greengrass service mounted on it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_greengrass_greengrass::EmulatedService::new(engine),
    );
    router
}

/// A signed-looking request addressed at `region`'s greengrass endpoint. The
/// credential scope is what routes the request and what the handlers read the region
/// from; signatures are not verified unless an authenticator is registered.
fn request(
    method: &str,
    region: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> http::Request<bytes::Bytes> {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON value serializes"))
        .unwrap_or_default();
    http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{region}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTGREENGRASS/20260101/{region}/greengrass/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a valid test request")
}

/// Dispatch and decode: the status code plus the JSON body (`Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    region: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let response = router.dispatch(request(method, region, path, body)).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, decoded)
}

/// `call` with the default test region, for the many cases that do not vary it.
async fn ok(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let (status, decoded) = call(router, method, "us-east-1", path, body).await;
    assert_eq!(
        status, 200,
        "case {method} {path}: expected HTTP 200, got {status} with body {decoded:?}"
    );
    decoded
}

/// Read a string member, failing with the whole body when it is absent.
fn string_at(value: &serde_json::Value, member: &str) -> String {
    value
        .get(member)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {member}: expected a string member, got {value:?}"))
        .to_string()
}

/// Create a group named `name` and return its id.
async fn create_group(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "POST",
        "/greengrass/groups",
        Some(serde_json::json!({ "Name": name })),
    )
    .await;
    string_at(&created, "Id")
}

/// The whole script the issue's "Done when" names, in order, with every response
/// checked against what the previous call wrote.
#[tokio::test]
async fn the_group_lifecycle_runs_end_to_end() {
    let router = router();
    let region = "us-west-2";

    let created = {
        let (status, body) = call(
            &router,
            "POST",
            region,
            "/greengrass/groups",
            Some(serde_json::json!({
                "Name": "demo-group",
                "tags": { "stage": "test" }
            })),
        )
        .await;
        assert_eq!(status, 200, "CreateGroup: expected 200, got {status} ({body:?})");
        body
    };
    let group_id = string_at(&created, "Id");
    let group_arn = string_at(&created, "Arn");

    assert_eq!(
        group_arn,
        format!("arn:aws:greengrass:{region}:000000000000:/greengrass/groups/{group_id}"),
        "CreateGroup: the ARN must be derived from the minted id and the request's region, got {group_arn}"
    );
    assert_eq!(
        string_at(&created, "Name"),
        "demo-group",
        "CreateGroup: expected the stored name, got {created:?}"
    );
    assert!(
        created.get("LatestVersion").is_none_or(serde_json::Value::is_null),
        "CreateGroup without InitialVersion: LatestVersion must be absent, got {created:?}"
    );
    assert!(
        created.get("LatestVersionArn").is_none_or(serde_json::Value::is_null),
        "CreateGroup without InitialVersion: LatestVersionArn must be absent, got {created:?}"
    );

    // CreateGroupVersion appends a version and advances the group's pointers.
    let core_arn = format!(
        "arn:aws:greengrass:{region}:000000000000:/greengrass/definition/cores/abc/versions/1"
    );
    let version = {
        let (status, body) = call(
            &router,
            "POST",
            region,
            &format!("/greengrass/groups/{group_id}/versions"),
            Some(serde_json::json!({ "CoreDefinitionVersionArn": core_arn })),
        )
        .await;
        assert_eq!(status, 200, "CreateGroupVersion: expected 200, got {status} ({body:?})");
        body
    };
    let version_id = string_at(&version, "Version");
    assert_eq!(
        string_at(&version, "Id"),
        group_id,
        "CreateGroupVersion: Id must be the parent group id, got {version:?}"
    );
    assert_eq!(
        string_at(&version, "Arn"),
        format!("{group_arn}/versions/{version_id}"),
        "CreateGroupVersion: the version ARN must extend the group ARN, got {version:?}"
    );

    // GetGroup reads back exactly what the two creates persisted.
    let fetched = {
        let (status, body) = call(
            &router,
            "GET",
            region,
            &format!("/greengrass/groups/{group_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "GetGroup: expected 200, got {status} ({body:?})");
        body
    };
    assert_eq!(
        string_at(&fetched, "Name"),
        "demo-group",
        "GetGroup: expected the stored name, got {fetched:?}"
    );
    assert_eq!(
        string_at(&fetched, "Arn"),
        group_arn,
        "GetGroup: expected the ARN CreateGroup returned, got {fetched:?}"
    );
    assert_eq!(
        string_at(&fetched, "CreationTimestamp"),
        string_at(&created, "CreationTimestamp"),
        "GetGroup: CreationTimestamp must be the stored one, got {fetched:?}"
    );
    assert_eq!(
        string_at(&fetched, "LatestVersion"),
        version_id,
        "GetGroup: LatestVersion must be the version just created, got {fetched:?}"
    );
    assert_eq!(
        string_at(&fetched, "LatestVersionArn"),
        string_at(&version, "Arn"),
        "GetGroup: LatestVersionArn must be the stored version ARN, got {fetched:?}"
    );
    assert_eq!(
        fetched.get("tags"),
        Some(&serde_json::json!({ "stage": "test" })),
        "GetGroup: expected the tags CreateGroup stored, got {fetched:?}"
    );
    assert_eq!(
        string_at(&fetched, "LastUpdatedTimestamp"),
        string_at(&version, "CreationTimestamp"),
        "GetGroup: LastUpdatedTimestamp must advance to the newest version, got {fetched:?}"
    );

    // GetGroupVersion reconstructs the definition from the stored ARN set.
    let read_version = {
        let (status, body) = call(
            &router,
            "GET",
            region,
            &format!("/greengrass/groups/{group_id}/versions/{version_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "GetGroupVersion: expected 200, got {status} ({body:?})");
        body
    };
    assert_eq!(
        read_version
            .get("Definition")
            .and_then(|d| d.get("CoreDefinitionVersionArn"))
            .and_then(serde_json::Value::as_str),
        Some(core_arn.as_str()),
        "GetGroupVersion: Definition must carry the ARN CreateGroupVersion stored, got {read_version:?}"
    );

    // ListGroups enumerates it.
    let listed = {
        let (status, body) = call(&router, "GET", region, "/greengrass/groups", None).await;
        assert_eq!(status, 200, "ListGroups: expected 200, got {status} ({body:?})");
        body
    };
    let groups = listed
        .get("Groups")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("ListGroups: expected a Groups list, got {listed:?}"));
    assert_eq!(groups.len(), 1, "ListGroups: expected 1 group, got {listed:?}");
    assert_eq!(
        string_at(&groups[0], "LatestVersion"),
        version_id,
        "ListGroups: the entry must carry the stored LatestVersion, got {listed:?}"
    );

    // CreateDeployment + GetDeploymentStatus.
    let deployment = {
        let (status, body) = call(
            &router,
            "POST",
            region,
            &format!("/greengrass/groups/{group_id}/deployments"),
            Some(serde_json::json!({
                "DeploymentType": "NewDeployment",
                "GroupVersionId": version_id
            })),
        )
        .await;
        assert_eq!(status, 200, "CreateDeployment: expected 200, got {status} ({body:?})");
        body
    };
    let deployment_id = string_at(&deployment, "DeploymentId");
    assert_eq!(
        string_at(&deployment, "DeploymentArn"),
        format!("{group_arn}/deployments/{deployment_id}"),
        "CreateDeployment: the deployment ARN must extend the group ARN, got {deployment:?}"
    );

    let status_body = {
        let (status, body) = call(
            &router,
            "GET",
            region,
            &format!("/greengrass/groups/{group_id}/deployments/{deployment_id}/status"),
            None,
        )
        .await;
        assert_eq!(status, 200, "GetDeploymentStatus: expected 200, got {status} ({body:?})");
        body
    };
    assert_eq!(
        string_at(&status_body, "DeploymentType"),
        "NewDeployment",
        "GetDeploymentStatus: expected the stored DeploymentType, got {status_body:?}"
    );
    let reported = string_at(&status_body, "DeploymentStatus");
    assert!(
        DEPLOYMENT_STATUSES.contains(&reported.as_str()),
        "GetDeploymentStatus: DeploymentStatus must be a modelled value, got {reported:?}"
    );
    assert!(
        !string_at(&status_body, "UpdatedAt").is_empty(),
        "GetDeploymentStatus: expected an UpdatedAt, got {status_body:?}"
    );

    // DeleteGroup removes the group, its versions and its deployments.
    let (delete_status, delete_body) = call(
        &router,
        "DELETE",
        region,
        &format!("/greengrass/groups/{group_id}"),
        None,
    )
    .await;
    assert_eq!(
        delete_status, 200,
        "DeleteGroup: expected 200, got {delete_status} ({delete_body:?})"
    );

    let (after_delete, after_body) = call(
        &router,
        "GET",
        region,
        &format!("/greengrass/groups/{group_id}"),
        None,
    )
    .await;
    assert_eq!(
        after_delete, 400,
        "GetGroup after DeleteGroup: expected 400, got {after_delete} ({after_body:?})"
    );
    assert_eq!(
        after_body.get("__type").and_then(serde_json::Value::as_str),
        Some("BadRequestException"),
        "GetGroup after DeleteGroup: expected BadRequestException, got {after_body:?}"
    );

    let listed = ok(&router, "GET", "/greengrass/groups", None).await;
    assert_eq!(
        listed.get("Groups").and_then(serde_json::Value::as_array),
        Some(&vec![]),
        "ListGroups after DeleteGroup: the group must be gone, got {listed:?}"
    );
}

/// The rule for a group version: whichever subset of the seven component ARN members
/// is written is what reads back, member for member, and nothing else is invented.
/// The rows span the member count (0, 1, a few, all seven) and which members carry a
/// value, including members the issue never cited.
#[tokio::test]
async fn a_group_version_reads_back_exactly_the_component_arns_it_was_written_with() {
    let router = router();

    let subsets: &[&[&str]] = &[
        &[],
        &["CoreDefinitionVersionArn"],
        // A member the issue's reproduction never used.
        &["ConnectorDefinitionVersionArn"],
        &["LoggerDefinitionVersionArn", "ResourceDefinitionVersionArn"],
        &[
            "CoreDefinitionVersionArn",
            "FunctionDefinitionVersionArn",
            "SubscriptionDefinitionVersionArn",
        ],
        GROUP_VERSION_MEMBERS,
    ];

    for (row, subset) in subsets.iter().enumerate() {
        let group_id = create_group(&router, &format!("subset-{row}")).await;
        let mut body = serde_json::Map::new();
        for member in subset.iter() {
            body.insert(
                (*member).to_string(),
                serde_json::Value::String(format!(
                    "arn:aws:greengrass:us-east-1:000000000000:/greengrass/definition/{member}/versions/{row}"
                )),
            );
        }
        let created = ok(
            &router,
            "POST",
            &format!("/greengrass/groups/{group_id}/versions"),
            Some(serde_json::Value::Object(body.clone())),
        )
        .await;
        let version_id = string_at(&created, "Version");

        let read = ok(
            &router,
            "GET",
            &format!("/greengrass/groups/{group_id}/versions/{version_id}"),
            None,
        )
        .await;
        let definition = read
            .get("Definition")
            .unwrap_or_else(|| panic!("case {subset:?}: expected a Definition, got {read:?}"));

        for member in GROUP_VERSION_MEMBERS {
            let actual = definition.get(*member).and_then(serde_json::Value::as_str);
            let expected = body.get(*member).and_then(serde_json::Value::as_str);
            assert_eq!(
                actual, expected,
                "case {subset:?} member {member}: expected {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            string_at(&read, "Id"),
            group_id,
            "case {subset:?}: GetGroupVersion Id must be the parent group id, got {read:?}"
        );
        assert_eq!(
            string_at(&read, "Arn"),
            string_at(&created, "Arn"),
            "case {subset:?}: the version ARN must be the stored one, got {read:?}"
        );
    }
}

/// `CreateGroup`'s `InitialVersion` becomes version one and the group's latest
/// version, reachable through `GetGroupVersion`. Rows span an absent, an empty and a
/// populated `InitialVersion`.
#[tokio::test]
async fn an_initial_version_becomes_the_groups_first_version() {
    let router = router();
    let cases: &[(&str, Option<serde_json::Value>)] = &[
        ("absent", None),
        ("empty", Some(serde_json::json!({}))),
        (
            "populated",
            Some(serde_json::json!({
                "CoreDefinitionVersionArn": "arn:aws:greengrass:us-east-1:000000000000:/greengrass/definition/cores/c1/versions/1",
                "DeviceDefinitionVersionArn": "arn:aws:greengrass:us-east-1:000000000000:/greengrass/definition/devices/d1/versions/1"
            })),
        ),
    ];

    for (case, initial) in cases {
        let mut body = serde_json::Map::new();
        body.insert(
            "Name".to_string(),
            serde_json::Value::String(format!("initial-{case}")),
        );
        if let Some(initial) = initial {
            body.insert("InitialVersion".to_string(), initial.clone());
        }
        let created = ok(
            &router,
            "POST",
            "/greengrass/groups",
            Some(serde_json::Value::Object(body)),
        )
        .await;
        let group_id = string_at(&created, "Id");

        match initial {
            None => {
                assert!(
                    created.get("LatestVersion").is_none_or(serde_json::Value::is_null),
                    "case {case}: no InitialVersion must leave LatestVersion absent, got {created:?}"
                );
            }
            Some(initial) => {
                let version_id = string_at(&created, "LatestVersion");
                assert_eq!(
                    string_at(&created, "LatestVersionArn"),
                    format!("{}/versions/{version_id}", string_at(&created, "Arn")),
                    "case {case}: LatestVersionArn must extend the group ARN, got {created:?}"
                );
                let read = ok(
                    &router,
                    "GET",
                    &format!("/greengrass/groups/{group_id}/versions/{version_id}"),
                    None,
                )
                .await;
                let definition = read.get("Definition").unwrap_or_else(|| {
                    panic!("case {case}: expected a Definition, got {read:?}")
                });
                for member in GROUP_VERSION_MEMBERS {
                    let expected = initial.get(*member).and_then(serde_json::Value::as_str);
                    let actual = definition.get(*member).and_then(serde_json::Value::as_str);
                    assert_eq!(
                        actual, expected,
                        "case {case} member {member}: expected {expected:?}, got {actual:?}"
                    );
                }
            }
        }
    }
}

/// `ListGroups` pages by `MaxResults` and its `NextToken` resumes exactly where the
/// page ended: concatenating every page yields each stored group once, in creation
/// order, with no token on the last page. Rows span the collection size and the page
/// size, including a page size larger than the collection and an absent one.
#[tokio::test]
async fn list_groups_pages_by_max_results_and_resumes_on_the_token() {
    let cases: &[(usize, Option<usize>)] = &[
        (0, None),
        (1, Some(1)),
        (3, Some(1)),
        (3, Some(2)),
        (3, Some(3)),
        (3, Some(10)),
        (5, Some(2)),
        (4, None),
    ];

    for (count, page_size) in cases {
        let router = router();
        let mut expected = Vec::new();
        for index in 0..*count {
            expected.push(create_group(&router, &format!("paged-{index}")).await);
        }

        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = "/greengrass/groups".to_string();
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("MaxResults={size}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(format!("NextToken={token}"));
            }
            if !query.is_empty() {
                path.push('?');
                path.push_str(&query.join("&"));
            }

            let body = ok(&router, "GET", &path, None).await;
            let groups = body
                .get("Groups")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case ({count}, {page_size:?}): expected a Groups list, got {body:?}")
                });
            if let Some(size) = page_size {
                assert!(
                    groups.len() <= *size,
                    "case ({count}, {page_size:?}): a page may not exceed MaxResults, got {} entries",
                    groups.len()
                );
            }
            for entry in groups {
                seen.push(string_at(entry, "Id"));
            }
            token = body
                .get("NextToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= count + 2,
                "case ({count}, {page_size:?}): paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case ({count}, {page_size:?}): every stored group must appear once, in creation order"
        );
    }
}

/// A `NextToken` the service did not issue, and a `MaxResults` that is not a positive
/// integer, are refused rather than silently ignored. Rows span the published bound
/// (1, the smallest legal page size) and values past it.
#[tokio::test]
async fn list_groups_refuses_pagination_input_it_cannot_honour() {
    let router = router();
    create_group(&router, "one").await;

    // `MaxResults=1` is the smallest page size that can be honoured; it must work.
    let body = ok(&router, "GET", "/greengrass/groups?MaxResults=1", None).await;
    assert_eq!(
        body.get("Groups").and_then(serde_json::Value::as_array).map(Vec::len),
        Some(1),
        "case MaxResults=1: expected one entry, got {body:?}"
    );

    let refused = [
        "/greengrass/groups?MaxResults=0",
        "/greengrass/groups?MaxResults=-1",
        "/greengrass/groups?MaxResults=many",
        "/greengrass/groups?NextToken=not-a-token",
        // Sixteen characters, but outside the alphabet this service's cursor is
        // rendered in (lowercase hex), so not a token it can have issued.
        "/greengrass/groups?NextToken=%2B000000000000001",
        "/greengrass/groups?NextToken=00000000000000AF",
    ];
    for path in refused {
        let (status, body) = call(&router, "GET", "us-east-1", path, None).await;
        assert_eq!(
            status, 400,
            "case {path}: expected HTTP 400, got {status} with body {body:?}"
        );
    }
}

/// Every lifecycle read refuses an id the store does not hold with
/// `BadRequestException` (HTTP 400) — never a 200 and never a 501. Rows span each
/// operation and each label it resolves.
#[tokio::test]
async fn an_unknown_id_is_a_bad_request_on_every_lifecycle_operation() {
    let router = router();
    let group_id = create_group(&router, "known").await;
    let version = ok(
        &router,
        "POST",
        &format!("/greengrass/groups/{group_id}/versions"),
        Some(serde_json::json!({})),
    )
    .await;
    let version_id = string_at(&version, "Version");
    let deployment = ok(
        &router,
        "POST",
        &format!("/greengrass/groups/{group_id}/deployments"),
        Some(serde_json::json!({
            "DeploymentType": "NewDeployment",
            "GroupVersionId": version_id
        })),
    )
    .await;
    let deployment_id = string_at(&deployment, "DeploymentId");
    let missing = "11111111-2222-3333-4444-555555555555";

    let cases: Vec<(&str, String, Option<serde_json::Value>)> = vec![
        ("GET", format!("/greengrass/groups/{missing}"), None),
        ("DELETE", format!("/greengrass/groups/{missing}"), None),
        (
            "GET",
            format!("/greengrass/groups/{missing}/versions/{version_id}"),
            None,
        ),
        (
            "GET",
            format!("/greengrass/groups/{group_id}/versions/{missing}"),
            None,
        ),
        (
            "POST",
            format!("/greengrass/groups/{missing}/versions"),
            Some(serde_json::json!({})),
        ),
        (
            "POST",
            format!("/greengrass/groups/{missing}/deployments"),
            Some(serde_json::json!({
                "DeploymentType": "NewDeployment",
                "GroupVersionId": version_id
            })),
        ),
        (
            "POST",
            format!("/greengrass/groups/{group_id}/deployments"),
            Some(serde_json::json!({
                "DeploymentType": "NewDeployment",
                "GroupVersionId": missing
            })),
        ),
        (
            "GET",
            format!("/greengrass/groups/{missing}/deployments/{deployment_id}/status"),
            None,
        ),
        (
            "GET",
            format!("/greengrass/groups/{group_id}/deployments/{missing}/status"),
            None,
        ),
    ];

    for (method, path, body) in cases {
        let (status, decoded) = call(&router, method, "us-east-1", &path, body).await;
        assert_eq!(
            status, 400,
            "case {method} {path}: expected HTTP 400, got {status} with body {decoded:?}"
        );
        assert_eq!(
            decoded.get("__type").and_then(serde_json::Value::as_str),
            Some("BadRequestException"),
            "case {method} {path}: expected BadRequestException, got {decoded:?}"
        );
    }
}

/// `CreateDeployment` accepts exactly the deployment types the model says are valid
/// for it — "When used for ''CreateDeployment'', only ''NewDeployment'' and
/// ''Redeployment'' are valid" — and refuses the reset types with
/// `BadRequestException`. The rows are the whole `DeploymentType` value set the model
/// declares, so a new enum value cannot slip through untested.
#[tokio::test]
async fn create_deployment_accepts_only_the_types_the_model_declares_valid_for_it() {
    let router = router();

    for deployment_type in DEPLOYMENT_TYPE_VALUES {
        let group_id = create_group(&router, &format!("deploy-{deployment_type}")).await;
        let version = ok(
            &router,
            "POST",
            &format!("/greengrass/groups/{group_id}/versions"),
            Some(serde_json::json!({})),
        )
        .await;
        let version_id = string_at(&version, "Version");

        // Every group starts with one accepted deployment, so `Redeployment` has a
        // previous deployment to replay.
        let first = ok(
            &router,
            "POST",
            &format!("/greengrass/groups/{group_id}/deployments"),
            Some(serde_json::json!({
                "DeploymentType": "NewDeployment",
                "GroupVersionId": version_id
            })),
        )
        .await;
        let first_id = string_at(&first, "DeploymentId");

        let mut body = serde_json::Map::new();
        body.insert(
            "DeploymentType".to_string(),
            serde_json::Value::String((*deployment_type).to_string()),
        );
        body.insert(
            "GroupVersionId".to_string(),
            serde_json::Value::String(version_id.clone()),
        );
        body.insert(
            "DeploymentId".to_string(),
            serde_json::Value::String(first_id.clone()),
        );

        let (status, decoded) = call(
            &router,
            "POST",
            "us-east-1",
            &format!("/greengrass/groups/{group_id}/deployments"),
            Some(serde_json::Value::Object(body)),
        )
        .await;

        let valid_for_create = matches!(*deployment_type, "NewDeployment" | "Redeployment");
        if valid_for_create {
            assert_eq!(
                status, 200,
                "case {deployment_type}: expected HTTP 200, got {status} with body {decoded:?}"
            );
            let deployment_id = string_at(&decoded, "DeploymentId");
            assert_ne!(
                deployment_id, first_id,
                "case {deployment_type}: a second deployment must get its own id"
            );
            let reported = ok(
                &router,
                "GET",
                &format!("/greengrass/groups/{group_id}/deployments/{deployment_id}/status"),
                None,
            )
            .await;
            assert_eq!(
                string_at(&reported, "DeploymentType"),
                *deployment_type,
                "case {deployment_type}: GetDeploymentStatus must report the stored type, got {reported:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {deployment_type}: expected HTTP 400, got {status} with body {decoded:?}"
            );
            assert_eq!(
                decoded.get("__type").and_then(serde_json::Value::as_str),
                Some("BadRequestException"),
                "case {deployment_type}: expected BadRequestException, got {decoded:?}"
            );
        }
    }
}

/// A `GroupVersionId` the caller supplied is never ignored: whichever deployment type
/// is used, it must be a version the group holds, and a `Redeployment` that names none
/// inherits the version of the deployment it replays. Rows span both types that
/// `CreateDeployment` accepts, with a version this group does not hold and with none.
#[tokio::test]
async fn create_deployment_refuses_a_group_version_the_group_does_not_hold() {
    let router = router();
    let group_id = create_group(&router, "version-check").await;
    let version = ok(
        &router,
        "POST",
        &format!("/greengrass/groups/{group_id}/versions"),
        Some(serde_json::json!({})),
    )
    .await;
    let version_id = string_at(&version, "Version");
    let deployments = format!("/greengrass/groups/{group_id}/deployments");

    let first = ok(
        &router,
        "POST",
        &deployments,
        Some(serde_json::json!({
            "DeploymentType": "NewDeployment",
            "GroupVersionId": version_id
        })),
    )
    .await;
    let first_id = string_at(&first, "DeploymentId");

    // A version no group holds: one minted by another group, and one that is merely
    // a well-formed id nobody minted.
    let other_group = create_group(&router, "version-check-other").await;
    let foreign_version = string_at(
        &ok(
            &router,
            "POST",
            &format!("/greengrass/groups/{other_group}/versions"),
            Some(serde_json::json!({})),
        )
        .await,
        "Version",
    );
    let unknown_versions = [
        foreign_version.as_str(),
        "00000000-0000-4000-8000-000000000000",
    ];

    for deployment_type in ["NewDeployment", "Redeployment"] {
        for unknown in unknown_versions {
            let (status, body) = call(
                &router,
                "POST",
                "us-east-1",
                &deployments,
                Some(serde_json::json!({
                    "DeploymentType": deployment_type,
                    "GroupVersionId": unknown,
                    "DeploymentId": first_id
                })),
            )
            .await;
            assert_eq!(
                status, 400,
                "case ({deployment_type}, {unknown}): a version this group does not hold must be refused, got {status} with body {body:?}"
            );
            assert_eq!(
                body.get("__type").and_then(serde_json::Value::as_str),
                Some("BadRequestException"),
                "case ({deployment_type}, {unknown}): expected BadRequestException, got {body:?}"
            );
        }
    }

    // A `Redeployment` naming no version inherits the replayed deployment's.
    let inherited = ok(
        &router,
        "POST",
        &deployments,
        Some(serde_json::json!({
            "DeploymentType": "Redeployment",
            "DeploymentId": first_id
        })),
    )
    .await;
    let inherited_id = string_at(&inherited, "DeploymentId");
    assert_ne!(
        inherited_id, first_id,
        "case Redeployment without a version: the replay must get its own id, got {inherited:?}"
    );
    let reported = ok(
        &router,
        "GET",
        &format!("/greengrass/groups/{group_id}/deployments/{inherited_id}/status"),
        None,
    )
    .await;
    assert_eq!(
        string_at(&reported, "DeploymentType"),
        "Redeployment",
        "case Redeployment without a version: the stored type must be reported, got {reported:?}"
    );
}

/// Every id this service hands out is a UUID: the canonical 8-4-4-4-12 lowercase
/// hexadecimal form, version 4, RFC 4122 variant — and unique. Rows cover each kind of
/// id the slice mints (group, group version, deployment) over many mints.
#[tokio::test]
async fn every_minted_id_is_a_unique_version_4_uuid() {
    /// The canonical form: five hyphen-separated groups of 8, 4, 4, 4 and 12 lowercase
    /// hex digits, with the version nibble and variant bits of RFC 4122 §4.4.
    fn assert_uuid_v4(kind: &str, id: &str) {
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case {kind} {id:?}: expected the 8-4-4-4-12 UUID form"
        );
        assert!(
            id.chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {kind} {id:?}: expected lowercase hexadecimal digits only"
        );
        assert_eq!(
            groups[2].chars().next(),
            Some('4'),
            "case {kind} {id:?}: expected version 4 in the third group"
        );
        let variant = groups[3].chars().next().unwrap_or('0');
        assert!(
            matches!(variant, '8' | '9' | 'a' | 'b'),
            "case {kind} {id:?}: expected the RFC 4122 variant, got {variant:?}"
        );
    }

    let router = router();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for index in 0..32 {
        let group_id = create_group(&router, &format!("uuid-{index}")).await;
        let version = ok(
            &router,
            "POST",
            &format!("/greengrass/groups/{group_id}/versions"),
            Some(serde_json::json!({})),
        )
        .await;
        let version_id = string_at(&version, "Version");
        let deployment = ok(
            &router,
            "POST",
            &format!("/greengrass/groups/{group_id}/deployments"),
            Some(serde_json::json!({
                "DeploymentType": "NewDeployment",
                "GroupVersionId": version_id
            })),
        )
        .await;
        let deployment_id = string_at(&deployment, "DeploymentId");

        for (kind, id) in [
            ("group id", &group_id),
            ("group version id", &version_id),
            ("deployment id", &deployment_id),
        ] {
            assert_uuid_v4(kind, id);
            assert!(
                seen.insert(id.clone()),
                "case {kind} {id:?}: every minted id must be unique, but this one repeated"
            );
        }
    }
}

/// A deployment is accepted in a non-terminal status and the clock — not a read —
/// carries it to `Success`. Two polls taken straight after the create see the
/// transient status, so a caller cannot miss it; waiting out the span reaches the
/// terminal one and stamps a new `UpdatedAt`.
#[tokio::test]
async fn a_deployments_status_advances_on_the_clock_not_on_being_read() {
    let router = router();
    let group_id = create_group(&router, "clock").await;
    let version = ok(
        &router,
        "POST",
        &format!("/greengrass/groups/{group_id}/versions"),
        Some(serde_json::json!({})),
    )
    .await;
    let version_id = string_at(&version, "Version");
    let deployment = ok(
        &router,
        "POST",
        &format!("/greengrass/groups/{group_id}/deployments"),
        Some(serde_json::json!({
            "DeploymentType": "NewDeployment",
            "GroupVersionId": version_id
        })),
    )
    .await;
    let deployment_id = string_at(&deployment, "DeploymentId");
    let status_path = format!("/greengrass/groups/{group_id}/deployments/{deployment_id}/status");

    let first = ok(&router, "GET", &status_path, None).await;
    let second = ok(&router, "GET", &status_path, None).await;
    // Non-terminal, not one exact value: which of the two transient statuses a poll
    // lands on depends on how much of the 300 ms step has elapsed, and asserting one
    // of them would make the test fail on a loaded machine for correct behaviour.
    let transient = ["Building", "InProgress"];
    assert!(
        transient.contains(&string_at(&first, "DeploymentStatus").as_str()),
        "case immediate poll: a caller acting straight after CreateDeployment must see a non-terminal status, got {first:?}"
    );
    assert_eq!(
        string_at(&second, "DeploymentStatus"),
        string_at(&first, "DeploymentStatus"),
        "case repeated poll: reading must not advance the status, got {second:?}"
    );
    assert_eq!(
        string_at(&second, "UpdatedAt"),
        string_at(&first, "UpdatedAt"),
        "case repeated poll: UpdatedAt must not move without a status change, got {second:?}"
    );

    // Wait out both owed transitions, then the status is terminal. `std::thread::sleep`
    // rather than `tokio::time::sleep`: the workspace enables tokio's `macros`/`rt`
    // features only, so the `time` driver this test would need is not built.
    std::thread::sleep(std::time::Duration::from_millis(900));
    let settled = ok(&router, "GET", &status_path, None).await;
    assert_eq!(
        string_at(&settled, "DeploymentStatus"),
        "Success",
        "case after the owed span: expected Success, got {settled:?}"
    );
    // Stamped when the status changed, so it never moves BACKWARDS. Compared as an
    // ordering rather than an inequality: both strings are the same ISO-8601 rendering,
    // which sorts lexicographically, and an equal pair is correct behaviour whenever the
    // rendering carries no sub-second digits.
    let settled_updated_at = string_at(&settled, "UpdatedAt");
    let first_updated_at = string_at(&first, "UpdatedAt");
    assert!(
        settled_updated_at >= first_updated_at,
        "case after the owed span: UpdatedAt must not move backwards, got {settled_updated_at:?} after {first_updated_at:?}"
    );
    assert!(
        settled.get("ErrorMessage").is_none_or(serde_json::Value::is_null),
        "case after the owed span: a successful deployment reports no ErrorMessage, got {settled:?}"
    );
}

/// A create retried under the same `X-Amzn-Client-Token` answers with the resource the
/// first attempt made rather than making a second one; a different token makes a
/// second resource. Rows cover each create in the slice.
#[tokio::test]
async fn a_retried_create_under_one_client_token_is_idempotent() {
    let router = router();

    // CreateGroup.
    let first = ok_with_token(&router, "POST", "/greengrass/groups", Some(serde_json::json!({"Name": "idem"})), "token-a").await;
    let retry = ok_with_token(&router, "POST", "/greengrass/groups", Some(serde_json::json!({"Name": "idem"})), "token-a").await;
    let other = ok_with_token(&router, "POST", "/greengrass/groups", Some(serde_json::json!({"Name": "idem"})), "token-b").await;
    assert_eq!(
        string_at(&first, "Id"),
        string_at(&retry, "Id"),
        "case CreateGroup retry: the same token must return the same group, got {retry:?}"
    );
    assert_ne!(
        string_at(&first, "Id"),
        string_at(&other, "Id"),
        "case CreateGroup new token: a different token must create a new group, got {other:?}"
    );
    let group_id = string_at(&first, "Id");
    let listed = ok(&router, "GET", "/greengrass/groups", None).await;
    assert_eq!(
        listed.get("Groups").and_then(serde_json::Value::as_array).map(Vec::len),
        Some(2),
        "case CreateGroup retry: the retry must not have created a third group, got {listed:?}"
    );

    // CreateGroupVersion.
    let versions_path = format!("/greengrass/groups/{group_id}/versions");
    let v1 = ok_with_token(&router, "POST", &versions_path, Some(serde_json::json!({})), "v-token").await;
    let v1_retry = ok_with_token(&router, "POST", &versions_path, Some(serde_json::json!({})), "v-token").await;
    assert_eq!(
        string_at(&v1, "Version"),
        string_at(&v1_retry, "Version"),
        "case CreateGroupVersion retry: the same token must return the same version, got {v1_retry:?}"
    );

    // CreateDeployment.
    let version_id = string_at(&v1, "Version");
    let deployments_path = format!("/greengrass/groups/{group_id}/deployments");
    let body = serde_json::json!({ "DeploymentType": "NewDeployment", "GroupVersionId": version_id });
    let d1 = ok_with_token(&router, "POST", &deployments_path, Some(body.clone()), "d-token").await;
    let d1_retry = ok_with_token(&router, "POST", &deployments_path, Some(body), "d-token").await;
    assert_eq!(
        string_at(&d1, "DeploymentId"),
        string_at(&d1_retry, "DeploymentId"),
        "case CreateDeployment retry: the same token must return the same deployment, got {d1_retry:?}"
    );

    // A client token is scoped to the resource the create is scoped to: the same
    // token against a DIFFERENT group names a different version/deployment, and must
    // not disturb the first group's idempotency.
    let other_group = string_at(&other, "Id");
    let other_versions_path = format!("/greengrass/groups/{other_group}/versions");
    let other_v = ok_with_token(
        &router,
        "POST",
        &other_versions_path,
        Some(serde_json::json!({})),
        "v-token",
    )
    .await;
    assert_ne!(
        string_at(&other_v, "Version"),
        string_at(&v1, "Version"),
        "case cross-group token: the same token in another group must mint its own version, got {other_v:?}"
    );
    let v1_again = ok_with_token(
        &router,
        "POST",
        &versions_path,
        Some(serde_json::json!({})),
        "v-token",
    )
    .await;
    assert_eq!(
        string_at(&v1_again, "Version"),
        string_at(&v1, "Version"),
        "case cross-group token: the first group's token must still replay its own version, got {v1_again:?}"
    );
}

/// `ListGroups` bounds its own response: with no `MaxResults` it applies the server's
/// default page size rather than rendering the whole store, and a caller-supplied
/// `MaxResults` above the server ceiling is clamped to it. Rows span below, at and
/// above the ceiling, and the default is below it so the two are distinguishable.
#[tokio::test]
async fn list_groups_caps_the_page_size_server_side() {
    // The server's default and maximum page size, from `DEFAULT_PAGE_SIZE` /
    // `MAX_PAGE_SIZE` in `src/ops/list_groups.rs`.
    const SERVER_DEFAULT_PAGE: usize = 50;
    const SERVER_PAGE_CAP: usize = 100;

    let router = router();
    for index in 0..SERVER_PAGE_CAP + 1 {
        create_group(&router, &format!("capped-{index}")).await;
    }

    let cases: &[(Option<&str>, usize)] = &[
        (None, SERVER_DEFAULT_PAGE),
        (Some("1"), 1),
        (Some("99"), 99),
        (Some("100"), SERVER_PAGE_CAP),
        (Some("101"), SERVER_PAGE_CAP),
        (Some("100000"), SERVER_PAGE_CAP),
    ];

    for (max_results, expected) in cases {
        let path = match max_results {
            Some(value) => format!("/greengrass/groups?MaxResults={value}"),
            None => "/greengrass/groups".to_string(),
        };
        let body = ok(&router, "GET", &path, None).await;
        let entries = body
            .get("Groups")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {max_results:?}: expected a Groups list, got {body:?}"));
        assert_eq!(
            entries.len(),
            *expected,
            "case MaxResults={max_results:?}: expected {expected} entries, got {}",
            entries.len()
        );
        assert!(
            body.get("NextToken")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "case MaxResults={max_results:?}: a short page must carry a NextToken, got {body:?}"
        );
    }
}

/// `DeleteGroup` releases everything the group held, including the client tokens that
/// named its resources: a create/delete loop under fresh tokens does not accumulate
/// state, and a token whose group is gone no longer replays.
#[tokio::test]
async fn delete_group_releases_the_client_tokens_that_named_its_resources() {
    let router = router();

    for round in 0..5 {
        let token = format!("loop-token-{round}");
        let created = ok_with_token(
            &router,
            "POST",
            "/greengrass/groups",
            Some(serde_json::json!({ "Name": format!("looped-{round}") })),
            &token,
        )
        .await;
        let group_id = string_at(&created, "Id");

        // The same token still replays while the group lives.
        let replayed = ok_with_token(
            &router,
            "POST",
            "/greengrass/groups",
            Some(serde_json::json!({ "Name": format!("looped-{round}") })),
            &token,
        )
        .await;
        assert_eq!(
            string_at(&replayed, "Id"),
            group_id,
            "case round {round}: the live token must replay, got {replayed:?}"
        );

        let (status, body) = call(
            &router,
            "DELETE",
            "us-east-1",
            &format!("/greengrass/groups/{group_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case round {round}: DeleteGroup expected 200, got {status} ({body:?})"
        );

        // After the delete the token no longer names a live resource, so the next
        // create mints a new group instead of resurrecting the deleted one.
        let after = ok_with_token(
            &router,
            "POST",
            "/greengrass/groups",
            Some(serde_json::json!({ "Name": format!("looped-{round}") })),
            &token,
        )
        .await;
        assert_ne!(
            string_at(&after, "Id"),
            group_id,
            "case round {round}: a token whose group was deleted must not replay it"
        );
        let (status, _) = call(
            &router,
            "DELETE",
            "us-east-1",
            &format!("/greengrass/groups/{}", string_at(&after, "Id")),
            None,
        )
        .await;
        assert_eq!(status, 200, "case round {round}: cleanup DeleteGroup failed");
    }

    let listed = ok(&router, "GET", "/greengrass/groups", None).await;
    assert_eq!(
        listed.get("Groups").and_then(serde_json::Value::as_array),
        Some(&vec![]),
        "the create/delete loop must leave no group behind, got {listed:?}"
    );
}

/// `ok`, with an `X-Amzn-Client-Token` header on the request.
async fn ok_with_token(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    token: &str,
) -> serde_json::Value {
    let mut req = request(method, "us-east-1", path, body);
    req.headers_mut().insert(
        "x-amzn-client-token",
        http::HeaderValue::from_str(token).expect("a valid token header"),
    );
    let response = router.dispatch(req).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {method} {path} token {token}: expected HTTP 200, got {status} with body {decoded:?}"
    );
    decoded
}

/// A group needs a name: `Name` is `@required` but `clientOptional`, so a blank one
/// reaches the handler and must be refused rather than stored.
#[tokio::test]
async fn create_group_refuses_a_blank_name() {
    let router = router();
    for name in ["", "   "] {
        let (status, body) = call(
            &router,
            "POST",
            "us-east-1",
            "/greengrass/groups",
            Some(serde_json::json!({ "Name": name })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case Name={name:?}: expected HTTP 400, got {status} with body {body:?}"
        );
    }
    let listed = ok(&router, "GET", "/greengrass/groups", None).await;
    assert_eq!(
        listed.get("Groups").and_then(serde_json::Value::as_array),
        Some(&vec![]),
        "a refused CreateGroup must store nothing, got {listed:?}"
    );
}

/// Each group's ARN carries the region the caller addressed, and every read renders
/// the identical string. Rows span several regions.
#[tokio::test]
async fn arns_carry_the_region_the_request_was_signed_for() {
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let router = router();
        let created = {
            let (status, body) = call(
                &router,
                "POST",
                region,
                "/greengrass/groups",
                Some(serde_json::json!({ "Name": "regional" })),
            )
            .await;
            assert_eq!(status, 200, "case {region}: expected 200, got {status} ({body:?})");
            body
        };
        let group_id = string_at(&created, "Id");
        let expected =
            format!("arn:aws:greengrass:{region}:000000000000:/greengrass/groups/{group_id}");
        assert_eq!(
            string_at(&created, "Arn"),
            expected,
            "case {region}: CreateGroup must mint the region's ARN"
        );

        let (status, fetched) = call(
            &router,
            "GET",
            region,
            &format!("/greengrass/groups/{group_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {region}: GetGroup expected 200, got {status}");
        assert_eq!(
            string_at(&fetched, "Arn"),
            expected,
            "case {region}: GetGroup must render the stored ARN"
        );

        let listed = {
            let (status, body) = call(&router, "GET", region, "/greengrass/groups", None).await;
            assert_eq!(status, 200, "case {region}: ListGroups expected 200, got {status}");
            body
        };
        let entries = listed
            .get("Groups")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {region}: expected a Groups list, got {listed:?}"));
        assert_eq!(
            string_at(&entries[0], "Arn"),
            expected,
            "case {region}: ListGroups must render the stored ARN"
        );
    }
}
