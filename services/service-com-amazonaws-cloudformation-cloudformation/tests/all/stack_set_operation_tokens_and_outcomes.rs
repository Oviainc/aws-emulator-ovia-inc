// SPDX-License-Identifier: CC0-1.0

//! What a StackSet operation's `OperationId` and its OUTCOME are, end to end over the
//! awsQuery wire.
//!
//! Four rules, each asserted over a table of inputs rather than over one reported request:
//!
//! 1. **An `OperationId` the StackSet already holds an operation under is that operation**:
//!    the request is refused `409 OperationIdAlreadyExistsException` ("Operation with Id
//!    <id> already exists") whether that operation is still in flight or has concluded, and
//!    that refusal comes BEFORE the single-flight one — `OperationInProgressException` is
//!    the answer to a request carrying a DIFFERENT token while another operation holds the
//!    StackSet, because the two prescribe opposite client actions. The rows span every call
//!    that takes the token and is served by this suite's fixtures (`ImportStacksToStackSet`,
//!    the fifth, is covered in `import_stacks_to_stack_set.rs`).
//! 2. **A `CreateStackSet` for a name the administrator account and Region already holds is
//!    refused one FIXED sentence**: `StackSet already exists.`, which carries no part of the
//!    request — the rows differ in the name's length and spelling and in whether the request
//!    carries a `ClientRequestToken` of its own.
//! 3. **A stop of an operation that is not in a stoppable state is refused naming the status
//!    that operation is IN**: `Cannot Stop since Operation is in <status>`, where the status
//!    is the one `DescribeStackSetOperation` reports for it at that instant. The rows span
//!    the terminal statuses an operation can hold.
//! 4. **A target's deployment is reported as what the operation really did there**: it has
//!    "yet to start" while the operation is in flight, the operation's conclusion is what
//!    reports it SUCCEEDED, and a target whose deployment a stop cancelled reports
//!    CANCELLED — so a stopped operation reports no target as SUCCEEDED. The rows span the
//!    target counts a deployment can have: one pair, several accounts, several Regions.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees. The one thing the rows reach into the store
//! for is the instant an operation's conclusion is due: this store concludes one half a
//! second after the call that started it, which is shorter than a row's own wire work, so
//! holding that conclusion open (and then re-dating it) is what makes a row deterministic.

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, operation_action, operation_status, result_status, StoredStackSet,
    StoredStackSetOperation,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, member, router, router_with_engine, store, wait_for_the_transition, xml_blocks,
    xml_text, ACCOUNT_ID, REGION,
};
use crate::stop_stack_set_operation::hold_the_operation_open;

/// Target accounts beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";

/// A target Region beside [`REGION`]: a stack instance's Region is a target the deploying
/// request chooses, so a deployment of one account to two Regions has two targets.
const WEST: &str = "us-west-2";

/// The template every StackSet here is created from: one topic, really created in Amazon SNS
/// by each instance's stack.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"token-and-outcome"}}}}"#;

/// The `OperationIdAlreadyExistsException` message, spelled out here rather than read from
/// the crate: the sentence is the requirement, so a reworded one must fail this suite. The
/// id is the only variable part — the one the request carried.
fn operation_id_already_exists(operation_id: &str) -> String {
    format!("Operation with Id {operation_id} already exists")
}

/// The `NameAlreadyExistsException` message a duplicate `CreateStackSet` is refused with: a
/// fixed sentence with nothing of the request in it.
const STACK_SET_ALREADY_EXISTS: &str = "StackSet already exists.";

/// The `InvalidOperationException` message a stop of an operation in a non-stoppable state is
/// refused with, naming the status that operation is in.
fn cannot_stop(status: &str) -> String {
    format!("Cannot Stop since Operation is in {status}")
}

/// The `<Code>` and `<Message>` of a refusal.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// `CreateStackSet` from [`TEMPLATE`], plus whatever else the row names.
async fn create_stack_set(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("TemplateBody", TEMPLATE),
    ];
    members.extend_from_slice(extra);
    call(router, "CreateStackSet", &members).await
}

