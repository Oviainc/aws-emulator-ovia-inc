// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the data-protection-policy READ: the inline document
//! `GetDataProtectionPolicy` reports for a topic, the empty document it reports for a
//! topic that holds none, and the two refusals a `ResourceArn` is held to.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{Answer, Sns};
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::MAX_DATA_PROTECTION_POLICY_LEN;

/// Create a topic carrying `policy` as its inline data protection policy, returning
/// its ARN. `CreateTopic` is the write path a topic's first policy arrives through
/// (`CreateTopicInput$DataProtectionPolicy`).
async fn topic_with_policy(sns: &Sns, case: &str, name: &str, policy: &str) -> String {
    let answer = sns
        .call(
            "CreateTopic",
            &[("Name", name), ("DataProtectionPolicy", policy)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: CreateTopic expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateTopic answered no TopicArn: {}", answer.body))
}

/// Read one topic's data protection policy, asserting the call was answered.
async fn read(sns: &Sns, case: &str, arn: &str) -> Answer {
    let answer = sns
        .call("GetDataProtectionPolicy", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetDataProtectionPolicy expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
}

/// The document a read reported: the `DataProtectionPolicy` member of the response,
/// which a topic holding no policy carries EMPTY rather than omitted.
fn reported(case: &str, answer: &Answer) -> String {
    answer.element("DataProtectionPolicy").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the response carries no DataProtectionPolicy member: {}",
            answer.body
        )
    })
}

/// A JSON document of exactly `len` characters: the policy a caller may install at the
/// documented ceiling.
fn document_of_length(len: usize) -> String {
    let overhead = "{\"a\":\"\"}".len();
    debug_assert!(
        len >= overhead,
        "a json document holds at least {overhead} characters, not {len}"
    );
    format!("{{\"a\":\"{}\"}}", "p".repeat(len - overhead))
}

/// `GetDataProtectionPolicy` reports the document the topic holds, byte for byte,
/// whatever that document is. The rows differ in the form, the content and the size of
/// the document — what "the document the topic holds" generalises over — and include
/// one at the maximum length `CreateTopicInput$DataProtectionPolicy` documents
/// (30,720 characters).
#[tokio::test]
async fn the_reported_document_is_the_one_the_topic_holds() {
    let sns = Sns::new();
    let at_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN);
    let cases: &[(&str, &str, &str)] = &[
        ("an empty json object", "dpp-empty-object", "{}"),
        (
            "a statement document",
            "dpp-statement",
            "{\"Name\":\"policy\",\"Version\":\"2021-06-01\",\"Statement\":[{\"Sid\":\"audit\",\
             \"DataDirection\":\"Inbound\",\"Principal\":[\"*\"],\
             \"DataIdentifier\":[\"arn:aws:dataprotection::aws:data-identifier/EmailAddress\"],\
             \"Operation\":{\"Audit\":{\"SampleRate\":\"99\"}}}]}",
        ),
        (
            "non-ascii content",
            "dpp-non-ascii",
            "{\"Name\":\"Grüße 😀 — déjà vu\",\"Statement\":[]}",
        ),
        (
            "content carrying the characters the xml envelope escapes",
            "dpp-xml-metacharacters",
            "{\"Name\":\"a & b <c> \\\"d\\\" 'e'\"}",
        ),
        (
            "a single space",
            "dpp-whitespace-only",
            " ",
        ),
        (
            "at the documented 30720 character bound",
            "dpp-at-the-bound",
            at_the_bound.as_str(),
        ),
    ];

    for (case, name, policy) in cases {
        let arn = topic_with_policy(&sns, case, name, policy).await;
        let answer = read(&sns, case, &arn).await;
        let actual = reported(case, &answer);
        assert_eq!(
            &actual, policy,
            "case {case:?}: expected the document the topic holds ({} characters), got {} characters: {actual:?}",
            policy.len(),
            actual.len()
        );
    }
}

/// One row of the no-policy table: the case, the topic's name, and the form
/// parameters the create carries beyond the name.
type CreationCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// A topic that holds NO policy is reported with an empty document, not a refusal and
/// not an omitted member: the topic exists, and it is its policy that is absent. The
/// rows differ in what else the topic carries, none of which installs a policy.
#[tokio::test]
async fn a_topic_holding_no_policy_reports_an_empty_document() {
    let sns = Sns::new();
    let cases: &[CreationCase<'_>] = &[
        ("a topic created with nothing else", "dpp-bare", &[]),
        (
            "a topic created with attributes",
            "dpp-with-attributes",
            &[("Attributes.entry.1.key", "DisplayName"), ("Attributes.entry.1.value", "Sales")],
        ),
        (
            "a topic created with tags",
            "dpp-with-tags",
            &[("Tags.member.1.Key", "team"), ("Tags.member.1.Value", "payments")],
        ),
        (
            "a fifo topic",
            "dpp-fifo.fifo",
            &[("Attributes.entry.1.key", "FifoTopic"), ("Attributes.entry.1.value", "true")],
        ),
    ];

    for (case, name, extra) in cases {
        let mut params: Vec<(&str, &str)> = vec![("Name", name)];
        params.extend_from_slice(extra);
        let created = sns.call("CreateTopic", &params).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTopic expected 200, got {} / {}",
            created.status, created.body
        );
        let arn = created
            .element("TopicArn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateTopic answered no TopicArn: {}", created.body));

        let answer = read(&sns, case, &arn).await;
        let actual = reported(case, &answer);
        assert_eq!(
            actual, "",
            "case {case:?}: expected an empty document for a topic holding no policy, got {actual:?}"
        );
    }
}

