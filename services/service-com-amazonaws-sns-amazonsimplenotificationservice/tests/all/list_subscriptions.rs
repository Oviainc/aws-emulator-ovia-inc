// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-WIDE subscription listing, driven over the wire: every call below is an
//! awsQuery HTTP request dispatched through the router the emulator serves in
//! production, so nothing but persisted state carries information between calls.
//!
//! The rule covered: `ListSubscriptions` reports, from persisted state, every
//! subscription the account holds over EVERY topic — each with the members SNS
//! publishes for a subscription (its ARN, owner, protocol, endpoint and topic) — one
//! page of at most the documented page size at a time, with an opaque `NextToken`
//! while subscriptions remain, and it changes nothing a later caller sees. A
//! `NextToken` it did not mint is refused with `InvalidParameterException` (HTTP 400).
//!
//! Every assertion runs over a SET of inputs: several topics, every protocol SNS
//! addresses (so both the confirmed and the pending halves of the model are listed),
//! subscriptions added and removed, and the page boundary crossed.
//!
//! The sibling suite `account_subscriptions_listing.rs` covers the same operation over
//! its own table; this one drives it through the shared wire harness this test target
//! already has, and its own rows turn on the two things a second table can add: that
//! the account-wide listing and a topic's own listing agree MEMBER FOR MEMBER about the
//! subscriptions they share, and that the page boundary sits exactly at the documented
//! page size and does not move between reads.

use std::collections::{HashMap, HashSet};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_NEXT_TOKEN_REFUSAL, PAGE_SIZE, PENDING_CONFIRMATION_LISTED,
};

use super::topics_subscriptions_publish::{text_of, Answer, Sns};

/// One subscription as the listing reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    subscription_arn: String,
    owner: String,
    protocol: String,
    endpoint: String,
    topic_arn: String,
}

/// The `Subscriptions` list of a listing response, or `None` when the response carries
/// no such member at all — a refused call serves no listing.
fn subscriptions_block(body: &str) -> Option<&str> {
    if let Some(start) = body.find("<Subscriptions>") {
        let rest = &body[start + "<Subscriptions>".len()..];
        let end = rest.find("</Subscriptions>")?;
        return Some(&rest[..end]);
    }
    body.contains("<Subscriptions/>").then_some("")
}

/// The subscriptions a listing reports, in document order.
///
/// A list member's open tag carries the envelope's default namespace, so the opening is
/// matched up to its first `>` rather than as a bare `<member>`.
fn reported(answer: &Answer) -> Vec<Listed> {
    let block = subscriptions_block(&answer.body)
        .unwrap_or_else(|| panic!("no Subscriptions member in {}", answer.body));
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        rest = &rest[open + 1..];
        let Some(end) = rest.find("</member>") else {
            break;
        };
        let member = &rest[..end];
        found.push(Listed {
            subscription_arn: text_of(member, "SubscriptionArn"),
            owner: text_of(member, "Owner"),
            protocol: text_of(member, "Protocol"),
            endpoint: text_of(member, "Endpoint"),
            topic_arn: text_of(member, "TopicArn"),
        });
        rest = &rest[end + "</member>".len()..];
    }
    found
}

