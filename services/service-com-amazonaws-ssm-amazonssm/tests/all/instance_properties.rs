// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstanceProperties` end to end: which filter keys and values the operation
//! accepts, and which managed nodes of the store it reports for them.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"). A managed node enters the
//! store when it registers with the Systems Manager endpoint, which is the SSM Agent's
//! own call and is not part of this service's model, so these suites seed the store the
//! way a registration will and then assert that every response is computed from it.
//!
//! The suites are tables: each row is one input along the axis the rule under test
//! generalises over (filter key, filter value, operator, node count, page size,
//! Region), rather than the single filter the recorded unit happened to send.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::ops::types::{
    InstanceProperty, ASSOCIATION_STATUS_NAME_VALUES, INSTANCE_PROPERTY_FILTER_KEY_VALUES,
    INSTANCE_PROPERTY_FILTER_OPERATOR_VALUES, PING_STATUS_VALUES, PLATFORM_TYPE_VALUES,
    RESOURCE_TYPE_VALUES, SOURCE_TYPE_VALUES,
};
use service_com_amazonaws_ssm_amazonssm::state::{
    node_filter_keys, ComAmazonawsSsmAmazonssmState, ManagedNodeRecord,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise.
const REGION: &str = "us-west-2";

/// A second Region, to pin that a node is reported only through the endpoint it
/// registered with.
const OTHER_REGION: &str = "eu-west-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// The operation under test.
const OPERATION: &str = "DescribeInstanceProperties";

/// A router with just this service mounted on a fresh engine, plus the engine itself so
/// a suite can seed the managed nodes a registration would have written.
fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where a node's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `DescribeInstanceProperties` signed for `region`, returning `(status, body)`.
async fn call_in_region(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{OPERATION}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // The error shape travels in `__type`; fold in the header the framework also sets
    // so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call the operation signed for [`REGION`].
async fn call(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The node ids a response reports, in the order it reported them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["InstanceProperties"]
        .as_array()
        .expect("InstanceProperties is always present")
        .iter()
        .map(|property| {
            property["InstanceId"]
                .as_str()
                .expect("a seeded node reports its id")
                .to_string()
        })
        .collect()
}

/// One `FiltersWithOperator` request body.
fn string_filter(key: &str, values: &[&str], operator: Option<&str>) -> Value {
    let mut filter = json!({ "Key": key, "Values": values });
    if let Some(operator) = operator {
        filter["Operator"] = json!(operator);
    }
    json!({ "FiltersWithOperator": [filter] })
}

/// A managed node as a registration would have persisted it. The members are the
/// modelled `InstanceProperty` ones (`smithy_query(query="shape",
/// shape="InstanceProperty")`).
struct Seed<'a> {
    id: &'a str,
    platform_type: &'a str,
    platform_name: &'a str,
    ping_status: &'a str,
    agent_version: &'a str,
    resource_type: &'a str,
    source_type: &'a str,
    iam_role: &'a str,
    association_status: &'a str,
    tags: &'a [(&'a str, &'a str)],
}

/// Seed one managed node into `region`'s store, the way the agent's registration will.
fn seed(engine: &Arc<engine::InMemoryEngine>, region: &str, node: &Seed<'_>) {
    let record = ManagedNodeRecord {
        properties: InstanceProperty {
            name: Some(format!("node-{}", node.id)),
            instance_id: Some(node.id.to_string()),
            platform_type: Some(node.platform_type.to_string()),
            platform_name: Some(node.platform_name.to_string()),
            ping_status: Some(node.ping_status.to_string()),
            agent_version: Some(node.agent_version.to_string()),
            resource_type: Some(node.resource_type.to_string()),
            source_type: Some(node.source_type.to_string()),
            source_id: Some(node.id.to_string()),
            iam_role: Some(node.iam_role.to_string()),
            association_status: Some(node.association_status.to_string()),
            ..InstanceProperty::default()
        },
        tags: node
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<String, String>>(),
        ..ManagedNodeRecord::default()
    };
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(region, record),
        "case {}: a node reporting an id must be storable",
        node.id
    );
}

/// The three nodes every selection suite describes: one per modelled `PlatformType`,
/// each with its own ping status, agent version, resource type, source type, role and
/// tags, so a filter on any of those keys separates them.
const LINUX: &str = "i-0000000000000000a";
const WINDOWS: &str = "i-0000000000000000b";
const MACOS: &str = "mi-0000000000000000c";

