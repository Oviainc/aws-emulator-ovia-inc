// SPDX-License-Identifier: CC0-1.0

//! `ListExports`, end to end over the awsQuery wire.
//!
//! The rule under test: `ListExports` reports one `Export` per output of a LIVE stack of
//! the account and Region the request was signed for whose `Outputs` entry declares an
//! `Export` name — carrying that stack's `StackId` as `ExportingStackId`, the evaluated
//! export name as `Name`, and the output's evaluated value as `Value` — and nothing else:
//! an output that declares no export name publishes none, an export name that does not
//! resolve publishes none, a stack whose deployment has not completed publishes none yet,
//! a stack whose deletion has completed publishes none any more, and another Region's
//! stacks are not in the answer at all.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client listing the account's exported output values sees. The table spans the class the
//! rule generalises over: both template spellings (JSON and YAML, long-form and short-form
//! intrinsics), the count of exported outputs per stack (none, one, several), export names
//! that are literals and names composed by an intrinsic function, values that are
//! literals, values taken from a parameter and values this service cannot evaluate at all,
//! and outputs that declare no export beside outputs that do.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    PAGE_SIZE, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, create_stack, member, router, wait_for_the_transition, xml_blocks, xml_text,
    REGION,
};

/// One `Export` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireExport {
    exporting_stack_id: Option<String>,
    name: Option<String>,
    value: Option<String>,
}

/// Every `Export` in a `ListExports` response, in wire order.
///
/// Read member by member, so an export's name stays paired with its own value and with
/// the stack that published it (and so an export reported WITHOUT a `Value` is visible as
/// such instead of silently borrowing the next one's).
fn wire_exports(xml: &str) -> Vec<WireExport> {
    let Some(block) = xml_blocks(xml, "Exports").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .iter()
        .map(|entry| WireExport {
            exporting_stack_id: xml_text(entry, "ExportingStackId"),
            name: xml_text(entry, "Name"),
            value: xml_text(entry, "Value"),
        })
        .collect()
}

/// One `ListExports` call, optionally resuming at a `NextToken`.
async fn list_exports(router: &MultiServiceRouter, next_token: Option<&str>) -> (u16, String) {
    let members: Vec<(String, String)> = next_token
        .map(|token| vec![member("NextToken", token)])
        .unwrap_or_default();
    call(router, "ListExports", &members).await
}

/// The exports one `ListExports` call reports, with the call asserted to have succeeded.
async fn exports_now(case: &str, router: &MultiServiceRouter) -> Vec<WireExport> {
    let (status, body) = list_exports(router, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListExports to succeed, got {status} {body}"
    );
    wire_exports(&body)
}

/// The `(Name, Value)` pairs one stack published, in the order the response carries them.
fn published_by(exports: &[WireExport], stack_id: &str) -> Vec<(String, Option<String>)> {
    exports
        .iter()
        .filter(|export| export.exporting_stack_id.as_deref() == Some(stack_id))
        .map(|export| {
            (
                export.name.clone().unwrap_or_default(),
                export.value.clone(),
            )
        })
        .collect()
}

/// One row of the table: a stack to create, and every export it must publish once its
/// create has completed.
struct ExportCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    /// The exports the stack publishes, as `(Name, Value)`, in `Name` order.
    expected: Vec<(&'static str, Option<&'static str>)>,
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The class the rule covers: any template, in either spelling, declaring any number of
/// outputs, of which any subset declares an export name.
fn export_cases() -> Vec<ExportCase> {
    vec![
        ExportCase {
            case: "json template, one output exported under a literal name",
            stack_name: "json-one-export",
            template: r#"{"Resources":{"T":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Endpoint":{"Value":"https://example.test/v1",
                   "Export":{"Name":"json-one-endpoint"}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![("json-one-endpoint", Some("https://example.test/v1"))],
        },
        ExportCase {
            case: "yaml template with short-form intrinsics, two outputs exported",
            stack_name: "yaml-two-exports",
            template: concat!(
                "Parameters:\n",
                "  Alpha:\n",
                "    Type: String\n",
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::CloudFormation::WaitConditionHandle\n",
                "Outputs:\n",
                "  First:\n",
                "    Value: !Ref Alpha\n",
                "    Export:\n",
                "      Name: yaml-first\n",
                "  Second:\n",
                "    Value: literal-second\n",
                "    Export:\n",
                "      Name: yaml-second\n",
            )
            .to_string(),
            parameters: params(&[("Alpha", "alpha value")]),
            expected: vec![
                ("yaml-first", Some("alpha value")),
                ("yaml-second", Some("literal-second")),
            ],
        },
        ExportCase {
            case: "only the outputs that declare an Export name are published",
            stack_name: "partly-exported",
            template: r#"{"Outputs":{"Exported":{"Value":"exported-value",
                   "Export":{"Name":"partly-exported-name"}},
                 "Plain":{"Value":"unexported-value"}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![("partly-exported-name", Some("exported-value"))],
        },
        ExportCase {
            case: "an export name composed by an intrinsic function over a pseudo parameter",
            stack_name: "sub-named",
            template: r#"{"Outputs":{"Queue":{"Value":"sub-value",
                   "Export":{"Name":{"Fn::Sub":"${AWS::StackName}-queue"}}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![("sub-named-queue", Some("sub-value"))],
        },
        ExportCase {
            case: "an exported output whose value does not resolve is published without a Value",
            stack_name: "unresolved-value",
            template: r#"{"Outputs":{"Imported":{"Value":{"Fn::ImportValue":"no-such-export"},
                   "Export":{"Name":"unresolved-value-name"}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![("unresolved-value-name", None)],
        },
        ExportCase {
            case: "an output whose Export name does not resolve publishes no export",
            stack_name: "unresolved-name",
            template: r#"{"Outputs":{"Imported":{"Value":"has-a-value",
                   "Export":{"Name":{"Fn::ImportValue":"no-such-export"}}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![],
        },
        ExportCase {
            case: "a stack whose template declares no Outputs section publishes nothing",
            stack_name: "no-outputs",
            template: r#"{"Resources":{"T":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![],
        },
    ]
}

