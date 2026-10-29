// SPDX-License-Identifier: CC0-1.0

//! What an `AcceptConnectionProposal` answers does NOT depend on how long the caller
//! took to make it, driven over the wire through the shared `crate::emulator` harness.
//!
//! The RULE under test: the clock carries a new connection along the documented
//! provisioning progression on its own, but it stands in for the TIME each leg takes
//! and accepts nothing — so the acceptance, which is what selects the Attach Point "to
//! which the connection should be associated", stays the caller's to make however far
//! that progression has run, applies the attach point, description and tags it carries,
//! and hands the connection on along the documented acceptance progression from the
//! instant it is made. A proposal is accepted ONCE, by that call: only after it has
//! been made is a differing acceptance refused, and a repeat of the same one reports
//! the same connection and changes nothing.
//!
//! Every assertion is checked over a TABLE whose rows differ in how long the caller
//! waited between the proposal and its acceptance — nothing, past one leg of the
//! progression, and past the whole of it — as well as in the environment, the
//! bandwidth, the Region, the gateway the acceptance selects, the presence of the
//! optional description and the tags each call carries.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, tags_of, text,
    Emulator,
};

/// The two Regions the rows sign their requests for, so the rule holds across the
/// Region a proposal belongs to as well as across the wait.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The gateways the rows attach to: an attach point IS a Direct Connect gateway of the
/// account, so the suite creates its own and the rows differ in which one the proposal
/// was created on, which one its acceptance selects, in each gateway's name and in its
/// `amazonSideAsn`.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] =
    [("ic-unraced-a", None), ("ic-unraced-b", Some(64_512))];

/// The `ConnectionState` the model documents for an accepted proposal's connection:
/// "pending: The connection has been accepted and is being provisioned between AWS and
/// the Partner" (`smithy_query(query="shape",shape="Connection")`, member `state`).
const ACCEPTED_STATE: &str = "pending";

/// The `ConnectionState` the model documents as the end of that progression:
/// "available: The connection has been fully provisioned between AWS and the Partner".
const PROVISIONED_STATE: &str = "available";

/// The `ConnectionState` a new connection is returned in: "requested: The initial state
/// of a connection."
const INITIAL_STATE: &str = "requested";

/// How long a poll waits for the connection to be fully provisioned before the case
/// fails — well past the spans the service owes either leg.
const POLL_BUDGET: Duration = Duration::from_secs(5);

/// How often such a poll re-reads.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long a row waits between making the proposal and accepting it.
///
/// `Nothing` is the acceptance issued as the very next call. `PastOneLeg` is long
/// enough that the clock has carried the connection out of the state it was created in,
/// and `PastTheProgression` long enough that it has carried it all the way to the fully
/// provisioned one — which each row CHECKS by reading the connection back before it
/// accepts, so the row is known to exercise the stage it means to rather than trusting
/// a sleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Wait {
    Nothing,
    PastOneLeg,
    PastTheProgression,
}

impl Wait {
    fn duration(self) -> Duration {
        match self {
            Wait::Nothing => Duration::ZERO,
            Wait::PastOneLeg => Duration::from_millis(400),
            Wait::PastTheProgression => Duration::from_millis(900),
        }
    }
}

/// Every wait a row can be made after, so the rule is checked on both sides of the
/// clock's progression.
const WAITS: [Wait; 3] = [Wait::Nothing, Wait::PastOneLeg, Wait::PastTheProgression];

/// How many of the catalog's `(environment, bandwidth)` placements the table spans.
/// Each one is crossed with every wait, and a wait sleeps, so the table is bounded by
/// count rather than by the catalog's size.
const PLACEMENTS_PER_SUITE: usize = 3;

/// One row: the proposal to create, how long to wait, and the acceptance to make.
#[derive(Debug, Clone)]
struct Row {
    environment: Value,
    bandwidth: String,
    region: &'static str,
    /// The gateway the proposal's connection was created on.
    created_on: String,
    /// The gateway the acceptance selects — the other one on most rows, so the
    /// acceptance really has an attach point to apply, and the same one on the rest.
    accept_on: String,
    /// How long the caller waited before accepting.
    wait: Wait,
    /// The tags the proposal itself was created with.
    proposal_tags: BTreeMap<String, String>,
    /// The optional description the acceptance supplies.
    description: Option<String>,
    /// The tags the acceptance asks to associate with the resulting Connection.
    tags: BTreeMap<String, String>,
}

impl Row {
    fn environment_id(&self) -> String {
        text(&self.environment, "environmentId")
    }

