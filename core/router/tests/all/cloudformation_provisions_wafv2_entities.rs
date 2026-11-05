// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the `AWS::WAFv2::*` resources its template declares —
//! in WAF itself — and the identifiers it publishes for each one are the ones that service
//! minted.
//!
//! The rule under test: **where the service that owns a declared resource MINTS the
//! resource's identifiers, the values a stack publishes for that resource are the ones that
//! service answered with, so a handle read out of the stack addresses the resource through
//! the owning service's own API.** For a WAFv2 entity those identifiers are the `Id` its
//! create assigns and the `ARN` WAF builds from the scope, the name and that id
//! (`…:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`), which is the ARN
//! `AssociateWebACL` and `GetWebACL` decompose into the triple the API addresses an entity
//! by — not an ARN of CloudFormation's own shape, which identifies nothing and which WAF
//! refuses as malformed. Two crates have to agree for that to hold (CloudFormation makes the
//! calls, WAF serves them), so the suite lives here: a crate-local test can only ever see
//! one side of it.
//!
//! The rows differ along what the rule generalises over: which of the four entity kinds the
//! template declares (a web ACL, a rule group, an IP set, a regex pattern set), the scope it
//! is created in (`REGIONAL` and `CLOUDFRONT`, whose ARNs differ in both the Region segment
//! and the scope segment), whether the template NAMES the entity or leaves the name to
//! CloudFormation, whether it declares rules at all and in which of the two spellings a
//! byte-match statement's search value is written (the bytes as text, and the already
//! base64-encoded form), how the ARN output is written (`Fn::GetAtt` of `Arn` and of
//! `<Type>Arn`), how many entities one stack declares, and which spelling the template is in
//! (JSON and YAML). Every expected value is computed from what the row declares plus the id
//! WAF minted, never read back out of the answer being checked.
//!
//! The association half and the deletion half are rows of the same rule: the web ACL ARN a
//! stack publishes is accepted by `AssociateWebACL` for a protected resource and reads the
//! web ACL back through `GetWebACL(ARN=…)`, and deleting the stack deletes the entities it
//! created, so WAF then answers `WAFNonexistentItemException` for them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate publishes ONCE, in `query_wire`: a suite that
// writes its own percent-encoder or element reader drifts from the others.
use crate::query_wire::{elements, form_encode, opt_element};

/// The region every request here is signed for, and so the region the stack's entities
/// belong to.
///
/// It is the region a CLOUDFRONT-scoped entity is reached through
/// ([`CLOUDFRONT_REGION`]), because one case of this suite declares such an entity:
/// WAF serves the global namespace at that endpoint ALONE ("API and SDKs - For all
/// calls, use the Region endpoint us-east-1", `Scope`'s own doc), so a stack that
/// declares a CLOUDFRONT-scoped web ACL has to run there — and a REGIONAL entity is
/// served at every endpoint, so the regional cases are unaffected.
const REGION: &str = CLOUDFRONT_REGION;

/// The one Region a CLOUDFRONT-scoped entity is reached through, and so the Region segment
/// of its ARN: "API and SDKs - For all calls, use the Region endpoint us-east-1" (`Scope`).
const CLOUDFRONT_REGION: &str = "us-east-1";

/// The two `Scope` values a WAFv2 entity is created in.
const SCOPE_REGIONAL: &str = "REGIONAL";
const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due. A WAFv2 entity has no transient status of its own: WAF answers each create with
/// a summary its own `Get…` reads at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
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

