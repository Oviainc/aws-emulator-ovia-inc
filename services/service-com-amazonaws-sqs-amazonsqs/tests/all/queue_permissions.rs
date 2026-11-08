// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the permissions a queue grants.
//!
//! The rule under test: **a permission IS a statement of the queue's own access policy**
//! — a grant appends one statement, identified by the request's `Label`, allowing each
//! named account the named actions on that queue, to whatever the queue's `Policy`
//! attribute already carried; revoking a label takes exactly that statement back out,
//! and a queue left with no statement carries no policy again. Every grant and every
//! revoke is therefore observable through `GetQueueAttributes`, the one place the policy
//! lives, and a request outside what the parameters document — a label that is not a
//! label, a principal that is not an account number, an action that is not an action,
//! more than a policy may hold — is refused rather than persisted.
//!
//! The tables span what the rule generalises over: how many principals and how many
//! actions one grant names (one, several, the wildcard), how many grants a queue
//! carries, which of them is revoked, and each published bound with the first value past
//! it. The action catalog, the bounds and the policy's own member values come from the
//! constants [`service_com_amazonaws_sqs_amazonsqs::state`] exports, never from a string
//! copied out of an answer.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    authorization::policy_action_names, MAX_ACTIONS_PER_STATEMENT, MAX_PERMISSION_LABEL_LENGTH,
    MAX_POLICY_STATEMENTS, MAX_PRINCIPALS_PER_STATEMENT,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    account, all_attributes, call, create_ok, emulator, error_code, expected_arn, ok,
    OTHER_ACCOUNT,
};

// The wire values AWS fixes for a generated queue policy are spelled HERE, as AWS
// publishes them, rather than read back off the implementation's own constants: an
// expectation taken from the code it checks would pass whatever that code said.

/// The policy language version a GENERATED queue policy declares — the long-standing
/// default policy language version Amazon SQS synthesises `SQSDefaultPolicy` in, not the
/// one a caller writing a policy today would use.
const EXPECTED_POLICY_VERSION: &str = "2008-10-17";

/// The policy language version the rows that UPLOAD a document write, which the queue
/// keeps as written: a policy a caller supplies is stored with its own version.
const UPLOADED_POLICY_VERSION: &str = "2012-10-17";

/// What the `Id` of a generated queue policy is: the queue's ARN followed by this.
const EXPECTED_POLICY_ID_SUFFIX: &str = "/SQSDefaultPolicy";

/// The prefix a generated queue policy qualifies its action names with.
const EXPECTED_ACTION_PREFIX: &str = "SQS:";

/// The action name standing for every action ("the name of any action or *").
const EXPECTED_ANY_ACTION: &str = "*";

/// A second 12-digit account number, for the rows that grant to more than one principal.
const SECOND_ACCOUNT: &str = "210987654321";

/// `count` distinct 12-digit account numbers, for the rows that name as many principals
/// as a statement may and one more.
fn account_numbers(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{index:012}")).collect()
}

/// The principal a policy names for an account number.
fn root_of(account_number: &str) -> String {
    format!("arn:aws:iam::{account_number}:root")
}

/// The `Policy` the queue at `url` reports, parsed — `None` when it reports none at all.
async fn policy_of(router: &MultiServiceRouter, url: &str) -> Option<Value> {
    let attributes = all_attributes(router, url).await;
    let policy = attributes.get("Policy")?.clone();
    Some(serde_json::from_str(&policy).unwrap_or_else(|error| {
        panic!("case the policy of {url}: expected a JSON document, got {policy:?} ({error})")
    }))
}