    fn label(&self) -> String {
        format!(
            "{} / {} / {} / accepted on {} after {:?}",
            self.environment_id(),
            self.bandwidth,
            self.region,
            self.accept_on,
            self.wait
        )
    }

    fn accept_attach_point(&self) -> Value {
        json!({ "directConnectGateway": self.accept_on })
    }

    /// The tag set the accepted Connection must hold: the proposal's own with the
    /// acceptance's merged over them, because a tag write merges rather than replaces.
    fn merged_tags(&self) -> BTreeMap<String, String> {
        let mut merged = self.proposal_tags.clone();
        merged.extend(self.tags.clone());
        merged
    }
}

/// The table: every wait crossed with the placements the live catalog offers, so each
/// stage of the clock's progression is exercised on a different environment, bandwidth,
/// Region, gateway, description and tag set.
fn table(catalog: &[Value], gateways: &[String]) -> Vec<Row> {
    // Every placement the live catalog offers, bounded so the suite's waits stay a few
    // seconds in total: the rows that run still cover every wait, each on a different
    // environment, bandwidth, Region, gateway, description and tag set.
    let placements: Vec<(Value, String)> = creatable_environments(catalog)
        .into_iter()
        .flat_map(|environment| {
            available_bandwidths(&environment)
                .into_iter()
                .map(move |bandwidth| (environment.clone(), bandwidth))
        })
        .take(PLACEMENTS_PER_SUITE)
        .collect();

    let mut rows = Vec::new();
    for (seed, (environment, bandwidth)) in placements.into_iter().enumerate() {
        let created_on = gateways[seed % gateways.len()].clone();
        for (offset, wait) in WAITS.iter().enumerate() {
            let turn = seed + offset;
            let accept_on = if turn.is_multiple_of(4) {
                created_on.clone()
            } else {
                gateways[(seed + 1) % gateways.len()].clone()
            };
            rows.push(Row {
                environment: environment.clone(),
                bandwidth: bandwidth.clone(),
                region: REGIONS[turn % REGIONS.len()],
                created_on: created_on.clone(),
                accept_on,
                wait: *wait,
                proposal_tags: if turn.is_multiple_of(2) {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([("stage".to_string(), "proposal".to_string())])
                },
                description: if turn.is_multiple_of(3) {
                    None
                } else {
                    Some(format!("accepted after waiting {turn}"))
                },
                tags: match turn % 3 {
                    0 => BTreeMap::new(),
                    1 => BTreeMap::from([("owner".to_string(), format!("team-{turn}"))]),
                    _ => BTreeMap::from([
                        ("owner".to_string(), format!("team-{turn}")),
                        ("stage".to_string(), "accepted".to_string()),
                    ]),
                },
            });
        }
    }
    rows
}

/// `CreateConnection` for one row, answering the stored `Connection` — the proposal
/// whose activation key the acceptance is made with.
async fn create_proposal(emulator: &Emulator, row: &Row) -> Value {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(row.environment_id()));
    request.insert("bandwidth".to_string(), json!(row.bandwidth));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": row.created_on }),
    );
    request.insert(
        "description".to_string(),
        json!(format!("proposal on {}", row.created_on)),
    );
    if let Some(remote_account) = remote_account_for(&row.environment) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    if !row.proposal_tags.is_empty() {
        request.insert("tags".to_string(), json!(row.proposal_tags));
    }
    let created = emulator
        .ic_ok_in(row.region, "CreateConnection", Value::Object(request))
        .await;
    created["connection"].clone()
}

/// An `AcceptConnectionProposal` request body.
fn accept_request(
    activation_key: &str,
    attach_point: &Value,
    description: Option<&String>,
    tags: &BTreeMap<String, String>,
) -> Value {
    let mut request = Map::new();
    request.insert("activationKey".to_string(), json!(activation_key));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(description) = description {
        request.insert("description".to_string(), json!(description));
    }
    if !tags.is_empty() {
        request.insert("tags".to_string(), json!(tags));
    }
    Value::Object(request)
}

/// `GetConnection` in `region`, answering the stored `Connection`.
async fn read_back(emulator: &Emulator, region: &str, identifier: &str) -> Value {
    let fetched = emulator
        .ic_ok_in(region, "GetConnection", json!({ "identifier": identifier }))
        .await;
    fetched["connection"].clone()
}

