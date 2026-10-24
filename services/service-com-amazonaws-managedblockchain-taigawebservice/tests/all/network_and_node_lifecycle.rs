// SPDX-License-Identifier: CC0-1.0

//! The Managed Blockchain network/member/node lifecycle, end to end over HTTP:
//! `CreateNetwork`, `GetNetwork`, `ListNetworks`, `GetMember`, `CreateNode`, `GetNode`,
//! `ListNodes`, `DeleteNode`.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures.
//!
//! The tables below span the classes the rules generalise over: the framework versions
//! and editions a Fabric network is created with, tag counts, the `StateDB` values a node
//! can take (and its omission), page sizes from one to past the item count plus both
//! published `MaxResults` bounds and one past each, and every id-shaped lookup that names
//! nothing stored.
//!
//! The harness at the top (the router, the signed call and the answer it returns) is
//! `pub(crate)` so the other suites in this target reuse it rather than growing a second
//! copy of it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::TRANSITION_SPAN;
use service_com_amazonaws_managedblockchain_taigawebservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the service reads it off the credential scope, so it
/// must appear in every ARN and endpoint it mints.
pub(crate) const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// Comfortably longer than [`TRANSITION_SPAN`], so a transition the clock owes has come
/// due by the time the next call is made.
pub(crate) const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// One answer from the emulator.
///
/// `pub(crate)`, like the four helpers below, so a sibling suite in this test crate
/// drives the emulator through the one harness rather than a second copy of it.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Answer {
    /// One member of the body, as a string (empty when absent).
    pub(crate) fn text(&self, path: &[&str]) -> String {
        let mut value = &self.body;
        for key in path {
            value = &value[*key];
        }
        value.as_str().unwrap_or_default().to_string()
    }
}

/// A router with only this service mounted, over `engine` — so a caller that needs to
/// reach the same store the handlers write to can keep a handle on it.
pub(crate) fn router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A router with only this service mounted, over a fresh engine.
pub(crate) fn new_router() -> MultiServiceRouter {
    router_over(Arc::new(engine::InMemoryEngine::new()))
}

/// Send one request SIGNED FOR `region` and return what the emulator answered.
///
/// The credential scope is the only place a request says which Region it addresses, so a
/// suite that drives two Regions goes through here; [`call`] is this with [`REGION`].
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://managedblockchain.{region}.amazonaws.com{path}"
        ))
        .header("content-type", "application/json")
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/managedblockchain/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Send one request signed for [`REGION`] and return what the emulator answered.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    call_in_region(router, REGION, method, path, body).await
}

/// A `MemberConfiguration` naming `name`, with the admin user every Fabric member needs.
pub(crate) fn member_configuration(name: &str) -> Value {
    json!({
        "Name": name,
        "FrameworkConfiguration": {
            "Fabric": { "AdminUsername": "AdminUser", "AdminPassword": "MyPassword1" }
        }
    })
}

/// A minimal valid `CreateNetwork` request body.
pub(crate) fn create_network_body(token: &str, name: &str) -> Value {
    json!({
        "ClientRequestToken": token,
        "Name": name,
        "Framework": "HYPERLEDGER_FABRIC",
        "FrameworkVersion": "2.2",
        "FrameworkConfiguration": { "Fabric": { "Edition": "STANDARD" } },
        "VotingPolicy": {
            "ApprovalThresholdPolicy": {
                "ThresholdPercentage": 50,
                "ProposalDurationInHours": 24,
                "ThresholdComparator": "GREATER_THAN"
            }
        },
        "MemberConfiguration": member_configuration("org1")
    })
}

/// Create one network and return `(NetworkId, MemberId)`.
pub(crate) async fn create_network(router: &MultiServiceRouter, body: Value) -> (String, String) {
    let answer = call(router, "POST", "/networks", Some(body.clone())).await;
    assert_eq!(
        answer.status, 200,
        "case {body:?}: expected CreateNetwork 200, got {} {:?}",
        answer.status, answer.body
    );
    (answer.text(&["NetworkId"]), answer.text(&["MemberId"]))
}

/// Create one network that has already settled to `AVAILABLE`, so it can take nodes and
/// members.
pub(crate) async fn available_network(
    router: &MultiServiceRouter,
    token: &str,
    name: &str,
) -> (String, String) {
    let ids = create_network(router, create_network_body(token, name)).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    ids
}

/// Create one node on `network_id`/`member_id` and return its `NodeId`.
pub(crate) async fn create_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
    configuration: Value,
) -> String {
    let body = json!({
        "ClientRequestToken": token,
        "MemberId": member_id,
        "NodeConfiguration": configuration,
    });
    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {body:?}: expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["NodeId"])
}

