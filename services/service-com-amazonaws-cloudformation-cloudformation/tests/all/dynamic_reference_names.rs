// SPDX-License-Identifier: CC0-1.0

//! A `Ref` whose argument is an EXPRESSION, end to end over the awsQuery wire.
//!
//! The rule under test: `Ref` returns the value of the parameter or resource its argument
//! NAMES, and under the `AWS::LanguageExtensions` transform that name may be spelled by an
//! intrinsic function rather than written out — so the reference is dereferenced by the
//! name its argument evaluates to, exactly as `Fn::GetAtt`'s first argument already is. A
//! written-out name evaluates to itself, so the plain form is unchanged.
//!
//! The first table spans the class the rule generalises over: the FORM of the expression
//! that spells the name (`Fn::Sub` with and without its own variable map, a nested `Ref`,
//! `Fn::Join`, `Fn::FindInMap`, `Fn::If`, `Fn::Select` over a list-typed parameter) and
//! WHAT the name resolves to (a resource's physical id, a parameter's value, a pseudo
//! parameter, and a name the template declares nothing for — which carries no value rather
//! than an invented one). The written-out `Ref` is one row of the same table.
//!
//! The second table is that rule where the issue found it missing: an `Fn::ForEach` block
//! in the `Outputs` section, whose collection is a `Ref` to a `CommaDelimitedList`
//! parameter and whose generated outputs each read the resource that iteration created
//! through `{"Ref": {"Fn::Sub": "…${Identifier}"}}`. Its rows differ in the number of
//! collection items, in how the items are spelled, and in whether the list comes from the
//! parameter's own `Default` or from the submitting request.
//!
//! Every value asserted here is read back out of the deployed stack — an output's text
//! against the physical id `DescribeStackResources` reports for the resource it names — so
//! what the rows pin is what the deployment COMPUTED, not request input echoed back.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, REGION,
};

/// `Capabilities` acknowledging the transform every row declares.
fn acknowledged() -> Vec<(String, String)> {
    vec![member("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND")]
}

