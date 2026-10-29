// SPDX-License-Identifier: CC0-1.0

//! The ONE positional-cursor rule every listing of this service that pages over
//! a DERIVED sequence walks by, driven end to end through each of those
//! listings.
//!
//! The rule: a listing whose entries carry nothing that identifies them on their
//! own resumes at a POSITION in the sequence it reports — an absent `NextToken`
//! is the first page, a token that is not a whole number continues nothing the
//! service handed out and is refused with the listing's own modelled 400, a token
//! is handed out exactly while an entry is left over after the page, and a
//! position past the end is a valid walk of a sequence that has since shrunk, so
//! it answers an empty last page. It is the SERVICE's rule, not one listing's, so
//! the table below spans the OPERATION: `ListAlerts`, `ListClusterAlerts` and
//! `DescribeSchedule` each run every row.
//!
//! `tests/all/channel_alerts.rs`, `tests/all/cluster_alerts.rs` and
//! `tests/all/channel_schedule.rs` pin what each of those listings REPORTS;
//! `tests/all/shared_page_size_rule.rs` pins the page-size rule beside this one.
//! This suite pins that all three reach the same cursor rule — which page a token
//! resumes at, which token is refused and when a token is handed out at all —
//! whichever listing a client asks.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{service_state_in, ClusterAlertRecord};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive_with_engine, DEFAULT_REGION};

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape", shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// The sequence a row of the table pages over, and so how this suite fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sequence {
    /// A channel's alert log, in the order the service set the alerts.
    ChannelAlerts,
    /// A cluster's alert log, oldest first.
    ClusterAlerts,
    /// A channel's schedule, in the order its actions start.
    Schedule,
}

/// One row of the table: a listing that pages by position, the sequence it
/// reports and how a client reads that sequence back.
struct Positional {
    /// The operation the row drives, as the model names it.
    op: &'static str,
    /// The sequence it pages over.
    sequence: Sequence,
    /// Its URI, from `smithy_query(query="operation", operation="<Operation>")`,
    /// built from the id of the resource the sequence hangs off.
    uri: fn(&str) -> String,
    /// The member it reports its page in (the `jsonName` of the operation's
    /// output list member).
    member: &'static str,
    /// The member of one reported entry this suite names that entry by.
    identity: &'static str,
}

/// Every listing of this service that resumes at a position rather than at an
/// identifier: `smithy_query(query="operation", operation="ListAlerts,
/// ListClusterAlerts,DescribeSchedule")` for the method, the URI and the output
/// member of each.
fn positional_listings() -> Vec<Positional> {
    vec![
        Positional {
            op: "ListAlerts",
            sequence: Sequence::ChannelAlerts,
            uri: |channel| format!("/prod/channels/{channel}/alerts"),
            member: "alerts",
            identity: "alertType",
        },
        Positional {
            op: "ListClusterAlerts",
            sequence: Sequence::ClusterAlerts,
            uri: |cluster| format!("/prod/clusters/{cluster}/alerts"),
            member: "alerts",
            identity: "alertType",
        },
        Positional {
            op: "DescribeSchedule",
            sequence: Sequence::Schedule,
            uri: |channel| format!("/prod/channels/{channel}/schedule"),
            member: "scheduleActions",
            identity: "actionName",
        },
    ]
}

/// How many entries this suite fills each sequence with — more than one page at
/// every size the walk below asks for, so a row that reported its whole sequence
/// whatever the token fails.
const ENTRIES: usize = 5;

/// The names the entries of a filled sequence carry, in the order the listing
/// must report them.
fn entry_names() -> Vec<String> {
    (0..ENTRIES).map(|index| format!("entry-{index}")).collect()
}

/// An instant a fixed number of seconds after the Unix epoch, so the order a
/// seeded log is read back in is the order this suite chose and not the wall
/// clock's.
fn at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset)
}