/// Whether `value` is an ISO-8601 date-time string, the format
/// `com.amazonaws.managedblockchain#Timestamp` carries (`@timestampFormat("date-time")`).
pub(crate) fn is_date_time(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
            .is_ok()
    })
}

/// Whether `id` has the shape every Managed Blockchain resource id takes: a prefix, a
/// hyphen, then 26 characters of the base-32 alphabet, within `ResourceIdString`'s
/// `@length(1, 32)`.
pub(crate) fn is_resource_id(id: &str, prefix: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{prefix}-")) else {
        return false;
    };
    id.len() <= 32
        && suffix.chars().count() == 26
        && suffix
            .chars()
            .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
}

/// One row of the network-create table: what the request configures, so the read back
/// can be asserted against it.
#[derive(Debug)]
struct NetworkCase {
    name: &'static str,
    version: &'static str,
    edition: Option<&'static str>,
    description: Option<&'static str>,
    tag_count: usize,
    threshold: i32,
    comparator: &'static str,
}

/// `CreateNetwork` persists what it was given, and `GetNetwork` / `GetMember` answer from
/// those records — over a table spanning the versions, editions, descriptions, tag counts
/// and voting policies a Fabric network is created with.
#[tokio::test]
async fn create_persists_the_network_and_its_first_member() {
    let router = new_router();

    // Rows differ along every member the response is projected from. The editions are
    // `Edition`'s own values (smithy_query: shape Edition = STANDARD | STARTER); the
    // `None` row omits `FrameworkConfiguration` altogether, and the versions are the
    // Fabric versions `FrameworkVersionString` (@length 1..8) can hold.
    let cases: &[NetworkCase] = &[
        NetworkCase {
            name: "net-a",
            version: "1.2",
            edition: Some("STARTER"),
            description: None,
            tag_count: 0,
            threshold: 0,
            comparator: "GREATER_THAN",
        },
        NetworkCase {
            name: "net-b",
            version: "1.4",
            edition: Some("STANDARD"),
            description: Some("a described network"),
            tag_count: 1,
            threshold: 50,
            comparator: "GREATER_THAN",
        },
        NetworkCase {
            name: "net-c",
            version: "2.2",
            edition: None,
            description: Some(""),
            tag_count: 3,
            threshold: 100,
            comparator: "GREATER_THAN_OR_EQUAL_TO",
        },
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for case in cases {
        let &NetworkCase {
            name,
            version,
            edition,
            description,
            tag_count,
            threshold,
            comparator,
        } = case;
        let tags: serde_json::Map<String, Value> = (0..tag_count)
            .map(|i| (format!("key{i}"), Value::String(format!("value{i}"))))
            .collect();
        let mut body = json!({
            "ClientRequestToken": format!("token-{name}"),
            "Name": name,
            "Framework": "HYPERLEDGER_FABRIC",
            "FrameworkVersion": version,
            "VotingPolicy": {
                "ApprovalThresholdPolicy": {
                    "ThresholdPercentage": threshold,
                    "ProposalDurationInHours": 24,
                    "ThresholdComparator": comparator
                }
            },
            "MemberConfiguration": member_configuration("org-one"),
            "Tags": Value::Object(tags.clone()),
        });
        if let Some(edition) = edition {
            body["FrameworkConfiguration"] = json!({ "Fabric": { "Edition": edition } });
        }
        if let Some(description) = description {
            body["Description"] = Value::String(description.to_string());
        }

        let (network_id, member_id) = create_network(&router, body.clone()).await;
        assert!(
            is_resource_id(&network_id, "n"),
            "case {name}: expected a minted network id, got {network_id:?}"
        );
        assert!(
            is_resource_id(&member_id, "m"),
            "case {name}: expected a minted member id, got {member_id:?}"
        );
        assert!(
            !seen_ids.contains(&network_id) && !seen_ids.contains(&member_id),
            "case {name}: every create must mint fresh ids, got a repeat in \
             {network_id:?}/{member_id:?}"
        );
        seen_ids.push(network_id.clone());
        seen_ids.push(member_id.clone());

        let answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected GetNetwork 200, got {} {:?}",
            answer.status, answer.body
        );
        let network = &answer.body["Network"];

        for (member, expected) in [
            ("Id", network_id.clone()),
            ("Name", name.to_string()),
            ("Framework", "HYPERLEDGER_FABRIC".to_string()),
            ("FrameworkVersion", version.to_string()),
            (
                "Arn",
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:networks/{network_id}"),
            ),
            (
                "VpcEndpointServiceName",
                format!("com.amazonaws.{REGION}.managedblockchain.{network_id}"),
            ),
            // The create has only just returned, so the network is still in the
            // non-terminal status the service reports it in.
            ("Status", "CREATING".to_string()),
        ] {
            assert_eq!(
                network[member].as_str(),
                Some(expected.as_str()),
                "case {name}: expected Network.{member} {expected:?}, got {network:?}"
            );
        }
        assert_eq!(
            network["Description"].as_str(),
            description,
            "case {name}: expected Network.Description {description:?}, got {network:?}"
        );
        assert!(
            is_date_time(&network["CreationDate"]),
            "case {name}: expected an ISO-8601 Network.CreationDate, got {network:?}"
        );
        assert_eq!(
            network["FrameworkAttributes"]["Fabric"]["OrderingServiceEndpoint"].as_str(),
            Some(
                format!("orderer.{network_id}.managedblockchain.{REGION}.amazonaws.com:30001")
                    .as_str()
            ),
            "case {name}: expected the network's ordering-service endpoint, got {network:?}"
        );
        assert_eq!(
            network["FrameworkAttributes"]["Fabric"]["Edition"].as_str(),
            edition,
            "case {name}: expected Edition {edition:?}, got {network:?}"
        );
        assert_eq!(
            network["VotingPolicy"]["ApprovalThresholdPolicy"]["ThresholdPercentage"].as_i64(),
            Some(i64::from(threshold)),
            "case {name}: expected the voting policy back, got {network:?}"
        );
        assert_eq!(
            network["VotingPolicy"]["ApprovalThresholdPolicy"]["ThresholdComparator"].as_str(),
            Some(comparator),
            "case {name}: expected the voting policy back, got {network:?}"
        );
        assert_eq!(
            network["Tags"].as_object().map(serde_json::Map::len),
            Some(tag_count),
            "case {name}: expected {tag_count} tags back, got {network:?}"
        );

        // The member `CreateNetwork` minted reads back as a resource of its own.
        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members/{member_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected GetMember 200, got {} {:?}",
            answer.status, answer.body
        );
        let member = &answer.body["Member"];
        for (field, expected) in [
            ("Id", member_id.clone()),
            ("NetworkId", network_id.clone()),
            ("Name", "org-one".to_string()),
            (
                "Arn",
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:members/{member_id}"),
            ),
            // `MemberConfiguration$KmsKeyArn` documents an undefined value as "use an
            // KMS key that is owned and managed by Amazon Web Services on your behalf",
            // which `Member$KmsKeyArn` reports as this sentinel.
            ("KmsKeyArn", "AWS Owned KMS Key".to_string()),
        ] {
            assert_eq!(
                member[field].as_str(),
                Some(expected.as_str()),
                "case {name}: expected Member.{field} {expected:?}, got {member:?}"
            );
        }
        assert_eq!(
            member["FrameworkAttributes"]["Fabric"]["AdminUsername"].as_str(),
            Some("AdminUser"),
            "case {name}: expected the admin user back, got {member:?}"
        );
        assert!(
            is_date_time(&member["CreationDate"]),
            "case {name}: expected an ISO-8601 Member.CreationDate, got {member:?}"
        );
        assert_eq!(
            member["FrameworkAttributes"]["Fabric"]["CaEndpoint"].as_str(),
            Some(
                format!(
                    "ca.{member_id}.{network_id}.managedblockchain.{REGION}.amazonaws.com:30002"
                )
                .as_str()
            ),
            "case {name}: expected the member's CA endpoint, got {member:?}"
        );
    }
}