/// What the single output of a dynamic-reference row must carry.
#[derive(Debug)]
enum Expected {
    /// The physical id `DescribeStackResources` reports for this logical id: a value the
    /// deployment minted, which is what `Ref` answers for a resource.
    PhysicalIdOf(&'static str),
    /// This exact text: what `Ref` answers for a parameter or a pseudo parameter.
    Text(String),
    /// No `OutputValue` at all — a name the template declares nothing for resolves to
    /// nothing, and nothing is invented for it.
    NoValue,
}

/// One row of the dynamic-reference table: the `Outputs` value under test and what the
/// deployed stack must report for it.
struct DynamicRef {
    case: &'static str,
    stack_name: &'static str,
    /// The expression the output's `Value` is declared as.
    value: &'static str,
    expected: Expected,
}

/// The template every dynamic-reference row is deployed as: two resources to name, two
/// scalar parameters, a list-typed parameter, a mapping and a condition — so a row's own
/// expression is the only thing that varies.
fn dynamic_ref_template(stack_name: &str, value: &str) -> String {
    format!(
        r#"{{"Transform":"AWS::LanguageExtensions",
             "Parameters":{{
               "Which":{{"Type":"String","Default":"Alpha"}},
               "Stage":{{"Type":"String","Default":"prod"}},
               "Names":{{"Type":"CommaDelimitedList","Default":"Alpha,Beta"}}}},
             "Mappings":{{"Pick":{{"prod":{{"Resource":"Beta"}}}}}},
             "Conditions":{{"IsProd":{{"Fn::Equals":[{{"Ref":"Stage"}},"prod"]}}}},
             "Resources":{{
               "Alpha":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{stack_name}-alpha"}}}},
               "Beta":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{stack_name}-beta"}}}}}},
             "Outputs":{{"V":{{"Value":{value}}}}}}}"#
    )
}

/// The table: one row per form a reference's name can be spelled in.
fn dynamic_refs() -> Vec<DynamicRef> {
    vec![
        DynamicRef {
            // The written-out form, which the rule must leave exactly as it was.
            case: "a name written out",
            stack_name: "ref-plain",
            value: r#"{"Ref":"Alpha"}"#,
            expected: Expected::PhysicalIdOf("Alpha"),
        },
        DynamicRef {
            case: "a name composed by Fn::Sub from a parameter",
            stack_name: "ref-sub",
            value: r#"{"Ref":{"Fn::Sub":"${Which}"}}"#,
            expected: Expected::PhysicalIdOf("Alpha"),
        },
        DynamicRef {
            case: "a name composed by Fn::Sub from its own variable map",
            stack_name: "ref-sub-vars",
            value: r#"{"Ref":{"Fn::Sub":["Bet${Tail}",{"Tail":"a"}]}}"#,
            expected: Expected::PhysicalIdOf("Beta"),
        },
        DynamicRef {
            case: "a name another Ref supplies",
            stack_name: "ref-nested",
            value: r#"{"Ref":{"Ref":"Which"}}"#,
            expected: Expected::PhysicalIdOf("Alpha"),
        },
        DynamicRef {
            case: "a name Fn::Join builds from literals",
            stack_name: "ref-join",
            value: r#"{"Ref":{"Fn::Join":["",["Al","pha"]]}}"#,
            expected: Expected::PhysicalIdOf("Alpha"),
        },
        DynamicRef {
            case: "a name the Mappings section carries",
            stack_name: "ref-findinmap",
            value: r#"{"Ref":{"Fn::FindInMap":["Pick","prod","Resource"]}}"#,
            expected: Expected::PhysicalIdOf("Beta"),
        },
        DynamicRef {
            case: "a name a condition chooses",
            stack_name: "ref-if",
            value: r#"{"Ref":{"Fn::If":["IsProd","Beta","Alpha"]}}"#,
            expected: Expected::PhysicalIdOf("Beta"),
        },
        DynamicRef {
            case: "a name selected out of a list-typed parameter",
            stack_name: "ref-select",
            value: r#"{"Ref":{"Fn::Select":[1,{"Ref":"Names"}]}}"#,
            expected: Expected::PhysicalIdOf("Beta"),
        },
        DynamicRef {
            // The same rule where the resolved name is a PARAMETER's rather than a
            // resource's: `Ref` answers that parameter's value in force.
            case: "a parameter named by an expression",
            stack_name: "ref-parameter",
            value: r#"{"Ref":{"Fn::Join":["",["St","age"]]}}"#,
            expected: Expected::Text("prod".to_string()),
        },
        DynamicRef {
            // And where it is a PSEUDO parameter's: "use them the same way as you would a
            // parameter, as the argument for the Ref function".
            case: "a pseudo parameter named by an expression",
            stack_name: "ref-pseudo",
            value: r#"{"Ref":{"Fn::Join":["::",["AWS","Region"]]}}"#,
            expected: Expected::Text(REGION.to_string()),
        },
        DynamicRef {
            case: "an expression naming nothing the template declares",
            stack_name: "ref-unknown",
            value: r#"{"Ref":{"Fn::Join":["",["No","Such"]]}}"#,
            expected: Expected::NoValue,
        },
    ]
}

/// Every output a `DescribeStacks` response reports, as `(OutputKey, OutputValue)` in wire
/// order.
///
/// Read INSIDE the `Outputs` block, one `member` at a time, so a key and a value are paired
/// by the element that carries both: an output reported WITHOUT a value (a reference that
/// resolved to nothing) leaves its pair `None` instead of shifting every later value onto
/// the wrong key, and the `member`s of the response's other lists (its `Parameters`, its
/// `Capabilities`) cannot be swept up with them.
fn reported_outputs(xml: &str) -> Vec<(String, Option<String>)> {
    xml_blocks(xml, "Outputs")
        .into_iter()
        .flat_map(|block| xml_blocks(&block, "member"))
        .filter_map(|output| {
            let key = xml_text(&output, "OutputKey")?;
            Some((key, xml_text(&output, "OutputValue")))
        })
        .collect()
}

/// The physical id `DescribeStackResources` reports for one logical id of a stack: the
/// value the deployment minted, which is what a `Ref` to that resource must answer.
async fn physical_id_of(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
    logical_id: &str,
) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack_name),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeStackResources({logical_id}) must succeed, got {status_code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeStackResources({logical_id}) reported no physical id: {body}")
    })
}

#[tokio::test]
async fn a_reference_is_dereferenced_by_the_name_its_argument_evaluates_to() {
    let router = router();
    let cases = dynamic_refs();

    for case in &cases {
        let template = dynamic_ref_template(case.stack_name, case.value);
        let (status_code, body) =
            create_stack(&router, case.stack_name, &template, &acknowledged()).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must accept the template, got {status_code} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let (_, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have completed, got {described}",
            case.case
        );
        let outputs = reported_outputs(&described);
        assert_eq!(
            outputs.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            vec!["V"],
            "case {:?}: the stack declares the one output V, got {described}",
            case.case
        );
        let reported = outputs.first().and_then(|(_, value)| value.clone());
        let expected = match &case.expected {
            Expected::PhysicalIdOf(logical_id) => {
                Some(physical_id_of(case.case, &router, case.stack_name, logical_id).await)
            }
            Expected::Text(text) => Some(text.clone()),
            Expected::NoValue => None,
        };
        assert_eq!(
            reported, expected,
            "case {:?}: the output must carry {expected:?}, got {reported:?} in {described}",
            case.case
        );
    }
}

/// One row of the loop table: the collection the `Outputs` loop runs over, and where that
/// collection's value comes from.
struct LoopCase {
    case: &'static str,
    stack_name: &'static str,
    /// The items the loop must expand to one entry per.
    items: Vec<String>,
    /// Whether the list is the parameter's own `Default` or a value the request supplies.
    from_the_request: bool,
}

