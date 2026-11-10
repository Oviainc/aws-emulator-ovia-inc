// SPDX-License-Identifier: CC0-1.0

//! The filter validation the two managed-node describes share:
//! `DescribeInstanceInformation` and `DescribeInstanceProperties` narrow the SAME node
//! inventory through the same filter vocabulary, so they refuse the same requests the same
//! way.
//!
//! Two rules are pinned here, each over a table of inputs rather than over the one filter
//! a recorded call happened to send:
//!
//! 1. A value that violates a modelled regular-expression constraint is refused with the
//!    constraint it violated — the member the value filled and the expression the model
//!    constrains that member with — so the refusal can be read by a client, not only by a
//!    human. The constraint here is the managed-node id one (`smithy_query(query="shape",
//!    shape="InstanceId")`), and the message is the same whatever the bad value was,
//!    because it reports the RULE and not the value.
//! 2. "Tag filters can't be combined with other filter types"
//!    (`smithy_query(query="doc", shape="InstanceInformationStringFilter", member="Key")`
//!    and the `Filters` member doc): a filter on a tag and a filter on a property of a
//!    node cannot be ANDed, while two tag filters, and two property filters, can. Both
//!    keys of a refused pair are keys the operation accepts, so the refusal is a
//!    `ValidationException` about the request as a whole, worded about the filter KINDS,
//!    while a key the operation does not know is refused naming THAT key.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_ssm_amazonssm::ops::types::InstanceProperty;
use service_com_amazonaws_ssm_amazonssm::state::{ComAmazonawsSsmAmazonssmState, ManagedNodeRecord};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// The modelled `InstanceId` pattern is `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`
/// (`smithy_query(query="shape", shape="InstanceId")`). What a constraint violation
/// reports is the alternation that pattern wraps: each branch anchors itself, so the
/// alternation is the whole constraint.
const NODE_ID_PATTERN: &str = r"(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)";

/// The input member a managed-node id fills, as a constraint violation names it.
const NODE_ID_MEMBER: &str = "instanceId";

/// The refusal a value that violates the node-id constraint must carry, written out here
/// independently of the service so the test pins the wording rather than echoing it.
fn expected_constraint_violation() -> String {
    format!(
        "Value at '{NODE_ID_MEMBER}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {NODE_ID_PATTERN}"
    )
}

/// The refusal a request mixing filter kinds carries: one sentence about the KINDS, which
/// the service answers whichever keys the request named.
const MIXED_FILTER_KINDS_REJECTION: &str = "Cannot mix tag or inventory filter with any other filter.";

/// One of the two managed-node describes, and the request and response members its
/// filters and its report travel in.
#[derive(Clone, Copy, Debug)]
struct NodeApi {
    /// The operation name, as `X-Amz-Target` carries it.
    operation: &'static str,
    /// The member a string filter list goes in.
    filters_member: &'static str,
    /// The member the legacy, operator-less filter list goes in.
    legacy_member: &'static str,
    /// The member a successful report lists its nodes in.
    report_member: &'static str,
}

const INFORMATION: NodeApi = NodeApi {
    operation: "DescribeInstanceInformation",
    filters_member: "Filters",
    legacy_member: "InstanceInformationFilterList",
    report_member: "InstanceInformationList",
};

const PROPERTIES: NodeApi = NodeApi {
    operation: "DescribeInstanceProperties",
    filters_member: "FiltersWithOperator",
    legacy_member: "InstancePropertyFilterList",
    report_member: "InstanceProperties",
};

/// Both managed-node describes, so every rule below is asserted on each of them.
const BOTH: [NodeApi; 2] = [INFORMATION, PROPERTIES];

/// A router with just this service mounted on a fresh engine, plus the engine a case
/// seeds the node inventory through.
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

/// One Linux EC2 node carrying the tags the combination cases filter on, registered the
/// way the agent's own registration will — so an accepted combination has something to
/// report and a refused one is refused before any node is looked at.
fn seeded_router() -> MultiServiceRouter {
    let (router, engine) = harness();
    let mut record = ManagedNodeRecord {
        properties: InstanceProperty {
            instance_id: Some(SEEDED_NODE.to_string()),
            resource_type: Some("EC2Instance".to_string()),
            platform_type: Some("Linux".to_string()),
            ping_status: Some("Online".to_string()),
            ..InstanceProperty::default()
        },
        ..ManagedNodeRecord::default()
    };
    record.tags.insert("Purpose".to_string(), "Test".to_string());
    record.tags.insert("Owner".to_string(), "platform".to_string());
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(REGION, record),
        "a seeded node must carry the node id it is stored under"
    );
    router
}

