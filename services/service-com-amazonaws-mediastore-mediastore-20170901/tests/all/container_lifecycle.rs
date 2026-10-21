// SPDX-License-Identifier: CC0-1.0

//! The container lifecycle and the container policy round-trip, end to end over the
//! wire: `CreateContainer`, `DescribeContainer`, `ListContainers`,
//! `PutContainerPolicy`, `GetContainerPolicy` and `DeleteContainer`.
//!
//! Every case drives the real `awsJson1_1` request walk through
//! [`MultiServiceRouter::dispatch`] — deserialize, modelled-constraint validation,
//! handler, serialize — so what these tests assert is what an SDK client sees, and the
//! state each request writes is reached by the next request through the one shared
//! engine.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// One mounted MediaStore emulator over one shared engine, driven by signed-looking
/// requests whose SigV4 credential scope names `region`.
struct Client {
    router: MultiServiceRouter,
    region: &'static str,
}

impl Client {
    /// A client in `us-west-2`.
    fn new() -> Self {
        Self::in_region("us-west-2")
    }

    fn in_region(region: &'static str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediastore_mediastore_20170901::EmulatedService::new(engine),
        );
        Self { router, region }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed JSON body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{}/mediastore/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
                    self.region
                ),
            )
            .header("x-amz-target", format!("MediaStore_20170901.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require a 200, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} was expected to succeed, got {status} {parsed:?}"
        );
        parsed
    }

    /// Create `name` and require a 200, answering the returned `Container`.
    async fn create(&self, name: &str) -> Value {
        self.ok("CreateContainer", json!({ "ContainerName": name }))
            .await["Container"]
            .clone()
    }
}

/// The wire error code of a failure body: the `__type` discriminator awsJson1_1
/// carries, without the optional `namespace#` prefix.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split('#')
        .next_back()
        .unwrap_or_default()
}