/// The statements a reported policy carries, in policy order.
fn statements(policy: &Value) -> Vec<Value> {
    policy
        .get("Statement")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The labels (statement `Sid`s) a reported policy carries, in policy order.
fn labels(policy: &Value) -> Vec<String> {
    statements(policy)
        .iter()
        .filter_map(|statement| {
            statement
                .get("Sid")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

/// The one statement a reported policy carries for `label`.
fn statement_for(policy: &Value, label: &str) -> Value {
    let found: Vec<Value> = statements(policy)
        .into_iter()
        .filter(|statement| statement.get("Sid").and_then(Value::as_str) == Some(label))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case the statement labelled {label}: expected exactly one, got {found:?} in {policy}"
    );
    found.into_iter().next().expect("one statement")
}

/// Grant `actions` to `accounts` on the queue at `url` under `label`.
async fn grant(
    router: &MultiServiceRouter,
    url: &str,
    label: &str,
    accounts: &[&str],
    actions: &[&str],
) -> (u16, Value) {
    call(
        router,
        "AddPermission",
        &json!({
            "QueueUrl": url,
            "Label": label,
            "AWSAccountIds": accounts,
            "Actions": actions,
        }),
    )
    .await
}

/// One row of the grant table: what the request names, and the `Principal`/`Action` the
/// statement it leaves behind must carry.
struct Grant {
    case: &'static str,
    label: String,
    accounts: Vec<String>,
    actions: Vec<String>,
    principal: Value,
    action: Value,
}

/// A grant of any shape becomes one statement of the queue's policy, carrying the label
/// it was granted under, the principals it named and the actions it allowed — and every
/// earlier grant stays exactly where it was.
#[tokio::test]
async fn every_shape_of_grant_becomes_one_statement_of_the_queue_policy() {
    let router = emulator();
    let url = create_ok(&router, "granting", &json!({})).await;
    let arn = expected_arn("granting");

    assert!(
        policy_of(&router, &url).await.is_none(),
        "case a queue nobody granted a permission on: expected no Policy, got {:?}",
        policy_of(&router, &url).await
    );

    // The bound is the documented maximum of actions per statement, taken off the
    // catalog of action names a policy may state rather than spelled out again.
    let at_the_action_bound: Vec<String> = policy_action_names()
        .iter()
        .take(MAX_ACTIONS_PER_STATEMENT)
        .map(ToString::to_string)
        .collect();
    let rows = vec![
        Grant {
            case: "one principal, one action",
            label: "one-to-one".to_string(),
            accounts: vec![OTHER_ACCOUNT.to_string()],
            actions: vec!["SendMessage".to_string()],
            principal: json!(root_of(OTHER_ACCOUNT)),
            action: json!(format!("{EXPECTED_ACTION_PREFIX}SendMessage")),
        },
        Grant {
            case: "one principal, several actions",
            label: "one-to-many".to_string(),
            accounts: vec![OTHER_ACCOUNT.to_string()],
            actions: vec!["ReceiveMessage".to_string(), "DeleteMessage".to_string()],
            principal: json!(root_of(OTHER_ACCOUNT)),
            action: json!([
                format!("{EXPECTED_ACTION_PREFIX}ReceiveMessage"),
                format!("{EXPECTED_ACTION_PREFIX}DeleteMessage"),
            ]),
        },
        Grant {
            // `PurgeQueue` is an entry of the modelled action catalog that no recorded
            // grant names, so the catalog is exercised beyond the familiar actions.
            case: "several principals, one action",
            label: "many-to-one".to_string(),
            accounts: vec![OTHER_ACCOUNT.to_string(), SECOND_ACCOUNT.to_string()],
            actions: vec!["PurgeQueue".to_string()],
            principal: json!([root_of(OTHER_ACCOUNT), root_of(SECOND_ACCOUNT)]),
            action: json!(format!("{EXPECTED_ACTION_PREFIX}PurgeQueue")),
        },
        Grant {
            case: "the wildcard action",
            label: "every_action".to_string(),
            accounts: vec![OTHER_ACCOUNT.to_string()],
            actions: vec![EXPECTED_ANY_ACTION.to_string()],
            principal: json!(root_of(OTHER_ACCOUNT)),
            action: json!(format!("{EXPECTED_ACTION_PREFIX}{EXPECTED_ANY_ACTION}")),
        },
        Grant {
            case: "as many actions as one statement may allow",
            label: "at-the-action-bound".to_string(),
            accounts: vec![OTHER_ACCOUNT.to_string()],
            actions: at_the_action_bound.clone(),
            principal: json!(root_of(OTHER_ACCOUNT)),
            action: Value::Array(
                at_the_action_bound
                    .iter()
                    .map(|action| json!(format!("{EXPECTED_ACTION_PREFIX}{action}")))
                    .collect(),
            ),
        },
        Grant {
            // The longest label a label may be, so the length bound is exercised by a
            // grant that must be accepted as well as by one that must not.
            case: "a label of the greatest length a label may have",
            label: "l".repeat(MAX_PERMISSION_LABEL_LENGTH),
            accounts: vec![OTHER_ACCOUNT.to_string()],
            actions: vec!["GetQueueAttributes".to_string()],
            principal: json!(root_of(OTHER_ACCOUNT)),
            action: json!(format!("{EXPECTED_ACTION_PREFIX}GetQueueAttributes")),
        },
        Grant {
            // As many principals as one statement may name, so that bound too is
            // exercised by a grant that must be accepted as well as by one that must not.
            case: "as many principals as one statement may name",
            label: "at-the-principal-bound".to_string(),
            accounts: account_numbers(MAX_PRINCIPALS_PER_STATEMENT),
            actions: vec!["SendMessage".to_string()],
            principal: Value::Array(
                account_numbers(MAX_PRINCIPALS_PER_STATEMENT)
                    .iter()
                    .map(|account_number| json!(root_of(account_number)))
                    .collect(),
            ),
            action: json!(format!("{EXPECTED_ACTION_PREFIX}SendMessage")),
        },
    ];

    let mut granted: Vec<String> = Vec::new();
    for row in &rows {
        let label = row.label.clone();
        let actions: Vec<&str> = row.actions.iter().map(String::as_str).collect();
        let accounts: Vec<&str> = row.accounts.iter().map(String::as_str).collect();
        let (status, parsed) = grant(&router, &url, &label, &accounts, &actions).await;
        assert_eq!(
            status, 200,
            "case {}: expected HTTP 200, got {status} with {parsed}",
            row.case
        );
        granted.push(label.clone());

        let policy = policy_of(&router, &url)
            .await
            .unwrap_or_else(|| panic!("case {}: the queue reports no Policy", row.case));
        assert_eq!(
            policy.get("Version").and_then(Value::as_str),
            Some(EXPECTED_POLICY_VERSION),
            "case {}: expected the policy language version, got {policy}",
            row.case
        );
        assert_eq!(
            policy.get("Id").and_then(Value::as_str),
            Some(format!("{arn}{EXPECTED_POLICY_ID_SUFFIX}").as_str()),
            "case {}: expected the generated policy id, got {policy}",
            row.case
        );
        // Every grant so far is still there, in the order it was granted: a grant adds a
        // statement and displaces none.
        assert_eq!(
            labels(&policy),
            granted,
            "case {}: expected every label granted so far, got {policy}",
            row.case
        );

        let statement = statement_for(&policy, &label);
        assert_eq!(
            statement.get("Effect").and_then(Value::as_str),
            Some("Allow"),
            "case {}: expected an allowing statement, got {statement}",
            row.case
        );
        assert_eq!(
            statement.get("Resource").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {}: expected the queue's own ARN as the resource, got {statement}",
            row.case
        );
        assert_eq!(
            statement.get("Principal").and_then(|principal| principal.get("AWS")),
            Some(&row.principal),
            "case {}: expected {} as the principal, got {statement}",
            row.case,
            row.principal
        );
        assert_eq!(
            statement.get("Action"),
            Some(&row.action),
            "case {}: expected {} as the allowed action, got {statement}",
            row.case,
            row.action
        );
    }
}

/// Revoking a label takes out exactly the statement it identifies; the queue keeps every
/// other permission, and a queue whose last permission was revoked reports no policy at
/// all — exactly as one that was never granted a permission does.
#[tokio::test]
async fn revoking_a_label_removes_only_that_statement() {
    let router = emulator();
    let url = create_ok(&router, "revoking", &json!({})).await;

    let all: Vec<&str> = vec!["first", "second", "third"];
    for label in &all {
        let (status, parsed) = grant(&router, &url, label, &[OTHER_ACCOUNT], &["SendMessage"]).await;
        assert_eq!(
            status, 200,
            "case granting {label}: expected HTTP 200, got {status} with {parsed}"
        );
    }

    // Each row revokes one of what is left and names what must remain, so the revoked
    // statement is a different one each time — the first, a middle one, and the last.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("second", vec!["first", "third"]),
        ("first", vec!["third"]),
        ("third", vec![]),
    ];
    for (revoked, remaining) in &rows {
        let _ = ok(
            &router,
            "RemovePermission",
            json!({"QueueUrl": url, "Label": revoked}),
        )
        .await;
        match policy_of(&router, &url).await {
            Some(policy) => assert_eq!(
                labels(&policy),
                *remaining,
                "case revoking {revoked}: expected {remaining:?} to remain, got {policy}"
            ),
            None => assert!(
                remaining.is_empty(),
                "case revoking {revoked}: expected {remaining:?} to remain, got no Policy at all"
            ),
        }
    }

    // The same label cannot be revoked twice: with the statement gone there is no
    // permission to revoke.
    let (status, parsed) = call(
        &router,
        "RemovePermission",
        &json!({"QueueUrl": url, "Label": "third"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidParameterValue"),
        "case revoking a label twice: expected a 400 InvalidParameterValue, got {status} with {parsed}"
    );
}

/// A grant adds a statement to a policy a caller uploaded, and the labels it adds can be
/// revoked out of it again while the uploaded statement stays.
#[tokio::test]
async fn a_grant_adds_to_an_uploaded_policy_rather_than_replacing_it() {
    let router = emulator();
    let url = create_ok(&router, "uploaded-policy", &json!({})).await;
    let arn = expected_arn("uploaded-policy");

    let uploaded = json!({
        "Version": UPLOADED_POLICY_VERSION,
        "Id": format!("{arn}/uploaded"),
        "Statement": [{
            "Sid": "uploaded",
            "Effect": "Allow",
            "Principal": {"AWS": root_of(OTHER_ACCOUNT)},
            "Action": format!("{EXPECTED_ACTION_PREFIX}SendMessage"),
            "Resource": arn,
        }],
    });
    let _ = ok(
        &router,
        "SetQueueAttributes",
        json!({"QueueUrl": url, "Attributes": {"Policy": uploaded.to_string()}}),
    )
    .await;

    let (status, parsed) = grant(
        &router,
        &url,
        "granted",
        &[SECOND_ACCOUNT],
        &["ReceiveMessage"],
    )
    .await;
    assert_eq!(
        status, 200,
        "case granting onto an uploaded policy: expected HTTP 200, got {status} with {parsed}"
    );
    let policy = policy_of(&router, &url)
        .await
        .expect("case granting onto an uploaded policy: the queue reports no Policy");
    assert_eq!(
        labels(&policy),
        vec!["uploaded".to_string(), "granted".to_string()],
        "case granting onto an uploaded policy: expected both statements, got {policy}"
    );
    assert_eq!(
        policy.get("Id").and_then(Value::as_str),
        Some(format!("{arn}/uploaded").as_str()),
        "case granting onto an uploaded policy: expected the uploaded id to stay, got {policy}"
    );

    let _ = ok(
        &router,
        "RemovePermission",
        json!({"QueueUrl": url, "Label": "granted"}),
    )
    .await;
    let policy = policy_of(&router, &url)
        .await
        .expect("case revoking the granted label: the queue reports no Policy");
    assert_eq!(
        labels(&policy),
        vec!["uploaded".to_string()],
        "case revoking the granted label: expected the uploaded statement to stay, got {policy}"
    );
}

/// A request outside what the parameters document is refused under the code the service
/// documents for that fault, and changes nothing: the queue's policy is the same
/// afterwards as before.
#[tokio::test]
async fn a_request_outside_the_documented_rules_is_refused_and_persists_nothing() {
    let router = emulator();
    let url = create_ok(&router, "refusing", &json!({})).await;
    let (status, parsed) = grant(&router, &url, "granted", &[OTHER_ACCOUNT], &["SendMessage"]).await;
    assert_eq!(
        status, 200,
        "case the grant every refusal is judged against: expected HTTP 200, got {status} with {parsed}"
    );
    let before = policy_of(&router, &url).await;

    let too_long_label = "l".repeat(MAX_PERMISSION_LABEL_LENGTH + 1);
    let past_the_action_bound: Vec<&str> = policy_action_names()
        .iter()
        .take(MAX_ACTIONS_PER_STATEMENT + 1)
        .copied()
        .collect();
    // Each row: the case, the request, the status and the error code it must answer.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a label the policy already grants",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "granted",
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a label holding a character a label may not hold",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "not a label!",
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a label one character past the length bound",
            "AddPermission",
            json!({"QueueUrl": url, "Label": too_long_label,
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a principal that is not an account number",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "non-account",
                   "AWSAccountIds": ["an-iam-user"], "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a principal one digit short of an account number",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "short-account",
                   "AWSAccountIds": ["12345678901"], "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a principal one digit past an account number",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "long-account",
                   "AWSAccountIds": ["1234567890123"], "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "one bad principal among good ones",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "mixed-principals",
                   "AWSAccountIds": [OTHER_ACCOUNT, "not-an-account"],
                   "Actions": ["SendMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "an action this service does not have",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "no-such-action",
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": ["FlyMessage"]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "an action named as a policy spells it rather than as the model does",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "prefixed-action",
                   "AWSAccountIds": [OTHER_ACCOUNT],
                   "Actions": [format!("{EXPECTED_ACTION_PREFIX}SendMessage")]}),
            400,
            "InvalidParameterValue",
        ),
        (
            "one action past what a statement may allow",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "too-many-actions",
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": past_the_action_bound}),
            400,
            "InvalidParameterValue",
        ),
        (
            "one principal past what a statement may name",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "too-many-principals",
                   "AWSAccountIds": account_numbers(MAX_PRINCIPALS_PER_STATEMENT + 1),
                   "Actions": ["SendMessage"]}),
            403,
            "OverLimit",
        ),
        (
            "no principal at all",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "no-principal",
                   "AWSAccountIds": [], "Actions": ["SendMessage"]}),
            400,
            "MissingParameter",
        ),
        (
            "no action at all",
            "AddPermission",
            json!({"QueueUrl": url, "Label": "no-action",
                   "AWSAccountIds": [OTHER_ACCOUNT], "Actions": []}),
            400,
            "MissingParameter",
        ),
        (
            "a label the policy does not grant",
            "RemovePermission",
            json!({"QueueUrl": url, "Label": "never-granted"}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a label that could never have been granted",
            "RemovePermission",
            json!({"QueueUrl": url, "Label": "not a label!"}),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in &rows {
        let (status, parsed) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (*expected_status, *expected_code),
            "case {case}: expected a {expected_status} {expected_code}, got {status} with {parsed}"
        );
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(&too_long_label),
            "case {case}: a refusal must not echo an over-long value back, got {message:?}"
        );
        assert_eq!(
            policy_of(&router, &url).await,
            before,
            "case {case}: a refused request must leave the policy as it was"
        );
    }
}

