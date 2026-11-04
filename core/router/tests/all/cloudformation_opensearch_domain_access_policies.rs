// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose `AWS::OpenSearchService::Domain` declares a policy document
//! deploys, and the domain the owning service holds carries that document as its text.
//!
//! The rule under test: **a property a resource provisioner forwards to an API member the
//! owning service's model types as a STRING is sent as that member's TEXT — a document the
//! template wrote inline, with its intrinsics already resolved, as the compact JSON of that
//! document, and one the template wrote as text unchanged — so the request matches the
//! member's modelled type.** `AWS::OpenSearchService::Domain`'s `AccessPolicies` is such a
//! property: `CreateDomain$AccessPolicies` targets `PolicyDocument`, a string
//! (`smithy_query(query="operation", operation="CreateDomain")`), while a template declares
//! an access policy the way every policy document is declared — as the policy itself, with
//! `Fn::GetAtt`/`Fn::Sub` inside it. Sending the object as an object is a request no such
//! member accepts, and the resource then fails for a reason that is about the provisioner
//! rather than about the template.
//!
//! The rows differ along what the rule generalises over: the FORM the template writes the
//! document in (an inline object, the quoted text of one, an object built by an intrinsic),
//! the number of statements it carries, the JSON types inside it (nested maps, arrays,
//! booleans, numbers), whether an intrinsic inside it resolves to one role or to two, and a
//! declaration carrying no such property at all. The recorded case — two roles swapped
//! between `Allow` and `Deny` — is one row of that table.
//!
//! The last test pins the other half: a DIRECT `CreateDomain` carrying that member as an
//! object is still refused, because the member is modelled a string. The deployment works
//! because the provisioner sends text, not because the service grew tolerant of objects.
//!
//! Three services have to agree for the rule to hold (CloudFormation makes the calls, IAM
//! mints the role ARNs the document names, Amazon OpenSearch Service serves the create), so
//! the suite lives here rather than in any one service's crate.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-west-2";

/// How long one poll of a stack waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How many times a stack is polled before the suite gives up on it reaching a terminal
/// status. Generous next to the spans that have to fall due — the stack's own transition and
/// the domain's provisioning window — so a slow machine does not fail the suite.
const POLL_LIMIT: usize = 40;

/// The engine release and cluster every row deploys: one the owning service's catalogue
/// offers, so the only thing a row can fail on is the property document under test.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The marker a row's expected document carries where the ARN IAM minted for the first role
/// belongs, substituted from the stack's own outputs before the comparison.
const ROLE_A: &str = "@ROLE_A@";

/// The same, for the second role.
const ROLE_B: &str = "@ROLE_B@";

/// And for the ARN the owning service minted for the domain, which a row's `Fn::Sub`
/// resource is built from.
const DOMAIN_ARN: &str = "@DOMAIN_ARN@";

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
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
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

/// Call one CloudFormation operation over its awsQuery wire (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
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

/// The text of the first `<tag>` of an XML answer, or the empty string.
fn xml_text(xml: &str, tag: &str) -> String {
    opt_element(xml, tag).unwrap_or_default()
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

/// Every `ResourceStatusReason` the stack's events carry, which is where a resource the
/// owning service refused reports why.
async fn resource_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    crate::query_wire::elements(&events, "ResourceStatusReason")
}