/// A created channel, answering its minted id.
async fn channel(router: &MultiServiceRouter, case: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "position-cursor-rule",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateChannel to answer 201, got {status}: {created}"
    );
    created["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the created channel to carry an id"))
        .to_string()
}

/// A created cluster, answering its minted id.
async fn cluster(router: &MultiServiceRouter, case: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/clusters",
        Some(json!({ "name": "position-cursor-rule", "clusterType": ON_PREMISES })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCluster to answer 201, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the created cluster to carry an id"))
        .to_string()
}

/// The resource one row's sequence hangs off, created on `router` with the
/// sequence filled with [`entry_names`] in that order — answering its id.
///
/// An alert is the SERVICE's observation of a condition and nothing in this
/// emulator ingests media or runs encoding hardware, so the two alert logs are
/// seeded through the store's own writers (the only way an alert enters a log)
/// rather than by asking an operation to invent alerts. The schedule is filled
/// the way a client fills it, through `BatchUpdateSchedule`.
async fn filled_sequence(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Positional,
) -> String {
    let case = row.op;
    match row.sequence {
        Sequence::ChannelAlerts => {
            let channel_id = channel(router, case).await;
            let store = service_state_in(&**engine, DEFAULT_REGION).expect("the medialive store");
            let mut store = store.lock();
            let channel = store
                .channels
                .get_mut(&channel_id)
                .unwrap_or_else(|| panic!("case {case:?}: the created channel must be in the store"));
            for (index, name) in entry_names().into_iter().enumerate() {
                assert!(
                    channel
                        .raise_alert(&name, None, None, at(index as u64))
                        .is_some(),
                    "case {case:?}: expected the alert {name:?} to be recorded"
                );
            }
            channel_id
        }
        Sequence::ClusterAlerts => {
            let cluster_id = cluster(router, case).await;
            let store = service_state_in(&**engine, DEFAULT_REGION).expect("the medialive store");
            let mut store = store.lock();
            for (index, name) in entry_names().into_iter().enumerate() {
                assert!(
                    store.raise_cluster_alert(
                        &cluster_id,
                        ClusterAlertRecord::raised(name.clone(), at(index as u64)),
                    ),
                    "case {case:?}: expected the cluster to take the alert {name:?}"
                );
            }
            cluster_id
        }
        Sequence::Schedule => {
            let channel_id = channel(router, case).await;
            // Fixed start times, ascending, so the order the schedule reports is
            // the order this suite seeded — and far enough in the future that
            // nothing sweeps an action out between two pages.
            let actions: Vec<Value> = entry_names()
                .into_iter()
                .enumerate()
                .map(|(index, name)| {
                    json!({
                        "actionName": name,
                        "scheduleActionSettings": {
                            "scte35ReturnToNetworkSettings": { "spliceEventId": index + 1 }
                        },
                        "scheduleActionStartSettings": {
                            "fixedModeScheduleActionStartSettings": {
                                "time": format!("20{}0-01-01T00:00:00Z", 3 + index)
                            }
                        },
                    })
                })
                .collect();
            let (status, body) = call(
                router,
                "PUT",
                &format!("/prod/channels/{channel_id}/schedule"),
                Some(json!({ "creates": { "scheduleActions": actions } })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected BatchUpdateSchedule to answer 200, got {status}: {body}"
            );
            channel_id
        }
    }
}

/// One page of a row's listing: the entries it reported, named by the row's
/// identity member, and the token beside them.
async fn page(
    router: &MultiServiceRouter,
    row: &Positional,
    owner: &str,
    case: &str,
    query: &[(&str, String)],
) -> (Vec<String>, Option<String>) {
    let (status, body) = answer(router, row, owner, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing to be served, got {status}: {body}"
    );
    (reported(case, row, &body), token(&body))
}

/// A row's listing called with `query` appended to its URI.
async fn answer(
    router: &MultiServiceRouter,
    row: &Positional,
    owner: &str,
    query: &[(&str, String)],
) -> (u16, Value) {
    let path = (row.uri)(owner);
    let uri = if query.is_empty() {
        path
    } else {
        let encoded: Vec<String> = query
            .iter()
            // Percent-encoded, so a token carrying a character the query string
            // gives its own meaning reaches the handler as the request named it.
            .map(|(name, value)| format!("{name}={}", percent_encoded(value)))
            .collect();
        format!("{path}?{}", encoded.join("&"))
    };
    call(router, "GET", &uri, None).await
}

/// The entries a page reported, named by the row's identity member: an absent
/// member and an empty list both mean "no entries" to a client, and which of the
/// two the restJson1 codec writes is the serializer's choice.
fn reported(case: &str, row: &Positional, body: &Value) -> Vec<String> {
    let entries = match body.get(row.member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a {} list, got {other}", row.member),
    };
    entries
        .iter()
        .map(|entry| {
            entry[row.identity]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected each entry to carry {}, got {entry}",
                        row.identity
                    )
                })
                .to_string()
        })
        .collect()
}

