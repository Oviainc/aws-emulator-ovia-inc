// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack of AWS WAF resources whose rules REFERENCE one another deploys,
//! and the references it deployed are live in AWS WAF's own state.
//!
//! The rule under test: **where a resource type's property spells a member differently from
//! the member the owning service's model declares, the deployment sends that service's
//! spelling and reports the template's spelling back.** AWS WAF has two such members, and
//! both sit inside a rule's statement tree: the ARN a reference statement names another
//! entity by is `ARN` in the model (`IPSetReferenceStatement$ARN`,
//! `RegexPatternSetReferenceStatement$ARN`, `RuleGroupReferenceStatement$ARN`, each
//! `@required`) and `Arn` in a template, and a byte-match statement's search value is a blob
//! the wire carries base64-encoded while a template declares the bytes. So a rule that names
//! another entity reaches `CreateWebACL`/`CreateRuleGroup` under the member that call
//! requires — instead of being refused for a required member the call did not carry, which
//! rolls the whole stack back — and the configuration the stack reads back for drift carries
//! the spelling the declaration used.
//!
//! Several crates have to agree for that to hold (CloudFormation makes the calls and AWS WAF
//! serves them, resolves each reference when the rule is saved and keeps it from being
//! deleted), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: which of the three reference
//! statements carries the ARN, which spelling the template writes it in, how deeply the
//! statement is nested in the rule tree (a rule's own statement, one inside an
//! `AndStatement` beside the other differently-spelled member, one inside a
//! `NotStatement`), which kind of entity does the referencing (a web ACL and a rule group),
//! and which kind of entity is referenced (an IP set, a regex pattern set and a rule group).
//! Every expected value is computed from what the row declares plus the identifiers AWS WAF
//! minted, never read back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate publishes ONCE, in `query_wire`: a suite that
// writes its own percent-encoder or element reader drifts from the others.
use crate::query_wire::{elements, form_encode};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-west-2";

/// The namespace every entity in this suite lives in: these are resources put in front of
/// regional protected resources rather than CloudFront distributions.
const SCOPE: &str = "REGIONAL";

/// Long enough for the clock-owed spans the stack's deployment and its drift detection
/// come due after.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_200);

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(
        engine,
    ));
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

/// One answer, as this suite reads it: the status and the body as text and as JSON.
struct Answer {
    status: u16,
    text: String,
    body: Value,
}

/// Drive one request at `signing_name`'s wire.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    body: String,
    target: Option<String>,
) -> Answer {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, body }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    send(
        router,
        "cloudformation",
        "application/x-www-form-urlencoded",
        body,
        None,
    )
    .await
}

/// One CloudFormation call that must succeed.
async fn stack_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let answer = cloudformation(router, action, members).await;
    assert!(
        answer.status == 200,
        "case {case:?}: expected HTTP 200 from {action}, got {} with {}",
        answer.status,
        answer.text
    );
    answer.text
}

/// Call one WAFv2 operation over its awsJson1_1 wire.
async fn wafv2(router: &MultiServiceRouter, operation: &str, request: &Value) -> Answer {
    send(
        router,
        "wafv2",
        "application/x-amz-json-1.1",
        serde_json::to_string(request).expect("the request serializes"),
        Some(format!("AWSWAF_20190729.{operation}")),
    )
    .await
}

/// One WAFv2 call that must succeed.
async fn waf_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let answer = wafv2(router, operation, &request).await;
    assert!(
        answer.status == 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {}",
        answer.status,
        answer.text
    );
    answer.body
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Deploy `template` as a new stack, wait past the clock-owed spans and answer the
/// `DescribeStacks` body, having first established the stack reached `CREATE_COMPLETE` — a
/// deployment any resource's create was refused for rolls the whole stack back, so the
/// reasons its events carry are what the failure is reported with.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    stack_ok(
        router,
        case,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    tokio::time::sleep(PAST_THE_SPANS).await;
    let described = stack_ok(
        router,
        case,
        "DescribeStacks",
        &[("StackName", stack_name)],
    )
    .await;
    let status = elements(&described, "StackStatus");
    if status != vec!["CREATE_COMPLETE".to_string()] {
        let events = stack_ok(
            router,
            case,
            "DescribeStackEvents",
            &[("StackName", stack_name)],
        )
        .await;
        panic!(
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {status:?} with \
             resource statuses {:?} because {:?}",
            elements(&events, "ResourceStatus"),
            elements(&events, "ResourceStatusReason"),
        );
    }
    described
}

