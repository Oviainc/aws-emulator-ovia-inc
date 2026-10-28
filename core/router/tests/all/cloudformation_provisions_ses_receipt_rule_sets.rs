// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::SES::ReceiptRuleSet` and `AWS::SES::ReceiptRule` resources its template declares —
//! in Amazon SES.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns the type created it, so the handle the stack publishes for that resource
//! (`Ref`/`Fn::GetAtt`) addresses a resource that service really holds.** For a receipt rule
//! set the handle is its NAME and for a receipt rule the RULE's name, so once the create
//! waiter returns, the owning service's own reads (`DescribeReceiptRuleSet`,
//! `DescribeReceiptRule`) and the writes a client makes next (`SetActiveReceiptRuleSet`,
//! another `CreateReceiptRule` in the same set) answer for what the stack published —
//! rather than Amazon SES refusing `RuleSetDoesNotExist` for a set its own stack just
//! published. Two crates have to agree for that to hold (CloudFormation makes the call,
//! Amazon SES serves it), so the suite lives here: a crate-local test can only ever see one
//! side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the set
//! and the rule or leaves either name to CloudFormation, whether a name arrives through a
//! template parameter and an intrinsic rather than as a literal, how many rules one set
//! declares, where in the set's ORDER each declared rule lands (the default places a new
//! rule first, `After` places it behind the rule it names), and whether a rule carries the
//! nested structures of its own shape — a recipient list and an ordered action list. Every
//! expected value is computed from what the row declares, never read back out of the answer
//! being checked.
//!
//! The deletion half and the refusal half are the same rule read the other way: deleting the
//! stack deletes the rule set it created, and a resource Amazon SES REFUSES to create — or
//! one whose declaration describes no resource at all — leaves the stack
//! `ROLLBACK_COMPLETE` naming it, with nothing of the stack left behind, rather
//! than publishing a name nothing answers to. Drift detection reads the same question off
//! the owning service, so a rule set deleted out of band reports the set AND the rule it
//! carried as `DELETED`.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, elements, form_encode, members_of, opt_element, outputs_of};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// The region every request here is signed for, and so the region the stack's receipt rule
/// sets belong to. Deliberately NOT the emulator's default region: the call that creates the
/// set has to be made in the Region the stack was created in.
const REGION: &str = "us-west-2";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due. Neither type has a transient status of its own: Amazon SES answers
/// `CreateReceiptRuleSet` and `CreateReceiptRule` with a resource that is usable at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(engine),
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

/// Call one Amazon SES operation (API version 2010-12-01).
async fn ses(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "ses", "2010-12-01", action, members).await
}

/// One Amazon SES call that must be answered.
async fn ses_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = ses(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SES {action}, got {status} with {body}"
    );
    body
}

/// The awsQuery error `<Code>` of a refusal, or `""` on a success envelope.
fn error_code(xml: &str) -> String {
    opt_element(xml, "Code").unwrap_or_default()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
///
/// `parameters` are the template `Parameters` the `CreateStack` call supplies.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut owned: Vec<(String, String)> = vec![
        ("StackName".to_string(), stack_name.to_string()),
        ("TemplateBody".to_string(), template.to_string()),
    ];
    for (index, (key, value)) in parameters.iter().enumerate() {
        let position = index + 1;
        owned.push((
            format!("Parameters.member.{position}.ParameterKey"),
            (*key).to_string(),
        ));
        owned.push((
            format!("Parameters.member.{position}.ParameterValue"),
            (*value).to_string(),
        ));
    }
    let members: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
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

/// The name a stack output carries, or a panic quoting the whole answer.
fn output(outputs: &BTreeMap<String, String>, case: &str, key: &str, body: &str) -> String {
    outputs
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
        .clone()
}

/// Read the rule set back through the owning service.
async fn describe_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    ses(router, "DescribeReceiptRuleSet", &[("RuleSetName", name)]).await
}

