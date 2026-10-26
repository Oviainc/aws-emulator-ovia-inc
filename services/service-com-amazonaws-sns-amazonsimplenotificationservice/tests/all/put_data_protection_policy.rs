// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the data-protection-policy WRITE: SNS message data protection
//! is no longer available to new customers, so the regional endpoints do not serve
//! `PutDataProtectionPolicy` at all. The action gate in front of the service refuses
//! every call to it — HTTP 400, awsQuery `<Code>InvalidAction</Code>` and one fixed
//! sentence naming the operation — before anything about the request is examined, and
//! no topic acquires a document. The companion READ is served and is NOT gated.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls, and every claim
//! about what a topic holds is read back through `GetDataProtectionPolicy`.

use super::topics_subscriptions_publish::{Answer, Sns};
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::MAX_DATA_PROTECTION_POLICY_LEN;

/// The awsQuery error code the action gate names an operation the endpoint does not
/// serve by.
const GATE_CODE: &str = "InvalidAction";

/// The sentence the gate words that refusal with. Every character is fixed except the
/// parenthesised operation name, which is the operation the request named.
const GATE_MESSAGE: &str = "Operation (PutDataProtectionPolicy) is not supported in this region";

/// Attempt the write, returning the answer unasserted.
async fn put(sns: &Sns, arn: &str, policy: &str) -> Answer {
    sns.call(
        "PutDataProtectionPolicy",
        &[("ResourceArn", arn), ("DataProtectionPolicy", policy)],
    )
    .await
}

/// Assert `answer` is the gate's refusal: the status, the code, the `Sender` fault an
/// awsQuery client reads, and the sentence itself.
fn assert_gated(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        GATE_CODE,
        "case {case:?}: expected the {GATE_CODE} code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Type").unwrap_or_default(),
        "Sender",
        "case {case:?}: a 400 is the caller's fault, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").unwrap_or_default(),
        GATE_MESSAGE,
        "case {case:?}: expected the gate's sentence, got {}",
        answer.body
    );
}

/// The document `GetDataProtectionPolicy` reports for the topic `arn` names — the read
/// is not gated, so it is what proves the refused write installed nothing.
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
/// character of the document's content.
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

/// The gate refuses the call whatever the document is, so the refusal is identical for
/// every class of payload and no payload-specific refusal can appear in its place. The
/// rows differ in the form, the content, the size and even the well-formedness of the
/// document — what "the document the request carries" generalises over — and include
/// one at the documented 30,720-character bound and one past it, which the gate refuses
/// under its own code rather than under the length rule.
#[tokio::test]
async fn every_document_is_refused_by_the_action_gate() {
    let sns = Sns::new();
    let at_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN, 'p');
    let past_the_bound = document_of_length(MAX_DATA_PROTECTION_POLICY_LEN + 1, 'p');
    let cases: &[(&str, &str, &str)] = &[
        (
            "a deny statement over a managed data identifier",
            "put-dpp-deny",
            "{\"Name\":\"p\",\"Description\":\"d\",\"Version\":\"2021-06-01\",\
             \"Statement\":[{\"Sid\":\"denyEmailInbound\",\"DataDirection\":\"Inbound\",\
             \"Principal\":[\"*\"],\
             \"DataIdentifier\":[\"arn:aws:dataprotection::aws:data-identifier/EmailAddress\"],\
             \"Operation\":{\"Deny\":{}}}]}",
        ),
        (
            "a deidentify statement carrying a mask configuration",
            "put-dpp-deidentify",
            "{\"Name\":\"p\",\"Version\":\"2021-06-01\",\"Statement\":[{\"Sid\":\"mask\",\
             \"DataDirection\":\"Inbound\",\"Principal\":[\"*\"],\
             \"DataIdentifier\":[\"arn:aws:dataprotection::aws:data-identifier/EmailAddress\"],\
             \"Operation\":{\"Deidentify\":{\"MaskConfig\":{\"MaskWithCharacter\":\"#\"}}}}]}",
        ),
        (
            "content in several non-latin scripts",
            "put-dpp-unicode",
            "{\"Name\":\"p\",\"Description\":\"中文描述 Привет 😀 café\",\
             \"Statement\":[{\"Sid\":\"denyEmailInbound-日本語\"}]}",
        ),
        (
            "a truncated json string",
            "put-dpp-truncated",
            "{\"Name\": \"broken\", \"Description\": \"unterminated",
        ),
        (
            "a document that is not json at all",
            "put-dpp-not-json",
            "this is not a policy",
        ),
        ("an empty json object", "put-dpp-empty-object", "{}"),
        ("the empty document", "put-dpp-empty", ""),
        ("a single space", "put-dpp-whitespace-only", " "),
        (
            "at the documented 30720 character bound",
            "put-dpp-at-the-bound",
            at_the_bound.as_str(),
        ),
        (
            "one character past the documented bound",
            "put-dpp-past-the-bound",
            past_the_bound.as_str(),
        ),
    ];

    for (case, name, policy) in cases {
        let arn = sns.topic(name).await;
        let answer = put(&sns, &arn, policy).await;
        assert_gated(case, &answer);

        // The refusal stored nothing: the read reports the empty document a topic that
        // never acquired one carries, and the record holds no document either.
        let actual = reported(&sns, case, &arn).await;
        assert_eq!(
            actual, "",
            "case {case:?}: a refused write installs nothing, got {actual:?}"
        );
        let held = sns
            .state()
            .lock()
            .topics
            .get(&arn)
            .map(|topic| topic.data_protection_policy.clone());
        assert_eq!(
            held,
            Some(None),
            "case {case:?}: the record must hold no document, got {held:?}"
        );
    }
}

