// SPDX-License-Identifier: CC0-1.0

//! `DeleteStackInstances` over the awsQuery wire: taking stack instances out of a StackSet.
//!
//! The rule every case here holds the service to: **`DeleteStackInstances` deletes the stack
//! instances of the StackSet the request names for every target account and Region pair the
//! request selects and the StackSet really holds, running as ONE `DELETE` StackSet operation
//! whose `OperationId` it answers — each selected instance's own stack is deleted unless
//! `RetainStacks` asks for the stack to be kept, the instances leave the StackSet when that
//! operation concludes, and a pair the StackSet holds no instance for is passed over rather
//! than refused.**
//!
//! The table below spans the class that rule generalises over rather than one recorded
//! request: one pair, one account across two target Regions, two accounts in one Region, the
//! accounts named through `Accounts` and through `DeploymentTargets`, a selection whose
//! product includes pairs the StackSet holds no instance for, and both `RetainStacks`
//! settings. The refusals are a table of their own: a `StackSetName` nothing resolves, each
//! way a request names its targets wrongly, an `OperationId` the StackSet already holds, and
//! a deletion asked for while another operation is in flight.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, operation_action, operation_status, MAX_INSTANCES_PER_STACK_SET,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{status, STATUS_TRANSITION_DELAY};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router, wait_for_the_transition, xml_text, ACCOUNT_ID, REGION,
};

/// The template every StackSet here is created from: one topic, really created in Amazon SNS
/// by each instance's stack and really deleted when that stack is deleted.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"instance-display"}}}}"#;

/// Two target accounts beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";

/// Two target Regions beside [`REGION`]: a stack instance's Region is a target the deploying
/// request chooses, so the instance's stack lives in it and is read through it.
const WEST: &str = "us-west-2";
const IRELAND: &str = "eu-west-1";

/// One account and Region pair: how a stack instance is addressed within its StackSet.
type Pair = (&'static str, &'static str);

/// One row of the deletion table.
struct Case {
    /// What the row is, named in every assertion it makes.
    case: &'static str,
    /// The StackSet the row deploys and then deletes from, unique per row.
    stack_set_name: &'static str,
    /// The deployments that put the row's instances there, as the `(Accounts, Regions)` of
    /// one `CreateStackInstances` call each: a row whose pairs are not one product takes
    /// more than one call.
    deployments: Vec<(Vec<&'static str>, Vec<&'static str>)>,
    /// The accounts the deletion names.
    accounts: Vec<&'static str>,
    /// Whether it names them through `DeploymentTargets.Accounts` rather than `Accounts` —
    /// "you can specify Accounts or DeploymentTargets, but not both".
    through_deployment_targets: bool,
    /// The Regions the deletion names.
    regions: Vec<&'static str>,
    /// `RetainStacks`: "removes the stack instances from the specified StackSet, but doesn't
    /// delete the stacks".
    retain_stacks: bool,
    /// The pairs that must have left the StackSet once the operation concludes.
    gone: Vec<Pair>,
    /// The pairs that must still be there, untouched by the deletion.
    kept: Vec<Pair>,
}

/// The pairs a row's deployments put in its StackSet: the union of their products, in the
/// order they were deployed.
fn deployed_pairs(case: &Case) -> Vec<Pair> {
    let mut pairs: Vec<Pair> = Vec::new();
    for (accounts, regions) in &case.deployments {
        for account in accounts {
            for region in regions {
                if !pairs.contains(&(account, region)) {
                    pairs.push((account, region));
                }
            }
        }
    }
    pairs
}

