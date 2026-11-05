// SPDX-License-Identifier: CC0-1.0

//! The Amazon Bedrock AgentCore SANDBOX resource types in a stack's `Outputs`.
//!
//! The rule under test is the one `stack_outputs.rs` states, applied to the AgentCore
//! types the return-value catalogue did not carry: `DescribeStacks` reports each declared
//! `Output` as a COMPLETE pair — an `OutputKey` AND its resolved `OutputValue` — and the
//! value of a `Ref`/`Fn::GetAtt` output is the return value the resource TYPE publishes,
//! in the identifier and ARN form the owning API's model fixes, so a client that deploys
//! a template can read the identifiers of what it just created. An expression whose value
//! nothing here can compute is reported with its key and NO value instead of an invented
//! one.
//!
//! What the rows span: both sandbox types (`AWS::BedrockAgentCore::CodeInterpreter` and
//! `AWS::BedrockAgentCore::Browser`), `Ref` and each `Fn::GetAtt` attribute, both template
//! spellings, a name the identifier's alphabet admits as it is and one it does not, a
//! named and an unnamed resource, and an attribute nothing publishes. The forms asserted
//! are the ones the AgentCore control plane's own model fixes —
//! `smithy_query(query="operation",
//! operation="CreateCodeInterpreter,CreateBrowser,CreateAgentRuntime")` on
//! `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol`
//! — never a literal taken from one recorded stack.

