// SPDX-License-Identifier: CC0-1.0

//! An `Fn::ImportValue` naming an export NOTHING publishes, end to end over the awsQuery
//! wire.
//!
//! RULE under test: **`Fn::ImportValue` resolves against the exported output values in force
//! in the account and Region the deployment runs in, so an export name no LIVE stack of that
//! Region publishes has no value to import and CloudFormation cannot carry the deployment
//! out: it refuses it before anything is asked of the services that own the resources, the
//! stack reports `No export named <export> found.` as its reason, and the create does not
//! reach `CREATE_COMPLETE` — it fails and rolls back to `ROLLBACK_COMPLETE` (and an UPDATE
//! to `UPDATE_ROLLBACK_COMPLETE`, leaving the configuration in force). An import of a name
//! that IS published resolves to that export's value and the deployment completes.**
//!
//! Why it matters: a typo'd or not-yet-deployed export must not produce a stack that reports
//! success while the dependent resource was configured from a value that does not exist.
//!
//! The rows differ along what the rule generalises over: WHERE the import stands (a tag
//! value of a resource an owning service really creates, a scalar property of another
//! resource type, an output value, nested inside another intrinsic), whether the export NAME
//! is written out or composed by an intrinsic, how many imports the template carries and how
//! many of them resolve (including two unresolved ones, where the failure names the one the
//! template names FIRST), and whether the name is published at all — with a published name as
//! the control row of every shape. The deploying operations are covered beside them, since
//! the rule is the DEPLOYMENT's rather than one call's: `CreateStack`, `UpdateStack`,
//! `ExecuteChangeSet` and a StackSet's `CreateStackInstances`.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::detailed_status;
use service_com_amazonaws_cloudformation_cloudformation::state::status;

use crate::change_set_lifecycle::{create_creating_change_set, execute_change_set};
use crate::stack_lifecycle::{
    call, call_in, create_stack, describe_stack, member, router, wait_for_the_transition,
    xml_blocks, xml_text, xml_texts, REGION,
};
use crate::stack_resources::element;

/// The reason a deployment whose import names nothing reports, as AWS words it: the fixed
/// sentence around the export name the template imported. Written out here rather than read
/// off the emulator's own formatter, so the assertion pins the wording.
fn no_export_reason(export_name: &str) -> String {
    format!("No export named {export_name} found.")
}

/// The clause CloudFormation appends when it rolls a failed create back, which the rolled
/// back stack's event carries beside the failure itself.
const ROLLBACK_CLAUSE: &str = "Rollback requested by user.";

/// The exports the live exporter stack of these cases publishes, and the value each carries.
const PUBLISHED: [(&str, &str); 2] = [
    ("published-endpoint", "https://example.test/v1"),
    ("published-table", "orders-table"),
];

