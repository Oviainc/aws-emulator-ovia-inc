// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::XRay::SamplingRule` really creates a
//! sampling rule in AWS X-Ray, and the rule the stack hands back is the one that service
//! samples with.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, under the configuration the
//! template declared — so once the stack reports `CREATE_COMPLETE`, the owning service's own
//! reads resolve the resource (here `GetSamplingRules` reports the rule and
//! `GetSamplingTargets` serves an allocation for it), and deleting the stack removes it
//! there.** Two crates have to agree for that to hold (CloudFormation makes the calls, X-Ray
//! serves them and mints the `RuleARN`), so the suite lives here rather than in either
//! crate.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the rule
//! or leaves CloudFormation to generate its name, the `Priority`, `FixedRate` and
//! `ReservoirSize` it declares (including both published `Priority` bounds and a reservoir of
//! none), whether the matchers are the `*` wildcards or a concrete service, host and path,
//! whether it carries `Tags` and an `Attributes` map, whether it is written in JSON or YAML,
//! and the Region it deploys in.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none, which
//! is CloudFormation's to compute: what is checked about it is that the ARN the stack
//! publishes is that name's sampling-rule ARN, that the name is inside the member's published
//! bound, and that X-Ray's own listing resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The stack-level tag every row's `CreateStack` states, which CloudFormation propagates to
/// the resources the stack creates.
const STACK_TAG_KEY: &str = "Stack";
const STACK_TAG_VALUE: &str = "xray-sampling-suite";

/// The longest a rule's name may be: `SamplingRule$RuleName` is
/// `@length(min: 1, max: 32)` in X-Ray's own model, which is the bound a name
/// CloudFormation generates has to be inside.
const RULE_NAME_MAX: usize = 32;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
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
    router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one X-Ray operation over its restJson1 wire, signed for `region`.