/// A customer managed key named on the member configuration is stored and reported
/// instead of the Amazon-owned sentinel.
#[tokio::test]
async fn a_named_kms_key_is_stored_on_the_member() {
    let router = new_router();
    let key_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab");

    // Rows: the documented "undefined or empty string" default and a valid symmetric
    // customer managed key (source: `MemberConfiguration$KmsKeyArn`'s own doc), plus the
    // empty string itself — which `ArnString`'s modelled @length(1, 1011) and
    // `^arn:.+:.+:.+:.+:.+$` pattern refuse at the boundary, so only omitting the member
    // reaches the documented default.
    let cases: &[(&str, Option<&str>, u16, &str)] = &[
        ("omitted", None, 200, "AWS Owned KMS Key"),
        ("empty", Some(""), 400, ""),
        (
            "customer-managed",
            Some(key_arn.as_str()),
            200,
            key_arn.as_str(),
        ),
    ];

    for (case, configured, expected_status, expected) in cases {
        let mut configuration = member_configuration("org-kms");
        if let Some(configured) = configured {
            configuration["KmsKeyArn"] = Value::String((*configured).to_string());
        }
        let mut body = create_network_body(&format!("kms-{case}"), "kms-net");
        body["MemberConfiguration"] = configuration;

        let answer = call(&router, "POST", "/networks", Some(body)).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case}: expected CreateNetwork HTTP {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
        if *expected_status != 200 {
            continue;
        }
        let network_id = answer.text(&["NetworkId"]);
        let member_id = answer.text(&["MemberId"]);
        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members/{member_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.text(&["Member", "KmsKeyArn"]),
            *expected,
            "case {case}: expected Member.KmsKeyArn {expected:?}, got {:?}",
            answer.body
        );
    }
}

