// SPDX-License-Identifier: CC0-1.0

//! The filter rules the paginated listings of this service SHARE, driven through one
//! operation per listing.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! Two of a listing's filters are the same rule in every listing that carries them, so
//! each is asserted over a TABLE whose rows are the listings themselves, each with its own
//! records and its own stored instants:
//!
//! 1. `createdBefore` / `createdAfter` bound the stored `createdAt` STRICTLY, so the two
//!    together name an OPEN interval: a bound keeps all, some or none of the records, and
//!    the record created exactly at a bound is never one of them. Asserted for all five
//!    paginated listings — `ListVirtualClusters`, `ListJobRuns`, `ListJobTemplates`,
//!    `ListSecurityConfigurations` and `ListManagedEndpoints`.
//! 2. `states` keeps the records whose stored state is one of the states the request
//!    listed, and a list of no states keeps everything. Asserted for the three listings
//!    the model gives a `states` member — `ListVirtualClusters`, `ListJobRuns` and
//!    `ListManagedEndpoints`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::{
    ENDPOINT_STATE_VALUES, JOB_RUN_STATE_VALUES, VIRTUAL_CLUSTER_STATE_VALUES,
};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{create_endpoint, endpoint_body};
use crate::virtual_cluster_job_run_loop::{
    field, instant_millis, query_instant, text, Answer, Emr,
};
use crate::virtual_cluster_scheduler_status::run_id;

/// The paginated listings of this service: one row per operation that filters a listing.
#[derive(Clone, Copy, Debug)]
enum Which {
    VirtualClusters,
    JobRuns,
    JobTemplates,
    SecurityConfigurations,
    ManagedEndpoints,
}

/// Every paginated listing, for the bound rule they all apply.
const EVERY_LISTING: [Which; 5] = [
    Which::VirtualClusters,
    Which::JobRuns,
    Which::JobTemplates,
    Which::SecurityConfigurations,
    Which::ManagedEndpoints,
];

/// The listings whose input carries a `states` member (smithy_query: operations
/// `ListVirtualClusters`, `ListJobRuns`, `ListManagedEndpoints`); `ListJobTemplates` and
/// `ListSecurityConfigurations` have none.
const LISTINGS_THAT_FILTER_BY_STATE: [Which; 3] = [
    Which::VirtualClusters,
    Which::JobRuns,
    Which::ManagedEndpoints,
];

/// One listing mounted on a fresh emulator: where it is served, which output member its
/// records come back in, the state catalog its `states` filter draws from, and which
/// reported states a record does not leave on its own.
struct Listing {
    which: Which,
    label: &'static str,
    /// The request path, one query string away from a call.
    path: String,
    /// The JSON pointer of the output member the records come back in.
    member: &'static str,
    /// The cluster the per-cluster listings are served under, for the calls that retire
    /// one of their records.
    cluster: Option<String>,
    /// The generated catalog of the states this listing filters by, or `None` for a
    /// listing the model gives no `states` member.
    state_catalog: Option<&'static [&'static str]>,
    /// Whether a reported state is one the clock does not move a record out of, so a
    /// fixture that reports only such states has stable states to filter by.
    settled: fn(&str) -> bool,
    /// Whether one of this listing's records can be sent to a terminal state, so the
    /// fixture holds records in more than one state. `false` for the endpoint listing,
    /// whose delete is still scaffolded and whose records therefore all settle in the same
    /// state; its rows discriminate between the state they are in and one no record is in.
    retires_a_record: bool,
}

/// The smallest `jobTemplateData` the model accepts: `executionRoleArn`, `releaseLabel`
/// and `jobDriver` are all `@required` (smithy_query: shape `JobTemplateData`).
fn minimal_template_data() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.15.0-latest",
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/filters.py"}},
    })
}

/// A virtual cluster never leaves `RUNNING` or a terminal state by itself; `TERMINATING`
/// is the step the clock still owes.
fn cluster_state_is_settled(state: &str) -> bool {
    state == state::CLUSTER_RUNNING || state == state::CLUSTER_TERMINATED
}

/// A job run is settled once it has reached a terminal state: this service owes a run it
/// admitted its outcome, so `PENDING` and `RUNNING` are both on the way somewhere.
fn job_state_is_settled(state: &str) -> bool {
    state::job_state_is_terminal(state)
}

/// A managed endpoint never leaves `ACTIVE` or a terminal state by itself.
fn endpoint_state_is_settled(state: &str) -> bool {
    state == state::ENDPOINT_ACTIVE || state::endpoint_state_is_terminal(state)
}