/// The declaration of one REGIONAL IP set, the entity an `IPSetReferenceStatement` names.
fn ip_set(name: &str, addresses: &[&str]) -> Value {
    json!({
        "Type": "AWS::WAFv2::IPSet",
        "Properties": {
            "Name": name,
            "Scope": SCOPE,
            "IPAddressVersion": "IPV4",
            "Addresses": addresses,
        }
    })
}

/// The declaration of one REGIONAL regex pattern set, the entity a
/// `RegexPatternSetReferenceStatement` names.
fn regex_pattern_set(name: &str) -> Value {
    json!({
        "Type": "AWS::WAFv2::RegexPatternSet",
        "Properties": {
            "Name": name,
            "Scope": SCOPE,
            "RegularExpressionList": [{"RegexString": "^/admin"}],
        }
    })
}

/// The declaration of one REGIONAL rule group, the entity a `RuleGroupReferenceStatement`
/// names. Its own one rule needs no reference of its own.
fn rule_group(name: &str, rules: Value) -> Value {
    json!({
        "Type": "AWS::WAFv2::RuleGroup",
        "Properties": {
            "Name": name,
            "Scope": SCOPE,
            "Capacity": 500,
            "Rules": rules,
            "VisibilityConfig": visibility(name),
        }
    })
}

/// The declaration of one REGIONAL web ACL that lets everything it does not match through,
/// carrying `rules`.
fn web_acl(name: &str, rules: Value) -> Value {
    json!({
        "Type": "AWS::WAFv2::WebACL",
        "Properties": {
            "Name": name,
            "Scope": SCOPE,
            "DefaultAction": {"Allow": {}},
            "Rules": rules,
            "VisibilityConfig": visibility(name),
        }
    })
}

/// The `VisibilityConfig` every web ACL, rule group and rule of either requires.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": false,
        "CloudWatchMetricsEnabled": false,
        "MetricName": metric,
    })
}

/// The `Fn::GetAtt <logical id>.Arn` of a resource of the same stack, which is the ARN AWS
/// WAF minted for it: the value a reference statement names the entity by.
fn referenced_arn(logical_id: &str) -> Value {
    json!({"Fn::GetAtt": [logical_id, "Arn"]})
}

/// The outputs every row publishes: the ARN the referenced entity carries, and the `Id` AWS
/// WAF assigned the REFERRING one, which is its `Ref` and what its own `Get…` takes.
fn outputs(referenced: &str, referrer: &str) -> Value {
    json!({
        "ReferencedArn": {"Value": referenced_arn(referenced)},
        "ReferrerId": {"Value": {"Ref": referrer}},
    })
}