/// `CreateNetwork` applies only to Hyperledger Fabric, so every other framework value is
/// refused rather than stored.
#[tokio::test]
async fn only_a_hyperledger_fabric_network_can_be_created() {
    let router = new_router();

    // Rows: every value `Framework` declares (smithy_query: shape Framework =
    // ETHEREUM | HYPERLEDGER_FABRIC), plus one the enum does not declare.
    let cases: &[(&str, u16)] = &[
        ("HYPERLEDGER_FABRIC", 200),
        ("ETHEREUM", 400),
        ("BITCOIN", 400),
    ];

    for (framework, expected) in cases {
        let mut body = create_network_body(&format!("fw-{framework}"), "fw-net");
        body["Framework"] = Value::String((*framework).to_string());
        let answer = call(&router, "POST", "/networks", Some(body)).await;
        assert_eq!(
            answer.status, *expected,
            "case {framework}: expected HTTP {expected}, got {} {:?}",
            answer.status, answer.body
        );
        if *expected != 200 {
            assert!(
                !answer.text(&["message"]).is_empty() || !answer.text(&["Message"]).is_empty(),
                "case {framework}: a refusal must say what was wrong, got {:?}",
                answer.body
            );
        }
    }

    // Nothing was stored for the refused frameworks.
    let answer = call(&router, "GET", "/networks", None).await;
    assert_eq!(
        answer.body["Networks"].as_array().map(Vec::len),
        Some(1),
        "expected only the Fabric network to have been stored, got {:?}",
        answer.body
    );
}

/// A repeated `ClientRequestToken` replays the ids its first call minted, and a fresh
/// token mints new ones — over a table of tokens.
#[tokio::test]
async fn a_repeated_client_request_token_replays_the_same_ids() {
    let router = new_router();

    // Rows differ along the token and along whether the retry carries the same request
    // body: an idempotent operation "completes no more than once" either way.
    let cases: &[(&str, &str)] = &[
        ("tok-1", "same body"),
        ("tok-2", "same body"),
        ("tok-3", "different name"),
    ];

    let mut first_ids: Vec<(String, String)> = Vec::new();
    for (token, retry) in cases {
        let (network_id, member_id) =
            create_network(&router, create_network_body(token, "idem-net")).await;
        let retry_body = if *retry == "different name" {
            create_network_body(token, "renamed-net")
        } else {
            create_network_body(token, "idem-net")
        };
        let (replayed_network, replayed_member) = create_network(&router, retry_body).await;
        assert_eq!(
            (replayed_network.as_str(), replayed_member.as_str()),
            (network_id.as_str(), member_id.as_str()),
            "case {token}/{retry}: a repeated token must replay the original ids, got \
             {replayed_network:?}/{replayed_member:?}"
        );
        assert!(
            !first_ids
                .iter()
                .any(|(seen, _)| seen.as_str() == network_id.as_str()),
            "case {token}/{retry}: a fresh token must mint a new network, got a repeat of \
             {network_id:?}"
        );
        first_ids.push((network_id, member_id));
    }

    // One network per token, not two: the replays created nothing.
    let answer = call(&router, "GET", "/networks", None).await;
    assert_eq!(
        answer.body["Networks"].as_array().map(Vec::len),
        Some(cases.len()),
        "expected one network per token, got {:?}",
        answer.body
    );
}