/// [`describe_rule_set`] where the read must be answered.
async fn describe_rule_set_ok(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = describe_rule_set(router, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the rule set {name:?} back, got {status} \
         with {body}"
    );
    body
}

/// The names of the rules a `DescribeReceiptRuleSet` answer reports, in the order the set
/// applies them.
///
/// Read off the `Rules` block alone, so the set's own `Metadata` name is not one of them. An
/// action structure carries no `Name` member of its own (`HeaderName`, `BucketName`,
/// `TopicArn`), so every `Name` inside the block is one rule's.
fn rule_names(described: &str) -> Vec<String> {
    let Some(rules) = block_of(described, "Rules") else {
        return Vec::new();
    };
    elements(rules, "Name")
}

/// What the row says about one receipt rule the template declares.
struct DeclaredRule {
    /// The output key whose value is the rule's name (`{"Ref": <logical id>}`).
    name_output: &'static str,
    /// The resource's logical id, which a generated name is built from.
    logical_id: &'static str,
    /// The name the rule must end up carrying, or `None` where CloudFormation generates it
    /// (the declaration states none inside its `Rule`).
    expected_name: Option<&'static str>,
    /// Whether the rule is active, as the wire carries the declared boolean.
    expected_enabled: &'static str,
    /// Whether messages matching the rule are scanned, the same way.
    expected_scan_enabled: &'static str,
    /// The declared TLS policy, which every row states.
    expected_tls_policy: &'static str,
    /// The recipients the declaration lists, in order.
    expected_recipients: &'static [&'static str],
    /// The action structure each declared action carries, in the order declared.
    expected_actions: &'static [&'static str],
    /// The member of an action the declaration states, and the value it states — so a
    /// nested action structure is shown to reach the owning service as its own members
    /// rather than as the text of the document it was written as.
    expected_action_members: &'static [(&'static str, &'static str)],
}