/// A member of a returned `Container`, as a string.
fn member<'a>(container: &'a Value, name: &str) -> &'a str {
    container
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `name`, hex-encoded: the first half of the pagination-token form this service
/// issues (`<hex(name)>=<keyed digest>`), used to build tokens it never issued.
fn hex_name(name: &str) -> String {
    name.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Names spanning the modelled `ContainerName` catalog: `^[\w-]+$` (letters, digits,
/// underscore and hyphen) with length 1..255. Source: `smithy_query(query="shape",
/// shape="Container")`, member `Name`, and the identical constraints on
/// `CreateContainerInput$ContainerName`. Each row is `(case, name)`.
fn name_catalog() -> Vec<(&'static str, String)> {
    vec![
        ("a hyphenated name", "demo-container".to_string()),
        ("the minimum length of 1", "a".to_string()),
        ("the maximum length of 255", "n".repeat(255)),
        ("digits and mixed case", "Movies2024".to_string()),
        ("every character class at once", "_a-B_9".to_string()),
    ]
}

/// Policy documents spanning the modelled `ContainerPolicy` catalog: length 1..8192
/// over the charset `^[\x00-\x7F]+$` (any 7-bit ASCII byte). Source:
/// `smithy_query(query="operation", operation="PutContainerPolicy")`, member `Policy`.
/// Each row is `(case, policy)`.
fn policy_catalog() -> Vec<(&'static str, String)> {
    vec![
        ("the minimum length of 1", "x".to_string()),
        ("an empty JSON document", "{}".to_string()),
        (
            "a resource policy with newlines and tabs",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": []\n}".to_string(),
        ),
        (
            "every printable ASCII character",
            (0x20u8..0x7f).map(char::from).collect(),
        ),
        ("the maximum length of 8192", "p".repeat(8192)),
    ]
}

/// RULE: a created container is a stored record — the ARN, endpoint, creation time,
/// name, status and access-logging flag `CreateContainer` minted are what every later
/// read of that container reports, for any name the modelled catalog admits.
#[tokio::test]
async fn a_created_container_is_read_back_from_state() {
    let client = Client::new();
    let mut endpoints: Vec<String> = Vec::new();

    for (case, name) in name_catalog() {
        let created = client.create(&name).await;

        assert_eq!(
            member(&created, "Name"),
            name,
            "case {case:?}: expected Name {name:?}, got {created:?}"
        );
        assert_eq!(
            created.get("Status").and_then(Value::as_str),
            Some("CREATING"),
            "case {case:?}: a container is CREATING until the endpoint is available, got {created:?}"
        );
        assert_eq!(
            created.get("AccessLoggingEnabled").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: access logging is false by default, got {created:?}"
        );
        // "The return value does not include the container's endpoint. To make
        // downstream requests, you must obtain this value by using DescribeContainer or
        // ListContainers."
        assert!(
            created.get("Endpoint").is_none(),
            "case {case:?}: CreateContainer must not report the endpoint, got {created:?}"
        );

        // The ARN carries the signed region, the emulator's account and the name:
        // `arn:aws:mediastore:<region>:<12 digits>:container/<name>`.
        let arn = member(&created, "ARN").to_string();
        let fields: Vec<&str> = arn.split(':').collect();
        assert_eq!(
            fields.len(),
            6,
            "case {case:?}: expected a 6-field ARN, got {arn:?}"
        );
        assert_eq!(
            &fields[..3],
            &["arn", "aws", "mediastore"],
            "case {case:?}: expected an aws mediastore ARN, got {arn:?}"
        );
        assert_eq!(
            fields[3], client.region,
            "case {case:?}: expected the signed region in {arn:?}"
        );
        assert!(
            fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
            "case {case:?}: expected a 12-digit account in {arn:?}"
        );
        assert_eq!(
            fields[5],
            format!("container/{name}"),
            "case {case:?}: expected the container resource in {arn:?}"
        );

        // DescribeContainer answers from the same record, endpoint included.
        let described = client
            .ok("DescribeContainer", json!({ "ContainerName": name }))
            .await["Container"]
            .clone();
        for reported in ["ARN", "Name", "Status", "CreationTime", "AccessLoggingEnabled"] {
            assert_eq!(
                described.get(reported),
                created.get(reported),
                "case {case:?}: DescribeContainer changed {reported}: {described:?} vs {created:?}"
            );
        }

        let endpoint = member(&described, "Endpoint").to_string();
        let expected_suffix = format!(".data.mediastore.{}.amazonaws.com", client.region);
        let label = endpoint
            .strip_prefix("https://")
            .and_then(|rest| rest.strip_suffix(&expected_suffix))
            .unwrap_or_default();
        assert!(
            !label.is_empty() && label.bytes().all(|b| b.is_ascii_hexdigit()) && endpoint.len() <= 255,
            "case {case:?}: expected https://<label>{expected_suffix} within 255 characters, got {endpoint:?}"
        );
        assert!(
            !endpoints.contains(&endpoint),
            "case {case:?}: endpoint {endpoint:?} is already assigned to another container"
        );
        // Reading twice reports the same endpoint: it is stored, not re-minted.
        let again = client
            .ok("DescribeContainer", json!({ "ContainerName": name }))
            .await["Container"]
            .clone();
        assert_eq!(
            member(&again, "Endpoint"),
            endpoint,
            "case {case:?}: the endpoint changed between reads, got {again:?}"
        );
        endpoints.push(endpoint);

        // The name is unique to the account: creating it again never overwrites.
        let (status, body) = client
            .call("CreateContainer", json!({ "ContainerName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ContainerInUseException"),
            "case {case:?}: re-creating an existing container must be refused, got {status} {body:?}"
        );
    }

    // Every container created above is listed, in name order.
    let listed = client.ok("ListContainers", json!({})).await;
    let mut expected: Vec<String> = name_catalog().into_iter().map(|(_, name)| name).collect();
    expected.sort();
    let got: Vec<String> = listed["Containers"]
        .as_array()
        .expect("Containers is a required list")
        .iter()
        .map(|container| member(container, "Name").to_string())
        .collect();
    assert_eq!(
        got, expected,
        "ListContainers must enumerate the stored containers in name order, got {got:?}"
    );
    assert!(
        listed.get("NextToken").is_none(),
        "a single unpaged response must carry no NextToken, got {listed:?}"
    );
}

/// RULE: deleting a container removes the record, so every later read of that name is
/// refused as unknown and no listing includes it — for any name the catalog admits.
#[tokio::test]
async fn a_deleted_container_is_gone_from_state() {
    let client = Client::new();
    for (_, name) in name_catalog() {
        client.create(&name).await;
    }

    for (case, name) in name_catalog() {
        client
            .ok("DeleteContainer", json!({ "ContainerName": name }))
            .await;

        let (status, body) = client
            .call("DescribeContainer", json!({ "ContainerName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ContainerNotFoundException"),
            "case {case:?}: describing a deleted container must be refused, got {status} {body:?}"
        );

        let (status, body) = client
            .call("DeleteContainer", json!({ "ContainerName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ContainerNotFoundException"),
            "case {case:?}: deleting it twice must be refused, got {status} {body:?}"
        );

        let listed = client.ok("ListContainers", json!({})).await;
        let names: Vec<String> = listed["Containers"]
            .as_array()
            .expect("Containers is a required list")
            .iter()
            .map(|container| member(container, "Name").to_string())
            .collect();
        assert!(
            !names.contains(&name),
            "case {case:?}: {name:?} is still listed after deletion: {names:?}"
        );
    }
}

/// RULE: `GetContainerPolicy` answers the exact bytes the last `PutContainerPolicy`
/// stored on that container, for any document the modelled catalog admits; a container
/// with no policy stored is refused with `PolicyNotFoundException`, and a name that
/// holds no container with `ContainerNotFoundException`.
#[tokio::test]
async fn a_container_policy_round_trips_verbatim() {
    let client = Client::new();
    let container = "policy-host";
    client.create(container).await;

    let (status, body) = client
        .call("GetContainerPolicy", json!({ "ContainerName": container }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "PolicyNotFoundException"),
        "a container with no policy must be refused, got {status} {body:?}"
    );

    for (case, policy) in policy_catalog() {
        client
            .ok(
                "PutContainerPolicy",
                json!({ "ContainerName": container, "Policy": policy }),
            )
            .await;

        let got = client
            .ok("GetContainerPolicy", json!({ "ContainerName": container }))
            .await;
        assert_eq!(
            got.get("Policy").and_then(Value::as_str),
            Some(policy.as_str()),
            "case {case:?}: the stored policy must come back byte-identical, got {got:?}"
        );
    }

    // The policy belongs to its container: an unknown name holds neither.
    for operation in ["GetContainerPolicy", "PutContainerPolicy"] {
        let (status, body) = client
            .call(
                operation,
                json!({ "ContainerName": "no-such-container", "Policy": "{}" }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ContainerNotFoundException"),
            "case {operation:?}: an unknown container must be refused, got {status} {body:?}"
        );
    }

    // Deleting the container takes its policy with it.
    client
        .ok("DeleteContainer", json!({ "ContainerName": container }))
        .await;
    client.create(container).await;
    let (status, body) = client
        .call("GetContainerPolicy", json!({ "ContainerName": container }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "PolicyNotFoundException"),
        "a re-created container must hold no policy, got {status} {body:?}"
    );
}

/// RULE: paging `ListContainers` with `MaxResults` returns every stored container
/// exactly once, in name order, in pages no larger than `MaxResults`, and carries a
/// `NextToken` only while containers remain — for any (population, page size) pair.
#[tokio::test]
async fn paging_list_containers_returns_each_container_exactly_once() {
    // Rows differ along both axes the rule generalises over: how many containers are
    // stored, and the page size (absent = one unpaged response).
    let rows: &[(usize, Option<i32>)] = &[
        (1, Some(1)),
        (5, Some(1)),
        (5, Some(2)),
        (5, Some(5)),
        (7, Some(3)),
        (3, Some(100)),
        (5, None),
        (0, Some(1)),
    ];

    for &(total, max_results) in rows {
        let case = format!("{total} containers, MaxResults {max_results:?}");
        let client = Client::new();
        let mut expected: Vec<String> = (0..total).map(|i| format!("c-{i:03}")).collect();
        for name in &expected {
            client.create(name).await;
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One page per container plus the terminating page is the most any correct
        // implementation needs; more means the token is not advancing.
        for _ in 0..=total + 1 {
            let mut request = json!({});
            if let Some(limit) = max_results {
                request["MaxResults"] = json!(limit);
            }
            if let Some(ref token) = token {
                request["NextToken"] = json!(token);
            }
            let page = client.ok("ListContainers", request).await;
            let names: Vec<String> = page["Containers"]
                .as_array()
                .expect("Containers is a required list")
                .iter()
                .map(|container| member(container, "Name").to_string())
                .collect();
            if let Some(limit) = max_results {
                assert!(
                    names.len() <= usize::try_from(limit).unwrap_or_default(),
                    "case {case:?}: a page of {} exceeds MaxResults {limit}",
                    names.len()
                );
            }
            seen.extend(names);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        assert!(
            token.is_none(),
            "case {case:?}: paging did not terminate, last token {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every container exactly once in name order, got {seen:?}"
        );
    }
}

/// RULE: a container AWS reports as `CREATING` moves to `ACTIVE` once the clock owes
/// the transition; nothing else about the record changes with it.
#[tokio::test]
async fn a_creating_container_becomes_active_on_the_clock() {
    let client = Client::new();
    let name = "activating-container";
    let created = client.create(name).await;

    // The span is non-zero: a caller reading straight back still sees CREATING.
    let immediately = client
        .ok("DescribeContainer", json!({ "ContainerName": name }))
        .await["Container"]
        .clone();
    assert_eq!(
        immediately.get("Status").and_then(Value::as_str),
        Some("CREATING"),
        "a container read straight after creation is still CREATING, got {immediately:?}"
    );

    tokio::time::sleep(
        service_com_amazonaws_mediastore_mediastore_20170901::state::CONTAINER_ACTIVATION_DELAY
            + Duration::from_millis(200),
    )
    .await;

    let settled = client
        .ok("DescribeContainer", json!({ "ContainerName": name }))
        .await["Container"]
        .clone();
    assert_eq!(
        settled.get("Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "once the endpoint is available the status is ACTIVE, got {settled:?}"
    );
    for unchanged in ["ARN", "Name", "CreationTime"] {
        assert_eq!(
            settled.get(unchanged),
            created.get(unchanged),
            "the status move must not change {unchanged}: {settled:?} vs {created:?}"
        );
    }
    // And the listing reports the settled status too.
    let listed = client.ok("ListContainers", json!({})).await;
    assert_eq!(
        listed["Containers"][0].get("Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "ListContainers reports the same status, got {listed:?}"
    );
}

/// RULE: a `NextToken` is a cursor the store that answered the previous page issued —
/// one it did not issue is refused, and one it did resumes exactly after the container
/// it names, even if that container has since been deleted.
#[tokio::test]
async fn only_a_pagination_token_this_store_issued_is_accepted() {
    let client = Client::new();
    for name in ["t-0", "t-1", "t-2"] {
        client.create(name).await;
    }

    // A token this store issued: the first page of two stops at `t-0`.
    let first = client
        .ok("ListContainers", json!({ "MaxResults": 1 }))
        .await;
    let issued = first
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a first page of three containers carries a token")
        .to_string();

    // Another store's token names the same cursor but is keyed differently.
    let other = Client::new();
    for name in ["t-0", "t-1", "t-2"] {
        other.create(name).await;
    }
    let foreign = other
        .ok("ListContainers", json!({ "MaxResults": 1 }))
        .await
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("the other store issues a token too")
        .to_string();
    assert_ne!(
        foreign, issued,
        "two stores must key their cursors differently, got {issued:?} twice"
    );

    let refused: Vec<(&str, String)> = vec![
        ("a token that is not hex at all", "zzzz".to_string()),
        ("a token carrying no digest", hex_name("t-0")),
        (
            "a token with a forged digest",
            format!("{}=00000000", hex_name("t-0")),
        ),
        (
            "a token truncated to its name half",
            format!("{}=", hex_name("t-0")),
        ),
        ("another store's token", foreign),
    ];
    for (case, token) in refused {
        let (status, body) = client
            .call("ListContainers", json!({ "MaxResults": 1, "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token this store did not issue must be refused, got {status} {body:?}"
        );
    }

    // The issued token still resumes after `t-0` once `t-0` is gone: the cursor names
    // a position in the order, not a row that has to still be there.
    client
        .ok("DeleteContainer", json!({ "ContainerName": "t-0" }))
        .await;
    let resumed = client
        .ok(
            "ListContainers",
            json!({ "MaxResults": 10, "NextToken": issued }),
        )
        .await;
    let names: Vec<String> = resumed["Containers"]
        .as_array()
        .expect("Containers is a required list")
        .iter()
        .map(|container| member(container, "Name").to_string())
        .collect();
    assert_eq!(
        names,
        vec!["t-1".to_string(), "t-2".to_string()],
        "a token must resume after the name it carries, got {names:?}"
    );
}

/// RULE: a request outside a modelled constraint — or one the operation cannot act on
/// at all — is refused with HTTP 400 and a named error, never accepted.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let client = Client::new();
    client.create("constraint-host").await;

    let rows: Vec<(&str, &str, Value)> = vec![
        // ContainerName: length 1..255, pattern ^[\w-]+$ — each published bound, one
        // past it, and a character outside the class.
        (
            "an empty container name",
            "CreateContainer",
            json!({ "ContainerName": "" }),
        ),
        (
            "a container name one past the 255 maximum",
            "CreateContainer",
            json!({ "ContainerName": "n".repeat(256) }),
        ),
        (
            "a container name outside the pattern",
            "CreateContainer",
            json!({ "ContainerName": "not a name!" }),
        ),
        (
            "no container name at all",
            "CreateContainer",
            json!({}),
        ),
        // MaxResults: range 1..100 — one below and one above the published bounds.
        (
            "MaxResults below the minimum of 1",
            "ListContainers",
            json!({ "MaxResults": 0 }),
        ),
        (
            "MaxResults one past the maximum of 100",
            "ListContainers",
            json!({ "MaxResults": 101 }),
        ),
        (
            "a NextToken that is not hex at all",
            "ListContainers",
            json!({ "NextToken": "zz" }),
        ),
        (
            "a hex NextToken carrying no keyed digest",
            "ListContainers",
            json!({ "NextToken": hex_name("constraint-host") }),
        ),
        (
            "a hex NextToken with a forged digest",
            "ListContainers",
            json!({ "NextToken": format!("{}=00000000", hex_name("constraint-host")) }),
        ),
        (
            "a NextToken naming a container that never existed, under a forged digest",
            "ListContainers",
            json!({ "NextToken": format!("{}=0123abcd", hex_name("never-created")) }),
        ),
        // Policy: length 1..8192, charset ^[\x00-\x7F]+$.
        (
            "an empty policy",
            "PutContainerPolicy",
            json!({ "ContainerName": "constraint-host", "Policy": "" }),
        ),
        (
            "a policy one past the 8192 maximum",
            "PutContainerPolicy",
            json!({ "ContainerName": "constraint-host", "Policy": "p".repeat(8193) }),
        ),
        (
            "a policy outside the ASCII charset",
            "PutContainerPolicy",
            json!({ "ContainerName": "constraint-host", "Policy": "€" }),
        ),
        (
            "no policy at all",
            "PutContainerPolicy",
            json!({ "ContainerName": "constraint-host" }),
        ),
        // DescribeContainer's ContainerName is optional in the model, but there is
        // nothing to describe without one.
        ("no container to describe", "DescribeContainer", json!({})),
    ];

    for (case, operation, body) in rows {
        let (status, answered) = client.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} must refuse the request, got {status} {answered:?}"
        );
        assert!(
            !error_code(&answered).is_empty(),
            "case {case:?}: the refusal must name an error code, got {answered:?}"
        );
    }

    // The refusals changed nothing: the only container is still the one created above.
    let listed = client.ok("ListContainers", json!({})).await;
    let names: Vec<String> = listed["Containers"]
        .as_array()
        .expect("Containers is a required list")
        .iter()
        .map(|container| member(container, "Name").to_string())
        .collect();
    assert_eq!(
        names,
        vec!["constraint-host".to_string()],
        "a refused request must not reach state, got {names:?}"
    );
}

/// RULE: the ARN a container reports carries the region the request was signed for.
#[tokio::test]
async fn the_container_arn_carries_the_signed_region() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let client = Client::in_region(region);
        let created = client.create("regional-container").await;
        let arn = member(&created, "ARN");
        assert!(
            arn.starts_with(&format!("arn:aws:mediastore:{region}:")),
            "case {region:?}: expected the signed region in {arn:?}"
        );
        let described = client
            .ok(
                "DescribeContainer",
                json!({ "ContainerName": "regional-container" }),
            )
            .await["Container"]
            .clone();
        assert!(
            member(&described, "Endpoint").contains(&format!(".data.mediastore.{region}.")),
            "case {region:?}: expected the signed region in the endpoint, got {described:?}"
        );
    }
}

/// RULE: a container beyond the account's documented container quota is refused with
/// `LimitExceededException`, and the store stays at the quota.
#[tokio::test]
async fn creating_past_the_container_quota_is_refused() {
    // AWS Elemental MediaStore's documented quota: 100 containers per account per
    // Region (user-guide "Service quotas").
    const QUOTA: usize = 100;

    let client = Client::new();
    for i in 0..QUOTA {
        client.create(&format!("quota-{i:03}")).await;
    }

    let (status, body) = client
        .call("CreateContainer", json!({ "ContainerName": "one-too-many" }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "LimitExceededException"),
        "the container past the quota must be refused, got {status} {body:?}"
    );

    let listed = client.ok("ListContainers", json!({})).await;
    assert_eq!(
        listed["Containers"].as_array().map(Vec::len),
        Some(QUOTA),
        "the refused container must not be stored, got {listed:?}"
    );

    // Deleting one makes room again: the quota is read from state, not a counter.
    client
        .ok("DeleteContainer", json!({ "ContainerName": "quota-000" }))
        .await;
    client.create("one-too-many").await;
}
