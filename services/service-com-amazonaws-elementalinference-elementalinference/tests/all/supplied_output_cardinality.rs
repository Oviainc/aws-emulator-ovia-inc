// SPDX-License-Identifier: CC0-1.0

//! The CARDINALITY rule every request that supplies a feed's output list is held to, end
//! to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what these tests assert is
//! what a client sees.
//!
//! THE RULE under test: an operation whose request carries a feed's output list requires
//! at least one output in it — a create "must specify at least one output"
//! (`CreateFeedRequest$outputs`), an update's list is the one the feed is left holding,
//! and a claim supplies "an array of one or more outputs that you want to add to this
//! feed now" (`AssociateFeedRequest$outputs`) — so a list carrying none is refused with
//! the modelled `ValidationException` (HTTP 400) and one invariant sentence, whichever
//! operation carried it and whatever else the request says, and nothing is written. The
//! list shape carries no `@length`, so the boundary validator decides none of this and
//! the service's own rule does.
//!
//! Both halves are asserted over a TABLE whose rows differ along what the rule
//! generalises over: which operation carries the list, the optional members the request
//! carries beside it (a create's tags and access role, a claim's `dryRun`), how many
//! outputs the stored feed holds, whether a resource already holds that feed, and — for
//! the lists the rule ADMITS — how many outputs they carry, from one to the bound a feed
//! is held to. The further rules a supplied list is held to (a name twice, the bound) are
//! `output_list_rule.rs` and `feed_output_validation.rs`.

use crate::harness::{call, enabled_output, message_of, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_OUTPUTS_PER_FEED, MISSING_OUTPUTS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The resource every claim below names. One name throughout, so a claim repeated against
/// a feed that already holds it is that same claim once more rather than a second one.
const CLAIMING_RESOURCE: &str = "cardinality-source-media";

/// The operations whose request CARRIES a feed's output list (`OPERATIONS.md`): the two
/// that replace the stored list and the one that appends to it, the latter both for real
/// and as a dry run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Supplier {
    /// `CreateFeed`: the list the minted feed holds (HTTP 202 when accepted).
    Create,
    /// `UpdateFeed`: the list that replaces the stored one (HTTP 200 when accepted).
    Update,
    /// `AssociateFeed`: the list appended to the stored one (HTTP 200 when accepted).
    Associate { dry_run: bool },
}

impl Supplier {
    /// The status this operation answers when it accepts the list.
    fn accepted_status(self) -> u16 {
        match self {
            Self::Create => 202,
            Self::Update | Self::Associate { .. } => 200,
        }
    }
}