/// One `ListSubscriptions` call, asserting it was answered with HTTP 200.
async fn list(sns: &Sns, case: &str, params: &[(&str, &str)]) -> Answer {
    let answer = sns.call("ListSubscriptions", params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
}

/// Subscribe `endpoint` over `protocol`, asking for the ARN in every case, and answer
/// the subscription ARN the call reports.
async fn subscribe(sns: &Sns, case: &str, topic: &str, protocol: &str, endpoint: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", protocol),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: Subscribe expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body))
}

/// One seeded subscription: the protocol, an endpoint in the form that protocol
/// addresses, and whether SNS leaves it awaiting the subscriber's confirmation.
///
/// The protocols and endpoint forms are the per-protocol catalog of the
/// `SubscribeInput$Protocol` / `SubscribeInput$Endpoint` docs; the rows cover protocols
/// the issue's trace never exercised (`sqs`, `email-json`, `firehose`, `sms`) beside the
/// `lambda` one it did, and two endpoint forms for most of them.
const PROTOCOLS: &[(&str, &str, bool)] = &[
    (
        "lambda",
        "arn:aws:lambda:us-east-1:000000000000:function:trigger",
        false,
    ),
    (
        "lambda",
        "arn:aws:lambda:eu-west-1:000000000000:function:drain:PROD",
        false,
    ),
    ("sqs", "arn:aws:sqs:us-east-1:000000000000:queue", false),
    (
        "firehose",
        "arn:aws:firehose:us-east-1:000000000000:deliverystream/s",
        false,
    ),
    ("sms", "+15550100", false),
    ("http", "http://example.test/hook", true),
    ("https", "https://deep.example.test/a/b/c", true),
    ("email", "ops@example.test", true),
    ("email-json", "alerts@sub.example.test", true),
];

/// `ListSubscriptions` enumerates the whole account: every subscription of every topic,
/// each with the members SNS publishes for it, and nothing else.
///
/// Also the baseline: an account holding no subscription at all is answered 200 with an
/// empty listing, not a refusal.
#[tokio::test]
async fn list_subscriptions_enumerates_every_subscription_of_every_topic() {
    let sns = Sns::new();

    let empty = list(&sns, "an account with no subscription", &[]).await;
    assert!(
        reported(&empty).is_empty(),
        "an account with no subscription must be listed as empty, got {}",
        empty.body
    );
    assert!(
        empty.element("NextToken").is_none(),
        "an empty listing must carry no NextToken, got {}",
        empty.body
    );

    // Spread the rows over THREE topics, so the account-wide listing cannot pass by
    // reporting one topic's subscriptions.
    let topics = [
        sns.topic("account-wide-a").await,
        sns.topic("account-wide-b").await,
        sns.topic("account-wide-c").await,
    ];
    let mut expected: HashMap<String, Listed> = HashMap::new();
    for (index, (protocol, endpoint, pending)) in PROTOCOLS.iter().enumerate() {
        let case = format!("{protocol} -> {endpoint}");
        let topic = &topics[index % topics.len()];
        let arn = subscribe(&sns, &case, topic, protocol, endpoint).await;
        let listed_arn = if *pending {
            PENDING_CONFIRMATION_LISTED.to_string()
        } else {
            arn.clone()
        };
        expected.insert(
            arn,
            Listed {
                subscription_arn: listed_arn,
                // Every ARN and owner this emulator reports carries its own account.
                owner: owner_of(&topics[0]),
                protocol: protocol.to_string(),
                endpoint: endpoint.to_string(),
                topic_arn: topic.clone(),
            },
        );
    }

    let answer = list(&sns, "every protocol over three topics", &[]).await;
    let found = reported(&answer);
    assert_eq!(
        found.len(),
        PROTOCOLS.len(),
        "case \"every protocol over three topics\": expected {} subscriptions, got {found:?}",
        PROTOCOLS.len()
    );
    for (arn, want) in &expected {
        let case = format!("{} -> {}", want.protocol, want.endpoint);
        let got = found
            .iter()
            .find(|listed| listed.endpoint == want.endpoint && listed.protocol == want.protocol)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {arn} is not in the account-wide listing: {found:?}")
            });
        assert_eq!(
            got, want,
            "case {case:?}: expected the subscription's published members {want:?}, got {got:?}"
        );
    }

    // Each topic's own listing is a SUBSET of the account-wide one: the two listings
    // agree member for member about the subscriptions they share.
    for topic in &topics {
        let by_topic = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", topic)])
            .await;
        assert_eq!(
            by_topic.status, 200,
            "case {topic:?}: ListSubscriptionsByTopic expected 200, got {} / {}",
            by_topic.status, by_topic.body
        );
        let per_topic = reported(&by_topic);
        let account_wide: Vec<Listed> = found
            .iter()
            .filter(|listed| &listed.topic_arn == topic)
            .cloned()
            .collect();
        assert_eq!(
            per_topic, account_wide,
            "case {topic:?}: the account-wide listing must report this topic's \
             subscriptions exactly as the topic's own listing does, got {per_topic:?}"
        );
    }

    // A read must not change what a later caller sees.
    let again = list(&sns, "a second read", &[]).await;
    assert_eq!(
        reported(&again),
        found,
        "case \"a second read\": a listing must not change what the next one reports, got {}",
        again.body
    );
}