/// The table: each row selects its instances differently, and names exactly which of the
/// StackSet's instances the deletion must take and which it must leave.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one account and one Region, deleting the instance's stack",
            stack_set_name: "delete-one-pair",
            deployments: vec![(vec![ACCOUNT_A], vec![REGION])],
            accounts: vec![ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION],
            retain_stacks: false,
            gone: vec![(ACCOUNT_A, REGION)],
            kept: vec![],
        },
        Case {
            case: "one account across two target Regions",
            stack_set_name: "delete-two-regions",
            deployments: vec![(vec![ACCOUNT_A], vec![REGION, WEST])],
            accounts: vec![ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION, WEST],
            retain_stacks: false,
            gone: vec![(ACCOUNT_A, REGION), (ACCOUNT_A, WEST)],
            kept: vec![],
        },
        Case {
            case: "two accounts in one Region, retaining the stacks",
            stack_set_name: "delete-retaining",
            deployments: vec![(vec![ACCOUNT_A, ACCOUNT_B], vec![IRELAND])],
            accounts: vec![ACCOUNT_A, ACCOUNT_B],
            through_deployment_targets: false,
            regions: vec![IRELAND],
            retain_stacks: true,
            gone: vec![(ACCOUNT_A, IRELAND), (ACCOUNT_B, IRELAND)],
            kept: vec![],
        },
        Case {
            case: "the accounts named through DeploymentTargets",
            stack_set_name: "delete-through-targets",
            deployments: vec![(vec![ACCOUNT_B], vec![REGION])],
            accounts: vec![ACCOUNT_B],
            through_deployment_targets: true,
            regions: vec![REGION],
            retain_stacks: false,
            gone: vec![(ACCOUNT_B, REGION)],
            kept: vec![],
        },
        Case {
            case: "a selection whose product includes pairs the StackSet holds no instance \
                   for",
            stack_set_name: "delete-partial-product",
            deployments: vec![
                (vec![ACCOUNT_A], vec![REGION]),
                (vec![ACCOUNT_B], vec![WEST]),
            ],
            accounts: vec![ACCOUNT_A, ACCOUNT_B],
            through_deployment_targets: false,
            regions: vec![REGION],
            retain_stacks: false,
            gone: vec![(ACCOUNT_A, REGION)],
            kept: vec![(ACCOUNT_B, WEST)],
        },
        Case {
            case: "a request repeating an account and a Region, which names one pair",
            stack_set_name: "delete-repeated-pair",
            deployments: vec![(vec![ACCOUNT_A], vec![REGION])],
            accounts: vec![ACCOUNT_A, ACCOUNT_A],
            through_deployment_targets: false,
            regions: vec![REGION, REGION],
            retain_stacks: false,
            gone: vec![(ACCOUNT_A, REGION)],
            kept: vec![],
        },
    ]
}

