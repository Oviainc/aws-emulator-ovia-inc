// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the READINESS an `AttachPoint$arn` has to find behind it, driven
//! over the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: an attach point is a handle whose whole subject is the connectivity a
//! Connection is provisioned onto, so the handle names something only while the resource
//! behind it is in the state its own service documents as ready for use — for the ARN
//! member, the `ConnectionState` the model documents as "available: The connection has
//! been fully provisioned between AWS and the Partner". A connection that does not
//! provide that connectivity yet, and one that no longer provides it, are alike refused
//! with the operation's modelled 404, worded exactly as an ARN this scope never allocated
//! is, having provisioned and associated nothing — the same rule the union's gateway
//! member is held to, where only a gateway ready for use is a valid attach point.
//!
//! The rows differ along what the rule generalises over: the `ConnectionState` the
//! connection behind the ARN is in, the operation that carries the attach point
//! (`smithy_query(query="operation",operation="CreateConnection,AcceptConnectionProposal")`
//! — the two inputs that carry an `AttachPoint`), the Region the request addresses and
//! the environment and bandwidth it is placed on.
//!
//! The states come from the modelled enum
//! (`smithy_query(query="shape",shape="ConnectionState")` -> values: `available`,
//! `requested`, `pending`, `down`, `deleting`, `deleted`, `failed`, `updating`). Six of
//! them are reachable over the wire and each is a row below; `down` and `failed` are the
//! two no call of this service produces — they report a circuit fault this emulator
//! cannot have — so no row can drive a connection into them.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::attach_point_not_valid_message;

use crate::emulator::{
    available_bandwidths, code, message, placements_in, remote_account_for, text, Emulator,
};

/// The Regions the rows sign their requests for, so the rule is checked across the Region
/// the ARN carries as well as across the state of the connection it names.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The `ConnectionState` the model documents as "available: The connection has been fully
/// provisioned between AWS and the Partner" — the one state a connection IS the
/// connectivity an attach point names.
const READY_STATE: &str = "available";

/// How long a wait for a state the clock owes runs before the case fails.
const POLL_BUDGET: Duration = Duration::from_secs(5);

/// How often such a wait re-reads.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The state a row leaves the connection behind the ARN in before offering that ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behind {
    /// The state a create returns: "the state will remain here until the Connection is
    /// accepted on the Partner portal", so nothing is provisioned yet.
    Requested,
    /// "The connection has been accepted and is being provisioned between AWS and the
    /// Partner" — still being brought up.
    Pending,
    /// "The connection is being updated": a circuit being re-provisioned.
    Updating,
    /// "The connection is being deleted".
    Deleting,
    /// "The connection has been deleted".
    Deleted,
    /// The fully provisioned state — the positive control of each table.
    Available,
}

impl Behind {
    /// The `ConnectionState` value this row's connection reports.
    fn state(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Pending => "pending",
            Self::Updating => "updating",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
            Self::Available => READY_STATE,
        }
    }

    /// Whether a connection in this state provides the connectivity an attach point is a
    /// handle on, which is what decides the row's expected answer.
    fn provides_connectivity(self) -> bool {
        self.state() == READY_STATE
    }
}

/// Every state a row drives the connection behind the ARN into.
const BEHIND: [Behind; 6] = [
    Behind::Requested,
    Behind::Pending,
    Behind::Updating,
    Behind::Deleting,
    Behind::Deleted,
    Behind::Available,
];

/// `CreateConnection` on `placement` with `attach_point`, signed for `region`, as a
/// `(status, body)` pair so a refusal is readable. The `remoteAccount` is taken from the
/// Environment's own record, so a row about the attach point is not also a row about the
/// identifying information.
async fn create_in(
    emulator: &Emulator,
    region: &str,
    placement: &(String, String),
    attach_point: &Value,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(placement.0));
    request.insert("bandwidth".to_string(), json!(placement.1));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(remote_account) =
        remote_account_for(&emulator.environment_in(region, &placement.0).await)
    {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// A connection of `region`, created on one of the account's own Direct Connect gateways
/// — the one attach point that needs no connection behind it, so every row starts here.
async fn created_connection(
    emulator: &Emulator,
    region: &str,
    placement: &(String, String),
) -> Value {
    let attach_point = json!({ "directConnectGateway": emulator.attach_point().await });
    let (status, body) = create_in(emulator, region, placement, &attach_point).await;
    assert_eq!(
        status, 200,
        "setup {region:?}: expected the connection the ARN will name to be created, got {status} with {body:?}"
    );
    body["connection"].clone()
}

/// `AcceptConnectionProposal` with `attach_point`, as a `(status, body)` pair.
async fn accept_in(
    emulator: &Emulator,
    region: &str,
    activation_key: &str,
    attach_point: &Value,
) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            "AcceptConnectionProposal",
            json!({ "activationKey": activation_key, "attachPoint": attach_point }),
        )
        .await
}