/// The gate sits in FRONT of the service, so it answers before the `ResourceArn` is
/// judged and before any topic is looked up: a value that is not an ARN at all, an ARN
/// naming a topic no account holds and an ARN of a deleted topic are refused with the
/// same code and the same sentence, never with the shape refusal or the lookup miss the
/// model declares. The rows differ in HOW the value would otherwise fail.
#[tokio::test]
async fn the_gate_answers_before_the_resource_arn_is_judged() {
    let sns = Sns::new();
    let present = sns.topic("put-dpp-present").await;
    let deleted = sns.topic("put-dpp-deleted").await;
    let gone = sns.call("DeleteTopic", &[("TopicArn", &deleted)]).await;
    assert_eq!(
        gone.status, 200,
        "DeleteTopic expected 200, got {} / {}",
        gone.status, gone.body
    );
    let subscription_shaped = format!("{present}:11111111-2222-3333-4444-555555555555");

    let cases: &[(&str, &str)] = &[
        ("a topic of this account", present.as_str()),
        ("not an arn", "put-dpp-present"),
        ("too few arn elements", "arn:aws:sns"),
        ("an empty value", ""),
        (
            "an arn that does not begin with arn:",
            "urn:aws:sns:us-east-1:000000000000:put-dpp-present",
        ),
        (
            "a topic no account holds",
            "arn:aws:sns:us-east-1:000000000000:put-dpp-never-created",
        ),
        (
            "a subscription of a topic that exists",
            subscription_shaped.as_str(),
        ),
        (
            "another service's resource",
            "arn:aws:sqs:us-east-1:000000000000:put-dpp-present",
        ),
        ("a deleted topic", deleted.as_str()),
    ];

    for (case, arn) in cases {
        let answer = put(&sns, arn, "{\"Name\":\"refused\"}").await;
        assert_gated(case, &answer);
    }
}

/// A request that omits a required member never reaches the gate: the boundary
/// validator answers first, under the query protocols' own refusal for a body it could
/// not read into the input shape. This is the one case where a gate implemented in the
/// handler cannot word the refusal, and the row pins it rather than leaving it
/// unasserted — the call is still refused and still installs nothing, which is what a
/// caller depends on.
#[tokio::test]
async fn a_request_missing_a_required_member_is_refused_before_the_gate() {
    let sns = Sns::new();
    let arn = sns.topic("put-dpp-incomplete").await;
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("no document member", &[("ResourceArn", "")]),
        ("no resource member", &[("DataProtectionPolicy", "{}")]),
        ("neither member", &[]),
    ];

    for (case, params) in cases {
        // The ARN is substituted into the row that carries it, so the only thing the
        // row varies is WHICH required member is absent.
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| {
                if *name == "ResourceArn" {
                    (*name, arn.as_str())
                } else {
                    (*name, *value)
                }
            })
            .collect();
        let answer = sns.call("PutDataProtectionPolicy", &params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an incomplete request must be refused, got {} / {}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.code(),
            "",
            "case {case:?}: the refusal must carry an error code, got {}",
            answer.body
        );
        let actual = reported(&sns, case, &arn).await;
        assert_eq!(
            actual, "",
            "case {case:?}: a refused request installs nothing, got {actual:?}"
        );
    }
}

/// A refused write changes nothing a later caller sees: a topic that holds a document
/// still reports it, a topic that holds none still reports the empty one, and repeating
/// the refused call answers the same refusal every time. The READ path is served
/// throughout — it is not gated.
#[tokio::test]
async fn a_refused_write_leaves_every_topic_as_it_was() {
    let sns = Sns::new();
    let case = "a refused write beside two topics";
    // `CreateTopic` is the write path a topic's document arrives through
    // (`CreateTopicInput$DataProtectionPolicy`), and it is not the gated action.
    let created = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "put-dpp-holder"),
                ("DataProtectionPolicy", "{\"Name\":\"its-own\"}"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateTopic expected 200, got {} / {}",
        created.status, created.body
    );
    let holder = created.element("TopicArn").unwrap_or_else(|| {
        panic!(
            "case {case:?}: CreateTopic answered no TopicArn: {}",
            created.body
        )
    });
    let bare = sns.topic("put-dpp-bare").await;

    for attempt in 1..=3 {
        for arn in [&holder, &bare] {
            let answer = put(&sns, arn, "{\"Name\":\"never-installed\"}").await;
            assert_gated(&format!("{case}, attempt {attempt}"), &answer);
        }
    }

    for (arn, expected) in [(&holder, "{\"Name\":\"its-own\"}"), (&bare, "")] {
        let actual = reported(&sns, case, arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: topic {arn:?} must still report its own document, got {actual:?}"
        );
    }

    let held = sns
        .state()
        .lock()
        .topics
        .get(&holder)
        .map(|topic| (topic.name.clone(), topic.data_protection_policy.clone()));
    assert_eq!(
        held,
        Some((
            "put-dpp-holder".to_string(),
            Some("{\"Name\":\"its-own\"}".to_string())
        )),
        "case {case:?}: the record must still hold the document the create installed, got {held:?}"
    );
}