/// The exporting stack's template: one exported output per entry of [`PUBLISHED`], over a
/// resource no owning service has to provision.
fn exporter_template() -> String {
    let outputs: Vec<String> = PUBLISHED
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

/// One row of the table: a stack whose template imports something, and what the deployment
/// owes it.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The template the row deploys.
    template: String,
    /// The export name the failure must name, for a row whose import NOTHING publishes;
    /// `None` for a row whose imports all resolve, which must complete instead.
    unresolved: Option<String>,
    /// The logical id of the resource the imported value configures, where the row declares
    /// one: nothing may be asked of that resource's owning service for a refused deployment.
    carrier: Option<&'static str>,
    /// `(OutputKey, OutputValue)` the completed stack must report, which is what says the
    /// import really resolved.
    expected_output: Option<(&'static str, String)>,
}

/// The class the rule covers: any template importing any number of exports, from anywhere an
/// intrinsic function may appear, under a name written out or composed.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket tagged with an export nothing publishes",
            stack_name: "tagged-missing",
            template: r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket","Properties":{
                 "BucketName":"tagged-missing-bucket",
                 "Tags":[{"Key":"Table","Value":{"Fn::ImportValue":"never-declared-table"}}]}}}}"#
                .to_string(),
            unresolved: Some("never-declared-table".to_string()),
            carrier: Some("Data"),
            expected_output: None,
        },
        Row {
            case: "the same template against an export a live stack publishes",
            stack_name: "tagged-published",
            template: format!(
                r#"{{"Resources":{{"Data":{{"Type":"AWS::S3::Bucket","Properties":{{
                 "BucketName":"tagged-published-bucket",
                 "Tags":[{{"Key":"Table","Value":{{"Fn::ImportValue":"{}"}}}}]}}}}}},
                 "Outputs":{{"Imported":{{"Value":{{"Fn::ImportValue":"{}"}}}}}}}}"#,
                PUBLISHED[1].0, PUBLISHED[1].0
            ),
            unresolved: None,
            carrier: Some("Data"),
            expected_output: Some(("Imported", PUBLISHED[1].1.to_string())),
        },
        Row {
            case: "a scalar property of another resource type, unpublished",
            stack_name: "property-missing",
            template: r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{
                 "Type":"String","Value":{"Fn::ImportValue":"never-declared-endpoint"}}}}}"#
                .to_string(),
            unresolved: Some("never-declared-endpoint".to_string()),
            carrier: Some("Param"),
            expected_output: None,
        },
        Row {
            case: "an output value importing an export nothing publishes",
            stack_name: "output-missing",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"never-declared-output"}}}}"#
                .to_string(),
            unresolved: Some("never-declared-output".to_string()),
            carrier: None,
            expected_output: None,
        },
        Row {
            case: "an import nested inside another intrinsic, unpublished",
            stack_name: "nested-missing",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Joined":{"Value":{"Fn::Join":["/",
                   [{"Fn::ImportValue":"never-declared-nested"},"tail"]]}}}}"#
                .to_string(),
            unresolved: Some("never-declared-nested".to_string()),
            carrier: None,
            expected_output: None,
        },
        Row {
            case: "an import nested inside another intrinsic, published",
            stack_name: "nested-published",
            template: format!(
                r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                 "Outputs":{{"Joined":{{"Value":{{"Fn::Join":["/",
                   [{{"Fn::ImportValue":"{}"}},"tail"]]}}}}}}}}"#,
                PUBLISHED[0].0
            ),
            unresolved: None,
            carrier: None,
            expected_output: Some(("Joined", format!("{}/tail", PUBLISHED[0].1))),
        },
        Row {
            case: "an export name an intrinsic composes, resolving to a name nothing publishes",
            stack_name: "composed-missing",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Composed":{"Value":{"Fn::ImportValue":
                   {"Fn::Sub":"${AWS::StackName}-table"}}}}}"#
                .to_string(),
            // The name the import really resolved to, which is what the failure names: the
            // stack's own name with the suffix the `Fn::Sub` appends.
            unresolved: Some("composed-missing-table".to_string()),
            carrier: None,
            expected_output: None,
        },
        Row {
            case: "one unpublished import beside two that resolve",
            stack_name: "one-of-three-missing",
            template: format!(
                r#"{{"Resources":{{"Param":{{"Type":"AWS::SSM::Parameter","Properties":{{
                 "Type":"String","Value":{{"Fn::ImportValue":"{}"}}}}}}}},
                 "Outputs":{{"Endpoint":{{"Value":{{"Fn::ImportValue":"{}"}}}},
                   "Absent":{{"Value":{{"Fn::ImportValue":"never-declared-third"}}}}}}}}"#,
                PUBLISHED[1].0, PUBLISHED[0].0
            ),
            unresolved: Some("never-declared-third".to_string()),
            carrier: Some("Param"),
            expected_output: None,
        },
        Row {
            case: "two unpublished imports: the failure names the one the template names first",
            stack_name: "two-missing",
            // The import of the RESOURCE is declared first and sorts LAST of the two, so a
            // reason naming it can only come from the order the template declares them in.
            template: r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{
                 "Type":"String","Value":{"Fn::ImportValue":"zz-never-declared-first"}}}},
                 "Outputs":{"Second":{"Value":{"Fn::ImportValue":"aa-never-declared-second"}}}}"#
                .to_string(),
            unresolved: Some("zz-never-declared-first".to_string()),
            carrier: Some("Param"),
            expected_output: None,
        },
        Row {
            case: "a template that imports nothing at all",
            stack_name: "imports-nothing",
            template: r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{
                 "Type":"String","Value":"a plain literal"}}},
                 "Outputs":{"Plain":{"Value":"a plain output"}}}"#
                .to_string(),
            unresolved: None,
            carrier: Some("Param"),
            expected_output: Some(("Plain", "a plain output".to_string())),
        },
    ]
}

