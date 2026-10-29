// SPDX-License-Identifier: CC0-1.0

//! What `ListImports` answers for an export name that has no importers to report, end to end
//! over the awsQuery wire.
//!
//! RULE under test: **`ListImports` reads the importers of ONE export, so it answers only
//! where that export has importers: an export name the account and Region publishes nothing
//! under is refused `ValidationError` with "Export '<name>' does not exist.", an export that
//! IS published and no stack imports is refused `ValidationError` with "Export '<name>' is
//! not imported by any stack." — never an empty page, which a client deciding whether an
//! export is safe to change could not tell from a live export with no importers.**
//!
//! The documented workflow is what makes those two refusals different answers: "to modify or
//! remove an exported output value, first use this action to see which stacks are using it"
//! (`smithy_query(query="operation", operation="ListImports")`). The operation models no
//! error of its own, so both ride the service-common `ValidationError` CloudFormation refuses
//! a bad request with.
//!
//! The rows differ along what the rule generalises over: whether the export exists, how many
//! stacks import it (none, one, two), the Region the read is signed for (an export belongs to
//! one account and Region, so the same name read in another Region is published nowhere), and
//! a name differing from a published one only in case (an export name is matched exactly).
//! The second case walks the same export through the index as stacks come and go, so each
//! refusal is shown to follow the store rather than the request.
//!
//! `cross_stack_imports.rs` pins WHICH stacks the index holds and `list_imports_pages.rs` how
//! it is paged; this suite is what an export with nothing to report answers.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    export_does_not_exist, export_is_not_imported,
};

use crate::harness::{
    list_imports_ok, list_imports_page, list_imports_refusal, xml_blocks, REGION,
};
use crate::stack_lifecycle::{call, call_in, member, router, wait_for_the_transition};

/// A stack that publishes one export under `export_name`, and nothing else.
fn exporter(export_name: &str) -> String {
    format!(
        r#"{{"Outputs":{{"Shared":{{"Value":"shared-value","Export":{{"Name":"{export_name}"}}}}}}}}"#
    )
}

