// SPDX-License-Identifier: CC0-1.0

//! CloudFormation creates an `AWS::OpenSearchService::Domain` in Amazon OpenSearch Service,
//! so that service's own judgement of the declared cluster is what decides the stack.
//!
//! The rule under test: **a stack's resource is only real if the service that OWNS it
//! created it, and that service's refusal of the create FAILS the resource and rolls the
//! whole stack back.** A domain whose cluster names an instance type the requested engine
//! release does not offer is refused by Amazon OpenSearch Service with its own
//! `InvalidTypeException`, so the declaring resource fails carrying that refusal — as the
//! type's own resource handler words it — every resource the deployment had already made
//! (the topic the domain `DependsOn`) is deleted again, and the stack settles in
//! `ROLLBACK_COMPLETE`. A pairing that service DOES offer deploys, and the domain reads back
//! through `DescribeDomain` on the release and the type the template declared, with `Ref`
//! and `Fn::GetAtt` resolving to the identifiers that service minted.
//!
//! The rows span what the rule generalises over rather than one recorded template: both
//! engine lines, each instance family the published catalog gates on a release, the TIER of
//! the cluster that names the type (the data nodes, the dedicated masters, the warm tier),
//! and both directions of the rule — a release that does not offer the type and one that
//! does. Which pairings those are is the owning service's own catalogue, read per row from
//! `ListInstanceTypeDetails` rather than restated here, so a row's premise cannot drift from
//! the service it is asserted against.
//!
//! This suite needs THREE services on one engine — CloudFormation reaches Amazon OpenSearch
//! Service and Amazon SNS through the in-process callback — which is why it lives here
//! rather than in any one service's crate.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-west-2";

/// How long one poll of a stack waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How many times a stack is polled before the suite gives up on it reaching a terminal
/// status. Generous next to the spans that have to fall due — the stack's own transition and
/// the domain's provisioning window, each half a second on this emulator's compressed clock
/// — so a slow machine does not fail the suite.
const POLL_LIMIT: usize = 40;

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon SNS operation (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// Call one Amazon OpenSearch Service operation over its `restJson1` wire (signing name
/// `es`), answered with the status and the parsed document.
async fn opensearch(
    router: &MultiServiceRouter,
    method: &'static str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("es.{REGION}.amazonaws.com"))
        .header("authorization", authorization("es"));
    let payload = match body {
        Some(document) => {
            builder = builder.header("content-type", "application/json");
            Bytes::from(document.to_string())
        }
        None => Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The `DomainStatus` Amazon OpenSearch Service reports for `name`, with the status it
/// answered: `DescribeDomain`, `GET /2021-01-01/opensearch/domain/{DomainName}`.
async fn describe_domain(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let (status, body) = opensearch(
        router,
        "GET",
        &format!("/2021-01-01/opensearch/domain/{name}"),
        None,
    )
    .await;
    let reported = body
        .get("DomainStatus")
        .cloned()
        .unwrap_or_else(|| body.clone());
    (status, reported)
}

/// The tags Amazon OpenSearch Service holds for one resource, as the map a client reads them
/// as: `ListTags`, `GET /2021-01-01/tags?arn=<arn>`.
async fn domain_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = opensearch(
        router,
        "GET",
        &format!("/2021-01-01/tags?arn={}", form_encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTags({arn}), got {status} with {body}"
    );
    body.get("TagList")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag.get("Key").and_then(Value::as_str)?.to_string(),
                        tag.get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every instance type the owning service publishes for one engine release, read to the end
/// of its pagination: `ListInstanceTypeDetails`. This is what each row's premise is checked
/// against, so the suite never asserts the gates against themselves.
async fn offered_instance_types(router: &MultiServiceRouter, engine_version: &str) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let path = match token.as_deref() {
            None => format!("/2021-01-01/opensearch/instanceTypeDetails/{engine_version}"),
            Some(next) => format!(
                "/2021-01-01/opensearch/instanceTypeDetails/{engine_version}?nextToken={next}"
            ),
        };
        let (status, body) = opensearch(router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case ListInstanceTypeDetails({engine_version}): expected 200, got {status} with \
             {body}"
        );
        types.extend(
            body.get("InstanceTypeDetails")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|entry| entry.get("InstanceType").and_then(Value::as_str))
                .map(str::to_string),
        );
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return types;
        }
    }
}

/// The text of the first `<tag>` of an XML answer, or the empty string.
fn xml_text(xml: &str, tag: &str) -> String {
    opt_element(xml, tag).unwrap_or_default()
}

/// The awsQuery error `<Code>` of a refusal, or `""` on a success envelope.
fn error_code(xml: &str) -> String {
    xml_text(xml, "Code")
}