use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_text,
};
use crate::stack_outputs::{
    agentcore_id_parts, is_agentcore_arn, is_agentcore_runtime_id, output_for, wire_outputs,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The LABEL half of an AgentCore identifier, when `value` is one at all.
///
/// The identifier and ARN FORMS are `stack_outputs.rs`'s — `agentcore_id_parts` and
/// `is_agentcore_runtime_id`, which is the `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$`
/// form `CodeInterpreterId` and `BrowserId` share with `AgentRuntimeId`, and
/// `is_agentcore_arn` — so this suite holds no second copy of them. What it adds is the
/// LABEL check those predicates cannot express: that the label is the name the template
/// declared, reduced to the identifier's alphabet.
fn label_of(value: &str) -> Option<&str> {
    agentcore_id_parts(value).map(|(label, _)| label)
}

/// The id portion of an AgentCore ARN, `arn:…:<segment>/<id>`.
fn id_in_arn(value: &str) -> Option<&str> {
    value.rsplit('/').next()
}

/// What one declared output of an AgentCore stack must carry.
enum Expect {
    /// Exactly this text.
    Exact(String),
    /// A modelled identifier whose label is EXACTLY this one — the name the template
    /// declared, reduced to the identifier's alphabet — and whose suffix no row can spell.
    Id(&'static str),
    /// A modelled identifier whose label BEGINS with this, for a resource the template
    /// does not name: CloudFormation generates `<stack>-<logical id>-<unique id>`, whose
    /// `-` the alphabet does not admit and whose digest no row can spell.
    IdLabelled(&'static str),
    /// An AgentCore ARN whose resource portion is `<segment>/<a modelled id labelled
    /// with this>`.
    Arn {
        segment: &'static str,
        label: &'static str,
    },
    /// The output is reported (its key is there) and carries NO value, because nothing
    /// here can compute the expression.
    NoValue,
}

impl Expect {
    /// Whether `value` satisfies this expectation, and the English of what was required.
    fn check(&self, value: Option<&str>) -> (bool, String) {
        match self {
            Self::Exact(text) => (value == Some(text.as_str()), format!("exactly {text:?}")),
            Self::Id(label) => (
                value.is_some_and(|actual| {
                    is_agentcore_runtime_id(actual) && label_of(actual) == Some(*label)
                }),
                format!("a modelled identifier labelled {label:?} + `-` + 10 characters"),
            ),
            Self::IdLabelled(prefix) => (
                value.is_some_and(|actual| {
                    is_agentcore_runtime_id(actual)
                        && label_of(actual).is_some_and(|label| label.starts_with(prefix))
                }),
                format!("a modelled identifier whose label begins {prefix:?}"),
            ),
            Self::Arn { segment, label } => {
                let ok = value.is_some_and(|actual| {
                    is_agentcore_arn(actual, segment, is_agentcore_runtime_id)
                        && id_in_arn(actual).and_then(label_of) == Some(*label)
                });
                (
                    ok,
                    format!(
                        "an agentcore {segment} ARN naming a modelled identifier labelled \
                         {label:?}"
                    ),
                )
            }
            Self::NoValue => (value.is_none(), "no OutputValue at all".to_string()),
        }
    }
}

/// One row: a template, and what each of its declared outputs must carry.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    expected: Vec<(&'static str, Expect)>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // A code interpreter and a browser named by the template, in the YAML
            // spelling a CFN-driven integration writes. `Ref` answers the type's primary
            // identifier, its own id, and `Fn::GetAtt` the id and the ARN; the `-custom`
            // ARN segment is the one a CUSTOMER-created sandbox carries (the bare
            // `code-interpreter`/`browser` segment is the service's built-in
            // `aws.codeinterpreter.v1` / `aws.browser.v1`, which no template declares).
            case: "a named code interpreter and browser: id, arn and Ref",
            stack_name: "agentcore-sandboxes",
            template: concat!(
                "Resources:\n",
                "  Interpreter:\n",
                "    Type: AWS::BedrockAgentCore::CodeInterpreter\n",
                "    Properties:\n",
                "      Name: probeInterpreter\n",
                "  Browser:\n",
                "    Type: AWS::BedrockAgentCore::Browser\n",
                "    Properties:\n",
                // A `_` IS inside the label alphabet, so it survives as it is.
                "      Name: probe_browser\n",
                "Outputs:\n",
                "  InterpreterId:\n",
                "    Value: !GetAtt Interpreter.CodeInterpreterId\n",
                "  InterpreterArn:\n",
                "    Value: !GetAtt Interpreter.CodeInterpreterArn\n",
                "  InterpreterRef:\n",
                "    Value: !Ref Interpreter\n",
                "  BrowserId:\n",
                "    Value: !GetAtt Browser.BrowserId\n",
                "  BrowserArn:\n",
                "    Value: !GetAtt Browser.BrowserArn\n",
                "  BrowserRef:\n",
                "    Value: !Ref Browser\n",
            ),
            expected: vec![
                ("InterpreterId", Expect::Id("probeInterpreter")),
                (
                    "InterpreterArn",
                    Expect::Arn {
                        segment: "code-interpreter-custom",
                        label: "probeInterpreter",
                    },
                ),
                ("InterpreterRef", Expect::Id("probeInterpreter")),
                ("BrowserId", Expect::Id("probe_browser")),
                (
                    "BrowserArn",
                    Expect::Arn {
                        segment: "browser-custom",
                        label: "probe_browser",
                    },
                ),
                ("BrowserRef", Expect::Id("probe_browser")),
            ],
        },
        Case {
            // The JSON spelling with long-form intrinsics, and the OTHER half of the
            // naming rule: `AWS::BedrockAgentCore::Runtime` takes its name from
            // `AgentRuntimeName` — the member `CreateAgentRuntime` takes — which is
            // neither the conventional `RuntimeName` nor plain `Name`, so a stack that
            // names its runtime that way must still get an id labelled with that name.
            case: "json, long-form GetAtt: a runtime named by AgentRuntimeName",
            stack_name: "agentcore-runtime",
            template: r#"{"Resources":{
                 "Runtime":{"Type":"AWS::BedrockAgentCore::Runtime",
                   "Properties":{"AgentRuntimeName":"probeRuntime"}}},
               "Outputs":{
                 "RuntimeId":{"Value":{"Fn::GetAtt":["Runtime","AgentRuntimeId"]}},
                 "RuntimeArn":{"Value":{"Fn::GetAtt":["Runtime","AgentRuntimeArn"]}},
                 "RuntimeVersion":{"Value":{"Fn::GetAtt":["Runtime","AgentRuntimeVersion"]}}}}"#,
            expected: vec![
                ("RuntimeId", Expect::Id("probeRuntime")),
                (
                    "RuntimeArn",
                    Expect::Arn {
                        segment: "runtime",
                        label: "probeRuntime",
                    },
                ),
                // `CreateAgentRuntime` answers an `agentRuntimeVersion` matching
                // `^([1-9][0-9]{0,4})$`; a create publishes the first version.
                ("RuntimeVersion", Expect::Exact("1".to_string())),
            ],
        },
        Case {
            // A declared name the identifier's alphabet does NOT admit, and one that does
            // not start with a letter either: the label keeps only what the pattern
            // admits and takes a leading letter, so the identifier is one the model allows
            // rather than one it forbids.
            case: "a declared name outside the identifier's alphabet",
            stack_name: "agentcore-awkward-name",
            template: concat!(
                "Resources:\n",
                "  Interpreter:\n",
                "    Type: AWS::BedrockAgentCore::CodeInterpreter\n",
                "    Properties:\n",
                "      Name: 9probe..interp-2.\n",
                "Outputs:\n",
                "  InterpreterId:\n",
                "    Value: !GetAtt Interpreter.CodeInterpreterId\n",
            ),
            expected: vec![("InterpreterId", Expect::Id("c9probeinterp2"))],
        },
        Case {
            // An UNNAMED sandbox still resolves: it takes CloudFormation's generated
            // `<stack name>-<logical id>-<unique id>` name, whose `-` the alphabet does
            // not admit, so the label is that name reduced — never reported as no value,
            // and never an identifier the model forbids.
            case: "an unnamed browser still publishes a modelled arn",
            stack_name: "agentcore-unnamed",
            template: concat!(
                "Resources:\n",
                "  Anon:\n",
                "    Type: AWS::BedrockAgentCore::Browser\n",
                "Outputs:\n",
                "  BrowserId:\n",
                "    Value: !GetAtt Anon.BrowserId\n",
                "  BrowserRef:\n",
                "    Value: !Ref Anon\n",
            ),
            expected: vec![
                ("BrowserId", Expect::IdLabelled("agentcoreunnamedAnon")),
                ("BrowserRef", Expect::IdLabelled("agentcoreunnamedAnon")),
            ],
        },
        Case {
            // The complement, which keeps the rule honest: an attribute NOTHING here can
            // compute is reported with its key and no value. A sandbox's RUNNING state is
            // one — nothing here provisioned a sandbox to be in a state.
            case: "an attribute nothing can compute carries no value",
            stack_name: "agentcore-unknowable",
            template: concat!(
                "Resources:\n",
                "  Interpreter:\n",
                "    Type: AWS::BedrockAgentCore::CodeInterpreter\n",
                "    Properties:\n",
                "      Name: probeInterpreter\n",
                "Outputs:\n",
                "  InterpreterId:\n",
                "    Value: !GetAtt Interpreter.CodeInterpreterId\n",
                "  Status:\n",
                "    Value: !GetAtt Interpreter.Status\n",
            ),
            expected: vec![
                ("InterpreterId", Expect::Id("probeInterpreter")),
                ("Status", Expect::NoValue),
            ],
        },
    ]
}