/// Call one WAFv2 operation over its awsJson1_1 wire.
async fn wafv2(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("wafv2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
        .header("authorization", authorization("wafv2"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One WAFv2 call that must succeed.
async fn waf_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = wafv2(router, operation, &request).await;
    assert!(
        status == 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of a WAFv2 refusal: awsJson1_1 carries it as the body's `__type`.
fn waf_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
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
    assert!(
        status == 200,
        "case {case:?}: expected HTTP 200 from CreateStack for a published resource type, got \
         {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert!(
        status == 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Whether `id` has the shape WAF's `EntityId` takes —
/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`, from that model — which is the form of an
/// id that service minted, and what distinguishes it from a name the template or
/// CloudFormation chose.
fn is_waf_entity_id(id: &str) -> bool {
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

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for a
/// resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

/// The ARN WAF gives an entity of `kind` named `name` with the minted `id`, in `scope`:
/// `arn:aws:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`, where a
/// CLOUDFRONT-scoped entity carries the `global` segment and the one Region that scope is
/// reached through. Written out here from that layout rather than taken from the answer.
fn expected_arn(scope: &str, kind: &str, name: &str, id: &str) -> String {
    let (region, segment) = if scope == SCOPE_CLOUDFRONT {
        (CLOUDFRONT_REGION, "global")
    } else {
        (REGION, "regional")
    };
    format!(
        "arn:aws:wafv2:{region}:{}:{segment}/{kind}/{name}/{id}",
        engine::emulator_account_id()
    )
}

/// An API Gateway stage a web ACL is put in front of, in the ARN format
/// `AssociateWebACLRequest$ResourceArn` documents for one
/// (`arn:aws:apigateway:<region>::/restapis/<api id>/stages/<stage name>`).
fn stage_arn(api_id: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api_id}/stages/{stage}")
}

/// What the row says about one WAFv2 entity the template declares.
struct DeclaredEntity {
    /// The resource's logical id, which a generated name is built from.
    logical_id: &'static str,
    /// The resource-path segment this entity's kind occupies in its ARN.
    kind: &'static str,
    /// The operation that reads one back, and the member it answers the entity under.
    get: &'static str,
    entity_member: &'static str,
    /// The `Scope` the template declares.
    scope: &'static str,
    /// The name the template declares, or `None` where CloudFormation generates it — in
    /// which case the row's template outputs the resource's `Name` under `name_output`.
    /// That key is read for that case alone, and is empty for a row that declares a name.
    declared_name: Option<&'static str>,
    name_output: &'static str,
    /// The output key carrying `{"Fn::GetAtt": [<logical id>, "Arn"]}`, and the ones that
    /// are `Fn::GetAtt` of the uniform `<Type>Arn` name and must carry the same ARN.
    arn_output: &'static str,
    typed_arn_outputs: &'static [&'static str],
    /// The output key carrying the entity's `Id`.
    id_output: &'static str,
    /// The bytes the entity's one byte-match rule searches for, where it declares one: a
    /// blob on the wire, which the template declares as the text itself or — for
    /// `already_encoded` — in the base64 spelling of those same bytes.
    search_string: Option<&'static str>,
    already_encoded: bool,
    /// Whether this entity is a web ACL a protected resource can be put behind: a REGIONAL
    /// one, CloudFront's being bound through the distribution's own configuration instead.
    associable: bool,
}

/// One row: a template and the WAFv2 entities it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    entities: &'static [DeclaredEntity],
}

const ROWS: &[Row] = &[
    Row {
        // The reported case: a stack whose web ACL blocks a marked request, with its ARN and
        // its id in the outputs.
        case: "a named REGIONAL web ACL with a header byte-match Block rule",
        stack_name: "wafv2-block-stack",
        template: r#"{"Resources":{"BlockingAcl":{"Type":"AWS::WAFv2::WebACL","Properties":{
               "Name":"stack-waf-block","Scope":"REGIONAL",
               "DefaultAction":{"Allow":{}},
               "VisibilityConfig":{"SampledRequestsEnabled":true,
                 "CloudWatchMetricsEnabled":true,"MetricName":"stackWafBlock"},
               "Rules":[{"Name":"BlockMarkedRequests","Priority":0,"Action":{"Block":{}},
                 "VisibilityConfig":{"SampledRequestsEnabled":true,
                   "CloudWatchMetricsEnabled":true,"MetricName":"blockMarked"},
                 "Statement":{"ByteMatchStatement":{"SearchString":"sim2real-block",
                   "FieldToMatch":{"SingleHeader":{"Name":"x-sim2real"}},
                   "TextTransformations":[{"Priority":0,"Type":"NONE"}],
                   "PositionalConstraint":"EXACTLY"}}}]}}},
             "Outputs":{"AclArn":{"Value":{"Fn::GetAtt":["BlockingAcl","Arn"]}},
               "AclTypedArn":{"Value":{"Fn::GetAtt":["BlockingAcl","WebACLArn"]}},
               "AclName":{"Value":{"Fn::GetAtt":["BlockingAcl","Name"]}},
               "AclId":{"Value":{"Fn::GetAtt":["BlockingAcl","Id"]}}}}"#,
        entities: &[DeclaredEntity {
            logical_id: "BlockingAcl",
            kind: "webacl",
            get: "GetWebACL",
            entity_member: "WebACL",
            scope: SCOPE_REGIONAL,
            declared_name: Some("stack-waf-block"),
            name_output: "AclName",
            arn_output: "AclArn",
            typed_arn_outputs: &["AclTypedArn"],
            id_output: "AclId",
            search_string: Some("sim2real-block"),
            already_encoded: false,
            associable: true,
        }],
    },
    Row {
        case: "an unnamed CLOUDFRONT web ACL with no rules, in the YAML spelling",
        stack_name: "wafv2-global-stack",
        template: concat!(
            "Resources:\n",
            "  GlobalAcl:\n",
            "    Type: AWS::WAFv2::WebACL\n",
            "    Properties:\n",
            "      Scope: CLOUDFRONT\n",
            "      DefaultAction:\n",
            "        Block: {}\n",
            "      VisibilityConfig:\n",
            "        SampledRequestsEnabled: false\n",
            "        CloudWatchMetricsEnabled: false\n",
            "        MetricName: globalAcl\n",
            "Outputs:\n",
            "  GlobalArn:\n",
            "    Value: !GetAtt GlobalAcl.Arn\n",
            "  GlobalName:\n",
            "    Value: !GetAtt GlobalAcl.Name\n",
            "  GlobalId:\n",
            "    Value: !GetAtt GlobalAcl.Id\n",
        ),
        entities: &[DeclaredEntity {
            logical_id: "GlobalAcl",
            kind: "webacl",
            get: "GetWebACL",
            entity_member: "WebACL",
            scope: SCOPE_CLOUDFRONT,
            declared_name: None,
            name_output: "GlobalName",
            arn_output: "GlobalArn",
            typed_arn_outputs: &[],
            id_output: "GlobalId",
            search_string: None,
            already_encoded: false,
            associable: false,
        }],
    },
    Row {
        case: "a named REGIONAL IP set and a named REGIONAL regex pattern set in one stack",
        stack_name: "wafv2-sets-stack",
        template: r#"{"Resources":{
               "Blocked":{"Type":"AWS::WAFv2::IPSet","Properties":{
                 "Name":"stack-blocked-addresses","Scope":"REGIONAL",
                 "IPAddressVersion":"IPV4",
                 "Addresses":["203.0.113.0/24","198.51.100.7/32"]}},
               "Patterns":{"Type":"AWS::WAFv2::RegexPatternSet","Properties":{
                 "Name":"stack-blocked-patterns","Scope":"REGIONAL",
                 "RegularExpressionList":[{"RegexString":"^/admin"},
                   {"RegexString":"\\.php$"}]}}},
             "Outputs":{"SetArn":{"Value":{"Fn::GetAtt":["Blocked","Arn"]}},
               "SetTypedArn":{"Value":{"Fn::GetAtt":["Blocked","IPSetArn"]}},
               "SetId":{"Value":{"Fn::GetAtt":["Blocked","Id"]}},
               "PatternsArn":{"Value":{"Fn::GetAtt":["Patterns","Arn"]}},
               "PatternsTypedArn":{"Value":{"Fn::GetAtt":["Patterns","RegexPatternSetArn"]}},
               "PatternsId":{"Value":{"Fn::GetAtt":["Patterns","Id"]}}}}"#,
        entities: &[
            DeclaredEntity {
                logical_id: "Blocked",
                kind: "ipset",
                get: "GetIPSet",
                entity_member: "IPSet",
                scope: SCOPE_REGIONAL,
                declared_name: Some("stack-blocked-addresses"),
                name_output: "",
                arn_output: "SetArn",
                typed_arn_outputs: &["SetTypedArn"],
                id_output: "SetId",
                search_string: None,
                already_encoded: false,
                associable: false,
            },
            DeclaredEntity {
                logical_id: "Patterns",
                kind: "regexpatternset",
                get: "GetRegexPatternSet",
                entity_member: "RegexPatternSet",
                scope: SCOPE_REGIONAL,
                declared_name: Some("stack-blocked-patterns"),
                name_output: "",
                arn_output: "PatternsArn",
                typed_arn_outputs: &["PatternsTypedArn"],
                id_output: "PatternsId",
                search_string: None,
                already_encoded: false,
                associable: false,
            },
        ],
    },
    Row {
        case: "an unnamed REGIONAL rule group whose byte-match rule declares its search \
               value already base64-encoded",
        stack_name: "wafv2-rule-group-stack",
        template: r#"{"Resources":{"Guard":{"Type":"AWS::WAFv2::RuleGroup","Properties":{
               "Scope":"REGIONAL","Capacity":100,
               "Description":"the rules this stack guards with",
               "VisibilityConfig":{"SampledRequestsEnabled":false,
                 "CloudWatchMetricsEnabled":false,"MetricName":"stackGuard"},
               "Rules":[{"Name":"CountProbes","Priority":3,"Action":{"Count":{}},
                 "VisibilityConfig":{"SampledRequestsEnabled":false,
                   "CloudWatchMetricsEnabled":false,"MetricName":"countProbes"},
                 "Statement":{"ByteMatchStatement":{
                   "SearchStringBase64":"L2FkbWluL3Byb2Jl",
                   "FieldToMatch":{"UriPath":{}},
                   "TextTransformations":[{"Priority":0,"Type":"LOWERCASE"}],
                   "PositionalConstraint":"STARTS_WITH"}}}]}}},
             "Outputs":{"GuardArn":{"Value":{"Fn::GetAtt":["Guard","Arn"]}},
               "GuardName":{"Value":{"Fn::GetAtt":["Guard","Name"]}},
               "GuardId":{"Value":{"Fn::GetAtt":["Guard","Id"]}}}}"#,
        entities: &[DeclaredEntity {
            logical_id: "Guard",
            kind: "rulegroup",
            get: "GetRuleGroup",
            entity_member: "RuleGroup",
            scope: SCOPE_REGIONAL,
            declared_name: None,
            name_output: "GuardName",
            arn_output: "GuardArn",
            typed_arn_outputs: &[],
            id_output: "GuardId",
            // `L2FkbWluL3Byb2Jl` is the base64 the template above declares, which is these
            // bytes.
            search_string: Some("/admin/probe"),
            already_encoded: true,
            associable: false,
        }],
    },
];

/// The ARN the entity document an answer carries reports for itself.
fn reported_arn(entity: &Value) -> String {
    entity
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `SearchString` of the first byte-match statement in an entity's rules, as the answer
/// carries it: a blob, so awsJson1_1 renders it base64.
fn reported_search_string(entity: &Value) -> Option<String> {
    entity
        .get("Rules")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|rule| {
            rule.pointer("/Statement/ByteMatchStatement/SearchString")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

#[tokio::test]
async fn a_completed_stack_publishes_the_wafv2_identifiers_waf_minted() {
    let router = build_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, row.template).await;
        assert!(
            opt_element(&body, "StackStatus").as_deref() == Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for entity in row.entities {
            let case = format!("{} [{}]", row.case, entity.logical_id);
            let output = |key: &str| {
                outputs
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                    .clone()
            };
            let id = output(entity.id_output);
            let arn = output(entity.arn_output);
            // The name the entity was created under: the declared one, or the
            // `<stack>-<logical id>-<suffix>` one CloudFormation generated and published.
            let name = match entity.declared_name {
                Some(declared) => declared.to_string(),
                None => {
                    let generated = output(entity.name_output);
                    assert!(
                        is_generated_name(&generated, row.stack_name, entity.logical_id),
                        "case {case:?}: expected a generated <stack>-<logical id>-<suffix> \
                         name, got {generated:?}"
                    );
                    generated
                }
            };

            // The id is one WAF minted, not the name the template asked for.
            assert!(
                is_waf_entity_id(&id),
                "case {case:?}: expected the published id to be a minted WAF entity id, got \
                 {id:?}"
            );
            assert!(
                id != name,
                "case {case:?}: expected the published id to be the minted entity id rather \
                 than the entity's name {name:?}"
            );

            // The published ARN is the one WAF builds from the scope, that name and that id —
            // the layout its API decomposes — under every name the type publishes it as.
            let expected = expected_arn(entity.scope, entity.kind, &name, &id);
            assert!(
                arn == expected,
                "case {case:?}: expected the published ARN to be {expected:?}, got {arn:?}"
            );
            for typed in entity.typed_arn_outputs {
                let reported = outputs.get(*typed);
                assert!(
                    reported.map(String::as_str) == Some(arn.as_str()),
                    "case {case:?}: expected the {typed} output to be the entity's ARN \
                     {arn:?}, got {reported:?}"
                );
            }

            // WAF itself reads the entity back under the triple the stack published, and
            // reports the same ARN for it.
            let answer = waf_ok(
                &router,
                &case,
                entity.get,
                json!({"Name": name.clone(), "Scope": entity.scope, "Id": id.clone()}),
            )
            .await;
            let document = answer
                .get(entity.entity_member)
                .cloned()
                .unwrap_or(Value::Null);
            assert!(
                reported_arn(&document) == expected,
                "case {case:?}: expected WAF to report the ARN {expected:?} for the entity, \
                 got {answer}"
            );

            // A byte-match statement's search value is a blob: whichever of the two
            // spellings the template declared it in, what WAF holds is the bytes declared.
            if let Some(declared) = entity.search_string {
                let encoded = aws_smithy_types::base64::encode(declared);
                let reported = reported_search_string(&document);
                let spelling = if entity.already_encoded {
                    "already encoded"
                } else {
                    "as text"
                };
                assert!(
                    reported.as_deref() == Some(encoded.as_str()),
                    "case {case:?}: expected the rule to search for the declared bytes \
                     ({declared:?}, declared {spelling}, {encoded:?} on the wire), got \
                     {reported:?}"
                );
            }

            // ...and a web ACL the stack published is usable through its own API under that
            // ARN: a protected resource is put behind it, the association is reported, and
            // the ARN alone reads the web ACL back.
            if entity.associable {
                let resource = stage_arn("a1b2c3d4e5", "prod");
                waf_ok(
                    &router,
                    &case,
                    "AssociateWebACL",
                    json!({"WebACLArn": arn.clone(), "ResourceArn": resource.clone()}),
                )
                .await;
                let listed = waf_ok(
                    &router,
                    &case,
                    "ListResourcesForWebACL",
                    json!({"WebACLArn": arn.clone(), "ResourceType": "API_GATEWAY"}),
                )
                .await;
                let behind: Vec<String> = listed
                    .get("ResourceArns")
                    .and_then(Value::as_array)
                    .map(|arns| {
                        arns.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                assert!(
                    behind == vec![resource.clone()],
                    "case {case:?}: expected the protected resource {resource:?} behind the \
                     published web ACL, got {behind:?}"
                );
                let by_arn = waf_ok(&router, &case, "GetWebACL", json!({"ARN": arn.clone()})).await;
                let read_back = by_arn.get("WebACL").cloned().unwrap_or(Value::Null);
                assert!(
                    reported_arn(&read_back) == arn,
                    "case {case:?}: expected GetWebACL(ARN) to read the web ACL the stack \
                     published, got {by_arn}"
                );
            }
        }
    }
}

/// One entity a deleted stack created, as the deletion half addresses it.
struct HeldEntity {
    get: &'static str,
    name: &'static str,
    id_output: &'static str,
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_wafv2_entities_it_created() {
    let router = build_router();
    let case = "a web ACL and an IP set deleted with their stack";
    let stack_name = "wafv2-deleted-stack";
    let template = r#"{"Resources":{
           "Acl":{"Type":"AWS::WAFv2::WebACL","Properties":{
             "Name":"deleted-stack-acl","Scope":"REGIONAL",
             "DefaultAction":{"Block":{}},
             "VisibilityConfig":{"SampledRequestsEnabled":false,
               "CloudWatchMetricsEnabled":false,"MetricName":"deletedStackAcl"}}},
           "Addresses":{"Type":"AWS::WAFv2::IPSet","Properties":{
             "Name":"deleted-stack-addresses","Scope":"REGIONAL",
             "IPAddressVersion":"IPV6","Addresses":["2001:db8::/32"]}}},
         "Outputs":{"AclId":{"Value":{"Fn::GetAtt":["Acl","Id"]}},
           "AddressesId":{"Value":{"Fn::GetAtt":["Addresses","Id"]}}}}"#;
    let held = [
        HeldEntity {
            get: "GetWebACL",
            name: "deleted-stack-acl",
            id_output: "AclId",
        },
        HeldEntity {
            get: "GetIPSet",
            name: "deleted-stack-addresses",
            id_output: "AddressesId",
        },
    ];

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert!(
        opt_element(&body, "StackStatus").as_deref() == Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let ids: Vec<String> = held
        .iter()
        .map(|entity| {
            outputs
                .get(entity.id_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", entity.id_output)
                })
                .clone()
        })
        .collect();
    for (entity, id) in held.iter().zip(&ids) {
        waf_ok(
            &router,
            case,
            entity.get,
            json!({"Name": entity.name, "Scope": SCOPE_REGIONAL, "Id": id}),
        )
        .await;
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert!(
        status == 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    for (entity, id) in held.iter().zip(&ids) {
        let (status, answer) = wafv2(
            &router,
            entity.get,
            &json!({"Name": entity.name, "Scope": SCOPE_REGIONAL, "Id": id}),
        )
        .await;
        assert!(
            status == 400 && waf_error_code(&answer) == "WAFNonexistentItemException",
            "case {case:?}: expected {} on the deleted {:?} to answer 400 \
             WAFNonexistentItemException, got {status} with {answer}",
            entity.get,
            entity.name
        );
    }
}