/// One row: a template declaring a referenced entity and an entity whose rule references it,
/// and where that reference sits in the referring entity's own configuration.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: fn() -> String,
    /// The name the referring entity is declared under, which its `Get…` takes.
    referrer_name: &'static str,
    /// The call that reads the referring entity back, and the member it answers it under.
    get: &'static str,
    entity_member: &'static str,
    /// Where the reference statement sits in that answer.
    statement_pointer: &'static str,
    /// The logical id of the referring entity, whose drift status the row checks.
    referrer_logical_id: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        // The reported case: a web ACL whose one rule blocks the addresses of an IP set the
        // same stack declares, with the ARN in the spelling the resource type publishes.
        case: "an IP set reference in a web ACL rule, spelled Arn",
        stack_name: "wafv2-ref-ipset-arn",
        template: || {
            json!({
                "Resources": {
                    "Listed": ip_set("ref-ipset-arn-set", &["198.51.100.17/32"]),
                    "Acl": web_acl("ref-ipset-arn-acl", json!([{
                        "Name": "block-listed",
                        "Priority": 0,
                        "Action": {"Block": {}},
                        "Statement": {
                            "IPSetReferenceStatement": {"Arn": referenced_arn("Listed")}
                        },
                        "VisibilityConfig": visibility("block-listed"),
                    }])),
                },
                "Outputs": outputs("Listed", "Acl"),
            })
            .to_string()
        },
        referrer_name: "ref-ipset-arn-acl",
        get: "GetWebACL",
        entity_member: "WebACL",
        statement_pointer: "/Rules/0/Statement/IPSetReferenceStatement",
        referrer_logical_id: "Acl",
    },
    Row {
        // The same reference written in the API's OWN spelling: a declaration already in the
        // member the call requires is carried through untouched.
        case: "an IP set reference in a web ACL rule, spelled as the model's own ARN",
        stack_name: "wafv2-ref-ipset-upper",
        template: || {
            json!({
                "Resources": {
                    "Listed": ip_set("ref-upper-set", &["203.0.113.0/24", "198.51.100.9/32"]),
                    "Acl": web_acl("ref-upper-acl", json!([{
                        "Name": "count-listed",
                        "Priority": 7,
                        "Action": {"Count": {}},
                        "Statement": {
                            "IPSetReferenceStatement": {"ARN": referenced_arn("Listed")}
                        },
                        "VisibilityConfig": visibility("count-listed"),
                    }])),
                },
                "Outputs": outputs("Listed", "Acl"),
            })
            .to_string()
        },
        referrer_name: "ref-upper-acl",
        get: "GetWebACL",
        entity_member: "WebACL",
        statement_pointer: "/Rules/0/Statement/IPSetReferenceStatement",
        referrer_logical_id: "Acl",
    },
    Row {
        // A regex pattern set reference NESTED inside an `AndStatement`, beside the other
        // member the two sides spell differently (a byte-match statement's search value), so
        // one walk has to put both in the API's spelling.
        case: "a regex pattern set reference nested in an AndStatement beside a byte match",
        stack_name: "wafv2-ref-regex-nested",
        template: || {
            json!({
                "Resources": {
                    "Patterns": regex_pattern_set("ref-regex-set"),
                    "Acl": web_acl("ref-regex-acl", json!([{
                        "Name": "block-admin-paths",
                        "Priority": 1,
                        "Action": {"Block": {}},
                        "Statement": {
                            "AndStatement": {
                                "Statements": [
                                    {"ByteMatchStatement": {
                                        "SearchString": "/admin",
                                        "FieldToMatch": {"UriPath": {}},
                                        "PositionalConstraint": "STARTS_WITH",
                                        "TextTransformations": [
                                            {"Priority": 0, "Type": "NONE"}
                                        ],
                                    }},
                                    {"RegexPatternSetReferenceStatement": {
                                        "Arn": referenced_arn("Patterns"),
                                        "FieldToMatch": {"UriPath": {}},
                                        "TextTransformations": [
                                            {"Priority": 0, "Type": "NONE"}
                                        ],
                                    }},
                                ]
                            }
                        },
                        "VisibilityConfig": visibility("block-admin-paths"),
                    }])),
                },
                "Outputs": outputs("Patterns", "Acl"),
            })
            .to_string()
        },
        referrer_name: "ref-regex-acl",
        get: "GetWebACL",
        entity_member: "WebACL",
        statement_pointer:
            "/Rules/0/Statement/AndStatement/Statements/1/RegexPatternSetReferenceStatement",
        referrer_logical_id: "Acl",
    },
    Row {
        // A RULE GROUP reference, the one of the three that carries an override action
        // instead of a rule action, with an excluded rule naming a rule of the referenced
        // group — so the statement's other members travel beside the renamed one.
        case: "a rule group reference in a web ACL rule, with an override action",
        stack_name: "wafv2-ref-rule-group",
        template: || {
            json!({
                "Resources": {
                    "Group": rule_group("ref-group", json!([{
                        "Name": "group-rule",
                        "Priority": 0,
                        "Action": {"Allow": {}},
                        "Statement": {
                            "GeoMatchStatement": {"CountryCodes": ["SE"]}
                        },
                        "VisibilityConfig": visibility("group-rule"),
                    }])),
                    "Acl": web_acl("ref-group-acl", json!([{
                        "Name": "run-group",
                        "Priority": 2,
                        "OverrideAction": {"None": {}},
                        "Statement": {
                            "RuleGroupReferenceStatement": {
                                "Arn": referenced_arn("Group"),
                                "ExcludedRules": [{"Name": "group-rule"}],
                            }
                        },
                        "VisibilityConfig": visibility("run-group"),
                    }])),
                },
                "Outputs": outputs("Group", "Acl"),
            })
            .to_string()
        },
        referrer_name: "ref-group-acl",
        get: "GetWebACL",
        entity_member: "WebACL",
        statement_pointer: "/Rules/0/Statement/RuleGroupReferenceStatement",
        referrer_logical_id: "Acl",
    },
    Row {
        // The referring entity is a RULE GROUP rather than a web ACL, and the reference sits
        // inside a `NotStatement`: the rule is the type's, not one type's.
        case: "an IP set reference nested in a NotStatement of a rule group's own rule",
        stack_name: "wafv2-ref-group-not",
        template: || {
            json!({
                "Resources": {
                    "Allowed": ip_set("ref-not-set", &["192.0.2.0/24"]),
                    "Group": rule_group("ref-not-group", json!([{
                        "Name": "block-everyone-else",
                        "Priority": 0,
                        "Action": {"Block": {}},
                        "Statement": {
                            "NotStatement": {
                                "Statement": {
                                    "IPSetReferenceStatement": {
                                        "Arn": referenced_arn("Allowed")
                                    }
                                }
                            }
                        },
                        "VisibilityConfig": visibility("block-everyone-else"),
                    }])),
                },
                "Outputs": outputs("Allowed", "Group"),
            })
            .to_string()
        },
        referrer_name: "ref-not-group",
        get: "GetRuleGroup",
        entity_member: "RuleGroup",
        statement_pointer: "/Rules/0/Statement/NotStatement/Statement/IPSetReferenceStatement",
        referrer_logical_id: "Group",
    },
];