/// `CreateStackSet` with the shared template.
async fn create_stack_set(router: &MultiServiceRouter, stack_set_name: &str) -> (u16, String) {
    call(
        router,
        "CreateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await
}

/// `CreateStackInstances` for one product of accounts and Regions.
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

/// The `DeleteStackInstances` request members a row states.
fn deletion_members(case: &Case, operation_id: Option<&str>) -> Vec<(String, String)> {
    let mut members = vec![
        member("StackSetName", case.stack_set_name),
        member(
            "RetainStacks",
            if case.retain_stacks { "true" } else { "false" },
        ),
    ];
    for (index, account) in case.accounts.iter().enumerate() {
        let key = if case.through_deployment_targets {
            format!("DeploymentTargets.Accounts.member.{}", index + 1)
        } else {
            format!("Accounts.member.{}", index + 1)
        };
        members.push(member(&key, account));
    }
    for (index, region) in case.regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    if let Some(operation_id) = operation_id {
        members.push(member("OperationId", operation_id));
    }
    members
}

/// `DescribeStackInstance` for one pair: the status and the body, so a case can read a
/// member off it or assert the refusal for an instance that is gone.
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

/// The `StackStatus` a stack reports, read in the Region the stack lives in — a stack is
/// addressable only through its own Region.
async fn stack_status(router: &MultiServiceRouter, region: &str, stack_id: &str) -> String {
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
    xml_text(&body, "StackStatus")
        .unwrap_or_else(|| panic!("DescribeStacks answered no StackStatus for {stack_id}: {body}"))
}

#[tokio::test]
async fn deleting_stack_instances_takes_them_out_of_the_stack_set() {
    let router = router();
    let cases = cases();

    for case in &cases {
        let (status, body) = create_stack_set(&router, case.stack_set_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStackSet to succeed, got {status} {body}",
            case.case
        );
    }
    // Every row's StackSet is deployed to the pairs it names, through the real wire, so the
    // instances the deletion acts on are instances a deployment really created. One
    // deployment call per row at a time — a StackSet performs one operation at a time — and
    // the rows' StackSets are each their own, so one wait covers the whole round.
    let rounds = cases
        .iter()
        .map(|case| case.deployments.len())
        .max()
        .expect("the table has rows");
    for round in 0..rounds {
        for case in &cases {
            let Some((accounts, regions)) = case.deployments.get(round) else {
                continue;
            };
            let (status, body) =
                create_stack_instances(&router, case.stack_set_name, accounts, regions).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateStackInstances for {accounts:?}/{regions:?} to \
                 succeed, got {status} {body}",
                case.case
            );
        }
        wait_for_the_transition().await;
    }

    // What each instance deployed, so the deletion can be held to what became of it.
    let mut deployed_stacks: Vec<(&'static str, Pair, String)> = Vec::new();
    for case in &cases {
        for (account, region) in deployed_pairs(case) {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the deployed instance {account}/{region} to be \
                 described, got {status} {body}",
                case.case
            );
            let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
                panic!(
                    "case {:?}: DescribeStackInstance answered no StackId: {body}",
                    case.case
                )
            });
            deployed_stacks.push((case.stack_set_name, (account, region), stack_id));
        }
    }

    // The deletion itself, one row at a time: each row's StackSet is its own, so the
    // operations the rows start all run at once and one wait covers them.
    let mut deletions: Vec<(&Case, String)> = Vec::new();
    for case in &cases {
        let members = deletion_members(case, None);
        let started = Instant::now();
        let (status, body) = call(&router, "DeleteStackInstances", &members).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeleteStackInstances to succeed, got {status} {body}",
            case.case
        );
        let operation_id = xml_text(&body, "OperationId").unwrap_or_else(|| {
            panic!(
                "case {:?}: DeleteStackInstances answered no OperationId: {body}",
                case.case
            )
        });

        // The operation the call started is a record of its own, reported with the action
        // that started it and the `RetainStacks` it was asked for.
        let (status, body) = describe_operation(&router, case.stack_set_name, &operation_id).await;
        // Whether the read above certainly happened before the conclusion became due: the
        // emulator owes the in-flight answer only until that instant, so a read that lost
        // the race to the clock is not held to the transient state.
        let in_flight = started.elapsed() < STATUS_TRANSITION_DELAY;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::DELETE),
            "case {:?}: expected the deletion's own action, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "RetainStacks").as_deref(),
            Some(if case.retain_stacks { "true" } else { "false" }),
            "case {:?}: expected the RetainStacks the deletion was asked for, got {body}",
            case.case
        );
        if in_flight {
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(operation_status::RUNNING),
                "case {:?}: expected the deletion to still be in flight, got {body}",
                case.case
            );
        }
        // While the operation is in flight the instances it is deleting are still the
        // StackSet's, reporting the deletion as not yet started in their account and Region:
        // the operation's conclusion is what carries it out there.
        for (account, region) in &case.gone {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            if started.elapsed() >= STATUS_TRANSITION_DELAY {
                continue;
            }
            assert_eq!(
                status, 200,
                "case {:?}: expected {account}/{region} to still be an instance while the \
                 deletion runs, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "DetailedStatus").as_deref(),
                Some(detailed_status::PENDING),
                "case {:?}: expected the deletion to have yet to start in {account}/{region}, \
                 got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "LastOperationId").as_deref(),
                Some(operation_id.as_str()),
                "case {:?}: expected {account}/{region} to report the deletion as its last \
                 operation, got {body}",
                case.case
            );
        }
        deletions.push((case, operation_id));
    }

    wait_for_the_transition().await;

    for (case, operation_id) in &deletions {
        // The operation concluded, and the instances it deleted are no longer the
        // StackSet's.
        let (status, body) = describe_operation(&router, case.stack_set_name, operation_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::SUCCEEDED),
            "case {:?}: expected the deletion to have concluded, got {body}",
            case.case
        );
        for (account, region) in &case.gone {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            assert_eq!(
                status, 404,
                "case {:?}: expected the deleted instance {account}/{region} to be gone, got \
                 {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("StackInstanceNotFoundException"),
                "case {:?}: expected the modelled refusal for {account}/{region}, got {body}",
                case.case
            );
            // The sentence the operations that READ one stack instance answer for a pair this
            // StackSet holds no record of; the operations that UPDATE a selection word the
            // same exception their own way (`stack_set_refusal_wording.rs`).
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some("StackInstance not found"),
                "case {:?}: expected the stack-instance-not-found sentence for \
                 {account}/{region}, got {body}",
                case.case
            );
        }
        // An instance the selection never reached is untouched: still there, and still up to
        // date with its StackSet.
        for (account, region) in &case.kept {
            let (status, body) =
                describe_stack_instance(&router, case.stack_set_name, account, region).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected {account}/{region} to be left alone, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "DetailedStatus").as_deref(),
                Some(detailed_status::SUCCEEDED),
                "case {:?}: expected {account}/{region} to be untouched by the deletion, got \
                 {body}",
                case.case
            );
        }
        // Each instance's own stack: deleted with the instance, or left exactly as it was
        // when the deletion was asked to retain it.
        for (stack_set_name, (account, region), stack_id) in &deployed_stacks {
            if *stack_set_name != case.stack_set_name {
                continue;
            }
            let reported = stack_status(&router, region, stack_id).await;
            let selected = case.gone.iter().any(|pair| pair == &(*account, *region));
            let expected = if selected && !case.retain_stacks {
                status::DELETE_COMPLETE
            } else {
                status::CREATE_COMPLETE
            };
            assert_eq!(
                reported, expected,
                "case {:?}: expected the stack of {account}/{region} to be {expected}, got \
                 {reported:?}",
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
async fn a_deletion_a_stack_set_cannot_perform_is_refused() {
    let router = router();
    let stack_set_name = "refusals";
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

    // One well-formed deletion of the instance just deployed, reused by the rows that are
    // about something other than the request's own shape.
    let well_formed = vec![
        member("StackSetName", stack_set_name),
        member("RetainStacks", "false"),
        member("Accounts.member.1", ACCOUNT_ID),
        member("Regions.member.1", REGION),
    ];

    // "Only one operation can be performed for a stack set at a given time": the deployment
    // above is still in flight.
    let (status, body) = call(&router, "DeleteStackInstances", &well_formed).await;
    assert_eq!(
        status, 409,
        "case {:?}: expected the deletion to be refused, got {status} {body}",
        "a deletion while another operation is in flight"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("OperationInProgressException"),
        "case {:?}: expected the modelled refusal, got {body}",
        "a deletion while another operation is in flight"
    );

    wait_for_the_transition().await;

    // Each way a request names a StackSet or its targets that the service cannot act on.
    for refusal in [
        Refusal {
            case: "a StackSetName no StackSet of this Region carries",
            members: vec![
                member("StackSetName", "no-such-stack-set"),
                member("RetainStacks", "false"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("Regions.member.1", REGION),
            ],
            status: 404,
            code: Some("StackSetNotFoundException"),
        },
        Refusal {
            case: "both Accounts and DeploymentTargets",
            members: vec![
                member("StackSetName", stack_set_name),
                member("RetainStacks", "false"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("DeploymentTargets.Accounts.member.1", ACCOUNT_A),
                member("Regions.member.1", REGION),
            ],
            status: 400,
            code: None,
        },
        Refusal {
            case: "neither Accounts nor DeploymentTargets",
            members: vec![
                member("StackSetName", stack_set_name),
                member("RetainStacks", "false"),
                member("Regions.member.1", REGION),
            ],
            status: 400,
            code: None,
        },
        Refusal {
            case: "DeploymentTargets naming an AccountsUrl nothing reads",
            members: vec![
                member("StackSetName", stack_set_name),
                member("RetainStacks", "false"),
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
                member("RetainStacks", "false"),
                member("Accounts.member.1", ACCOUNT_ID),
            ],
            status: 400,
            code: None,
        },
    ] {
        let (status, body) = call(&router, "DeleteStackInstances", &refusal.members).await;
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

    // "The specified operation ID already exists": the id names the operation that already
    // ran, so the same id does not start a second one. The first deletion retains the stack,
    // so the instance's stack is still there to be read after it.
    let operation_id = "deletion-one";
    let mut named = well_formed.clone();
    named.push(member("OperationId", operation_id));
    let (status, body) = call(&router, "DeleteStackInstances", &named).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the first deletion to succeed, got {status} {body}",
        "an OperationId the StackSet already holds"
    );
    assert_eq!(
        xml_text(&body, "OperationId").as_deref(),
        Some(operation_id),
        "case {:?}: expected the operation id the request named, got {body}",
        "an OperationId the StackSet already holds"
    );

    wait_for_the_transition().await;

    let (status, body) = call(&router, "DeleteStackInstances", &named).await;
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

/// A request naming `accounts` accounts and `regions` Regions, each distinct, so its selection
/// is exactly their product: what the bound on that product is read against.
fn a_product_of(stack_set_name: &str, accounts: usize, regions: usize) -> Vec<(String, String)> {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("RetainStacks", "true"),
    ];
    for index in 0..accounts {
        // `Account` is `@pattern("^[0-9]{12}$")`, so each one is twelve digits.
        members.push(member(
            &format!("Accounts.member.{}", index + 1),
            &format!("{:012}", index + 1),
        ));
    }
    for index in 0..regions {
        // Each Region is an entry of the published catalog
        // (`emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the wire may
        // carry"), which is what a `RegionList` entry is held to: the selections below are
        // then judged on their SIZE rather than refused for naming a Region AWS does not
        // have.
        let region = ::emulator_telemetry::policy::KNOWN_REGIONS
            .get(index)
            .expect("the Region catalog holds at least as many Regions as a case asks for");
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    members
}

#[tokio::test]
async fn a_selection_wider_than_a_stack_set_can_hold_instances_for_is_refused() {
    let router = router();
    let stack_set_name = "bounded-selection";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );

    // A selection of exactly as many pairs as a StackSet holds instances is served — none of
    // them carries an instance, so the deletion deletes nothing and still answers its
    // operation id.
    let at_the_bound = a_product_of(stack_set_name, MAX_INSTANCES_PER_STACK_SET / 2, 2);
    let (status, body) = call(&router, "DeleteStackInstances", &at_the_bound).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected a selection of {MAX_INSTANCES_PER_STACK_SET} pairs to be \
         served, got {status} {body}",
        "a selection at the bound"
    );
    assert!(
        xml_text(&body, "OperationId").is_some(),
        "case {:?}: expected the deletion to answer an operation id, got {body}",
        "a selection at the bound"
    );

    wait_for_the_transition().await;

    // One pair past it is refused, before the store is read.
    for (case, accounts, regions) in [
        ("one pair past the bound", MAX_INSTANCES_PER_STACK_SET + 1, 1),
        (
            "a product far past the bound",
            MAX_INSTANCES_PER_STACK_SET,
            2,
        ),
    ] {
        let members = a_product_of(stack_set_name, accounts, regions);
        let (status, body) = call(&router, "DeleteStackInstances", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a selection of {accounts}x{regions} pairs to be \
             refused, got {status} {body}"
        );
    }
}
