// SPDX-License-Identifier: CC0-1.0

//! The one BOUND a feed's output list is held to, driven through EVERY operation that
//! carries such a list.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what these tests assert is
//! what a client sees.
//!
//! THE RULE under test: a feed holds at most `MAX_OUTPUTS_PER_FEED` outputs, so an
//! operation whose list would leave it holding more is refused with the modelled
//! `ServiceQuotaExceededException` (HTTP 402) and the sentence that states that bound,
//! and writes nothing. The carriers differ only in WHICH list the feed is left holding:
//! `CreateFeed` mints the list and `UpdateFeed` replaces the stored one, so for them it is
//! the request's own list, while `AssociateFeed` appends — its outputs "supplement any
//! outputs that you specified when you created or updated the feed" — so for it the feed
//! is left holding the stored list plus what the call really adds. The figure they are all
//! measured against is the one the service states once, which is why the tables below send
//! the same counts through each carrier: neither can come to admit a list the other
//! refuses.
//!
//! The rows differ along what the rule generalises over: how many outputs the feed would
//! be left holding (one, several, one under the bound, the bound itself, one past it, well
//! past it), how that total is split between what the feed already holds and what the call
//! adds, and whether the outputs a call supplies are new to the feed at all.

use crate::harness::{call, enabled_output, message_of, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    too_many_appended_outputs_message, too_many_outputs_message, MAX_OUTPUTS_PER_FEED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The resource every claim below names. One name throughout, so a claim sent twice
/// against the same feed is that claim once more rather than a second one.
const CLAIMING_RESOURCE: &str = "quota-source-media";

/// Which operation CARRIES the output list under test (`OPERATIONS.md`): the two that
/// replace a feed's stored list and the one that appends to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `CreateFeed`: the list the minted feed holds (HTTP 202 when accepted).
    Create,
    /// `UpdateFeed`: the list that replaces the stored one (HTTP 200 when accepted).
    Update,
    /// `AssociateFeed`: the list appended to the stored one (HTTP 200 when accepted).
    Associate,
}

/// Every carrier of a feed's output list, so a rule about such a list is asserted on all
/// of them.
const CARRIERS: [Carrier; 3] = [Carrier::Create, Carrier::Update, Carrier::Associate];

impl Carrier {
    /// The status this operation answers when it accepts the list.
    fn accepted_status(self) -> u16 {
        match self {
            Self::Create => 202,
            Self::Update | Self::Associate => 200,
        }
    }

    /// The sentence a list past the bound is refused with, which the service states once
    /// per carrier of a list: the same bound either way, differing only in the remedy the
    /// caller's own operation offers.
    fn too_many_message(self) -> String {
        match self {
            Self::Create | Self::Update => too_many_outputs_message(),
            Self::Associate => too_many_appended_outputs_message(),
        }
    }
}

/// `count` outputs, each named `<prefix><index>`.
fn outputs(prefix: &str, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| enabled_output(&format!("{prefix}{index}"), "cropping"))
        .collect()
}

/// Mint a feed holding `held` outputs (named `held0`…) and return its id.
async fn feed_holding(router: &MultiServiceRouter, label: &str, held: usize) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": "quota-feed", "outputs": outputs("held", held)})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: the fixture feed holding {held} outputs is created, got {status} with \
         {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// Append `appended` to the feed `id` holds, as a claim by [`CLAIMING_RESOURCE`].
async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    appended: &[Value],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": CLAIMING_RESOURCE,
            "outputs": appended,
            "dryRun": false,
        })),
    )
    .await
}

/// How many outputs the feed `id` reports, read back through `GetFeed`.
async fn held_count(router: &MultiServiceRouter, label: &str, id: &str) -> usize {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed["outputs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: a feed reports an outputs array, got {feed}"))
        .len()
}