/// The node the seeded inventory holds: a 17-character EC2 id, one of the forms the
/// modelled pattern admits.
const SEEDED_NODE: &str = "i-0123456789abcdef0";

/// Call `operation` signed for [`REGION`], returning `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/ssm/aws4_request, \
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

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The message of a failure body, under either spelling the protocols use.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// One filter of a request: the key and the values it carries.
type Filter<'a> = (&'a str, &'a [&'a str]);

/// One case of a combination table: what it exercises, and the filters it sends.
type CombinationCase<'a> = (&'a str, &'a [Filter<'a>]);

/// A request carrying `filters` in `api`'s string-filter member.
fn filters_body(api: NodeApi, filters: &[Filter<'_>]) -> Value {
    let list: Vec<Value> = filters
        .iter()
        .map(|(key, values)| json!({ "Key": key, "Values": values }))
        .collect();
    let mut body = Map::new();
    body.insert(api.filters_member.to_string(), Value::Array(list));
    Value::Object(body)
}

/// A request carrying one filter in `api`'s legacy, operator-less filter member, whose
/// members are the lowercase `key` / `valueSet` pair both legacy lists model.
fn legacy_body(api: NodeApi, key: &str, values: &[&str]) -> Value {
    let mut body = Map::new();
    body.insert(
        api.legacy_member.to_string(),
        json!([{ "key": key, "valueSet": values }]),
    );
    Value::Object(body)
}

/// A node-id filter value that violates the modelled pattern is refused with the
/// constraint it violated, on both node APIs and through both of each one's filter
/// members.
///
/// The rows differ along every axis the pattern constrains: the prefix, the length of the
/// suffix, its character set, and its case.
#[tokio::test]
async fn a_malformed_node_id_is_refused_with_the_constraint_it_violated() {
    let router = seeded_router();
    let expected = expected_constraint_violation();

    let malformed: &[(&str, &str)] = &[
        ("no prefix at all", "not-an-instance-id"),
        ("the i- prefix with nothing after it", "i-"),
        ("the i- prefix and 3 characters", "i-bad"),
        ("the i- prefix and 9 characters", "i-0abc12345"),
        ("the i- prefix and 16 characters", "i-0123456789abcdef"),
        ("the i- prefix and 18 characters", "i-0123456789abcdef01"),
        ("a non-word character in the suffix", "i-0abc-234"),
        ("a trailing space", "i-0abc1234 "),
        ("an uppercase prefix", "I-0abc1234"),
        ("the mi- prefix and 3 characters", "mi-123"),
        ("the mi- prefix and 8 characters, not 17", "mi-01234567"),
        ("the mi- prefix and 16 characters", "mi-0123456789abcdef"),
    ];

    for api in BOTH {
        for (case, value) in malformed {
            for body in [
                filters_body(api, &[("InstanceIds", &[value])]),
                legacy_body(api, "InstanceIds", &[value]),
            ] {
                let case = format!("{} {case} ({value:?}) in {body}", api.operation);
                let (status, parsed) = call(&router, api.operation, &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {parsed}"
                );
                assert_eq!(
                    error_code(&parsed),
                    "InvalidInstanceId",
                    "case {case:?}: expected InvalidInstanceId, got {parsed}"
                );
                assert_eq!(
                    error_message(&parsed),
                    expected,
                    "case {case:?}: expected the violated constraint {expected:?}, got {:?}",
                    error_message(&parsed)
                );
            }
        }
    }
}

/// Every node-id form the modelled pattern admits is answered rather than refused, so the
/// constraint the refusal above reports is the one the service actually enforces.
#[tokio::test]
async fn every_node_id_form_the_pattern_admits_is_accepted() {
    let router = seeded_router();

    // One row per branch of the alternation, plus the `\w` character classes it admits
    // (digits, letters and the underscore).
    let well_formed: &[(&str, &str)] = &[
        ("i- and 8 characters", "i-01234567"),
        ("i- and 17 characters", SEEDED_NODE),
        ("i- and an underscore in the suffix", "i-0abc_234"),
        ("mi- and 17 characters", "mi-01234567890123456"),
        ("mi- and 17 word characters", "mi-abcdef_0123456789"),
    ];

    for api in BOTH {
        for (case, value) in well_formed {
            let body = filters_body(api, &[("InstanceIds", &[value])]);
            let case = format!("{} {case} ({value:?})", api.operation);
            let (status, parsed) = call(&router, api.operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {parsed}"
            );
            assert!(
                parsed[api.report_member].is_array(),
                "case {case:?}: expected a report in {}, got {parsed}",
                api.report_member
            );
        }
    }
}

/// A tag filter ANDed with a filter on a property of a node is refused as a
/// `ValidationException` on both node APIs: both keys are keys the operation accepts, so
/// what the request got wrong is the COMBINATION.
#[tokio::test]
async fn a_tag_filter_combined_with_a_property_filter_is_refused_as_a_validation_error() {
    let router = seeded_router();

    // Rows differ in which tag form is used, which property is filtered on, the order the
    // two kinds appear in, and how many filters the request carries.
    let refused: &[CombinationCase<'_>] = &[
        (
            "tag: then PlatformTypes",
            &[("tag:Purpose", &["Test"]), ("PlatformTypes", &["Windows"])],
        ),
        (
            "PlatformTypes then tag:",
            &[("PlatformTypes", &["Windows"]), ("tag:Purpose", &["Test"])],
        ),
        (
            "tag-key then PingStatus",
            &[("tag-key", &["Purpose"]), ("PingStatus", &["Online"])],
        ),
        (
            "tag: then InstanceIds",
            &[("tag:Purpose", &["Test"]), ("InstanceIds", &[SEEDED_NODE])],
        ),
        (
            "two tag filters and one property filter",
            &[
                ("tag:Purpose", &["Test"]),
                ("tag-key", &["Owner"]),
                ("ResourceType", &["EC2Instance"]),
            ],
        ),
        (
            "one tag filter and two property filters",
            &[
                ("ResourceType", &["EC2Instance"]),
                ("PingStatus", &["Online"]),
                ("tag:Owner", &["platform"]),
            ],
        ),
    ];

    for api in BOTH {
        for (case, filters) in refused {
            let body = filters_body(api, filters);
            let case = format!("{} {case}", api.operation);
            let (status, parsed) = call(&router, api.operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {parsed}"
            );
            let message = error_message(&parsed);
            assert_eq!(
                message, MIXED_FILTER_KINDS_REJECTION,
                "case {case:?}: expected the refusal {MIXED_FILTER_KINDS_REJECTION:?}, got \
                 {message:?}"
            );
        }
    }
}

/// Filters of ONE kind combine freely: two tag filters, and any number of property
/// filters, are answered — the rule is about which filter kinds may be ANDed, not a
/// blanket refusal of a request carrying two filters.
#[tokio::test]
async fn filters_of_one_kind_combine_freely() {
    let router = seeded_router();

    let accepted: &[CombinationCase<'_>] = &[
        ("one tag filter", &[("tag:Purpose", &["Test"])]),
        ("one property filter", &[("PlatformTypes", &["Linux"])]),
        (
            "two tag: filters",
            &[("tag:Purpose", &["Test"]), ("tag:Owner", &["platform"])],
        ),
        (
            "tag: and tag-key",
            &[("tag:Purpose", &["Test"]), ("tag-key", &["Owner"])],
        ),
        (
            "InstanceIds and PlatformTypes",
            &[("InstanceIds", &[SEEDED_NODE]), ("PlatformTypes", &["Linux"])],
        ),
        (
            "three property filters",
            &[
                ("InstanceIds", &[SEEDED_NODE]),
                ("PlatformTypes", &["Linux"]),
                ("PingStatus", &["Online"]),
            ],
        ),
    ];

    for api in BOTH {
        for (case, filters) in accepted {
            let body = filters_body(api, filters);
            let case = format!("{} {case}", api.operation);
            let (status, parsed) = call(&router, api.operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {parsed}"
            );
            let reported = parsed[api.report_member]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a report, got {parsed}"));
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected the one seeded node, got {parsed}"
            );
        }
    }
}

