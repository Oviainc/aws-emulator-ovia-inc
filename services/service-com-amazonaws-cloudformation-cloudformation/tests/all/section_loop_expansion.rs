// SPDX-License-Identifier: CC0-1.0

//! The `Fn::ForEach` expansion over EVERY section the language-extensions transform defines
//! the directive for, end to end over the awsQuery wire.
//!
//! The rule under test: the expansion pass rewrites the whole submitted document, so a loop
//! is expanded wherever the transform defines it — the `Conditions`, `Outputs` and
//! `Resources` sections alike — and its collection argument may be a reference to a
//! list-typed parameter, resolved to the value in force for that parameter (the one the
//! request supplies, else the one the declaration defaults to) before the loop is unrolled.
//! The rewritten document is what `GetTemplate(TemplateStage=Processed)` serves and what the
//! stack deploys; the submitted body is still served, byte for byte, as `Original`.
//!
//! The sections the directive may appear in are the transform's own catalogue, which this
//! crate records at `state::template_validation`'s `FOR_EACH_SECTIONS` ("you can use the
//! Fn::ForEach intrinsic function in the Conditions, Resources and Outputs sections"), and
//! the table below has a row for each of the three.
//!
//! The rows differ along what the rule generalises over: which section carries the loop, how
//! the collection is spelled (a reference resolved from the parameter's `Default`, the same
//! reference resolved from the submitting request's own `ParameterValue`, and a list written
//! out in the template), how many items it holds (one, two, three, four) and how those items
//! are spelled (bare letters, words, letters carrying digits).
//!
//! Every expectation is read back out of the deployed stack — the processed stage, the
//! resources `DescribeStackResources` reports and the outputs `DescribeStacks` reports — so
//! what the rows pin is the document the expansion COMPUTED.

use service_com_amazonaws_cloudformation_cloudformation::state::{status, CAPABILITY_AUTO_EXPAND};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_stack, get_template, member, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// One row: a template declaring the transform, the document its processed stage must be,
/// and what the deployed stack must then report.
struct LoopRow {
    case: &'static str,
    stack_name: &'static str,
    /// The submitted body, which `Original` must answer byte for byte.
    template: String,
    /// The expanded document, as JSON text — compared parsed, so the order its members
    /// happen to be written in is not part of the expectation.
    processed: String,
    /// The logical ids the expanded document declares, sorted.
    resources: Vec<String>,
    /// The `(OutputKey, OutputValue)` pairs the deployed stack must report, sorted by key.
    outputs: Vec<(String, String)>,
    /// A parameter value the request supplies, for the row whose collection comes from the
    /// request rather than from the declaration's `Default`.
    supplied: Option<(String, String)>,
}