/// One row: the `AccessPolicies` property as a template writes it, and the document the
/// owning service must end up holding for it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the template names, inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and
    /// 3..28 characters.
    domain: &'static str,
    /// The property as the template spells it — JSON pasted into the template body — or the
    /// empty string for a declaration that carries no such property at all.
    declared: &'static str,
    /// The document the member must carry once the property is resolved and sent, with
    /// [`ROLE_A`], [`ROLE_B`] and [`DOMAIN_ARN`] standing for the identifiers the owning
    /// services mint; `None` for a row that declares no policy.
    expected: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded form: a policy OBJECT whose statements name two role ARNs through
        // `Fn::GetAtt` and whose resource is built with `Fn::Sub`, one allowed and one
        // denied.
        case: "an object policy allowing one role and denying another",
        stack: "sim-osap-swap",
        domain: "sim-osap-swap",
        declared: r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Principal":{"AWS":{"Fn::GetAtt":["RoleA","Arn"]}},
             "Action":["es:ESHttpGet","es:ESHttpHead","es:ESHttpPost","es:ESHttpPut"],
             "Resource":{"Fn::Sub":"arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/sim-osap-swap/*"}},
            {"Effect":"Deny","Principal":{"AWS":{"Fn::GetAtt":["RoleB","Arn"]}},
             "Action":["es:ESHttpGet","es:ESHttpHead","es:ESHttpPost","es:ESHttpPut"],
             "Resource":{"Fn::Sub":"arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/sim-osap-swap/*"}}]}"#,
        expected: Some(
            r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Principal":{"AWS":"@ROLE_A@"},
             "Action":["es:ESHttpGet","es:ESHttpHead","es:ESHttpPost","es:ESHttpPut"],
             "Resource":"@DOMAIN_ARN@/*"},
            {"Effect":"Deny","Principal":{"AWS":"@ROLE_B@"},
             "Action":["es:ESHttpGet","es:ESHttpHead","es:ESHttpPost","es:ESHttpPut"],
             "Resource":"@DOMAIN_ARN@/*"}]}"#,
        ),
    },
    Row {
        // The same rule with the effects the other way round: swapping them changes the
        // document, never how it is carried.
        case: "the same object policy with the two effects swapped",
        stack: "sim-osap-swapped",
        domain: "sim-osap-swapped",
        declared: r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Deny","Principal":{"AWS":{"Fn::GetAtt":["RoleA","Arn"]}},
             "Action":"es:ESHttpGet","Resource":"*"},
            {"Effect":"Allow","Principal":{"AWS":{"Fn::GetAtt":["RoleB","Arn"]}},
             "Action":"es:ESHttpGet","Resource":"*"}]}"#,
        expected: Some(
            r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Deny","Principal":{"AWS":"@ROLE_A@"},
             "Action":"es:ESHttpGet","Resource":"*"},
            {"Effect":"Allow","Principal":{"AWS":"@ROLE_B@"},
             "Action":"es:ESHttpGet","Resource":"*"}]}"#,
        ),
    },
    Row {
        // One statement, with no intrinsic anywhere in it: the smallest document of the
        // class.
        case: "an object policy with a single literal statement",
        stack: "sim-osap-single",
        domain: "sim-osap-single",
        declared: r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Principal":"*","Action":"es:ESHttpGet","Resource":"*"}]}"#,
        expected: Some(
            r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Principal":"*","Action":"es:ESHttpGet","Resource":"*"}]}"#,
        ),
    },
    Row {
        // Every JSON type a document can carry inside it — a nested map, an array, a
        // boolean and a number — so the rendering is the document's own JSON and not a
        // flattening of it.
        case: "an object policy carrying a nested condition, a boolean and a number",
        stack: "sim-osap-condition",
        domain: "sim-osap-condition",
        declared: r#"{"Version":"2012-10-17","Statement":[
            {"Sid":"OnlyOverTls","Effect":"Deny","Principal":{"AWS":"*"},
             "Action":["es:ESHttpGet","es:ESHttpPut"],"Resource":"*",
             "Condition":{"Bool":{"aws:SecureTransport":false},
                          "NumericLessThan":{"aws:MultiFactorAuthAge":300}}}]}"#,
        expected: Some(
            r#"{"Version":"2012-10-17","Statement":[
            {"Sid":"OnlyOverTls","Effect":"Deny","Principal":{"AWS":"*"},
             "Action":["es:ESHttpGet","es:ESHttpPut"],"Resource":"*",
             "Condition":{"Bool":{"aws:SecureTransport":false},
                          "NumericLessThan":{"aws:MultiFactorAuthAge":300}}}]}"#,
        ),
    },
    Row {
        // The other FORM the property is written in: the quoted TEXT of a document, which
        // is already what the member takes and passes through as it is.
        case: "a policy written as the quoted text of a document",
        stack: "sim-osap-text",
        domain: "sim-osap-text",
        declared: r#""{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"*\"},\"Action\":\"es:ESHttpHead\",\"Resource\":\"*\"}]}""#,
        expected: Some(
            r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Principal":{"AWS":"*"},"Action":"es:ESHttpHead",
             "Resource":"*"}]}"#,
        ),
    },
    Row {
        // Text again, but produced BY an intrinsic: what reaches the member is the string
        // the intrinsic resolved to, so a text-valued property is not re-rendered.
        case: "a policy whose text an intrinsic builds",
        stack: "sim-osap-substituted",
        domain: "sim-osap-substituted",
        declared: r#"{"Fn::Sub":"{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"*\"},\"Action\":\"es:ESHttpGet\",\"Resource\":\"arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/sim-osap-substituted/*\"}]}"}"#,
        expected: Some(
            r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Principal":{"AWS":"*"},"Action":"es:ESHttpGet",
             "Resource":"@DOMAIN_ARN@/*"}]}"#,
        ),
    },
    Row {
        // And a declaration that carries no such property: nothing is sent for it, so the
        // domain holds the access policy the SERVICE sets for a create that named none,
        // which is the empty document.
        case: "a domain declaring no access policy at all",
        stack: "sim-osap-none",
        domain: "sim-osap-none",
        declared: "",
        expected: None,
    },
];

