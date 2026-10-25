// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the data-protection-policy WRITE: the document
//! `PutDataProtectionPolicy` installs on a topic, that the document a read reports is
//! the one the LAST write installed, the one documented length bound the write is held
//! to, and the two refusals its `ResourceArn` is held to.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls, and every
//! installed document is read back through `GetDataProtectionPolicy`.

use super::topics_subscriptions_publish::{Answer, Sns};
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::MAX_DATA_PROTECTION_POLICY_LEN;

/// Install `policy` on the topic `arn` names, returning the answer unasserted: the
/// refusal tables below read its status and code, the success tables assert the 200.
async fn put(sns: &Sns, arn: &str, policy: &str) -> Answer {
    sns.call(
        "PutDataProtectionPolicy",
        &[("ResourceArn", arn), ("DataProtectionPolicy", policy)],
    )
    .await
}

/// Install `policy`, asserting the write was accepted.
async fn put_ok(sns: &Sns, case: &str, arn: &str, policy: &str) {
    let answer = put(sns, arn, policy).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutDataProtectionPolicy expected 200, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "",
        "case {case:?}: an accepted write carries no error code, got {}",
        answer.body
    );
}

/// The document `GetDataProtectionPolicy` reports for the topic `arn` names: the write
/// is only proved by reading it back through the implemented reader.
async fn reported(sns: &Sns, case: &str, arn: &str) -> String {
    let answer = sns
        .call("GetDataProtectionPolicy", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetDataProtectionPolicy expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer.element("DataProtectionPolicy").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the read carries no DataProtectionPolicy member: {}",
            answer.body
        )
    })
}

/// A JSON document of exactly `len` characters, each occurrence of `filler` one
/// character of the document's content: the policy a caller may install at (or, with
/// one more, past) the documented ceiling.
fn document_of_length(len: usize, filler: char) -> String {
    let overhead = "{\"a\":\"\"}".chars().count();
    assert!(
        len >= overhead,
        "a json document holds at least {overhead} characters, not {len}"
    );
    let mut document = String::from("{\"a\":\"");
    document.extend(std::iter::repeat_n(filler, len - overhead));
    document.push_str("\"}");
    document
}

/// The document the write installs is the document the topic holds, byte for byte,
/// whatever that document is. The rows differ in the form, the content and the size of
/// the document — what "the document the request carries" generalises over.
#[tokio::test]
async fn the_installed_document_is_the_one_the_request_carried() {
    let sns = Sns::new();
    let at_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN, 'p');
    let cases: &[(&str, &str, &str)] = &[
        ("an empty json object", "put-dpp-empty-object", "{}"),
        (
            "a statement document",
            "put-dpp-statement",
            "{\"Name\":\"policy\",\"Version\":\"2021-06-01\",\"Statement\":[{\"Sid\":\"audit\",\
             \"DataDirection\":\"Inbound\",\"Principal\":[\"*\"],\
             \"DataIdentifier\":[\"arn:aws:dataprotection::aws:data-identifier/EmailAddress\"],\
             \"Operation\":{\"Audit\":{\"SampleRate\":\"99\"}}}]}",
        ),
        (
            "non-ascii content",
            "put-dpp-non-ascii",
            "{\"Name\":\"Grüße 😀 — déjà vu\",\"Statement\":[]}",
        ),
        (
            "content carrying the characters the xml envelope escapes",
            "put-dpp-xml-metacharacters",
            "{\"Name\":\"a & b <c> \\\"d\\\" 'e'\"}",
        ),
        ("a single space", "put-dpp-whitespace-only", " "),
        ("the empty document", "put-dpp-empty", ""),
        (
            "at the documented 30720 character bound",
            "put-dpp-at-the-bound",
            at_the_bound.as_str(),
        ),
    ];

    for (case, name, policy) in cases {
        let arn = sns.topic(name).await;
        put_ok(&sns, case, &arn, policy).await;
        let actual = reported(&sns, case, &arn).await;
        assert_eq!(
            &actual, policy,
            "case {case:?}: expected the document the write carried ({} characters), got {} characters: {actual:?}",
            policy.chars().count(),
            actual.chars().count()
        );
    }
}