/// A created resource is reported `CREATING` and settles to `AVAILABLE` once the clock
/// owes the transition; until then the calls that need a settled resource are refused
/// with the modelled `ResourceNotReadyException`.
#[tokio::test]
async fn creating_settles_to_available_and_gates_the_calls_that_need_it() {
    let router = new_router();
    let started = std::time::Instant::now();
    let (network_id, member_id) =
        create_network(&router, create_network_body("settle-1", "settling-net")).await;

    let answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
    assert_eq!(
        answer.text(&["Network", "Status"]),
        "CREATING",
        "case straight after create: expected CREATING, got {:?}",
        answer.body
    );

    // A node cannot be created on a network that is not ready yet.
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(json!({
            "ClientRequestToken": "settle-node-early",
            "MemberId": member_id,
            "NodeConfiguration": { "InstanceType": "bc.t3.small" }
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (409, "ResourceNotReadyException"),
        "case CreateNode while CREATING: expected 409 ResourceNotReadyException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert!(
        started.elapsed() < TRANSITION_SPAN,
        "case CreateNode while CREATING: the transient status must outlive the create call \
         ({TRANSITION_SPAN:?}), but {:?} had already passed",
        started.elapsed()
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    for (resource, path) in [
        ("network", format!("/networks/{network_id}")),
        (
            "member",
            format!("/networks/{network_id}/members/{member_id}"),
        ),
    ] {
        let answer = call(&router, "GET", &path, None).await;
        let status = if resource == "network" {
            answer.text(&["Network", "Status"])
        } else {
            answer.text(&["Member", "Status"])
        };
        assert_eq!(
            status, "AVAILABLE",
            "case {resource} after the transition span: expected AVAILABLE, got {:?}",
            answer.body
        );
    }

    // The same node create now succeeds, and the node itself starts out CREATING.
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "settle-node",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.text(&["Node", "Status"]),
        "CREATING",
        "case straight after CreateNode: expected CREATING, got {:?}",
        answer.body
    );

    // And a node that is not ready cannot be deleted yet.
    let answer = call(
        &router,
        "DELETE",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (409, "ResourceNotReadyException"),
        "case DeleteNode while CREATING: expected 409 ResourceNotReadyException, got {} {:?}",
        answer.status,
        answer.body
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.text(&["Node", "Status"]),
        "AVAILABLE",
        "case node after the transition span: expected AVAILABLE, got {:?}",
        answer.body
    );
}

/// One row of the node-create table: the configuration the request carries, and the
/// `StateDB` the stored node must end up with.
#[derive(Debug)]
struct NodeCase {
    instance_type: &'static str,
    zone: Option<&'static str>,
    state_db: Option<&'static str>,
    expected_state_db: &'static str,
    tag_count: usize,
}

/// `CreateNode` persists its `NodeConfiguration`, `GetNode` answers from that record, and
/// an omitted `StateDB` takes its documented default — over a table of configurations.
#[tokio::test]
async fn create_node_persists_its_configuration() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "nodes-1", "node-net").await;

    // Rows differ along the instance type (one size from each `bc.*` family the catalog
    // in `state::INSTANCE_TYPES` publishes), the presence of an Availability Zone, the
    // `StateDB` value (smithy_query: shape StateDBType = CouchDB | LevelDB) and its
    // omission, and the tag count.
    let cases: &[NodeCase] = &[
        NodeCase {
            instance_type: "bc.t3.small",
            zone: Some("eu-west-1a"),
            state_db: Some("CouchDB"),
            expected_state_db: "CouchDB",
            tag_count: 0,
        },
        NodeCase {
            instance_type: "bc.m5.large",
            zone: Some("eu-west-1b"),
            state_db: Some("LevelDB"),
            expected_state_db: "LevelDB",
            tag_count: 2,
        },
        // `NodeConfiguration$StateDB`: "the default is CouchDB".
        NodeCase {
            instance_type: "bc.m5.4xlarge",
            zone: None,
            state_db: None,
            expected_state_db: "CouchDB",
            tag_count: 1,
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for case in cases {
        let &NodeCase {
            instance_type,
            zone,
            state_db,
            expected_state_db,
            tag_count,
        } = case;
        let mut configuration = json!({ "InstanceType": instance_type });
        if let Some(zone) = zone {
            configuration["AvailabilityZone"] = Value::String(zone.to_string());
        }
        if let Some(state_db) = state_db {
            configuration["StateDB"] = Value::String(state_db.to_string());
        }
        let tags: serde_json::Map<String, Value> = (0..tag_count)
            .map(|i| (format!("tag{i}"), Value::String(format!("v{i}"))))
            .collect();
        let body = json!({
            "ClientRequestToken": format!("node-{instance_type}"),
            "MemberId": member_id,
            "NodeConfiguration": configuration,
            "Tags": Value::Object(tags),
        });
        let answer = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/nodes"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {instance_type}: expected CreateNode 200, got {} {:?}",
            answer.status, answer.body
        );
        let node_id = answer.text(&["NodeId"]);
        assert!(
            is_resource_id(&node_id, "nd"),
            "case {instance_type}: expected a minted node id, got {node_id:?}"
        );
        assert!(
            !created.contains(&node_id),
            "case {instance_type}: every create must mint a fresh node id, got a repeat of \
             {node_id:?}"
        );
        created.push(node_id.clone());

        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {instance_type}: expected GetNode 200, got {} {:?}",
            answer.status, answer.body
        );
        let node = &answer.body["Node"];
        for (field, expected) in [
            ("Id", node_id.clone()),
            ("NetworkId", network_id.clone()),
            ("MemberId", member_id.clone()),
            ("InstanceType", instance_type.to_string()),
            ("StateDB", expected_state_db.to_string()),
            (
                "Arn",
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:nodes/{node_id}"),
            ),
            // Inherited from the member: "The node inherits this parameter from the
            // member that it belongs to" (`Node$KmsKeyArn`).
            ("KmsKeyArn", "AWS Owned KMS Key".to_string()),
        ] {
            assert_eq!(
                node[field].as_str(),
                Some(expected.as_str()),
                "case {instance_type}: expected Node.{field} {expected:?}, got {node:?}"
            );
        }
        assert_eq!(
            node["AvailabilityZone"].as_str(),
            zone,
            "case {instance_type}: expected Node.AvailabilityZone {zone:?}, got {node:?}"
        );
        assert_eq!(
            node["Tags"].as_object().map(serde_json::Map::len),
            Some(tag_count),
            "case {instance_type}: expected {tag_count} tags back, got {node:?}"
        );
        assert_eq!(
            node["FrameworkAttributes"]["Fabric"]["PeerEndpoint"].as_str(),
            Some(
                format!(
                    "{node_id}.{member_id}.{network_id}.managedblockchain.{REGION}\
                     .amazonaws.com:30003"
                )
                .as_str()
            ),
            "case {instance_type}: expected the node's peer endpoint, got {node:?}"
        );
        assert!(
            is_date_time(&node["CreationDate"]),
            "case {instance_type}: expected an ISO-8601 Node.CreationDate, got {node:?}"
        );
    }

    // Every created node is listed, in one page, with the summary members of each.
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/nodes?memberId={member_id}"),
        None,
    )
    .await;
    let summaries = answer.body["Nodes"].as_array().cloned().unwrap_or_default();
    let listed: Vec<String> = summaries
        .iter()
        .map(|node| node["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case ListNodes: expected every created node, got {:?}",
        answer.body
    );
    for summary in &summaries {
        let id = summary["Id"].as_str().unwrap_or_default();
        assert!(
            is_date_time(&summary["CreationDate"]),
            "case NodeSummary {id}: expected an ISO-8601 CreationDate, got {summary:?}"
        );
        assert_eq!(
            summary["Arn"].as_str(),
            Some(format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:nodes/{id}").as_str()),
            "case NodeSummary {id}: expected the node's ARN, got {summary:?}"
        );
        assert!(
            summary["InstanceType"].as_str().is_some() && summary["Status"].as_str().is_some(),
            "case NodeSummary {id}: expected the stored instance type and status, got {summary:?}"
        );
    }
}

/// `DeleteNode` removes the node from every read: `GetNode` answers 404, `ListNodes` no
/// longer reports it, and deleting an id no node holds is a 404 too.
#[tokio::test]
async fn delete_node_removes_it_from_every_read() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "delete-1", "delete-net").await;

    let mut node_ids: Vec<String> = Vec::new();
    for i in 0..3 {
        node_ids.push(
            create_node(
                &router,
                &network_id,
                &member_id,
                &format!("delete-node-{i}"),
                json!({ "InstanceType": "bc.t3.small" }),
            )
            .await,
        );
    }
    node_ids.sort();
    tokio::time::sleep(PAST_TRANSITION).await;

    // Rows: the first, the middle and the last node of the stored ordering, so the
    // deletion is exercised at every position a page cursor can sit on.
    for (case, index) in [("middle", 1_usize), ("first", 0), ("last", 0)] {
        let doomed = node_ids.remove(index);
        let answer = call(
            &router,
            "DELETE",
            &format!("/networks/{network_id}/nodes/{doomed}?memberId={member_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected DeleteNode 200, got {} {:?}",
            answer.status, answer.body
        );

        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/nodes/{doomed}?memberId={member_id}"),
            None,
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected GetNode 404 ResourceNotFoundException after the delete, \
             got {} {:?}",
            answer.status,
            answer.body
        );

        // Deleting it a second time finds nothing to delete.
        let answer = call(
            &router,
            "DELETE",
            &format!("/networks/{network_id}/nodes/{doomed}?memberId={member_id}"),
            None,
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected a repeated DeleteNode to be a 404, got {} {:?}",
            answer.status,
            answer.body
        );

        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/nodes?memberId={member_id}"),
            None,
        )
        .await;
        let listed: Vec<String> = answer.body["Nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|node| node["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed, node_ids,
            "case {case}: expected ListNodes to report only the surviving nodes, got {:?}",
            answer.body
        );
    }
}