/// Poll `identifier` until it reports the fully provisioned state, answering the record
/// it settled as.
async fn wait_until_provisioned(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    case: &str,
) -> Value {
    let started = Instant::now();
    loop {
        let connection = read_back(emulator, region, identifier).await;
        let state = text(&connection, "state");
        if state == PROVISIONED_STATE {
            return connection;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: expected the connection to reach {PROVISIONED_STATE:?} within {POLL_BUDGET:?}, it is still {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `Connection` with its `state` projected out, so two readings of one record can be
/// compared on everything the clock does not move on its own.
fn without_state(connection: &Value) -> Value {
    let mut copy = connection.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("state");
    }
    copy
}

// ---------------------------------------------------------------------------
// The acceptance is the caller's to make, whatever the clock did in between
// ---------------------------------------------------------------------------

/// RULE: accepting a proposal applies the Attach Point the request selects — and the
/// description and tags it carries — and reports the connection in the documented
/// accepted state, whatever stage of the provisioning progression the clock had already
/// carried that connection to; the same well-formed request is therefore answered
/// identically however long the caller waited.
#[tokio::test]
async fn an_acceptance_applies_its_attach_point_however_long_the_caller_waited() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let rows = table(&catalog, &gateways);
    assert!(
        rows.len() >= WAITS.len() * 2,
        "the table must cover every wait on several placements, got {} rows",
        rows.len()
    );

    // What each row's acceptance answered, so the rule's claim — the answer does not
    // depend on the wait — is checked across the whole table and not only row by row.
    let mut answered_states: BTreeMap<Wait, Vec<String>> = BTreeMap::new();

    for row in &rows {
        let label = row.label();
        let proposal = create_proposal(&emulator, row).await;
        let activation_key = text(&proposal, "activationKey");
        let identifier = text(&proposal, "id");

        // The wait the row is about, and then the check that it really put the
        // connection where the row means to accept it from.
        if row.wait != Wait::Nothing {
            tokio::time::sleep(row.wait.duration()).await;
            let before = read_back(&emulator, row.region, &identifier).await;
            let state_before = text(&before, "state");
            assert_ne!(
                state_before, INITIAL_STATE,
                "case {label:?}: expected the clock to have carried the connection out of {INITIAL_STATE:?} before the acceptance, got {state_before:?}"
            );
            if row.wait == Wait::PastTheProgression {
                assert_eq!(
                    state_before, PROVISIONED_STATE,
                    "case {label:?}: expected the whole progression to have run before the acceptance, got {state_before:?}"
                );
            }
            // And nobody's acceptance was recorded by that progression: the attach
            // point is still the one the proposal was made with.
            assert_eq!(
                before["attachPoint"],
                json!({ "directConnectGateway": row.created_on }),
                "case {label:?}: expected the clock to leave the proposal's own attach point alone, got {:?}",
                before["attachPoint"]
            );
        }

        let (status, body) = emulator
            .ic_in(
                row.region,
                "AcceptConnectionProposal",
                accept_request(
                    &activation_key,
                    &row.accept_attach_point(),
                    row.description.as_ref(),
                    &row.tags,
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from AcceptConnectionProposal, got {status} with {body:?}"
        );
        let accepted = body["connection"].clone();

        // What the acceptance DOES set.
        assert_eq!(
            accepted["attachPoint"],
            row.accept_attach_point(),
            "case {label:?}: expected the connection to carry the attach point the acceptance selected, got {:?}",
            accepted["attachPoint"]
        );
        let expected_description = row
            .description
            .clone()
            .unwrap_or_else(|| text(&proposal, "description"));
        assert_eq!(
            text(&accepted, "description"),
            expected_description,
            "case {label:?}: expected the description {expected_description:?}, got {:?}",
            accepted["description"]
        );
        assert_eq!(
            tags_of(&accepted),
            row.merged_tags(),
            "case {label:?}: expected the acceptance's tags merged onto the proposal's ({:?}), got {:?}",
            row.merged_tags(),
            tags_of(&accepted)
        );

        // And what it leaves alone: the parameters chosen on the partner's portal, and
        // the connection's own identity.
        for member in [
            "id",
            "arn",
            "sharedId",
            "activationKey",
            "ownerAccount",
            "environmentId",
            "bandwidth",
            "provider",
            "location",
            "type",
        ] {
            assert_eq!(
                accepted[member], proposal[member],
                "case {label:?}: expected the acceptance to leave {member} as the proposal holds it, got {:?} vs {:?}",
                accepted[member], proposal[member]
            );
        }

        // The documented acceptance progression starts HERE, so the state the
        // acceptance reports is the same whatever the clock had done beforehand.
        let state = text(&accepted, "state");
        assert_eq!(
            state, ACCEPTED_STATE,
            "case {label:?}: expected an accepted proposal's connection to report {ACCEPTED_STATE:?}, got {state:?}"
        );
        answered_states.entry(row.wait).or_default().push(state);

        // Round trip: the acceptance was PERSISTED, and the connection is handed on to
        // the fully provisioned state from here.
        let settled = wait_until_provisioned(&emulator, row.region, &identifier, &label).await;
        assert_eq!(
            without_state(&settled),
            without_state(&accepted),
            "case {label:?}: expected the provisioned connection to be the accepted record, got {settled:?}"
        );
    }

    // The whole table agrees: every wait answered with the accepted state, so the
    // operation's outcome is not a function of the wall clock.
    for wait in WAITS {
        let states = answered_states.get(&wait).cloned().unwrap_or_default();
        assert!(
            !states.is_empty() && states.iter().all(|state| state == ACCEPTED_STATE),
            "case {wait:?}: expected every row of this wait to answer {ACCEPTED_STATE:?}, got {states:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The acceptance is what makes a proposal accepted — not the clock
// ---------------------------------------------------------------------------

/// RULE: a proposal is accepted once, by the acceptance itself. Once that call has been
/// made, a repeat asking for exactly what the connection holds reports the same
/// connection and changes nothing, while one asking for anything else is refused as
/// this service's own client fault, naming the connection the acceptance made and never
/// the `@sensitive` key — and both answers hold after the clock has carried the accepted
/// connection to the end of the progression.
#[tokio::test]
async fn only_an_acceptance_makes_a_proposal_accepted() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let rows = table(&catalog, &gateways);

    // Rows differ in the wait before the first acceptance, so the refusal is pinned
    // both for a proposal accepted at once and for one accepted long afterwards.
    for row in rows.iter().take(WAITS.len()) {
        let label = row.label();
        let proposal = create_proposal(&emulator, row).await;
        let activation_key = text(&proposal, "activationKey");
        let identifier = text(&proposal, "id");
        if row.wait != Wait::Nothing {
            tokio::time::sleep(row.wait.duration()).await;
        }

        let request = accept_request(
            &activation_key,
            &row.accept_attach_point(),
            row.description.as_ref(),
            &row.tags,
        );
        let accepted = emulator
            .ic_ok_in(row.region, "AcceptConnectionProposal", request.clone())
            .await["connection"]
            .clone();

        // The accepted connection is carried to the end of the progression, so the two
        // answers below are the ones a caller gets long after the acceptance.
        let settled = wait_until_provisioned(&emulator, row.region, &identifier, &label).await;

        // The SAME acceptance repeated: the same connection, unchanged — including its
        // state, which a replay does not hand back to the clock.
        let again = emulator
            .ic_ok_in(row.region, "AcceptConnectionProposal", request)
            .await["connection"]
            .clone();
        assert_eq!(
            again, settled,
            "case {label:?}: expected a repeated acceptance to report the settled connection unchanged, got {again:?}"
        );

        // A DIFFERING acceptance of the same proposal is refused, and changes nothing.
        let other_gateway = gateways
            .iter()
            .find(|gateway| **gateway != row.accept_on)
            .expect("the account holds more than one gateway")
            .clone();
        let (status, answered) = emulator
            .ic_in(
                row.region,
                "AcceptConnectionProposal",
                accept_request(
                    &activation_key,
                    &json!({ "directConnectGateway": other_gateway }),
                    row.description.as_ref(),
                    &row.tags,
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected an accepted proposal to refuse a different acceptance, got {status} with {answered:?}"
        );
        assert!(
            code(&answered).contains("InterconnectClientException"),
            "case {label:?}: expected InterconnectClientException, got {:?}",
            code(&answered)
        );
        assert!(
            message(&answered).contains(&identifier)
                && !message(&answered).contains(&activation_key),
            "case {label:?}: expected the refusal to name the connection and not the sensitive key, got {:?}",
            message(&answered)
        );
        let after = read_back(&emulator, row.region, &identifier).await;
        assert_eq!(
            after, settled,
            "case {label:?}: expected the refused acceptance to change nothing, got {after:?}"
        );
        // And the acceptance that WAS made is the one the record holds.
        assert_eq!(
            after["attachPoint"],
            row.accept_attach_point(),
            "case {label:?}: expected the record to keep the accepted attach point, got {:?}",
            after["attachPoint"]
        );
        assert_eq!(
            text(&accepted, "id"),
            identifier,
            "case {label:?}: expected the acceptance to report the proposal's own connection, got {:?}",
            accepted["id"]
        );
    }
}
