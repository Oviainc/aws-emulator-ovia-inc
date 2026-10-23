// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the `AWS::WAFRegional::IPSet` resources its template
//! declares — in AWS WAF Classic — and the handle it publishes for each one is the id that
//! service minted.
//!
//! The rule under test: **a template may declare any resource type AWS publishes, and a
//! stack reaches its completed status only once the service that owns each declared
//! resource has created it — with the physical id the stack reports (`Ref`) being the id
//! the OWNING service answered with, wherever that service is the one that mints the
//! resource's identity.** `AWS::WAFRegional::IPSet`'s published `Ref` is "the IPSet ID", a
//! value only AWS WAF Classic produces, so the stack's output has to be an id `GetIPSet`
//! resolves — not the `Name` the template asked for, and not a `ValidationError` refusing
//! the template at submission. Two crates have to agree for that to hold (CloudFormation
//! makes the calls, AWS WAF Classic Regional serves them), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! IPSet or leaves the name to CloudFormation, how many address descriptors it declares
//! (none, one, several), which address family and prefix length each descriptor carries,
//! how many IPSets one stack declares, how the output is written (`Ref`, `Fn::GetAtt` of
//! `Id` and of `IPSetId`), and which spelling the template is in (JSON and YAML). Every
//! expected value is computed from what the row declares, never read back out of the
//! answer being checked.
//!
//! The deletion half, the re-deploy half and the refusal half are rows of the same rule:
//! deleting the stack deletes the IPSets it created — so `GetIPSet` then answers
//! `WAFNonexistentItemException` and `UpdateIPSet` `WAFNonexistentContainerException` for
//! that id — an update that re-declares an IPSet unchanged keeps the id that service
//! minted rather than creating a second IPSet, and an IPSet AWS WAF Classic REFUSES to
//! configure leaves the stack `CREATE_FAILED` naming the resource, with nothing left
//! behind in that service.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate publishes ONCE, in `query_wire`: a suite that
// writes its own percent-encoder or element reader drifts from the others.
use crate::query_wire::{elements, form_encode, opt_element};

/// The region every request here is signed for, and so the region the stack's IPSets
/// belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due. An IPSet has no transient status of its own: AWS WAF Classic answers
/// `CreateIPSet` with a record its own `GetIPSet` reads at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
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
        service_com_amazonaws_wafregional_awswaf_regional_20161128::EmulatedService::new(engine),
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

/// Call one CloudFormation operation over its awsQuery wire.
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

