// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule about every paged list of this service, driven over the
//! real wire path:
//!
//! * a continuation page resumes AFTER the element the previous page reported LAST, named
//!   by that element's own identity in the list's order — a cursor is not a count of the
//!   elements already reported — so a write that lands between two pages of one walk can
//!   neither hide an element the walk has not reported yet nor report one twice.
//!
//! The list exercised here is `ListRegisteredSubscriptionProviders`, because
//! `DeregisterSubscriptionProvider` and `RegisterSubscriptionProvider` let a CALLER write
//! the store between two pages of one walk. The sibling discovery lists page the same way
//! and are covered where their own store changes under a walk
//! (`tests/all/discovery_from_ec2.rs`).
//!
//! The registrations are seeded into the store directly ([`crate::harness::seed_provider`])
//! because the rule under test is about the walk, not about the span a fresh registration
//! takes to become usable; the deregistration and the registration that land mid-walk go
//! over the wire, which is how a client writes the store.

use serde_json::{json, Value};

use crate::harness::{
    deregister, get_provider, harness, list_providers, listed, provider_arn, register,
    seed_provider, Harness, RED_HAT,
};

/// How many registrations the walks below page over: enough that several page sizes take
/// more than two pages, so a write can land between pages in more than one place.
const PROVIDERS: usize = 5;

/// The id of the `index`-th seeded registration.
///
/// UUID-shaped with an ascending sequence in the node field, the form a minted
/// `SubscriptionProviderArn` carries (`crate::state::provider_id`), offset well past the
/// sequence a wire registration in the same store mints so a registration created mid-walk
/// cannot land on a seeded ARN.
fn seeded_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{:012x}", 0x1000 + index)
}

/// Seed [`PROVIDERS`] registrations and answer their ARNs in the order the list reports
/// them.
fn seeded(h: &Harness) -> Vec<String> {
    (0..PROVIDERS)
        .map(|index| {
            seed_provider(
                h,
                &seeded_id(index),
                &format!("arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-{index}"),
                &[("owner", "platform")],
            )
        })
        .collect()
}

/// One page of the provider list: the ARNs it reported, and the cursor it minted for the
/// next page (`None` on the last page).
async fn page(
    h: &Harness,
    page_size: usize,
    token: Option<&str>,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let mut request = json!({ "MaxResults": page_size });
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let reply = list_providers(h, &request).await;
    let arns = listed(&reply, case)
        .into_iter()
        .map(|(arn, _, _, _)| arn)
        .collect();
    let next = reply
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (arns, next)
}