async fn xray(
    router: &MultiServiceRouter,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("xray.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("xray", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Every `SamplingRule` X-Ray reports for the account, walked over every page of
/// `GetSamplingRules` the way a client reading the whole listing does.
async fn listed_rules(router: &MultiServiceRouter, region: &str) -> Vec<Value> {
    let mut rules = Vec::new();
    let mut token: Option<String> = None;
    // Far above the number of rules a row creates, so the walk ends on the listing's own
    // last page rather than on this bound.
    for _ in 0..16 {
        let mut body = json!({});
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let (status, answered) = xray(router, region, "/GetSamplingRules", &body).await;
        assert_eq!(
            status, 200,
            "GetSamplingRules must answer 200, got {status} with body {answered}"
        );
        for record in answered
            .get("SamplingRuleRecords")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(rule) = record.get("SamplingRule") {
                rules.push(rule.clone());
            }
        }
        match answered.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    rules
}

/// The rule of that name in the listing, or `None`.
async fn listed_rule(router: &MultiServiceRouter, region: &str, name: &str) -> Option<Value> {
    listed_rules(router, region)
        .await
        .into_iter()
        .find(|rule| rule.get("RuleName").and_then(Value::as_str) == Some(name))
}

/// The ARN a sampling rule of `name` carries in `region`: the same total function of the
/// Region and the name both services compute it from.
fn sampling_rule_arn(region: &str, name: &str) -> String {
    format!("arn:aws:xray:{region}:{ACCOUNT}:sampling-rule/{name}")
}

/// Now, in the epoch seconds a reporting service stamps its statistics with.
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// One row of the table: a template, and what the rule says the sampling rule it declares
/// must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the rule is created in and the one
    /// its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The name the template declares, or `None` for a template that names no rule — whose
    /// rule carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The members `GetSamplingRules` must report for the rule, each as the template
    /// declared it. A JSON row's are read off its own template body
    /// ([`declared_rule`]); a YAML row states them here, since this suite reads no YAML.
    expected_members: Vec<(&'static str, Value)>,
    /// The tags `ListTagsForResource` must report for the rule's ARN, beside the three
    /// CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the rule's ARN.
    arn_outputs: &'static [&'static str],
}

/// The rows. Written as a function rather than a `const` because each expected member is a
/// `serde_json` value.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a wildcard rule in JSON, tagged, read with Fn::GetAtt RuleARN",
            region: "us-east-1",
            template: || {
                json!({
                    "AWSTemplateFormatVersion": "2010-09-09",
                    "Description": "an X-Ray sampling rule",
                    "Resources": {
                        "Rule": {
                            "Type": "AWS::XRay::SamplingRule",
                            "Properties": {
                                "SamplingRule": {
                                    "RuleName": "orders-sampling",
                                    "ResourceARN": "*",
                                    "Priority": 500,
                                    "FixedRate": 0.05,
                                    "ReservoirSize": 1,
                                    "ServiceName": "*",
                                    "ServiceType": "*",
                                    "Host": "*",
                                    "HTTPMethod": "*",
                                    "URLPath": "*",
                                    "Version": 1
                                },
                                "Tags": [
                                    { "Key": "Owner", "Value": "orders" },
                                    { "Key": "Tier", "Value": "prod" }
                                ]
                            }
                        }
                    },
                    "Outputs": {
                        "RuleARN": { "Value": { "Fn::GetAtt": ["Rule", "RuleARN"] } }
                    }
                })
                .to_string()
            },
            declared_name: Some("orders-sampling"),
            expected_members: Vec::new(),
            expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
            arn_outputs: &["RuleARN"],
        },
        Row {
            case: "a concrete matcher set at the highest priority, in YAML, with attributes",
            region: "eu-west-1",
            template: || {
                [
                    "Resources:",
                    "  Rule:",
                    "    Type: AWS::XRay::SamplingRule",
                    "    Properties:",
                    "      SamplingRule:",
                    "        RuleName: checkout-sampling",
                    "        ResourceARN: \"*\"",
                    "        Priority: 1",
                    "        FixedRate: 1.0",
                    "        ReservoirSize: 60",
                    "        ServiceName: checkout",
                    "        ServiceType: AWS::ECS::Container",
                    "        Host: checkout.example.com",
                    "        HTTPMethod: POST",
                    "        URLPath: /orders/*",
                    "        Version: 1",
                    "        Attributes:",
                    "          tier: gold",
                    "",
                ]
                .join("\n")
            },
            declared_name: Some("checkout-sampling"),
            // The YAML row's declared members, stated here because this suite reads no
            // YAML: every member of the rule its template above declares.
            expected_members: vec![
                ("ResourceARN", json!("*")),
                ("Priority", json!(1)),
                ("FixedRate", json!(1.0)),
                ("ReservoirSize", json!(60)),
                ("ServiceName", json!("checkout")),
                ("ServiceType", json!("AWS::ECS::Container")),
                ("Host", json!("checkout.example.com")),
                ("HTTPMethod", json!("POST")),
                ("URLPath", json!("/orders/*")),
                ("Version", json!(1)),
                ("Attributes", json!({"tier": "gold"})),
            ],
            expected_tags: &[],
            arn_outputs: &[],
        },
        Row {
            case: "the lowest priority and no reservoir at all",
            region: "ap-southeast-2",
            template: || {
                json!({
                    "Resources": {
                        "Rule": {
                            "Type": "AWS::XRay::SamplingRule",
                            "Properties": {
                                "SamplingRule": {
                                    "RuleName": "catch-all",
                                    "ResourceARN": "*",
                                    "Priority": 9999,
                                    "FixedRate": 0.0,
                                    "ReservoirSize": 0,
                                    "ServiceName": "*",
                                    "ServiceType": "*",
                                    "Host": "*",
                                    "HTTPMethod": "*",
                                    "URLPath": "*",
                                    "Version": 1
                                }
                            }
                        }
                    }
                })
                .to_string()
            },
            declared_name: Some("catch-all"),
            expected_members: Vec::new(),
            expected_tags: &[],
            arn_outputs: &[],
        },
        Row {
            case: "a rule the template does not name, so CloudFormation generates one",
            region: "us-west-2",
            template: || {
                json!({
                    "Resources": {
                        "Rule": {
                            "Type": "AWS::XRay::SamplingRule",
                            "Properties": {
                                "SamplingRule": {
                                    "ResourceARN": "*",
                                    "Priority": 4000,
                                    "FixedRate": 0.31,
                                    "ReservoirSize": 7,
                                    "ServiceName": "*",
                                    "ServiceType": "*",
                                    "Host": "*",
                                    "HTTPMethod": "GET",
                                    "URLPath": "*",
                                    "Version": 1
                                },
                                "Tags": [{ "Key": "Owner", "Value": "platform" }]
                            }
                        }
                    }
                })
                .to_string()
            },
            declared_name: None,
            expected_members: Vec::new(),
            expected_tags: &[("Owner", "platform")],
            arn_outputs: &[],
        },
    ]
}