/// The continuation token a page carried, if it carried one.
fn token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The tokens that are not positions at all, spanning the ways a string can fail
/// to be a whole number: a word, a negative number, a fraction, scientific and
/// hexadecimal notation, a number with whitespace around it, a list of two
/// positions, and a number too large for any sequence to be that long.
const NOT_POSITIONS: [&str; 9] = [
    "halfway",
    "-1",
    "1.5",
    "1e2",
    "0x1",
    " 1",
    "1 ",
    "1,2",
    "99999999999999999999999999999999",
];

/// Rule: the positional cursor is the SERVICE's, so every listing that pages
/// over a derived sequence resumes by the same rule — no token is the first page,
/// a token is the number of entries already reported and resumes exactly there,
/// a token is handed out only while an entry is left over, a token that is not a
/// whole number is refused with the modelled 400, and a position past the end
/// answers an empty last page with no token.
///
/// The rows span the OPERATION — all three positional listings, over three
/// unrelated sequences (two alert logs, one schedule) derived in three different
/// orders — and, within each, the page size the walk is cut at, every position
/// inside the sequence, three positions past its end and nine tokens that are no
/// position at all.
#[tokio::test]
async fn one_position_cursor_rule_for_every_positional_listing() {
    let names = entry_names();
    // Per listing: the refusal it answered for each token of [`NOT_POSITIONS`],
    // in that order.
    let mut refusals: Vec<(&'static str, Vec<String>)> = Vec::new();

    for row in positional_listings() {
        // A router of its own per row, so each listing is driven against a store
        // only this row filled.
        let (router, engine) = medialive_with_engine();
        let owner = filled_sequence(&router, &engine, &row).await;

        // No token at all: the first page, which at the service's default page
        // size is the whole sequence and so carries no continuation.
        let case = format!("{} no token", row.op);
        let (whole, handed_out) = page(&router, &row, &owner, &case, &[]).await;
        assert_eq!(
            whole, names,
            "case {case:?}: expected the whole sequence in order, got {whole:?}"
        );
        assert_eq!(
            handed_out, None,
            "case {case:?}: expected no token beside a page holding every entry, got \
             {handed_out:?}"
        );

        // A token resumes exactly at the position it names, wherever in the
        // sequence that is — including its first position, which is the first
        // page again.
        for position in 0..=names.len() {
            let case = format!("{} nextToken={position}", row.op);
            let (resumed, handed_out) = page(
                &router,
                &row,
                &owner,
                &case,
                &[("nextToken", position.to_string())],
            )
            .await;
            assert_eq!(
                resumed,
                names[position..],
                "case {case:?}: expected the sequence from position {position} on, got {resumed:?}"
            );
            assert_eq!(
                handed_out, None,
                "case {case:?}: expected no token beside a page reaching the end, got \
                 {handed_out:?}"
            );
        }

        // Walked a page at a time: the pages are the sequence, each is cut at the
        // size asked for while entries are left over, and the token is handed out
        // exactly while they are.
        for page_size in [1_usize, 2, 3, ENTRIES, ENTRIES + 2] {
            let case = format!("{} maxResults={page_size}", row.op);
            let mut walked: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut query = vec![("maxResults", page_size.to_string())];
                if let Some(cursor) = cursor.as_deref() {
                    query.push(("nextToken", cursor.to_string()));
                }
                let (entries, handed_out) = page(&router, &row, &owner, &case, &query).await;
                assert!(
                    entries.len() <= page_size,
                    "case {case:?}: expected at most {page_size} entries in a page, got \
                     {entries:?}"
                );
                walked.extend(entries);
                // A token is handed out exactly while an entry is left over
                // after what has been reported so far, and it is that count.
                let left_over = walked.len() < names.len();
                assert_eq!(
                    handed_out.is_some(),
                    left_over,
                    "case {case:?}: after {} of {} entries, expected a token: {left_over}, got \
                     {handed_out:?}",
                    walked.len(),
                    names.len()
                );
                if let Some(handed_out) = handed_out.as_deref() {
                    assert_eq!(
                        handed_out,
                        walked.len().to_string(),
                        "case {case:?}: expected the token to be the number of entries reported \
                         so far ({}), got {handed_out:?}",
                        walked.len()
                    );
                }
                cursor = handed_out;
                pages += 1;
                assert!(
                    pages <= names.len() + 1,
                    "case {case:?}: pagination did not terminate, walked {walked:?}"
                );
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(
                walked, names,
                "case {case:?}: expected the pages to walk the whole sequence in order, got \
                 {walked:?}"
            );
        }

        // A position past the end is a walk of a sequence that has shrunk since
        // the token was handed out, not a bad request: the page is empty and
        // ends the walk.
        for position in [names.len() + 1, names.len() + 2, 999] {
            let case = format!("{} nextToken={position} past the end", row.op);
            let (entries, handed_out) = page(
                &router,
                &row,
                &owner,
                &case,
                &[("nextToken", position.to_string())],
            )
            .await;
            assert!(
                entries.is_empty(),
                "case {case:?}: expected an empty last page, got {entries:?}"
            );
            assert_eq!(
                handed_out, None,
                "case {case:?}: expected an empty last page to hand out no token, got \
                 {handed_out:?}"
            );
        }

        // Anything that is not a whole-number position: the modelled 400, with
        // the one wording the shared rule states, and nothing reported.
        let mut answered: Vec<String> = Vec::new();
        for asked_for in NOT_POSITIONS {
            let case = format!("{} nextToken={asked_for:?}", row.op);
            let (status, body) =
                answer(&router, &row, &owner, &[("nextToken", asked_for.to_string())]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a token that is not a position to be refused, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected the modelled client error, got {body}"
            );
            let message = error_message(&body);
            let opening = format!("nextToken {asked_for} is not ");
            assert!(
                message.starts_with(&opening) && message.ends_with(" position"),
                "case {case:?}: expected the shared refusal to name the token and the position \
                 it is not ({opening:?}… \" position\"), got {message:?}"
            );
            assert!(
                reported(&case, &row, &body).is_empty(),
                "case {case:?}: expected a refusal to report no entries, got {body}"
            );
            answered.push(message);
        }
        refusals.push((row.op, answered));
    }

    // Every positional listing of this service ran the rows above, which is what
    // makes this suite a statement about the SERVICE's rule and not one
    // listing's.
    assert_eq!(
        refusals.len(),
        3,
        "case \"every positional listing\": expected all three listings to have run, got \
         {refusals:?}"
    );
    for (op, answered) in &refusals {
        assert_eq!(
            answered.len(),
            NOT_POSITIONS.len(),
            "case {op:?}: expected every token of {NOT_POSITIONS:?} to have been refused, got \
             {answered:?}"
        );
    }
}
