// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for THE DOCUMENT `AddPermission` SYNTHESISES, driven over the wire
//! (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: **a permission added to a queue that carries no policy is served by
//! the default queue policy the service SYNTHESISES, whose fixed parts are its own — the
//! policy language version `2008-10-17` and the `<queue arn>/SQSDefaultPolicy` id — while
//! its variable parts come from the request and the queue's identity; a policy a caller
//! UPLOADED is kept as written, version included.** `AddPermission` "generates a policy for
//! you", so the generated document's fixed text is the service's and not the caller's: a
//! client that round-trips the document, compares it against the one AWS emits or feeds it
//! to a strict policy parser depends on those fixed parts being what the service really
//! writes.
//!
//! The rows span what the rule generalises over: the number of principals and of actions a
//! grant names, the action that stands for every action, the kind of queue the policy is
//! generated for (standard, FIFO), and how many grants the document has accumulated — plus
//! the row where the queue already carries an uploaded policy, which is what keeps the rule
//! about the SYNTHESISED document rather than about every policy a queue can report.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, create_ok, emulator, expected_arn, fifo_attributes, ok, OTHER_ACCOUNT,
};

// The wire values Amazon SQS fixes for its generated queue policy are spelled HERE, as the
// service emits them, rather than read back off the implementation's own constants: an
// expectation taken from the code it checks would pass whatever that code said.

/// The policy language version the GENERATED queue policy declares — the long-standing
/// default policy language version Amazon SQS synthesises `SQSDefaultPolicy` in.
const GENERATED_POLICY_VERSION: &str = "2008-10-17";

/// What the `Id` of the generated queue policy is: the queue's ARN followed by this.
const GENERATED_POLICY_ID_SUFFIX: &str = "/SQSDefaultPolicy";

/// The policy language version a caller writing a policy today uses, which the rows that
/// UPLOAD one write and the queue must keep.
const UPLOADED_POLICY_VERSION: &str = "2012-10-17";

/// A second 12-digit account number, for the rows granting to more than one principal.
const SECOND_ACCOUNT: &str = "210987654321";

/// The `Policy` the queue at `url` reports, parsed — `None` when it reports none at all.
async fn policy_of(router: &MultiServiceRouter, url: &str) -> Option<Value> {
    let policy = all_attributes(router, url).await.get("Policy")?.clone();
    Some(serde_json::from_str(&policy).unwrap_or_else(|error| {
        panic!("case the policy of {url}: expected a JSON document, got {policy:?} ({error})")
    }))
}

/// Grant one permission on the queue at `url`, which must be served.
async fn grant(
    router: &MultiServiceRouter,
    url: &str,
    label: &str,
    accounts: &[&str],
    actions: &[&str],
) {
    let _ = ok(
        router,
        "AddPermission",
        json!({
            "QueueUrl": url,
            "Label": label,
            "AWSAccountIds": accounts,
            "Actions": actions,
        }),
    )
    .await;
}