/// The document reported is the one the LAST write installed, so the read projects
/// persisted state rather than anything the read itself carries. The rows differ in
/// the document each write installs, including a write that replaces a document with
/// the empty one.
#[tokio::test]
async fn the_reported_document_follows_the_last_write() {
    let sns = Sns::new();
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "a document replaced by another",
            "dpp-replaced",
            "{\"Name\":\"first\"}",
            "{\"Name\":\"second\",\"Statement\":[]}",
        ),
        (
            "a document replaced by a longer one",
            "dpp-grown",
            "{}",
            "{\"Name\":\"grown\",\"Statement\":[{\"Sid\":\"s\"}]}",
        ),
        (
            "a document replaced by the empty one",
            "dpp-emptied",
            "{\"Name\":\"only\"}",
            "",
        ),
    ];

    for (case, name, first, second) in cases {
        let arn = topic_with_policy(&sns, case, name, first).await;
        let before = reported(case, &read(&sns, case, &arn).await);
        assert_eq!(
            &before, first,
            "case {case:?}: expected the first document, got {before:?}"
        );

        // A re-create applies what it carries to the topic that already exists, so it
        // is the second write of this topic's policy.
        let rewritten = topic_with_policy(&sns, case, name, second).await;
        assert_eq!(
            rewritten, arn,
            "case {case:?}: the re-create must name the same topic, got {rewritten:?}"
        );
        let after = reported(case, &read(&sns, case, &arn).await);
        assert_eq!(
            &after, second,
            "case {case:?}: expected the second document, got {after:?}"
        );
    }
}

/// The read reports, so it changes nothing a later caller sees: repeated reads of one
/// topic answer the same document, and the record still holds it afterwards.
#[tokio::test]
async fn reading_the_policy_leaves_the_topic_as_it_was() {
    let sns = Sns::new();
    let case = "repeated reads";
    let policy = "{\"Name\":\"stable\",\"Statement\":[]}";
    let arn = topic_with_policy(&sns, case, "dpp-stable", policy).await;
    let other = topic_with_policy(&sns, case, "dpp-untouched", "{\"Name\":\"other\"}").await;

    for attempt in 1..=3 {
        let actual = reported(case, &read(&sns, case, &arn).await);
        assert_eq!(
            &actual, &policy,
            "case {case:?}: read {attempt} must report the same document, got {actual:?}"
        );
    }

    let held = sns
        .state()
        .lock()
        .topics
        .get(&arn)
        .and_then(|topic| topic.data_protection_policy.clone());
    assert_eq!(
        held.as_deref(),
        Some(policy),
        "case {case:?}: the record must still hold the document after the reads, got {held:?}"
    );

    // Reading one topic says nothing about another's policy.
    let untouched = reported(case, &read(&sns, case, &other).await);
    assert_eq!(
        untouched, "{\"Name\":\"other\"}",
        "case {case:?}: another topic's document must be its own, got {untouched:?}"
    );
}

/// A `ResourceArn` that is not an ARN at all is bad input (400 `InvalidParameter`),
/// and an ARN this account holds no topic under is a lookup miss (404 `NotFound`).
/// The rows differ in HOW the value fails: its shape, or the resource it names.
#[tokio::test]
async fn an_arn_that_names_no_topic_is_refused() {
    let sns = Sns::new();
    let present = topic_with_policy(&sns, "setup", "dpp-present", "{}").await;
    let deleted = topic_with_policy(&sns, "setup", "dpp-deleted", "{}").await;
    let gone = sns.call("DeleteTopic", &[("TopicArn", &deleted)]).await;
    assert_eq!(
        gone.status, 200,
        "DeleteTopic expected 200, got {} / {}",
        gone.status, gone.body
    );
    let subscription_shaped = format!("{present}:11111111-2222-3333-4444-555555555555");

    let cases: &[(&str, &str, u16, &str)] = &[
        ("not an arn", "dpp-present", 400, "InvalidParameter"),
        ("too few arn elements", "arn:aws:sns", 400, "InvalidParameter"),
        ("an empty value", "", 400, "InvalidParameter"),
        (
            "an arn that does not begin with arn:",
            "urn:aws:sns:us-east-1:000000000000:dpp-present",
            400,
            "InvalidParameter",
        ),
        (
            "a topic no account holds",
            "arn:aws:sns:us-east-1:000000000000:dpp-never-created",
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
            "arn:aws:sqs:us-east-1:000000000000:dpp-present",
            404,
            "NotFound",
        ),
        ("a deleted topic", deleted.as_str(), 404, "NotFound"),
    ];

    for (case, arn, status, code) in cases {
        let answer = sns
            .call("GetDataProtectionPolicy", &[("ResourceArn", arn)])
            .await;
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

    // The topic that does exist is still readable: the refusals above left it alone.
    let still_there = reported("after the refusals", &read(&sns, "after the refusals", &present).await);
    assert_eq!(
        still_there, "{}",
        "the surviving topic must still report its own document, got {still_there:?}"
    );
}
