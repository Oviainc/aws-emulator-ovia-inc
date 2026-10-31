// SPDX-License-Identifier: CC0-1.0

//! Cross-stack references, end to end over the awsQuery wire: `Fn::ImportValue`,
//! `ListImports`, and what becomes of a deletion that would remove an export in use.
//!
//! The rule under test: `Fn::ImportValue` resolves to the value of the output another LIVE
//! stack of the same account and Region exported under that name, and the importing stack
//! becomes part of the cross-stack reference index — `ListImports(ExportName)` reports the
//! name of every live stack of that Region whose deployed template resolved an import for
//! it, and while that index names an importer, deleting the EXPORTING stack is ACCEPTED and
//! then CANCELLED, leaving the stack where it was in the healthy status it held with a
//! reason naming the export and the importing stack (that outcome itself is pinned by
//! `delete_stack_export_in_use.rs`). Once the last importer is gone the byte-identical
//! `DeleteStack` succeeds and the export disappears from `ListExports`.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness, so what it asserts is what a client reasoning about cross-stack
//! references sees. The table spans the class the rule generalises over: both template
//! spellings (JSON long form and YAML short form), an import in a resource property and an
//! import in an output value, an import nested inside another intrinsic, an export name that
//! is a literal and one composed by an intrinsic function, several imports in one stack, and
//! an import of a name nothing exports (which belongs to no export's index — that it also
//! fails its own deployment is pinned by `unresolvable_imports.rs`).

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::list_imports_refusal;
use crate::stack_lifecycle::{
    call, call_in, create_stack, list_imports_ok, member, router, wait_for_the_transition,
    xml_text, xml_texts, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    export_does_not_exist, export_is_not_imported,
};

/// The export names the exporting stack of these cases publishes, and the value each
/// carries.
const EXPORTS: [(&str, &str); 3] = [
    ("shared-endpoint", "https://example.test/v1"),
    ("shared-queue", "shared-queue-value"),
    ("shared-suffix", "suffix-value"),
];

/// The exporting stack's template: one output per entry of [`EXPORTS`].
fn exporter_template() -> String {
    let outputs: Vec<String> = EXPORTS
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            format!(r#""O{index}":{{"Value":"{value}","Export":{{"Name":"{name}"}}}}"#)
        })
        .collect();
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{{}}}}}"#,
        outputs.join(",")
    )
}

/// One row of the table: an importing stack, the export names its deployed template must
/// end up importing, and the output value its import has to resolve to.
struct ImportCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The export names `ListImports` must report this stack for.
    imports: Vec<&'static str>,
    /// `(OutputKey, OutputValue)` the stack must report, which is what says the import
    /// really resolved.
    expected_output: Option<(&'static str, String)>,
}

/// The class the rule covers: any template, in either spelling, importing any number of
/// exports from anywhere an intrinsic function may appear.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            case: "json long form, an import in a resource property",
            stack_name: "json-property-importer",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
                 "Properties":{"DisplayName":{"Fn::ImportValue":"shared-endpoint"}}}}}"#
                .to_string(),
            imports: vec!["shared-endpoint"],
            expected_output: None,
        },
        ImportCase {
            case: "yaml short form, an import echoed by an output",
            stack_name: "yaml-output-importer",
            template: concat!(
                "Resources:\n",
                "  Handle:\n",
                "    Type: AWS::CloudFormation::WaitConditionHandle\n",
                "Outputs:\n",
                "  Echoed:\n",
                "    Value: !ImportValue shared-endpoint\n",
            )
            .to_string(),
            imports: vec!["shared-endpoint"],
            expected_output: Some(("Echoed", EXPORTS[0].1.to_string())),
        },
        ImportCase {
            case: "an import nested inside another intrinsic function",
            stack_name: "nested-importer",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Joined":{"Value":{"Fn::Join":["/",
                   [{"Fn::ImportValue":"shared-queue"},"tail"]]}}}}"#
                .to_string(),
            imports: vec!["shared-queue"],
            expected_output: Some(("Joined", format!("{}/tail", EXPORTS[1].1))),
        },
        ImportCase {
            case: "two imports in one stack, one named by an intrinsic function",
            stack_name: "two-import-importer",
            template: r#"{"Parameters":{"Which":{"Type":"String"}},
                 "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
                   "Properties":{"DisplayName":{"Fn::ImportValue":"shared-endpoint"}}}},
                 "Outputs":{"Composed":{"Value":{"Fn::ImportValue":
                   {"Fn::Sub":"shared-${Which}"}}}}}"#
                .to_string(),
            imports: vec!["shared-endpoint", "shared-suffix"],
            expected_output: Some(("Composed", EXPORTS[2].1.to_string())),
        },
        ImportCase {
            case: "an import of a name nothing exports imports nothing",
            stack_name: "unresolved-importer",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Missing":{"Value":{"Fn::ImportValue":"no-such-export"}}}}"#
                .to_string(),
            imports: vec![],
            expected_output: None,
        },
        ImportCase {
            case: "a stack that imports nothing is in no export's index",
            stack_name: "plain-stack",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#
                .to_string(),
            imports: vec![],
            expected_output: None,
        },
    ]
}

