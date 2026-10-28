// SPDX-License-Identifier: CC0-1.0

//! The StackSet control plane's MUTATIONS over the awsQuery wire: the single-flight lock,
//! how a request names its targets, which Regions it may target, and what an update writes.
//!
//! Four rules, each held over a TABLE rather than one recorded request:
//!
//! 1. **Single flight.** While an operation on a StackSet has not concluded, every mutation
//!    of that StackSet is refused `409 OperationInProgressException` whose `Message` names
//!    the StackSet that is locked and the operation holding it — `Another Operation on
//!    StackSet <StackSetARN> is in progress: <OperationId>` — and the identical call is
//!    served once that operation settles.
//! 2. **Targeting.** `Accounts` and `DeploymentTargets` are mutually exclusive targeting
//!    modes, and `DeploymentTargets.OrganizationalUnitIds` targets an organization, which
//!    only a service-managed StackSet has: each refusal is a `ValidationError` whose message
//!    is the one the real service words for that case.
//! 3. **Regions.** Every entry of a `Regions` list a deployment targets must be an Amazon
//!    Web Services Region; a request naming one that is not is refused WHOLESALE, so no
//!    instance is deployed to the Regions beside it either.
//! 4. **What an update writes.** `UpdateStackSet` records the revision the StackSet is
//!    updated to and rolls it out over the instances the request selects, and
//!    `UpdateStackInstances` records the parameter overrides in force on the instances it
//!    selects — both read back through `DescribeStackSet`, `DescribeStackInstance`,
//!    `DescribeStacks` and `GetTemplate`.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, instance_status, operation_action, operation_status, permission_model,
    selection_wider_than_a_stack_set, stack_set_status, MAX_INSTANCES_PER_STACK_SET,
    MAX_SUPPLIED_PARAMETERS,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, get_template, member, router, wait_for_the_transition, xml_blocks, xml_text,
    xml_texts, ACCOUNT_ID, REGION,
};

/// The template a StackSet is created from here: one topic, really created in Amazon SNS by
/// each instance's stack, whose `DisplayName` is the parameter in force on that instance.
const TEMPLATE: &str = r#"{"Parameters":{"Shown":{"Type":"String","Default":"first-display"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"Shown"}}}}}"#;

/// The revision an update puts in force: the same resource with a second parameter, so a
/// read can tell which revision an instance's stack is deployed from.
const REVISED_TEMPLATE: &str = r#"{"Parameters":{"Shown":{"Type":"String","Default":"second-display"},
     "Note":{"Type":"String","Default":"revised"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"Shown"}}}}}"#;

/// A second target account beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";

/// An organizational unit id, as `DeploymentTargets.OrganizationalUnitIds` names one.
const OU_ID: &str = "ou-1a2b-3c4d5e6f";

/// The refusal for a request naming BOTH targeting modes, spelled out here rather than read
/// from the crate: the sentence is the requirement, so a reworded one must fail this suite.
const BOTH_MODES: &str = "Cannot specify both Accounts and DeploymentTargets";

/// The refusal for a request naming NEITHER targeting mode, spelled out for the same reason.
const NO_MODE: &str = "You must specify at least one value for either Accounts or DeploymentTargets";

/// The refusal for a Region the partition does not have, spelled out for the same reason:
/// the Region is the request's, the rest of the sentence is fixed.
fn unknown_region_message(region: &str) -> String {
    format!("Region {region} is not supported")
}

/// `CreateStackSet` with a template and whatever else a case states.
async fn create_stack_set(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    template: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("TemplateBody", template),
    ];
    members.extend_from_slice(extra);
    call(router, "CreateStackSet", &members).await
}