/// A queue carries at most as many permissions as a policy may hold statements: the
/// grant that would take it past that is refused as `OverLimit`, and the one that takes
/// it exactly to the bound is served.
#[tokio::test]
async fn a_grant_past_the_permission_bound_is_refused_as_over_limit() {
    let router = emulator();
    let url = create_ok(&router, "bounded-policy", &json!({})).await;

    for index in 0..MAX_POLICY_STATEMENTS {
        let label = format!("permission-{index}");
        let (status, parsed) = grant(&router, &url, &label, &[OTHER_ACCOUNT], &["SendMessage"]).await;
        assert_eq!(
            status, 200,
            "case granting permission {index} of {MAX_POLICY_STATEMENTS}: expected HTTP 200, \
             got {status} with {parsed}"
        );
    }
    let policy = policy_of(&router, &url)
        .await
        .expect("case a queue at the permission bound: the queue reports no Policy");
    assert_eq!(
        labels(&policy).len(),
        MAX_POLICY_STATEMENTS,
        "case a queue at the permission bound: expected {MAX_POLICY_STATEMENTS} statements, \
         got {policy}"
    );

    let (status, parsed) = grant(
        &router,
        &url,
        "one-past-the-bound",
        &[OTHER_ACCOUNT],
        &["SendMessage"],
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (403, "OverLimit"),
        "case one permission past the bound: expected a 403 OverLimit, got {status} with {parsed}"
    );
    assert_eq!(
        policy_of(&router, &url).await.map(|policy| labels(&policy).len()),
        Some(MAX_POLICY_STATEMENTS),
        "case one permission past the bound: the refused grant must persist nothing"
    );

    // Revoking one makes room for exactly one more, so the bound is on what the queue
    // holds and not on how many grants it has ever been sent.
    let _ = ok(
        &router,
        "RemovePermission",
        json!({"QueueUrl": url, "Label": "permission-0"}),
    )
    .await;
    let (status, parsed) = grant(
        &router,
        &url,
        "after-a-revoke",
        &[OTHER_ACCOUNT],
        &["SendMessage"],
    )
    .await;
    assert_eq!(
        status, 200,
        "case granting after a revoke made room: expected HTTP 200, got {status} with {parsed}"
    );
}