/// One row: a template, the rule set it declares, and the rules that set must hold.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The template `Parameters` the `CreateStack` call supplies.
    parameters: &'static [(&'static str, &'static str)],
    /// The output key whose value is the rule set's name.
    set_name_output: &'static str,
    /// The rule set resource's logical id.
    set_logical_id: &'static str,
    /// The name the set must end up carrying, or `None` where CloudFormation generates it.
    expected_set_name: Option<&'static str>,
    /// The rules the set must hold, in the ORDER the set applies them — which is not the
    /// declaration order where a rule is placed first by default.
    rules: &'static [DeclaredRule],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named set with one named rule carrying recipients and two actions",
        stack_name: "inbound-rules-stack",
        template: r#"{"Resources":{
               "Inbound":{"Type":"AWS::SES::ReceiptRuleSet",
                 "Properties":{"RuleSetName":"stack-inbound-rules"}},
               "Stamp":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Inbound"},
                   "Rule":{"Name":"stamp-inbound","Enabled":true,"ScanEnabled":true,
                     "TlsPolicy":"Require",
                     "Recipients":["sales@example.com","example.com"],
                     "Actions":[{"AddHeaderAction":{"HeaderName":"X-Stack-Origin",
                         "HeaderValue":"cloudformation"}},
                       {"StopAction":{"Scope":"RuleSet"}}]}}}},
             "Outputs":{"SetName":{"Value":{"Ref":"Inbound"}},
               "StampName":{"Value":{"Ref":"Stamp"}}}}"#,
        parameters: &[],
        set_name_output: "SetName",
        set_logical_id: "Inbound",
        expected_set_name: Some("stack-inbound-rules"),
        rules: &[DeclaredRule {
            name_output: "StampName",
            logical_id: "Stamp",
            expected_name: Some("stamp-inbound"),
            expected_enabled: "true",
            expected_scan_enabled: "true",
            expected_tls_policy: "Require",
            expected_recipients: &["sales@example.com", "example.com"],
            expected_actions: &["AddHeaderAction", "StopAction"],
            expected_action_members: &[
                ("HeaderName", "X-Stack-Origin"),
                ("HeaderValue", "cloudformation"),
                ("Scope", "RuleSet"),
            ],
        }],
    },
    Row {
        // Neither resource is named by the template, so both names are CloudFormation's to
        // generate — and the rule's has to reach the owning service, whose `Rule.Name` is
        // required.
        case: "an unnamed set and an unnamed rule, both named by CloudFormation",
        stack_name: "unnamed-rules-stack",
        template: r#"{"Resources":{
               "Plain":{"Type":"AWS::SES::ReceiptRuleSet"},
               "Catchall":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Plain"},
                   "Rule":{"Enabled":false,"ScanEnabled":false,"TlsPolicy":"Optional",
                     "Actions":[{"StopAction":{"Scope":"RuleSet"}}]}}}},
             "Outputs":{"SetName":{"Value":{"Ref":"Plain"}},
               "CatchallName":{"Value":{"Ref":"Catchall"}}}}"#,
        parameters: &[],
        set_name_output: "SetName",
        set_logical_id: "Plain",
        expected_set_name: None,
        rules: &[DeclaredRule {
            name_output: "CatchallName",
            logical_id: "Catchall",
            expected_name: None,
            expected_enabled: "false",
            expected_scan_enabled: "false",
            expected_tls_policy: "Optional",
            expected_recipients: &[],
            expected_actions: &["StopAction"],
            expected_action_members: &[("Scope", "RuleSet")],
        }],
    },
    Row {
        // The set's name arrives through a template parameter and an intrinsic rather than as
        // a literal, and neither rule names a predecessor: `After` being absent, "the new
        // rule is inserted at the beginning of the rule list", so the set applies them in the
        // REVERSE of the order the template declares them.
        case: "a parameterised set with two rules, neither naming a predecessor",
        stack_name: "prepended-rules-stack",
        template: r#"{"Parameters":{"SetName":{"Type":"String"}},
             "Resources":{
               "Named":{"Type":"AWS::SES::ReceiptRuleSet",
                 "Properties":{"RuleSetName":{"Ref":"SetName"}}},
               "Alpha":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Named"},
                   "Rule":{"Name":"rule-alpha","Enabled":true,"ScanEnabled":false,
                     "TlsPolicy":"Optional","Recipients":["alpha@example.com"],
                     "Actions":[{"AddHeaderAction":{"HeaderName":"X-Alpha",
                       "HeaderValue":"first-declared"}}]}}},
               "Beta":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Named"},
                   "Rule":{"Name":"rule-beta","Enabled":false,"ScanEnabled":true,
                     "TlsPolicy":"Require","Recipients":["beta@example.com"],
                     "Actions":[{"StopAction":{"Scope":"RuleSet"}}]}}}},
             "Outputs":{"TheSet":{"Value":{"Ref":"Named"}},
               "AlphaName":{"Value":{"Ref":"Alpha"}},
               "BetaName":{"Value":{"Ref":"Beta"}}}}"#,
        parameters: &[("SetName", "stack_parameterised_rules")],
        set_name_output: "TheSet",
        set_logical_id: "Named",
        expected_set_name: Some("stack_parameterised_rules"),
        rules: &[
            DeclaredRule {
                name_output: "BetaName",
                logical_id: "Beta",
                expected_name: Some("rule-beta"),
                expected_enabled: "false",
                expected_scan_enabled: "true",
                expected_tls_policy: "Require",
                expected_recipients: &["beta@example.com"],
                expected_actions: &["StopAction"],
                expected_action_members: &[("Scope", "RuleSet")],
            },
            DeclaredRule {
                name_output: "AlphaName",
                logical_id: "Alpha",
                expected_name: Some("rule-alpha"),
                expected_enabled: "true",
                expected_scan_enabled: "false",
                expected_tls_policy: "Optional",
                expected_recipients: &["alpha@example.com"],
                expected_actions: &["AddHeaderAction"],
                expected_action_members: &[
                    ("HeaderName", "X-Alpha"),
                    ("HeaderValue", "first-declared"),
                ],
            },
        ],
    },
    Row {
        // The second rule names the first as the rule it goes AFTER — through a reference to
        // that resource, so what reaches the owning service is the name the first rule was
        // really created under — and the set then applies them in declaration order.
        case: "two rules the second of which is placed after the first",
        stack_name: "ordered-rules-stack",
        template: r#"{"Resources":{
               "Ordered":{"Type":"AWS::SES::ReceiptRuleSet",
                 "Properties":{"RuleSetName":"stack-ordered-rules"}},
               "First":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Ordered"},
                   "Rule":{"Name":"rule-first","Enabled":true,"ScanEnabled":true,
                     "TlsPolicy":"Optional","Recipients":["first@example.com"],
                     "Actions":[{"AddHeaderAction":{"HeaderName":"X-Order",
                       "HeaderValue":"first"}}]}}},
               "Second":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Ordered"},"After":{"Ref":"First"},
                   "Rule":{"Name":"rule-second","Enabled":true,"ScanEnabled":false,
                     "TlsPolicy":"Require","Recipients":["second@example.com"],
                     "Actions":[{"StopAction":{"Scope":"RuleSet"}}]}}}},
             "Outputs":{"TheSet":{"Value":{"Ref":"Ordered"}},
               "FirstName":{"Value":{"Ref":"First"}},
               "SecondName":{"Value":{"Ref":"Second"}}}}"#,
        parameters: &[],
        set_name_output: "TheSet",
        set_logical_id: "Ordered",
        expected_set_name: Some("stack-ordered-rules"),
        rules: &[
            DeclaredRule {
                name_output: "FirstName",
                logical_id: "First",
                expected_name: Some("rule-first"),
                expected_enabled: "true",
                expected_scan_enabled: "true",
                expected_tls_policy: "Optional",
                expected_recipients: &["first@example.com"],
                expected_actions: &["AddHeaderAction"],
                expected_action_members: &[
                    ("HeaderName", "X-Order"),
                    ("HeaderValue", "first"),
                ],
            },
            DeclaredRule {
                name_output: "SecondName",
                logical_id: "Second",
                expected_name: Some("rule-second"),
                expected_enabled: "true",
                expected_scan_enabled: "false",
                expected_tls_policy: "Require",
                expected_recipients: &["second@example.com"],
                expected_actions: &["StopAction"],
                expected_action_members: &[("Scope", "RuleSet")],
            },
        ],
    },
];

