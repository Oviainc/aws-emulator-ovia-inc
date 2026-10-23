// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstanceInformation` end to end: the managed nodes this Region holds,
//! narrowed by filters and served one page at a time.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.DescribeInstanceInformation` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! The inventory itself is seeded through the service's own state
//! (`state::ComAmazonawsSsmAmazonssmState::insert_managed_node`, the one node store the
//! sibling `DescribeInstanceProperties` reads too), because a node enters it only by
//! REGISTERING — an SSM Agent installed on an instance or a hybrid node activated with an
//! activation code — and no public API of this service registers one (`CreateActivation`
//! and `DeregisterManagedInstance` are still scaffolds). Seeding the store is what lets
//! these cases prove the response is computed from persisted state: what is registered is
//! what is reported, per Region, and nothing else.
//!
//! The suites are tables: each row is one input along the axis the rule under test
//! generalises over (filter key, filter value, value count, filter count, node kind,
//! platform, tag, Region, page size, node count), rather than the single filter the
//! recorded units happened to send.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::ops::types::InstanceAggregatedAssociationOverview;
use service_com_amazonaws_ssm_amazonssm::state::{
    ComAmazonawsSsmAmazonssmState, ManagedNodeRecord, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_MANAGED_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so a node reported here proves the Region came from the request's
/// SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that a node is reported only through the Region it registered
/// in.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// The operation under test.
const OPERATION: &str = "DescribeInstanceInformation";

/// An EC2 node id whose suffix is 8 characters long (the modelled `InstanceId` pattern
/// `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$` admits both lengths).
const EC2_SHORT: &str = "i-0abc1234";

/// An EC2 node id whose suffix is 17 characters long (same pattern).
const EC2_LONG: &str = "i-0123456789abcdef0";

/// A hybrid node id: `mi-` and 17 characters (same pattern).
const HYBRID: &str = "mi-01234567890123456";

/// A node id that is well formed (8 characters after `i-`, as the modelled pattern
/// admits) but names no registered node.
const UNREGISTERED: &str = "i-0dead999";

/// The activation [`HYBRID`] registered through, matching the modelled `ActivationId`
/// pattern `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
const ACTIVATION: &str = "1a2b3c4d-1111-2222-3333-444455556666";

/// A router with just this service mounted, and the engine it was mounted on — the
/// handle a case seeds the node inventory through.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region a node is reported through comes
    // from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Register `record` in `region` the way a node's own registration would.
fn register(engine: &Arc<engine::InMemoryEngine>, region: &str, record: ManagedNodeRecord) {
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(region, record),
        "a seeded node must carry the node id it is stored under"
    );
}

/// How many nodes the inventory holds, to pin that a report wrote nothing.
fn registered_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .managed_nodes
        .len()
}

/// Call `DescribeInstanceInformation` signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
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
    // The error shape travels in `__type`; fold in the header the framework also sets so
    // one accessor reads either.
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

/// The node ids a successful report lists, in the order it listed them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["InstanceInformationList"]
        .as_array()
        .expect("a report carries InstanceInformationList")
        .iter()
        .map(|node| {
            node["InstanceId"]
                .as_str()
                .expect("every reported node carries an InstanceId")
                .to_string()
        })
        .collect()
}

/// A report that must have succeeded, with its node ids.
async fn report(router: &MultiServiceRouter, case: &str, body: &Value) -> (Value, Vec<String>) {
    let (status, parsed) = call(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
    );
    let ids = reported_ids(&parsed);
    (parsed, ids)
}