/// `CreateStackInstances` over the product of `accounts` and `regions`, under `operation_id`
/// when the row names one.
async fn create_stack_instances(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
    operation_id: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    for (index, account) in accounts.iter().enumerate() {
        members.push(member(&format!("Accounts.member.{}", index + 1), account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    if let Some(operation_id) = operation_id {
        members.push(member("OperationId", operation_id));
    }
    call(router, "CreateStackInstances", &members).await
}

/// `ListStackSetOperationResults` for one operation of one StackSet.
async fn list_results(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "ListStackSetOperationResults",
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// `DescribeStackSetOperation`, which is what a caller monitors an operation through.
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

/// `DescribeStackInstance` for one account and Region pair.
async fn describe_instance(
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

/// `StopStackSetOperation` for one operation of one StackSet.
async fn stop(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "StopStackSetOperation",
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// `GetTemplateSummary` over the body the request carries.
async fn summarise_body(router: &MultiServiceRouter, template_body: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("TemplateBody", template_body)],
    )
    .await
}

/// `GetTemplateSummary` for the template a StackSet holds.
async fn summarise_stack_set(router: &MultiServiceRouter, stack_set_name: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("StackSetName", stack_set_name)],
    )
    .await
}

/// Let the conclusion a held-open operation still owes become due NOW, so the next call
/// settles it: the complement of [`hold_the_operation_open`], and the only other thing a row
/// reaches into the store for.
fn let_the_operation_conclude(
    engine: &Arc<engine::InMemoryEngine>,
    stack_set_name: &str,
    operation_id: &str,
) {
    let state = store(engine);
    let mut held = state.lock();
    let stack_set = held
        .resolve_stack_set_mut(REGION, stack_set_name)
        .unwrap_or_else(|| panic!("the store holds StackSet {stack_set_name}"));
    let operation = stack_set
        .operations
        .iter_mut()
        .find(|operation| operation.operation_id == operation_id)
        .unwrap_or_else(|| panic!("StackSet {stack_set_name} holds operation {operation_id}"));
    if let Some(pending) = operation.pending.as_mut() {
        pending.due_at = SystemTime::now();
    }
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

/// One row of the token table: a call that takes an `OperationId`, with the members it needs
/// beside the token.
struct TokenRow {
    case: &'static str,
    stack_set: &'static str,
    /// The `Action` the row calls.
    starts: &'static str,
    /// The request members beside `OperationId`, which both the first call and the replay of
    /// it carry: a replay is the SAME request with the same token.
    members: Vec<(String, String)>,
}

/// One row per call that takes an `OperationId` and is served against a StackSet holding one
/// instance: a deployment of a new pair, an update of the held one, a deletion of it, and an
/// update of the StackSet itself.
fn token_rows() -> Vec<TokenRow> {
    vec![
        TokenRow {
            case: "a deployment of a new pair",
            stack_set: "tokens-create-instances",
            starts: "CreateStackInstances",
            members: vec![
                member("StackSetName", "tokens-create-instances"),
                member("Accounts.member.1", ACCOUNT_A),
                member("Regions.member.1", REGION),
            ],
        },
        TokenRow {
            case: "an update of the pair the StackSet holds",
            stack_set: "tokens-update-instances",
            starts: "UpdateStackInstances",
            members: vec![
                member("StackSetName", "tokens-update-instances"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("Regions.member.1", REGION),
            ],
        },
        TokenRow {
            case: "a deletion of the pair the StackSet holds",
            stack_set: "tokens-delete-instances",
            starts: "DeleteStackInstances",
            members: vec![
                member("StackSetName", "tokens-delete-instances"),
                member("RetainStacks", "true"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("Regions.member.1", REGION),
            ],
        },
        TokenRow {
            case: "an update of the StackSet itself",
            stack_set: "tokens-update-set",
            starts: "UpdateStackSet",
            members: vec![
                member("StackSetName", "tokens-update-set"),
                member("UsePreviousTemplate", "true"),
            ],
        },
    ]
}

/// A StackSet with one instance, in [`ACCOUNT_ID`] and [`REGION`], whose deployment has
/// concluded: the fixture the token rows start from.
async fn stack_set_with_an_instance(router: &MultiServiceRouter, stack_set_name: &str) {
    let (status, body) = create_stack_set(router, stack_set_name, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet {stack_set_name} to succeed, got {status} {body}"
    );
    let (status, body) =
        create_stack_instances(router, stack_set_name, &[ACCOUNT_ID], &[REGION], None).await;
    assert_eq!(
        status, 200,
        "expected the fixture's deployment to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;
}

#[tokio::test]
async fn a_replayed_operation_id_is_the_operation_it_names_not_the_single_flight_lock() {
    for row in token_rows() {
        let case = row.case;
        let (router, engine) = router_with_engine();
        stack_set_with_an_instance(&router, row.stack_set).await;

        // The operation this row starts, under a token the caller chose
        // (`OperationId` is a `ClientRequestToken`: `@length(1..128)`, `@pattern
        // ^[a-zA-Z0-9][-a-zA-Z0-9]*$`).
        let token = "theOne-1";
        let mut named = row.members.clone();
        named.push(member("OperationId", token));
        let (http_status, body) = call(&router, row.starts, &named).await;
        let answered = operation_id_of(case, http_status, &body);
        assert_eq!(
            answered, token,
            "case {case:?}: expected the operation to run as the token the request named, \
             got {answered:?}"
        );
        // Held open so the probes below land on an operation that really has not concluded.
        hold_the_operation_open(&engine, row.stack_set, token);

        // The SAME token while that operation is in flight: the caller is re-sending its own
        // request, and the work already happened.
        let (http_status, body) = call(&router, row.starts, &named).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected the replayed token to be refused with 409, got \
             {http_status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("OperationIdAlreadyExistsException"),
            "case {case:?}: expected the idempotency refusal while in flight, got {body}"
        );
        let expected = operation_id_already_exists(token);
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // A DIFFERENT token while that operation is in flight: another operation holds the
        // StackSet, which is the retry-after signal rather than the idempotency one.
        let mut other = row.members.clone();
        other.push(member("OperationId", "theOther-2"));
        let (http_status, body) = call(&router, row.starts, &other).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected a second operation to be refused with 409, got \
             {http_status} {body}"
        );
        let (code, _) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("OperationInProgressException"),
            "case {case:?}: expected the single-flight refusal for another token, got {body}"
        );

        // The same token once that operation has CONCLUDED: still the operation it names, so
        // still the idempotency refusal — and it is answered before any refusal about what
        // the request would do a second time.
        let_the_operation_conclude(&engine, row.stack_set, token);
        wait_for_the_transition().await;
        let (http_status, body) = call(&router, row.starts, &named).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected the replayed token to be refused after the operation \
             concluded, got {http_status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("OperationIdAlreadyExistsException"),
            "case {case:?}: expected the idempotency refusal after the conclusion, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// One row of the duplicate-name table: the name the second create asks for, and what that
/// request carries beside it.
struct NameRow {
    case: &'static str,
    stack_set: &'static str,
    extra: Vec<(String, String)>,
}

/// Names of several lengths and spellings — the model constrains `StackSetName` with
/// neither a length nor a pattern (`smithy_query(query="shape", shape="StackSetName")`), so
/// what the rows vary is what a caller may really send — and both with and without a request
/// token of the second create's own: the refusal is the same fixed sentence for each.
fn name_rows() -> Vec<NameRow> {
    vec![
        NameRow {
            case: "a one-character name",
            stack_set: "z",
            extra: Vec::new(),
        },
        NameRow {
            case: "a name of digits and hyphens",
            stack_set: "Fleet-2-of-9",
            extra: Vec::new(),
        },
        NameRow {
            case: "a long name",
            stack_set: "a-very-long-stack-set-name-that-goes-on-and-on-and-on-and-on-and-on-\
                        and-on-and-on-and-on-and-on-and-on-and-on-and-onX",
            extra: Vec::new(),
        },
        NameRow {
            case: "a second create carrying a request token of its own",
            stack_set: "fleet-with-a-token",
            extra: vec![member("ClientRequestToken", "a-token-of-its-own")],
        },
    ]
}

#[tokio::test]
async fn a_duplicate_stack_set_name_is_refused_one_fixed_sentence() {
    let router = router();
    for row in name_rows() {
        let case = row.case;
        let (status, body) = create_stack_set(&router, row.stack_set, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to succeed, got {status} {body}"
        );

        let (status, body) = create_stack_set(&router, row.stack_set, &row.extra).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the duplicate name to be refused with 409, got {status} \
             {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("NameAlreadyExistsException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(STACK_SET_ALREADY_EXISTS),
            "case {case:?}: expected the fixed sentence, got {message:?}"
        );
        // The invariant the sentence being fixed is FOR: nothing of the request is in it, so
        // two runs against two generated names read the same answer.
        let message = message.unwrap_or_default();
        assert!(
            !message.contains(row.stack_set),
            "case {case:?}: expected the refusal to carry no part of the request, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_stop_of_an_operation_that_has_ended_names_the_status_it_is_in() {
    // The wire-driven row: an operation this service really ran and concluded.
    const DEPLOYED: &str = "stop-after-the-deployment";
    let (router, engine) = router_with_engine();
    stack_set_with_an_instance(&router, DEPLOYED).await;
    let (http_status, body) =
        create_stack_instances(&router, DEPLOYED, &[ACCOUNT_A], &[REGION], None).await;
    let operation_id = operation_id_of("a deployment that completed", http_status, &body);
    wait_for_the_transition().await;

    let (http_status, body) = describe_operation(&router, DEPLOYED, &operation_id).await;
    assert_eq!(
        http_status, 200,
        "case \"a deployment that completed\": expected the operation to be readable, got \
         {http_status} {body}"
    );
    let reported = xml_text(&body, "Status").unwrap_or_default();
    assert_eq!(
        reported,
        operation_status::SUCCEEDED,
        "case \"a deployment that completed\": expected the concluded status, got {body}"
    );

    let (http_status, body) = stop(&router, DEPLOYED, &operation_id).await;
    assert_eq!(
        http_status, 400,
        "case \"a deployment that completed\": expected the stop to be refused with 400, got \
         {http_status} {body}"
    );
    let (code, message) = refusal(&body);
    assert_eq!(
        code.as_deref(),
        Some("InvalidOperationException"),
        "case \"a deployment that completed\": expected the modelled refusal, got {body}"
    );
    // The status the refusal names is the status the operation IS in, which is the one a read
    // of it reports at the same instant.
    let expected = cannot_stop(&reported);
    assert_eq!(
        message.as_deref(),
        Some(expected.as_str()),
        "case \"a deployment that completed\": expected {expected:?}, got {message:?}"
    );

    // The terminal statuses a wire call cannot leave an operation in by itself, seeded
    // straight into the store: the refusal names each of them.
    for (case, stack_set, status) in [
        (
            "an operation that exceeded its failure tolerance",
            "stop-failed",
            operation_status::FAILED,
        ),
        (
            "an operation the caller already stopped",
            "stop-stopped",
            operation_status::STOPPED,
        ),
    ] {
        let operation_id = format!("operation-{}", status.to_lowercase());
        {
            let state = store(&engine);
            let mut held = state.lock();
            let mut stack_set = StoredStackSet::new(stack_set, REGION);
            let mut operation = StoredStackSetOperation::running(
                operation_id.clone(),
                operation_action::CREATE,
                SystemTime::now(),
            );
            operation.status = status.to_string();
            operation.pending = None;
            operation.end = Some(SystemTime::now());
            stack_set.record_operation(operation);
            held.record_stack_set(stack_set);
        }

        let (http_status, body) = describe_operation(&router, stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the operation to be readable, got {http_status} {body}"
        );
        let reported = xml_text(&body, "Status").unwrap_or_default();
        assert_eq!(
            reported, status,
            "case {case:?}: expected the seeded status, got {body}"
        );

        let (http_status, body) = stop(&router, stack_set, &operation_id).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected the stop to be refused with 400, got {http_status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidOperationException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        let expected = cannot_stop(&reported);
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// One row of the targets table: the accounts and Regions one deployment acts on, whose
/// product is the set of targets its results report.
struct TargetsRow {
    case: &'static str,
    stack_set: &'static str,
    accounts: &'static [&'static str],
    regions: &'static [&'static str],
}

/// The target counts a deployment can have: one pair, two accounts in one Region, and one
/// account in two Regions — a result is reported per PAIR, so the last two have two targets
/// each by different means.
const TARGET_ROWS: &[TargetsRow] = &[
    TargetsRow {
        case: "one account in one Region",
        stack_set: "outcome-one-pair",
        accounts: &[ACCOUNT_A],
        regions: &[REGION],
    },
    TargetsRow {
        case: "two accounts in one Region",
        stack_set: "outcome-two-accounts",
        accounts: &[ACCOUNT_A, ACCOUNT_B],
        regions: &[REGION],
    },
    TargetsRow {
        case: "one account in two Regions",
        stack_set: "outcome-two-regions",
        accounts: &[ACCOUNT_A],
        regions: &[REGION, WEST],
    },
];

/// The `Status` the response reports for each target of one operation, as `(account, region,
/// status)` triples in the order it reports them.
fn reported_results(body: &str) -> Vec<(String, String, String)> {
    xml_blocks(body, "member")
        .iter()
        .map(|summary| {
            (
                xml_text(summary, "Account").unwrap_or_default(),
                xml_text(summary, "Region").unwrap_or_default(),
                xml_text(summary, "Status").unwrap_or_default(),
            )
        })
        .collect()
}

/// The `(account, region, status)` triples one row's deployment must report, in the order the
/// StackSet holds its instances: the product of the row's accounts and Regions.
fn expected_results(row: &TargetsRow, status: &str) -> Vec<(String, String, String)> {
    let mut expected = Vec::new();
    for account in row.accounts {
        for region in row.regions {
            expected.push((
                (*account).to_string(),
                (*region).to_string(),
                status.to_string(),
            ));
        }
    }
    expected
}

#[tokio::test]
async fn a_deployment_reports_each_target_as_what_the_operation_did_there() {
    for row in TARGET_ROWS {
        let case = row.case;
        let (router, engine) = router_with_engine();
        let (status, body) = create_stack_set(&router, row.stack_set, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStackSet to succeed, got {status} {body}"
        );

        let (http_status, body) =
            create_stack_instances(&router, row.stack_set, row.accounts, row.regions, None).await;
        let operation_id = operation_id_of(case, http_status, &body);
        // Held open so what the row reads next is what an operation in flight reports.
        hold_the_operation_open(&engine, row.stack_set, &operation_id);

        // While the operation is in flight no target has been reached yet: "PENDING: The
        // operation in the specified account and Region has yet to start."
        let (status, body) = list_results(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the results to be readable, got {status} {body}"
        );
        assert_eq!(
            reported_results(&body),
            expected_results(row, result_status::PENDING),
            "case {case:?}: expected every target to have yet to start, got {body}"
        );

        // The conclusion is what carries the deployment out in each target.
        let_the_operation_conclude(&engine, row.stack_set, &operation_id);
        wait_for_the_transition().await;
        let (status, body) = list_results(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the results to be readable, got {status} {body}"
        );
        assert_eq!(
            reported_results(&body),
            expected_results(row, result_status::SUCCEEDED),
            "case {case:?}: expected the conclusion to report every target, got {body}"
        );
    }
}

#[tokio::test]
async fn a_stopped_deployment_reports_no_target_as_succeeded() {
    for row in TARGET_ROWS {
        let case = row.case;
        let (router, engine) = router_with_engine();
        let (status, body) = create_stack_set(&router, row.stack_set, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStackSet to succeed, got {status} {body}"
        );

        let (http_status, body) =
            create_stack_instances(&router, row.stack_set, row.accounts, row.regions, None).await;
        let operation_id = operation_id_of(case, http_status, &body);
        hold_the_operation_open(&engine, row.stack_set, &operation_id);

        let (http_status, body) = stop(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the stop to be served, got {http_status} {body}"
        );
        wait_for_the_transition().await;

        // "STOPPED: The user has canceled the operation", and the deployments it cancelled
        // are "CANCELLED: The operation in the specified account and Region has been
        // canceled" — a target the stop caught before it started never deployed, so nothing
        // reports a success for it.
        let (http_status, body) = describe_operation(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the operation to be readable, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::STOPPED),
            "case {case:?}: expected the stop to conclude the operation, got {body}"
        );

        let (status, body) = list_results(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the results to be readable, got {status} {body}"
        );
        let reported = reported_results(&body);
        assert_eq!(
            reported,
            expected_results(row, result_status::CANCELLED),
            "case {case:?}: expected every cancelled target to report that, got {body}"
        );
        assert!(
            !reported
                .iter()
                .any(|(_, _, status)| status == result_status::SUCCEEDED),
            "case {case:?}: a stopped operation reports no target as succeeded, got \
             {reported:?}"
        );

        // The instances themselves agree with the results they are read off: a cancelled
        // deployment is not up to date with its StackSet.
        for account in row.accounts {
            for region in row.regions {
                let (status, body) =
                    describe_instance(&router, row.stack_set, account, region).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {account}/{region} to be described, got {status} \
                     {body}"
                );
                assert_eq!(
                    xml_text(&body, "DetailedStatus").as_deref(),
                    Some(detailed_status::CANCELLED),
                    "case {case:?}: expected {account}/{region} to report the cancellation, \
                     got {body}"
                );
            }
        }
    }
}

/// One row of the format-version table: a template body, and the `AWSTemplateFormatVersion`
/// it DECLARES — `None` for one that declares none.
struct VersionRow {
    case: &'static str,
    stack_set: &'static str,
    template: &'static str,
    declared: Option<&'static str>,
}

/// The latest `AWSTemplateFormatVersion`, which is the version in force for a template that
/// declares none: "if you don't specify a value, CloudFormation assumes the latest template
/// format version", and the latest is the only value the section accepts. Spelled out here
/// rather than read from the crate: the value is the requirement.
const LATEST_FORMAT_VERSION: &str = "2010-09-09";

/// Both template spellings, each declaring the version and each declaring none: what the
/// rule generalises over is whether the template STATES the version, not how it is spelled.
const VERSION_ROWS: &[VersionRow] = &[
    VersionRow {
        case: "JSON declaring the format version",
        stack_set: "version-json-declared",
        template: r#"{"AWSTemplateFormatVersion":"2010-09-09",
             "Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
        declared: Some("2010-09-09"),
    },
    VersionRow {
        case: "JSON declaring none",
        stack_set: "version-json-absent",
        template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
        declared: None,
    },
    VersionRow {
        case: "YAML declaring the format version",
        stack_set: "version-yaml-declared",
        template: "AWSTemplateFormatVersion: '2010-09-09'\n\
                   Resources:\n\
                   \x20 Topic:\n\
                   \x20   Type: AWS::SNS::Topic\n",
        declared: Some("2010-09-09"),
    },
    VersionRow {
        case: "YAML declaring none",
        stack_set: "version-yaml-absent",
        template: "Resources:\n\
                   \x20 Topic:\n\
                   \x20   Type: AWS::SNS::Topic\n",
        declared: None,
    },
];

#[tokio::test]
async fn a_template_summary_reports_the_format_version_in_force() {
    let router = router();
    for row in VERSION_ROWS {
        let case = row.case;
        let in_force = row.declared.unwrap_or(LATEST_FORMAT_VERSION);

        // The body the request carries.
        let (status, body) = summarise_body(&router, row.template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetTemplateSummary, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Version").as_deref(),
            Some(in_force),
            "case {case:?}: expected the format version in force, got {body}"
        );

        // The same template read back through the StackSet it was created with: a summary of
        // the template that StackSet really holds.
        let (status, body) = call(
            &router,
            "CreateStackSet",
            &[
                member("StackSetName", row.stack_set),
                member("TemplateBody", row.template),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStackSet to succeed, got {status} {body}"
        );
        let (status, body) = summarise_stack_set(&router, row.stack_set).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for the StackSet's own template, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Version").as_deref(),
            Some(in_force),
            "case {case:?}: expected the StackSet's template to report the same version in \
             force, got {body}"
        );
    }
}