/// The `(ResourceStatus, ResourceStatusReason)` of every event one stack recorded for one
/// logical id, newest first.
async fn resource_events(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| element(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .map(|entry| {
            (
                element(entry, "ResourceStatus").unwrap_or_default(),
                element(entry, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every `StackStatusReason`-carrying event text of one stack, as one blob: what a client
/// polling the create reads.
async fn stack_event_reasons(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_texts(&body, "ResourceStatusReason").join(" | ")
}

/// The `(OutputKey, OutputValue)` pairs one stack reports.
fn reported_outputs(body: &str) -> Vec<(String, String)> {
    let keys = xml_texts(body, "OutputKey");
    let values = xml_texts(body, "OutputValue");
    keys.into_iter()
        .enumerate()
        .map(|(index, key)| (key, values.get(index).cloned().unwrap_or_default()))
        .collect()
}

/// Deploy the exporter whose exports the control rows import, and wait for them to be
/// published.
async fn deploy_exporter(router: &MultiServiceRouter, stack_name: &str) {
    let (code, body) = create_stack(router, stack_name, &exporter_template(), &[]).await;
    assert_eq!(
        code, 200,
        "expected CreateStack for the exporter to succeed, got {code} {body}"
    );
    wait_for_the_transition().await;
}

#[tokio::test]
async fn an_import_of_an_unpublished_export_fails_the_stack_and_rolls_it_back() {
    let router = router();
    deploy_exporter(&router, "exporter").await;

    for row in rows() {
        // "After the call completes successfully, the stack creation starts": the request is
        // valid and the stack exists, so the call is accepted — the DEPLOYMENT is what
        // reports the outcome.
        let (code, created) = create_stack(&router, row.stack_name, &row.template, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateStack to be accepted, got {code} {created}",
            row.case
        );
        assert!(
            xml_text(&created, "StackId").is_some_and(|id| id.contains(row.stack_name)),
            "case {:?}: expected CreateStack to answer the stack id, got {created}",
            row.case
        );
        // Twice: a failed create owes its rollback a transition of its own past the one the
        // create itself took.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (code, described) = describe_stack(&router, row.stack_name).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected DescribeStacks to succeed, got {code} {described}",
            row.case
        );
        let reported = xml_text(&described, "StackStatus").unwrap_or_default();
        let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();

        match &row.unresolved {
            Some(export_name) => {
                assert_eq!(
                    reported,
                    status::ROLLBACK_COMPLETE,
                    "case {:?}: an import of {export_name:?} must fail the create and roll it \
                     back, got {reported:?} ({reason:?})",
                    row.case
                );
                // The reason NAMES the export that could not be resolved, which is all a
                // client has to go on.
                assert_eq!(
                    reason,
                    no_export_reason(export_name),
                    "case {:?}: expected the unresolved export's own reason, got {reason:?}",
                    row.case
                );
                // The same failure is on the event stream a client polling the create reads,
                // with the clause CloudFormation adds when it rolls the create back.
                let events = stack_event_reasons(row.case, &router, row.stack_name).await;
                assert!(
                    events.contains(&no_export_reason(export_name))
                        && events.contains(ROLLBACK_CLAUSE),
                    "case {:?}: expected the failure and the rollback clause on the event \
                     stream, got {events:?}",
                    row.case
                );
                // Nothing was asked of the service that owns the resource the imported value
                // configures: CloudFormation refused the deployment before it deployed
                // anything, so that resource has no event of its own at all.
                if let Some(carrier) = row.carrier {
                    let carrier_events =
                        resource_events(row.case, &router, row.stack_name, carrier).await;
                    assert!(
                        carrier_events.is_empty(),
                        "case {:?}: expected no event for {carrier} — nothing was deployed — \
                         got {carrier_events:?}",
                        row.case
                    );
                }
                // A refused deployment publishes nothing either: a stack that rolled back
                // exports none of the outputs it declared.
                let outputs = reported_outputs(&described);
                assert!(
                    outputs.is_empty(),
                    "case {:?}: expected a rolled-back stack to report no outputs, got \
                     {outputs:?}",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    reported,
                    status::CREATE_COMPLETE,
                    "case {:?}: every import resolves, so the create must complete, got \
                     {reported:?} ({reason:?})",
                    row.case
                );
                if let Some((key, value)) = &row.expected_output {
                    let outputs = reported_outputs(&described);
                    assert!(
                        outputs.contains(&((*key).to_string(), value.clone())),
                        "case {:?}: expected the output {key:?} to carry {value:?}, got \
                         {outputs:?}",
                        row.case
                    );
                }
                // The resource the imported value configures was really asked for.
                if let Some(carrier) = row.carrier {
                    let carrier_events =
                        resource_events(row.case, &router, row.stack_name, carrier).await;
                    assert!(
                        !carrier_events.is_empty(),
                        "case {:?}: expected {carrier} to have been deployed, got no event",
                        row.case
                    );
                }
            }
        }
    }
}

/// An UPDATE whose new template imports an export nothing publishes is refused the same way:
/// the update fails naming the export and is rolled back, and the stack keeps the resources
/// and outputs the completed create left.
#[tokio::test]
async fn an_update_importing_an_unpublished_export_fails_and_rolls_back() {
    let case = "an update whose new template imports a name nothing exports";
    let router = router();
    deploy_exporter(&router, "update-exporter").await;

    let created = format!(
        r#"{{"Resources":{{"Param":{{"Type":"AWS::SSM::Parameter","Properties":{{
         "Type":"String","Value":{{"Fn::ImportValue":"{}"}}}}}}}},
         "Outputs":{{"Imported":{{"Value":{{"Fn::ImportValue":"{}"}}}}}}}}"#,
        PUBLISHED[0].0, PUBLISHED[0].0
    );
    let (code, body) = create_stack(&router, "updated-importer", &created, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "updated-importer").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected the create to complete, got {described}"
    );

    // The same template, with the import repointed at a name nothing exports.
    let updated = created.replace(PUBLISHED[0].0, "never-declared-on-update");
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated-importer"),
            member("TemplateBody", &updated),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (code, described) = describe_stack(&router, "updated-importer").await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {code} {described}"
    );
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the failed update to roll back, got {reported:?} in \
         {described}"
    );
    let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        no_export_reason("never-declared-on-update"),
        "case {case:?}: expected the unresolved export's own reason, got {reason:?}"
    );
    // The configuration in force is the one the create left: the failed update installed
    // nothing, so the output still carries the value it resolved then.
    let outputs = reported_outputs(&described);
    assert!(
        outputs.contains(&("Imported".to_string(), PUBLISHED[0].1.to_string())),
        "case {case:?}: expected the stack to keep the outputs the create left, got \
         {outputs:?}"
    );
}