/// The table.
fn loop_rows() -> Vec<LoopRow> {
    vec![
        // The section the issue found unexpanded: a loop in `Outputs` whose collection is a
        // reference to a list-typed parameter.
        LoopRow {
            case: "a loop in the Outputs section over a reference to a list parameter",
            stack_name: "sections-outputs",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"a,b,c"}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Fn::ForEach::Slots":["Slot",{"Ref":"Ids"},{"Slot${Slot}":{"Value":{"Fn::Sub":"slot-${Slot}"}}}]}}"#.to_string(),
            processed: r#"{"Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"a,b,c"}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Slota":{"Value":{"Fn::Sub":"slot-a"}},"Slotb":{"Value":{"Fn::Sub":"slot-b"}},"Slotc":{"Value":{"Fn::Sub":"slot-c"}}}}"#.to_string(),
            resources: vec!["Sentinel".to_string()],
            outputs: vec![
                ("Slota".to_string(), "slot-a".to_string()),
                ("Slotb".to_string(), "slot-b".to_string()),
                ("Slotc".to_string(), "slot-c".to_string()),
            ],
            supplied: None,
        },
        // Two loops over the SAME reference, one per section: the pass is one walk of the
        // document, so neither section is expanded at the other's expense.
        LoopRow {
            case: "a loop in Resources and a loop in Outputs over the same reference",
            stack_name: "sections-both",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"one,two"}},"Resources":{"Fn::ForEach::Handles":["Slot",{"Ref":"Ids"},{"Handle${Slot}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}],"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Fn::ForEach::Names":["Slot",{"Ref":"Ids"},{"Name${Slot}":{"Value":{"Fn::Sub":"${AWS::StackName}-${Slot}"}}}]}}"#.to_string(),
            processed: r#"{"Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"one,two"}},"Resources":{"Handleone":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Handletwo":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Nameone":{"Value":{"Fn::Sub":"${AWS::StackName}-one"}},"Nametwo":{"Value":{"Fn::Sub":"${AWS::StackName}-two"}}}}"#.to_string(),
            resources: vec![
                "Handleone".to_string(),
                "Handletwo".to_string(),
                "Sentinel".to_string(),
            ],
            outputs: vec![
                ("Nameone".to_string(), "sections-both-one".to_string()),
                ("Nametwo".to_string(), "sections-both-two".to_string()),
            ],
            supplied: None,
        },
        // The third section of the catalogue. The condition the loop emits is one the
        // deployment then EVALUATES — the output below reads a condition name that exists
        // only because the loop expanded.
        LoopRow {
            case: "a loop in the Conditions section, whose condition an output reads",
            stack_name: "sections-conditions",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"red,green"},"Pick":{"Type":"String","Default":"red"}},"Conditions":{"Fn::ForEach::Flags":["Id",{"Ref":"Ids"},{"Is${Id}":{"Fn::Equals":[{"Ref":"Pick"},{"Ref":"Id"}]}}]},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Chosen":{"Value":{"Fn::If":["Isred","the-red-one","another-one"]}}}}"#.to_string(),
            processed: r#"{"Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"red,green"},"Pick":{"Type":"String","Default":"red"}},"Conditions":{"Isred":{"Fn::Equals":[{"Ref":"Pick"},"red"]},"Isgreen":{"Fn::Equals":[{"Ref":"Pick"},"green"]}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Chosen":{"Value":{"Fn::If":["Isred","the-red-one","another-one"]}}}}"#.to_string(),
            resources: vec!["Sentinel".to_string()],
            outputs: vec![("Chosen".to_string(), "the-red-one".to_string())],
            supplied: None,
        },
        // The same reference, resolved from the value the REQUEST supplies rather than from
        // the declaration's own default.
        LoopRow {
            case: "the collection the request supplies rather than the declared default",
            stack_name: "sections-supplied",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"unused"}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Fn::ForEach::Slots":["Slot",{"Ref":"Ids"},{"Slot${Slot}":{"Value":{"Fn::Sub":"slot-${Slot}"}}}]}}"#.to_string(),
            processed: r#"{"Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"unused"}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Slotx1":{"Value":{"Fn::Sub":"slot-x1"}},"Slotx2":{"Value":{"Fn::Sub":"slot-x2"}},"Slotx3":{"Value":{"Fn::Sub":"slot-x3"}},"Slotx4":{"Value":{"Fn::Sub":"slot-x4"}}}}"#.to_string(),
            resources: vec!["Sentinel".to_string()],
            outputs: vec![
                ("Slotx1".to_string(), "slot-x1".to_string()),
                ("Slotx2".to_string(), "slot-x2".to_string()),
                ("Slotx3".to_string(), "slot-x3".to_string()),
                ("Slotx4".to_string(), "slot-x4".to_string()),
            ],
            supplied: Some(("Ids".to_string(), "x1,x2,x3,x4".to_string())),
        },
        // A collection of ONE: the loop still replaces its own key with the entry it emits.
        LoopRow {
            case: "a one-item collection in the Outputs section",
            stack_name: "sections-one-item",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"solo"}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Fn::ForEach::Slots":["Slot",{"Ref":"Ids"},{"Slot${Slot}":{"Value":{"Fn::Sub":"slot-${Slot}"}}}]}}"#.to_string(),
            processed: r#"{"Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"solo"}},"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Slotsolo":{"Value":{"Fn::Sub":"slot-solo"}}}}"#.to_string(),
            resources: vec!["Sentinel".to_string()],
            outputs: vec![("Slotsolo".to_string(), "slot-solo".to_string())],
            supplied: None,
        },
        // How the collection is SPELLED is not what decides whether the section is
        // expanded: a list written out in the template loops in `Outputs` too.
        LoopRow {
            case: "a collection written out in the template, in the Outputs section",
            stack_name: "sections-literal",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Fn::ForEach::Slots":["Slot",["p1","q2"],{"Slot${Slot}":{"Value":{"Fn::Sub":"slot-${Slot}"}}}]}}"#.to_string(),
            processed: r#"{"Resources":{"Sentinel":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Slotp1":{"Value":{"Fn::Sub":"slot-p1"}},"Slotq2":{"Value":{"Fn::Sub":"slot-q2"}}}}"#.to_string(),
            resources: vec!["Sentinel".to_string()],
            outputs: vec![
                ("Slotp1".to_string(), "slot-p1".to_string()),
                ("Slotq2".to_string(), "slot-q2".to_string()),
            ],
            supplied: None,
        },
    ]
}