/// One row of the generated-policy table.
struct GrantRow {
    case: &'static str,
    /// Whether the queue the grant is made on is a FIFO queue.
    fifo: bool,
    /// The labels granted, in order, each with the principals and actions it names.
    grants: Vec<(&'static str, Vec<&'static str>, Vec<&'static str>)>,
}

/// Every grant onto a queue that carries no policy is served by the synthesised document,
/// whose version and id are the service's own fixed text — whatever the grant names and
/// however many grants the document has accumulated.
#[tokio::test]
async fn the_generated_queue_policy_declares_the_default_policy_language_version() {
    let rows = [
        GrantRow {
            case: "one principal, one action",
            fifo: false,
            grants: vec![("one-to-one", vec![OTHER_ACCOUNT], vec!["SendMessage"])],
        },
        GrantRow {
            case: "two principals, two actions",
            fifo: false,
            grants: vec![(
                "two-to-two",
                vec![OTHER_ACCOUNT, SECOND_ACCOUNT],
                vec!["SendMessage", "ReceiveMessage"],
            )],
        },
        GrantRow {
            case: "the action that stands for every action",
            fifo: false,
            grants: vec![("every-action", vec![OTHER_ACCOUNT], vec!["*"])],
        },
        GrantRow {
            case: "three grants accumulated on one document",
            fifo: false,
            grants: vec![
                ("first", vec![OTHER_ACCOUNT], vec!["SendMessage"]),
                ("second", vec![SECOND_ACCOUNT], vec!["ReceiveMessage"]),
                ("third", vec![OTHER_ACCOUNT], vec!["DeleteMessage"]),
            ],
        },
        GrantRow {
            case: "a grant on a FIFO queue",
            fifo: true,
            grants: vec![("fifo-grant", vec![OTHER_ACCOUNT], vec!["SendMessage"])],
        },
    ];


    for (index, row) in rows.iter().enumerate() {
        let router = emulator();
        let suffix = if row.fifo { ".fifo" } else { "" };
        let name = format!("generated-{index}{suffix}");
        let attributes = if row.fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        let url = create_ok(&router, &name, &attributes).await;
        let arn = expected_arn(&name);

        // A queue nobody granted anything on carries no policy, so the document every row
        // asserts on is one this call synthesised.
        assert!(
            policy_of(&router, &url).await.is_none(),
            "case {}: a queue with no grant must report no Policy",
            row.case
        );

        for (label, accounts, actions) in &row.grants {
            grant(&router, &url, label, accounts, actions).await;

            let policy = policy_of(&router, &url)
                .await
                .unwrap_or_else(|| panic!("case {}: the queue reports no Policy", row.case));
            assert_eq!(
                policy.get("Version").and_then(Value::as_str),
                Some(GENERATED_POLICY_VERSION),
                "case {} after granting {label}: expected the generated policy language \
                 version, got {policy}",
                row.case
            );
            assert_eq!(
                policy.get("Id").and_then(Value::as_str),
                Some(format!("{arn}{GENERATED_POLICY_ID_SUFFIX}").as_str()),
                "case {} after granting {label}: expected the generated policy id, got \
                 {policy}",
                row.case
            );
            // The variable parts are the request's and the queue's: the statement the grant
            // added carries the label as its `Sid` and the queue's own ARN as its resource.
            let statements = policy
                .get("Statement")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let statement = statements
                .iter()
                .find(|statement| statement.get("Sid").and_then(Value::as_str) == Some(*label))
                .unwrap_or_else(|| {
                    panic!(
                        "case {} after granting {label}: no statement for the label in {policy}",
                        row.case
                    )
                });
            assert_eq!(
                statement.get("Resource").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {} after granting {label}: expected the queue's own ARN as the \
                 resource, got {statement}",
                row.case
            );
        }
    }
}

/// The rule is about the SYNTHESISED document: a policy a caller uploaded keeps the version
/// it was written with, and a grant onto it adds a statement without rewriting it.
#[tokio::test]
async fn an_uploaded_policy_keeps_the_version_it_was_written_with() {
    let router = emulator();
    let name = "uploaded-version";
    let url = create_ok(&router, name, &json!({})).await;
    let arn = expected_arn(name);

    let uploaded = json!({
        "Version": UPLOADED_POLICY_VERSION,
        "Id": format!("{arn}/uploaded"),
        "Statement": [{
            "Sid": "uploaded",
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
            "Action": "SQS:SendMessage",
            "Resource": arn,
        }],
    });
    let _ = ok(
        &router,
        "SetQueueAttributes",
        json!({"QueueUrl": url, "Attributes": {"Policy": uploaded.to_string()}}),
    )
    .await;

    grant(&router, &url, "granted", &[SECOND_ACCOUNT], &["ReceiveMessage"]).await;

    let policy = policy_of(&router, &url)
        .await
        .expect("case a grant onto an uploaded policy: the queue reports no Policy");
    assert_eq!(
        policy.get("Version").and_then(Value::as_str),
        Some(UPLOADED_POLICY_VERSION),
        "case a grant onto an uploaded policy: expected the uploaded version to stay, got \
         {policy}"
    );
    assert_eq!(
        policy.get("Id").and_then(Value::as_str),
        Some(format!("{arn}/uploaded").as_str()),
        "case a grant onto an uploaded policy: expected the uploaded id to stay, got {policy}"
    );
}
