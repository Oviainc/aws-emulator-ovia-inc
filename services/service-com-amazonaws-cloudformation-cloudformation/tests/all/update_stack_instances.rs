// SPDX-License-Identifier: CC0-1.0

//! `UpdateStackInstances` over the awsQuery wire: overriding parameter values on the stack
//! instances a StackSet already holds.
//!
//! The rule every case here holds the service to: **`UpdateStackInstances` puts the request's
//! `ParameterOverrides` in force on exactly the stack instances the request selects — the
//! account and Region pairs the StackSet really holds an instance in — redeploys each one's
//! stack with the StackSet's own parameter values carrying those overrides on top, and runs
//! as ONE `UPDATE` StackSet operation whose `OperationId` it answers; a selected pair the
//! StackSet holds no instance in is refused `StackInstanceNotFoundException` and nothing of
//! the request is carried out.**
//!
//! The table below spans the class that rule generalises over rather than one recorded
//! request: one pair, one account across two target Regions, two accounts in one Region, the
//! accounts named through `Accounts` and through `DeploymentTargets`, a selection narrower
//! than the StackSet's instances, a request repeating a pair, and each documented shape of
//! `ParameterOverrides` — a value, `UsePreviousValue`, a list that leaves a previously
//! overridden parameter out, and the member absent altogether. The refusals are a table of
//! their own: a pair no instance is deployed in, a pair whose instance a deletion has taken
//! away, a `StackSetName` nothing resolves, each way a request names its targets wrongly, an
//! `OperationId` the StackSet already holds, and an update asked for while another operation
//! is in flight.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, instance_status, operation_action, operation_status,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::MAX_PARAMETERS_PER_TEMPLATE;
use service_com_amazonaws_cloudformation_cloudformation::state::{status, STATUS_TRANSITION_DELAY};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router, wait_for_the_transition, xml_blocks, xml_text, ACCOUNT_ID,
    REGION,
};

/// The template every StackSet here is created from: a topic whose display name is a declared
/// parameter, so the value in force on an instance's stack is observable both as the stack's
/// `Parameters` and as the output it resolves.
const TEMPLATE: &str = r#"{"Parameters":{"DisplayName":{"Type":"String","Default":"template-display"},
     "Note":{"Type":"String","Default":"template-note"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"DisplayName"}}}},
     "Outputs":{"Display":{"Value":{"Ref":"DisplayName"}}}}"#;

/// The parameter values every StackSet here states, which an instance's overrides replace one
/// at a time — neither is the template's own `Default`, so a value falling back to the
/// template rather than keeping the StackSet's is visible.
const SET_DISPLAY: &str = "stackset-display";
const SET_NOTE: &str = "stackset-note";

/// Two target accounts beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";

/// Two target Regions beside [`REGION`]: a stack instance's Region is a target the deploying
/// request chooses, so the instance's stack lives in it and is read through it.
const WEST: &str = "us-west-2";
const IRELAND: &str = "eu-west-1";

/// One account and Region pair: how a stack instance is addressed within its StackSet.
type Pair = (&'static str, &'static str);

/// One entry of a request's `ParameterOverrides`: a key with the value to override it with,
/// or `None` for `UsePreviousValue` — "to leave an overridden parameter set to its present
/// value, include the parameter and specify UsePreviousValue as true".
type Override = (&'static str, Option<&'static str>);

/// One row of the update table.
struct Case {
    /// What the row is, named in every assertion it makes.
    case: &'static str,
    /// The StackSet the row deploys and then updates in, unique per row.
    stack_set_name: &'static str,
    /// The accounts the row's instances are deployed to.
    deployed_accounts: Vec<&'static str>,
    /// The Regions the row's instances are deployed to; its instances are the product.
    deployed_regions: Vec<&'static str>,
    /// A FIRST update that seeds overrides on every deployed instance, for a row about what a
    /// later update does to overrides already in force.
    seed: Option<Vec<Override>>,
    /// The accounts the row's update names (which may repeat one).
    accounts: Vec<&'static str>,
    /// Whether it names them through `DeploymentTargets.Accounts` rather than `Accounts` —
    /// "you can specify Accounts or DeploymentTargets, but not both".
    through_deployment_targets: bool,
    /// The Regions the row's update names (which may repeat one).
    regions: Vec<&'static str>,
    /// The `ParameterOverrides` the update names, or `None` for the member left out
    /// altogether — "to leave all parameters set to their present values, don't specify this
    /// property at all".
    overrides: Option<Vec<Override>>,
    /// The pairs the update must act on, and the `ParameterOverrides` each must then report.
    updated: Vec<Pair>,
    expected_overrides: Vec<(&'static str, &'static str)>,
    /// The parameter values each updated instance's STACK must then have in force.
    expected_in_force: Vec<(&'static str, &'static str)>,
    /// A pair of the same StackSet the update does not select, which must be left exactly as
    /// the deployment left it.
    untouched: Vec<Pair>,
}