/// The idempotency ledger does not outlive the resource it describes: once a node is
/// deleted, its create token mints a new node instead of replaying an id nothing holds.
#[tokio::test]
async fn a_deleted_node_releases_its_idempotency_record() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "ledger-1", "ledger-net").await;

    let first = create_node(
        &router,
        &network_id,
        &member_id,
        "reused-token",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let answer = call(
        &router,
        "DELETE",
        &format!("/networks/{network_id}/nodes/{first}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case delete before reuse: expected DeleteNode 200, got {} {:?}",
        answer.status, answer.body
    );

    let second = create_node(
        &router,
        &network_id,
        &member_id,
        "reused-token",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    assert_ne!(
        second, first,
        "case reuse after delete: expected a freshly minted node id, got the deleted \
         {first:?}"
    );
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/nodes/{second}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case reuse after delete: expected the new node to read back, got {} {:?}",
        answer.status, answer.body
    );
}

/// Every read of a resource id that names nothing stored answers the modelled
/// `ResourceNotFoundException` with HTTP 404 — over a table of operations and ids.
#[tokio::test]
async fn an_id_that_names_nothing_stored_is_a_404() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "missing-1", "present-net").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "missing-node",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Rows differ along which segment of the path is unknown, and along the shape of the
    // unknown id: one shaped as the service mints them, one the issue's own example, and
    // a one-character id (`ResourceIdString` is @length(1, 32), so all three are legal
    // inputs the handler must answer for).
    let unknown_network = "n-DOESNOTEXISTDOESNOTEXISTAA";
    let cases: Vec<(String, String)> = vec![
        (
            "GetNetwork/unknown".to_string(),
            format!("/networks/{unknown_network}"),
        ),
        (
            "GetNetwork/short".to_string(),
            "/networks/x".to_string(),
        ),
        (
            "GetNetwork/issue-example".to_string(),
            "/networks/n-doesnotexist00000000000000000".to_string(),
        ),
        (
            "GetMember/unknown-network".to_string(),
            format!("/networks/{unknown_network}/members/{member_id}"),
        ),
        (
            "GetMember/unknown-member".to_string(),
            format!("/networks/{network_id}/members/m-NOSUCHMEMBERNOSUCHMEMBERAA"),
        ),
        (
            "GetNode/unknown-network".to_string(),
            format!("/networks/{unknown_network}/nodes/{node_id}?memberId={member_id}"),
        ),
        (
            "GetNode/unknown-node".to_string(),
            format!("/networks/{network_id}/nodes/nd-NOSUCHNODENOSUCHNODEAAAA?memberId={member_id}"),
        ),
        (
            "GetNode/other-member".to_string(),
            format!("/networks/{network_id}/nodes/{node_id}?memberId=m-NOSUCHMEMBERNOSUCHMEMBERAA"),
        ),
    ];

    for (case, path) in cases {
        let answer = call(&router, "GET", &path, None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // The same for the mutating path: a node id nothing holds cannot be deleted.
    let answer = call(
        &router,
        "DELETE",
        &format!("/networks/{network_id}/nodes/nd-NOSUCHNODENOSUCHNODEAAAA?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case DeleteNode/unknown-node: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );

    // A node create on a network nothing holds is a 404 as well.
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{unknown_network}/nodes"),
        Some(json!({
            "ClientRequestToken": "missing-network-node",
            "MemberId": member_id,
            "NodeConfiguration": { "InstanceType": "bc.t3.small" }
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case CreateNode/unknown-network: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );
}

/// On a Hyperledger Fabric network the owning member scopes every node call, so one that
/// names no member is refused — over the three operations that take `memberId`.
#[tokio::test]
async fn the_owning_member_is_required_for_node_calls_on_fabric() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "scope-1", "scope-net").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "scope-node",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Rows: each operation that takes the optional `memberId` query, called without it.
    // Source: `GetNodeInput$MemberId`, `ListNodesInput$MemberId` and
    // `DeleteNodeInput$MemberId` — "Applies only to Hyperledger Fabric and is required
    // for Hyperledger Fabric".
    let cases: &[(&str, &str, String)] = &[
        (
            "GetNode",
            "GET",
            format!("/networks/{network_id}/nodes/{node_id}"),
        ),
        ("ListNodes", "GET", format!("/networks/{network_id}/nodes")),
        (
            "DeleteNode",
            "DELETE",
            format!("/networks/{network_id}/nodes/{node_id}"),
        ),
    ];

    for (case, method, path) in cases {
        let answer = call(&router, method, path, None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case {case} without memberId: expected 400 InvalidRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // And the node survived every refusal.
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case scoped GetNode: expected 200 after the refusals, got {} {:?}",
        answer.status, answer.body
    );
}

/// `ListNetworks` reports every persisted network, pages by `MaxResults` through
/// `NextToken`, and filters on `name`, `framework` and `status`.
#[tokio::test]
async fn list_networks_pages_and_filters_over_persisted_networks() {
    let router = new_router();

    // Seven networks, so a page size at the published maximum (10) exceeds the item
    // count while the smaller sizes do not.
    let mut expected_ids: Vec<String> = Vec::new();
    let mut named_ids: Vec<String> = Vec::new();
    for i in 0..7 {
        let name = if i % 2 == 0 { "evens" } else { "odds" };
        let (network_id, _) =
            create_network(&router, create_network_body(&format!("list-{i}"), name)).await;
        if name == "evens" {
            named_ids.push(network_id.clone());
        }
        expected_ids.push(network_id);
    }
    // The listing is ordered by the stored id, which is what the page cursor keys on.
    expected_ids.sort();
    named_ids.sort();

    // Page sizes from one to past the item count, including both published `MaxResults`
    // bounds (@range(1, 10)).
    for page_size in [1_usize, 2, 3, 7, 10] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("/networks?maxResults={page_size}&nextToken={token}"),
                None => format!("/networks?maxResults={page_size}"),
            };
            let answer = call(&router, "GET", &query, None).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected ListNetworks 200, got {} {:?}",
                answer.status, answer.body
            );
            let items = answer.body["Networks"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page holds at most MaxResults items, got {}",
                items.len()
            );
            for summary in &items {
                let id = summary["Id"].as_str().unwrap_or_default();
                assert!(
                    is_date_time(&summary["CreationDate"]),
                    "case NetworkSummary {id}: expected an ISO-8601 CreationDate, got {summary:?}"
                );
                assert_eq!(
                    summary["Arn"].as_str(),
                    Some(
                        format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:networks/{id}")
                            .as_str()
                    ),
                    "case NetworkSummary {id}: expected the network's ARN, got {summary:?}"
                );
            }
            collected.extend(
                items
                    .iter()
                    .map(|network| network["Id"].as_str().unwrap_or_default().to_string()),
            );
            token = answer.body["NextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected_ids,
            "case page size {page_size}: paging must walk every network exactly once, got \
             {collected:?}"
        );
    }

    // One page past each published bound is refused by the modelled range.
    for (case, page_size) in [("below", 0), ("above", 11)] {
        let answer = call(
            &router,
            "GET",
            &format!("/networks?maxResults={page_size}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {case} the range: expected 400, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A token this service never issued cannot resume a page.
    let answer = call(&router, "GET", "/networks?nextToken=not-a-token", None).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case a forged nextToken: expected 400 InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );

    // Each filter selects on the stored value of the member it names.
    let cases: Vec<(&str, String, usize)> = vec![
        ("name matches", "name=evens".to_string(), named_ids.len()),
        ("name matches nothing", "name=nope".to_string(), 0),
        (
            "framework matches",
            "framework=HYPERLEDGER_FABRIC".to_string(),
            expected_ids.len(),
        ),
        ("framework matches nothing", "framework=ETHEREUM".to_string(), 0),
        (
            "status matches",
            "status=CREATING".to_string(),
            expected_ids.len(),
        ),
        ("status matches nothing", "status=DELETED".to_string(), 0),
    ];
    for (case, query, expected) in cases {
        let answer = call(&router, "GET", &format!("/networks?{query}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected ListNetworks 200, got {} {:?}",
            answer.status, answer.body
        );
        let count = answer.body["Networks"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, expected,
            "case {case}: expected {expected} networks, got {count} ({:?})",
            answer.body
        );
    }

    // The same filter follows the status the clock moved the networks to.
    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = call(&router, "GET", "/networks?status=AVAILABLE&maxResults=10", None).await;
    let count = answer.body["Networks"].as_array().map_or(0, Vec::len);
    assert_eq!(
        count,
        expected_ids.len(),
        "case status=AVAILABLE after the transition span: expected every network, got {count} \
         ({:?})",
        answer.body
    );
}