/// The three nodes of [`REGION`], plus one in [`OTHER_REGION`], spanning node kind,
/// platform, ping status, agent version, activation, source and tags.
fn seed_mixed_inventory(engine: &Arc<engine::InMemoryEngine>) {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let mut linux = ManagedNodeRecord::registering(EC2_SHORT, RESOURCE_TYPE_EC2_INSTANCE, at);
    linux.properties.platform_type = Some("Linux".to_string());
    linux.properties.platform_name = Some("Amazon Linux".to_string());
    linux.properties.platform_version = Some("2023".to_string());
    linux.properties.agent_version = Some("3.2.1234.0".to_string());
    linux.is_latest_version = Some(true);
    linux.properties.iam_role = Some("SSMInstanceRole".to_string());
    linux.properties.ip_address = Some("10.0.1.17".to_string());
    linux.properties.computer_name = Some("ip-10-0-1-17.us-west-2.compute.internal".to_string());
    linux.tags = BTreeMap::from([
        ("env".to_string(), "dev".to_string()),
        ("Name".to_string(), "web".to_string()),
    ]);
    register(engine, REGION, linux);

    let mut windows = ManagedNodeRecord::registering(EC2_LONG, RESOURCE_TYPE_EC2_INSTANCE, at);
    windows.properties.ping_status = Some("ConnectionLost".to_string());
    windows.properties.platform_type = Some("Windows".to_string());
    windows.properties.agent_version = Some("3.1.1.0".to_string());
    windows.is_latest_version = Some(false);
    windows.tags = BTreeMap::from([("env".to_string(), "prod".to_string())]);
    register(engine, REGION, windows);

    let mut hybrid = ManagedNodeRecord::registering(HYBRID, RESOURCE_TYPE_MANAGED_INSTANCE, at);
    hybrid.properties.platform_type = Some("MacOS".to_string());
    hybrid.properties.activation_id = Some(ACTIVATION.to_string());
    hybrid.properties.name = Some("edge-1".to_string());
    hybrid.properties.association_status = Some("Success".to_string());
    hybrid.properties.association_overview = Some(InstanceAggregatedAssociationOverview {
        detailed_status: Some("Success".to_string()),
        instance_association_status_aggregated_count: Some(HashMap::from([(
            "Success".to_string(),
            2,
        )])),
    });
    hybrid.properties.source_type = Some("AWS::IoT::Thing".to_string());
    hybrid.properties.source_id = Some("thing-1".to_string());
    hybrid.properties.source_location = Some("edge-site-1".to_string());
    hybrid.tags = BTreeMap::from([("env".to_string(), "dev".to_string())]);
    register(engine, REGION, hybrid);

    // Same account, another Region: never reported through [`REGION`].
    register(
        engine,
        OTHER_REGION,
        ManagedNodeRecord::registering("i-0eeeeeee", RESOURCE_TYPE_EC2_INSTANCE, at),
    );
}