/// The pairs a row's deployment puts in its StackSet: the product of its accounts and Regions.
fn deployed_pairs(case: &Case) -> Vec<Pair> {
    let mut pairs: Vec<Pair> = Vec::new();
    for account in &case.deployed_accounts {
        for region in &case.deployed_regions {
            pairs.push((account, region));
        }
    }
    pairs
}

/// The table: each row selects its instances differently and names its overrides in a
/// different one of the documented shapes, and states exactly what each instance it acts on
/// must report afterwards.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one pair, one of the StackSet's two parameters overridden",
            stack_set_name: "update-one-pair",
            deployed_accounts: vec![ACCOUNT_A],
            deployed_regions: vec![REGION],
            seed: None,
            accounts: vec![ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION],
            overrides: Some(vec![("DisplayName", Some("one-pair-display"))]),
            updated: vec![(ACCOUNT_A, REGION)],
            expected_overrides: vec![("DisplayName", "one-pair-display")],
            expected_in_force: vec![("DisplayName", "one-pair-display"), ("Note", SET_NOTE)],
            untouched: vec![],
        },
        Case {
            case: "one account across two target Regions, both parameters overridden",
            stack_set_name: "update-two-regions",
            deployed_accounts: vec![ACCOUNT_A],
            deployed_regions: vec![REGION, WEST],
            seed: None,
            accounts: vec![ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION, WEST],
            overrides: Some(vec![
                ("DisplayName", Some("two-regions-display")),
                ("Note", Some("two-regions-note")),
            ]),
            updated: vec![(ACCOUNT_A, REGION), (ACCOUNT_A, WEST)],
            expected_overrides: vec![
                ("DisplayName", "two-regions-display"),
                ("Note", "two-regions-note"),
            ],
            expected_in_force: vec![
                ("DisplayName", "two-regions-display"),
                ("Note", "two-regions-note"),
            ],
            untouched: vec![],
        },
        Case {
            case: "two accounts in one Region, named through DeploymentTargets",
            stack_set_name: "update-through-targets",
            deployed_accounts: vec![ACCOUNT_A, ACCOUNT_B],
            deployed_regions: vec![IRELAND],
            seed: None,
            accounts: vec![ACCOUNT_A, ACCOUNT_B],
            through_deployment_targets: true,
            regions: vec![IRELAND],
            overrides: Some(vec![("Note", Some("through-targets-note"))]),
            updated: vec![(ACCOUNT_A, IRELAND), (ACCOUNT_B, IRELAND)],
            expected_overrides: vec![("Note", "through-targets-note")],
            expected_in_force: vec![
                ("DisplayName", SET_DISPLAY),
                ("Note", "through-targets-note"),
            ],
            untouched: vec![],
        },
        Case {
            case: "a selection narrower than the StackSet's instances",
            stack_set_name: "update-one-of-two",
            deployed_accounts: vec![ACCOUNT_A, ACCOUNT_B],
            deployed_regions: vec![REGION],
            seed: None,
            accounts: vec![ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION],
            overrides: Some(vec![("DisplayName", Some("one-of-two-display"))]),
            updated: vec![(ACCOUNT_A, REGION)],
            expected_overrides: vec![("DisplayName", "one-of-two-display")],
            expected_in_force: vec![("DisplayName", "one-of-two-display"), ("Note", SET_NOTE)],
            untouched: vec![(ACCOUNT_B, REGION)],
        },
        Case {
            case: "a request repeating an account and a Region, which names one pair",
            stack_set_name: "update-repeated-pair",
            deployed_accounts: vec![ACCOUNT_A],
            deployed_regions: vec![REGION],
            seed: None,
            accounts: vec![ACCOUNT_A, ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION, REGION],
            overrides: Some(vec![("DisplayName", Some("repeated-display"))]),
            updated: vec![(ACCOUNT_A, REGION)],
            expected_overrides: vec![("DisplayName", "repeated-display")],
            expected_in_force: vec![("DisplayName", "repeated-display"), ("Note", SET_NOTE)],
            untouched: vec![],
        },
        Case {
            case: "UsePreviousValue keeps the value the instance already overrides",
            stack_set_name: "update-use-previous",
            deployed_accounts: vec![ACCOUNT_A],
            deployed_regions: vec![WEST],
            seed: Some(vec![("DisplayName", Some("seeded-display"))]),
            accounts: vec![ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![WEST],
            overrides: Some(vec![("DisplayName", None)]),
            updated: vec![(ACCOUNT_A, WEST)],
            expected_overrides: vec![("DisplayName", "seeded-display")],
            expected_in_force: vec![("DisplayName", "seeded-display"), ("Note", SET_NOTE)],
            untouched: vec![],
        },
        Case {
            case: "a list leaving out an overridden parameter sets it back to the StackSet's \
                   value",
            stack_set_name: "update-back-to-the-set",
            deployed_accounts: vec![ACCOUNT_B],
            deployed_regions: vec![REGION],
            seed: Some(vec![
                ("DisplayName", Some("seeded-display")),
                ("Note", Some("seeded-note")),
            ]),
            accounts: vec![ACCOUNT_B],
            through_deployment_targets: false,
            regions: vec![REGION],
            overrides: Some(vec![("Note", Some("kept-note"))]),
            updated: vec![(ACCOUNT_B, REGION)],
            expected_overrides: vec![("Note", "kept-note")],
            expected_in_force: vec![("DisplayName", SET_DISPLAY), ("Note", "kept-note")],
            untouched: vec![],
        },
        Case {
            case: "no ParameterOverrides at all leaves every override as it is",
            stack_set_name: "update-no-overrides",
            deployed_accounts: vec![ACCOUNT_B],
            deployed_regions: vec![IRELAND],
            seed: Some(vec![("DisplayName", Some("seeded-display"))]),
            accounts: vec![ACCOUNT_B],
            through_deployment_targets: false,
            regions: vec![IRELAND],
            overrides: None,
            updated: vec![(ACCOUNT_B, IRELAND)],
            expected_overrides: vec![("DisplayName", "seeded-display")],
            expected_in_force: vec![("DisplayName", "seeded-display"), ("Note", SET_NOTE)],
            untouched: vec![],
        },
    ]
}