/// Call one AWS WAF Classic Regional operation over its awsJson1_1 wire.
async fn waf_regional(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("waf-regional.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSWAF_Regional_20161128.{operation}"),
        )
        .header("authorization", authorization("waf-regional"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One AWS WAF Classic call that must succeed.
async fn waf_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> Value {
    let (status, parsed) = waf_regional(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of an AWS WAF Classic refusal: awsJson1_1 carries it as the body's
/// `__type`.
fn waf_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A change token for one mutating AWS WAF Classic call, as a client gets one.
async fn change_token(router: &MultiServiceRouter, case: &str) -> String {
    let answer = waf_ok(router, case, "GetChangeToken", json!({})).await;
    answer
        .get("ChangeToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ChangeToken in {answer}"))
        .to_string()
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

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack for a published resource type, \
         got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Whether `id` has the 36-character UUID shape AWS WAF Classic's `ResourceId` values take
/// (`8-4-4-4-12` lower-case hexadecimal groups) — the form of an id that service minted,
/// which is what distinguishes it from a name the template or CloudFormation chose.
fn is_waf_resource_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for
/// a resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

/// What the row says about one IPSet the template declares.
struct DeclaredIpSet {
    /// The resource's logical id, which a generated name is built from.
    logical_id: &'static str,
    /// The output key carrying `{"Ref": <logical id>}`.
    ref_output: &'static str,
    /// Output keys that are `Fn::GetAtt` of this resource and must carry the same id as
    /// the `Ref` does: the uniform `Id`/`<Type>Id` return values.
    id_outputs: &'static [&'static str],
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// The `IPSetDescriptors` the template declares, as `(Type, Value)` in declaration
    /// order — which is the order AWS WAF Classic reports them back in.
    declared_descriptors: &'static [(&'static str, &'static str)],
}

/// One row: a template and the IPSets it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    ip_sets: &'static [DeclaredIpSet],
}

const ROWS: &[Row] = &[
    Row {
        // The reported case: a template whose only resource is a named IPSet with a single
        // output referencing it.
        case: "a named IPSet with no descriptors, output by Ref",
        stack_name: "ipset-lifecycle-stack",
        template: r#"{"Resources":{"TestIPSet":{"Type":"AWS::WAFRegional::IPSet",
               "Properties":{"Name":"stack-blocked-addresses"}}},
             "Outputs":{"IPSetId":{"Value":{"Ref":"TestIPSet"}}}}"#,
        ip_sets: &[DeclaredIpSet {
            logical_id: "TestIPSet",
            ref_output: "IPSetId",
            id_outputs: &[],
            declared_name: Some("stack-blocked-addresses"),
            declared_descriptors: &[],
        }],
    },
    Row {
        case: "an unnamed IPSet with one IPv4 descriptor, output by Ref and Fn::GetAtt Id",
        stack_name: "unnamed-ipset-stack",
        template: r#"{"Resources":{"Blocked":{"Type":"AWS::WAFRegional::IPSet",
               "Properties":{"IPSetDescriptors":[{"Type":"IPV4","Value":"203.0.113.0/24"}]}}},
             "Outputs":{"BlockedId":{"Value":{"Ref":"Blocked"}},
               "BlockedAttId":{"Value":{"Fn::GetAtt":["Blocked","Id"]}}}}"#,
        ip_sets: &[DeclaredIpSet {
            logical_id: "Blocked",
            ref_output: "BlockedId",
            id_outputs: &["BlockedAttId"],
            declared_name: None,
            // `/24` is inside the IPv4 range AWS WAF Classic supports (`/16` through
            // `/32`), per `UpdateIPSet`'s documentation.
            declared_descriptors: &[("IPV4", "203.0.113.0/24")],
        }],
    },
    Row {
        case: "a named IPSet with several descriptors of both address families",
        stack_name: "mixed-family-ipset-stack",
        template: r#"{"Resources":{"Mixed":{"Type":"AWS::WAFRegional::IPSet",
               "Properties":{"Name":"stack-mixed-addresses",
                 "IPSetDescriptors":[{"Type":"IPV4","Value":"192.0.2.44/32"},
                   {"Type":"IPV4","Value":"198.51.100.0/16"},
                   {"Type":"IPV6","Value":"2001:db8::1/128"},
                   {"Type":"IPV6","Value":"2001:db8:1234::/64"}]}}},
             "Outputs":{"MixedId":{"Value":{"Ref":"Mixed"}},
               "MixedTypedId":{"Value":{"Fn::GetAtt":["Mixed","IPSetId"]}}}}"#,
        ip_sets: &[DeclaredIpSet {
            logical_id: "Mixed",
            ref_output: "MixedId",
            id_outputs: &["MixedTypedId"],
            declared_name: Some("stack-mixed-addresses"),
            // The prefix lengths AWS WAF Classic documents: IPv4 /8 and /16../32, IPv6
            // /24, /32, /48, /56, /64 and /128.
            declared_descriptors: &[
                ("IPV4", "192.0.2.44/32"),
                ("IPV4", "198.51.100.0/16"),
                ("IPV6", "2001:db8::1/128"),
                ("IPV6", "2001:db8:1234::/64"),
            ],
        }],
    },
    Row {
        case: "two IPSets in one stack, one named and one not",
        stack_name: "two-ipset-stack",
        template: r#"{"Resources":{
               "Allowed":{"Type":"AWS::WAFRegional::IPSet",
                 "Properties":{"Name":"stack-allowed-addresses",
                   "IPSetDescriptors":[{"Type":"IPV4","Value":"10.0.0.0/8"}]}},
               "Denied":{"Type":"AWS::WAFRegional::IPSet"}},
             "Outputs":{"AllowedId":{"Value":{"Ref":"Allowed"}},
               "DeniedId":{"Value":{"Ref":"Denied"}}}}"#,
        ip_sets: &[
            DeclaredIpSet {
                logical_id: "Allowed",
                ref_output: "AllowedId",
                id_outputs: &[],
                declared_name: Some("stack-allowed-addresses"),
                declared_descriptors: &[("IPV4", "10.0.0.0/8")],
            },
            DeclaredIpSet {
                logical_id: "Denied",
                ref_output: "DeniedId",
                id_outputs: &[],
                declared_name: None,
                declared_descriptors: &[],
            },
        ],
    },
    Row {
        case: "the YAML spelling of a template declaring a named IPSet with a descriptor",
        stack_name: "yaml-ipset-stack",
        template: concat!(
            "Resources:\n",
            "  Watched:\n",
            "    Type: AWS::WAFRegional::IPSet\n",
            "    Properties:\n",
            "      Name: stack-watched-addresses\n",
            "      IPSetDescriptors:\n",
            "        - Type: IPV6\n",
            "          Value: 2001:db8:abcd::/48\n",
            "Outputs:\n",
            "  WatchedId:\n",
            "    Value: !Ref Watched\n",
        ),
        ip_sets: &[DeclaredIpSet {
            logical_id: "Watched",
            ref_output: "WatchedId",
            id_outputs: &[],
            declared_name: Some("stack-watched-addresses"),
            declared_descriptors: &[("IPV6", "2001:db8:abcd::/48")],
        }],
    },
];