/// An IAM role ARN in the account this emulator serves, satisfying `IamRoleArn`
/// (len 32..255, `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$`): a role a create may be
/// asked to pass, so a row carrying it is not refused for the role.
fn own_account_role() -> String {
    format!(
        "arn:aws:iam::{}:role/elemental-inference-cardinality",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The request body `supplier` carries for `outputs`, with the members of `extra` merged
/// in — the optional members a row varies.
fn request_body(supplier: Supplier, outputs: &Value, extra: &Value) -> Value {
    let mut body = match supplier {
        Supplier::Create => json!({"name": "minted-feed", "outputs": outputs}),
        Supplier::Update => json!({"name": "replaced-feed", "outputs": outputs}),
        Supplier::Associate { dry_run } => json!({
            "associatedResourceName": CLAIMING_RESOURCE,
            "outputs": outputs,
            "dryRun": dry_run,
        }),
    };
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body.as_object_mut()
            .expect("a request body is an object")
            .insert(key, value);
    }
    body
}

/// Send `outputs` to `supplier`, against the stored feed `id` where the operation names
/// one.
async fn supply(
    router: &MultiServiceRouter,
    supplier: Supplier,
    id: &str,
    outputs: &Value,
    extra: &Value,
) -> (u16, Value) {
    let body = Some(request_body(supplier, outputs, extra));
    match supplier {
        Supplier::Create => call(router, "POST", "/v1/feed", body).await,
        Supplier::Update => call(router, "PUT", &format!("/v1/feed/{id}"), body).await,
        Supplier::Associate { .. } => {
            call(router, "POST", &format!("/v1/feed/{id}/associate"), body).await
        }
    }
}

/// `GET /v1/feed/{id}`, asserting the read itself is served.
async fn get_feed(router: &MultiServiceRouter, label: &str, id: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// The names of the outputs a feed response reports, in the order it reports them.
fn output_names(feed: &Value) -> Vec<String> {
    feed["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|output| output["name"].as_str().unwrap_or_default().to_string())
        .collect()
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

/// `held` distinct output names, as a stored feed carries them.
fn held_names(held: usize) -> Vec<String> {
    (0..held).map(|index| format!("held{index}")).collect()
}

/// A list of `count` outputs named apart from any a stored feed holds.
fn fresh_outputs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| enabled_output(&format!("fresh{index}"), "cropping"))
            .collect(),
    )
}

/// Mint the stored feed a row operates on — holding `held` outputs, claimed by
/// [`CLAIMING_RESOURCE`] when `claimed` — and return its id.
async fn stored_feed(
    router: &MultiServiceRouter,
    label: &str,
    held: usize,
    claimed: bool,
) -> String {
    let outputs: Vec<Value> = held_names(held)
        .iter()
        .map(|name| enabled_output(name, "cropping"))
        .collect();
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": "stored-feed", "outputs": outputs})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: the stored feed is created, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string();
    if claimed {
        let (status, body) = supply(
            router,
            Supplier::Associate { dry_run: false },
            &id,
            &json!([enabled_output("claimed1", "contextualMetadata")]),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the setup claim is accepted, got {status} with {body}"
        );
    }
    id
}

/// One row of the refusal table: which operation carries the empty list, what else the
/// request carries, and the stored feed it is sent against.
struct EmptyListCase {
    label: &'static str,
    supplier: Supplier,
    /// How many outputs the stored feed holds.
    held: usize,
    /// Whether a resource already holds that feed.
    claimed: bool,
    /// The optional members this row adds to the request body.
    extra: Value,
}

fn empty_list_cases() -> Vec<EmptyListCase> {
    vec![
        EmptyListCase {
            label: "a create carrying the required members alone",
            supplier: Supplier::Create,
            held: 1,
            claimed: false,
            extra: json!({}),
        },
        EmptyListCase {
            label: "a create carrying tags and an access role this account holds",
            supplier: Supplier::Create,
            held: 1,
            claimed: false,
            extra: json!({"tags": {"team": "video"}, "accessRoleArn": own_account_role()}),
        },
        EmptyListCase {
            label: "an update of a feed holding one output",
            supplier: Supplier::Update,
            held: 1,
            claimed: false,
            extra: json!({}),
        },
        EmptyListCase {
            label: "an update of a feed holding three outputs",
            supplier: Supplier::Update,
            held: 3,
            claimed: false,
            extra: json!({}),
        },
        EmptyListCase {
            label: "an update of a feed a resource holds",
            supplier: Supplier::Update,
            held: 1,
            claimed: true,
            extra: json!({}),
        },
        EmptyListCase {
            label: "a claim of a feed no resource holds yet",
            supplier: Supplier::Associate { dry_run: false },
            held: 1,
            claimed: false,
            extra: json!({}),
        },
        EmptyListCase {
            label: "a claim of a feed holding three outputs",
            supplier: Supplier::Associate { dry_run: false },
            held: 3,
            claimed: false,
            extra: json!({}),
        },
        EmptyListCase {
            label: "a claim repeated by the resource that already holds the feed",
            supplier: Supplier::Associate { dry_run: false },
            held: 1,
            claimed: true,
            extra: json!({}),
        },
        EmptyListCase {
            label: "a dry-run claim of a feed no resource holds yet",
            supplier: Supplier::Associate { dry_run: true },
            held: 1,
            claimed: false,
            extra: json!({}),
        },
        EmptyListCase {
            label: "a dry-run claim of a feed a resource already holds",
            supplier: Supplier::Associate { dry_run: true },
            held: 1,
            claimed: true,
            extra: json!({}),
        },
    ]
}

/// A request that supplies no output at all is refused by every operation that carries a
/// feed's output list, with one invariant sentence — and writes nothing.
#[tokio::test]
async fn a_supplied_output_list_carrying_no_output_is_refused_by_every_operation() {
    let mut sentences: Vec<String> = Vec::new();

    for case in empty_list_cases() {
        let label = case.label;
        let router = router();
        let id = stored_feed(&router, label, case.held, case.claimed).await;
        let before = get_feed(&router, label, &id).await;
        let feeds_before = feed_count(&router, label).await;

        let (status, body) =
            supply(&router, case.supplier, &id, &json!([]), &case.extra).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a list carrying no output is a 400, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {body}"
        );
        let message = message_of(&body).to_string();
        assert_eq!(
            message, MISSING_OUTPUTS_MESSAGE,
            "case {label:?}: expected {MISSING_OUTPUTS_MESSAGE:?}, got {message:?}"
        );
        sentences.push(message);

        // Nothing was written: no feed was minted, and the stored feed holds the outputs,
        // the status and the association it held before.
        assert_eq!(
            feed_count(&router, label).await,
            feeds_before,
            "case {label:?}: the refused request minted no feed"
        );
        let after = get_feed(&router, label, &id).await;
        assert_eq!(
            after, before,
            "case {label:?}: the refused request left the stored feed as it was ({before}), \
             got {after}"
        );
    }

    sentences.dedup();
    assert_eq!(
        sentences.len(),
        1,
        "the rule is about the shape of the request, so every row answers one sentence, got \
         {sentences:?}"
    );
}