/// The stack names one `ListImports` call reports, in wire order.
async fn list_imports(
    case: &str,
    router: &MultiServiceRouter,
    export_name: &str,
) -> Vec<String> {
    list_imports_ok(case, router, REGION, export_name, None)
        .await
        .0
}

/// The `(OutputKey, OutputValue)` pairs one stack reports.
async fn outputs_of(case: &str, router: &MultiServiceRouter, stack_name: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "DescribeStacks",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {status} {body}"
    );
    let keys = xml_texts(&body, "OutputKey");
    let values = xml_texts(&body, "OutputValue");
    keys.into_iter()
        .enumerate()
        .map(|(index, key)| (key, values.get(index).cloned().unwrap_or_default()))
        .collect()
}

/// Deploy the exporting stack and wait for its exports to be published.
async fn deploy_exporter(router: &MultiServiceRouter, stack_name: &str) -> String {
    let (status, body) = create_stack(router, stack_name, &exporter_template(), &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack for the exporter to succeed, got {status} {body}"
    );
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;
    stack_id
}

#[tokio::test]
async fn an_import_resolves_and_puts_the_importing_stack_in_the_export_index() {
    let router = router();
    deploy_exporter(&router, "exporter").await;

    for case in import_cases() {
        let extra = if case.stack_name == "two-import-importer" {
            vec![
                member("Parameters.member.1.ParameterKey", "Which"),
                member("Parameters.member.1.ParameterValue", "suffix"),
            ]
        } else {
            Vec::new()
        };
        let (status, body) =
            create_stack(&router, case.stack_name, &case.template, &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    // The index, read per export name: exactly the stacks whose deployed template resolved
    // an import for that name.
    for (export_name, _) in EXPORTS {
        let expected: Vec<String> = import_cases()
            .iter()
            .filter(|case| case.imports.contains(&export_name))
            .map(|case| case.stack_name.to_string())
            .collect();
        let reported = list_imports(export_name, &router, export_name).await;
        assert_eq!(
            reported, expected,
            "case {export_name:?}: expected exactly the importing stacks, got {reported:?}"
        );
    }
    // An export name nothing publishes has no importers to report, however many stacks named
    // it: the read is refused rather than answered with an empty page.
    assert_eq!(
        list_imports_refusal(
            "an export name nothing publishes",
            &router,
            REGION,
            "no-such-export"
        )
        .await,
        (
            "ValidationError".to_string(),
            export_does_not_exist("no-such-export")
        ),
        "case {:?}: expected the unpublished name's own refusal",
        "an export name nothing publishes"
    );

    // The import really RESOLVED: the value the importing stack reports is the exporting
    // stack's.
    for case in import_cases() {
        let Some((key, value)) = case.expected_output else {
            continue;
        };
        let outputs = outputs_of(case.case, &router, case.stack_name).await;
        assert!(
            outputs.contains(&(key.to_string(), value.clone())),
            "case {:?}: expected the output {key:?} to carry the imported value {value:?}, got \
             {outputs:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn deleting_an_exporting_stack_fails_while_an_importer_holds_its_export() {
    let router = router();
    deploy_exporter(&router, "held-exporter").await;

    let importer_template = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
         "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"shared-queue"}}}}"#;
    let (status, body) = create_stack(&router, "held-importer", importer_template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack for the importer to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let reported = list_imports("the importer is registered", &router, "shared-queue").await;
    assert_eq!(
        reported,
        vec!["held-importer".to_string()],
        "case {:?}: expected the importer in the index, got {reported:?}",
        "the importer is registered"
    );

    // While the index names an importer, deleting the EXPORTING stack is accepted and then
    // CANCELLED: the stack is left where it was, in the healthy status it held, and says
    // which export and which importer stood in the way.
    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "held-exporter")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the delete to be accepted, got {status} {body}",
        "deleting the exporter while its export is in use"
    );
    wait_for_the_transition().await;
    let (status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "held-exporter")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the exporter to still be there, got {status} {body}",
        "deleting the exporter while its export is in use"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {:?}: expected the cancelled delete to restore CREATE_COMPLETE, got {body}",
        "deleting the exporter while its export is in use"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("shared-queue") && reason.contains("held-importer"),
        "case {:?}: expected the reason to name the export and the importing stack, got \
         {reason:?}",
        "deleting the exporter while its export is in use"
    );

    // Once the importer is gone the index drops it, and the byte-identical delete succeeds.
    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "held-importer")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected deleting the importer to succeed, got {status} {body}",
        "after the importer is deleted"
    );
    wait_for_the_transition().await;

    // Once the importer is gone the index drops it, so the export is published and unused.
    assert_eq!(
        list_imports_refusal(
            "after the importer is deleted",
            &router,
            REGION,
            "shared-queue"
        )
        .await,
        (
            "ValidationError".to_string(),
            export_is_not_imported("shared-queue")
        ),
        "case {:?}: expected the deleted importer to drop out of the index",
        "after the importer is deleted"
    );

    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "held-exporter")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the identical delete to succeed now, got {status} {body}",
        "after the importer is deleted"
    );
    wait_for_the_transition().await;

    let (status, body) = call(&router, "ListExports", &[]).await;
    assert_eq!(
        status, 200,
        "expected ListExports to succeed, got {status} {body}"
    );
    assert!(
        !body.contains("shared-queue"),
        "case {:?}: expected the export to be gone once its stack was deleted, got {body}",
        "after the exporter is deleted"
    );
}