/// Node ids in the order a report lists them (the store's node-id order).
fn sorted(ids: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_filter_reports_the_registered_nodes_whose_attribute_it_names_matches() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    // One row per filter key the `InstanceInformationStringFilter$Key` doc lists, plus
    // the two tag forms, each with the nodes seeded above that hold that value. The
    // `Values` list unions (rows with two values) and a key the node holds nothing for
    // matches nothing (the `tag:absent` and unregistered-id rows).
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("InstanceIds", &[EC2_SHORT], &[EC2_SHORT]),
        ("InstanceIds", &[EC2_SHORT, HYBRID], &[EC2_SHORT, HYBRID]),
        ("InstanceIds", &[UNREGISTERED], &[]),
        ("PingStatus", &["Online"], &[EC2_SHORT, HYBRID]),
        ("PingStatus", &["ConnectionLost"], &[EC2_LONG]),
        ("PingStatus", &["Inactive"], &[]),
        ("PlatformTypes", &["Linux"], &[EC2_SHORT]),
        ("PlatformTypes", &["Windows", "MacOS"], &[EC2_LONG, HYBRID]),
        ("ResourceType", &["EC2Instance"], &[EC2_SHORT, EC2_LONG]),
        ("ResourceType", &["ManagedInstance"], &[HYBRID]),
        ("AgentVersion", &["3.2.1234.0"], &[EC2_SHORT]),
        ("AgentVersion", &["9.9.9.9"], &[]),
        ("IamRole", &["SSMInstanceRole"], &[EC2_SHORT]),
        ("ActivationIds", &[ACTIVATION], &[HYBRID]),
        ("AssociationStatus", &["Success"], &[HYBRID]),
        ("AssociationStatus", &["Pending"], &[]),
        ("SourceIds", &["thing-1"], &[HYBRID]),
        ("SourceTypes", &["AWS::IoT::Thing"], &[HYBRID]),
        ("SourceTypes", &["AWS::EC2::Instance"], &[]),
        ("tag-key", &["env"], &[EC2_SHORT, EC2_LONG, HYBRID]),
        ("tag-key", &["Name"], &[EC2_SHORT]),
        ("tag:env", &["dev"], &[EC2_SHORT, HYBRID]),
        ("tag:env", &["prod"], &[EC2_LONG]),
        ("tag:env", &["dev", "prod"], &[EC2_SHORT, EC2_LONG, HYBRID]),
        ("tag:Name", &["web"], &[EC2_SHORT]),
        ("tag:absent", &["web"], &[]),
    ];

    for (key, values, expected) in cases {
        let case = format!("Key={key} Values={values:?}");
        let body = json!({ "Filters": [{ "Key": key, "Values": values }] });
        let (_, ids) = report(&router, &case, &body).await;
        assert_eq!(
            ids,
            sorted(expected),
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn filters_intersect_while_each_filters_values_union() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    // Rows differ in how many filters they carry and in whether the filters can hold of
    // one node at once: a node is reported only when EVERY filter is satisfied.
    let cases: &[(&str, Value, &[&str])] = &[
        (
            "one filter",
            json!([{ "Key": "PingStatus", "Values": ["Online"] }]),
            &[EC2_SHORT, HYBRID],
        ),
        (
            "two filters, both hold of one node",
            json!([
                { "Key": "PingStatus", "Values": ["Online"] },
                { "Key": "PlatformTypes", "Values": ["Linux"] }
            ]),
            &[EC2_SHORT],
        ),
        (
            "two filters, no node holds both",
            json!([
                { "Key": "PingStatus", "Values": ["ConnectionLost"] },
                { "Key": "ResourceType", "Values": ["ManagedInstance"] }
            ]),
            &[],
        ),
        (
            "three filters narrowing to the hybrid node",
            json!([
                { "Key": "ResourceType", "Values": ["ManagedInstance"] },
                { "Key": "PingStatus", "Values": ["Online"] },
                { "Key": "ActivationIds", "Values": [ACTIVATION] }
            ]),
            &[HYBRID],
        ),
        (
            "no filters at all",
            json!([]),
            &[EC2_SHORT, EC2_LONG, HYBRID],
        ),
    ];

    for (case, filters, expected) in cases {
        let (_, ids) = report(&router, case, &json!({ "Filters": filters })).await;
        assert_eq!(
            ids,
            sorted(expected),
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn the_legacy_filter_list_narrows_the_same_report() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    // The legacy `InstanceInformationFilterList` carries the same keys (the modelled
    // `InstanceInformationFilterKey` enum) under lowercase member names, and no tag form.
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("InstanceIds", &[EC2_LONG], &[EC2_LONG]),
        ("PlatformTypes", &["Linux"], &[EC2_SHORT]),
        ("ResourceType", &["ManagedInstance"], &[HYBRID]),
        ("PingStatus", &["Online"], &[EC2_SHORT, HYBRID]),
    ];

    for (key, value_set, expected) in cases {
        let case = format!("legacy key={key} valueSet={value_set:?}");
        let body = json!({
            "InstanceInformationFilterList": [{ "key": key, "valueSet": value_set }]
        });
        let (_, ids) = report(&router, &case, &body).await;
        assert_eq!(
            ids,
            sorted(expected),
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_node_is_reported_only_through_the_region_it_registered_in() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    let cases: &[(&str, &[&str])] = &[
        (REGION, &[EC2_SHORT, EC2_LONG, HYBRID]),
        (OTHER_REGION, &["i-0eeeeeee"]),
        ("eu-west-1", &[]),
    ];

    for (region, expected) in cases {
        let (status, parsed) = call_in_region(&router, region, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {parsed:?}"
        );
        let ids = reported_ids(&parsed);
        assert_eq!(
            ids,
            sorted(expected),
            "case {region:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn an_account_with_no_registered_node_is_reported_as_holding_none() {
    let (router, _engine) = service();

    // Rows differ in what the request asks for; none of them can invent a node.
    let cases: &[(&str, Value)] = &[
        ("no filters", json!({})),
        (
            "a node id that is well formed",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": [EC2_LONG] }] }),
        ),
        (
            "a tag filter",
            json!({ "Filters": [{ "Key": "tag:env", "Values": ["dev"] }] }),
        ),
        ("a page size", json!({ "MaxResults": 50 })),
    ];

    for (case, body) in cases {
        let (parsed, ids) = report(&router, case, body).await;
        assert!(
            ids.is_empty(),
            "case {case:?}: expected no nodes, got {ids:?}"
        );
        assert!(
            parsed.get("NextToken").is_none(),
            "case {case:?}: an exhausted report carries no NextToken, got {parsed:?}"
        );
    }
}

#[tokio::test]
async fn a_reported_node_carries_what_registration_recorded() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    let (_, _) = report(&router, "warm up", &json!({})).await;
    let (parsed, _) = report(
        &router,
        "the hybrid node",
        &json!({ "Filters": [{ "Key": "InstanceIds", "Values": [HYBRID] }] }),
    )
    .await;
    let node = &parsed["InstanceInformationList"][0];

    // One row per member the record held, so a response that is not built from the
    // stored node cannot pass.
    let cases: &[(&str, &str)] = &[
        ("InstanceId", HYBRID),
        ("PingStatus", "Online"),
        ("ResourceType", "ManagedInstance"),
        ("PlatformType", "MacOS"),
        ("ActivationId", ACTIVATION),
        ("Name", "edge-1"),
        ("AssociationStatus", "Success"),
        ("SourceType", "AWS::IoT::Thing"),
        ("SourceId", "thing-1"),
        ("SourceLocation", "edge-site-1"),
    ];
    for (member, expected) in cases {
        assert_eq!(
            node[member].as_str(),
            Some(*expected),
            "case {member:?}: expected {expected:?}, got {:?}",
            node[member]
        );
    }
    assert_eq!(
        node["AssociationOverview"]["DetailedStatus"].as_str(),
        Some("Success"),
        "case \"AssociationOverview\": expected the stored detailed status, got {:?}",
        node["AssociationOverview"]
    );
    assert_eq!(
        node["AssociationOverview"]["InstanceAssociationStatusAggregatedCount"]["Success"].as_i64(),
        Some(2),
        "case \"AssociationOverview\": expected the stored aggregated count, got {:?}",
        node["AssociationOverview"]
    );
    assert!(
        node["RegistrationDate"].is_number(),
        "case \"RegistrationDate\": expected the stored registration instant, got {:?}",
        node["RegistrationDate"]
    );

    // A node registered with no agent reported has no agent version to report, rather
    // than an invented one.
    let (parsed, _) = report(
        &router,
        "the Linux node",
        &json!({ "Filters": [{ "Key": "InstanceIds", "Values": [EC2_SHORT] }] }),
    )
    .await;
    let node = &parsed["InstanceInformationList"][0];
    assert_eq!(
        node["AgentVersion"].as_str(),
        Some("3.2.1234.0"),
        "case \"AgentVersion\": expected the stored version, got {:?}",
        node["AgentVersion"]
    );
    assert_eq!(
        node["IsLatestVersion"].as_bool(),
        Some(true),
        "case \"IsLatestVersion\": expected the stored flag, got {:?}",
        node["IsLatestVersion"]
    );
    assert!(
        node.get("ActivationId").is_none(),
        "case \"ActivationId\": an EC2 node registered without an activation carries \
         none, got {:?}",
        node["ActivationId"]
    );
}

#[tokio::test]
async fn a_report_changes_nothing_a_later_caller_sees() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);
    let before = registered_count(&engine);

    let (first, _) = report(&router, "first read", &json!({})).await;
    let (second, _) = report(&router, "second read", &json!({})).await;
    assert_eq!(
        first, second,
        "two reads of one inventory must report the same nodes, got {first:?} then {second:?}"
    );

    // Rows that would be where a report leaked a write: a filtered read and a paged one.
    for (case, body) in [
        (
            "filtered read",
            json!({ "Filters": [{ "Key": "PingStatus", "Values": ["Online"] }] }),
        ),
        ("paged read", json!({ "MaxResults": 5 })),
    ] {
        report(&router, case, &body).await;
    }
    assert_eq!(
        registered_count(&engine),
        before,
        "case \"report\": a report must register and remove nothing, held {before} before"
    );
}

#[tokio::test]
async fn a_page_holds_max_results_nodes_and_its_token_resumes_after_it() {
    // Rows differ in how many nodes are registered and how big a page is, including the
    // modelled bounds of `MaxResultsEC2Compatible` (5..50) and counts that divide the
    // page size exactly, leave a remainder, or fit in one page.
    let cases: &[(usize, i32)] = &[
        (12, 5),
        (10, 5),
        (3, 5),
        (51, 50),
        (50, 50),
        (23, 7),
        (1, 5),
    ];

    for (nodes, page_size) in cases {
        let case = format!("{nodes} nodes, MaxResults={page_size}");
        let (router, engine) = service();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut expected: Vec<String> = Vec::new();
        // Registered in DESCENDING id order, so a page boundary that depended on
        // insertion order rather than node-id order would drop or repeat nodes below.
        for index in (0..*nodes).rev() {
            // 17 characters after `i-`, so every id satisfies the modelled pattern.
            let instance_id = format!("i-{index:017}");
            expected.push(instance_id.clone());
            register(
                &engine,
                REGION,
                ManagedNodeRecord::registering(instance_id, RESOURCE_TYPE_EC2_INSTANCE, at),
            );
        }
        expected.sort();

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &token {
                None => json!({ "MaxResults": page_size }),
                Some(token) => json!({ "MaxResults": page_size, "NextToken": token }),
            };
            let (parsed, ids) = report(&router, &case, &body).await;
            pages += 1;
            assert!(
                ids.len() <= *page_size as usize,
                "case {case:?}: a page holds at most {page_size} nodes, got {}",
                ids.len()
            );
            walked.extend(ids);
            token = parsed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= nodes + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: the pages together must report every node exactly once, in \
             node-id order, got {walked:?}"
        );
        let full_pages = nodes / (*page_size as usize);
        let expected_pages = if nodes % (*page_size as usize) == 0 {
            full_pages.max(1)
        } else {
            full_pages + 1
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn a_filter_the_operation_does_not_accept_is_refused() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    // Each row is one request shape and the modelled error code it must answer with:
    //   * a key outside the documented catalog -> InvalidFilterKey
    //   * a tag filter combined with any other filter type -> InvalidFilterKey
    //     ("Tag filters can't be combined with other filter types")
    //   * a value that is not a value of the node attribute the key names ->
    //     InvalidInstanceInformationFilterValue, or InvalidInstanceId for a node id
    //   * both filter members at once -> ValidationException
    let cases: &[(&str, Value, &str)] = &[
        // Keys the operation does not document.
        (
            "key InstanceId (singular)",
            json!({ "Filters": [{ "Key": "InstanceId", "Values": [EC2_SHORT] }] }),
            "InvalidFilterKey",
        ),
        (
            "key PingStatuses",
            json!({ "Filters": [{ "Key": "PingStatuses", "Values": ["Online"] }] }),
            "InvalidFilterKey",
        ),
        (
            "key platformtypes (wrong case)",
            json!({ "Filters": [{ "Key": "platformtypes", "Values": ["Linux"] }] }),
            "InvalidFilterKey",
        ),
        (
            "key Tag-Key (wrong case)",
            json!({ "Filters": [{ "Key": "Tag-Key", "Values": ["env"] }] }),
            "InvalidFilterKey",
        ),
        (
            "key tag: naming no tag",
            json!({ "Filters": [{ "Key": "tag:", "Values": ["dev"] }] }),
            "InvalidFilterKey",
        ),
        // The legacy list's `key` is the modelled `InstanceInformationFilterKey` enum,
        // so a key outside it — including either tag form, which that legacy method does
        // not support — is refused by the protocol's boundary validator before the
        // handler runs, with the constraint message it writes.
        (
            "legacy key SourceIds (not in the legacy enum)",
            json!({
                "InstanceInformationFilterList": [{ "key": "SourceIds", "valueSet": ["thing-1"] }]
            }),
            "ValidationException",
        ),
        (
            "legacy key tag:env (the legacy list supports no tags)",
            json!({
                "InstanceInformationFilterList": [{ "key": "tag:env", "valueSet": ["dev"] }]
            }),
            "ValidationException",
        ),
        // Tag filters mixed with another filter type, either way round.
        (
            "tag:env then PingStatus",
            json!({ "Filters": [
                { "Key": "tag:env", "Values": ["dev"] },
                { "Key": "PingStatus", "Values": ["Online"] }
            ] }),
            "InvalidFilterKey",
        ),
        (
            "PlatformTypes then tag-key",
            json!({ "Filters": [
                { "Key": "PlatformTypes", "Values": ["Linux"] },
                { "Key": "tag-key", "Values": ["env"] }
            ] }),
            "InvalidFilterKey",
        ),
        (
            "InstanceIds then tag:Name",
            json!({ "Filters": [
                { "Key": "InstanceIds", "Values": [EC2_SHORT] },
                { "Key": "tag:Name", "Values": ["web"] }
            ] }),
            "InvalidFilterKey",
        ),
        // Node ids that are not node ids.
        (
            "node id i-bad",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": ["i-bad"] }] }),
            "InvalidInstanceId",
        ),
        (
            "node id not-an-id",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": ["not-an-id"] }] }),
            "InvalidInstanceId",
        ),
        (
            "node id i- with nothing after it",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": ["i-"] }] }),
            "InvalidInstanceId",
        ),
        (
            "node id mi- with 3 characters",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": ["mi-123"] }] }),
            "InvalidInstanceId",
        ),
        (
            "node id one character short of 17",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef"] }] }),
            "InvalidInstanceId",
        ),
        (
            "node id with a trailing space",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": ["i-0abc1234 "] }] }),
            "InvalidInstanceId",
        ),
        (
            "a malformed id beside a well-formed one",
            json!({ "Filters": [{ "Key": "InstanceIds", "Values": [EC2_SHORT, "i-xyz"] }] }),
            "InvalidInstanceId",
        ),
        (
            "legacy list with a malformed id",
            json!({
                "InstanceInformationFilterList": [{ "key": "InstanceIds", "valueSet": ["i-xyz"] }]
            }),
            "InvalidInstanceId",
        ),
        // Values outside the catalog of the node attribute the key names.
        (
            "PingStatus Bogus",
            json!({ "Filters": [{ "Key": "PingStatus", "Values": ["Bogus"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "PlatformTypes Solaris",
            json!({ "Filters": [{ "Key": "PlatformTypes", "Values": ["Solaris"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "PlatformTypes linux (wrong case)",
            json!({ "Filters": [{ "Key": "PlatformTypes", "Values": ["linux"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "ResourceType Lambda",
            json!({ "Filters": [{ "Key": "ResourceType", "Values": ["Lambda"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "SourceTypes AWS::Fake::Thing",
            json!({ "Filters": [{ "Key": "SourceTypes", "Values": ["AWS::Fake::Thing"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "AssociationStatus Unknown",
            json!({ "Filters": [{ "Key": "AssociationStatus", "Values": ["Unknown"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "ActivationIds not-a-uuid",
            json!({ "Filters": [{ "Key": "ActivationIds", "Values": ["not-a-uuid"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "AgentVersion with two components",
            json!({ "Filters": [{ "Key": "AgentVersion", "Values": ["3.2"] }] }),
            "InvalidInstanceInformationFilterValue",
        ),
        (
            "legacy list with an out-of-catalog value",
            json!({
                "InstanceInformationFilterList": [{ "key": "PingStatus", "valueSet": ["Bogus"] }]
            }),
            "InvalidInstanceInformationFilterValue",
        ),
        // The two filter members are alternatives.
        (
            "both filter members",
            json!({
                "Filters": [{ "Key": "PingStatus", "Values": ["Online"] }],
                "InstanceInformationFilterList": [
                    { "key": "PlatformTypes", "valueSet": ["Linux"] }
                ]
            }),
            "ValidationException",
        ),
        // A token this service did not issue: the cursor it hands out is a node id, so a
        // token that is not shaped like one was never ours.
        (
            "next token that is not a node id",
            json!({ "NextToken": "zzzz" }),
            "InvalidNextToken",
        ),
        (
            "next token with a node id's prefix but the wrong length",
            json!({ "NextToken": "i-0abc12345" }),
            "InvalidNextToken",
        ),
        (
            "next token naming a parameter instead of a node",
            json!({ "NextToken": "/Dev/DBServer" }),
            "InvalidNextToken",
        ),
        // One past each published bound of `MaxResults` (the modelled
        // `MaxResultsEC2Compatible` range 5..50; the bounds themselves are paged above).
        (
            "MaxResults one below the minimum",
            json!({ "MaxResults": 4 }),
            "ValidationException",
        ),
        (
            "MaxResults one above the maximum",
            json!({ "MaxResults": 51 }),
            "ValidationException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, parsed) = call(&router, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            *expected,
            "case {case:?}: expected {expected}, got {parsed:?}"
        );
    }
}

#[tokio::test]
async fn every_documented_filter_key_and_value_is_accepted() {
    let (router, engine) = service();
    seed_mixed_inventory(&engine);

    // The complement of the refusal table: each key the operation documents, with a
    // value from its own catalog (or a legal id form), is answered rather than refused —
    // including the node-id forms the modelled `InstanceId` pattern admits.
    let cases: &[(&str, &[&str])] = &[
        ("InstanceIds", &["i-01234567", "i-0123456789abcdef0", HYBRID]),
        ("PingStatus", &["Online", "ConnectionLost", "Inactive"]),
        ("PlatformTypes", &["Windows", "Linux", "MacOS"]),
        ("ResourceType", &["EC2Instance", "ManagedInstance"]),
        (
            "SourceTypes",
            &[
                "AWS::EC2::Instance",
                "AWS::IoT::Thing",
                "AWS::SSM::ManagedInstance",
                "Microsoft.Compute/virtualMachines",
            ],
        ),
        ("AssociationStatus", &["Success", "Pending", "Failed"]),
        ("ActivationIds", &[ACTIVATION]),
        ("AgentVersion", &["3.2.1234.0", "3.1.1.0", "123456.1.2"]),
        ("IamRole", &["any-role-name"]),
        ("SourceIds", &["thing-1"]),
        ("tag-key", &["env"]),
        ("tag:env", &["dev"]),
    ];

    for (key, values) in cases {
        for value in *values {
            let case = format!("Key={key} Values=[{value}]");
            let body = json!({ "Filters": [{ "Key": key, "Values": [value] }] });
            let (status, parsed) = call(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
            );
            assert!(
                parsed["InstanceInformationList"].is_array(),
                "case {case:?}: expected a report carrying InstanceInformationList, got {parsed:?}"
            );
        }
    }
}