/// How many feeds this endpoint reports.
async fn feed_count(router: &MultiServiceRouter, label: &str) -> usize {
    let (status, listed) = call(router, "GET", "/v1/feeds?maxResults=100", None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListFeeds answers 200, got {status} with {listed}"
    );
    listed["feeds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: ListFeeds reports a feeds array, got {listed}"))
        .len()
}

/// Assert `answered` is the modelled refusal a list past the bound earns from `carrier`.
fn assert_refused(label: &str, carrier: Carrier, answered: (u16, Value)) {
    let (status, error) = answered;
    assert_eq!(
        status, 402,
        "case {label:?}: a list past the bound is refused 402, got {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ServiceQuotaExceededException"),
        "case {label:?}: that 402 is the modelled ServiceQuotaExceededException, got {error}"
    );
    let expected = carrier.too_many_message();
    assert_eq!(
        message_of(&error),
        expected,
        "case {label:?}: expected {expected:?}, got {:?}",
        message_of(&error)
    );
    assert!(
        message_of(&error).contains(&MAX_OUTPUTS_PER_FEED.to_string()),
        "case {label:?}: the refusal states the bound ({MAX_OUTPUTS_PER_FEED}), got {:?}",
        message_of(&error)
    );
}

/// EVERY carrier of a feed's output list is measured against the one bound: a list that
/// would leave the feed holding no more than `MAX_OUTPUTS_PER_FEED` outputs is accepted and
/// stored by each of them, and one that would leave it holding more is refused 402 by each
/// of them, leaving the store as it was.
#[tokio::test]
async fn every_carrier_of_a_feeds_output_list_is_held_to_the_one_bound() {
    // How many outputs the feed would be LEFT holding: one, several, one under the bound,
    // the bound itself, one past it, and well past it.
    let totals = [
        1_usize,
        3,
        MAX_OUTPUTS_PER_FEED - 1,
        MAX_OUTPUTS_PER_FEED,
        MAX_OUTPUTS_PER_FEED + 1,
        MAX_OUTPUTS_PER_FEED + 6,
    ];

    for left_holding in totals {
        for carrier in CARRIERS {
            let label = format!("{carrier:?}: the feed is left holding {left_holding} outputs");
            let router = router();
            // The feed a carrier that names one operates on. An append is measured
            // against what this feed HOLDS, so it supplies the rest of the total.
            let stored = feed_holding(&router, &label, 1).await;

            let answered = match carrier {
                Carrier::Create => {
                    call(
                        &router,
                        "POST",
                        "/v1/feed",
                        Some(json!({
                            "name": "minted-feed",
                            "outputs": outputs("fresh", left_holding)
                        })),
                    )
                    .await
                }
                Carrier::Update => {
                    call(
                        &router,
                        "PUT",
                        &format!("/v1/feed/{stored}"),
                        Some(json!({
                            "name": "replaced-feed",
                            "outputs": outputs("fresh", left_holding)
                        })),
                    )
                    .await
                }
                Carrier::Associate => {
                    associate(&router, &stored, &outputs("fresh", left_holding - 1)).await
                }
            };

            if left_holding > MAX_OUTPUTS_PER_FEED {
                assert_refused(&label, carrier, answered);
                // A refused list is stored nowhere: no feed was minted beyond the
                // fixture, and the fixture still holds exactly what it was created with.
                assert_eq!(
                    feed_count(&router, &label).await,
                    1,
                    "case {label:?}: the refused call minted no feed"
                );
                assert_eq!(
                    held_count(&router, &label, &stored).await,
                    1,
                    "case {label:?}: the refused call left the stored outputs as they were"
                );
                continue;
            }

            let (status, body) = answered;
            assert_eq!(
                status,
                carrier.accepted_status(),
                "case {label:?}: a list the bound admits is accepted with HTTP {}, got {status} \
                 with {body}",
                carrier.accepted_status()
            );
            // And the list the bound admits is really held: read back through GetFeed,
            // the feed holds exactly the total the row asked for.
            let id = match carrier {
                Carrier::Create => body["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: a minted feed carries an id"))
                    .to_string(),
                Carrier::Update | Carrier::Associate => stored,
            };
            assert_eq!(
                held_count(&router, &label, &id).await,
                left_holding,
                "case {label:?}: the accepted list leaves the feed holding {left_holding} outputs"
            );
        }
    }
}