/// The account id an SNS ARN carries, read back out of an ARN this emulator minted —
/// never written into the test as a literal.
fn owner_of(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("{arn} is not an SNS ARN"))
        .to_string()
}

/// The account-wide listing is computed from persisted state at every call: a
/// subscription added appears, one removed by `Unsubscribe` is gone, and the
/// subscriptions of a deleted topic go with it — while the subscriptions of every other
/// topic stay.
///
/// The removals are the invariant the issue's traces turn on: that an unsubscribe is
/// visible through the listing, and that a subscription whose endpoint resource no
/// longer exists is still the account's (SNS holds no state about the endpoint's
/// service, so nothing about the endpoint can remove the subscription).
#[tokio::test]
async fn the_account_wide_listing_follows_the_stored_subscriptions() {
    let sns = Sns::new();
    let kept = sns.topic("kept-topic").await;
    let doomed = sns.topic("doomed-topic").await;

    let mut live: HashSet<String> = HashSet::new();
    for (index, (protocol, endpoint, pending)) in PROTOCOLS.iter().enumerate() {
        let case = format!("{protocol} -> {endpoint}");
        let topic = if index % 2 == 0 { &kept } else { &doomed };
        let arn = subscribe(&sns, &case, topic, protocol, endpoint).await;
        live.insert(arn.clone());

        // Each Subscribe is visible in the account-wide listing straight away.
        let answer = list(&sns, &case, &[]).await;
        assert_eq!(
            reported(&answer).len(),
            live.len(),
            "case {case:?}: expected {} subscriptions after subscribing, got {}",
            live.len(),
            answer.body
        );
        let listed_arn = if *pending {
            PENDING_CONFIRMATION_LISTED.to_string()
        } else {
            arn.clone()
        };
        assert!(
            reported(&answer)
                .iter()
                .any(|listed| listed.subscription_arn == listed_arn
                    && listed.endpoint == *endpoint),
            "case {case:?}: expected {listed_arn} in the listing, got {}",
            answer.body
        );
    }

    // Unsubscribe every CONFIRMED subscription of the kept topic: each one leaves the
    // account-wide listing, and no other subscription does.
    let confirmed_of_kept: Vec<String> = reported(&list(&sns, "before unsubscribing", &[]).await)
        .into_iter()
        .filter(|listed| {
            listed.topic_arn == kept && listed.subscription_arn != PENDING_CONFIRMATION_LISTED
        })
        .map(|listed| listed.subscription_arn)
        .collect();
    assert!(
        !confirmed_of_kept.is_empty(),
        "the table must seed at least one confirmed subscription of the kept topic"
    );
    for arn in &confirmed_of_kept {
        let answer = sns.call("Unsubscribe", &[("SubscriptionArn", arn)]).await;
        assert_eq!(
            answer.status, 200,
            "case {arn:?}: Unsubscribe expected 200, got {} / {}",
            answer.status, answer.body
        );
        live.remove(arn);
        let listed = reported(&list(&sns, arn, &[]).await);
        assert!(
            !listed
                .iter()
                .any(|found| &found.subscription_arn == arn),
            "case {arn:?}: an unsubscribed subscription must be gone from the \
             account-wide listing, got {listed:?}"
        );
        assert_eq!(
            listed.len(),
            live.len(),
            "case {arn:?}: expected {} subscriptions left, got {listed:?}",
            live.len()
        );
    }

    // Deleting a topic takes its subscriptions out of the account-wide listing, and
    // only its own.
    let before: Vec<Listed> = reported(&list(&sns, "before DeleteTopic", &[]).await);
    let survivors: Vec<Listed> = before
        .iter()
        .filter(|listed| listed.topic_arn != doomed)
        .cloned()
        .collect();
    let deleted = sns.call("DeleteTopic", &[("TopicArn", &doomed)]).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteTopic expected 200, got {} / {}",
        deleted.status, deleted.body
    );
    let after = reported(&list(&sns, "after DeleteTopic", &[]).await);
    assert_eq!(
        after, survivors,
        "case \"after DeleteTopic\": only the deleted topic's subscriptions may go, \
         got {after:?}"
    );
}