/// The members of the rule each row declares, read off the row's own template so the
/// expectations are the template's statement rather than the answer's.
fn declared_rule(template: &str) -> BTreeMap<String, Value> {
    // A JSON template is read as a document; a YAML one is read from the lines above,
    // whose declared members this mirrors.
    if let Ok(document) = serde_json::from_str::<Value>(template) {
        let declared = document
            .get("Resources")
            .and_then(|resources| resources.get("Rule"))
            .and_then(|rule| rule.get("Properties"))
            .and_then(|properties| properties.get("SamplingRule"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        return declared.into_iter().collect();
    }
    BTreeMap::new()
}

#[tokio::test]
async fn a_stack_provisions_the_xray_sampling_rule_its_template_declares() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("xray-sampling-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                // A stack-level tag, which CloudFormation propagates to the resources it
                // creates — so the rule has to end up carrying it beside its own.
                ("Tags.member.1.Key", STACK_TAG_KEY),
                ("Tags.member.1.Value", STACK_TAG_VALUE),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));

        // The type's identity is the `RuleARN` X-Ray minted, which carries the rule's name:
        // the one the template asked for, or the one CloudFormation generated for a template
        // that named none.
        let name = match row.declared_name {
            Some(declared) => declared.to_string(),
            None => physical_id
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
        };
        let arn = sampling_rule_arn(region, &name);
        assert_eq!(
            physical_id, arn,
            "case {case:?}: expected the physical id to be the rule ARN {arn}, got \
             {physical_id:?}"
        );
        assert!(
            !name.is_empty() && name.len() <= RULE_NAME_MAX,
            "case {case:?}: expected a rule name inside the published 1..{RULE_NAME_MAX}, got \
             {name:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&arn),
                "case {case:?}: expected output {key:?} to be the rule ARN {arn:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // X-Ray itself reports the rule, carrying every member the template declared and the
        // ARN that service minted.
        let reported = listed_rule(&router, region, &name).await.unwrap_or_else(|| {
            panic!("case {case:?}: GetSamplingRules reports no rule named {name:?}")
        });
        assert_eq!(
            reported.get("RuleARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the listed rule to carry the ARN {arn}, got {reported}"
        );
        for (member, declared) in declared_rule(&template) {
            if member == "RuleName" {
                continue;
            }
            assert_eq!(
                reported.get(&member),
                Some(&declared),
                "case {case:?}: expected the listed rule's {member} to be the declared \
                 {declared}, got {:?}",
                reported.get(&member)
            );
        }
        for (member, expected) in &row.expected_members {
            assert_eq!(
                reported.get(*member),
                Some(expected),
                "case {case:?}: expected the listed rule's {member} to be {expected}, got {:?}",
                reported.get(*member)
            );
        }
        // Every row states its declared members ONE way or the other, so no row can
        // pass this block without having its configuration checked.
        assert!(
            !declared_rule(&template).is_empty() || !row.expected_members.is_empty(),
            "case {case:?}: the row must state the members its template declares, either in \
             its JSON body or in expected_members"
        );

        // And the rule SAMPLES: a service reporting statistics against it is served an
        // allocation for it rather than having its statistics left unprocessed.
        let (status, targets) = xray(
            &router,
            region,
            "/SamplingTargets",
            &json!({
                "SamplingStatisticsDocuments": [{
                    "RuleName": name,
                    "ClientID": format!("{index:024x}"),
                    "Timestamp": now(),
                    "RequestCount": 10,
                    "SampledCount": 1,
                    "BorrowCount": 0,
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetSamplingTargets, got {status} with body \
             {targets}"
        );
        let unprocessed = targets
            .get("UnprocessedStatistics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            unprocessed.is_empty(),
            "case {case:?}: expected the statistics of a provisioned rule to be processed, got \
             {unprocessed:?}"
        );
        let target = targets
            .get("SamplingTargetDocuments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|document| document.get("RuleName").and_then(Value::as_str) == Some(&name))
            .unwrap_or_else(|| {
                panic!("case {case:?}: no SamplingTargetDocument for {name:?} in {targets}")
            });
        let declared_rate = declared_rule(&template)
            .get("FixedRate")
            .and_then(Value::as_f64)
            .or_else(|| reported.get("FixedRate").and_then(Value::as_f64))
            .unwrap_or_default();
        assert_eq!(
            target.get("FixedRate").and_then(Value::as_f64),
            Some(declared_rate),
            "case {case:?}: expected the target to serve the declared rate {declared_rate}, got \
             {target}"
        );
        let reservoir = reported
            .get("ReservoirSize")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let quota = target
            .get("ReservoirQuota")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            (0.0..=reservoir).contains(&quota),
            "case {case:?}: expected a reservoir share inside the rule's pool of {reservoir}, \
             got {quota}"
        );

        // The tags the declaration asked for are on the rule, beside the stack's own, and
        // reachable through the ARN the stack published. The three `aws:cloudformation:*`
        // tags are NOT: X-Ray refuses that namespace to a caller, and CloudFormation must
        // not fail a stack over tags it adds itself.
        let (status, listed) = xray(
            &router,
            region,
            "/ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let tags: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'Tags' array in {listed}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("Key").and_then(Value::as_str)?.to_string(),
                    tag.get("Value").and_then(Value::as_str)?.to_string(),
                ))
            })
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                tags.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {tags:?}"
            );
        }
        assert_eq!(
            tags.get(STACK_TAG_KEY).map(String::as_str),
            Some(STACK_TAG_VALUE),
            "case {case:?}: expected the stack's own tag to be propagated, got {tags:?}"
        );
        assert!(
            !tags.keys().any(|key| key.starts_with("aws:")),
            "case {case:?}: expected no reserved tag key to be sent to X-Ray, got {tags:?}"
        );
        assert_eq!(
            tags.len(),
            row.expected_tags.len() + 1,
            "case {case:?}: expected exactly the {} tag(s) the template declared beside the \
             stack's own, got {tags:?}",
            row.expected_tags.len()
        );

        // Deleting the stack deletes the rule X-Ray holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        assert!(
            listed_rule(&router, region, &name).await.is_none(),
            "case {case:?}: expected GetSamplingRules to no longer report {name:?} once the \
             stack was deleted"
        );
    }
}