/// The `Accounts.member.N` / `Regions.member.N` members one selection states.
fn selection(accounts: &[&str], regions: &[&str]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        members.push(member(&format!("Accounts.member.{}", index + 1), account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    members
}

/// `CreateStackInstances` for one product of accounts and Regions.
async fn deploy(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    members.extend(selection(accounts, regions));
    call(router, "CreateStackInstances", &members).await
}

/// `DescribeStackSet`, answered with the HTTP status and the XML body.
async fn describe_stack_set(router: &MultiServiceRouter, stack_set_name: &str) -> (u16, String) {
    call(
        router,
        "DescribeStackSet",
        &[member("StackSetName", stack_set_name)],
    )
    .await
}

/// `DescribeStackInstance` for one account and Region pair.
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

/// The `StackSetARN` a StackSet reports, which the single-flight refusal names.
async fn stack_set_arn(router: &MultiServiceRouter, stack_set_name: &str) -> String {
    let (http_status, body) = describe_stack_set(router, stack_set_name).await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStackSet for {stack_set_name} to succeed, got {http_status} {body}"
    );
    xml_text(&body, "StackSetARN")
        .unwrap_or_else(|| panic!("DescribeStackSet answered no StackSetARN: {body}"))
}

/// The `OperationId` a call that starts a StackSet operation answered with.
fn operation_id_of(case: &str, http_status: u16, body: &str) -> String {
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the call to succeed, got {http_status} {body}"
    );
    xml_text(body, "OperationId")
        .unwrap_or_else(|| panic!("case {case:?}: no OperationId in {body}"))
}

/// The `<Message>` of a refusal, and the `<Code>` it was refused under.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// One mutation of a StackSet: what the row is, the action to call, and the request members
/// it sends.
struct Mutation {
    case: &'static str,
    action: &'static str,
    members: Vec<(String, String)>,
}