/// `CreateStackSet` with the shared template and the parameter values every row's StackSet
/// states.
async fn create_stack_set(router: &MultiServiceRouter, stack_set_name: &str) -> (u16, String) {
    call(
        router,
        "CreateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("TemplateBody", TEMPLATE),
            member("Parameters.member.1.ParameterKey", "DisplayName"),
            member("Parameters.member.1.ParameterValue", SET_DISPLAY),
            member("Parameters.member.2.ParameterKey", "Note"),
            member("Parameters.member.2.ParameterValue", SET_NOTE),
        ],
    )
    .await
}

/// `CreateStackInstances` for one product of accounts and Regions, with no overrides: the
/// instances a row's update acts on are instances a deployment really created.
async fn create_stack_instances(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    for (index, account) in accounts.iter().enumerate() {
        members.push(member(&format!("Accounts.member.{}", index + 1), account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    call(router, "CreateStackInstances", &members).await
}

/// The `UpdateStackInstances` request members for one selection and one set of overrides.
fn update_members(
    stack_set_name: &str,
    accounts: &[&str],
    through_deployment_targets: bool,
    regions: &[&str],
    overrides: Option<&[Override]>,
    operation_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut members = vec![member("StackSetName", stack_set_name)];
    for (index, account) in accounts.iter().enumerate() {
        let key = if through_deployment_targets {
            format!("DeploymentTargets.Accounts.member.{}", index + 1)
        } else {
            format!("Accounts.member.{}", index + 1)
        };
        members.push(member(&key, account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    for (index, (key, value)) in overrides.unwrap_or_default().iter().enumerate() {
        let entry = format!("ParameterOverrides.member.{}", index + 1);
        members.push(member(&format!("{entry}.ParameterKey"), key));
        match value {
            Some(value) => members.push(member(&format!("{entry}.ParameterValue"), value)),
            None => members.push(member(&format!("{entry}.UsePreviousValue"), "true")),
        }
    }
    if let Some(operation_id) = operation_id {
        members.push(member("OperationId", operation_id));
    }
    members
}

/// The row's own update request.
fn case_members(case: &Case, operation_id: Option<&str>) -> Vec<(String, String)> {
    update_members(
        case.stack_set_name,
        &case.accounts,
        case.through_deployment_targets,
        &case.regions,
        case.overrides.as_deref(),
        operation_id,
    )
}

/// `DescribeStackInstance` for one pair: the status and the body, so a case can read a member
/// off it or assert the refusal for an instance that is gone.
async fn describe_stack_instance(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    account: &str,
    region: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackInstance",
        &[
            member("StackSetName", stack_set_name),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", region),
        ],
    )
    .await
}

/// `DescribeStackSetOperation` for one operation of one StackSet.
async fn describe_operation(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackSetOperation",
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// The key/value pairs of the first `tag` list of `xml`, sorted: a parameter list's entries
/// are a SET, so the order the service walked the template in is not what is asserted.
fn parameter_pairs(xml: &str, tag: &str) -> Vec<(String, String)> {
    let Some(block) = xml_blocks(xml, tag).into_iter().next() else {
        return Vec::new();
    };
    let mut pairs: Vec<(String, String)> = xml_blocks(&block, "member")
        .iter()
        .filter_map(|entry| {
            Some((
                xml_text(entry, "ParameterKey")?,
                xml_text(entry, "ParameterValue")?,
            ))
        })
        .collect();
    pairs.sort();
    pairs
}

/// The expected pairs of a row, in the same sorted form.
fn expected_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// One instance's stack as `DescribeStacks` reports it, read in the Region the stack lives in
/// — a stack is addressable only through its own Region.
async fn describe_instance_stack(
    router: &MultiServiceRouter,
    region: &str,
    stack_id: &str,
) -> String {
    let (status, body) = call_in(
        router,
        region,
        "DescribeStacks",
        &[member("StackName", stack_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeStacks for {stack_id} to succeed, got {status} {body}"
    );
    body
}

/// The `StackId` of the instance deployed in `account` and `region`.
async fn instance_stack_id(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    account: &str,
    region: &str,
) -> String {
    let (status, body) = describe_stack_instance(router, stack_set_name, account, region).await;
    assert_eq!(
        status, 200,
        "expected the instance {account}/{region} of {stack_set_name} to be described, got \
         {status} {body}"
    );
    xml_text(&body, "StackId")
        .unwrap_or_else(|| panic!("DescribeStackInstance answered no StackId: {body}"))
}

#[tokio::test]
async fn updating_stack_instances_puts_the_overrides_in_force_on_the_selected_instances() {
    let router = router();
    let cases = cases();

    for case in &cases {
        let (status, body) = create_stack_set(&router, case.stack_set_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStackSet to succeed, got {status} {body}",
            case.case
        );
        let (status, body) = create_stack_instances(
            &router,
            case.stack_set_name,
            &case.deployed_accounts,
            &case.deployed_regions,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStackInstances to succeed, got {status} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    // The rows that are about overrides ALREADY in force seed them with an update of their
    // own, so what the row's own update does is read against a state a request really put
    // there.
    let mut seeded = false;
    for case in &cases {
        let Some(seed) = case.seed.as_deref() else {
            continue;
        };
        let (status, body) = call(
            &router,
            "UpdateStackInstances",
            &update_members(
                case.stack_set_name,
                &case.deployed_accounts,
                false,
                &case.deployed_regions,
                Some(seed),
                None,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the seeding update to succeed, got {status} {body}",
            case.case
        );
        seeded = true;
    }
    if seeded {
        wait_for_the_transition().await;
    }

    // What each instance's stack is BEFORE the row's update, so the update can be held to
    // what became of it.
    let mut stacks: Vec<(&'static str, Pair, String)> = Vec::new();
    for case in &cases {
        for (account, region) in deployed_pairs(case) {
            let stack_id = instance_stack_id(&router, case.stack_set_name, account, region).await;
            stacks.push((case.stack_set_name, (account, region), stack_id));
        }
    }

    // The update itself, one row at a time: each row's StackSet is its own, so the operations
    // the rows start all run at once and one wait covers them.
    let mut updates: Vec<(&Case, String)> = Vec::new();
    for case in &cases {
        let started = Instant::now();
        let (status, body) = call(&router, "UpdateStackInstances", &case_members(case, None)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected UpdateStackInstances to succeed, got {status} {body}",
            case.case
        );
        let operation_id = xml_text(&body, "OperationId").unwrap_or_else(|| {
            panic!(
                "case {:?}: UpdateStackInstances answered no OperationId: {body}",
                case.case
            )
        });

        // The operation the call started is a record of its own, reported with the action
        // that started it.
        let (status, body) = describe_operation(&router, case.stack_set_name, &operation_id).await;
        // Whether the read above certainly happened before the conclusion became due: the
        // emulator owes the in-flight answer only until that instant, so a read that lost the
        // race to the clock is not held to the transient state.
        let in_flight = started.elapsed() < STATUS_TRANSITION_DELAY;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::UPDATE),
            "case {:?}: expected the update's own action, got {body}",
            case.case
        );
        if in_flight {
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(operation_status::RUNNING),
                "case {:?}: expected the update to still be in flight, got {body}",
                case.case
            );
        }
        // While the operation is in flight the instances it is updating report it as not yet
        // started in their account and Region: the operation's conclusion is what carries it
        // out there.
        for (account, region) in &case.updated {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            if started.elapsed() >= STATUS_TRANSITION_DELAY {
                continue;
            }
            assert_eq!(
                status, 200,
                "case {:?}: expected {account}/{region} to be described while the update \
                 runs, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "DetailedStatus").as_deref(),
                Some(detailed_status::PENDING),
                "case {:?}: expected the update to have yet to start in {account}/{region}, \
                 got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "LastOperationId").as_deref(),
                Some(operation_id.as_str()),
                "case {:?}: expected {account}/{region} to report the update as its last \
                 operation, got {body}",
                case.case
            );
        }
        updates.push((case, operation_id));
    }

    wait_for_the_transition().await;

    for (case, operation_id) in &updates {
        let (status, body) = describe_operation(&router, case.stack_set_name, operation_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::SUCCEEDED),
            "case {:?}: expected the update to have concluded, got {body}",
            case.case
        );

        for (account, region) in &case.updated {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected {account}/{region} to be described, got {status} {body}",
                case.case
            );
            // "A list of parameters from the StackSet template whose values have been
            // overridden in this stack instance": exactly what this update left in force.
            assert_eq!(
                parameter_pairs(&body, "ParameterOverrides"),
                expected_pairs(&case.expected_overrides),
                "case {:?}: expected {account}/{region} to report the overrides this update \
                 put in force, got {body}",
                case.case
            );
            // The deployment concluded, so the instance is up to date with its StackSet
            // again.
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(instance_status::CURRENT),
                "case {:?}: expected {account}/{region} to be up to date with its StackSet, \
                 got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "DetailedStatus").as_deref(),
                Some(detailed_status::SUCCEEDED),
                "case {:?}: expected the update to have succeeded in {account}/{region}, got \
                 {body}",
                case.case
            );

            // The instance's STACK carries the values the update put in force — the
            // StackSet's own, with the overrides on top — and really redeployed its template
            // under them, which is what its resolved output reports.
            let stack_id = stacks
                .iter()
                .find(|(name, pair, _)| {
                    *name == case.stack_set_name && pair == &(*account, *region)
                })
                .map(|(_, _, stack_id)| stack_id.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: no stack was recorded for {account}/{region}",
                        case.case
                    )
                });
            let stack = describe_instance_stack(&router, region, &stack_id).await;
            assert_eq!(
                parameter_pairs(&stack, "Parameters"),
                expected_pairs(&case.expected_in_force),
                "case {:?}: expected the stack of {account}/{region} to carry the values in \
                 force, got {stack}",
                case.case
            );
            assert_eq!(
                xml_text(&stack, "StackStatus").as_deref(),
                Some(status::UPDATE_COMPLETE),
                "case {:?}: expected the stack of {account}/{region} to report a completed \
                 update, got {stack}",
                case.case
            );
            let display = case
                .expected_in_force
                .iter()
                .find(|(key, _)| *key == "DisplayName")
                .map(|(_, value)| *value)
                .expect("every row states the DisplayName in force");
            assert_eq!(
                xml_text(&stack, "OutputValue").as_deref(),
                Some(display),
                "case {:?}: expected the stack of {account}/{region} to resolve its output \
                 from the value in force, got {stack}",
                case.case
            );
        }

        // An instance the selection never reached is untouched: no overrides of its own, and
        // its stack still carrying the StackSet's own values.
        for (account, region) in &case.untouched {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected {account}/{region} to be left alone, got {status} {body}",
                case.case
            );
            assert!(
                parameter_pairs(&body, "ParameterOverrides").is_empty(),
                "case {:?}: expected {account}/{region} to carry no overrides, got {body}",
                case.case
            );
            assert_ne!(
                xml_text(&body, "LastOperationId").as_deref(),
                Some(operation_id.as_str()),
                "case {:?}: expected {account}/{region} not to have been touched by the \
                 update, got {body}",
                case.case
            );
            let stack_id = instance_stack_id(&router, case.stack_set_name, account, region).await;
            let stack = describe_instance_stack(&router, region, &stack_id).await;
            assert_eq!(
                parameter_pairs(&stack, "Parameters"),
                expected_pairs(&[("DisplayName", SET_DISPLAY), ("Note", SET_NOTE)]),
                "case {:?}: expected the stack of {account}/{region} to keep the StackSet's \
                 own values, got {stack}",
                case.case
            );
            assert_eq!(
                xml_text(&stack, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {:?}: expected the stack of {account}/{region} not to have been \
                 updated, got {stack}",
                case.case
            );
        }
    }
}

/// One row of the refusal table: the request members, and the HTTP status and error code it
/// must be answered with.
struct Refusal {
    case: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    code: Option<&'static str>,
}

#[tokio::test]
async fn an_update_a_stack_set_cannot_perform_is_refused() {
    let router = router();
    let stack_set_name = "update-refusals";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );
    let (status, body) =
        create_stack_instances(&router, stack_set_name, &[ACCOUNT_ID], &[REGION]).await;
    assert_eq!(
        status, 200,
        "expected CreateStackInstances to succeed, got {status} {body}"
    );
    let create_operation = xml_text(&body, "OperationId")
        .unwrap_or_else(|| panic!("CreateStackInstances answered no OperationId: {body}"));

    // One well-formed update of the instance just deployed, reused by the rows that are about
    // something other than the request's own shape.
    let well_formed = update_members(
        stack_set_name,
        &[ACCOUNT_ID],
        false,
        &[REGION],
        Some(&[("DisplayName", Some("refusals-display"))]),
        None,
    );

    // "Only one operation can be performed for a stack set at a given time": the deployment
    // above is still in flight, and the refusal names the StackSet and the operation holding
    // it.
    let (status, body) = call(&router, "UpdateStackInstances", &well_formed).await;
    assert_eq!(
        status, 409,
        "case {:?}: expected the update to be refused, got {status} {body}",
        "an update while another operation is in flight"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("OperationInProgressException"),
        "case {:?}: expected the modelled refusal, got {body}",
        "an update while another operation is in flight"
    );
    let stack_set_arn = {
        let (status, described) = call(
            &router,
            "DescribeStackSet",
            &[member("StackSetName", stack_set_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "expected DescribeStackSet to succeed, got {status} {described}"
        );
        xml_text(&described, "StackSetARN")
            .unwrap_or_else(|| panic!("DescribeStackSet answered no StackSetARN: {described}"))
    };
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(
            format!(
                "Another Operation on StackSet {stack_set_arn} is in progress: {create_operation}"
            )
            .as_str()
        ),
        "case {:?}: expected the refusal to name the StackSet and the operation holding it, \
         got {body}",
        "an update while another operation is in flight"
    );

    wait_for_the_transition().await;

    // Each way a request names a StackSet, its targets or its instances that the service
    // cannot act on.
    for refusal in [
        Refusal {
            case: "a StackSetName no StackSet of this Region carries",
            members: update_members(
                "no-such-stack-set",
                &[ACCOUNT_ID],
                false,
                &[REGION],
                None,
                None,
            ),
            status: 404,
            code: Some("StackSetNotFoundException"),
        },
        Refusal {
            case: "an account the StackSet holds no instance in",
            members: update_members(stack_set_name, &[ACCOUNT_A], false, &[REGION], None, None),
            status: 404,
            code: Some("StackInstanceNotFoundException"),
        },
        Refusal {
            case: "a Region the StackSet holds no instance in",
            members: update_members(stack_set_name, &[ACCOUNT_ID], false, &[WEST], None, None),
            status: 404,
            code: Some("StackInstanceNotFoundException"),
        },
        Refusal {
            case: "a selection whose product includes a pair no instance is deployed in",
            members: update_members(
                stack_set_name,
                &[ACCOUNT_ID],
                false,
                &[REGION, IRELAND],
                None,
                None,
            ),
            status: 404,
            code: Some("StackInstanceNotFoundException"),
        },
        Refusal {
            case: "both Accounts and DeploymentTargets",
            members: {
                let mut members = update_members(
                    stack_set_name,
                    &[ACCOUNT_ID],
                    false,
                    &[REGION],
                    None,
                    None,
                );
                members.push(member(
                    "DeploymentTargets.Accounts.member.1",
                    ACCOUNT_A,
                ));
                members
            },
            status: 400,
            code: None,
        },
        Refusal {
            case: "neither Accounts nor DeploymentTargets",
            members: vec![
                member("StackSetName", stack_set_name),
                member("Regions.member.1", REGION),
            ],
            status: 400,
            code: None,
        },
        Refusal {
            case: "DeploymentTargets naming an AccountsUrl nothing reads",
            members: vec![
                member("StackSetName", stack_set_name),
                member("DeploymentTargets.AccountsUrl", "s3://accounts/list.txt"),
                member("Regions.member.1", REGION),
            ],
            status: 400,
            code: None,
        },
        Refusal {
            case: "no Regions",
            members: vec![
                member("StackSetName", stack_set_name),
                member("Accounts.member.1", ACCOUNT_ID),
            ],
            status: 400,
            code: None,
        },
    ] {
        let (status, body) = call(&router, "UpdateStackInstances", &refusal.members).await;
        assert_eq!(
            status, refusal.status,
            "case {:?}: expected the request to be refused with {}, got {status} {body}",
            refusal.case, refusal.status
        );
        if let Some(code) = refusal.code {
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some(code),
                "case {:?}: expected <Code>{code}</Code>, got {body}",
                refusal.case
            );
        }
    }

    // A refused update carries nothing out: the instance the StackSet really holds still has
    // no overrides, and still reports the deployment as its last operation.
    let (status, body) =
        describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, REGION).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the deployed instance to be described, got {status} {body}",
        "a refused update changes nothing"
    );
    assert!(
        parameter_pairs(&body, "ParameterOverrides").is_empty(),
        "case {:?}: expected the instance to carry no overrides, got {body}",
        "a refused update changes nothing"
    );
    assert_eq!(
        xml_text(&body, "LastOperationId").as_deref(),
        Some(create_operation.as_str()),
        "case {:?}: expected the deployment to still be the instance's last operation, got \
         {body}",
        "a refused update changes nothing"
    );

    // "The specified operation ID already exists": the id names the operation that already
    // ran, so the same id does not start a second one.
    let operation_id = "update-one";
    let named = update_members(
        stack_set_name,
        &[ACCOUNT_ID],
        false,
        &[REGION],
        Some(&[("DisplayName", Some("named-display"))]),
        Some(operation_id),
    );
    let (status, body) = call(&router, "UpdateStackInstances", &named).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the first update to succeed, got {status} {body}",
        "an OperationId the StackSet already holds"
    );
    assert_eq!(
        xml_text(&body, "OperationId").as_deref(),
        Some(operation_id),
        "case {:?}: expected the operation id the request named, got {body}",
        "an OperationId the StackSet already holds"
    );

    wait_for_the_transition().await;

    let (status, body) = call(&router, "UpdateStackInstances", &named).await;
    assert_eq!(
        status, 409,
        "case {:?}: expected the repeated id to be refused, got {status} {body}",
        "an OperationId the StackSet already holds"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("OperationIdAlreadyExistsException"),
        "case {:?}: expected the modelled refusal, got {body}",
        "an OperationId the StackSet already holds"
    );
}