fn seeded_router() -> MultiServiceRouter {
    let (router, engine) = harness();
    let nodes = [
        Seed {
            id: LINUX,
            platform_type: "Linux",
            platform_name: "Amazon Linux",
            ping_status: "Online",
            agent_version: "3.0.1124.0",
            resource_type: "EC2Instance",
            source_type: "AWS::EC2::Instance",
            iam_role: "ManagedInstanceRole",
            association_status: "Success",
            tags: &[("Purpose", "Test"), ("Owner", "platform")],
        },
        Seed {
            id: WINDOWS,
            platform_type: "Windows",
            platform_name: "Microsoft Windows Server 2022 Datacenter",
            ping_status: "ConnectionLost",
            agent_version: "2.3.714.0",
            resource_type: "EC2Instance",
            source_type: "AWS::EC2::Instance",
            iam_role: "WindowsNodeRole",
            association_status: "Failed",
            tags: &[("Purpose", "Production")],
        },
        Seed {
            id: MACOS,
            platform_type: "MacOS",
            platform_name: "macOS",
            ping_status: "Inactive",
            agent_version: "3.1.1.0",
            resource_type: "ManagedInstance",
            source_type: "AWS::SSM::ManagedInstance",
            iam_role: "HybridNodeRole",
            association_status: "Pending",
            tags: &[("Owner", "platform")],
        },
    ];
    for node in &nodes {
        seed(&engine, REGION, node);
    }
    router
}

/// Every filter key this service's model publishes for its managed nodes is accepted,
/// and a key that names no property of a node is refused with a `ValidationException`
/// naming the key member — whatever the property it looks like it names.
///
/// The accepted set is the modelled `InstancePropertyFilterKey` enum plus the remaining
/// keys the sibling string filter over the same nodes documents
/// (`smithy_query(query="doc", shape="InstanceInformationStringFilter", member="Key")`:
/// "... | SourceIds | SourceTypes | "tag-key" | "tag:{keyname}"").
#[tokio::test]
async fn only_the_published_filter_keys_are_accepted() {
    let router = seeded_router();

    // (key, a value that key accepts)
    let accepted: &[(&str, &str)] = &[
        ("ActivationIds", "00000000-1111-2222-3333-444444444444"),
        ("AgentVersion", "3.0.1124.0"),
        ("AssociationStatus", "Success"),
        ("DocumentName", "AWS-RunShellScript"),
        ("IamRole", "ManagedInstanceRole"),
        ("InstanceIds", LINUX),
        ("PingStatus", "Online"),
        ("PlatformTypes", "Linux"),
        ("ResourceType", "EC2Instance"),
        ("SourceIds", LINUX),
        ("SourceTypes", "AWS::EC2::Instance"),
        ("tag-key", "Purpose"),
        ("tag:Purpose", "Test"),
    ];
    for key in INSTANCE_PROPERTY_FILTER_KEY_VALUES {
        assert!(
            accepted.iter().any(|(accepted, _)| accepted == key),
            "the accepted table must cover the modelled filter key {key}"
        );
    }
    for key in node_filter_keys() {
        // The tag form is published with the tag name as a placeholder, so a row naming
        // any tag covers it.
        let covered = accepted.iter().any(|(row, _)| {
            *row == key || (key == "tag:{keyname}" && row.starts_with("tag:") && *row != "tag:")
        });
        assert!(
            covered,
            "the accepted table must cover the published filter key {key}"
        );
    }
    for (key, value) in accepted {
        let (status, body) = call(&router, &string_filter(key, &[value], None)).await;
        assert_eq!(
            status, 200,
            "case {key}={value}: a published filter key is accepted, got {status} {body}"
        );
    }

    // A key that is not one of them: a property of a node that is not a filter key, a
    // singular or plural spelling of one that is, a different case, and a tag filter
    // naming no tag. `PlatformName` is the key the recorded unit sent.
    let rejected = [
        "PlatformName",
        "PlatformVersion",
        "AvailabilityZone",
        "ComputerName",
        "InstanceId",
        "ActivationId",
        "AssociationStatuses",
        "instanceids",
        "PINGSTATUS",
        "Tag-Key",
        "tag:",
        "",
    ];
    for key in rejected {
        let body = if key.is_empty() {
            // `InstancePropertyStringFilterKey` has a minimum length of 1, so an empty
            // key is refused by the modelled boundary rather than by the handler; both
            // refusals are the same status and code.
            json!({ "FiltersWithOperator": [{ "Key": key, "Values": ["x"] }] })
        } else {
            string_filter(key, &["x"], None)
        };
        let (status, response) = call(&router, &body).await;
        assert_eq!(
            status, 400,
            "case key={key:?}: an unpublished filter key is refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case key={key:?}: the refusal is a ValidationException, got {response}"
        );
        if !key.is_empty() {
            let message = response["message"]
                .as_str()
                .or_else(|| response["Message"].as_str())
                .unwrap_or_default();
            assert_eq!(
                message,
                format!("{key} filter Key is invalid."),
                "case key={key:?}: the refusal names the key the caller submitted, got {response}"
            );
        }
    }
}