/// The write ADDS a document to a topic holding none and UPDATES one that holds a
/// document ("Adds or updates an inline policy document that is stored in the
/// specified Amazon SNS topic"), so the document a read reports is always the one the
/// LAST write installed. The rows differ in how the topic's first document arrived and
/// in what the writes that follow install, including a write that empties the
/// document and a repeated (idempotent) write.
#[tokio::test]
async fn the_reported_document_follows_the_last_write() {
    let sns = Sns::new();
    // (case, topic name, the document the create carries, the documents written after)
    let cases: &[(&str, &str, Option<&str>, &[&str])] = &[
        (
            "a topic that held no document",
            "put-dpp-added",
            None,
            &["{\"Name\":\"first\"}"],
        ),
        (
            "a document the create installed, then updated",
            "put-dpp-updated",
            Some("{\"Name\":\"created-with\"}"),
            &["{\"Name\":\"put-over-it\",\"Statement\":[]}"],
        ),
        (
            "three writes in a row",
            "put-dpp-thrice",
            None,
            &["{}", "{\"Name\":\"second\"}", "{\"Name\":\"third\",\"Statement\":[]}"],
        ),
        (
            "a document emptied by a later write",
            "put-dpp-emptied",
            Some("{\"Name\":\"only\"}"),
            &[""],
        ),
        (
            "the same document written twice",
            "put-dpp-repeated",
            None,
            &["{\"Name\":\"same\"}", "{\"Name\":\"same\"}"],
        ),
    ];

    for (case, name, created_with, writes) in cases {
        let mut params: Vec<(&str, &str)> = vec![("Name", name)];
        if let Some(policy) = created_with {
            params.push(("DataProtectionPolicy", policy));
        }
        let created = sns.call("CreateTopic", &params).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTopic expected 200, got {} / {}",
            created.status, created.body
        );
        let arn = created
            .element("TopicArn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateTopic answered no TopicArn: {}", created.body));

        let before = reported(&sns, case, &arn).await;
        assert_eq!(
            before.as_str(),
            created_with.unwrap_or(""),
            "case {case:?}: expected the document the create carried, got {before:?}"
        );

        for policy in writes.iter() {
            put_ok(&sns, case, &arn, policy).await;
            let actual = reported(&sns, case, &arn).await;
            assert_eq!(
                &actual, policy,
                "case {case:?}: expected the document the last write installed, got {actual:?}"
            );
        }
    }
}

/// A write names ONE topic: the document it installs is held by that topic alone, and
/// every other record is left exactly as it was.
#[tokio::test]
async fn a_write_touches_only_the_topic_it_names() {
    let sns = Sns::new();
    let case = "one topic of three";
    let target = sns.topic("put-dpp-target").await;
    let bystander = sns.topic("put-dpp-bystander").await;
    let created = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "put-dpp-bystander-with-policy"),
                ("DataProtectionPolicy", "{\"Name\":\"its-own\"}"),
            ],
        )
        .await;
    let bystander_with_policy = created
        .element("TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateTopic answered no TopicArn: {}", created.body));

    put_ok(&sns, case, &target, "{\"Name\":\"the-target\"}").await;

    for (held, expected) in [
        (&target, "{\"Name\":\"the-target\"}"),
        (&bystander, ""),
        (&bystander_with_policy, "{\"Name\":\"its-own\"}"),
    ] {
        let actual = reported(&sns, case, held).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: topic {held:?} must report its own document, got {actual:?}"
        );
    }

    // The record itself, not only the read: the write installed a document and changed
    // nothing else about the topic it named.
    let record = sns
        .state()
        .lock()
        .topics
        .get(&target)
        .map(|topic| (topic.name.clone(), topic.data_protection_policy.clone()));
    assert_eq!(
        record,
        Some((
            "put-dpp-target".to_string(),
            Some("{\"Name\":\"the-target\"}".to_string())
        )),
        "case {case:?}: the record must hold the installed document under its own name, got {record:?}"
    );
}