/// A stack that imports `export_name`, which is what puts it in the index.
fn importer(export_name: &str) -> String {
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{"Echoed":{{"Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}"#
    )
}

/// Deploy one stack in `region` and wait past the instant its create settles.
async fn deploy_in(router: &MultiServiceRouter, case: &str, region: &str, name: &str, body: &str) {
    let (status_code, response) = call_in(
        router,
        region,
        "CreateStack",
        &[member("StackName", name), member("TemplateBody", body)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack({name}) must answer 200, got {status_code} {response}"
    );
    wait_for_the_transition().await;
}

/// What one row expects of its read.
enum Expected {
    /// The importers the read reports, in wire order.
    Importers(Vec<&'static str>),
    /// The refusal message, under the code CloudFormation refuses a bad request with.
    Refused(String),
}

#[tokio::test]
async fn an_export_with_nothing_to_report_is_refused_in_its_own_words() {
    let router = router();
    // One account and Region holding: an export nothing imports, an export one stack
    // imports, an export two stacks import — and the same name published in another Region,
    // which this Region's reads must not see.
    for (case, name, body) in [
        ("the unused export", "unused-export", exporter("unused-export")),
        ("the used export", "used-export", exporter("used-export")),
        ("the twice-used export", "shared-twice", exporter("shared-twice")),
    ] {
        deploy_in(&router, case, REGION, name, &body).await;
    }
    for (case, name, export) in [
        ("the only importer", "one-importer", "used-export"),
        ("the first importer", "first-importer", "shared-twice"),
        ("the second importer", "second-importer", "shared-twice"),
    ] {
        deploy_in(&router, case, REGION, name, &importer(export)).await;
    }
    // Another Region, publishing and importing the SAME name: neither read sees the other's.
    let other_region = "eu-west-1";
    deploy_in(
        &router,
        "the away exporter",
        other_region,
        "away-exporter",
        &exporter("away-only-export"),
    )
    .await;
    deploy_in(
        &router,
        "the away importer",
        other_region,
        "away-importer",
        &importer("away-only-export"),
    )
    .await;
    wait_for_the_transition().await;

    let rows = vec![
        (
            "an export one stack imports",
            REGION,
            "used-export",
            Expected::Importers(vec!["one-importer"]),
        ),
        (
            "an export two stacks import",
            REGION,
            "shared-twice",
            Expected::Importers(vec!["first-importer", "second-importer"]),
        ),
        (
            "an export this Region publishes and nothing imports",
            REGION,
            "unused-export",
            Expected::Refused(export_is_not_imported("unused-export")),
        ),
        (
            "an export name nothing was ever published under",
            REGION,
            "never-published-export",
            Expected::Refused(export_does_not_exist("never-published-export")),
        ),
        (
            // The refusal reflects the name the request sent, so the envelope has to ESCAPE
            // it: an unescaped value would close or split these elements.
            "an export name carrying xml metacharacters",
            REGION,
            "</Message><Injected>&\"'",
            Expected::Refused(export_does_not_exist("</Message><Injected>&\"'")),
        ),
        (
            "a name differing from a published one only in case",
            REGION,
            "Unused-Export",
            Expected::Refused(export_does_not_exist("Unused-Export")),
        ),
        (
            "an export only another Region publishes",
            REGION,
            "away-only-export",
            Expected::Refused(export_does_not_exist("away-only-export")),
        ),
        (
            "an export only this Region publishes, read in the other one",
            other_region,
            "used-export",
            Expected::Refused(export_does_not_exist("used-export")),
        ),
        (
            "the other Region's own export, with its own importer",
            other_region,
            "away-only-export",
            Expected::Importers(vec!["away-importer"]),
        ),
    ];

    for (case, region, export_name, expected) in rows {
        match expected {
            Expected::Importers(importers) => {
                let (reported, next) =
                    list_imports_ok(case, &router, region, export_name, None).await;
                let wanted: Vec<String> =
                    importers.iter().map(|name| name.to_string()).collect();
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: expected exactly those importers, got {reported:?}"
                );
                assert!(
                    next.is_none(),
                    "case {case:?}: expected no NextToken on a single page, got {next:?}"
                );
            }
            Expected::Refused(message) => {
                let answered = list_imports_refusal(case, &router, region, export_name).await;
                assert_eq!(
                    answered,
                    ("ValidationError".to_string(), message.clone()),
                    "case {case:?}: expected ('ValidationError', {message:?})"
                );
                // One envelope, whatever the name carried: a reflected value that was not
                // escaped would split or close these elements.
                let (_, body, _, _) = list_imports_page(&router, region, export_name, None).await;
                assert_eq!(
                    (
                        xml_blocks(&body, "Message").len(),
                        xml_blocks(&body, "RequestId").len()
                    ),
                    (1, 1),
                    "case {case:?}: expected one Message and one RequestId, got {body}"
                );
                // A refused read reports: the identical read answers the identical refusal,
                // and the export its name resolves nothing under is still published nowhere.
                let again = list_imports_refusal(case, &router, region, export_name).await;
                assert_eq!(
                    again, answered,
                    "case {case:?}: expected the repeated read to answer the same refusal"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_refusal_follows_the_index_as_the_stacks_come_and_go() {
    let router = router();
    let export = "walked-export";

    // Before anything publishes it, the name exists nowhere.
    assert_eq!(
        list_imports_refusal("before the export is published", &router, REGION, export).await,
        (
            "ValidationError".to_string(),
            export_does_not_exist(export)
        ),
        "case {:?}: expected the unpublished name's refusal",
        "before the export is published"
    );

    // Published, with no importer yet: the other refusal.
    deploy_in(
        &router,
        "the exporter",
        REGION,
        "walked-exporter",
        &exporter(export),
    )
    .await;
    assert_eq!(
        list_imports_refusal("published with no importer", &router, REGION, export).await,
        (
            "ValidationError".to_string(),
            export_is_not_imported(export)
        ),
        "case {:?}: expected the unimported export's refusal",
        "published with no importer"
    );

    // Imported: the read answers the importer instead of refusing at all.
    deploy_in(
        &router,
        "the importer",
        REGION,
        "walked-importer",
        &importer(export),
    )
    .await;
    wait_for_the_transition().await;
    let (reported, _) = list_imports_ok("the importer", &router, REGION, export, None).await;
    assert_eq!(
        reported,
        vec!["walked-importer".to_string()],
        "case {:?}: expected the importer reported, got {reported:?}",
        "the importer"
    );

    // The importer gone, the export is back to being published and unused.
    let (status_code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "walked-importer")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {:?}: DeleteStack must answer 200, got {status_code} {body}",
        "after the importer is deleted"
    );
    wait_for_the_transition().await;
    assert_eq!(
        list_imports_refusal("after the importer is deleted", &router, REGION, export).await,
        (
            "ValidationError".to_string(),
            export_is_not_imported(export)
        ),
        "case {:?}: expected the unimported export's refusal",
        "after the importer is deleted"
    );

    // The exporter gone, the export itself is gone with it.
    let (status_code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "walked-exporter")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {:?}: DeleteStack must answer 200, got {status_code} {body}",
        "after the exporter is deleted"
    );
    wait_for_the_transition().await;
    assert_eq!(
        list_imports_refusal("after the exporter is deleted", &router, REGION, export).await,
        (
            "ValidationError".to_string(),
            export_does_not_exist(export)
        ),
        "case {:?}: expected the export to be gone with its stack",
        "after the exporter is deleted"
    );
}