/// The modelled `InstancePropertyFilterList` reaches the same nodes and refuses the same
/// way: its `key` is the modelled enum, so a key outside it never reaches the handler.
#[tokio::test]
async fn the_modelled_filter_list_selects_and_refuses_too() {
    let router = seeded_router();

    let (status, body) = call(
        &router,
        &json!({ "InstancePropertyFilterList": [{ "key": "PlatformTypes", "valueSet": ["Linux"] }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case InstancePropertyFilterList/PlatformTypes: accepted, got {status} {body}"
    );
    assert_eq!(
        reported_ids(&body),
        vec![LINUX.to_string()],
        "case InstancePropertyFilterList/PlatformTypes: only the Linux node matches, got {body}"
    );

    let (status, body) = call(
        &router,
        &json!({ "InstancePropertyFilterList": [{ "key": "PlatformName", "valueSet": ["Amazon Linux"] }] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case InstancePropertyFilterList/PlatformName: refused, got {status} {body}"
    );
}

/// A filter selects the nodes whose property satisfies it, under every modelled
/// operator; filters are ANDed and a filter's values are ORed.
#[tokio::test]
async fn filters_select_the_nodes_the_store_holds() {
    let router = seeded_router();

    // (case, request body, the node ids the store answers with)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter at all", json!({}), vec![LINUX, WINDOWS, MACOS]),
        (
            "Equal on one value",
            string_filter("PlatformTypes", &["Linux"], Some("Equal")),
            vec![LINUX],
        ),
        (
            "Equal with no operator defaults to Equal",
            string_filter("PingStatus", &["Online"], None),
            vec![LINUX],
        ),
        (
            "Equal over two values is an OR",
            string_filter("PlatformTypes", &["Linux", "MacOS"], Some("Equal")),
            vec![LINUX, MACOS],
        ),
        (
            "NotEqual",
            string_filter("PlatformTypes", &["Linux"], Some("NotEqual")),
            vec![WINDOWS, MACOS],
        ),
        (
            "BeginWith on a free-text property",
            string_filter("IamRole", &["Managed"], Some("BeginWith")),
            vec![LINUX],
        ),
        (
            "BeginWith on a published value set takes a whole published value",
            string_filter("PlatformTypes", &["MacOS"], Some("BeginWith")),
            vec![MACOS],
        ),
        (
            "LessThan orders a dotted version by its numbers",
            string_filter("AgentVersion", &["3.0.1124.0"], Some("LessThan")),
            vec![WINDOWS],
        ),
        (
            "GreaterThan orders a dotted version by its numbers",
            string_filter("AgentVersion", &["3.0.1124.0"], Some("GreaterThan")),
            vec![MACOS],
        ),
        (
            "a node id",
            string_filter("InstanceIds", &[MACOS], None),
            vec![MACOS],
        ),
        (
            "a resource type",
            string_filter("ResourceType", &["ManagedInstance"], None),
            vec![MACOS],
        ),
        (
            "a source type",
            string_filter("SourceTypes", &["AWS::EC2::Instance"], None),
            vec![LINUX, WINDOWS],
        ),
        (
            "an association status",
            string_filter("AssociationStatus", &["Failed"], None),
            vec![WINDOWS],
        ),
        (
            "a tag key, whatever its value",
            string_filter("tag-key", &["Owner"], None),
            vec![LINUX, MACOS],
        ),
        (
            "a tag of a given value",
            string_filter("tag:Purpose", &["Production"], None),
            vec![WINDOWS],
        ),
        (
            "a tag nothing carries",
            string_filter("tag:Purpose", &["Absent"], None),
            vec![],
        ),
        (
            "a document no association targets",
            string_filter("DocumentName", &["AWS-RunShellScript"], None),
            vec![],
        ),
        (
            "two filters are ANDed",
            json!({
                "FiltersWithOperator": [
                    { "Key": "SourceTypes", "Values": ["AWS::EC2::Instance"] },
                    { "Key": "PingStatus", "Values": ["ConnectionLost"] },
                ]
            }),
            vec![WINDOWS],
        ),
        (
            "two legacy filters are ANDed",
            json!({
                "InstancePropertyFilterList": [
                    { "key": "ResourceType", "valueSet": ["EC2Instance"] },
                    { "key": "PingStatus", "valueSet": ["Online"] },
                ],
            }),
            vec![LINUX],
        ),
        (
            "two tag filters are ANDed (tag filters combine with each other)",
            json!({
                "FiltersWithOperator": [
                    { "Key": "tag-key", "Values": ["Owner"] },
                    { "Key": "tag:Purpose", "Values": ["Test"] },
                ]
            }),
            vec![LINUX],
        ),
    ];
    for operator in INSTANCE_PROPERTY_FILTER_OPERATOR_VALUES {
        assert!(
            cases
                .iter()
                .any(|(_, body, _)| serde_json::to_string(body)
                    .unwrap_or_default()
                    .contains(operator)),
            "the selection table must cover the modelled operator {operator}"
        );
    }

    for (case, body, expected) in cases {
        let (status, response) = call(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: the describe succeeds, got {status} {response}"
        );
        let mut reported = reported_ids(&response);
        reported.sort();
        let mut expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: expected the nodes {expected:?}, got {reported:?} ({response})"
        );
        assert!(
            response["NextToken"].is_null(),
            "case {case}: a page holding every match carries no token, got {response}"
        );

        // A read must not change what a later caller sees: the same request answers the
        // same way.
        let (_, repeated) = call(&router, &body).await;
        assert_eq!(
            reported_ids(&repeated),
            reported_ids(&response),
            "case {case}: describing twice reports the same nodes, got {repeated}"
        );
    }
}

/// A filter value outside the set its key publishes is refused whichever operator the
/// filter names: the published set is the domain of the node property the key selects on,
/// so a value outside it is one no node can hold — a prefix of a published value included.
#[tokio::test]
async fn filter_values_outside_a_published_set_are_refused() {
    let router = seeded_router();

    // (key, the value sets the model publishes for it)
    let published: &[(&str, &[&str])] = &[
        ("AssociationStatus", ASSOCIATION_STATUS_NAME_VALUES),
        ("PingStatus", PING_STATUS_VALUES),
        ("PlatformTypes", PLATFORM_TYPE_VALUES),
        ("ResourceType", RESOURCE_TYPE_VALUES),
        ("SourceTypes", SOURCE_TYPE_VALUES),
    ];
    for (key, values) in published {
        // Every published value is accepted, including ones no seeded node carries.
        for value in *values {
            let (status, body) = call(&router, &string_filter(key, &[value], Some("Equal"))).await;
            assert_eq!(
                status, 200,
                "case {key}={value}: a published value is accepted, got {status} {body}"
            );
        }
        // And a value that is not one of them is the modelled filter-value refusal, under
        // every modelled operator — including the ones that compare a PART of the value,
        // which a value the property can never hold is no part of either.
        for operator in ["Equal", "NotEqual", "BeginWith", "LessThan", "GreaterThan"] {
            let (status, body) = call(
                &router,
                &string_filter(key, &["NotAPublishedValue"], Some(operator)),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {key}/{operator}: an unpublished value is refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInstancePropertyFilterValue",
                "case {key}/{operator}: the refusal is the modelled one, got {body}"
            );
        }
        // A PREFIX of a published value is not a published value, so it is refused too,
        // and the refusal names the key and the set rather than the value it refused.
        let prefix = &values[0][..1];
        for operator in ["BeginWith", "LessThan", "GreaterThan"] {
            let (status, body) = call(&router, &string_filter(key, &[prefix], Some(operator))).await;
            assert_eq!(
                status, 400,
                "case {key}/{operator}={prefix}: a partial value is refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInstancePropertyFilterValue",
                "case {key}/{operator}={prefix}: the refusal is the modelled one, got {body}"
            );
        }
    }

    // An `InstanceIds` value that is not shaped like a node id (the modelled
    // `InstanceId` pattern `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`) names no node.
    let malformed = [
        "not-an-id",
        "i-123",
        "i-0000000000000000",
        "mi-12345678",
        "I-0000000000000000a",
    ];
    for value in malformed {
        let (status, body) = call(&router, &string_filter("InstanceIds", &[value], None)).await;
        assert_eq!(
            status, 400,
            "case InstanceIds={value}: a malformed node id is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInstanceId",
            "case InstanceIds={value}: the refusal is the modelled one, got {body}"
        );
    }
    // Both published node-id forms are accepted: 8 and 17 characters after `i-`, and 17
    // after `mi-`.
    for value in ["i-12345678", "i-0000000000000000a", MACOS] {
        let (status, body) = call(&router, &string_filter("InstanceIds", &[value], None)).await;
        assert_eq!(
            status, 200,
            "case InstanceIds={value}: a well-formed node id is accepted, got {status} {body}"
        );
    }
}

/// Paging walks every matching node exactly once, in ascending node-id order, whatever
/// the page size; the last page carries no token, and a token this operation never
/// issued is the modelled `InvalidNextToken`.
#[tokio::test]
async fn paging_walks_every_node_once() {
    let (router, engine) = harness();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..12_u32 {
        let id = format!("i-{index:017}");
        ids.push(id.clone());
        seed(
            &engine,
            REGION,
            &Seed {
                id: &id,
                platform_type: PLATFORM_TYPE_VALUES[(index as usize) % PLATFORM_TYPE_VALUES.len()],
                platform_name: "Amazon Linux",
                ping_status: PING_STATUS_VALUES[(index as usize) % PING_STATUS_VALUES.len()],
                agent_version: "3.0.1124.0",
                resource_type: "EC2Instance",
                source_type: "AWS::EC2::Instance",
                iam_role: "ManagedInstanceRole",
                association_status: "Success",
                tags: &[],
            },
        );
    }
    ids.sort();

    // The modelled `MaxResults` range is 5..1000, so 5 is the smallest page a caller may
    // ask for; 12 nodes over each size exercises a short last page, an exact fit and a
    // single page.
    for page_size in [5_usize, 6, 12, 1000] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let (status, response) = call(&router, &body).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: a page is served, got {status} {response}"
            );
            let page = reported_ids(&response);
            assert!(
                page.len() <= page_size,
                "case page_size={page_size}: a page holds at most that many nodes, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= 13,
                "case page_size={page_size}: paging must terminate, walked {walked:?}"
            );
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, ids,
            "case page_size={page_size}: paging reports every node once, in node-id order"
        );
    }

    // A token that is not one this operation issued.
    for token in ["not-a-token", "i-123", "0"] {
        let (status, body) = call(&router, &json!({ "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case NextToken={token}: an unissued token is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNextToken",
            "case NextToken={token}: the refusal is the modelled one, got {body}"
        );
    }
}

/// A node is reported only through the endpoint of the Region it registered with, and a
/// Region holding none reports an empty list rather than another Region's nodes.
#[tokio::test]
async fn nodes_are_reported_only_in_their_own_region() {
    let (router, engine) = harness();
    let node = Seed {
        id: LINUX,
        platform_type: "Linux",
        platform_name: "Amazon Linux",
        ping_status: "Online",
        agent_version: "3.0.1124.0",
        resource_type: "EC2Instance",
        source_type: "AWS::EC2::Instance",
        iam_role: "ManagedInstanceRole",
        association_status: "Success",
        tags: &[],
    };
    seed(&engine, REGION, &node);

    for (region, expected) in [(REGION, vec![LINUX.to_string()]), (OTHER_REGION, vec![])] {
        let (status, body) = call_in_region(&router, region, &json!({})).await;
        assert_eq!(
            status, 200,
            "case region={region}: the describe succeeds, got {status} {body}"
        );
        assert_eq!(
            reported_ids(&body),
            expected,
            "case region={region}: expected {expected:?}, got {body}"
        );
    }
}

/// A store nobody has registered a node in reports an empty list — not a refusal, and
/// not a node nobody created.
#[tokio::test]
async fn an_empty_store_reports_no_nodes() {
    let (router, _engine) = harness();
    let (status, body) = call(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "an empty store is described, not refused, got {status} {body}"
    );
    assert_eq!(
        reported_ids(&body),
        Vec::<String>::new(),
        "an empty store reports no nodes, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "an empty page carries no token, got {body}"
    );
}