#[tokio::test]
async fn an_agentcore_stacks_declared_outputs_carry_their_resolved_values() {
    let router = router();

    for case in cases() {
        let (status_code, body) = create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected CreateStack to succeed, got {status_code} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status_code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        for (key, expected) in &case.expected {
            let output = output_for(&outputs, key).unwrap_or_else(|| {
                panic!(
                    "case {:?}: output {key:?} must be reported, got {outputs:?}",
                    case.case
                )
            });
            let (ok, required) = expected.check(output.value.as_deref());
            assert!(
                ok,
                "case {:?}: output {key:?} must carry {required}, got {:?}",
                case.case, output.value
            );
        }
    }
}

/// Two sandboxes of one type in one stack get DIFFERENT identifiers, and a second read of
/// the same stack reports the SAME ones: an identifier is derived from the resource, not
/// drawn fresh per read.
#[tokio::test]
async fn agentcore_sandbox_identifiers_are_unique_per_resource_and_stable_across_reads() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  First:\n",
        "    Type: AWS::BedrockAgentCore::CodeInterpreter\n",
        "  Second:\n",
        "    Type: AWS::BedrockAgentCore::CodeInterpreter\n",
        "Outputs:\n",
        "  FirstId:\n",
        "    Value: !GetAtt First.CodeInterpreterId\n",
        "  SecondId:\n",
        "    Value: !GetAtt Second.CodeInterpreterId\n",
    );
    let (status_code, body) = create_stack(&router, "agentcore-sandbox-ids", template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"two code interpreters\": expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let (status_code, body) = describe_stack(&router, "agentcore-sandbox-ids").await;
    assert_eq!(
        status_code, 200,
        "case \"two code interpreters\": expected DescribeStacks to succeed, got {status_code} \
         {body}"
    );
    let outputs = wire_outputs(&body);
    let first = output_for(&outputs, "FirstId").and_then(|output| output.value.clone());
    let second = output_for(&outputs, "SecondId").and_then(|output| output.value.clone());
    assert!(
        first.as_deref().is_some_and(is_agentcore_runtime_id)
            && second.as_deref().is_some_and(is_agentcore_runtime_id),
        "case \"two code interpreters\": both identifiers must be reported in the modelled \
         form, got {outputs:?}"
    );
    assert_ne!(
        first, second,
        "case \"two code interpreters\": two resources of one type must not share an \
         identifier, got {first:?} and {second:?}"
    );

    let (status_code, body) = describe_stack(&router, "agentcore-sandbox-ids").await;
    assert_eq!(
        status_code, 200,
        "case \"two code interpreters\": expected the second DescribeStacks to succeed, got \
         {status_code} {body}"
    );
    let again = wire_outputs(&body);
    assert_eq!(
        output_for(&again, "FirstId").and_then(|output| output.value.clone()),
        first,
        "case \"two code interpreters\": a second read must report the same identifier, got \
         {again:?}"
    );
}