/// A key neither node API knows is refused as a validation of the request naming that
/// key, whatever it is combined with: whether a filter key is one the operation knows is
/// decided before the combination rule, so an unrecognised key is never reported as a
/// disallowed combination.
#[tokio::test]
async fn a_key_the_operation_does_not_know_is_refused_naming_that_key() {
    let router = seeded_router();

    // `tag:` names no tag, so it is not a tag filter key at all; the others are spellings
    // of keys that exist.
    let unknown: &[&str] = &["Nope", "InstanceId", "platformtypes", "Tag-Key", "tag:"];

    for api in BOTH {
        for key in unknown {
            for body in [
                filters_body(api, &[(key, &["x"])]),
                filters_body(api, &[(key, &["x"]), ("tag:Purpose", &["Test"])]),
                filters_body(api, &[("tag:Purpose", &["Test"]), (key, &["x"])]),
            ] {
                let case = format!("{} {key:?} in {body}", api.operation);
                let (status, parsed) = call(&router, api.operation, &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {parsed}"
                );
                assert_eq!(
                    error_code(&parsed),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {parsed}"
                );
                let message = error_message(&parsed);
                assert!(
                    message.contains(key),
                    "case {case:?}: the refusal must name the key the caller submitted, got \
                     {message:?}"
                );
                assert_ne!(
                    message, MIXED_FILTER_KINDS_REJECTION,
                    "case {case:?}: an unknown key is not a disallowed combination, got \
                     {message:?}"
                );
            }
        }
    }
}