/// A fresh emulator and the listing under test, with the virtual cluster the per-cluster
/// listings are served under already created.
async fn mount(which: Which) -> (Emr, Listing) {
    let emr = Emr::new();
    let listing = match which {
        Which::VirtualClusters => Listing {
            which,
            label: "ListVirtualClusters",
            path: "/virtualclusters".to_string(),
            member: "/virtualClusters",
            cluster: None,
            state_catalog: Some(VIRTUAL_CLUSTER_STATE_VALUES),
            settled: cluster_state_is_settled,
            retires_a_record: true,
        },
        Which::JobRuns => {
            let cluster = emr.cluster("filter-runs", "tok-filter-runs").await;
            Listing {
                which,
                label: "ListJobRuns",
                path: format!("/virtualclusters/{cluster}/jobruns"),
                member: "/jobRuns",
                cluster: Some(cluster),
                state_catalog: Some(JOB_RUN_STATE_VALUES),
                settled: job_state_is_settled,
                retires_a_record: true,
            }
        }
        Which::JobTemplates => Listing {
            which,
            label: "ListJobTemplates",
            path: "/jobtemplates".to_string(),
            member: "/templates",
            cluster: None,
            state_catalog: None,
            settled: |_| true,
            retires_a_record: false,
        },
        Which::SecurityConfigurations => Listing {
            which,
            label: "ListSecurityConfigurations",
            path: "/securityconfigurations".to_string(),
            member: "/securityConfigurations",
            cluster: None,
            state_catalog: None,
            settled: |_| true,
            retires_a_record: false,
        },
        Which::ManagedEndpoints => {
            let cluster = emr
                .cluster("filter-endpoints", "tok-filter-endpoints")
                .await;
            Listing {
                which,
                label: "ListManagedEndpoints",
                path: format!("/virtualclusters/{cluster}/endpoints"),
                member: "/endpoints",
                cluster: Some(cluster),
                state_catalog: Some(ENDPOINT_STATE_VALUES),
                settled: endpoint_state_is_settled,
                retires_a_record: false,
            }
        }
    };
    (emr, listing)
}

/// The cluster a per-cluster listing is served under.
fn parent_cluster(listing: &Listing) -> &str {
    listing
        .cluster
        .as_deref()
        .unwrap_or_else(|| panic!("case {:?}: this listing is served under a cluster", listing.label))
}