/// Every output a `DescribeStacks` response reports, as `(OutputKey, OutputValue)` sorted by
/// key.
///
/// Read INSIDE the `Outputs` block, one `member` at a time, so a key and a value are paired
/// by the element that carries both and the `member`s of the response's other lists cannot
/// be swept up with them.
fn reported_outputs(xml: &str) -> Vec<(String, String)> {
    let mut outputs: Vec<(String, String)> = xml_blocks(xml, "Outputs")
        .into_iter()
        .flat_map(|block| xml_blocks(&block, "member"))
        .filter_map(|output| {
            let key = xml_text(&output, "OutputKey")?;
            Some((key, xml_text(&output, "OutputValue").unwrap_or_default()))
        })
        .collect();
    outputs.sort();
    outputs
}

/// The logical ids `DescribeStackResources` reports for a stack, sorted.
async fn reported_resources(case: &str, router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (http_status, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackResources to succeed, got {http_status} {body}"
    );
    let mut ids = xml_texts(&body, "LogicalResourceId");
    ids.sort();
    ids
}

#[tokio::test]
async fn a_foreach_loop_is_expanded_in_every_section_the_transform_defines_it_for() {
    let router = router();
    let rows = loop_rows();

    for row in &rows {
        let mut extra = vec![member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND)];
        if let Some((key, value)) = &row.supplied {
            extra.push(member("Parameters.member.1.ParameterKey", key));
            extra.push(member("Parameters.member.1.ParameterValue", value));
        }
        let (http_status, body) =
            create_stack(&router, row.stack_name, &row.template, &extra).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to accept the template, got {http_status} {body}",
            row.case
        );
    }

    for row in &rows {
        // "To get the user-submitted template, specify Original" — byte for byte, expansion
        // or no expansion.
        let (http_status, body) = get_template(&router, row.stack_name, Some("Original")).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected GetTemplate(Original) to succeed, got {http_status} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(row.template.as_str()),
            "case {:?}: expected the submitted body as the Original stage, got {body}",
            row.case
        );

        let expected: serde_json::Value = serde_json::from_str(&row.processed)
            .unwrap_or_else(|error| panic!("case {:?}: bad row ({error})", row.case));
        let (http_status, body) = get_template(&router, row.stack_name, Some("Processed")).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected GetTemplate(Processed) to succeed, got {http_status} {body}",
            row.case
        );
        let served = xml_text(&body, "TemplateBody").unwrap_or_default();
        let actual: serde_json::Value = serde_json::from_str(&served).unwrap_or_else(|error| {
            panic!(
                "case {:?}: expected a JSON processed stage, got {served:?} ({error})",
                row.case
            )
        });
        assert_eq!(
            actual, expected,
            "case {:?}: expected the expanded document as the Processed stage, got {actual}",
            row.case
        );
        assert!(
            actual.get("Transform").is_none(),
            "case {:?}: expected the applied transform not to survive the expansion, got \
             {actual}",
            row.case
        );
    }

    wait_for_the_transition().await;

    for row in &rows {
        let (http_status, described) = call(
            &router,
            "DescribeStacks",
            &[member("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected DescribeStacks to succeed, got {http_status} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: expected the expanded template to deploy, got {described}",
            row.case
        );
        let actual = reported_outputs(&described);
        assert_eq!(
            actual, row.outputs,
            "case {:?}: expected the expanded outputs, got {actual:?} in {described}",
            row.case
        );
        let deployed = reported_resources(row.case, &router, row.stack_name).await;
        assert_eq!(
            deployed, row.resources,
            "case {:?}: expected the expanded document's resources to be deployed, got \
             {deployed:?}",
            row.case
        );
    }
}