/// The control: a list carrying one output or more is SERVED by each of those operations,
/// and what it supplies is what the feed goes on to hold — so the refusal above is keyed
/// to the cardinality of the list, not to the list.
#[tokio::test]
async fn a_supplied_output_list_carrying_one_output_or_more_is_served() {
    const HELD: usize = 1;

    for supplier in [
        Supplier::Create,
        Supplier::Update,
        Supplier::Associate { dry_run: false },
        Supplier::Associate { dry_run: true },
    ] {
        // The counts differ from one to as many as the operation may leave the feed
        // holding: a replacement is bounded by its own length, an append by the stored
        // length plus what it adds (`MAX_OUTPUTS_PER_FEED`).
        let counts: [usize; 3] = match supplier {
            Supplier::Associate { .. } => [1, 2, MAX_OUTPUTS_PER_FEED - HELD],
            Supplier::Create | Supplier::Update => [1, 2, MAX_OUTPUTS_PER_FEED],
        };

        for count in counts {
            let label = format!("{supplier:?}: a list of {count} outputs");
            let router = router();
            let id = stored_feed(&router, &label, HELD, false).await;
            let supplied = fresh_outputs(count);
            let supplied_names: Vec<String> =
                (0..count).map(|index| format!("fresh{index}")).collect();

            let (status, body) = supply(&router, supplier, &id, &supplied, &json!({})).await;
            assert_eq!(
                status,
                supplier.accepted_status(),
                "case {label:?}: expected HTTP {}, got {status} with {body}",
                supplier.accepted_status()
            );

            // What the stored feed holds afterwards: the supplied list for an operation
            // that replaces it, the stored one followed by the supplied for an append, and
            // the stored one untouched for a dry run.
            let (read_id, expected) = match supplier {
                Supplier::Create => (
                    body["id"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {label:?}: a minted feed carries an id, got {body}")
                        })
                        .to_string(),
                    supplied_names.clone(),
                ),
                Supplier::Update => (id.clone(), supplied_names.clone()),
                Supplier::Associate { dry_run: false } => (
                    id.clone(),
                    held_names(HELD)
                        .into_iter()
                        .chain(supplied_names.clone())
                        .collect(),
                ),
                Supplier::Associate { dry_run: true } => (id.clone(), held_names(HELD)),
            };
            let feed = get_feed(&router, &label, &read_id).await;
            assert_eq!(
                output_names(&feed),
                expected,
                "case {label:?}: expected the feed to hold {expected:?}, got {:?}",
                feed["outputs"]
            );
        }
    }
}