/// A stack whose template declares a rule X-Ray REFUSES does not report a resource nothing
/// created: the stack fails with that service's own refusal, and the rule a client already
/// had is untouched.
///
/// The refusal here is X-Ray's own: a rule name the account already holds, which
/// `CreateSamplingRule` rejects rather than redefining.
#[tokio::test]
async fn a_sampling_rule_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a template naming a rule a client already created";
    let region = "us-east-2";
    let name = "already-there";

    let (status, created) = xray(
        &router,
        region,
        "/CreateSamplingRule",
        &json!({
            "SamplingRule": {
                "RuleName": name,
                "ResourceARN": "*",
                "Priority": 100,
                "FixedRate": 0.5,
                "ReservoirSize": 3,
                "ServiceName": "*",
                "ServiceType": "*",
                "Host": "*",
                "HTTPMethod": "*",
                "URLPath": "*",
                "Version": 1,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own CreateSamplingRule to succeed, got {status} \
         with body {created}"
    );

    let template = json!({
        "Resources": {
            "Rule": {
                "Type": "AWS::XRay::SamplingRule",
                "Properties": {
                    "SamplingRule": {
                        "RuleName": name,
                        "ResourceARN": "*",
                        "Priority": 200,
                        "FixedRate": 0.9,
                        "ReservoirSize": 9,
                        "ServiceName": "*",
                        "ServiceType": "*",
                        "Host": "*",
                        "HTTPMethod": "*",
                        "URLPath": "*",
                        "Version": 1
                    }
                }
            }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "sampling-clash"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "sampling-clash")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Rule"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    // The rule the client created is still exactly as it was.
    let held = listed_rule(&router, region, name)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the client's own rule must still be listed"));
    assert_eq!(
        (
            held.get("Priority").and_then(Value::as_i64),
            held.get("FixedRate").and_then(Value::as_f64),
            held.get("ReservoirSize").and_then(Value::as_i64),
        ),
        (Some(100), Some(0.5), Some(3)),
        "case {case:?}: expected the client's own configuration to be untouched, got {held}"
    );
}
