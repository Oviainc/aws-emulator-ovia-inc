// SPDX-License-Identifier: CC0-1.0

//! What an `activationKey` CAPTURED stays captured — driven over the wire through the
//! shared `crate::emulator` harness (`POST /` with
//! `X-Amz-Target: Interconnect.<Operation>` on `awsJson1_0`).
//!
//! The RULE under test: the proposal an activation key names reports the parameters of
//! the creation request that minted the key ("this key captures the desired parameters
//! from the initial creation request", and the proposal's Environment and bandwidth
//! "cannot be modified"), so a later write to the connection record — whose own
//! `bandwidth` is "the specific selected bandwidth of this connection", the member
//! `UpdateConnection` reassigns — changes what `GetConnection` reports for that
//! connection and never what `DescribeConnectionProposal` reports for its key.
//!
//! Asserted over a TABLE built from the live seeded catalog, whose rows differ in the
//! environment and so in the provider kind, in the bandwidth the connection was created
//! with and the chain of bandwidths the updates ask for (upwards, downwards, across the
//! `Mbps`/`Gbps` unit, and one chain that ends by putting the captured value back), in
//! how many updates are made, in whether the update is made while the proposal is still
//! awaiting acceptance or once the clock has provisioned the connection, in the member
//! the update carries, and in the Region every request of the row is signed for.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::UPDATABLE_CONNECTION_STATES;

use crate::emulator::{
    available_bandwidths, creatable_environments, remote_account_for, text, Emulator,
};

/// The gateways the rows attach their connections to: an attach point IS a Direct
/// Connect gateway of the account, so the suite creates its own and the rows differ in
/// which one they use, in its name and in its `amazonSideAsn`.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] = [("ic-capture-a", None), ("ic-capture-b", Some(64_513))];

/// How long a wait for a state the clock owes a connection runs before the case fails.
const CLOCK_BUDGET: Duration = Duration::from_secs(5);

/// How often that wait re-reads.
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One row: a connection to create, and therefore a proposal whose captured parameters
/// the updates below must not reach.
#[derive(Debug, Clone)]
struct CaptureCase {
    environment: Value,
    /// The bandwidth the creation request asks for, which is the one its activation key
    /// captures.
    created_with: String,
    /// The bandwidths successive `UpdateConnection` calls ask for, in order.
    updates: Vec<String>,
    region: &'static str,
    attach_point: String,
    /// Whether the first update waits for the clock to provision the connection, or is
    /// made while the proposal is still awaiting acceptance — the window in which the
    /// caller still holds an unredeemed activation key.
    update_once_provisioned: bool,
}

impl CaptureCase {
    fn environment_id(&self) -> String {
        text(&self.environment, "environmentId")
    }

    /// The bandwidth the connection RECORD holds once every update of the row has been
    /// applied, which is the last one asked for.
    fn final_bandwidth(&self) -> String {
        self.updates
            .last()
            .cloned()
            .unwrap_or_else(|| self.created_with.clone())
    }

    fn label(&self) -> String {
        format!(
            "{} / created {} / updated to {:?} in {} / {}",
            self.environment_id(),
            self.created_with,
            self.updates,
            self.region,
            if self.update_once_provisioned {
                "updated once provisioned"
            } else {
                "updated while awaiting acceptance"
            }
        )
    }

    /// The `CreateConnection` body for `bandwidth` on this row's environment, carrying
    /// the identifying information that Environment asks every create on it for.
    fn create_request(&self, bandwidth: &str, description: &str) -> Value {
        let mut request = Map::new();
        request.insert("environmentId".to_string(), json!(self.environment_id()));
        request.insert("bandwidth".to_string(), json!(bandwidth));
        request.insert(
            "attachPoint".to_string(),
            json!({ "directConnectGateway": self.attach_point }),
        );
        request.insert("description".to_string(), json!(description));
        if let Some(remote_account) = remote_account_for(&self.environment) {
            request.insert("remoteAccount".to_string(), remote_account);
        }
        Value::Object(request)
    }
}

