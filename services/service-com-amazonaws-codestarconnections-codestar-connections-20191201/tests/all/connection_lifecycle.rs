// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CodeStar connections connection lifecycle.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it
//! (`POST /` + `X-Amz-Target: CodeStar_connections_20191201.<Operation>`, awsJson1_0
//! body), so what is asserted here is the service's HTTP behaviour and not a handler
//! called directly.
//!
//! The tables below span the classes the rules generalise over — every modeled
//! `ProviderType`, the published `ConnectionName` length bounds, a range of page sizes,
//! several tag sets — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

/// The region the test signs for; the ARNs the service mints must carry it.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical emulator account id (`engine::emulator_account_id`).
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// awsJson1_0 target prefix, from `OPERATIONS.md` ("Wire protocol").
pub(crate) const TARGET_PREFIX: &str = "CodeStar_connections_20191201";

/// Every value the model's `ProviderType` enum accepts.
///
/// Referenced, not copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::PROVIDER_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="ProviderType")`), so the table below widens
/// with the model instead of drifting from it.
const PROVIDER_TYPES: &[&str] = codestar::ops::types::shared_types::PROVIDER_TYPE_VALUES;

/// A host ARN shaped like `CreateConnection.HostArn`'s modeled pattern
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`.
fn host_arn(name: &str) -> String {
    format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/{name}")
}

/// One emulator: the router the production front door builds, with this service mounted
/// on a single shared engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing, so no authenticator may be registered.
        router.set_request_authentication(false);
        router.register_service(codestar::EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one awsJson1_0 call under this service's short target prefix and return
    /// `(status, parsed body)`.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_target(&format!("{TARGET_PREFIX}.{operation}"), body)
            .await
    }

    /// Dispatch one awsJson1_0 call naming `target` verbatim in `X-Amz-Target`, so a
    /// suite can drive the prefix forms a client writes as well as the operation.
    pub(crate) async fn call_target(&self, target: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("https://codestar-connections.us-west-2.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", target)
            // The SigV4 credential scope is what routes the request to this service and
            // what tells it which region to mint ARNs in.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}\
                     /codestar-connections/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `CreateConnection`, asserting success, and return the minted ARN.
    pub(crate) async fn create(&self, case: &str, body: Value) -> String {
        let (status, response) = self.call("CreateConnection", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateConnection to answer 200, got {status} {response:?}"
        );
        response["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {response:?}"))
            .to_string()
    }
}

/// The error code an awsJson error envelope carries (`__type`).
pub(crate) fn error_code(response: &Value) -> String {
    response["__type"].as_str().unwrap_or_default().to_string()
}

/// Assert `arn` is a connection ARN of the modeled form
/// `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`, in this request's region, whose resource is
/// `connection/<uuid>`.
fn assert_connection_arn(case: &str, arn: &str) {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected a 6-field ARN, got {arn:?}"
    );
    assert_eq!(
        (fields[0], fields[1], fields[2], fields[3]),
        ("arn", "aws", "codestar-connections", REGION),
        "case {case:?}: wrong ARN prefix in {arn:?}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a 12-digit account id, got {:?} in {arn:?}",
        fields[4]
    );
    let Some(id) = fields[5].strip_prefix("connection/") else {
        panic!("case {case:?}: expected a connection/<id> resource, got {arn:?}");
    };
    // The id is a UUID: 8-4-4-4-12 lowercase hex.
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a UUID connection id, got {id:?}"
    );
    assert!(
        groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_hexdigit())),
        "case {case:?}: expected a hex UUID connection id, got {id:?}"
    );
    // RFC 4122: the version nibble (4) opens the third group and the variant nibble
    // (`10xx`, i.e. 8..b) the fourth, so a client parsing the id gets a valid UUID.
    assert!(
        groups[2].starts_with('4'),
        "case {case:?}: expected a version-4 UUID connection id, got {id:?}"
    );
    assert!(
        matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
        "case {case:?}: expected the RFC 4122 variant nibble in {id:?}"
    );
}

/// Read every connection by paging with `max_results`, following `NextToken` to the end.
async fn list_every_arn(emulator: &Emulator, case: &str, max_results: Option<u32>) -> Vec<String> {
    let mut arns = Vec::new();
    let mut next_token: Option<String> = None;
    // A page must make progress, so the loop cannot need more rounds than connections.
    for _ in 0..64 {
        let mut body = Map::new();
        if let Some(max) = max_results {
            body.insert("MaxResults".to_string(), json!(max));
        }
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let (status, response) = emulator.call("ListConnections", Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListConnections to answer 200, got {status} {response:?}"
        );
        let page = response["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Connections in {response:?}"));
        if let Some(max) = max_results.filter(|max| *max > 0) {
            assert!(
                page.len() <= max as usize,
                "case {case:?}: page of {} exceeds MaxResults {max}",
                page.len()
            );
        }
        for connection in page {
            arns.push(
                connection["ConnectionArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {connection:?}"))
                    .to_string(),
            );
        }
        match response["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return arns,
        }
    }
    panic!("case {case:?}: ListConnections never stopped emitting a NextToken");
}

/// RULE: a created connection is persisted and every reader rebuilds it from that
/// record — name, provider type, owner account and `PENDING` status — for any accepted
/// `ProviderType` and any `ConnectionName` inside the modeled length bounds.
#[tokio::test]
async fn a_created_connection_round_trips_through_get_for_every_provider_type() {
    let emulator = Emulator::new();

    // `ConnectionName` is modeled `len 1..32`: both published bounds plus a middle size.
    let name_lengths = [1usize, 17, 32];

    for provider in PROVIDER_TYPES {
        for length in name_lengths {
            let name = "c".repeat(length);
            let case = format!("{provider}/name-len-{length}");

            let arn = emulator
                .create(
                    &case,
                    json!({ "ConnectionName": name, "ProviderType": provider }),
                )
                .await;
            assert_connection_arn(&case, &arn);

            let (status, response) = emulator
                .call("GetConnection", json!({ "ConnectionArn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetConnection to answer 200, got {status} {response:?}"
            );
            let connection = &response["Connection"];
            assert_eq!(
                connection["ConnectionName"].as_str(),
                Some(name.as_str()),
                "case {case:?}: wrong ConnectionName in {connection:?}"
            );
            assert_eq!(
                connection["ConnectionArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: wrong ConnectionArn in {connection:?}"
            );
            assert_eq!(
                connection["ProviderType"].as_str(),
                Some(*provider),
                "case {case:?}: wrong ProviderType in {connection:?}"
            );
            assert_eq!(
                connection["OwnerAccountId"].as_str(),
                Some(ACCOUNT_ID),
                "case {case:?}: wrong OwnerAccountId in {connection:?}"
            );
            // The model's `Connection` doc: a connection created through the CLI/SDK/
            // CloudFormation is PENDING until it is updated in the console.
            assert_eq!(
                connection["ConnectionStatus"].as_str(),
                Some("PENDING"),
                "case {case:?}: wrong ConnectionStatus in {connection:?}"
            );
            assert!(
                connection.get("HostArn").is_none_or(Value::is_null),
                "case {case:?}: a connection created without a HostArn reported one: \
                 {connection:?}"
            );
        }
    }
}

/// RULE: every ARN the service mints is distinct, and a `HostArn` supplied at create is
/// part of the stored connection, reported back by `GetConnection`.
#[tokio::test]
async fn each_connection_gets_its_own_arn_and_keeps_its_host_arn() {
    let emulator = Emulator::new();

    let cases = [
        ("no host", None),
        ("host one", Some(host_arn("enterprise-one"))),
        ("host two", Some(host_arn("enterprise-two"))),
    ];

    let mut arns = Vec::new();
    for (case, host) in &cases {
        let mut body = json!({ "ConnectionName": case.replace(' ', "-"), "ProviderType": "GitHubEnterpriseServer" });
        if let Some(host) = host {
            body["HostArn"] = json!(host);
        }
        let arn = emulator.create(case, body).await;
        assert_connection_arn(case, &arn);

        let (status, response) = emulator
            .call("GetConnection", json!({ "ConnectionArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetConnection to answer 200, got {status} {response:?}"
        );
        assert_eq!(
            response["Connection"]["HostArn"].as_str(),
            host.as_deref(),
            "case {case:?}: wrong HostArn in {response:?}"
        );
        arns.push(arn);
    }

    // Repeated creates mint distinct connections, never one shared record.
    let unique: std::collections::BTreeSet<&String> = arns.iter().collect();
    assert_eq!(
        unique.len(),
        arns.len(),
        "expected {} distinct ARNs, got {arns:?}",
        arns.len()
    );
}

/// RULE: `ListConnections` reports every connection in state, and paging with any page
/// size yields each connection exactly once in a stable order.
#[tokio::test]
async fn paging_yields_every_connection_exactly_once_for_every_page_size() {
    let emulator = Emulator::new();

    let mut created = Vec::new();
    for index in 0..7 {
        let case = format!("connection-{index}");
        created.push(
            emulator
                .create(
                    &case,
                    json!({ "ConnectionName": case, "ProviderType": PROVIDER_TYPES[index % PROVIDER_TYPES.len()] }),
                )
                .await,
        );
    }

    // `MaxResults` is modeled `range 0..100`: both published bounds, sizes that divide
    // the set and sizes that do not, plus an omitted member (which arrives as the
    // modeled default 0, i.e. "no page size requested").
    let page_sizes = [None, Some(0), Some(1), Some(2), Some(3), Some(7), Some(100)];
    for max_results in page_sizes {
        let case = format!("MaxResults={max_results:?}");
        let listed = list_every_arn(&emulator, &case, max_results).await;
        assert_eq!(
            listed, created,
            "case {case:?}: expected every connection exactly once in creation order, \
             got {listed:?}"
        );
    }
}

/// RULE: a `NextToken` this service did not issue is refused as a bad request, not
/// silently treated as the first page.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create(
            "seed",
            json!({ "ConnectionName": "seed", "ProviderType": "GitHub" }),
        )
        .await;

    for token in ["not-a-token", "0", "zzzzzzzzzzzzzzzz", " 000000000000000"] {
        let case = format!("NextToken={token:?}");
        let (status, response) = emulator
            .call("ListConnections", json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for a forged NextToken, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code in {response:?}"
        );
    }
}

/// RULE: each `ListConnections` filter selects exactly the connections whose stored
/// member equals the filter value.
#[tokio::test]
async fn list_filters_select_by_stored_member() {
    let emulator = Emulator::new();

    let shared_host = host_arn("shared");
    let lone_host = host_arn("lone");

    // (name, provider type, host arn)
    let fixtures: [(&str, &str, Option<&str>); 4] = [
        ("github-hosted", "GitHubEnterpriseServer", Some(&shared_host)),
        ("gitlab-hosted", "GitLabSelfManaged", Some(&shared_host)),
        ("gitlab-lone", "GitLabSelfManaged", Some(&lone_host)),
        ("bitbucket-plain", "Bitbucket", None),
    ];
    let mut arn_of = std::collections::BTreeMap::new();
    for (name, provider, host) in fixtures {
        let mut body = json!({ "ConnectionName": name, "ProviderType": provider });
        if let Some(host) = host {
            body["HostArn"] = json!(host);
        }
        arn_of.insert(name, emulator.create(name, body).await);
    }

    let expectations: Vec<(Value, Vec<&str>)> = vec![
        (json!({}), vec![
            "github-hosted",
            "gitlab-hosted",
            "gitlab-lone",
            "bitbucket-plain",
        ]),
        (
            json!({ "ProviderTypeFilter": "GitLabSelfManaged" }),
            vec!["gitlab-hosted", "gitlab-lone"],
        ),
        (
            json!({ "ProviderTypeFilter": "Bitbucket" }),
            vec!["bitbucket-plain"],
        ),
        (json!({ "ProviderTypeFilter": "GitLab" }), vec![]),
        (
            json!({ "HostArnFilter": shared_host }),
            vec!["github-hosted", "gitlab-hosted"],
        ),
        (
            json!({ "HostArnFilter": lone_host }),
            vec!["gitlab-lone"],
        ),
        (
            json!({ "ProviderTypeFilter": "GitLabSelfManaged", "HostArnFilter": shared_host }),
            vec!["gitlab-hosted"],
        ),
        (
            json!({ "ProviderTypeFilter": "Bitbucket", "HostArnFilter": shared_host }),
            vec![],
        ),
    ];

    for (filter, expected_names) in expectations {
        let case = format!("filter={filter}");
        let (status, response) = emulator.call("ListConnections", filter).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListConnections to answer 200, got {status} {response:?}"
        );
        let got: Vec<String> = response["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Connections in {response:?}"))
            .iter()
            .map(|connection| connection["ConnectionArn"].as_str().unwrap_or("").to_string())
            .collect();
        let expected: Vec<String> = expected_names
            .iter()
            .map(|name| arn_of[name].clone())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected_names:?}, got {got:?}"
        );
    }
}

/// RULE: a resource's tags are what state holds for its ARN — create seeds them,
/// `TagResource` merges (a repeated key overwrites), `UntagResource` removes, and
/// `ListTagsForResource` reports exactly that map.
#[tokio::test]
async fn tags_are_read_back_from_state_after_every_mutation() {
    let emulator = Emulator::new();

    let arn = emulator
        .create(
            "tagged at create",
            json!({
                "ConnectionName": "tagged",
                "ProviderType": "GitHub",
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
        )
        .await;

    // Each row: the mutation to apply, then the whole tag map it must leave behind.
    type Step<'a> = (&'a str, Value, Vec<(&'a str, &'a str)>);
    let steps: Vec<Step<'_>> = vec![
        (
            "seeded by create",
            Value::Null,
            vec![("env", "dev")],
        ),
        (
            "TagResource adds a new key",
            json!({ "operation": "TagResource", "Tags": [{ "Key": "team", "Value": "infra" }] }),
            vec![("env", "dev"), ("team", "infra")],
        ),
        (
            "TagResource overwrites a repeated key",
            json!({ "operation": "TagResource", "Tags": [{ "Key": "env", "Value": "prod" }] }),
            vec![("env", "prod"), ("team", "infra")],
        ),
        (
            "TagResource applies several keys at once",
            json!({ "operation": "TagResource", "Tags": [
                { "Key": "owner", "Value": "platform" },
                { "Key": "team", "Value": "tools" }
            ] }),
            vec![("env", "prod"), ("owner", "platform"), ("team", "tools")],
        ),
        (
            "TagResource with an empty list changes nothing",
            json!({ "operation": "TagResource", "Tags": [] }),
            vec![("env", "prod"), ("owner", "platform"), ("team", "tools")],
        ),
        (
            "UntagResource removes a key",
            json!({ "operation": "UntagResource", "TagKeys": ["owner"] }),
            vec![("env", "prod"), ("team", "tools")],
        ),
        (
            "UntagResource ignores a key that is not there",
            json!({ "operation": "UntagResource", "TagKeys": ["absent"] }),
            vec![("env", "prod"), ("team", "tools")],
        ),
        (
            "UntagResource removes the rest",
            json!({ "operation": "UntagResource", "TagKeys": ["env", "team"] }),
            vec![],
        ),
    ];

    for (case, mutation, expected) in steps {
        if let Some(operation) = mutation.get("operation").and_then(Value::as_str) {
            let mut body = mutation.clone();
            let body = body.as_object_mut().expect("the mutation is an object");
            body.remove("operation");
            body.insert("ResourceArn".to_string(), json!(arn));
            let (status, response) = emulator
                .call(operation, Value::Object(body.clone()))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {operation} to answer 200, got {status} {response:?}"
            );
        }

        let (status, response) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} \
             {response:?}"
        );
        let got: Vec<(String, String)> = response["Tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Tags in {response:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or("").to_string(),
                    tag["Value"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        let want: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            got, want,
            "case {case:?}: expected tags {want:?}, got {got:?}"
        );
    }
}

/// RULE: `CreateConnection` reports the tags it stored, so the create response and a
/// later `ListTagsForResource` agree.
#[tokio::test]
async fn create_reports_the_tags_it_stored() {
    let emulator = Emulator::new();

    let tag_sets: Vec<Value> = vec![
        json!([{ "Key": "env", "Value": "dev" }]),
        json!([
            { "Key": "b", "Value": "2" },
            { "Key": "a", "Value": "1" }
        ]),
        // A repeated key inside one request: the last value wins, as a merge does.
        json!([
            { "Key": "k", "Value": "first" },
            { "Key": "k", "Value": "second" }
        ]),
    ];

    for (index, tags) in tag_sets.into_iter().enumerate() {
        let case = format!("tag-set-{index}");
        let (status, response) = emulator
            .call(
                "CreateConnection",
                json!({ "ConnectionName": case, "ProviderType": "GitHub", "Tags": tags }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateConnection to answer 200, got {status} {response:?}"
        );
        let arn = response["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {response:?}"))
            .to_string();
        let created_tags = response["Tags"].clone();

        let (status, listed) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {listed:?}"
        );
        assert_eq!(
            created_tags, listed["Tags"],
            "case {case:?}: CreateConnection reported {created_tags:?} but state holds {:?}",
            listed["Tags"]
        );
    }
}

/// RULE: deleting a connection removes it and its tags from state, so every later
/// reader answers as it does for an ARN that was never created.
#[tokio::test]
async fn delete_removes_the_connection_and_its_tags() {
    let emulator = Emulator::new();

    let kept = emulator
        .create(
            "kept",
            json!({ "ConnectionName": "kept", "ProviderType": "GitHub" }),
        )
        .await;
    let doomed = emulator
        .create(
            "doomed",
            json!({
                "ConnectionName": "doomed",
                "ProviderType": "GitLab",
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
        )
        .await;

    let (status, response) = emulator
        .call("DeleteConnection", json!({ "ConnectionArn": doomed }))
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteConnection to answer 200, got {status} {response:?}"
    );
    assert!(
        response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty),
        "expected DeleteConnection to answer an empty body, got {response:?}"
    );

    assert_eq!(
        list_every_arn(&emulator, "after delete", Some(10)).await,
        vec![kept.clone()],
        "a deleted connection was still listed"
    );

    // Every reader of the deleted ARN now answers 404, including a second delete.
    let readers: Vec<(&str, Value)> = vec![
        ("GetConnection", json!({ "ConnectionArn": doomed })),
        ("DeleteConnection", json!({ "ConnectionArn": doomed })),
        ("ListTagsForResource", json!({ "ResourceArn": doomed })),
        (
            "TagResource",
            json!({ "ResourceArn": doomed, "Tags": [{ "Key": "env", "Value": "dev" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceArn": doomed, "TagKeys": ["env"] }),
        ),
    ];
    for (operation, body) in readers {
        let (status, response) = emulator.call(operation, body).await;
        assert_eq!(
            status, 404,
            "case {operation:?}: expected 404 for a deleted ARN, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {operation:?}: wrong error code in {response:?}"
        );
    }

    // The surviving connection is untouched.
    let (status, response) = emulator
        .call("GetConnection", json!({ "ConnectionArn": kept }))
        .await;
    assert_eq!(
        status, 200,
        "expected the surviving connection to still resolve, got {status} {response:?}"
    );
}

/// RULE: an ARN naming no stored connection is a `ResourceNotFoundException` (404) on
/// every operation that addresses one, whatever the ARN looks like.
#[tokio::test]
async fn an_arn_naming_no_connection_is_a_404() {
    let emulator = Emulator::new();
    // Create one connection so state is non-empty: the 404 must come from the lookup,
    // not from an empty store.
    emulator
        .create(
            "seed",
            json!({ "ConnectionName": "seed", "ProviderType": "GitHub" }),
        )
        .await;

    // Each ARN satisfies the modeled pattern `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`
    // but names nothing this emulator minted.
    let unknown_arns = [
        format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/00000000-0000-0000-0000-000000000000"),
        format!("arn:aws:codestar-connections:eu-west-1:{ACCOUNT_ID}:connection/11111111-2222-3333-4444-555555555555"),
        format!("arn:aws-cn:codestar-connections:cn-north-1:{ACCOUNT_ID}:connection/deadbeef"),
        format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/some-host"),
    ];

    for arn in &unknown_arns {
        for (operation, body) in [
            ("GetConnection", json!({ "ConnectionArn": arn })),
            ("DeleteConnection", json!({ "ConnectionArn": arn })),
            ("ListTagsForResource", json!({ "ResourceArn": arn })),
            (
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": [{ "Key": "k", "Value": "v" }] }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn, "TagKeys": ["k"] }),
            ),
        ] {
            let case = format!("{operation} {arn}");
            let (status, response) = emulator.call(operation, body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {response:?}"
            );
        }
    }
}

/// RULE: a request outside a modeled constraint is refused before anything is stored —
/// `ConnectionName` is `len 1..32`, so each published bound holds and one past it does
/// not.
#[tokio::test]
async fn a_connection_name_outside_the_modeled_bounds_is_refused() {
    let emulator = Emulator::new();

    // (length, must be accepted)
    let cases = [(0usize, false), (1, true), (32, true), (33, false)];
    for (length, accepted) in cases {
        let case = format!("name-len-{length}");
        let (status, response) = emulator
            .call(
                "CreateConnection",
                json!({ "ConnectionName": "n".repeat(length), "ProviderType": "GitHub" }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a name inside len 1..32 was refused: {status} {response:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a name outside len 1..32 was accepted: {status} {response:?}"
            );
        }
    }

    // Only the accepted names reached state.
    let listed = list_every_arn(&emulator, "accepted only", None).await;
    assert_eq!(
        listed.len(),
        2,
        "expected only the two in-bounds names to be stored, got {listed:?}"
    );
}

/// RULE: the number of connections one emulator holds is bounded, and a create past
/// the bound is refused with the `LimitExceededException` the model declares for it —
/// so repeated creates cannot grow state without limit.
#[tokio::test]
async fn creating_past_the_connection_bound_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let bound = codestar::state::MAX_CONNECTIONS;

    for index in 0..bound {
        let case = format!("fill-{index}");
        emulator
            .create(
                &case,
                json!({ "ConnectionName": format!("c{index}"), "ProviderType": "GitHub" }),
            )
            .await;
    }

    // Two attempts past the bound: the refusal is the steady state, not a one-off.
    for attempt in 0..2 {
        let case = format!("past-bound-{attempt}");
        let (status, response) = emulator
            .call(
                "CreateConnection",
                json!({ "ConnectionName": "overflow", "ProviderType": "GitHub" }),
            )
            .await;
        assert_eq!(
            status, 429,
            "case {case:?}: expected 429 past the {bound}-connection bound, got {status} \
             {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "LimitExceededException",
            "case {case:?}: wrong error code in {response:?}"
        );
    }

    // The refused creates stored nothing.
    let listed = list_every_arn(&emulator, "at the bound", Some(100)).await;
    assert_eq!(
        listed.len(),
        bound,
        "expected exactly {bound} connections at the bound, got {}",
        listed.len()
    );

    // Deleting one makes room again: the bound is on what is held, not on how many
    // creates were ever made.
    let (status, response) = emulator
        .call("DeleteConnection", json!({ "ConnectionArn": listed[0] }))
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteConnection to answer 200, got {status} {response:?}"
    );
    let arn = emulator
        .create(
            "after freeing a slot",
            json!({ "ConnectionName": "replacement", "ProviderType": "GitHub" }),
        )
        .await;
    assert_connection_arn("after freeing a slot", &arn);
}

/// RULE: a resource holds at most as many tags as one request may set (`TagList` is
/// modeled `len 0..200`), and a merge past that is refused with the modeled
/// `LimitExceededException` — a key that is only overwritten still fits.
#[tokio::test]
async fn tagging_past_the_per_resource_bound_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let bound = codestar::state::MAX_TAGS_PER_RESOURCE;

    let full: Vec<Value> = (0..bound)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
        .collect();
    let arn = emulator
        .create(
            "full at create",
            json!({ "ConnectionName": "tagged", "ProviderType": "GitHub", "Tags": full }),
        )
        .await;

    // Each row: the tag list to merge, whether it must be accepted.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "overwriting an existing key keeps the count",
            json!([{ "Key": "k0", "Value": "changed" }]),
            true,
        ),
        (
            "one key past the bound",
            json!([{ "Key": "overflow", "Value": "v" }]),
            false,
        ),
        (
            "several keys past the bound",
            json!([
                { "Key": "overflow-a", "Value": "v" },
                { "Key": "overflow-b", "Value": "v" }
            ]),
            false,
        ),
    ];

    for (case, tags, accepted) in cases {
        let (status, response) = emulator
            .call("TagResource", json!({ "ResourceArn": arn, "Tags": tags }))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
        } else {
            assert_eq!(
                status, 429,
                "case {case:?}: expected 429 past the {bound}-tag bound, got {status} \
                 {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "LimitExceededException",
                "case {case:?}: wrong error code in {response:?}"
            );
        }

        let (status, listed) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} \
             {listed:?}"
        );
        let count = listed["Tags"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, bound,
            "case {case:?}: expected the resource to still hold {bound} tags, got {count}"
        );
    }
}

/// RULE: state is shared across requests, so a connection created by one request is
/// visible to the next — and reading it never changes what a later reader sees.
#[tokio::test]
async fn reads_do_not_change_what_a_later_reader_sees() {
    let emulator = Emulator::new();
    let arn = emulator
        .create(
            "stable",
            json!({
                "ConnectionName": "stable",
                "ProviderType": "Bitbucket",
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
        )
        .await;

    let mut seen = Vec::new();
    for round in 0..3 {
        let case = format!("round-{round}");
        let (status, got) = emulator
            .call("GetConnection", json!({ "ConnectionArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {got:?}"
        );
        let (status, tags) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {tags:?}"
        );
        let listed = list_every_arn(&emulator, &case, Some(1)).await;
        assert_eq!(
            listed,
            vec![arn.clone()],
            "case {case:?}: ListConnections changed under repeated reads"
        );
        seen.push((got["Connection"].clone(), tags["Tags"].clone()));
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "repeated reads disagreed: {seen:?}"
    );
}