/// The policy a queue carries is bounded in BYTES as well as in statements: grants that
/// would grow the document past the size a queue policy may have are refused as
/// `OverLimit` before the statement bound is anywhere near reached, and the queue keeps
/// the policy it had.
#[tokio::test]
async fn a_grant_past_the_policy_size_bound_is_refused_as_over_limit() {
    let router = emulator();
    let url = create_ok(&router, "large-policy", &json!({})).await;

    // The widest statement a grant may add: as many principals as one may name and as
    // many actions as one may allow, so a handful of them fills the document.
    let principals = account_numbers(MAX_PRINCIPALS_PER_STATEMENT);
    let principals: Vec<&str> = principals.iter().map(String::as_str).collect();
    let actions: Vec<&str> = policy_action_names()
        .iter()
        .take(MAX_ACTIONS_PER_STATEMENT)
        .copied()
        .collect();

    let mut refused_at: Option<usize> = None;
    for index in 0..MAX_POLICY_STATEMENTS {
        let before = policy_of(&router, &url).await;
        let label = format!("wide-{index}");
        let (status, parsed) = grant(&router, &url, &label, &principals, &actions).await;
        if status == 200 {
            continue;
        }
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (403, "OverLimit"),
            "case the grant that outgrows the policy: expected a 403 OverLimit, \
             got {status} with {parsed}"
        );
        assert_eq!(
            policy_of(&router, &url).await,
            before,
            "case the grant that outgrows the policy: the refused grant must persist nothing"
        );
        refused_at = Some(index);
        break;
    }
    let refused_at = refused_at.unwrap_or_else(|| {
        panic!(
            "case the grant that outgrows the policy: {MAX_POLICY_STATEMENTS} widest grants were \
             all accepted, so the size bound was never reached"
        )
    });
    // The first widest grant fits — the bound is on the document, not on the statement —
    // and the policy carries exactly the grants that did fit.
    assert!(
        (1..MAX_POLICY_STATEMENTS).contains(&refused_at),
        "case the grant that outgrows the policy: expected the first widest grant to fit and \
         the size bound to bite before the statement bound, got a refusal at {refused_at}"
    );
    assert_eq!(
        policy_of(&router, &url)
            .await
            .map(|policy| labels(&policy).len()),
        Some(refused_at),
        "case the grant that outgrows the policy: the policy carries exactly the \
         {refused_at} grant(s) that fitted"
    );
}