/// The table: every environment the catalog offers more than one bandwidth on, once per
/// bandwidth it offers, each row changing to the NEXT one so the rows differ in the
/// direction of the change as well as in its size and unit.
fn capture_table(catalog: &[Value], gateways: &[String]) -> Vec<CaptureCase> {
    let mut rows = Vec::new();
    let mut seed = 0usize;
    for environment in creatable_environments(catalog) {
        let offered = available_bandwidths(&environment);
        // A row needs a second bandwidth to move to; an Environment offering one takes
        // no bandwidth change at all (`bandwidth_availability_violation`).
        if offered.len() < 2 {
            continue;
        }
        for (index, created_with) in offered.iter().enumerate() {
            seed += 1;
            let other = offered[(index + 1) % offered.len()].clone();
            rows.push(CaptureCase {
                environment: environment.clone(),
                created_with: created_with.clone(),
                // Every other row changes the bandwidth TWICE, the second change
                // putting back the very value the key captured: what the proposal
                // reports is the captured parameter itself, not merely whatever the
                // record last differed from.
                updates: if seed.is_multiple_of(2) {
                    vec![other, created_with.clone()]
                } else {
                    vec![other]
                },
                region: if seed.is_multiple_of(3) {
                    "eu-west-1"
                } else {
                    "us-east-1"
                },
                attach_point: gateways[seed % gateways.len()].clone(),
                update_once_provisioned: !seed.is_multiple_of(3),
            });
        }
    }
    rows
}

/// `DescribeConnectionProposal` for one key, required to succeed.
async fn describe(emulator: &Emulator, region: &str, activation_key: &str) -> Value {
    emulator
        .ic_ok_in(
            region,
            "DescribeConnectionProposal",
            json!({ "activationKey": activation_key }),
        )
        .await
}

/// The stored `Connection` as `GetConnection` reports it.
async fn read_back(emulator: &Emulator, region: &str, identifier: &str) -> Value {
    emulator
        .ic_ok_in(region, "GetConnection", json!({ "identifier": identifier }))
        .await["connection"]
        .clone()
}

/// Wait until `GetConnection` reports `identifier` in a state an update is permitted
/// from, so the next update of a chain is judged on the record and not refused for a
/// transition still in flight. The states are the service's own
/// (`UPDATABLE_CONNECTION_STATES`), never ones this suite sets.
async fn wait_until_updatable(emulator: &Emulator, region: &str, identifier: &str, case: &str) {
    let started = Instant::now();
    loop {
        let state = text(&read_back(emulator, region, identifier).await, "state");
        if UPDATABLE_CONNECTION_STATES.contains(&state.as_str()) {
            return;
        }
        assert!(
            started.elapsed() < CLOCK_BUDGET,
            "case {case:?}: expected {identifier} to rest in one of {UPDATABLE_CONNECTION_STATES:?} \
             within {CLOCK_BUDGET:?}, it is still {state:?}"
        );
        tokio::time::sleep(STATE_POLL_INTERVAL).await;
    }
}

/// Wait until `GetConnection` reports `identifier` fully provisioned — the terminal
/// state the clock owes a created connection.
async fn wait_until_provisioned(emulator: &Emulator, region: &str, identifier: &str, case: &str) {
    let started = Instant::now();
    loop {
        let state = text(&read_back(emulator, region, identifier).await, "state");
        if state == "available" {
            return;
        }
        assert!(
            started.elapsed() < CLOCK_BUDGET,
            "case {case:?}: expected {identifier} to reach \"available\" within {CLOCK_BUDGET:?}, \
             it is still {state:?}"
        );
        tokio::time::sleep(STATE_POLL_INTERVAL).await;
    }
}

/// One `UpdateConnection`, required to succeed, answering the stored `Connection` it
/// reports.
async fn update(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    bandwidth: Option<&str>,
    description: Option<&str>,
    case: &str,
) -> Value {
    let mut request = Map::new();
    request.insert("identifier".to_string(), json!(identifier));
    if let Some(bandwidth) = bandwidth {
        request.insert("bandwidth".to_string(), json!(bandwidth));
    }
    if let Some(description) = description {
        request.insert("description".to_string(), json!(description));
    }
    let (status, body) = emulator
        .ic_in(region, "UpdateConnection", Value::Object(request))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateConnection(bandwidth={bandwidth:?}, \
         description={description:?}), got {status} with {body:?}"
    );
    body["connection"].clone()
}