/// The template one row deploys: two roles the policy can name, and the domain that
/// `DependsOn` both, with the identifiers of all three as outputs.
fn template(row: &Row) -> String {
    let access_policies = if row.declared.is_empty() {
        String::new()
    } else {
        format!(r#","AccessPolicies":{}"#, row.declared)
    };
    format!(
        r#"{{"Resources":{{
             "RoleA":{{"Type":"AWS::IAM::Role","Properties":{{
               "AssumeRolePolicyDocument":{{"Version":"2012-10-17","Statement":[
                 {{"Effect":"Allow","Principal":{{"Service":"es.amazonaws.com"}},
                   "Action":"sts:AssumeRole"}}]}}}}}},
             "RoleB":{{"Type":"AWS::IAM::Role","Properties":{{
               "AssumeRolePolicyDocument":{{"Version":"2012-10-17","Statement":[
                 {{"Effect":"Allow","Principal":{{"Service":"es.amazonaws.com"}},
                   "Action":"sts:AssumeRole"}}]}}}}}},
             "Domain":{{"Type":"AWS::OpenSearchService::Domain",
               "DependsOn":["RoleA","RoleB"],
               "Properties":{{"DomainName":"{domain}","EngineVersion":"{engine}",
                 "ClusterConfig":{{"InstanceType":"t3.small.search","InstanceCount":1,
                   "DedicatedMasterEnabled":false,"ZoneAwarenessEnabled":false}},
                 "EBSOptions":{{"EBSEnabled":true,"VolumeType":"gp3","VolumeSize":10}},
                 "DomainEndpointOptions":{{"EnforceHTTPS":true,
                   "TLSSecurityPolicy":"Policy-Min-TLS-1-2-2019-07"}},
                 "Tags":[{{"Key":"tier","Value":"search"}}]{access_policies}}}}}}},
           "Outputs":{{
             "DomainArn":{{"Value":{{"Fn::GetAtt":["Domain","Arn"]}}}},
             "DomainEndpoint":{{"Value":{{"Fn::GetAtt":["Domain","DomainEndpoint"]}}}},
             "RoleAArn":{{"Value":{{"Fn::GetAtt":["RoleA","Arn"]}}}},
             "RoleBArn":{{"Value":{{"Fn::GetAtt":["RoleB","Arn"]}}}}}}}}"#,
        domain = row.domain,
        engine = ENGINE_VERSION,
        access_policies = access_policies,
    )
}