/// An update naming `count` parameter overrides, each a distinct key: what the bound on the
/// overrides one request may name is read against.
fn overriding(count: usize) -> Vec<Override> {
    (0..count)
        .map(|index| -> Override {
            // Leaked so the row can be the `&'static str` the request builder takes; bounded
            // by the two counts this test uses.
            let key: &'static str = Box::leak(format!("Spare{index}").into_boxed_str());
            (key, Some("spare"))
        })
        .collect()
}

#[tokio::test]
async fn an_update_naming_more_overrides_than_a_template_can_declare_is_refused() {
    let router = router();
    let stack_set_name = "bounded-overrides";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );
    let (status, body) =
        create_stack_instances(&router, stack_set_name, &[ACCOUNT_ID], &[REGION]).await;
    assert_eq!(
        status, 200,
        "expected CreateStackInstances to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    // AT the documented per-template parameter quota (`MAX_PARAMETERS_PER_TEMPLATE`, the
    // ceiling `state::template` carries for "the maximum number of parameters that you can
    // declare in a template"): the request is served.
    let at_the_bound = overriding(MAX_PARAMETERS_PER_TEMPLATE);
    let (status, body) = call(
        &router,
        "UpdateStackInstances",
        &update_members(
            stack_set_name,
            &[ACCOUNT_ID],
            false,
            &[REGION],
            Some(&at_the_bound),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected {MAX_PARAMETERS_PER_TEMPLATE} overrides to be served, got \
         {status} {body}",
        "an override list at the bound"
    );
    assert!(
        xml_text(&body, "OperationId").is_some(),
        "case {:?}: expected the update to answer an operation id, got {body}",
        "an override list at the bound"
    );

    wait_for_the_transition().await;

    // ONE past it: no template the StackSet could deploy declares that many parameters, so
    // the request is refused before anything is read or written.
    let past_the_bound = overriding(MAX_PARAMETERS_PER_TEMPLATE + 1);
    let (status, body) = call(
        &router,
        "UpdateStackInstances",
        &update_members(
            stack_set_name,
            &[ACCOUNT_ID],
            false,
            &[REGION],
            Some(&past_the_bound),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: expected {} overrides to be refused, got {status} {body}",
        "an override list past the bound",
        MAX_PARAMETERS_PER_TEMPLATE + 1
    );
}

#[tokio::test]
async fn an_update_of_an_instance_a_deletion_took_away_is_refused() {
    let router = router();
    let stack_set_name = "update-after-delete";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );
    // Two instances, so the deletion of one leaves the StackSet holding the other: what is
    // refused is the UPDATE OF THE DELETED PAIR, not an update of a StackSet with nothing in
    // it.
    let (status, body) = create_stack_instances(
        &router,
        stack_set_name,
        &[ACCOUNT_A, ACCOUNT_B],
        &[REGION],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateStackInstances to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    // The pair is a real instance of the StackSet until the deletion concludes.
    let (status, body) = call(
        &router,
        "UpdateStackInstances",
        &update_members(
            stack_set_name,
            &[ACCOUNT_A],
            false,
            &[REGION],
            Some(&[("DisplayName", Some("before-the-deletion"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the update of a live instance to succeed, got {status} {body}",
        "an update of an instance a deletion took away"
    );
    wait_for_the_transition().await;

    let (status, body) = call(
        &router,
        "DeleteStackInstances",
        &[
            member("StackSetName", stack_set_name),
            member("RetainStacks", "false"),
            member("Accounts.member.1", ACCOUNT_A),
            member("Regions.member.1", REGION),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteStackInstances to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    // "You can only update stack instances in Amazon Web Services Regions and accounts where
    // they already exist": the instance is gone, so the same update is now refused.
    let (status, body) = call(
        &router,
        "UpdateStackInstances",
        &update_members(
            stack_set_name,
            &[ACCOUNT_A],
            false,
            &[REGION],
            Some(&[("DisplayName", Some("after-the-deletion"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 404,
        "case {:?}: expected the update to be refused, got {status} {body}",
        "an update of an instance a deletion took away"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("StackInstanceNotFoundException"),
        "case {:?}: expected the modelled refusal, got {body}",
        "an update of an instance a deletion took away"
    );
    // A modelled CLIENT error: the awsQuery envelope reports the fault as the caller's.
    assert_eq!(
        xml_text(&body, "Type").as_deref(),
        Some("Sender"),
        "case {:?}: expected the refusal to be reported as the caller's fault, got {body}",
        "an update of an instance a deletion took away"
    );
    // The update path words the exception its own way: the Region of the selection that
    // resolved no instance, taken from the request's `Regions` list.
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(format!("No Stack Instance found in region {REGION}").as_str()),
        "case {:?}: expected the update path's not-found sentence naming the selected Region, \
         got {body}",
        "an update of an instance a deletion took away"
    );

    // The instance the deletion left alone still takes an update, so the refusal above is
    // about the pair rather than about the StackSet.
    let (status, body) = call(
        &router,
        "UpdateStackInstances",
        &update_members(
            stack_set_name,
            &[ACCOUNT_B],
            false,
            &[REGION],
            Some(&[("DisplayName", Some("still-there"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the surviving instance to take an update, got {status} {body}",
        "an update of an instance a deletion took away"
    );
    wait_for_the_transition().await;

    // Deploying the pair again makes it an instance of the StackSet once more, so the very
    // update that was refused is served: what the refusal is about is whether the StackSet
    // holds an instance in the pair, not what the pair once was.
    let (status, body) =
        create_stack_instances(&router, stack_set_name, &[ACCOUNT_A], &[REGION]).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the pair to be deployed again, got {status} {body}",
        "an update of an instance a deletion took away"
    );
    wait_for_the_transition().await;

    let (status, body) = call(
        &router,
        "UpdateStackInstances",
        &update_members(
            stack_set_name,
            &[ACCOUNT_A],
            false,
            &[REGION],
            Some(&[("DisplayName", Some("deployed-again"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the redeployed instance to take an update, got {status} {body}",
        "an update of an instance a deletion took away"
    );
    wait_for_the_transition().await;

    let (status, body) =
        describe_stack_instance(&router, stack_set_name, ACCOUNT_A, REGION).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the redeployed instance to be described, got {status} {body}",
        "an update of an instance a deletion took away"
    );
    assert_eq!(
        parameter_pairs(&body, "ParameterOverrides"),
        expected_pairs(&[("DisplayName", "deployed-again")]),
        "case {:?}: expected the redeployed instance to carry the update's overrides, got \
         {body}",
        "an update of an instance a deletion took away"
    );
}