#[tokio::test]
async fn a_rule_that_references_another_entity_deploys_and_is_live_in_waf() {
    for row in ROWS {
        let router = build_router();
        let case = row.case;
        let described = create_and_settle(&router, case, row.stack_name, &(row.template)()).await;
        let published = outputs_of(&described);
        let referenced = published
            .get("ReferencedArn")
            .cloned()
            .unwrap_or_default();
        let referrer_id = published.get("ReferrerId").cloned().unwrap_or_default();
        assert!(
            !referenced.is_empty() && !referrer_id.is_empty(),
            "case {case:?}: expected the stack to publish both identifiers, got {published:?}"
        );

        // The referring entity as AWS WAF itself reports it: the reference is under the
        // member the model declares, carrying the ARN that service minted for the referenced
        // entity, so the rule WAF stored is one it could resolve.
        let read = waf_ok(
            &router,
            case,
            row.get,
            json!({
                "Name": row.referrer_name,
                "Scope": SCOPE,
                "Id": referrer_id,
            }),
        )
        .await;
        let statement = read
            .pointer(&format!("/{}{}", row.entity_member, row.statement_pointer))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            statement.get("ARN").and_then(Value::as_str),
            Some(referenced.as_str()),
            "case {case:?}: expected {} to report the reference under ARN as {referenced:?}, \
             got {statement}",
            row.get
        );
        assert!(
            statement.get("Arn").is_none(),
            "case {case:?}: expected no template-spelled member in what {} answers, got \
             {statement}",
            row.get
        );

        // Nobody changed the entity, so the stack must not report it as modified: the
        // configuration the drift comparison reads back is in the spelling the declaration
        // used.
        stack_ok(
            &router,
            case,
            "DetectStackDrift",
            &[("StackName", row.stack_name)],
        )
        .await;
        tokio::time::sleep(PAST_THE_SPANS).await;
        let resources = stack_ok(
            &router,
            case,
            "DescribeStackResources",
            &[("StackName", row.stack_name)],
        )
        .await;
        let drift: BTreeMap<String, String> = elements(&resources, "LogicalResourceId")
            .into_iter()
            .zip(elements(&resources, "StackResourceDriftStatus"))
            .collect();
        assert_eq!(
            drift.get(row.referrer_logical_id).map(String::as_str),
            Some("IN_SYNC"),
            "case {case:?}: expected {} to be IN_SYNC, got {drift:?}",
            row.referrer_logical_id
        );
    }
}

/// The addresses the protection stack's IP set lists, which `GetIPSet` must report back.
const LISTED_ADDRESSES: &[&str] = &["198.51.100.17/32", "203.0.113.0/24"];

/// The name the protection stack declares its web ACL under.
const PROTECTING_ACL: &str = "protection-acl";

/// The stage of an Amazon API Gateway REST API this web ACL is put in front of, in the ARN
/// format `AssociateWebACLRequest$ResourceArn` documents for that resource kind ("for an
/// Amazon API Gateway REST API: arn:partition:apigateway:region::/restapis/api-id/stages/
/// stage-name"), written out from that format rather than read off any answer.
fn stage_arn() -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/a1b2c3d4e5/stages/prod")
}

/// Whether `id` is an `EntityId`: "^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$"
/// (`smithy_query(query="shape",shape="EntityId")`), the id every WAFv2 entity is addressed
/// by.
fn is_entity_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit))
            })
}

