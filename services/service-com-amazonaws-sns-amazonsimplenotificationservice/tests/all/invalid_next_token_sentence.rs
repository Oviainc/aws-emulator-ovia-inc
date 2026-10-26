// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE an SNS paginated listing refuses a `NextToken`
//! it did not mint with, driven over the wire through the router the emulator serves in
//! production.
//!
//! The rule under test: a pagination token SNS did not mint is bad input for the member
//! it arrived in, so the listing answers `InvalidParameterException` — HTTP 400,
//! awsQuery code `InvalidParameter`
//! (`smithy_query(query="operation", operation="ListTopics")`, and the same error on
//! `ListSubscriptions`, `ListSubscriptionsByTopic` and `ListSMSSandboxPhoneNumbers`) —
//! and the sentence is `Invalid parameter:` plus the name of that member, carrying
//! neither the rejected value nor a reason. The mobile-push listing states the same
//! refusal with a trailing reason clause, which is the one row whose sentence differs.
//!
//! The rows differ along the LISTING that refuses the token and the FORM of the token
//! — not a number at all, a base64-looking opaque string, past the end of the set,
//! negative, fractional, blank, a respelling of a number the listing does mint, and an
//! ARN — which is what the rule generalises over. The refusals are also asserted to
//! leave the records alone, so a refused page is not a page that consumed anything.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_NEXT_TOKEN_REFUSAL, MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL,
};

use super::topics_subscriptions_publish::Sns;

/// The member every one of these listings carries its pagination token in.
const MEMBER: &str = "NextToken";

/// The forms a token that no listing minted takes: `(case, token)`.
const TOKENS: &[(&str, &str)] = &[
    ("not a token at all", "bogus"),
    (
        "a base64-looking opaque string",
        "c2ltMnJlYWwtbm90LWEtcmVhbC10b2tlbg==",
    ),
    ("past the end of the set", "9999"),
    ("negative", "-1"),
    ("fractional", "1.5"),
    ("blank", " "),
    ("a zero-padded respelling of a minted token", "000"),
    ("an ARN", "arn:aws:sns:us-east-1:000000000000:paged-topic"),
];

/// One listing of the table: the action, the parameters it needs besides the token,
/// and the whole sentence it refuses an unminted token with.
type Listing<'a> = (&'a str, Vec<(&'a str, &'a str)>, &'a str);

/// Every paginated SNS listing refuses a token it did not mint with the sentence the
/// service states for the member the token arrived in.
#[tokio::test]
async fn a_next_token_no_listing_minted_is_refused_with_the_members_sentence() {
    let sns = Sns::new();
    let topic = sns.topic("paged-topic").await;
    let listings: Vec<Listing> = vec![
        ("ListTopics", Vec::new(), INVALID_NEXT_TOKEN_REFUSAL),
        ("ListSubscriptions", Vec::new(), INVALID_NEXT_TOKEN_REFUSAL),
        (
            "ListSubscriptionsByTopic",
            vec![("TopicArn", topic.as_str())],
            INVALID_NEXT_TOKEN_REFUSAL,
        ),
        (
            "ListSMSSandboxPhoneNumbers",
            Vec::new(),
            INVALID_NEXT_TOKEN_REFUSAL,
        ),
        (
            "ListPlatformApplications",
            Vec::new(),
            MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL,
        ),
    ];

    // The sentence of the notification plane is the fixed text plus the member's name
    // and nothing else; the mobile-push listing's is that sentence plus its reason
    // clause.
    assert_eq!(
        INVALID_NEXT_TOKEN_REFUSAL,
        format!("Invalid parameter: {MEMBER}"),
        "the refusal sentence names the member and nothing else, got \
         {INVALID_NEXT_TOKEN_REFUSAL:?}"
    );
    assert!(
        MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL.starts_with(INVALID_NEXT_TOKEN_REFUSAL),
        "the mobile-push sentence states the same member, got \
         {MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL:?}"
    );

    for (action, extra, sentence) in &listings {
        for (case, token) in TOKENS {
            let mut params = extra.clone();
            params.push((MEMBER, token));
            let answer = sns.call(action, &params).await;
            assert_eq!(
                answer.status, 400,
                "case {action} / {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {action} / {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(*sentence),
                "case {action} / {case:?}: expected {sentence:?}, got {}",
                answer.body
            );
        }
    }

    // A refused page changes nothing a later caller sees: the topic is still listed,
    // and the token the listing DOES mint is still the only one it accepts.
    let listed = sns.call("ListTopics", &[]).await;
    assert_eq!(
        listed.elements("TopicArn"),
        vec![topic.clone()],
        "the refused listings must leave the account's topics alone, got {}",
        listed.body
    );
    let accepted = sns.call("ListTopics", &[(MEMBER, "0")]).await;
    assert_eq!(
        accepted.status, 200,
        "case \"the first page's own token\": expected 200, got {} / {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.elements("TopicArn"),
        vec![topic],
        "case \"the first page's own token\": expected the same page, got {}",
        accepted.body
    );
}