/// `CreateStack` members for a row's parameters.
fn parameter_members(parameters: &[(String, String)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value)) in parameters.iter().enumerate() {
        let n = index + 1;
        members.push(member(&format!("Parameters.member.{n}.ParameterKey"), key));
        members.push(member(&format!("Parameters.member.{n}.ParameterValue"), value));
    }
    members
}

#[tokio::test]
async fn list_exports_reports_the_exported_outputs_of_every_live_stack() {
    let router = router();

    // Created up front, so the answer every row is checked against is one holding every
    // other row's exports too: an export belongs to the account and Region, not to one
    // read.
    let started = Instant::now();
    let mut created: Vec<(ExportCase, String)> = Vec::new();
    for case in export_cases() {
        let extra = parameter_members(&case.parameters);
        let (status, body) = create_stack(&router, case.stack_name, &case.template, &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateStack answered no StackId: {body}",
                case.case
            )
        });
        created.push((case, stack_id));
    }

    // Before the creates settle, nothing is deployed yet, so nothing is exported: an
    // export exists because a COMPLETED deployment published it. Checked only while the
    // transitions are not yet due — past that instant the emulator owes the settled
    // answer, so a read that lost the race to the clock is not held to this.
    let before = exports_now("during the creates", &router).await;
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        for (case, stack_id) in &created {
            let published = published_by(&before, stack_id);
            assert!(
                published.is_empty(),
                "case {:?}: expected no export while the create is in progress, got {published:?}",
                case.case
            );
        }
    }

    wait_for_the_transition().await;

    let exports = exports_now("after the creates", &router).await;
    for (case, stack_id) in &created {
        let expected: Vec<(String, Option<String>)> = case
            .expected
            .iter()
            .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
            .collect();
        let published = published_by(&exports, stack_id);
        assert_eq!(
            published, expected,
            "case {:?}: expected exports {expected:?}, got {published:?}",
            case.case
        );
    }

    // Every reported export carries the three members that identify it, and the answer
    // holds exactly the rows' exports — no export nothing published.
    let expected_total: usize = export_cases()
        .iter()
        .map(|case| case.expected.len())
        .sum();
    assert_eq!(
        exports.len(),
        expected_total,
        "expected exactly the exports the stacks published, got {exports:?}"
    );
    for export in &exports {
        assert!(
            export.name.is_some() && export.exporting_stack_id.is_some(),
            "every export names itself and its exporting stack, got {export:?}"
        );
    }
    let names: Vec<Option<String>> = exports.iter().map(|export| export.name.clone()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "expected the exports in a stable total order, got {names:?}"
    );
}

#[tokio::test]
async fn an_export_is_withdrawn_once_its_stack_is_deleted() {
    let router = router();

    let template = r#"{"Outputs":{"Shared":{"Value":"shared-value",
         "Export":{"Name":"withdrawn-export"}}}}"#;
    let (status, body) = create_stack(&router, "withdrawn", template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;

    let exports = exports_now("while the stack is live", &router).await;
    assert_eq!(
        published_by(&exports, &stack_id),
        vec![(
            "withdrawn-export".to_string(),
            Some("shared-value".to_string())
        )],
        "case {:?}: expected the live stack's export, got {exports:?}",
        "while the stack is live"
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "withdrawn")]).await;
    assert_eq!(
        status, 200,
        "expected DeleteStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let exports = exports_now("after the deletion completed", &router).await;
    assert!(
        published_by(&exports, &stack_id).is_empty(),
        "case {:?}: expected the deleted stack to publish nothing, got {exports:?}",
        "after the deletion completed"
    );
}