/// The account the emulator presents is the one a queue ARN carries, so a grant to it
/// names that same account as a principal — the grant is recorded for whatever account
/// number the request supplies, this emulator's included.
#[tokio::test]
async fn a_grant_to_this_account_names_this_account_as_the_principal() {
    let router = emulator();
    let url = create_ok(&router, "self-granting", &json!({})).await;

    let (status, parsed) = grant(&router, &url, "to-myself", &[account()], &["SendMessage"]).await;
    assert_eq!(
        status, 200,
        "case granting to this account: expected HTTP 200, got {status} with {parsed}"
    );
    let policy = policy_of(&router, &url)
        .await
        .expect("case granting to this account: the queue reports no Policy");
    let statement = statement_for(&policy, "to-myself");
    assert_eq!(
        statement
            .get("Principal")
            .and_then(|principal| principal.get("AWS"))
            .and_then(Value::as_str),
        Some(root_of(account()).as_str()),
        "case granting to this account: expected this account's principal, got {statement}"
    );
}

/// The sentence AWS answers an action name no action of this service answers to, spelled
/// as AWS publishes it: the caller is pointed at the service's interface description, and
/// the name is given back QUALIFIED with the service it was looked up in — the form the
/// statement a grant would have added names an action in.
fn unknown_action_message(qualified_action: &str) -> String {
    format!(
        "Value {qualified_action} for parameter ActionName is invalid. \
         Reason: Please refer to the appropriate WSDL for a list of valid actions."
    )
}