/// The listing pages at the documented page size, over the whole account rather than
/// one topic: a full page carries a `NextToken`, the pages together hold every
/// subscription exactly once, the last page carries no token — and a token the listing
/// never minted is refused with HTTP 400 `InvalidParameter` and the sentence SNS states
/// for the member.
#[tokio::test]
async fn the_account_wide_listing_pages_at_the_documented_page_size() {
    let sns = Sns::new();
    // One past the page size, split across two topics so the page boundary falls
    // INSIDE the account's subscriptions and not at a topic's edge.
    let total = PAGE_SIZE + 1;
    let topics = [sns.topic("paged-a").await, sns.topic("paged-b").await];
    for index in 0..total {
        let topic = &topics[index % topics.len()];
        subscribe(
            &sns,
            &format!("subscription {index:03}"),
            topic,
            "sqs",
            &format!("arn:aws:sqs:us-east-1:000000000000:q{index:03}"),
        )
        .await;
    }

    let first = list(&sns, "the first page", &[]).await;
    let first_page = reported(&first);
    assert_eq!(
        first_page.len(),
        PAGE_SIZE,
        "case \"the first page\": expected the full page size {PAGE_SIZE}, got {}",
        first_page.len()
    );
    let token = first.element("NextToken").unwrap_or_else(|| {
        panic!(
            "case \"the first page\": a full page must carry a NextToken, got {}",
            first.body
        )
    });

    let second = list(&sns, "the last page", &[("NextToken", &token)]).await;
    let second_page = reported(&second);
    assert_eq!(
        second_page.len(),
        total - PAGE_SIZE,
        "case \"the last page\": expected the remainder {}, got {}",
        total - PAGE_SIZE,
        second_page.len()
    );
    assert!(
        second.element("NextToken").is_none(),
        "case \"the last page\": the last page must carry no NextToken, got {}",
        second.body
    );

    let mut all = first_page.clone();
    all.extend(second_page);
    let unique: HashSet<&String> = all.iter().map(|listed| &listed.subscription_arn).collect();
    assert_eq!(
        unique.len(),
        total,
        "the pages together must hold every subscription exactly once, got {} of {total}",
        unique.len()
    );

    // The page boundary is stable: the same first page, and the same token, on a
    // second read.
    let repeat = list(&sns, "a repeated first page", &[]).await;
    assert_eq!(
        reported(&repeat),
        first_page,
        "case \"a repeated first page\": the page must not move between reads, got {}",
        repeat.body
    );
    assert_eq!(
        repeat.element("NextToken"),
        Some(token),
        "case \"a repeated first page\": the token must not move between reads, got {}",
        repeat.body
    );

    // A token this listing did not mint: not a number, a negative index, a position
    // past the end, and one past the largest index it could have minted.
    for bad in [
        "not-a-token",
        "-1",
        "99999999999999999999",
        &(total + 1).to_string(),
    ] {
        let answer = sns
            .call("ListSubscriptions", &[("NextToken", bad)])
            .await;
        assert_eq!(
            answer.status, 400,
            "case {bad:?}: an unissued token must be 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {bad:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_NEXT_TOKEN_REFUSAL),
            "case {bad:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
        assert!(
            subscriptions_block(&answer.body).is_none(),
            "case {bad:?}: a refused listing must serve no subscriptions, got {}",
            answer.body
        );
    }
}