#[tokio::test]
async fn a_completed_stack_published_receipt_rule_sets_ses_really_holds() {
    for row in ROWS {
        let case = row.case;
        let router = build_router();

        let body =
            create_and_settle(&router, case, row.stack_name, row.template, row.parameters).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );
        let outputs = outputs_of(&body);

        let set_name = output(&outputs, case, row.set_name_output, &body);
        match row.expected_set_name {
            Some(expected) => assert_eq!(
                set_name, expected,
                "case {case:?}: expected the declared rule set name {expected:?}, got \
                 {set_name:?}"
            ),
            None => assert!(
                is_generated_name(&set_name, row.stack_name, row.set_logical_id),
                "case {case:?}: expected a generated <stack>-<logical id>-<suffix> rule set \
                 name, got {set_name:?}"
            ),
        }

        // The owning service really holds the set, under the name the stack published, and
        // it holds exactly the rules the template declared for it, in the order the set
        // applies them.
        let described = describe_rule_set_ok(&router, case, &set_name).await;
        assert_eq!(
            block_of(&described, "Metadata").and_then(|metadata| opt_element(metadata, "Name")),
            Some(set_name.clone()),
            "case {case:?}: expected SES to report the set as {set_name:?}, got {described}"
        );
        let mut expected_names = Vec::new();
        for rule in row.rules {
            let name = output(&outputs, case, rule.name_output, &body);
            match rule.expected_name {
                Some(expected) => assert_eq!(
                    name, expected,
                    "case {case:?}: expected the declared rule name {expected:?}, got {name:?}"
                ),
                None => assert!(
                    is_generated_name(&name, row.stack_name, rule.logical_id),
                    "case {case:?}: expected a generated <stack>-<logical id>-<suffix> rule \
                     name, got {name:?}"
                ),
            }
            expected_names.push(name);
        }
        assert_eq!(
            rule_names(&described),
            expected_names,
            "case {case:?}: expected the set to hold the declared rules in that order, got \
             {described}"
        );

        // Each rule is the rule its own declaration asked for: the owning service's own
        // per-rule read answers with the flags, the recipients and the ordered actions the
        // template stated, including the nested structure of each action.
        for (rule, name) in row.rules.iter().zip(&expected_names) {
            let read = ses_ok(
                &router,
                case,
                "DescribeReceiptRule",
                &[("RuleSetName", &set_name), ("RuleName", name)],
            )
            .await;
            for (member, expected) in [
                ("Enabled", rule.expected_enabled),
                ("ScanEnabled", rule.expected_scan_enabled),
                ("TlsPolicy", rule.expected_tls_policy),
            ] {
                assert_eq!(
                    elements(&read, member).first().map(String::as_str),
                    Some(expected),
                    "case {case:?}: expected {name:?} to report {member}={expected:?}, got {read}"
                );
            }
            let recipients: Vec<String> = block_of(&read, "Recipients")
                .map(|block| {
                    members_of(block)
                        .into_iter()
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                recipients, rule.expected_recipients,
                "case {case:?}: expected {name:?} to report the declared recipients, got {read}"
            );
            let actions: Vec<Vec<String>> = block_of(&read, "Actions")
                .map(|block| {
                    members_of(block)
                        .into_iter()
                        .map(|action| {
                            rule.expected_actions
                                .iter()
                                .filter(|kind| action.contains(&format!("<{kind}")))
                                .map(ToString::to_string)
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default();
            let expected_actions: Vec<Vec<String>> = rule
                .expected_actions
                .iter()
                .map(|kind| vec![(*kind).to_string()])
                .collect();
            assert_eq!(
                actions, expected_actions,
                "case {case:?}: expected {name:?} to report the declared actions in that \
                 order, one structure each, got {read}"
            );
            // Each member of a declared action reaches the owning service as that member,
            // carrying the value the template stated.
            for (member, expected) in rule.expected_action_members {
                assert_eq!(
                    elements(&read, member).first().map(String::as_str),
                    Some(*expected),
                    "case {case:?}: expected {name:?}'s action to carry \
                     {member}={expected:?}, got {read}"
                );
            }
        }

        // The set the stack published is activatable: the write a client turns email
        // receiving onto it with lands on it, and the read of the ACTIVE set answers with it.
        ses_ok(
            &router,
            case,
            "SetActiveReceiptRuleSet",
            &[("RuleSetName", &set_name)],
        )
        .await;
        let active = ses_ok(&router, case, "DescribeActiveReceiptRuleSet", &[]).await;
        assert_eq!(
            block_of(&active, "Metadata").and_then(|metadata| opt_element(metadata, "Name")),
            Some(set_name.clone()),
            "case {case:?}: expected the stack's set to be the active one, got {active}"
        );

        // And an out-of-band write adds a rule to the set the stack published — a set Amazon
        // SES did not hold would refuse this as RuleSetDoesNotExist.
        ses_ok(
            &router,
            case,
            "CreateReceiptRule",
            &[
                ("RuleSetName", &set_name),
                ("Rule.Name", "added-out-of-band"),
                ("Rule.Enabled", "true"),
            ],
        )
        .await;
        let reread = describe_rule_set_ok(&router, case, &set_name).await;
        assert!(
            rule_names(&reread).contains(&"added-out-of-band".to_string()),
            "case {case:?}: expected the out-of-band rule to be readable in the stack's set, \
             got {reread}"
        );

        // The stack still reports the resources it created, under the same physical ids: a
        // write through Amazon SES changes the rule set, not the stack's record of it.
        let (status, reported) = cloudformation(
            &router,
            "DescribeStackResources",
            &[("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             {reported}"
        );
        let physical = elements(&reported, "PhysicalResourceId");
        for expected in std::iter::once(&set_name).chain(expected_names.iter()) {
            assert!(
                physical.contains(expected),
                "case {case:?}: expected the stack to still report {expected:?}, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_rule_sets_it_created() {
    let router = build_router();
    let case = "two rule sets deleted with their stack";
    let stack_name = "deleted-rules-stack";
    let template = r#"{"Resources":{
           "Kept":{"Type":"AWS::SES::ReceiptRuleSet",
             "Properties":{"RuleSetName":"deleted-stack-kept"}},
           "Other":{"Type":"AWS::SES::ReceiptRuleSet"}},
         "Outputs":{"KeptName":{"Value":{"Ref":"Kept"}},
           "OtherName":{"Value":{"Ref":"Other"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let names: Vec<String> = ["KeptName", "OtherName"]
        .iter()
        .map(|key| output(&outputs, case, key, &body))
        .collect();
    for name in &names {
        describe_rule_set_ok(&router, case, name).await;
    }

    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // Every set the stack created is gone from Amazon SES, which is what makes the stack's
    // resources the stack's own rather than leftovers.
    for name in &names {
        let (status, answer) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the rule set {name:?} to be gone, got {status} with \
             {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "RuleSetDoesNotExist",
            "case {case:?}: expected RuleSetDoesNotExist for {name:?}, got {answer}"
        );
    }
}

/// One row of the refusal half: a declaration the owning service will not create.
struct RefusedRow {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The logical id of the resource whose create must fail.
    failing_logical_id: &'static str,
    /// Text the failing resource's own event must carry — the owning service's own refusal.
    reason_fragment: &'static str,
    /// The rule set name no read may answer for afterwards: the one the row declares.
    absent_rule_set: &'static str,
}

const REFUSED_ROWS: &[RefusedRow] = &[
    RefusedRow {
        // `ReceiptRuleSetName` must "contain only ASCII letters (a-z, A-Z), numbers (0-9),
        // underscores (_), or dashes (-)" — a request Amazon SES's own CreateReceiptRuleSet
        // refuses, which is what the stack must report rather than completing.
        case: "a rule set name carrying a character the owning service's name rule excludes",
        stack_name: "refused-set-stack",
        template: r#"{"Resources":{"Bad":{"Type":"AWS::SES::ReceiptRuleSet",
               "Properties":{"RuleSetName":"refused.stack.rules"}}}}"#,
        failing_logical_id: "Bad",
        reason_fragment: "RuleSetName must contain only ASCII letters",
        absent_rule_set: "refused.stack.rules",
    },
    RefusedRow {
        // `ReceiptAction` documents that "an instance of this data type can represent only
        // one action", so an entry setting two is a rule Amazon SES refuses — and the rule
        // set created before it is rolled back with the rest of the stack.
        case: "a rule whose action entry sets two actions",
        stack_name: "refused-rule-stack",
        template: r#"{"Resources":{
               "Holder":{"Type":"AWS::SES::ReceiptRuleSet",
                 "Properties":{"RuleSetName":"refused-rule-holder"}},
               "TwoActions":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Holder"},
                   "Rule":{"Name":"two-actions","Enabled":true,
                     "Actions":[{"StopAction":{"Scope":"RuleSet"},
                       "AddHeaderAction":{"HeaderName":"X-Two","HeaderValue":"actions"}}]}}}}}"#,
        failing_logical_id: "TwoActions",
        reason_fragment: "must set exactly one action",
        absent_rule_set: "refused-rule-holder",
    },
    RefusedRow {
        // The type requires the `Rule` it creates, so a declaration carrying no such
        // structure describes no rule: the resource fails rather than reaching Amazon SES as
        // a rule carrying nothing but the name CloudFormation decided.
        case: "a rule resource declaring no rule at all",
        stack_name: "ruleless-rule-stack",
        template: r#"{"Resources":{
               "Holder":{"Type":"AWS::SES::ReceiptRuleSet",
                 "Properties":{"RuleSetName":"ruleless-rule-holder"}},
               "Ruleless":{"Type":"AWS::SES::ReceiptRule",
                 "Properties":{"RuleSetName":{"Ref":"Holder"}}}}}"#,
        failing_logical_id: "Ruleless",
        reason_fragment: "declares no Rule",
        absent_rule_set: "ruleless-rule-holder",
    },
];

#[tokio::test]
async fn a_resource_ses_refuses_to_create_fails_the_stack_and_leaves_nothing_behind() {
    for row in REFUSED_ROWS {
        let case = row.case;
        let router = build_router();

        let body = create_and_settle(&router, case, row.stack_name, row.template, &[]).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than publish \
             a name nothing answers to, got {body}"
        );
        // The STACK's reason summarises which resource failed; the owning service's own
        // refusal is reported on that resource's event.
        let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            reason,
            resource_failure_summary(failed_action::CREATE, [row.failing_logical_id]),
            "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
        );
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack_name)]).await;
        assert!(
            elements(&events, "ResourceStatusReason")
                .iter()
                .any(|reported| reported.contains(row.reason_fragment)),
            "case {case:?}: expected the failing resource's event to carry the owning \
             service's own refusal, got {events}"
        );

        // Nothing of the stack is left in the owning service: neither the resource it
        // refused nor one it had already created before the refusal.
        let (status, answer) = describe_rule_set(&router, row.absent_rule_set).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected no rule set {:?} to remain, got {status} with {answer}",
            row.absent_rule_set
        );
        assert_eq!(
            error_code(&answer),
            "RuleSetDoesNotExist",
            "case {case:?}: expected RuleSetDoesNotExist for {:?}, got {answer}",
            row.absent_rule_set
        );
    }
}