/// Store `count` records of this listing, tagged so two groups cannot collide, and return
/// their ids in creation order.
///
/// Successive records are created a few milliseconds apart, so no two share a stored
/// `createdAt` millisecond and a bound taken from one record's own instant names exactly
/// that record.
async fn store(emr: &Emr, listing: &Listing, tag: &str, count: usize) -> Vec<String> {
    let mut stored = Vec::new();
    for index in 0..count {
        let name = format!("filter-{tag}-{index}");
        let token = format!("tok-filter-{tag}-{index}");
        let id = match listing.which {
            Which::VirtualClusters => emr.cluster(&name, &token).await,
            Which::JobRuns => run_id(emr, listing.label, parent_cluster(listing), &token).await,
            Which::JobTemplates => {
                let created = emr
                    .send(
                        "POST",
                        "/jobtemplates",
                        Some(json!({
                            "name": name,
                            "clientToken": token,
                            "jobTemplateData": minimal_template_data(),
                        })),
                    )
                    .await;
                text(created.ok(listing.label), "/id", listing.label)
            }
            Which::SecurityConfigurations => {
                let created = emr
                    .send(
                        "POST",
                        "/securityconfigurations",
                        Some(json!({
                            "name": name,
                            "clientToken": token,
                            "securityConfigurationData": {},
                        })),
                    )
                    .await;
                text(created.ok(listing.label), "/id", listing.label)
            }
            Which::ManagedEndpoints => {
                let created = create_endpoint(
                    emr,
                    parent_cluster(listing),
                    endpoint_body(&name, &token, "JUPYTER_ENTERPRISE_GATEWAY"),
                )
                .await;
                text(created.ok(listing.label), "/id", listing.label)
            }
        };
        stored.push(id);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    stored
}

/// Send one of this listing's records to a terminal state, so the fixture holds records in
/// more than one state and a `states` filter has something to discriminate.
async fn retire(emr: &Emr, listing: &Listing, id: &str) {
    let answer = match listing.which {
        Which::VirtualClusters => emr.send("DELETE", &format!("/virtualclusters/{id}"), None).await,
        Which::JobRuns => {
            emr.send(
                "DELETE",
                &format!("/virtualclusters/{}/jobruns/{id}", parent_cluster(listing)),
                None,
            )
            .await
        }
        Which::JobTemplates | Which::SecurityConfigurations | Which::ManagedEndpoints => {
            panic!(
                "case {:?}: this listing retires none of its records",
                listing.label
            )
        }
    };
    answer.ok(&format!("{} / retiring {id}", listing.label));
}

/// One call to a listing with `query` appended to its path.
async fn list(emr: &Emr, listing: &Listing, query: &str) -> Answer {
    emr.send("GET", &format!("{}{query}", listing.path), None)
        .await
}

/// The records a page carries, in the order it reported them.
fn listed(body: &Value, listing: &Listing, case: &str) -> Vec<Value> {
    field(body, listing.member, case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {} is not a list: {body}", listing.member))
        .clone()
}

/// The ids a listing reported, as a set.
fn listed_set(body: &Value, listing: &Listing, case: &str) -> BTreeSet<String> {
    listed(body, listing, case)
        .iter()
        .map(|record| text(record, "/id", case))
        .collect()
}

/// The `createdAt` instant the listing reports for each record it holds.
async fn stored_instants(emr: &Emr, listing: &Listing) -> BTreeMap<String, i64> {
    let label = listing.label;
    let body = list(emr, listing, "").await.ok(label).clone();
    listed(&body, listing, label)
        .iter()
        .map(|record| {
            (
                text(record, "/id", label),
                instant_millis(record, "/createdAt", label),
            )
        })
        .collect()
}

/// The state the listing reports for each record it holds, once every one of them is
/// settled: the service stores the due instant of a transition and never sleeps, so this
/// polls the listing rather than guessing a span.
async fn settled_states(emr: &Emr, listing: &Listing) -> BTreeMap<String, String> {
    let label = listing.label;
    // Generous enough for a job run's whole walk (admission plus the outcome this service
    // owes it), which is the longest of the three.
    let budget = state::JOB_OUTCOME_SPAN + state::TRANSITION_SPAN * 4 + Duration::from_secs(2);
    let deadline = std::time::Instant::now() + budget;
    loop {
        let body = list(emr, listing, "").await.ok(label).clone();
        let reported: BTreeMap<String, String> = listed(&body, listing, label)
            .iter()
            .map(|record| (text(record, "/id", label), text(record, "/state", label)))
            .collect();
        if reported.values().all(|state| (listing.settled)(state)) {
            return reported;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {label:?}: expected every record's state to settle within {budget:?}, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// RULE 1: every paginated listing bounds the stored `createdAt` by one shared rule — the
/// `createdAfter` / `createdBefore` window is OPEN at both ends.
///
/// The fixture stores one group, lets the clock move past it, takes a MIDPOINT, lets the
/// clock move again and stores a second group, so a bound between the groups has records
/// on either side of it. The rows differ along which bound is applied, whether it keeps
/// all, some or none of the records, and whether it falls between the groups or exactly on
/// one record's own instant.
#[tokio::test]
async fn every_paginated_listing_bounds_created_at_with_one_shared_open_window() {
    // Wider than the one-second resolution an ISO-8601 instant can be rendered at, so a
    // bound taken between the two groups stays between them however the wire renders it.
    let gap = Duration::from_millis(1_100);

    for which in EVERY_LISTING {
        let (emr, listing) = mount(which).await;
        let label = listing.label;

        let before_everything = state::now_millis() - 2_000;
        let early = store(&emr, &listing, "early", 2).await;
        tokio::time::sleep(gap).await;
        let midpoint = state::now_millis();
        tokio::time::sleep(gap).await;
        let late = store(&emr, &listing, "late", 3).await;
        let after_everything = state::now_millis() + 2_000;

        let instants = stored_instants(&emr, &listing).await;
        let stored: Vec<String> = early.iter().chain(late.iter()).cloned().collect();
        let everything: BTreeSet<String> = stored.iter().cloned().collect();
        let early_set: BTreeSet<String> = early.iter().cloned().collect();
        let late_set: BTreeSet<String> = late.iter().cloned().collect();
        let nothing: BTreeSet<String> = BTreeSet::new();
        assert_eq!(
            instants.keys().cloned().collect::<BTreeSet<String>>(),
            everything,
            "case {label:?}: expected the listing to hold exactly {everything:?}, got {:?}",
            instants.keys().collect::<Vec<&String>>()
        );
        // The fixture's own precondition for the exactly-at-a-bound rows: creation order
        // is strictly increasing in the stored instant, so one record's instant names that
        // record alone.
        for pair in stored.windows(2) {
            let (older, newer) = (&pair[0], &pair[1]);
            assert!(
                instants[older] < instants[newer],
                "case {label:?}: expected {older} ({}) to be stored strictly before {newer} ({})",
                instants[older],
                instants[newer]
            );
        }

        // The last record of the earlier group and the first of the later one: a bound
        // taken from one of them sits exactly on a stored instant, which the strict rule
        // keeps out of both answers.
        let newest_early = early.last().expect("the earlier group holds a record").clone();
        let oldest_late = late.first().expect("the later group holds a record").clone();
        let early_without_its_newest: BTreeSet<String> = early_set
            .iter()
            .filter(|id| **id != newest_early)
            .cloned()
            .collect();
        let late_without_its_oldest: BTreeSet<String> = late_set
            .iter()
            .filter(|id| **id != oldest_late)
            .cloned()
            .collect();

        let rows: Vec<(String, String, &BTreeSet<String>)> = vec![
            ("no bound".to_string(), String::new(), &everything),
            (
                "createdAfter before every record".to_string(),
                format!("?createdAfter={}", query_instant(before_everything)),
                &everything,
            ),
            (
                "createdAfter after every record".to_string(),
                format!("?createdAfter={}", query_instant(after_everything)),
                &nothing,
            ),
            (
                "createdAfter at the midpoint".to_string(),
                format!("?createdAfter={}", query_instant(midpoint)),
                &late_set,
            ),
            (
                "createdBefore after every record".to_string(),
                format!("?createdBefore={}", query_instant(after_everything)),
                &everything,
            ),
            (
                "createdBefore before every record".to_string(),
                format!("?createdBefore={}", query_instant(before_everything)),
                &nothing,
            ),
            (
                "createdBefore at the midpoint".to_string(),
                format!("?createdBefore={}", query_instant(midpoint)),
                &early_set,
            ),
            (
                "a window around every record".to_string(),
                format!(
                    "?createdAfter={}&createdBefore={}",
                    query_instant(before_everything),
                    query_instant(after_everything)
                ),
                &everything,
            ),
            (
                "a window holding only the later group".to_string(),
                format!(
                    "?createdAfter={}&createdBefore={}",
                    query_instant(midpoint),
                    query_instant(after_everything)
                ),
                &late_set,
            ),
            (
                "a window holding only the earlier group".to_string(),
                format!(
                    "?createdAfter={}&createdBefore={}",
                    query_instant(before_everything),
                    query_instant(midpoint)
                ),
                &early_set,
            ),
            (
                "an empty window".to_string(),
                format!(
                    "?createdAfter={}&createdBefore={}",
                    query_instant(after_everything),
                    query_instant(before_everything)
                ),
                &nothing,
            ),
            // The strictness rows: a bound ON a stored instant never keeps the record
            // stored at it.
            (
                "createdAfter exactly at the newest record of the earlier group".to_string(),
                format!("?createdAfter={}", query_instant(instants[&newest_early])),
                &late_set,
            ),
            (
                "createdBefore exactly at the newest record of the earlier group".to_string(),
                format!("?createdBefore={}", query_instant(instants[&newest_early])),
                &early_without_its_newest,
            ),
            (
                "createdAfter exactly at the oldest record of the later group".to_string(),
                format!("?createdAfter={}", query_instant(instants[&oldest_late])),
                &late_without_its_oldest,
            ),
            (
                "createdBefore exactly at the oldest record of the later group".to_string(),
                format!("?createdBefore={}", query_instant(instants[&oldest_late])),
                &early_set,
            ),
            (
                "a window whose two bounds are one record's own instant".to_string(),
                format!(
                    "?createdAfter={}&createdBefore={}",
                    query_instant(instants[&oldest_late]),
                    query_instant(instants[&oldest_late])
                ),
                &nothing,
            ),
        ];

        for (row, query, expected) in rows {
            let case = format!("{label} / {row}");
            let body = list(&emr, &listing, &query).await.ok(&case).clone();
            let reported = listed_set(&body, &listing, &case);
            assert_eq!(
                &reported, expected,
                "case {case:?}: {query} expected {expected:?}, got {reported:?}"
            );
            // Whatever a bound kept really is inside it: the reported `createdAt` values
            // are the records' own instants, not the bounds the request carried.
            for record in listed(&body, &listing, &case) {
                let created_at = instant_millis(&record, "/createdAt", &case);
                let id = text(&record, "/id", &case);
                assert_eq!(
                    created_at, instants[&id],
                    "case {case:?}: expected {id} to report its stored instant {}, got {created_at}",
                    instants[&id]
                );
            }
        }

        // A filtered read is still a read: the unfiltered listing is what it was.
        let after = listed_set(list(&emr, &listing, "").await.ok(label), &listing, label);
        assert_eq!(
            after, everything,
            "case {label:?}: expected the bounded reads to leave {everything:?} listed, got {after:?}"
        );
    }
}

/// RULE 2: every listing the model gives a `states` member filters by one shared rule —
/// the records whose stored state is one of the states listed, with a list of no states
/// constraining nothing.
///
/// The fixture retires one record wherever the service's deletes let it, so a listing
/// holds records in more than one state and the rows below are built from the states the
/// listing itself reports.
#[tokio::test]
async fn every_listing_with_a_states_member_filters_by_one_shared_states_rule() {
    for which in LISTINGS_THAT_FILTER_BY_STATE {
        let (emr, listing) = mount(which).await;
        let label = listing.label;
        let catalog = listing
            .state_catalog
            .unwrap_or_else(|| panic!("case {label:?}: this listing filters by state"));

        let stored = store(&emr, &listing, "states", 3).await;
        if listing.retires_a_record {
            retire(&emr, &listing, &stored[0]).await;
        }
        let states = settled_states(&emr, &listing).await;
        let everything: BTreeSet<String> = states.keys().cloned().collect();

        // What the listing reports: the states present, and the records in each.
        let present: BTreeSet<String> = states.values().cloned().collect();
        let in_state = |wanted: &str| -> BTreeSet<String> {
            states
                .iter()
                .filter(|(_, state)| state.as_str() == wanted)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for state in &present {
            assert!(
                catalog.contains(&state.as_str()),
                "case {label:?}: reported state {state:?} is not one of {catalog:?}"
            );
        }
        // A state the catalog names and no stored record is in, so a filter naming it keeps
        // nothing. Tested on a catalog entry the fixture did not produce.
        let absent = catalog
            .iter()
            .find(|state| !present.contains(**state))
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected {catalog:?} to hold a state no record is in")
            });

        let mut rows: Vec<(String, String, BTreeSet<String>)> = vec![
            ("no states filter".to_string(), String::new(), everything.clone()),
            (
                format!("one state no record is in ({absent})"),
                format!("?states={absent}"),
                BTreeSet::new(),
            ),
            (
                "every state the catalog names".to_string(),
                catalog
                    .iter()
                    .map(|state| format!("states={state}"))
                    .collect::<Vec<String>>()
                    .join("&")
                    .replacen("states=", "?states=", 1),
                everything.clone(),
            ),
        ];
        // One row per state the listing actually reports, plus that state paired with one
        // nothing is in (a union keeps what either side keeps) and with itself (a repeat
        // keeps no more than once).
        for state in &present {
            rows.push((
                format!("the records in {state}"),
                format!("?states={state}"),
                in_state(state),
            ));
            rows.push((
                format!("{state} together with {absent}"),
                format!("?states={state}&states={absent}"),
                in_state(state),
            ));
            rows.push((
                format!("{state} listed twice"),
                format!("?states={state}&states={state}"),
                in_state(state),
            ));
        }
        // The states present partition the listing, so filtering by all of them at once
        // keeps exactly everything.
        rows.push((
            "every state the listing reports".to_string(),
            present
                .iter()
                .map(|state| format!("states={state}"))
                .collect::<Vec<String>>()
                .join("&")
                .replacen("states=", "?states=", 1),
            everything.clone(),
        ));

        for (row, query, expected) in rows {
            let case = format!("{label} / {row}");
            let body = list(&emr, &listing, &query).await.ok(&case).clone();
            let reported = listed_set(&body, &listing, &case);
            assert_eq!(
                reported, expected,
                "case {case:?}: {query} expected {expected:?}, got {reported:?}"
            );
            // Whatever the filter kept really is in a state it asked for: the reported
            // state is the record's own.
            for record in listed(&body, &listing, &case) {
                let id = text(&record, "/id", &case);
                let reported_state = text(&record, "/state", &case);
                assert_eq!(
                    reported_state, states[&id],
                    "case {case:?}: expected {id} to report its stored state {:?}, got {reported_state:?}",
                    states[&id]
                );
            }
        }
    }
}
