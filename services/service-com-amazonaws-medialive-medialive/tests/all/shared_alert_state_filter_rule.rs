// SPDX-License-Identifier: CC0-1.0

//! The ONE `StateFilter` membership rule every alert listing of this service
//! reads its filter through, driven end to end through each of those listings.
//!
//! The rule: an alert listing serves a request whose `StateFilter` is one of the
//! values the member documents, and a request naming none (which narrows
//! nothing), and refuses any other value with its own modelled 400 carrying the
//! one sentence that names the member, the accepted set and the value the
//! request carried. It is the SERVICE's rule, not one listing's — all three
//! alert surfaces admit their alerts out of one vocabulary — so the table below
//! spans the OPERATION: `ListAlerts`, `ListClusterAlerts` and
//! `ListMultiplexAlerts` each run every row, against the one shared sentence.
//!
//! `tests/all/channel_alerts.rs`, `tests/all/cluster_alerts.rs` and
//! `tests/all/multiplex_alerts.rs` pin what each listing REPORTS for a filter
//! against a filled log; this suite pins that each of them reaches the same
//! membership rule — the accepted set and its wording — whichever listing a
//! client asks.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    ALERT_STATE_FILTERS, CHANNEL_ALERT_FILTERS, CLUSTER_ALERT_FILTERS,
    MULTIPLEX_ALERT_STATE_FILTERS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_lifecycle::{
    create_body, settings, TRANSPORT_STREAM_BITRATE, TRANSPORT_STREAM_ID,
};
use crate::wire::{call, error_code, error_message, medialive};

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape", shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// The resource an alert listing hangs off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Channel,
    Cluster,
    Multiplex,
}

/// One row of the table: an alert listing, the resource its URI hangs off and
/// how it builds that URI from the owner's id.
struct AlertListing {
    /// The operation the row drives, as the model names it.
    op: &'static str,
    /// The resource its alerts belong to.
    owner: Owner,
    /// Its URI, from `smithy_query(query="operation", operation="<Operation>")`,
    /// where `StateFilter` is an `@httpQuery` member on all three.
    uri: fn(&str) -> String,
}

/// Every alert listing this service serves, each with the resource whose log it
/// reports: `smithy_query(query="operation", operation="ListAlerts,
/// ListClusterAlerts,ListMultiplexAlerts")` for the method and the URI of each.
fn alert_listings() -> Vec<AlertListing> {
    vec![
        AlertListing {
            op: "ListAlerts",
            owner: Owner::Channel,
            uri: |channel| format!("/prod/channels/{channel}/alerts"),
        },
        AlertListing {
            op: "ListClusterAlerts",
            owner: Owner::Cluster,
            uri: |cluster| format!("/prod/clusters/{cluster}/alerts"),
        },
        AlertListing {
            op: "ListMultiplexAlerts",
            owner: Owner::Multiplex,
            uri: |multiplex| format!("/prod/multiplexes/{multiplex}/alerts"),
        },
    ]
}

/// The owner a row's listing hangs off, created on `router`, answering its
/// minted id.
async fn owner_id(router: &MultiServiceRouter, case: &str, owner: Owner) -> String {
    let (uri, body, carrier) = match owner {
        Owner::Channel => (
            "/prod/channels",
            json!({
                "name": "alert-filter-rule",
                "encoderSettings": crate::support::channel_encoder_settings(),
            }),
            "channel",
        ),
        Owner::Cluster => (
            "/prod/clusters",
            json!({ "name": "alert-filter-rule", "clusterType": ON_PREMISES }),
            "",
        ),
        Owner::Multiplex => (
            "/prod/multiplexes",
            create_body(
                "alert-filter-rule",
                "alert-filter-rule-token",
                settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0),
            ),
            "multiplex",
        ),
    };
    let (status, response) = call(router, "POST", uri, Some(body)).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected the {owner:?} this listing hangs off to be created, got \
         {status}: {response}"
    );
    let resource = if carrier.is_empty() {
        response.clone()
    } else {
        response[carrier].clone()
    };
    resource["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the created {owner:?} to carry an id, got {response}")
        })
        .to_string()
}

/// The `alerts` member of a listing's response as a list, whatever its length:
/// an absent member and an empty list both mean "no alerts" to a client, and
/// which of the two the restJson1 codec writes is the serializer's choice.
fn reported(case: &str, body: &Value) -> Vec<Value> {
    match body.get("alerts") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(alerts)) => alerts.clone(),
        Some(other) => panic!("case {case:?}: expected an alerts list, got {other}"),
    }
}

/// The URI one row drives, with `stateFilter` set to `asked_for` when the row
/// names a filter at all.
fn uri_of(row: &AlertListing, owner: &str, asked_for: Option<&str>) -> String {
    let path = (row.uri)(owner);
    match asked_for {
        // Percent-encoded, so a value carrying a character the query string
        // gives its own meaning (a space, a comma) reaches the handler as the
        // request named it.
        Some(filter) => format!("{path}?stateFilter={}", crate::support::percent_encoded(filter)),
        None => path,
    }
}

