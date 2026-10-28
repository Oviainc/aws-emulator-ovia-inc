// SPDX-License-Identifier: CC0-1.0

//! How the StackSets refusals that address stack instances by account and Region are WORDED,
//! over the awsQuery wire.
//!
//! The rule every case here holds the service to: **a refusal's message belongs to the
//! OPERATION that refused, not to the exception shape alone — an operation that READS one
//! stack instance answers `StackInstanceNotFoundException` with its fixed sentence, an
//! operation that UPDATES the instances a `Regions` list selects answers the same exception
//! with a sentence naming the Region of that list which resolved no instance, and a
//! `RegionList` entry that is no Region at all is refused `ValidationError` with a sentence
//! naming that entry — in every case the fixed text is the service's and the only variable
//! part is taken from the request.**
//!
//! The tables below span that class rather than one recorded request: the reading side over
//! three pairs a StackSet holds no instance for (a Region it was never deployed to, an
//! account it was never deployed to, and neither), the updating side over three Regions, over
//! a selection mixing a Region that resolves with one that does not, over a pair whose
//! ACCOUNT is what is missing, and over both operations that update a selection
//! (`UpdateStackInstances` and `UpdateStackSet`); the Region check over three labels the
//! `Region` pattern admits and the Region catalog does not, on each operation that takes a
//! `RegionList`.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out), so
//! what is asserted is what a client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, member, router, wait_for_the_transition, xml_text, ACCOUNT_ID, REGION,
};

/// The template the StackSets here are created from: one topic, as a StackSet needs something
/// for an instance's stack to deploy.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"instance-display"}}}}"#;

/// A target account beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";

/// Two Regions of the catalog beside [`REGION`], neither of which the StackSet below is
/// deployed to.
const WEST: &str = "us-west-2";
const IRELAND: &str = "eu-west-1";

/// The sentence the operations that READ one stack instance answer when the StackSet holds
/// none for the account and Region the request named: entirely fixed text, spelled out here
/// rather than read from the crate, because the sentence IS the requirement and a reworded one
/// must fail this suite.
const READ_NOT_FOUND: &str = "StackInstance not found";

/// The sentence the operations that UPDATE the instances a `Regions` list selects answer for
/// the same exception: fixed text plus the one Region of that list which resolved nothing.
fn update_not_found(region: &str) -> String {
    format!("No Stack Instance found in region {region}")
}