/// An export belongs to ONE account and Region: a name published only in another Region is
/// not importable here, so a stack importing it fails exactly as one importing a name nobody
/// published does — while the same name published in the Region the request is signed for
/// resolves.
#[tokio::test]
async fn an_export_of_another_region_does_not_satisfy_an_import() {
    let case = "the export is published in another Region";
    let router = router();
    let other_region = "eu-west-1";
    let export_name = "regional-only-export";
    let exporter = format!(
        r#"{{"Outputs":{{"Regional":{{"Value":"regional-value",
         "Export":{{"Name":"{export_name}"}}}}}}}}"#
    );
    let importer = format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
         "Outputs":{{"Echoed":{{"Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}"#
    );

    // Published in the OTHER Region only.
    let (code, body) = call_in(
        &router,
        other_region,
        "CreateStack",
        &[
            member("StackName", "away-exporter"),
            member("TemplateBody", &exporter),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack for the away exporter to succeed, got {code} \
         {body}"
    );
    wait_for_the_transition().await;

    let (code, body) = create_stack(&router, "home-importer", &importer, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "home-importer").await;
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reported,
        status::ROLLBACK_COMPLETE,
        "case {case:?}: expected an import of another Region's export to fail the create, got \
         {reported:?} ({reason:?})"
    );
    assert_eq!(
        reason,
        no_export_reason(export_name),
        "case {case:?}: expected the unresolved export's own reason, got {reason:?}"
    );

    // ...and the same name published HERE resolves, which is what says the Region is the
    // only difference between the two cases.
    let case = "the same export name published in the Region the request is signed for";
    let (code, body) = call_in(
        &router,
        REGION,
        "CreateStack",
        &[
            member("StackName", "home-exporter"),
            member("TemplateBody", &exporter),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack for the home exporter to succeed, got {code} \
         {body}"
    );
    wait_for_the_transition().await;

    let (code, body) = create_stack(&router, "second-home-importer", &importer, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "second-home-importer").await;
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the import to resolve and the create to complete, got \
         {reported:?} in {described}"
    );
    let outputs = reported_outputs(&described);
    assert!(
        outputs.contains(&("Echoed".to_string(), "regional-value".to_string())),
        "case {case:?}: expected the output to carry the imported value, got {outputs:?}"
    );
}

/// Executing a CHANGE SET is the third way a template deploys, and the rule is the
/// deployment's: a change set whose template imports a name nothing exports is created and
/// executed — "after the call successfully completes, CloudFormation starts updating the
/// stack" — and the execution then fails naming the export and rolls the stack back.
#[tokio::test]
async fn a_change_set_execution_importing_an_unpublished_export_fails_and_rolls_back() {
    let case = "a CREATE change set whose template imports a name nothing exports";
    let router = router();
    deploy_exporter(&router, "change-set-exporter").await;

    let template = r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{
         "Type":"String","Value":{"Fn::ImportValue":"never-declared-on-execute"}}}}}"#;
    let (code, body) = create_creating_change_set(
        &router,
        "change-set-importer",
        "importing-change-set",
        template,
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateChangeSet to succeed, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (code, body) = execute_change_set(
        &router,
        "importing-change-set",
        Some("change-set-importer"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected ExecuteChangeSet to be accepted, got {code} {body}"
    );
    // Twice: a failed create owes its rollback a transition of its own.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (code, described) = describe_stack(&router, "change-set-importer").await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {code} {described}"
    );
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported,
        status::ROLLBACK_COMPLETE,
        "case {case:?}: expected the execution to fail and roll back, got {reported:?} in \
         {described}"
    );
    let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        no_export_reason("never-declared-on-execute"),
        "case {case:?}: expected the unresolved export's own reason, got {reason:?}"
    );
    // Nothing was asked of the service that owns the resource the imported value would have
    // configured.
    let carrier = resource_events(case, &router, "change-set-importer", "Param").await;
    assert!(
        carrier.is_empty(),
        "case {case:?}: expected no event for Param — nothing was deployed — got {carrier:?}"
    );
}