/// RULE: every update of the connection record leaves the proposal its activation key
/// names exactly as the creation request captured it, while the record itself reports
/// what the update asked for.
#[tokio::test]
async fn updating_a_connection_leaves_the_proposal_its_key_captured() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = capture_table(&catalog, &gateways);
    assert!(
        table.len() >= 4,
        "the capture table must span several environments and bandwidths, got {} rows",
        table.len()
    );

    for case in &table {
        let label = case.label();

        // A SIBLING proposal of the same environment and region, created with the very
        // bandwidth this row's first update asks for: once the update has landed the two
        // connections hold the same bandwidth, so a describe that read the live record
        // would answer the same for both keys.
        let sibling = emulator
            .ic_ok_in(
                case.region,
                "CreateConnection",
                case.create_request(&case.updates[0], "capture sibling"),
            )
            .await["connection"]
            .clone();
        let sibling_key = text(&sibling, "activationKey");

        let created = emulator
            .ic_ok_in(
                case.region,
                "CreateConnection",
                case.create_request(&case.created_with, "capture subject"),
            )
            .await["connection"]
            .clone();
        let activation_key = text(&created, "activationKey");
        let identifier = text(&created, "id");

        // The baseline: the proposal as the creation request captured it.
        let baseline = describe(&emulator, case.region, &activation_key).await;
        let captured = text(&baseline, "bandwidth");
        assert_eq!(
            captured, case.created_with,
            "case {label:?}: expected the proposal to report the created bandwidth, got {captured:?}"
        );
        assert_eq!(
            text(&baseline, "environmentId"),
            case.environment_id(),
            "case {label:?}: expected the proposal to report the created environment, got {:?}",
            baseline["environmentId"]
        );

        if case.update_once_provisioned {
            wait_until_provisioned(&emulator, case.region, &identifier, &label).await;
        }

        // Every update of the row, each asked for from a state the service permits an
        // update from.
        for (step, wanted) in case.updates.iter().enumerate() {
            if step > 0 {
                wait_until_updatable(&emulator, case.region, &identifier, &label).await;
            }
            let updated = update(
                &emulator,
                case.region,
                &identifier,
                Some(wanted),
                None,
                &label,
            )
            .await;
            let reported = text(&updated, "bandwidth");
            assert_eq!(
                reported, *wanted,
                "case {label:?}: expected update {step} to report the bandwidth it asked for, got {reported:?}"
            );

            // The key still names ITS proposal, with the parameters the creation request
            // captured — checked after each step, so no single write reaches them.
            let described = describe(&emulator, case.region, &activation_key).await;
            assert_eq!(
                described, baseline,
                "case {label:?}: expected the proposal to survive update {step} unchanged, got {described:?}"
            );
        }

        // An update that carries only a description is the same answer: the proposal's
        // parameters are not what a write to the record touches.
        wait_until_updatable(&emulator, case.region, &identifier, &label).await;
        let recaptioned = format!("recaptioned {}", case.created_with);
        let renamed = update(
            &emulator,
            case.region,
            &identifier,
            None,
            Some(&recaptioned),
            &label,
        )
        .await;
        assert_eq!(
            text(&renamed, "description"),
            recaptioned,
            "case {label:?}: expected the update to report the description it asked for, got {:?}",
            renamed["description"]
        );

        // The whole proposal, after every write: byte for byte the baseline, twice over,
        // because describing is a read.
        let after = describe(&emulator, case.region, &activation_key).await;
        assert_eq!(
            after, baseline,
            "case {label:?}: expected the proposal to report the captured parameters after every update, got {after:?}"
        );
        let again = describe(&emulator, case.region, &activation_key).await;
        assert_eq!(
            again, after,
            "case {label:?}: expected a repeated describe to answer identically, got {again:?}"
        );

        // While the connection reports what the updates asked for: the two views are
        // different things, and only the record moved.
        let record = read_back(&emulator, case.region, &identifier).await;
        let live = text(&record, "bandwidth");
        assert_eq!(
            live,
            case.final_bandwidth(),
            "case {label:?}: expected the connection to report the last bandwidth asked for, got {live:?}"
        );
        if case.final_bandwidth() != case.created_with {
            assert_ne!(
                live, captured,
                "case {label:?}: expected the connection's bandwidth to differ from the proposal's captured {captured:?}, got {live:?}"
            );
        }
        assert_eq!(
            text(&record, "environmentId"),
            text(&baseline, "environmentId"),
            "case {label:?}: expected the connection and its proposal to name one environment, got {:?} vs {:?}",
            record["environmentId"],
            baseline["environmentId"]
        );

        // And the sibling key still names its OWN proposal, created with the bandwidth
        // this row updated to: each key reports the request that minted it.
        let sibling_proposal = describe(&emulator, case.region, &sibling_key).await;
        assert_eq!(
            text(&sibling_proposal, "bandwidth"),
            case.updates[0],
            "case {label:?}: expected the sibling proposal to report its own captured bandwidth, got {:?}",
            sibling_proposal["bandwidth"]
        );
        assert_eq!(
            text(&sibling_proposal, "environmentId"),
            case.environment_id(),
            "case {label:?}: expected the sibling proposal to report its own environment, got {:?}",
            sibling_proposal["environmentId"]
        );
    }
}