/// The refusal for a `RegionList` entry that is no Region: fixed text plus the entry, as the
/// request spelled it.
fn unsupported_region(region: &str) -> String {
    format!("Region {region} is not supported")
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

/// `DescribeStackInstance` for one pair.
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

/// The `<Code>` and `<Message>` of a refusal envelope.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// One row of a refusal-wording table: the request to send and the message it must answer.
struct Row {
    case: &'static str,
    action: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    message: String,
}

/// Send every row and hold each answer to its status, code and message.
async fn assert_rows(router: &MultiServiceRouter, rows: Vec<Row>) {
    for row in rows {
        let Row {
            case,
            action,
            members,
            status,
            code,
            message,
        } = row;
        let (http_status, body) = call(router, action, &members).await;
        assert_eq!(
            http_status, status,
            "case {case:?} on {action}: expected HTTP {status}, got {http_status} {body}"
        );
        let (actual_code, actual_message) = refusal(&body);
        assert_eq!(
            actual_code.as_deref(),
            Some(code),
            "case {case:?} on {action}: expected <Code>{code}</Code>, got {actual_code:?}"
        );
        assert_eq!(
            actual_message.as_deref(),
            Some(message.as_str()),
            "case {case:?} on {action}: expected {message:?}, got {actual_message:?}"
        );
    }
}

/// The reading operations and the updating operations word `StackInstanceNotFoundException`
/// differently, and the updating ones name the Region of the selection that resolved nothing.
#[tokio::test]
async fn stack_instance_not_found_is_worded_by_the_operation_that_refused() {
    let router = router();
    let stack_set_name = "instance-refusal-wording";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );
    // ONE instance, so every pair below is a pair this StackSet really holds none for while
    // the StackSet itself resolves.
    let mut members = vec![member("StackSetName", stack_set_name)];
    members.extend(selection(&[ACCOUNT_ID], &[REGION]));
    let (status, body) = call(&router, "CreateStackInstances", &members).await;
    assert_eq!(
        status, 200,
        "expected CreateStackInstances to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    // The reading side: one fixed sentence, whichever pair the request named — the account and
    // Region are the caller's own, so they are no part of it.
    let mut rows: Vec<Row> = Vec::new();
    for (case, account, region) in [
        ("a Region the StackSet was never deployed to", ACCOUNT_ID, WEST),
        ("an account the StackSet was never deployed to", ACCOUNT_A, REGION),
        ("neither the account nor the Region deployed", ACCOUNT_A, IRELAND),
    ] {
        rows.push(Row {
            case,
            action: "DescribeStackInstance",
            members: vec![
                member("StackSetName", stack_set_name),
                member("StackInstanceAccount", account),
                member("StackInstanceRegion", region),
            ],
            status: 404,
            code: "StackInstanceNotFoundException",
            message: READ_NOT_FOUND.to_string(),
        });
    }

    // The updating side: the Region of the selection that resolved nothing, taken from the
    // request's own `Regions` list — a different Region per row, so a constant cannot pass.
    for (case, accounts, regions, named) in [
        (
            "an update of a Region the StackSet was never deployed to",
            vec![ACCOUNT_ID],
            vec![WEST],
            WEST,
        ),
        (
            "an update of another Region the StackSet was never deployed to",
            vec![ACCOUNT_ID],
            vec![IRELAND],
            IRELAND,
        ),
        (
            "a selection mixing a Region that resolves with one that does not",
            vec![ACCOUNT_ID],
            vec![REGION, WEST],
            WEST,
        ),
        (
            "a pair whose ACCOUNT is the half with no instance",
            vec![ACCOUNT_A],
            vec![REGION],
            REGION,
        ),
    ] {
        let mut members = vec![member("StackSetName", stack_set_name)];
        members.extend(selection(&accounts, &regions));
        rows.push(Row {
            case,
            action: "UpdateStackInstances",
            members,
            status: 404,
            code: "StackInstanceNotFoundException",
            message: update_not_found(named),
        });
        // `UpdateStackSet` selects the instances to roll an update over the same way, so it
        // words the refusal the same way.
        let mut members = vec![
            member("StackSetName", stack_set_name),
            member("UsePreviousTemplate", "true"),
        ];
        members.extend(selection(&accounts, &regions));
        rows.push(Row {
            case,
            action: "UpdateStackSet",
            members,
            status: 404,
            code: "StackInstanceNotFoundException",
            message: update_not_found(named),
        });
    }
    assert_rows(&router, rows).await;

    // None of the refusals touched the instance the StackSet does hold, so each was about the
    // pair it named rather than about the StackSet.
    let (status, body) =
        describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, REGION).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the deployed instance to be readable still, got {status} {body}",
        "the instance the refusals did not name"
    );
}

/// A `Regions` entry the catalog does not hold is refused naming that entry, on every
/// operation that takes a `RegionList`.
#[tokio::test]
async fn an_unknown_target_region_is_refused_naming_the_region() {
    let router = router();
    let stack_set_name = "region-refusal-wording";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );

    // Labels the model's `Region` `@pattern("^[a-zA-Z0-9-]{1,128}$")` admits and
    // `emulator_telemetry::policy::KNOWN_REGIONS` — the one catalog of Region codes this
    // emulator holds — does not, none of them the label the issue cited.
    let mut rows: Vec<Row> = Vec::new();
    for (case, unknown) in [
        ("an unpublished number in a published family", "eu-central-7"),
        ("a partition-wide label, which is no Region", "aws-global"),
        ("a two-letter label", "xx"),
    ] {
        for action in [
            "CreateStackInstances",
            "UpdateStackInstances",
            "DeleteStackInstances",
            "UpdateStackSet",
        ] {
            // A real Region beside the unknown one: the request is refused WHOLESALE, so the
            // Region that does resolve carries no instance either (asserted below).
            let mut members = vec![member("StackSetName", stack_set_name)];
            members.extend(selection(&[ACCOUNT_ID], &[REGION, unknown]));
            if action == "DeleteStackInstances" {
                members.push(member("RetainStacks", "true"));
            }
            if action == "UpdateStackSet" {
                members.push(member("UsePreviousTemplate", "true"));
            }
            rows.push(Row {
                case,
                action,
                members,
                status: 400,
                code: "ValidationError",
                message: unsupported_region(unknown),
            });
        }
    }
    assert_rows(&router, rows).await;

    // Nothing was deployed by any of those refused requests.
    let (status, body) =
        describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, REGION).await;
    assert_eq!(
        status, 404,
        "case {:?}: expected no instance to have been created, got {status} {body}",
        "the valid Region named beside an unknown one"
    );
}