#[tokio::test]
async fn the_export_index_is_scoped_to_the_region_its_stacks_live_in() {
    let router = router();
    let other_region = "eu-west-1";

    // The SAME export name is published in two Regions, and imported in each: an export and
    // its importers belong to one account and Region, so neither read sees the other's.
    let exporter = r#"{"Outputs":{"Regional":{"Value":"regional-value",
         "Export":{"Name":"regional-shared"}}}}"#;
    let importer = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
         "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"regional-shared"}}}}"#;

    for (region, exporter_name, importer_name) in [
        (REGION, "home-exporter", "home-importer"),
        (other_region, "away-exporter", "away-importer"),
    ] {
        let (status, body) = call_in(
            &router,
            region,
            "CreateStack",
            &[
                member("StackName", exporter_name),
                member("TemplateBody", exporter),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected CreateStack for the exporter to succeed, got {status} \
             {body}"
        );
        wait_for_the_transition().await;
        let (status, body) = call_in(
            &router,
            region,
            "CreateStack",
            &[
                member("StackName", importer_name),
                member("TemplateBody", importer),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected CreateStack for the importer to succeed, got {status} \
             {body}"
        );
    }
    wait_for_the_transition().await;

    for (region, importer_name) in [(REGION, "home-importer"), (other_region, "away-importer")] {
        let (reported, _) = list_imports_ok(region, &router, region, "regional-shared", None).await;
        assert_eq!(
            reported,
            vec![importer_name.to_string()],
            "case {region:?}: expected only that Region's importer, got {reported:?}"
        );
    }
}