/// The one bound AWS documents for the document — "Maximum length of 30,720"
/// (`PutDataProtectionPolicyInput$DataProtectionPolicy` doc) — is enforced at the
/// published bound, in CHARACTERS: a document at the bound is installed and one
/// character past it is refused with 400 `InvalidParameter`, whether its characters
/// are one byte each or several. A refused write installs nothing.
#[tokio::test]
async fn a_document_past_the_documented_length_is_refused() {
    let sns = Sns::new();
    let held = "{\"Name\":\"held\"}";
    // (case, the document, accepted)
    let ascii_at_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN, 'p');
    let ascii_past_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN + 1, 'p');
    // 'é' is two bytes in UTF-8 and one character, so a document of these is past the
    // bound in bytes long before it is past the bound in characters.
    let multibyte_at_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN, 'é');
    let multibyte_past_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN + 1, 'é');
    let cases: &[(&str, &str, bool)] = &[
        ("one character", "x", true),
        ("at the bound", ascii_at_the_bound.as_str(), true),
        ("one character past the bound", ascii_past_the_bound.as_str(), false),
        (
            "at the bound in multi-byte characters",
            multibyte_at_the_bound.as_str(),
            true,
        ),
        (
            "one multi-byte character past the bound",
            multibyte_past_the_bound.as_str(),
            false,
        ),
    ];

    for (case, policy, accepted) in cases {
        // A fresh topic per row, carrying a document the refused write must not disturb.
        let arn = sns.topic(&format!("put-dpp-bound-{}", policy.chars().count())).await;
        put_ok(&sns, case, &arn, held).await;

        let answer = put(&sns, &arn, policy).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let actual = reported(&sns, case, &arn).await;
            assert_eq!(
                &actual, policy,
                "case {case:?}: expected the installed document, got {} characters",
                actual.chars().count()
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            let actual = reported(&sns, case, &arn).await;
            assert_eq!(
                actual, held,
                "case {case:?}: a refused write must install nothing, got {actual:?}"
            );
        }
    }
}

/// A `ResourceArn` that is not an ARN at all is bad input (400 `InvalidParameter`),
/// and an ARN this account holds no topic under is a lookup miss (404 `NotFound`) —
/// the same two rules the READ holds its own `ResourceArn` to. The rows differ in HOW
/// the value fails: its shape, or the resource it names. No refused write installs
/// anything.
#[tokio::test]
async fn an_arn_that_names_no_topic_is_refused() {
    let sns = Sns::new();
    let present = sns.topic("put-dpp-present").await;
    put_ok(&sns, "setup", &present, "{\"Name\":\"survivor\"}").await;
    let deleted = sns.topic("put-dpp-deleted").await;
    let gone = sns.call("DeleteTopic", &[("TopicArn", &deleted)]).await;
    assert_eq!(
        gone.status, 200,
        "DeleteTopic expected 200, got {} / {}",
        gone.status, gone.body
    );
    let subscription_shaped = format!("{present}:11111111-2222-3333-4444-555555555555");

    let cases: &[(&str, &str, u16, &str)] = &[
        ("not an arn", "put-dpp-present", 400, "InvalidParameter"),
        ("too few arn elements", "arn:aws:sns", 400, "InvalidParameter"),
        ("an empty value", "", 400, "InvalidParameter"),
        (
            "an arn that does not begin with arn:",
            "urn:aws:sns:us-east-1:000000000000:put-dpp-present",
            400,
            "InvalidParameter",
        ),
        (
            "a topic no account holds",
            "arn:aws:sns:us-east-1:000000000000:put-dpp-never-created",
            404,
            "NotFound",
        ),
        (
            "a subscription of a topic that exists",
            subscription_shaped.as_str(),
            404,
            "NotFound",
        ),
        (
            "another service's resource",
            "arn:aws:sqs:us-east-1:000000000000:put-dpp-present",
            404,
            "NotFound",
        ),
        ("a deleted topic", deleted.as_str(), 404, "NotFound"),
    ];

    for (case, arn, status, code) in cases {
        let answer = put(&sns, arn, "{\"Name\":\"refused\"}").await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            &answer.code(),
            code,
            "case {case:?}: expected the {code} code, got {}",
            answer.body
        );
    }

    // The topic that does exist still holds the document the accepted write installed:
    // the refusals above wrote nothing.
    let actual = reported(&sns, "after the refusals", &present).await;
    assert_eq!(
        actual, "{\"Name\":\"survivor\"}",
        "the surviving topic must still report its own document, got {actual:?}"
    );
}