/// The whole protection a template deploys: an IP set, a web ACL whose rule blocks the
/// addresses in it, and the binding that puts the ACL in front of a REST API stage. Every
/// identifier the stack publishes is one AWS WAF minted, and each resolves against that
/// service.
#[tokio::test]
async fn a_deployed_protection_publishes_identifiers_waf_resolves() {
    let case = "an IP set, a web ACL blocking it, and the binding to a protected stage";
    let router = build_router();
    let template = json!({
        "Resources": {
            "Listed": ip_set("protection-set", LISTED_ADDRESSES),
            "Acl": web_acl(PROTECTING_ACL, json!([{
                "Name": "block-listed",
                "Priority": 0,
                "Action": {"Block": {}},
                "Statement": {
                    "IPSetReferenceStatement": {"Arn": referenced_arn("Listed")}
                },
                "VisibilityConfig": visibility("block-listed"),
            }])),
            "Protection": {
                "Type": "AWS::WAFv2::WebACLAssociation",
                "Properties": {
                    "ResourceArn": stage_arn(),
                    "WebACLArn": referenced_arn("Acl"),
                }
            },
        },
        "Outputs": {
            "WebACLArn": {"Value": referenced_arn("Acl")},
            "WebACLId": {"Value": {"Ref": "Acl"}},
            "IPSetArn": {"Value": referenced_arn("Listed")},
            "IPSetId": {"Value": {"Ref": "Listed"}},
        },
    })
    .to_string();

    let described = create_and_settle(&router, case, "wafv2-protection", &template).await;
    let published = outputs_of(&described);
    for key in ["WebACLArn", "WebACLId", "IPSetArn", "IPSetId"] {
        assert!(
            published.get(key).is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected the stack to publish {key}, got {published:?}"
        );
    }
    let web_acl_arn = published["WebACLArn"].clone();
    let web_acl_id = published["WebACLId"].clone();
    let ip_set_id = published["IPSetId"].clone();

    // The identifier published for the IP set is the one AWS WAF assigned, so that service
    // resolves it — and answers the addresses the template declared.
    assert!(
        is_entity_id(&ip_set_id),
        "case {case:?}: expected the published IP set id to be an EntityId, got {ip_set_id:?}"
    );
    let read = waf_ok(
        &router,
        case,
        "GetIPSet",
        json!({"Name": "protection-set", "Scope": SCOPE, "Id": ip_set_id}),
    )
    .await;
    // `IPSet$Addresses` is the SET those strings name, and AWS WAF reports it in its own
    // order rather than the order the array was written in — the rule that service
    // implements in `state/ip_set_addresses.rs` (`canonical_ip_set_addresses`), where the
    // order is a function of the membership. What the stack has to get right is therefore
    // the MEMBERSHIP that reached AWS WAF, compared here independently of order.
    let mut addresses: Vec<&str> = read
        .pointer("/IPSet/Addresses")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    addresses.sort_unstable();
    let mut declared: Vec<&str> = LISTED_ADDRESSES.to_vec();
    declared.sort_unstable();
    assert_eq!(
        addresses, declared,
        "case {case:?}: expected GetIPSet to report the declared addresses, got {read}"
    );

    // The web ACL's ARN carries the triple that service addresses an entity by, so it can be
    // decomposed again: its tail is the entity's name and the id AWS WAF assigned.
    assert!(
        is_entity_id(&web_acl_id),
        "case {case:?}: expected the published web ACL id to be an EntityId, got {web_acl_id:?}"
    );
    let tail = format!("/webacl/{PROTECTING_ACL}/{web_acl_id}");
    assert!(
        web_acl_arn.ends_with(&tail),
        "case {case:?}: expected the published web ACL ARN to end in {tail:?}, got \
         {web_acl_arn:?}"
    );

    // And the binding the stack made is one AWS WAF holds: the protected stage reports this
    // web ACL in front of it, and the web ACL reports that stage behind it.
    let in_front = waf_ok(
        &router,
        case,
        "GetWebACLForResource",
        json!({"ResourceArn": stage_arn()}),
    )
    .await;
    assert_eq!(
        in_front.pointer("/WebACL/ARN").and_then(Value::as_str),
        Some(web_acl_arn.as_str()),
        "case {case:?}: expected the protected stage to report the deployed web ACL, got \
         {in_front}"
    );
    let behind = waf_ok(
        &router,
        case,
        "ListResourcesForWebACL",
        json!({"WebACLArn": web_acl_arn, "ResourceType": "API_GATEWAY"}),
    )
    .await;
    let protected: Vec<&str> = behind
        .get("ResourceArns")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        protected,
        vec![stage_arn().as_str()],
        "case {case:?}: expected the web ACL to report the protected stage behind it, got \
         {behind}"
    );
}