/// The `ConnectionState` `region`'s endpoint reports for `identifier` NOW.
///
/// The read also pins the other half of the rule: the record stays readable by its
/// identifier however the handle on it answers.
async fn state_now(emulator: &Emulator, region: &str, identifier: &str) -> String {
    let (status, body) = emulator
        .ic_in(region, "GetConnection", json!({ "identifier": identifier }))
        .await;
    assert_eq!(
        status, 200,
        "case {identifier:?}: expected the connection to stay readable by its identifier, got {status} with {body:?}"
    );
    text(&body["connection"], "state")
}

/// Poll `identifier` until `region`'s endpoint reports it in `wanted`.
async fn wait_for_state(emulator: &Emulator, region: &str, identifier: &str, wanted: &str) {
    let started = Instant::now();
    loop {
        let state = state_now(emulator, region, identifier).await;
        if state == wanted {
            return;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "setup {identifier:?}: expected the connection to reach {wanted:?} within {POLL_BUDGET:?}, it is still {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `(environmentId, bandwidth)` pair of `region` whose Environment offers a SECOND
/// bandwidth, together with that second value.
///
/// What driving a connection into `updating` takes: only a bandwidth CHANGE re-provisions
/// the circuit, so the row needs two values the same Environment offers.
async fn changeable_placement(emulator: &Emulator, region: &str) -> ((String, String), String) {
    for placement in placements_in(emulator, region).await {
        let environment = emulator.environment_in(region, &placement.0).await;
        if let Some(other) = available_bandwidths(&environment)
            .into_iter()
            .find(|bandwidth| *bandwidth != placement.1)
        {
            return (placement, other);
        }
    }
    panic!(
        "setup {region:?}: expected the catalog to offer an environment with two available \
         bandwidths to change between"
    );
}

/// Leave a connection of `region` in the state `behind` names, and answer the record as
/// the call that left it there reported it.
///
/// Every state is reached by the operation that AWS documents as producing it, so a row's
/// subject is a connection the service itself put in that state: the create for the
/// initial one, an acceptance for the accepted one, a bandwidth change for the
/// re-provisioning one, a delete for the two teardown ones, and the clock for the fully
/// provisioned one.
async fn connection_behind(
    emulator: &Emulator,
    region: &str,
    placement: &(String, String),
    change_to: Option<&str>,
    behind: Behind,
) -> Value {
    let created = created_connection(emulator, region, placement).await;
    let identifier = text(&created, "id");
    match behind {
        Behind::Requested => created,
        Behind::Pending => {
            let (status, body) = accept_in(
                emulator,
                region,
                &text(&created, "activationKey"),
                &json!({ "directConnectGateway": emulator.attach_point().await }),
            )
            .await;
            assert_eq!(
                status, 200,
                "setup {identifier:?}: expected the proposal to be accepted, got {status} with {body:?}"
            );
            body["connection"].clone()
        }
        Behind::Updating => {
            let bandwidth = change_to.expect("an updating row carries a second bandwidth");
            emulator.provisioned_connection(region, &identifier).await;
            let (status, body) = emulator
                .ic_in(
                    region,
                    "UpdateConnection",
                    json!({ "identifier": identifier, "bandwidth": bandwidth }),
                )
                .await;
            assert_eq!(
                status, 200,
                "setup {identifier:?}: expected the bandwidth change to be accepted, got {status} with {body:?}"
            );
            body["connection"].clone()
        }
        Behind::Deleting | Behind::Deleted => {
            let (status, body) = emulator
                .ic_in(
                    region,
                    "DeleteConnection",
                    json!({ "identifier": identifier }),
                )
                .await;
            assert_eq!(
                status, 200,
                "setup {identifier:?}: expected the delete to be accepted, got {status} with {body:?}"
            );
            if behind == Behind::Deleted {
                wait_for_state(emulator, region, &identifier, behind.state()).await;
                let read = emulator
                    .ic_ok_in(region, "GetConnection", json!({ "identifier": identifier }))
                    .await;
                read["connection"].clone()
            } else {
                body["connection"].clone()
            }
        }
        Behind::Available => emulator.provisioned_connection(region, &identifier).await,
    }
}

/// The `ConnectionId`s `region`'s endpoint holds, walked page by page so the answer is
/// the whole store however it is windowed. Ids rather than whole records, because the
/// clock moves a record's `state` on its own between two listings.
async fn stored_ids(emulator: &Emulator, region: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok_in(region, "ListConnections", Value::Object(request))
            .await;
        all.extend(
            listed["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|connection| text(connection, "id")),
        );
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// Assert `(status, body)` is the modelled 404 this service answers an attach point that
/// resolves to nothing with, worded for `arn` on `environment_id`.
fn assert_not_an_attach_point(
    case: &str,
    arn: &str,
    environment_id: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404 for an attach point whose connection provides no connectivity, got {status} with {body:?}"
    );
    assert!(
        code(body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        code(body)
    );
    assert_eq!(
        message(body),
        attach_point_not_valid_message(arn, environment_id),
        "case {case:?}: expected this service's own wording for an attach point that resolves to nothing, got {:?}",
        message(body)
    );
}

/// The placement a row sits on, and the second bandwidth an `updating` row changes to.
async fn row_placement(
    emulator: &Emulator,
    region: &str,
    seed: usize,
    behind: Behind,
) -> ((String, String), Option<String>) {
    if behind == Behind::Updating {
        let (placement, change_to) = changeable_placement(emulator, region).await;
        return (placement, Some(change_to));
    }
    let places = placements_in(emulator, region).await;
    (places[seed % places.len()].clone(), None)
}

/// Every state the table judged while the connection was still in it, so a row the clock
/// moved across its own call is not mistaken for coverage.
fn assert_every_state_was_judged(proved: &BTreeSet<&'static str>) {
    let wanted: BTreeSet<&'static str> = BEHIND.iter().map(|behind| behind.state()).collect();
    assert_eq!(
        *proved, wanted,
        "case \"coverage\": expected every state of the table to be judged while the connection was still in it, got {proved:?}"
    );
}

// ---------------------------------------------------------------------------
// CreateConnection
// ---------------------------------------------------------------------------

/// RULE: a `CreateConnection` whose `attachPoint.arn` names a connection that is not
/// fully provisioned — still awaiting the partner portal, still being provisioned, being
/// re-provisioned, being torn down or torn down — is refused with the modelled 404 and
/// provisions nothing, while the ARN of a fully provisioned connection is accepted and
/// read back.
#[tokio::test]
async fn a_create_refuses_an_attach_point_arn_whose_connection_provides_no_connectivity() {
    let emulator = Emulator::new();
    let mut proved: BTreeSet<&'static str> = BTreeSet::new();

    for (seed, behind) in BEHIND.into_iter().enumerate() {
        for (offset, region) in REGIONS.iter().enumerate() {
            // The row's placement comes from the catalog of its OWN Region: an
            // Environment belongs to the Region whose endpoint offers it.
            let (placement, change_to) =
                row_placement(&emulator, region, seed * REGIONS.len() + offset, behind).await;
            let case = format!(
                "{behind:?} / {region} / {} / {}",
                placement.0, placement.1
            );

            let subject =
                connection_behind(&emulator, region, &placement, change_to.as_deref(), behind)
                    .await;
            let arn = text(&subject, "arn");
            let subject_id = text(&subject, "id");
            assert_eq!(
                text(&subject, "state"),
                behind.state(),
                "setup {case:?}: expected the connection behind the ARN to report {:?}, got {:?}",
                behind.state(),
                subject["state"]
            );

            // Taken before the offer, so this is the id set a refused create must not
            // change.
            let before_ids = stored_ids(&emulator, region).await;
            let before = state_now(&emulator, region, &subject_id).await;
            let (status, body) =
                create_in(&emulator, region, &placement, &json!({ "arn": arn })).await;
            let after = state_now(&emulator, region, &subject_id).await;

            // The clock carries a connection on by itself, so a row it moved ACROSS the
            // call above judged a state of its own and is not counted; the reads bracket
            // the call, and the clock only ever moves a state forward.
            if before != after {
                continue;
            }
            if behind.provides_connectivity() {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the ARN of a fully provisioned connection to be accepted, got {status} with {body:?}"
                );
                let fetched = emulator
                    .ic_ok_in(
                        region,
                        "GetConnection",
                        json!({ "identifier": text(&body["connection"], "id") }),
                    )
                    .await;
                assert_eq!(
                    fetched["connection"]["attachPoint"],
                    json!({ "arn": arn }),
                    "case {case:?}: expected GetConnection to report the ARN attach point, got {:?}",
                    fetched["connection"]["attachPoint"]
                );
            } else {
                assert_not_an_attach_point(&case, &arn, &placement.0, status, &body);
                let after_ids = stored_ids(&emulator, region).await;
                assert_eq!(
                    after_ids, before_ids,
                    "case {case:?}: expected the refused create to have provisioned nothing, got {after_ids:?}"
                );
            }
            proved.insert(behind.state());
        }
    }

    assert_every_state_was_judged(&proved);
}

// ---------------------------------------------------------------------------
// AcceptConnectionProposal
// ---------------------------------------------------------------------------

/// RULE: the same readiness decides the attach point an ACCEPTANCE selects — an ARN
/// naming a connection that provides no connectivity resolves to nothing, leaving the
/// proposal exactly as it was, while the ARN of a fully provisioned connection is
/// accepted and read back.
#[tokio::test]
async fn an_acceptance_refuses_an_attach_point_arn_whose_connection_provides_no_connectivity() {
    let emulator = Emulator::new();
    let mut proved: BTreeSet<&'static str> = BTreeSet::new();

    for (seed, behind) in BEHIND.into_iter().enumerate() {
        for (offset, region) in REGIONS.iter().enumerate() {
            let (placement, change_to) =
                row_placement(&emulator, region, seed * REGIONS.len() + offset + 1, behind).await;
            let case = format!(
                "{behind:?} / {region} / {} / {}",
                placement.0, placement.1
            );

            let subject =
                connection_behind(&emulator, region, &placement, change_to.as_deref(), behind)
                    .await;
            let arn = text(&subject, "arn");
            let subject_id = text(&subject, "id");

            // One proposal per acceptance, created right before it: a proposal is only
            // awaiting acceptance until somebody accepts it.
            let proposal = created_connection(&emulator, region, &placement).await;
            let before = state_now(&emulator, region, &subject_id).await;
            let (status, body) = accept_in(
                &emulator,
                region,
                &text(&proposal, "activationKey"),
                &json!({ "arn": arn }),
            )
            .await;
            let after = state_now(&emulator, region, &subject_id).await;

            if before != after {
                continue;
            }
            if behind.provides_connectivity() {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the ARN of a fully provisioned connection to be accepted, got {status} with {body:?}"
                );
                assert_eq!(
                    body["connection"]["attachPoint"],
                    json!({ "arn": arn }),
                    "case {case:?}: expected the accepted connection to carry the ARN attach point, got {:?}",
                    body["connection"]["attachPoint"]
                );
            } else {
                assert_not_an_attach_point(&case, &arn, &placement.0, status, &body);
            }

            // What the proposal carries now: the ARN an accepted one selected, and the
            // attach point it was created on wherever the acceptance was refused.
            let read_back = emulator
                .ic_ok_in(
                    region,
                    "GetConnection",
                    json!({ "identifier": text(&proposal, "id") }),
                )
                .await;
            let expected = if behind.provides_connectivity() {
                json!({ "arn": arn })
            } else {
                proposal["attachPoint"].clone()
            };
            assert_eq!(
                read_back["connection"]["attachPoint"], expected,
                "case {case:?}: expected the proposal to carry {expected:?}, got {:?}",
                read_back["connection"]["attachPoint"]
            );
            proved.insert(behind.state());
        }
    }

    assert_every_state_was_judged(&proved);
}