/// Whether a stack status is one a deployment stops in, so polling it again tells a client
/// nothing new.
fn is_terminal(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack reaches a terminal status, and answer the last body
/// it was described with. Nothing sleeps inside a handler: what each poll collects is what
/// the clock already owed.
async fn settle_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLL_LIMIT {
        tokio::time::sleep(POLL_INTERVAL).await;
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeStacks, got {status} with {body}"
        );
        if is_terminal(&xml_text(&body, "StackStatus")) {
            return body;
        }
        last = body;
    }
    panic!("case {case:?}: the stack never reached a terminal status, last seen {last}");
}

/// The ARN a topic named `name` carries in the account this emulator mints ARNs for, read off
/// the topic SNS created rather than rebuilt here.
async fn topic_arn(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = sns(router, "ListTopics", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTopics, got {status} with {body}"
    );
    elements(&body, "TopicArn")
        .into_iter()
        .find(|arn| arn.ends_with(&format!(":{name}")))
        .unwrap_or_default()
}

/// One row: an engine release, the cluster a template declares on it, and what the owning
/// service's catalogue says about the pairing.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the template names, inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and
    /// 3..28 characters.
    domain: &'static str,
    /// The topic the template declares FIRST, which the domain `DependsOn`: what a rollback
    /// has to take away again.
    topic: &'static str,
    engine_version: &'static str,
    /// The `ClusterConfig` the template declares, as the template spells it.
    cluster: &'static str,
    /// The instance type the row's cluster provisions a node on, which the engine release is
    /// judged against: the one the refusal must name, for a row the release does not offer.
    instance_type: &'static str,
    /// Whether the owning service offers the pairing — checked against its published
    /// catalogue before the row is deployed.
    offered: bool,
}

/// What the owning service answers a DIRECT create of the row's cluster with: the HTTP
/// status and the message, which is what the stack's reason has to be built from.
///
/// Asked under a name of its own, so the direct call neither collides with the domain the
/// stack declares nor leaves one behind.
async fn direct_refusal(router: &MultiServiceRouter, row: &Row) -> (u16, String) {
    let cluster: Value =
        serde_json::from_str(row.cluster).expect("the row's cluster is a JSON object");
    let (status, body) = opensearch(
        router,
        "POST",
        "/2021-01-01/opensearch/domain",
        Some(serde_json::json!({
            "DomainName": format!("direct-{}", row.domain),
            "EngineVersion": row.engine_version,
            "ClusterConfig": cluster,
        })),
    )
    .await;
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, message)
}

/// The message the resource's handler reports for a refusal the owning service answered
/// directly with `message` and `status`: that service's own sentence, behind the opener the
/// handler's invalid-request class carries and in front of the diagnostic the SDK call
/// raises. Restated here rather than imported, so a change to any part of it fails this
/// suite instead of being asserted against itself.
fn expected_refusal(status: u16, message: &str) -> String {
    format!(
        "Invalid request provided: {message} \
         (Service: OpenSearch, Status Code: {status}, Request ID: "
    )
}

/// The `HandlerErrorCode` class a provider's validation refusal is reported under, which is
/// what a client reads to decide whether a failed resource is worth retrying.
const INVALID_REQUEST: &str = "HandlerErrorCode: InvalidRequest";