/// The LEGACY filter list and the current one are ALTERNATIVES on both node APIs: a
/// request naming both is refused with the one sentence that states the rule, whichever
/// keys the two lists carry and however many filters each holds.
///
/// The rows differ along what makes the two lists legal on their own — the key each names,
/// how many filters it holds — so the refusal is about naming BOTH members and not about
/// any filter in them.
#[tokio::test]
async fn naming_both_filter_members_is_refused() {
    let router = seeded_router();

    /// The sentence both describes state, written out here independently of the service
    /// (`crate::state::LEGACY_AND_NEW_FILTERS_REJECTION`): it names neither member, so it
    /// is the same sentence for both operations.
    const BOTH_FILTER_MEMBERS: &str = "Cannot use both legacy and new filter simultaneously";

    // (case, the filters the legacy member carries, the filters the current member carries)
    let cases: &[(&str, &[Filter<'_>], &[Filter<'_>])] = &[
        (
            "one filter in each",
            &[("ResourceType", &["EC2Instance"])],
            &[("PingStatus", &["Online"])],
        ),
        (
            "the same key in both",
            &[("PingStatus", &["Online"])],
            &[("PingStatus", &["Online"])],
        ),
        (
            "two filters beside one",
            &[("ResourceType", &["EC2Instance"]), ("PingStatus", &["Online"])],
            &[("AgentVersion", &["3.0.1124.0"])],
        ),
        (
            "a node id beside a tag",
            &[("InstanceIds", &[SEEDED_NODE])],
            &[("tag:Purpose", &["Test"])],
        ),
    ];

    for api in BOTH {
        for (case, legacy, current) in cases {
            let mut body = Map::new();
            body.insert(
                api.legacy_member.to_string(),
                Value::Array(
                    legacy
                        .iter()
                        .map(|(key, values)| json!({ "key": key, "valueSet": values }))
                        .collect(),
                ),
            );
            body.insert(
                api.filters_member.to_string(),
                Value::Array(
                    current
                        .iter()
                        .map(|(key, values)| json!({ "Key": key, "Values": values }))
                        .collect(),
                ),
            );
            let body = Value::Object(body);
            let case = format!("{} {case} in {body}", api.operation);
            let (status, parsed) = call(&router, api.operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {parsed}"
            );
            assert_eq!(
                error_message(&parsed),
                BOTH_FILTER_MEMBERS,
                "case {case:?}: expected {BOTH_FILTER_MEMBERS:?}, got {:?}",
                error_message(&parsed)
            );
        }
    }
}