#[tokio::test]
async fn exports_belong_to_the_region_their_stack_was_created_in() {
    let router = router();
    // Two Regions, each with a stack exporting under the SAME name: an export is scoped to
    // the account and Region, so each read sees only its own.
    let other_region = "eu-west-1";
    let template = |value: &str| {
        format!(
            r#"{{"Outputs":{{"Regional":{{"Value":"{value}","Export":{{"Name":"regional-export"}}}}}}}}"#
        )
    };

    let (status, body) = create_stack(&router, "home", &template("home-value"), &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack in the home Region to succeed, got {status} {body}"
    );
    let home_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");

    let (status, body) = call_in(
        &router,
        other_region,
        "CreateStack",
        &[
            member("StackName", "away"),
            member("TemplateBody", &template("away-value")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateStack in {other_region} to succeed, got {status} {body}"
    );
    let away_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;

    for (case, region, exporting_stack, value) in [
        ("the home Region", REGION, &home_id, "home-value"),
        ("the other Region", other_region, &away_id, "away-value"),
    ] {
        let (status, body) = call_in(&router, region, "ListExports", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListExports to succeed, got {status} {body}"
        );
        let exports = wire_exports(&body);
        assert_eq!(
            exports,
            vec![WireExport {
                exporting_stack_id: Some(exporting_stack.clone()),
                name: Some("regional-export".to_string()),
                value: Some(value.to_string()),
            }],
            "case {case:?}: expected only that Region's export, got {exports:?}"
        );
    }
}

#[tokio::test]
async fn exports_are_paged_and_a_foreign_next_token_is_refused() {
    let router = router();

    // One stack publishing more exports than a page carries, so the whole collection takes
    // two pages. Each name is zero-padded, so the order the names sort in is the order the
    // numbers do and a case can say which names each page must hold.
    let count = PAGE_SIZE + 20;
    let declarations: Vec<String> = (0..count)
        .map(|index| {
            format!(
                r#""O{index:03}":{{"Value":"v{index:03}","Export":{{"Name":"paged-{index:03}"}}}}"#
            )
        })
        .collect();
    let template = format!(r#"{{"Outputs":{{{}}}}}"#, declarations.join(","));
    let (status, body) = create_stack(&router, "paged", &template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, body) = list_exports(&router, None).await;
    assert_eq!(
        status, 200,
        "expected the first page to succeed, got {status} {body}"
    );
    let first = wire_exports(&body);
    assert_eq!(
        first.len(),
        PAGE_SIZE,
        "expected a first page of {PAGE_SIZE} exports, got {}",
        first.len()
    );
    let token = xml_text(&body, "NextToken")
        .unwrap_or_else(|| panic!("expected a NextToken past the first page: {body}"));

    let (status, body) = list_exports(&router, Some(&token)).await;
    assert_eq!(
        status, 200,
        "expected the second page to succeed, got {status} {body}"
    );
    let second = wire_exports(&body);
    assert_eq!(
        second.len(),
        count - PAGE_SIZE,
        "expected the rest of the exports on the second page, got {}",
        second.len()
    );
    assert!(
        xml_text(&body, "NextToken").is_none(),
        "expected no NextToken on the last page: {body}"
    );

    let paged: Vec<String> = first
        .iter()
        .chain(second.iter())
        .map(|export| export.name.clone().unwrap_or_default())
        .collect();
    let expected: Vec<String> = (0..count).map(|index| format!("paged-{index:03}")).collect();
    assert_eq!(
        paged, expected,
        "expected both pages to cover every export exactly once, got {paged:?}"
    );

    // A token this collection never issued is a bad request, not an empty page: the
    // cursor a `ListStacks` response hands back counts into another collection.
    let (status, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(
        status, 200,
        "expected ListStacks to succeed, got {status} {body}"
    );
    for foreign in ["not-a-token", "6e6f7065"] {
        let (status, body) = list_exports(&router, Some(foreign)).await;
        assert_eq!(
            status, 400,
            "case {foreign:?}: expected a foreign NextToken to be refused, got {status} {body}"
        );
        assert!(
            body.contains(foreign),
            "case {foreign:?}: expected the refusal to name the token, got {body}"
        );
    }
}