/// Drift detection asks Amazon SES what the stack's resources ARE now, so a rule set deleted
/// out of band — which takes "all of the receipt rules it contains" with it — is reported
/// `DELETED` for the set AND for the rule it carried.
#[tokio::test]
async fn a_rule_set_deleted_out_of_band_is_detected_as_deleted() {
    let router = build_router();
    let case = "the declared rule set deleted through DeleteReceiptRuleSet";
    let stack_name = "drifted-rules-stack";
    let template = r#"{"Resources":{
           "Gone":{"Type":"AWS::SES::ReceiptRuleSet",
             "Properties":{"RuleSetName":"drifted-stack-rules"}},
           "Carried":{"Type":"AWS::SES::ReceiptRule",
             "Properties":{"RuleSetName":{"Ref":"Gone"},
               "Rule":{"Name":"carried-rule","Enabled":true,
                 "Actions":[{"StopAction":{"Scope":"RuleSet"}}]}}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );

    ses_ok(
        &router,
        case,
        "DeleteReceiptRuleSet",
        &[("RuleSetName", "drifted-stack-rules")],
    )
    .await;

    let (status, body) =
        cloudformation(&router, "DetectStackDrift", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DetectStackDrift, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let (status, body) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    assert_eq!(
        opt_element(&body, "StackDriftStatus").as_deref(),
        Some("DRIFTED"),
        "case {case:?}: expected the stack to report the drift its resources carry, got {body}"
    );

    let (status, body) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with {body}"
    );
    let reported: BTreeMap<String, String> = elements(&body, "LogicalResourceId")
        .into_iter()
        .zip(elements(&body, "StackResourceDriftStatus"))
        .collect();
    for logical_id in ["Gone", "Carried"] {
        assert_eq!(
            reported.get(logical_id).map(String::as_str),
            Some("DELETED"),
            "case {case:?}: expected {logical_id} to be DELETED, got {reported:?}"
        );
    }
}