const ROWS: &[Row] = &[
    Row {
        // The OpenSearch-optimized family is offered from OpenSearch 2.11, so an earlier
        // 2.x release does not run it.
        case: "an OpenSearch-optimized data node on an earlier 2.x release",
        stack: "sim-osrb-or1-early",
        domain: "sim-osrb-or1-early",
        topic: "sim-osrb-or1-early-alerts",
        engine_version: "OpenSearch_2.5",
        cluster: r#"{"InstanceType":"or1.medium.search","InstanceCount":2}"#,
        instance_type: "or1.medium.search",
        offered: false,
    },
    Row {
        // The same instance type on the newest supported 2.x release: the refusal is
        // engine-scoped, not a blanket rejection of the type.
        case: "the same data node on the newest supported 2.x release",
        stack: "sim-osrb-or1-newest",
        domain: "sim-osrb-or1-newest",
        topic: "sim-osrb-or1-newest-alerts",
        engine_version: "OpenSearch_2.19",
        cluster: r#"{"InstanceType":"or1.medium.search","InstanceCount":2}"#,
        instance_type: "or1.medium.search",
        offered: true,
    },
    Row {
        // A different family with a different gate, on the OTHER engine line: the Graviton
        // families are offered from Elasticsearch 7.9.
        case: "a Graviton data node on an Elasticsearch release before that family",
        stack: "sim-osrb-graviton-early",
        domain: "sim-osrb-grav-early",
        topic: "sim-osrb-graviton-early-alerts",
        engine_version: "Elasticsearch_7.1",
        cluster: r#"{"InstanceType":"r6g.large.search","InstanceCount":2}"#,
        instance_type: "r6g.large.search",
        offered: false,
    },
    Row {
        case: "a Graviton data node on an OpenSearch release",
        stack: "sim-osrb-graviton-ok",
        domain: "sim-osrb-grav-ok",
        topic: "sim-osrb-graviton-ok-alerts",
        engine_version: "OpenSearch_1.0",
        cluster: r#"{"InstanceType":"r6g.large.search","InstanceCount":2}"#,
        instance_type: "r6g.large.search",
        offered: true,
    },
    Row {
        // The type is not the data nodes' here: every node of the cluster runs the domain's
        // one release, so the dedicated masters are scoped to it too.
        case: "a dedicated master type the release does not offer",
        stack: "sim-osrb-master",
        domain: "sim-osrb-master",
        topic: "sim-osrb-master-alerts",
        engine_version: "OpenSearch_2.9",
        cluster: r#"{"InstanceType":"m5.large.search","InstanceCount":2,
             "DedicatedMasterEnabled":true,"DedicatedMasterType":"or1.large.search",
             "DedicatedMasterCount":3}"#,
        instance_type: "or1.large.search",
        offered: false,
    },
    Row {
        // And the warm tier, whose member names a type of its own enum. UltraWarm is
        // offered from Elasticsearch 6.8, and rests on dedicated master nodes.
        case: "an UltraWarm tier on an Elasticsearch release before that family",
        stack: "sim-osrb-warm",
        domain: "sim-osrb-warm",
        topic: "sim-osrb-warm-alerts",
        engine_version: "Elasticsearch_6.3",
        cluster: r#"{"InstanceType":"m5.large.search","InstanceCount":2,
             "DedicatedMasterEnabled":true,"DedicatedMasterType":"m5.large.search",
             "DedicatedMasterCount":3,"WarmEnabled":true,
             "WarmType":"ultrawarm1.medium.search","WarmCount":2}"#,
        instance_type: "ultrawarm1.medium.search",
        offered: false,
    },
];

/// The template one row deploys: the topic the domain depends on, declared first, then the
/// domain, with the identifiers of the domain as outputs.
fn template(row: &Row) -> String {
    format!(
        r#"{{"Resources":{{
             "Alerts":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{topic}"}}}},
             "Domain":{{"Type":"AWS::OpenSearchService::Domain","DependsOn":"Alerts",
               "Properties":{{"DomainName":"{domain}","EngineVersion":"{engine}",
                 "ClusterConfig":{cluster},
                 "Tags":[{{"Key":"tier","Value":"search"}}]}}}}}},
           "Outputs":{{
             "DomainRef":{{"Value":{{"Ref":"Domain"}}}},
             "DomainArn":{{"Value":{{"Fn::GetAtt":["Domain","Arn"]}}}},
             "DomainId":{{"Value":{{"Fn::GetAtt":["Domain","Id"]}}}},
             "DomainEndpoint":{{"Value":{{"Fn::GetAtt":["Domain","DomainEndpoint"]}}}}}}}}"#,
        topic = row.topic,
        domain = row.domain,
        engine = row.engine_version,
        cluster = row.cluster,
    )
}

/// Every `ResourceStatusReason` the stack's events carry, which is where the owning
/// service's own refusal of one resource is reported.
async fn resource_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&events, "ResourceStatusReason")
}

#[tokio::test]
async fn the_published_catalog_agrees_about_every_row() {
    let router = build_router();
    for row in ROWS {
        let offered = offered_instance_types(&router, row.engine_version).await;
        let holds = offered.contains(&row.instance_type.to_string());
        assert_eq!(
            holds, row.offered,
            "case {:?}: the row's premise is that {} {} {}, but the published catalog says \
             otherwise: {offered:?}",
            row.case,
            row.engine_version,
            if row.offered { "offers" } else { "does not offer" },
            row.instance_type,
        );
    }
}