/// An action outside the set this service publishes is refused as an unknown action,
/// whatever KIND of name it is, while every name the set does carry is granted.
#[tokio::test]
async fn an_action_this_service_does_not_have_is_refused_as_an_unknown_action() {
    let router = emulator();
    let url = create_ok(&router, "action-names", &json!({})).await;

    // The kinds of name that are not an action of this service: a name of nothing, an
    // action of ANOTHER service, an action of this one in the wrong case, one already
    // qualified with the service, and a near-miss of a name the set carries.
    let refused: &[(&str, &str)] = &[
        ("a name no service has", "NotARealSqsAction"),
        ("an action of another service", "GetObject"),
        ("an action of this service in the wrong case", "sendmessage"),
        (
            "an action already qualified with the service",
            "SQS:SendMessage",
        ),
        ("a near-miss of an action this service has", "SendMessages"),
    ];

    for (index, (case, action)) in refused.iter().enumerate() {
        let label = format!("refused-{index}");
        let (status, parsed) = grant(&router, &url, &label, &[OTHER_ACCOUNT], &[action]).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValue"),
            "case {case}: expected a 400 InvalidParameterValue, got {status} with {parsed}"
        );
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected = unknown_action_message(&format!("{EXPECTED_ACTION_PREFIX}{action}"));
        assert_eq!(
            message, expected,
            "case {case}: expected the message {expected:?}, got {message:?}"
        );
        assert!(
            policy_of(&router, &url).await.is_none(),
            "case {case}: a refused grant must leave the queue carrying no policy"
        );
    }

    // One bad name among good ones refuses the whole grant, and names the bad one.
    let (status, parsed) = grant(
        &router,
        &url,
        "mixed-actions",
        &[OTHER_ACCOUNT],
        &["SendMessage", "NotARealSqsAction", "ReceiveMessage"],
    )
    .await;
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expected = unknown_action_message(&format!("{EXPECTED_ACTION_PREFIX}NotARealSqsAction"));
    assert_eq!(
        (status, error_code(&parsed).as_str(), message),
        (400, "InvalidParameterValue", expected.as_str()),
        "case one bad action among good ones: expected a 400 InvalidParameterValue naming the \
         bad action, got {status} with {parsed}"
    );

    // Names the published set DOES carry are granted — entries the refusals above are
    // near-misses of, plus the wildcard, read from the catalog in the service's state.
    let accepted: Vec<&str> = policy_action_names()
        .iter()
        .copied()
        .filter(|action| ["ChangeMessageVisibility", "ReceiveMessage", "SendMessage"].contains(action))
        .chain(std::iter::once(EXPECTED_ANY_ACTION))
        .collect();
    for (index, action) in accepted.iter().enumerate() {
        let label = format!("accepted-{index}");
        let (status, parsed) = grant(&router, &url, &label, &[OTHER_ACCOUNT], &[action]).await;
        assert_eq!(
            status, 200,
            "case granting {action}: expected HTTP 200, got {status} with {parsed}"
        );
    }
    let policy = policy_of(&router, &url)
        .await
        .expect("case granting every published action: the queue reports no Policy");
    assert_eq!(
        labels(&policy).len(),
        accepted.len(),
        "case granting every published action: expected one statement per grant, got {policy}"
    );
}