/// The values that fall OUTSIDE the documented set, spanning the ways one can:
/// each documented value in the wrong case (an enum value in this API is
/// case-sensitive), one with trailing whitespace, a near-miss word, two
/// documented values at once, the empty string, and a resource state that
/// belongs to the resource rather than to its alerts.
const UNDOCUMENTED: [&str; 9] = [
    "set",
    "Cleared",
    "all",
    "SET ",
    "ANY",
    "SET,CLEARED",
    "",
    "RUNNING",
    "CREATING",
];

/// Rule: the accepted `StateFilter` set is the SERVICE's, so every alert listing
/// reads its filter by the same rule — each documented value and an absent
/// member are served, and any other value is refused with the modelled 400
/// carrying the one sentence that names the member, the accepted set and the
/// offending value.
///
/// The rows span the OPERATION — all three alert listings, over three unrelated
/// resources, stores and owner ids — and, within each, the filter asked for:
/// every documented value, no filter at all, and nine values outside the set.
/// The accepted set every refusal names is the documented vocabulary
/// (`ALERT_STATE_FILTERS`), so a listing that drifted into a set of its own
/// fails this suite.
#[tokio::test]
async fn one_state_filter_membership_rule_for_every_alert_listing() {
    // The accepted set each listing refuses against is one constant: the aliases
    // the three resource slices expose are that same set, which is why one rule
    // can serve all three.
    for (alias, values) in [
        ("CHANNEL_ALERT_FILTERS", CHANNEL_ALERT_FILTERS),
        ("CLUSTER_ALERT_FILTERS", CLUSTER_ALERT_FILTERS),
        ("MULTIPLEX_ALERT_STATE_FILTERS", MULTIPLEX_ALERT_STATE_FILTERS),
    ] {
        assert_eq!(
            values, ALERT_STATE_FILTERS,
            "case {alias:?}: expected the alias to be the one documented filter set \
             {ALERT_STATE_FILTERS:?}, got {values:?}"
        );
    }

    // The sentence every alert listing refuses an undocumented filter with, up
    // to the offending value it ends in: the member, the rule and the accepted
    // set, which is the documented vocabulary and not one listing's own.
    let sentence = format!(
        "stateFilter must be one of {}, got",
        ALERT_STATE_FILTERS.join(", ")
    );

    // Per listing: the refusal it answered for each value of [`UNDOCUMENTED`],
    // in that order.
    let mut refusals: Vec<(&'static str, Vec<String>)> = Vec::new();

    for row in alert_listings() {
        // A router of its own per row, so each listing is driven against a store
        // only this row filled.
        let router = medialive();
        let owner = owner_id(&router, row.op, row.owner).await;

        // Every documented value, and no filter at all: served.
        for asked_for in ALERT_STATE_FILTERS.map(Some).into_iter().chain([None]) {
            let case = format!("{} stateFilter={asked_for:?}", row.op);
            let uri = uri_of(&row, &owner, asked_for);
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GET {uri} to serve a documented stateFilter, got \
                 {status}: {body}"
            );
            // The owner was created through the API and this emulator observes
            // no alert condition, so the log it reports is empty — the store's
            // answer, which the suites named in this module's docs assert
            // against a filled log.
            assert!(
                reported(&case, &body).is_empty(),
                "case {case:?}: expected a listing of a log nothing has written to to report no \
                 alerts, got {body}"
            );
        }

        // Anything else: the modelled 400, with the shared wording.
        let mut answered: Vec<String> = Vec::new();
        for asked_for in UNDOCUMENTED {
            let case = format!("{} stateFilter={asked_for:?}", row.op);
            let uri = uri_of(&row, &owner, Some(asked_for));
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected GET {uri} to refuse a stateFilter outside \
                 {ALERT_STATE_FILTERS:?}, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected the modelled client error, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.starts_with(&sentence),
                "case {case:?}: expected the refusal to carry the shared sentence \
                 {sentence:?}, got {message:?}"
            );
            assert!(
                message[sentence.len()..].contains(asked_for),
                "case {case:?}: expected the refusal to end in the value the request carried, \
                 got {message:?}"
            );
            assert!(
                reported(&case, &body).is_empty(),
                "case {case:?}: expected a refusal to report no alerts, got {body}"
            );
            answered.push(message);
        }
        refusals.push((row.op, answered));
    }

    // Every alert surface of this service ran the rows above, which is what
    // makes this suite a statement about the SERVICE's rule and not one
    // listing's.
    assert_eq!(
        refusals.len(),
        3,
        "case \"every alert listing\": expected all three listings to have run, got {refusals:?}"
    );
    for (op, answered) in &refusals {
        assert_eq!(
            answered.len(),
            UNDOCUMENTED.len(),
            "case {op:?}: expected every value of {UNDOCUMENTED:?} to have been refused, got \
             {answered:?}"
        );
    }
}