/// A STACK INSTANCE deploys the StackSet's template in its own target Region, so the same
/// rule holds for it: an instance whose template imports a name nothing exports in that
/// Region fails, and the instance reports that failure with the export's own reason.
#[tokio::test]
async fn a_stack_instance_importing_an_unpublished_export_fails() {
    let case = "a stack instance whose template imports a name nothing exports";
    let router = router();
    // A target account other than the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
    let account = "111111111111";
    let template = r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{
         "Type":"String","Value":{"Fn::ImportValue":"never-declared-in-instance"}}}}}"#;

    let (code, body) = call(
        &router,
        "CreateStackSet",
        &[
            member("StackSetName", "importing-set"),
            member("TemplateBody", template),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStackSet to succeed, got {code} {body}"
    );

    let (code, body) = call(
        &router,
        "CreateStackInstances",
        &[
            member("StackSetName", "importing-set"),
            member("Accounts.member.1", account),
            member("Regions.member.1", REGION),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStackInstances to be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (code, described) = call(
        &router,
        "DescribeStackInstance",
        &[
            member("StackSetName", "importing-set"),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", REGION),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackInstance to succeed, got {code} {described}"
    );
    assert_eq!(
        xml_text(&described, "DetailedStatus").as_deref(),
        Some(detailed_status::FAILED),
        "case {case:?}: expected the instance's deployment to have failed, got {described}"
    );
    let reason = xml_text(&described, "StatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        no_export_reason("never-declared-in-instance"),
        "case {case:?}: expected the unresolved export's own reason, got {reason:?}"
    );
    // ...and the instance's own stack reports the failure too, rather than a completion
    // reporting a resource nothing created.
    let stack_id = xml_text(&described, "StackId").unwrap_or_default();
    let (code, stack) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks for the instance's stack to succeed, got \
         {code} {stack}"
    );
    let reported = xml_text(&stack, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported,
        status::CREATE_FAILED,
        "case {case:?}: expected the instance's stack to report the failed create, got \
         {reported:?} in {stack}"
    );
}