#[tokio::test]
async fn a_domain_the_owning_service_refuses_rolls_the_whole_stack_back() {
    let router = build_router();

    for row in ROWS.iter().filter(|row| !row.offered) {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "ROLLBACK_COMPLETE",
            "case {case:?}: a Domain that fails to create must roll the whole stack back, got \
             {described}"
        );
        // The STACK's reason summarises which resource failed; the service's own refusal is
        // reported on that resource's own event, with the class that refusal falls in.
        assert_eq!(
            xml_text(&described, "StackStatusReason"),
            resource_failure_summary(failed_action::CREATE, ["Domain"]),
            "case {case:?}: expected the stack reason to summarise what failed, got {described}"
        );
        let reasons = resource_reasons(&router, row.stack).await;
        // The refusal the resource reports is the one the owning service answers the same
        // cluster with when a client asks it directly — behind the handler's own opener and
        // in front of its SDK diagnostic, which names that service and the very status it
        // answered.
        let (direct_status, direct_message) = direct_refusal(&router, row).await;
        assert!(
            direct_message.contains(row.instance_type),
            "case {case:?}: expected the service's own refusal to name {}, got {direct_status} \
             with {direct_message:?}",
            row.instance_type
        );
        let expected = expected_refusal(direct_status, &direct_message);
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains(&expected) && reason.contains(INVALID_REQUEST)),
            "case {case:?}: expected a resource event carrying {expected:?} and \
             {INVALID_REQUEST:?}, got {reasons:?}"
        );

        // Nothing survives: the domain was never created, and the topic the domain
        // `DependsOn` — created first — is deleted again by the rollback.
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 409,
            "case {case:?}: the refused domain must not exist, got {status} with {reported}"
        );
        assert_eq!(
            topic_arn(&router, case, row.topic).await,
            String::new(),
            "case {case:?}: the rollback must delete the topic the domain depended on"
        );
    }
}

#[tokio::test]
async fn a_domain_the_owning_service_offers_is_created_and_reads_back() {
    let router = build_router();

    for row in ROWS.iter().filter(|row| row.offered) {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: a pairing the service offers must deploy, got {described}"
        );

        // Round trip: the domain the stack reports is the one that service holds, on the
        // release and the type the template declared.
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created domain to read back, got {status} with \
             {reported}"
        );
        assert_eq!(
            reported.get("EngineVersion").and_then(Value::as_str),
            Some(row.engine_version),
            "case {case:?}: expected the stored domain to run the declared release, got \
             {reported}"
        );
        assert_eq!(
            reported
                .get("ClusterConfig")
                .and_then(|cluster| cluster.get("InstanceType"))
                .and_then(Value::as_str),
            Some(row.instance_type),
            "case {case:?}: expected the stored cluster to run the declared type, got {reported}"
        );
        assert_eq!(
            reported.get("Processing").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a stack reported complete must not still be provisioning its \
             domain, got {reported}"
        );

        // `Ref` returns the domain name, and each `Fn::GetAtt` the identifier the OWNING
        // service minted — never one CloudFormation's naming rules computed.
        let outputs: BTreeMap<String, String> = outputs_of(&described);
        for (attribute, member) in [
            ("DomainRef", "DomainName"),
            ("DomainArn", "ARN"),
            ("DomainId", "DomainId"),
            ("DomainEndpoint", "Endpoint"),
        ] {
            let minted = reported
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !minted.is_empty(),
                "case {case:?}: the owning service must report {member}, got {reported}"
            );
            assert_eq!(
                outputs.get(attribute).map(String::as_str),
                Some(minted),
                "case {case:?}: expected {attribute} to be the service's own {member}, got \
                 {outputs:?}"
            );
        }
        assert_eq!(
            outputs.get("DomainRef").map(String::as_str),
            Some(row.domain),
            "case {case:?}: Ref returns the domain name, got {outputs:?}"
        );

        // The domain carries the tags the declaration asked for beside the ones
        // CloudFormation records on every resource it provisions, which only a create that
        // sent them as the owning service's own tag list could leave.
        let arn = outputs
            .get("DomainArn")
            .map(String::as_str)
            .unwrap_or_default();
        let tags = domain_tags(&router, case, arn).await;
        assert_eq!(
            tags.get("tier").map(String::as_str),
            Some("search"),
            "case {case:?}: expected the declared tag on the created domain, got {tags:?}"
        );
        assert_eq!(
            tags.get("aws:cloudformation:stack-name").map(String::as_str),
            Some(row.stack),
            "case {case:?}: expected CloudFormation's own tags on the created domain, got \
             {tags:?}"
        );

        // The topic the domain depended on is there too, and deleting the stack takes both
        // resources away again.
        assert_ne!(
            topic_arn(&router, case, row.topic).await,
            String::new(),
            "case {case:?}: the topic the domain depended on must exist"
        );
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteStack, got {status} with {body}"
        );
        for _ in 0..POLL_LIMIT {
            tokio::time::sleep(POLL_INTERVAL).await;
            let (_, reported) = describe_domain(&router, row.domain).await;
            if reported.get("DomainName").is_none() {
                break;
            }
        }
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 409,
            "case {case:?}: deleting the stack must delete the domain, got {status} with \
             {reported}"
        );
        let (status, answer) = sns(
            &router,
            "GetTopicAttributes",
            &[("TopicArn", &format!("arn:aws:sns:{REGION}:000000000000:{}", row.topic))],
        )
        .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "NotFound"),
            "case {case:?}: deleting the stack must delete the topic, got {status} with {answer}"
        );
    }
}