/// The table: the same loop over collections that differ in size, in item spelling, and in
/// where the list comes from.
fn loop_cases() -> Vec<LoopCase> {
    let letters: Vec<String> = ('a'..='x').map(|letter| letter.to_string()).collect();
    vec![
        LoopCase {
            case: "a one-item collection",
            stack_name: "foreach-one",
            items: vec!["only".to_string()],
            from_the_request: false,
        },
        LoopCase {
            case: "a three-item collection",
            stack_name: "foreach-three",
            items: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            from_the_request: false,
        },
        LoopCase {
            // The width the issue's own template loops over, from a..x.
            case: "a twenty-four-item collection",
            stack_name: "foreach-wide",
            items: letters,
            from_the_request: false,
        },
        LoopCase {
            case: "items carrying digits",
            stack_name: "foreach-digits",
            items: vec!["s1".to_string(), "s2".to_string(), "s3".to_string()],
            from_the_request: false,
        },
        LoopCase {
            // The same loop where the list is the REQUEST's rather than the template's.
            case: "a collection the request supplies",
            stack_name: "foreach-supplied",
            items: vec!["red".to_string(), "green".to_string()],
            from_the_request: true,
        },
    ]
}

/// The looping template: one resource per collection item, one resource OUTSIDE the loop,
/// and one output per item whose value reads that item's resource through a reference whose
/// name an `Fn::Sub` spells.
fn loop_template(default: &str) -> String {
    format!(
        r#"{{"Transform":"AWS::LanguageExtensions",
             "Parameters":{{"Ids":{{"Type":"CommaDelimitedList","Default":"{default}"}}}},
             "Resources":{{
               "Fn::ForEach::Fanned":["Slot",{{"Ref":"Ids"}},
                 {{"Q${{Slot}}":{{"Type":"AWS::SNS::Topic",
                   "Properties":{{"TopicName":{{"Fn::Sub":"${{AWS::StackName}}-${{Slot}}"}}}}}}}}],
               "Sentinel":{{"Type":"AWS::SNS::Topic"}}}},
             "Outputs":{{
               "Fn::ForEach::Urls":["Slot",{{"Ref":"Ids"}},
                 {{"Url${{Slot}}":{{"Value":{{"Ref":{{"Fn::Sub":"Q${{Slot}}"}}}}}}}}]}}}}"#
    )
}

#[tokio::test]
async fn a_foreach_block_in_outputs_expands_over_a_list_typed_parameter() {
    let router = router();
    let cases = loop_cases();

    for case in &cases {
        // The collection reaches the template as its parameter's `Default` or as the
        // request's own `ParameterValue`, which is the one difference between the rows.
        let joined = case.items.join(",");
        let (template, extra) = if case.from_the_request {
            let mut extra = acknowledged();
            extra.push(member("Parameters.member.1.ParameterKey", "Ids"));
            extra.push(member("Parameters.member.1.ParameterValue", &joined));
            (loop_template("placeholder"), extra)
        } else {
            (loop_template(&joined), acknowledged())
        };
        let (status_code, body) =
            create_stack(&router, case.stack_name, &template, &extra).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must accept the template, got {status_code} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let (_, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have completed, got {described}",
            case.case
        );
        // One generated output per collection item, and no other: the loop expands in the
        // `Outputs` section exactly as it does in `Resources`.
        let outputs = reported_outputs(&described);
        let mut expected_keys: Vec<String> = case
            .items
            .iter()
            .map(|item| format!("Url{item}"))
            .collect();
        let mut keys: Vec<String> = outputs.iter().map(|(key, _)| key.clone()).collect();
        keys.sort();
        expected_keys.sort();
        assert_eq!(
            keys, expected_keys,
            "case {:?}: one output per collection item is expected, got {described}",
            case.case
        );
        // Every generated output carries the physical id of the resource that iteration
        // created, which is what the reference inside the loop body names.
        for item in &case.items {
            let physical_id =
                physical_id_of(case.case, &router, case.stack_name, &format!("Q{item}")).await;
            let key = format!("Url{item}");
            let reported = outputs
                .iter()
                .find(|(reported, _)| *reported == key)
                .and_then(|(_, value)| value.clone());
            assert_eq!(
                reported.as_deref(),
                Some(physical_id.as_str()),
                "case {:?}: output {key:?} must carry the physical id of Q{item}, got \
                 {reported:?} in {described}",
                case.case
            );
        }
        // The resource declared OUTSIDE the loop is still deployed: the expansion replaces
        // the directive's own key, not its siblings.
        let sentinel = physical_id_of(case.case, &router, case.stack_name, "Sentinel").await;
        assert!(
            !sentinel.is_empty(),
            "case {:?}: the non-loop resource must be deployed, got {sentinel:?}",
            case.case
        );
    }
}