/// The descriptors an answer's `IPSet` carries, as `(Type, Value)` in reported order.
fn reported_descriptors(ip_set: &Value) -> Vec<(String, String)> {
    ip_set
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .map(|descriptors| {
            descriptors
                .iter()
                .map(|descriptor| {
                    (
                        descriptor
                            .get("Type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        descriptor
                            .get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The same list from the expectation side.
fn expected_descriptors(declared: &[(&str, &str)]) -> Vec<(String, String)> {
    declared
        .iter()
        .map(|(kind, value)| ((*kind).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn a_completed_stack_has_created_every_ip_set_it_declares_in_waf_regional() {
    let router = build_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, row.template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for ip_set in row.ip_sets {
            let case = format!("{} [{}]", row.case, ip_set.logical_id);
            let id = outputs
                .get(ip_set.ref_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", ip_set.ref_output)
                })
                .clone();

            // The Ref is an id AWS WAF Classic minted, not the name the template asked
            // for: "Ref returns the IPSet ID".
            assert!(
                is_waf_resource_id(&id),
                "case {case:?}: expected the Ref to be a minted AWS WAF resource id, got \
                 {id:?}"
            );
            if let Some(declared) = ip_set.declared_name {
                assert_ne!(
                    id, declared,
                    "case {case:?}: expected the Ref to be the minted IPSetId rather than \
                     the declared Name"
                );
            }
            // Every uniform id return value is that same id.
            for output in ip_set.id_outputs {
                let reported = outputs.get(*output);
                assert_eq!(
                    reported.map(String::as_str),
                    Some(id.as_str()),
                    "case {case:?}: expected the {output} output to be the resource's id \
                     {id:?}, got {reported:?}"
                );
            }

            // AWS WAF Classic itself reads that id back, and what it holds is what the
            // template declared.
            let answer = waf_ok(&router, &case, "GetIPSet", json!({"IPSetId": id})).await;
            let ip_set_document = answer.get("IPSet").cloned().unwrap_or(Value::Null);
            assert_eq!(
                ip_set_document.get("IPSetId").and_then(Value::as_str),
                Some(id.as_str()),
                "case {case:?}: expected GetIPSet to report the id the stack published, got \
                 {answer}"
            );
            let name = ip_set_document
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match ip_set.declared_name {
                Some(declared) => assert_eq!(
                    name, declared,
                    "case {case:?}: expected the IPSet to carry the declared name"
                ),
                None => assert!(
                    is_generated_name(&name, row.stack_name, ip_set.logical_id),
                    "case {case:?}: expected a generated <stack>-<logical id>-<suffix> \
                     name, got {name:?}"
                ),
            }
            let reported = reported_descriptors(&ip_set_document);
            assert_eq!(
                reported,
                expected_descriptors(ip_set.declared_descriptors),
                "case {case:?}: expected the declared descriptors, got {answer}"
            );

            // ...and the IPSet is usable through its own API under that id: one more
            // address can be inserted and read back.
            let token = change_token(&router, &case).await;
            waf_ok(
                &router,
                &case,
                "UpdateIPSet",
                json!({
                    "IPSetId": id,
                    "ChangeToken": token,
                    "Updates": [{"Action": "INSERT",
                                 "IPSetDescriptor": {"Type": "IPV4", "Value": "172.16.0.0/16"}}],
                }),
            )
            .await;
            let answer = waf_ok(&router, &case, "GetIPSet", json!({"IPSetId": id})).await;
            let mut expected = expected_descriptors(ip_set.declared_descriptors);
            expected.push(("IPV4".to_string(), "172.16.0.0/16".to_string()));
            assert_eq!(
                reported_descriptors(&answer.get("IPSet").cloned().unwrap_or(Value::Null)),
                expected,
                "case {case:?}: expected the inserted address beside the declared ones, got \
                 {answer}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_ip_sets_it_created() {
    let router = build_router();
    let case = "two IPSets deleted with their stack, one of them holding addresses";
    let stack_name = "deleted-ipset-stack";
    let template = r#"{"Resources":{
           "WithAddresses":{"Type":"AWS::WAFRegional::IPSet",
             "Properties":{"Name":"deleted-stack-with-addresses",
               "IPSetDescriptors":[{"Type":"IPV4","Value":"203.0.113.0/32"},
                 {"Type":"IPV6","Value":"2001:db8::/32"}]}},
           "Empty":{"Type":"AWS::WAFRegional::IPSet",
             "Properties":{"Name":"deleted-stack-empty"}}},
         "Outputs":{"WithAddressesId":{"Value":{"Ref":"WithAddresses"}},
           "EmptyId":{"Value":{"Ref":"Empty"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let ids: Vec<String> = ["WithAddressesId", "EmptyId"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for id in &ids {
        waf_ok(&router, case, "GetIPSet", json!({"IPSetId": id})).await;
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // Every IPSet the stack created is gone from AWS WAF Classic — including the one that
    // still held addresses, which the delete had to empty first.
    for id in &ids {
        let (status, answer) = waf_regional(&router, "GetIPSet", &json!({"IPSetId": id})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the IPSet {id} to be gone, got {status} with {answer}"
        );
        assert_eq!(
            waf_error_code(&answer),
            "WAFNonexistentItemException",
            "case {case:?}: expected WAFNonexistentItemException for {id}, got {answer}"
        );
        // And it is no container for an update either: the id addresses nothing.
        let token = change_token(&router, case).await;
        let (status, answer) = waf_regional(
            &router,
            "UpdateIPSet",
            &json!({
                "IPSetId": id,
                "ChangeToken": token,
                "Updates": [{"Action": "INSERT",
                             "IPSetDescriptor": {"Type": "IPV4", "Value": "203.0.113.0/32"}}],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused for {id}, got {status} with \
             {answer}"
        );
        assert_eq!(
            waf_error_code(&answer),
            "WAFNonexistentContainerException",
            "case {case:?}: expected WAFNonexistentContainerException for {id}, got {answer}"
        );
    }

    // Nothing of the stack's is left in the service at all.
    let listed = waf_ok(&router, case, "ListIPSets", json!({})).await;
    let held = listed
        .get("IPSets")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        held, 0,
        "case {case:?}: expected no IPSet left after the stack's deletion, got {listed}"
    );
}

#[tokio::test]
async fn an_update_keeps_the_ip_set_it_re_declares_and_creates_the_one_it_adds() {
    let router = build_router();
    let case = "an update that re-declares one IPSet unchanged and adds another";
    let stack_name = "updated-ipset-stack";
    // The first template declares `Kept` alone; the second declares it identically and
    // adds `Added`. The identity of an IPSet is the id AWS WAF Classic minted for it, so a
    // re-deploy must not mint a second one — and the stack's own deletion has to delete
    // both under the ids that service knows them by.
    let kept = r#""Kept":{"Type":"AWS::WAFRegional::IPSet",
           "Properties":{"Name":"updated-stack-kept",
             "IPSetDescriptors":[{"Type":"IPV4","Value":"192.0.2.0/24"}]}}"#;
    let before = format!(
        r#"{{"Resources":{{{kept}}},"Outputs":{{"KeptId":{{"Value":{{"Ref":"Kept"}}}}}}}}"#
    );
    let after = format!(
        r#"{{"Resources":{{{kept},
             "Added":{{"Type":"AWS::WAFRegional::IPSet",
               "Properties":{{"Name":"updated-stack-added"}}}}}},
           "Outputs":{{"KeptId":{{"Value":{{"Ref":"Kept"}}}},
             "AddedId":{{"Value":{{"Ref":"Added"}}}}}}}}"#
    );

    let body = create_and_settle(&router, case, stack_name, &before).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {body}"
    );
    let created_id = outputs_of(&body)
        .get("KeptId")
        .unwrap_or_else(|| panic!("case {case:?}: no KeptId output in {body}"))
        .clone();

    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &after)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    assert_eq!(
        outputs.get("KeptId").map(String::as_str),
        Some(created_id.as_str()),
        "case {case:?}: expected the re-declared IPSet to keep the id AWS WAF minted for \
         it ({created_id:?}), got {outputs:?}"
    );
    let added_id = outputs
        .get("AddedId")
        .unwrap_or_else(|| panic!("case {case:?}: no AddedId output in {body}"))
        .clone();
    assert!(
        is_waf_resource_id(&added_id) && added_id != created_id,
        "case {case:?}: expected a second, distinct minted id for the added IPSet, got \
         {added_id:?}"
    );

    // One IPSet per declared resource, each readable under the id the stack reports: the
    // re-deploy created no duplicate.
    let listed = waf_ok(&router, case, "ListIPSets", json!({})).await;
    let names: Vec<String> = listed
        .get("IPSets")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary
                        .get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![
            "updated-stack-kept".to_string(),
            "updated-stack-added".to_string(),
        ],
        "case {case:?}: expected exactly the two declared IPSets, got {listed}"
    );
    let kept_record = waf_ok(&router, case, "GetIPSet", json!({"IPSetId": created_id})).await;
    assert_eq!(
        reported_descriptors(&kept_record.get("IPSet").cloned().unwrap_or(Value::Null)),
        expected_descriptors(&[("IPV4", "192.0.2.0/24")]),
        "case {case:?}: expected the kept IPSet to keep its addresses, got {kept_record}"
    );

    // And the stack's deletion deletes both, under the ids AWS WAF Classic knows them by.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    for id in [&created_id, &added_id] {
        let (status, answer) = waf_regional(&router, "GetIPSet", &json!({"IPSetId": id})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the IPSet {id} to be gone with the stack, got {status} \
             with {answer}"
        );
    }
}

#[tokio::test]
async fn an_ip_set_waf_regional_refuses_to_configure_fails_the_stack() {
    let router = build_router();
    // Each row is a descriptor AWS WAF Classic Regional will not take, and the error code
    // IT answers with: one its model refuses (an `IPSetDescriptorType` outside the enum
    // `[IPV4, IPV6]`) and one its handler refuses ("AWS WAF supports IPv4 address ranges:
    // /8 and any range between /16 through /32", so a `/12` is an illegal argument).
    let rows = [
        (
            "a descriptor whose Type is not one of the modelled two",
            "refused-type-ipset-stack",
            r#"{"Type":"IPV5","Value":"203.0.113.0/32"}"#,
            "ValidationException",
        ),
        (
            "a descriptor whose prefix length AWS WAF does not support",
            "refused-prefix-ipset-stack",
            r#"{"Type":"IPV4","Value":"172.16.0.0/12"}"#,
            "WAFInvalidParameterException",
        ),
    ];

    for (case, stack_name, descriptor, expected_code) in rows {
        let template = format!(
            r#"{{"Resources":{{"Blocked":{{"Type":"AWS::WAFRegional::IPSet",
               "Properties":{{"Name":"{stack_name}-addresses",
                 "IPSetDescriptors":[{descriptor}]}}}}}},
             "Outputs":{{"BlockedId":{{"Value":{{"Ref":"Blocked"}}}}}}}}"#
        );

        let body = create_and_settle(&router, case, stack_name, &template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_FAILED"),
            "case {case:?}: expected the stack to fail rather than publish an IPSet that is \
             not the one declared, got {body}"
        );
        let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Blocked") && reason.contains(expected_code),
            "case {case:?}: expected the reason to name the resource and the refusal \
             {expected_code}, got {reason:?}"
        );

        // And nothing was left behind in the service: the IPSet the first call created was
        // deleted again when the configuration could not be applied.
        let listed = waf_ok(&router, case, "ListIPSets", json!({})).await;
        let held = listed
            .get("IPSets")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held, 0,
            "case {case:?}: expected the failed create to leave no IPSet behind, got {listed}"
        );
    }
}