/// One row of a refusal table: the call, and the status, code and message it must answer.
struct Refusal {
    case: &'static str,
    action: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_mutation_while_an_operation_is_in_flight_names_the_stack_set_and_the_operation() {
    let router = router();
    let stack_set_name = "single-flight";
    let (http_status, body) = create_stack_set(&router, stack_set_name, TEMPLATE, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );
    let arn = stack_set_arn(&router, stack_set_name).await;
    let (http_status, body) = deploy(&router, stack_set_name, &[ACCOUNT_ID], &[REGION]).await;
    let blocking = operation_id_of("the deployment that holds the lock", http_status, &body);

    // The fixed words are "Another Operation on StackSet" and "is in progress:"; the ARN is
    // the StackSet's own and the id is the operation still in flight.
    let expected = format!("Another Operation on StackSet {arn} is in progress: {blocking}");
    let mut mutations: Vec<Mutation> = Vec::new();
    mutations.push(Mutation {
        case: "CreateStackInstances for a second pair",
        action: "CreateStackInstances",
        members: {
            let mut members = vec![member("StackSetName", stack_set_name)];
            members.extend(selection(&[ACCOUNT_A], &[REGION]));
            members
        },
    });
    mutations.push(Mutation {
        case: "UpdateStackInstances",
        action: "UpdateStackInstances",
        members: {
            let mut members = vec![member("StackSetName", stack_set_name)];
            members.extend(selection(&[ACCOUNT_ID], &[REGION]));
            members
        },
    });
    mutations.push(Mutation {
        case: "UpdateStackSet with the template already in force",
        action: "UpdateStackSet",
        members: vec![
            member("StackSetName", stack_set_name),
            member("UsePreviousTemplate", "true"),
        ],
    });
    mutations.push(Mutation {
        case: "DeleteStackInstances",
        action: "DeleteStackInstances",
        members: {
            let mut members = vec![
                member("StackSetName", stack_set_name),
                member("RetainStacks", "true"),
            ];
            members.extend(selection(&[ACCOUNT_ID], &[REGION]));
            members
        },
    });
    mutations.push(Mutation {
        case: "DetectStackSetDrift",
        action: "DetectStackSetDrift",
        members: vec![member("StackSetName", stack_set_name)],
    });

    for mutation in &mutations {
        let (case, action) = (mutation.case, mutation.action);
        let (http_status, body) = call(&router, action, &mutation.members).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected the mutation to be refused with 409, got {http_status} \
             {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("OperationInProgressException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // "The rejection arrives synchronously, and the identical call is accepted once the
    // prior operation settles": each mutation that was refused above is served afterwards,
    // one at a time, since each of them starts an operation of its own.
    for mutation in &mutations {
        let (case, action) = (mutation.case, mutation.action);
        wait_for_the_transition().await;
        let (http_status, body) = call(&router, action, &mutation.members).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the same call to be served once the lock was free, got \
             {http_status} {body}"
        );
        assert!(
            xml_text(&body, "OperationId").is_some(),
            "case {case:?}: expected the served call to answer an OperationId, got {body}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_its_targets_wrongly_is_refused_in_the_services_own_words() {
    let router = router();
    let self_managed = "targeting-self-managed";
    let service_managed = "targeting-service-managed";
    for (stack_set_name, model) in [
        (self_managed, permission_model::SELF_MANAGED),
        (service_managed, permission_model::SERVICE_MANAGED),
    ] {
        let (http_status, body) = create_stack_set(
            &router,
            stack_set_name,
            TEMPLATE,
            &[member("PermissionModel", model)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "expected CreateStackSet({model}) to succeed, got {http_status} {body}"
        );
    }

    // The targeting members, by the way a row names them.
    let both = |stack_set_name: &str| -> Vec<(String, String)> {
        vec![
            member("StackSetName", stack_set_name),
            member("Accounts.member.1", ACCOUNT_ID),
            member("DeploymentTargets.Accounts.member.1", ACCOUNT_A),
            member("Regions.member.1", REGION),
        ]
    };
    let organizational_units = |stack_set_name: &str| -> Vec<(String, String)> {
        vec![
            member("StackSetName", stack_set_name),
            member("DeploymentTargets.OrganizationalUnitIds.member.1", OU_ID),
            member("Regions.member.1", REGION),
        ]
    };
    let neither = |stack_set_name: &str| -> Vec<(String, String)> {
        vec![
            member("StackSetName", stack_set_name),
            member("Regions.member.1", REGION),
        ]
    };

    let mut refusals: Vec<Refusal> = Vec::new();
    // Both targeting modes: a request-shape problem, refused the same way by every operation
    // that takes the two members, whatever the StackSet's permission model is.
    for (action, retain) in [
        ("CreateStackInstances", false),
        ("UpdateStackInstances", false),
        ("DeleteStackInstances", true),
        ("UpdateStackSet", false),
    ] {
        for stack_set_name in [self_managed, service_managed] {
            let mut members = both(stack_set_name);
            if retain {
                members.push(member("RetainStacks", "true"));
            }
            if action == "UpdateStackSet" {
                members.push(member("UsePreviousTemplate", "true"));
            }
            refusals.push(Refusal {
                case: "both Accounts and DeploymentTargets",
                action,
                members,
                status: 400,
                code: "ValidationError",
                message: BOTH_MODES.to_string(),
            });
        }
    }
    // Organizational units against a self-managed StackSet: the permission model the STACK
    // SET carries is what makes OU targeting impossible, so the refusal names that model.
    let self_managed_message = format!(
        "StackSets with {} permission model can only have accounts as target",
        permission_model::SELF_MANAGED
    );
    for (action, retain) in [
        ("CreateStackInstances", false),
        ("UpdateStackInstances", false),
        ("DeleteStackInstances", true),
        ("UpdateStackSet", false),
    ] {
        let mut members = organizational_units(self_managed);
        if retain {
            members.push(member("RetainStacks", "true"));
        }
        if action == "UpdateStackSet" {
            members.push(member("UsePreviousTemplate", "true"));
        }
        refusals.push(Refusal {
            case: "DeploymentTargets naming OUs against a SELF_MANAGED StackSet",
            action,
            members,
            status: 400,
            code: "ValidationError",
            message: self_managed_message.clone(),
        });
    }
    // Neither targeting mode: nothing is selected at all.
    for (action, retain) in [
        ("CreateStackInstances", false),
        ("UpdateStackInstances", false),
        ("DeleteStackInstances", true),
    ] {
        let mut members = neither(self_managed);
        if retain {
            members.push(member("RetainStacks", "true"));
        }
        refusals.push(Refusal {
            case: "neither Accounts nor DeploymentTargets",
            action,
            members,
            status: 400,
            code: "ValidationError",
            message: NO_MODE.to_string(),
        });
    }

    for refusal_case in &refusals {
        let (http_status, body) = call(&router, refusal_case.action, &refusal_case.members).await;
        assert_eq!(
            http_status, refusal_case.status,
            "case {:?} on {}: expected {}, got {http_status} {body}",
            refusal_case.case, refusal_case.action, refusal_case.status
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some(refusal_case.code),
            "case {:?} on {}: expected <Code>{}</Code>, got {body}",
            refusal_case.case,
            refusal_case.action,
            refusal_case.code
        );
        assert_eq!(
            message.as_deref(),
            Some(refusal_case.message.as_str()),
            "case {:?} on {}: expected {:?}, got {message:?}",
            refusal_case.case,
            refusal_case.action,
            refusal_case.message
        );
    }

    // The same OU targeting against a SERVICE_MANAGED StackSet is NOT a permission-model
    // problem: the rule keys on the model the StackSet carries, not on the member the
    // request used. Real CloudFormation serves that request by expanding the OU into the
    // accounts the organization holds under it, which this emulator holds no organization
    // membership for — so it answers as work nobody has written (501) rather than telling a
    // well-formed request that it is malformed.
    let (http_status, body) = call(
        &router,
        "CreateStackInstances",
        &organizational_units(service_managed),
    )
    .await;
    let case = "DeploymentTargets naming OUs against a SERVICE_MANAGED StackSet";
    assert_eq!(
        http_status, 501,
        "case {case:?}: expected the unimplemented channel, got {http_status} {body}"
    );
    let (code, message) = refusal(&body);
    assert_eq!(
        code.as_deref(),
        Some("NotImplemented"),
        "case {case:?}: expected <Code>NotImplemented</Code>, got {body}"
    );
    assert_ne!(
        message.as_deref(),
        Some(self_managed_message.as_str()),
        "case {case:?}: expected no SELF_MANAGED permission-model refusal, got {body}"
    );
    assert_ne!(
        message.as_deref(),
        Some(NO_MODE),
        "case {case:?}: expected no \"named no targeting mode\" refusal — it named one — got \
         {body}"
    );

    // The same holds when the request names accounts BESIDE the OUs: `AccountFilterType` is
    // how the two combine, so the selection is still the organization's to resolve and the
    // accounts the request listed are not quietly taken as the whole of it.
    let mut both = organizational_units(service_managed);
    both.push(member("DeploymentTargets.Accounts.member.1", ACCOUNT_ID));
    both.push(member("DeploymentTargets.AccountFilterType", "INTERSECTION"));
    let (http_status, body) = call(&router, "CreateStackInstances", &both).await;
    let case = "DeploymentTargets naming OUs and accounts against a SERVICE_MANAGED StackSet";
    assert_eq!(
        http_status, 501,
        "case {case:?}: expected the unimplemented channel, got {http_status} {body}"
    );
    assert_eq!(
        refusal(&body).0.as_deref(),
        Some("NotImplemented"),
        "case {case:?}: expected <Code>NotImplemented</Code>, got {body}"
    );

    // A pair the StackSet holds no instance for cannot be UPDATED: "you can only update
    // stack instances in Amazon Web Services Regions and accounts where they already exist".
    let mut members = vec![member("StackSetName", self_managed)];
    members.extend(selection(&[ACCOUNT_ID], &[REGION]));
    let (http_status, body) = call(&router, "UpdateStackInstances", &members).await;
    assert_eq!(
        http_status, 404,
        "case {:?}: expected 404, got {http_status} {body}",
        "UpdateStackInstances for a pair with no instance"
    );
    assert_eq!(
        refusal(&body).0.as_deref(),
        Some("StackInstanceNotFoundException"),
        "case {:?}: expected the modelled refusal, got {body}",
        "UpdateStackInstances for a pair with no instance"
    );
}

/// Region codes a deployment may target, and ones it may not.
///
/// SOURCE: `emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the wire may
/// carry" — the one catalog of Region codes this emulator holds, which is what a
/// `RegionList` entry is checked against. The accepted rows are entries of that catalog the
/// issue this suite covers did not cite; the refused rows are labels the model's own
/// `Region` `@pattern("^[a-zA-Z0-9-]{1,128}$")` admits and the catalog does not hold.
const TARGETABLE: &[&str] = &["eu-central-1", "ap-southeast-3", "us-gov-west-1"];

#[tokio::test]
async fn a_selection_wider_than_a_stack_set_can_hold_instances_for_is_refused() {
    let router = router();
    let stack_set_name = "bounded-selection";
    let (http_status, body) = create_stack_set(&router, stack_set_name, TEMPLATE, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );

    /// A selection of `accounts` distinct accounts across `regions` distinct Regions of the
    /// catalog, so its size is exactly their product.
    fn a_product_of(accounts: usize, regions: usize) -> Vec<(String, String)> {
        let mut members = Vec::new();
        for index in 1..=accounts {
            // `Account` is `@pattern("^[0-9]{12}$")`, so each one is twelve digits.
            members.push(member(
                &format!("Accounts.member.{index}"),
                &format!("{index:012}"),
            ));
        }
        for index in 1..=regions {
            // Each Region is an entry of the catalog, so the selection is judged on its
            // SIZE rather than refused for naming a Region AWS does not have.
            let region = ::emulator_telemetry::policy::KNOWN_REGIONS
                .get(index - 1)
                .expect("the Region catalog holds at least as many Regions as asked for");
            members.push(member(&format!("Regions.member.{index}"), region));
        }
        members
    }

    let bound = MAX_INSTANCES_PER_STACK_SET;
    let too_wide = selection_wider_than_a_stack_set();
    for action in [
        "CreateStackInstances",
        "UpdateStackInstances",
        "DeleteStackInstances",
        "UpdateStackSet",
    ] {
        // AT the bound the selection is not refused for its size: the StackSet holds no
        // instance in any of those pairs, so each operation answers for THAT instead. The
        // at-bound row is read against the operations that cannot DEPLOY, so the case does
        // not drive a rollout of a whole bound's worth of stacks.
        for (case, accounts, regions) in [
            ("a selection at the bound", bound / 2, 2),
            ("a selection one pair past the bound", bound + 1, 1),
            ("a selection far past the bound", bound, 2),
        ] {
            if case == "a selection at the bound" && action == "CreateStackInstances" {
                continue;
            }
            let mut members = vec![member("StackSetName", stack_set_name)];
            if action == "DeleteStackInstances" {
                members.push(member("RetainStacks", "true"));
            }
            if action == "UpdateStackSet" {
                members.push(member("UsePreviousTemplate", "true"));
            }
            members.extend(a_product_of(accounts, regions));
            let (http_status, body) = call(&router, action, &members).await;
            let (code, message) = refusal(&body);
            let past_the_bound = accounts * regions > bound;
            // `CreateStackInstances` models `LimitExceededException` for a deployment past a
            // quota, so that is the channel its own size refusal takes; the other three
            // model no such error, so theirs is a bad request.
            let expected_refusal = if action == "CreateStackInstances" {
                (
                    "LimitExceededException",
                    format!(
                        "Limit for stack instances exceeded: no more than \
                         {MAX_INSTANCES_PER_STACK_SET} stack instances can exist in one \
                         StackSet"
                    ),
                )
            } else {
                ("ValidationError", too_wide.clone())
            };
            let refused_for_its_size = code.as_deref() == Some(expected_refusal.0)
                && message.as_deref() == Some(expected_refusal.1.as_str());
            assert_eq!(
                refused_for_its_size, past_the_bound,
                "case {case:?} on {action}: expected the size refusal {past_the_bound:?} for \
                 {accounts}x{regions} pairs, got {http_status} {body}"
            );
            if past_the_bound {
                assert_eq!(
                    http_status, 400,
                    "case {case:?} on {action}: expected 400, got {http_status} {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_parameter_list_longer_than_a_template_can_declare_is_refused() {
    let router = router();
    let stack_set_name = "bounded-parameters";
    let (http_status, body) = create_stack_set(&router, stack_set_name, TEMPLATE, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );

    /// `count` supplied parameters under `member`, each with its own key.
    fn parameters(member_name: &str, count: usize) -> Vec<(String, String)> {
        (1..=count)
            .flat_map(|index| {
                [
                    member(
                        &format!("{member_name}.member.{index}.ParameterKey"),
                        &format!("Supplied{index}"),
                    ),
                    member(
                        &format!("{member_name}.member.{index}.ParameterValue"),
                        &format!("value-{index}"),
                    ),
                ]
            })
            .collect()
    }

    // AT the documented bound: served, and the deployment it drives really happens.
    let mut at_the_bound = vec![member("StackSetName", stack_set_name)];
    at_the_bound.extend(selection(&[ACCOUNT_ID], &[REGION]));
    at_the_bound.extend(parameters("ParameterOverrides", MAX_SUPPLIED_PARAMETERS));
    let (http_status, body) = call(&router, "CreateStackInstances", &at_the_bound).await;
    operation_id_of("a parameter list at the bound", http_status, &body);
    wait_for_the_transition().await;

    // ONE past it: refused, on every operation that takes a caller's parameter list, naming
    // the member that carried it and the bound it passed.
    let past_the_bound = MAX_SUPPLIED_PARAMETERS + 1;
    for (action, member_name) in [
        ("CreateStackInstances", "ParameterOverrides"),
        ("UpdateStackInstances", "ParameterOverrides"),
        ("UpdateStackSet", "Parameters"),
    ] {
        let mut members = vec![member("StackSetName", stack_set_name)];
        members.extend(selection(&[ACCOUNT_ID], &[REGION]));
        if action == "UpdateStackSet" {
            members.push(member("UsePreviousTemplate", "true"));
        }
        members.extend(parameters(member_name, past_the_bound));
        let (http_status, body) = call(&router, action, &members).await;
        assert_eq!(
            http_status, 400,
            "case {action:?}: expected a parameter list past the bound to be refused, got \
             {http_status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("ValidationError"),
            "case {action:?}: expected <Code>ValidationError</Code>, got {body}"
        );
        let expected = format!(
            "{member_name} count {past_the_bound} is greater than max allowed \
             {MAX_SUPPLIED_PARAMETERS}"
        );
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {action:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_deployment_targeting_a_region_that_does_not_exist_is_refused_wholesale() {
    let router = router();
    let stack_set_name = "region-catalog";
    let (http_status, body) = create_stack_set(&router, stack_set_name, TEMPLATE, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );

    for (case, regions, unknown) in [
        (
            "a Region that does not exist beside one that does",
            vec![REGION, "not-a-region"],
            "not-a-region",
        ),
        ("only a Region that does not exist", vec!["not-a-region"], "not-a-region"),
        (
            "a plausible but unpublished Region code",
            vec!["us-west-3"],
            "us-west-3",
        ),
        (
            "a published Region code in upper case",
            vec!["US-WEST-2"],
            "US-WEST-2",
        ),
        ("a one-letter label", vec!["x"], "x"),
    ] {
        // Every operation that takes a `RegionList` holds its entries to the catalog, so the
        // row is run against each of them.
        for action in [
            "CreateStackInstances",
            "UpdateStackInstances",
            "DeleteStackInstances",
            "UpdateStackSet",
        ] {
            let mut members = vec![member("StackSetName", stack_set_name)];
            members.extend(selection(&[ACCOUNT_ID], &regions));
            if action == "DeleteStackInstances" {
                members.push(member("RetainStacks", "true"));
            }
            if action == "UpdateStackSet" {
                members.push(member("UsePreviousTemplate", "true"));
            }
            let (http_status, body) = call(&router, action, &members).await;
            assert_eq!(
                http_status, 400,
                "case {case:?} on {action}: expected the request to be refused with 400, got \
                 {http_status} {body}"
            );
            let (code, message) = refusal(&body);
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {case:?} on {action}: expected <Code>ValidationError</Code>, got {body}"
            );
            let expected = unknown_region_message(unknown);
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {case:?} on {action}: expected {expected:?}, got {message:?}"
            );
        }
        // Refused WHOLESALE: the Region of the row that named a real one beside the unknown
        // carries no instance either.
        let (http_status, body) =
            describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, REGION).await;
        assert_ne!(
            http_status, 200,
            "case {case:?}: expected no instance to have been deployed, got {http_status} \
             {body}"
        );
    }

    // Every Region of the catalog IS targetable: one deployment across three of them, each
    // an entry the issue did not cite, deploys an instance in each.
    let mut members = vec![member("StackSetName", stack_set_name)];
    members.extend(selection(&[ACCOUNT_ID], TARGETABLE));
    let (http_status, body) = call(&router, "CreateStackInstances", &members).await;
    let operation_id = operation_id_of("the deployment across the catalog", http_status, &body);
    wait_for_the_transition().await;
    for region in TARGETABLE {
        let (http_status, body) =
            describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, region).await;
        assert_eq!(
            http_status, 200,
            "case {region:?}: expected the instance to be described, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(instance_status::CURRENT),
            "case {region:?}: expected the instance to be up to date, got {body}"
        );
        assert_eq!(
            xml_text(&body, "LastOperationId").as_deref(),
            Some(operation_id.as_str()),
            "case {region:?}: expected the deployment's own operation id, got {body}"
        );
    }
}

/// The `ParameterValue` a read reports for `key`, out of the first `block` list the response
/// carries (`Parameters` on a StackSet, `ParameterOverrides` on an instance).
fn parameter_value(body: &str, block: &str, key: &str) -> Option<String> {
    let list = xml_blocks(body, block).into_iter().next()?;
    let keys = xml_texts(&list, "ParameterKey");
    let values = xml_texts(&list, "ParameterValue");
    keys.iter()
        .position(|held| held == key)
        .and_then(|index| values.get(index).cloned())
}

#[tokio::test]
async fn updating_a_stack_set_rewrites_it_and_redeploys_the_instances_it_selects() {
    let router = router();
    let stack_set_name = "rolled-out";
    let (http_status, body) = create_stack_set(
        &router,
        stack_set_name,
        TEMPLATE,
        &[
            member("Description", "the first revision"),
            member("Tags.member.1.Key", "tier"),
            member("Tags.member.1.Value", "first"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );
    let (http_status, body) =
        deploy(&router, stack_set_name, &[ACCOUNT_ID, ACCOUNT_A], &[REGION]).await;
    operation_id_of("the deployment under the first revision", http_status, &body);
    wait_for_the_transition().await;

    // The update: a new template, a new parameter value, a new description and a new tag
    // set, rolled out over every instance since the request selects none.
    let (http_status, body) = call(
        &router,
        "UpdateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("TemplateBody", REVISED_TEMPLATE),
            member("Parameters.member.1.ParameterKey", "Shown"),
            member("Parameters.member.1.ParameterValue", "revised-display"),
            member("Description", "the second revision"),
            member("Tags.member.1.Key", "tier"),
            member("Tags.member.1.Value", "second"),
        ],
    )
    .await;
    let update = operation_id_of("the StackSet update", http_status, &body);

    // The operation is answered while it is still running, and it is an UPDATE.
    let (http_status, body) = call(
        &router,
        "DescribeStackSetOperation",
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", &update),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStackSetOperation to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Action").as_deref(),
        Some(operation_action::UPDATE),
        "expected the operation to be an UPDATE, got {body}"
    );
    wait_for_the_transition().await;

    // What the StackSet reports once the update is in force: the revision it was updated to.
    let (http_status, body) = describe_stack_set(&router, stack_set_name).await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStackSet to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(REVISED_TEMPLATE),
        "expected the revised template, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Description").as_deref(),
        Some("the second revision"),
        "expected the revised description, got {body}"
    );
    assert_eq!(
        parameter_value(&body, "Parameters", "Shown").as_deref(),
        Some("revised-display"),
        "expected the revised parameter value, got {body}"
    );
    assert_eq!(
        parameter_value(&body, "Parameters", "Note").as_deref(),
        Some("revised"),
        "expected the new parameter's template default, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Value").as_deref(),
        Some("second"),
        "expected the revised tag set, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(stack_set_status::ACTIVE),
        "expected the StackSet to stay ACTIVE, got {body}"
    );

    // Each instance: brought up to date by the operation, with its own stack redeployed
    // from the revised template.
    for account in [ACCOUNT_ID, ACCOUNT_A] {
        let (http_status, body) =
            describe_stack_instance(&router, stack_set_name, account, REGION).await;
        assert_eq!(
            http_status, 200,
            "case {account:?}: expected the instance to be described, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(instance_status::CURRENT),
            "case {account:?}: expected the instance to be up to date, got {body}"
        );
        assert_eq!(
            xml_text(&body, "DetailedStatus").as_deref(),
            Some(detailed_status::SUCCEEDED),
            "case {account:?}: expected the update to have succeeded, got {body}"
        );
        assert_eq!(
            xml_text(&body, "LastOperationId").as_deref(),
            Some(update.as_str()),
            "case {account:?}: expected the update's own operation id, got {body}"
        );
        let stack_id = xml_text(&body, "StackId")
            .unwrap_or_else(|| panic!("case {account:?}: no StackId in {body}"));
        let (http_status, body) = get_template(&router, &stack_id, None).await;
        assert_eq!(
            http_status, 200,
            "case {account:?}: expected GetTemplate to succeed, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(REVISED_TEMPLATE),
            "case {account:?}: expected the instance's stack to carry the revision, got {body}"
        );
        let (http_status, body) = call_in(
            &router,
            REGION,
            "DescribeStacks",
            &[member("StackName", &stack_id)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {account:?}: expected DescribeStacks to succeed, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {account:?}: expected the instance's stack to report its update, got {body}"
        );
        assert_eq!(
            parameter_value(&body, "Parameters", "Shown").as_deref(),
            Some("revised-display"),
            "case {account:?}: expected the stack to carry the revised value, got {body}"
        );
    }

    // `UpdateStackInstances` overrides a parameter on ONE instance, and only that one: "any
    // overridden parameter values will be applied to all stack instances in the specified
    // accounts and Amazon Web Services Regions".
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("ParameterOverrides.member.1.ParameterKey", "Shown"),
        member("ParameterOverrides.member.1.ParameterValue", "overridden"),
    ];
    members.extend(selection(&[ACCOUNT_ID], &[REGION]));
    let (http_status, body) = call(&router, "UpdateStackInstances", &members).await;
    let override_operation = operation_id_of("the instance override", http_status, &body);
    wait_for_the_transition().await;

    let (http_status, body) =
        describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, REGION).await;
    assert_eq!(
        http_status, 200,
        "expected the overridden instance to be described, got {http_status} {body}"
    );
    assert_eq!(
        parameter_value(&body, "ParameterOverrides", "Shown").as_deref(),
        Some("overridden"),
        "expected the override to be in force on the instance, got {body}"
    );
    assert_eq!(
        xml_text(&body, "LastOperationId").as_deref(),
        Some(override_operation.as_str()),
        "expected the override's own operation id, got {body}"
    );
    let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| panic!("no StackId in {body}"));
    let (http_status, body) = call_in(
        &router,
        REGION,
        "DescribeStacks",
        &[member("StackName", &stack_id)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStacks to succeed, got {http_status} {body}"
    );
    assert_eq!(
        parameter_value(&body, "Parameters", "Shown").as_deref(),
        Some("overridden"),
        "expected the instance's stack to carry the override, got {body}"
    );

    let (http_status, body) =
        describe_stack_instance(&router, stack_set_name, ACCOUNT_A, REGION).await;
    assert_eq!(
        http_status, 200,
        "expected the untouched instance to be described, got {http_status} {body}"
    );
    assert_eq!(
        parameter_value(&body, "ParameterOverrides", "Shown"),
        None,
        "expected the instance the override did not select to carry none, got {body}"
    );
    assert_eq!(
        xml_text(&body, "LastOperationId").as_deref(),
        Some(update.as_str()),
        "expected the untouched instance to still report the StackSet update, got {body}"
    );

    // The operation history the two updates left behind: both are recorded on the StackSet,
    // and both SUCCEEDED.
    let (http_status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", stack_set_name)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected ListStackSetOperations to succeed, got {http_status} {body}"
    );
    let recorded = xml_texts(&body, "OperationId");
    for operation in [&update, &override_operation] {
        assert!(
            recorded.contains(operation),
            "expected the operation {operation} to be recorded, got {body}"
        );
    }
    assert!(
        !xml_texts(&body, "Status").contains(&operation_status::FAILED.to_string()),
        "expected no operation to have failed, got {body}"
    );
}
