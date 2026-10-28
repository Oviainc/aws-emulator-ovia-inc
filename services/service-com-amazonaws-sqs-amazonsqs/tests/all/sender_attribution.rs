// SPDX-License-Identifier: CC0-1.0

//! Who a message is FROM: the `SenderId` system attribute a receive reports, driven over
//! the wire through the IAM/STS/SQS emulator in [`super::queue_policy_authorization`], so
//! every principal here is made with IAM's and STS's own operations and every Amazon SQS
//! call presents that principal's credential — the path a client takes.
//!
//! The rule under test: a send records the principal it spoke for, and `ReceiveMessage`
//! reports it under `SenderId` — "for a user, returns the user ID ... for an IAM role,
//! returns the IAM role ID" — which is the id STS reports for that same credential under
//! `GetCallerIdentity`'s `UserId`. Being a property of the SENDER and not of the read, it
//! is identical for two sends by one principal, differs between principals, is reported
//! whether the request names the attribute or asks for `All`, and does not change when the
//! message is received again.
//!
//! The table spans what the rule generalises over: the kind of principal (the account's own
//! credential, an IAM user, an assumed-role session), where its permission to send comes
//! from, the operation that sent the message (`SendMessage` and one entry of a
//! `SendMessageBatch`), and both ways a receive selects the attribute.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::queue_policy_authorization::{policy, Emulator};
use super::support::as_map;

/// The access key id that names no IAM user and no STS session: the account's own published
/// credential, which is the position AWS account root credentials are in.
const OWNER_KEY: &str = "AKIDEXAMPLEOWNER";

/// The system attribute under test.
const SENDER_ID: &str = "SenderId";

/// An identity-based policy letting its principal act on every queue of this account.
fn may_use_queues() -> Value {
    policy(json!([{"Effect": "Allow", "Action": "sqs:*", "Resource": "*"}]))
}

/// Receive one message from the queue at `url` as the queue's owner, selecting the system
/// attributes `names` asks for, and answer its `Attributes` map.
async fn received_attributes(
    emulator: &Emulator,
    case: &str,
    url: &str,
    names: &[&str],
) -> BTreeMap<String, String> {
    let reply = emulator
        .sqs_ok(
            case,
            OWNER_KEY,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 1,
                "VisibilityTimeout": 0,
                "MessageSystemAttributeNames": names,
            }),
        )
        .await;
    let delivered = reply
        .get("Messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        delivered.len(),
        1,
        "case {case}: expected one message back, got {reply}"
    );
    as_map(&delivered[0]["Attributes"])
}