/// RULE: the same capture answers for a connection whose proposal has been ACCEPTED and
/// whose bandwidth was changed afterwards — the Environment and bandwidth of the proposal
/// "cannot be modified", so the key keeps reporting what the creation request captured
/// however the connection the acceptance made is later updated.
#[tokio::test]
async fn an_accepted_proposal_still_reports_its_captured_parameters() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = capture_table(&catalog, &gateways);
    assert!(
        table.len() >= 4,
        "the capture table must span several environments and bandwidths, got {} rows",
        table.len()
    );

    for case in &table {
        let label = format!("{} / updated after the acceptance", case.label());

        let created = emulator
            .ic_ok_in(
                case.region,
                "CreateConnection",
                case.create_request(&case.created_with, "capture accepted"),
            )
            .await["connection"]
            .clone();
        let activation_key = text(&created, "activationKey");
        let identifier = text(&created, "id");
        let baseline = describe(&emulator, case.region, &activation_key).await;

        // The acceptance, as the very next call after the create: a proposal is only
        // awaiting acceptance until the clock hands it on.
        let accepted = emulator
            .ic_ok_in(
                case.region,
                "AcceptConnectionProposal",
                json!({
                    "activationKey": activation_key,
                    "attachPoint": { "directConnectGateway": case.attach_point },
                }),
            )
            .await["connection"]
            .clone();
        assert_eq!(
            text(&accepted, "bandwidth"),
            case.created_with,
            "case {label:?}: expected the acceptance to leave the proposal's bandwidth, got {:?}",
            accepted["bandwidth"]
        );

        // Then the connection the acceptance made is updated, once the clock has
        // provisioned it.
        wait_until_provisioned(&emulator, case.region, &identifier, &label).await;
        let updated = update(
            &emulator,
            case.region,
            &identifier,
            Some(&case.updates[0]),
            None,
            &label,
        )
        .await;
        assert_eq!(
            text(&updated, "bandwidth"),
            case.updates[0],
            "case {label:?}: expected the update to report the bandwidth it asked for, got {:?}",
            updated["bandwidth"]
        );

        // The key reports the creation request's parameters still.
        let described = describe(&emulator, case.region, &activation_key).await;
        assert_eq!(
            described, baseline,
            "case {label:?}: expected the captured parameters to survive the acceptance and the update, got {described:?}"
        );
        assert_eq!(
            text(&described, "bandwidth"),
            case.created_with,
            "case {label:?}: expected the proposal to still report the created bandwidth, got {:?}",
            described["bandwidth"]
        );
    }
}