/// The bound an APPEND is measured against is the list the feed HOLDS plus what the call
/// really adds — so the same supplied list is admitted or refused according to what the
/// stored feed holds, and a list that adds nothing is never past the bound.
#[tokio::test]
async fn an_appends_bound_is_measured_against_the_stored_list() {
    // (held, added) rows splitting the total differently on each side of the bound: up to
    // it from either direction, one past it from either direction, and a list that adds
    // nothing at all to a feed already AT it.
    let rows = [
        (1_usize, MAX_OUTPUTS_PER_FEED - 1),
        (MAX_OUTPUTS_PER_FEED - 1, 1),
        (3, MAX_OUTPUTS_PER_FEED - 3),
        (MAX_OUTPUTS_PER_FEED, 0),
        (MAX_OUTPUTS_PER_FEED, 1),
        (MAX_OUTPUTS_PER_FEED - 1, 2),
        (3, MAX_OUTPUTS_PER_FEED - 2),
        (1, MAX_OUTPUTS_PER_FEED + 4),
    ];

    for (held, added) in rows {
        let label = format!("{held} held + {added} added");
        let router = router();
        let id = feed_holding(&router, &label, held).await;
        let answered = associate(&router, &id, &outputs("added", added)).await;

        if held + added > MAX_OUTPUTS_PER_FEED {
            assert_refused(&label, Carrier::Associate, answered);
            assert_eq!(
                held_count(&router, &label, &id).await,
                held,
                "case {label:?}: the refused append grew nothing"
            );
            continue;
        }

        let (status, body) = answered;
        assert_eq!(
            status, 200,
            "case {label:?}: an append the bound admits is accepted, got {status} with {body}"
        );
        assert_eq!(
            held_count(&router, &label, &id).await,
            held + added,
            "case {label:?}: the feed holds what it held plus what the call added"
        );
    }
}

/// What the APPEND is measured on is what it really ADDS: a feed at the bound still serves
/// a repeat of the claim it already holds, because the outputs that claim named are in
/// place already and the call adds none.
#[tokio::test]
async fn a_repeat_that_adds_nothing_is_served_at_the_bound() {
    let router = router();
    let label = format!("a repeat at the bound of {MAX_OUTPUTS_PER_FEED}");
    let id = feed_holding(&router, &label, MAX_OUTPUTS_PER_FEED - 1).await;
    let filling = outputs("added", 1);

    let (status, body) = associate(&router, &id, &filling).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the append up to the bound is accepted, got {status} with {body}"
    );
    assert_eq!(
        held_count(&router, &label, &id).await,
        MAX_OUTPUTS_PER_FEED,
        "case {label:?}: the feed now holds the bound itself"
    );

    // The same request once more: every output it names is in place already, so it adds
    // nothing and the bound has nothing to refuse.
    let (status, body) = associate(&router, &id, &filling).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the repeat is served, got {status} with {body}"
    );
    assert_eq!(
        held_count(&router, &label, &id).await,
        MAX_OUTPUTS_PER_FEED,
        "case {label:?}: the repeat left one copy of each output"
    );

    // One output the feed does NOT hold is a real addition, and that one is past the
    // bound.
    let answered = associate(&router, &id, &outputs("past", 1)).await;
    assert_refused(
        &format!("one more output past the bound of {MAX_OUTPUTS_PER_FEED}"),
        Carrier::Associate,
        answered,
    );
    assert_eq!(
        held_count(&router, &label, &id).await,
        MAX_OUTPUTS_PER_FEED,
        "case {label:?}: the refused append grew nothing"
    );
}