/// Every send records the principal it spoke for, and a receive reports it under
/// `SenderId` — the same id STS reports for that credential.
#[tokio::test]
async fn a_message_carries_the_principal_id_of_the_send_that_arrived() {
    let emulator = Emulator::new();
    // Three kinds of principal, each with a credential of its own. The user carries an
    // identity-based grant; the role's session is granted by the queue's own policy
    // below, so neither path to a permission is the only one covered.
    let (_, user_key) = emulator
        .user("an IAM user", "sender", Some(&may_use_queues()), &[])
        .await;
    let (role_arn, session_key) = emulator
        .assumed_role("an assumed-role session", "sender-role", &[])
        .await;
    // (case, the queue name stem, the credential the send presents, the principal the
    // queue's policy must grant, if any)
    let senders: [(&str, &str, &str, Option<&str>); 3] = [
        ("the account's own credential", "owner", OWNER_KEY, None),
        ("an IAM user", "user", user_key.as_str(), None),
        (
            "an assumed-role session",
            "session",
            session_key.as_str(),
            Some(role_arn.as_str()),
        ),
    ];
    // Both ways a receive selects the attribute: by name, and through `All`.
    let selections: [(&str, &[&str]); 2] = [("by-name", &[SENDER_ID]), ("through-all", &["All"])];

    for (sender, stem, key, granted) in senders {
        // The id STS reports for this very credential, which is what the attribute has to
        // carry. Read from STS rather than spelled here, so nothing in this suite depends
        // on how an id is derived.
        let expected = emulator.caller_user_id(sender, key).await;
        assert!(
            !expected.is_empty(),
            "case {sender}: STS must report a UserId for the credential under test"
        );

        for (selection, names) in selections {
            let case = format!("{sender}, {selection}");
            let url = emulator.queue(&case, &format!("sender-{stem}-{selection}")).await;
            if let Some(principal) = granted {
                emulator
                    .set_policy(
                        &case,
                        &url,
                        &policy(json!([{
                            "Effect": "Allow",
                            "Principal": {"AWS": principal},
                            "Action": "sqs:*",
                            "Resource": "*",
                        }])),
                    )
                    .await;
            }

            emulator
                .sqs_ok(
                    &case,
                    key,
                    "SendMessage",
                    json!({"QueueUrl": url, "MessageBody": "one"}),
                )
                .await;
            let reported = received_attributes(&emulator, &case, &url, names).await;
            assert_eq!(
                reported.get(SENDER_ID).map(String::as_str),
                Some(expected.as_str()),
                "case {case}: expected the sender's own principal id, got {reported:?}"
            );

            // Re-receiving reports the same value: it is the SENDER's, not the reader's.
            let again = received_attributes(&emulator, &case, &url, names).await;
            assert_eq!(
                again.get(SENDER_ID),
                reported.get(SENDER_ID),
                "case {case}: a second receive must report the same sender"
            );
        }
    }
}

/// A batch send records the same principal for every entry it enqueues, and two sends by
/// one principal report the identical value while two principals' differ.
#[tokio::test]
async fn one_principals_sends_agree_and_two_principals_differ() {
    let emulator = Emulator::new();
    let (_, first_key) = emulator
        .user("the first user", "sender-one", Some(&may_use_queues()), &[])
        .await;
    let (_, second_key) = emulator
        .user("the second user", "sender-two", Some(&may_use_queues()), &[])
        .await;
    let url = emulator.queue("the shared queue", "sender-comparison").await;

    // The first principal sends a batch of three, the second sends one message.
    emulator
        .sqs_ok(
            "the first user",
            &first_key,
            "SendMessageBatch",
            json!({
                "QueueUrl": url,
                "Entries": [
                    {"Id": "a", "MessageBody": "a"},
                    {"Id": "b", "MessageBody": "b"},
                    {"Id": "c", "MessageBody": "c"},
                ],
            }),
        )
        .await;
    emulator
        .sqs_ok(
            "the second user",
            &second_key,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": "d"}),
        )
        .await;

    let first_id = emulator.caller_user_id("the first user", &first_key).await;
    let second_id = emulator.caller_user_id("the second user", &second_key).await;
    assert_ne!(
        first_id, second_id,
        "case two principals: their ids must differ for this suite to say anything"
    );

    let reply = emulator
        .sqs_ok(
            "the shared queue",
            OWNER_KEY,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "MessageSystemAttributeNames": ["All"],
            }),
        )
        .await;
    let delivered = reply
        .get("Messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        delivered.len(),
        4,
        "case the shared queue: expected all four messages, got {reply}"
    );
    // (body, the sender reported for it)
    let reported: BTreeMap<String, String> = delivered
        .iter()
        .map(|message| {
            (
                message["Body"].as_str().unwrap_or_default().to_string(),
                as_map(&message["Attributes"])
                    .get(SENDER_ID)
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .collect();
    let expected: BTreeMap<String, String> = [
        ("a", first_id.clone()),
        ("b", first_id.clone()),
        ("c", first_id.clone()),
        ("d", second_id.clone()),
    ]
    .into_iter()
    .map(|(body, sender)| (body.to_string(), sender))
    .collect();
    assert_eq!(
        reported, expected,
        "case the shared queue: every entry of one batch carries that sender, and the \
         other principal's message carries its own"
    );
}