/// Deregister `arn` over the wire and assert the write landed, so a row that depends on
/// the store having changed cannot pass on a refused deregistration.
async fn deregistered(h: &Harness, arn: &str, case: &str) {
    let reply = deregister(h, arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected the deregistration of {arn} to succeed, got {} with body \
         {:?}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn a_deregistration_between_two_pages_never_hides_a_stored_registration() {
    // The rows differ along what the rule generalises over: the page size, how far into the
    // walk the write lands, and WHICH registration it removes — the one the cursor names
    // (the last on the page before the write), one already reported before it, and ones the
    // walk has not reached yet.
    for page_size in 1..=PROVIDERS + 1 {
        for pages_before_the_write in 1..=2 {
            for victim in 0..PROVIDERS {
                let h = harness();
                let arns = seeded(&h);
                let case = format!(
                    "MaxResults {page_size}, deregister {} after {pages_before_the_write} page(s)",
                    seeded_id(victim)
                );

                // Walk the first pages, then write, then walk the rest. A row whose write
                // would land after the last page writes once the walk is over, which is
                // the same rule with nothing left to report.
                let mut walked: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;
                let mut reported_before_the_write: Option<usize> = None;
                loop {
                    let (reported, next) = page(&h, page_size, token.as_deref(), &case).await;
                    assert!(
                        reported.len() <= page_size,
                        "case {case:?}: page {pages} holds {} entries, more than MaxResults",
                        reported.len()
                    );
                    walked.extend(reported);
                    token = next;
                    pages += 1;

                    if pages == pages_before_the_write {
                        reported_before_the_write = Some(walked.len());
                        deregistered(&h, &arns[victim], &case).await;
                    }
                    assert!(
                        pages <= PROVIDERS + 2,
                        "case {case:?}: the walk did not terminate after {pages} pages"
                    );
                    if token.is_none() {
                        break;
                    }
                }
                let reported_before_the_write = match reported_before_the_write {
                    Some(reported) => reported,
                    None => {
                        deregistered(&h, &arns[victim], &case).await;
                        walked.len()
                    }
                };

                // What the walk must report: everything it had already reported when the
                // write landed, and then every registration ordered after the last of those
                // that the store still holds.
                let expected: Vec<String> = arns[..reported_before_the_write]
                    .iter()
                    .cloned()
                    .chain(
                        arns[reported_before_the_write..]
                            .iter()
                            .filter(|arn| *arn != &arns[victim])
                            .cloned(),
                    )
                    .collect();
                assert_eq!(
                    walked, expected,
                    "case {case:?}: expected the walk to report {expected:?}, got {walked:?}"
                );

                // The symptom itself: a registration the store still holds — one every
                // later caller can read — must not be missing from a walk that reported
                // success.
                for (index, arn) in arns.iter().enumerate() {
                    if index == victim {
                        continue;
                    }
                    let stored = get_provider(&h, arn).await;
                    assert_eq!(
                        stored.status, 200,
                        "case {case:?}: {arn} must still be readable, got {} with body {:?}",
                        stored.status, stored.body
                    );
                    assert!(
                        walked.contains(arn),
                        "case {case:?}: {arn} is still registered but the walk reported \
                         {walked:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_walk_whose_remainder_is_deregistered_ends_without_a_refusal() {
    let h = harness();
    let arns = seeded(&h);
    let case = "every unreported registration deregistered mid-walk";

    let (reported, token) = page(&h, 1, None, case).await;
    assert_eq!(
        reported,
        vec![arns[0].clone()],
        "case {case:?}: expected the first registration, got {reported:?}"
    );
    let token = token
        .unwrap_or_else(|| panic!("case {case:?}: a truncated page must carry a NextToken"));

    for arn in &arns[1..] {
        deregistered(&h, arn, case).await;
    }

    // The cursor names a registration ordered before nothing that is left, so the walk is
    // over: an empty last page, no token, and no refusal — the token is still a cursor of
    // this list.
    let (reported, token) = page(&h, 1, Some(&token), case).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no further registrations, got {reported:?}"
    );
    assert!(
        token.is_none(),
        "case {case:?}: the walk is over, so no NextToken, got {token:?}"
    );
}

#[tokio::test]
async fn a_registration_created_between_two_pages_is_reported_at_most_once() {
    let h = harness();
    let arns = seeded(&h);
    let case = "a registration created mid-walk";

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    let mut created = String::new();
    loop {
        let (reported, next) = page(&h, 2, token.as_deref(), case).await;
        walked.extend(reported);
        token = next;
        pages += 1;
        if pages == 1 {
            created = provider_arn(
                &register(
                    &h,
                    &json!({
                        "SubscriptionProviderSource": RED_HAT,
                        "SecretArn":
                            "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-fresh",
                    }),
                )
                .await,
                case,
            );
        }
        assert!(
            pages <= PROVIDERS + 2,
            "case {case:?}: the walk did not terminate after {pages} pages"
        );
        if token.is_none() {
            break;
        }
    }

    // Every registration that was in the store for the whole walk is reported exactly
    // once, and the one created while the walk ran is reported at most once — never twice,
    // whichever side of the cursor it was created on.
    for arn in &arns {
        assert_eq!(
            walked.iter().filter(|walked| *walked == arn).count(),
            1,
            "case {case:?}: expected {arn} reported exactly once, got {walked:?}"
        );
    }
    assert!(
        walked.iter().filter(|walked| **walked == created).count() <= 1,
        "case {case:?}: expected {created} reported at most once, got {walked:?}"
    );

    // And a walk started after the write reports the created registration with the rest.
    let after = "a fresh walk after the registration";
    let rows = listed(&list_providers(&h, &json!({})).await, after);
    let mut reported: Vec<String> = rows.into_iter().map(|(arn, _, _, _)| arn).collect();
    reported.sort();
    let mut expected: Vec<String> = arns.iter().cloned().chain([created.clone()]).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case {after:?}: expected {expected:?}, got {reported:?}"
    );
}