/// A row's expected document, with the identifiers the owning services minted put in place
/// of the markers it carries.
fn expected_document(expected: &str, outputs: &BTreeMap<String, String>) -> Value {
    let filled = expected
        .replace(ROLE_A, outputs.get("RoleAArn").map_or("", String::as_str))
        .replace(ROLE_B, outputs.get("RoleBArn").map_or("", String::as_str))
        .replace(
            DOMAIN_ARN,
            outputs.get("DomainArn").map_or("", String::as_str),
        );
    serde_json::from_str(&filled).expect("the row's expected document is JSON")
}

/// Every row deploys, and the domain the owning service holds carries the row's document as
/// the TEXT its member is modelled with.
#[tokio::test]
async fn a_declared_policy_document_reaches_the_domain_as_the_text_of_that_document() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row)),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
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
            "case {case:?}: a declared policy document must not fail the resource, got \
             {described} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );

        // All four identifiers the template publishes are resolved, which only a deployment
        // that really created both roles and the domain can report.
        let outputs: BTreeMap<String, String> = outputs_of(&described);
        for key in ["DomainArn", "DomainEndpoint", "RoleAArn", "RoleBArn"] {
            assert!(
                outputs.get(key).is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected {key} to carry a resolved value, got {outputs:?}"
            );
        }

        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created domain to read back, got {status} with \
             {reported}"
        );
        let carried = reported.get("AccessPolicies");
        match row.expected {
            None => assert_eq!(
                carried.and_then(Value::as_str),
                Some(""),
                "case {case:?}: a domain declaring no policy holds the empty document the \
                 service sets for one, got {carried:?}"
            ),
            Some(expected) => {
                let text = carried.and_then(Value::as_str).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the member is modelled a string, so the domain must \
                         carry its text, got {carried:?}"
                    )
                });
                let parsed: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: expected the carried text to be the document's own \
                         JSON, got {text:?} ({error})"
                    )
                });
                assert_eq!(
                    parsed,
                    expected_document(expected, &outputs),
                    "case {case:?}: expected the resolved document, got {text:?}"
                );
            }
        }
    }
}

/// The member is modelled a STRING, and stays one: a client that sends the document as an
/// object is still refused, with the status the model declares for a client error.
///
/// This is why the rendering belongs to the provisioner. Were the service to start accepting
/// an object there, it would accept a request no AWS client can send.
#[tokio::test]
async fn the_create_takes_the_policy_member_as_text_and_refuses_an_object() {
    let router = build_router();

    let document = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "*"},
            "Action": "es:ESHttpGet",
            "Resource": "*",
        }],
    });
    // (case, the value the request carries for the member, the status it is answered with)
    let rows: [(&str, Value, u16); 2] = [
        (
            "the document as its text",
            Value::String(document.to_string()),
            200,
        ),
        ("the document as an object", document.clone(), 400),
    ];

    for (index, (case, declared, expected_status)) in rows.into_iter().enumerate() {
        let name = format!("sim-osap-direct-{index}");
        let (status, body) = opensearch(
            &router,
            "POST",
            "/2021-01-01/opensearch/domain",
            Some(serde_json::json!({
                "DomainName": name,
                "EngineVersion": ENGINE_VERSION,
                "ClusterConfig": {"InstanceType": "t3.small.search", "InstanceCount": 1},
                "AccessPolicies": declared,
            })),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status} from CreateDomain, got {status} with \
             {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                body.get("DomainStatus")
                    .and_then(|reported| reported.get("AccessPolicies"))
                    .and_then(Value::as_str),
                Some(document.to_string().as_str()),
                "case {case:?}: expected the created domain to carry the text it was sent, \
                 got {body}"
            );
        }
    }
}
