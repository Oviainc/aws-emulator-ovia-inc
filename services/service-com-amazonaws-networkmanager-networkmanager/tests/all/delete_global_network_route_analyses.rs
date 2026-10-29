// SPDX-License-Identifier: CC0-1.0

//! What `DeleteGlobalNetwork` takes with the global network it removes, driven over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a record the store holds only as a record OF a resource — one with
//! no identity outside that resource and no operation of this service that removes it — goes
//! WITH the resource when it is deleted, while a dependent the caller can remove itself
//! blocks the delete until the caller does. A route analysis is the first kind: it is
//! addressed only through the global network it was started on, and this service models
//! `StartRouteAnalysis` and `GetRouteAnalysis` and no removal at all, so a global network's
//! analyses never block its delete and never outlive it.
//!
//! The rows span what the rule generalises over: how many analyses the global network holds
//! (none, one, several), the shape of each analysis (which endpoint carries an attachment
//! ARN, whether an IP address is given, and the two flags), and whether a dependent that
//! DOES block — a site — is held beside them, so the refusing and the succeeding delete are
//! both driven over the same analyses.
//!
//! Every analysis is started and read back over the wire. The one thing a wire read cannot
//! see is a record that survived the delete unreachably — the defect this pins — so the
//! store itself is asked, through `crate::state`, whether anything is left behind.

use serde_json::{json, Value};
use service_com_amazonaws_networkmanager_networkmanager::state;

use crate::global_network_lifecycle::{error_code, Emulator};

/// The Region the attachment ARNs of these analyses name.
const REGION: &str = "us-west-2";

/// An Amazon VPC transit gateway attachment ARN: the kind of ARN a route analysis endpoint
/// names ("The ARN of the transit gateway attachment", `smithy_query query="doc"
/// shape="RouteAnalysisEndpointOptions" member="TransitGatewayAttachmentArn"`). What each
/// endpoint resolves to is `crate::route_analysis`'s subject; here only the analysis RECORD
/// matters, so one ARN shape per index is enough.
fn attachment_arn(index: usize) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:transit-gateway-attachment/tgw-attach-{index:017x}",
        engine::emulator_account_id()
    )
}

/// The `StartRouteAnalysis` URI of `network`.
fn analyses_uri(network: &str) -> String {
    format!("/global-networks/{network}/route-analyses")
}

/// The `GetRouteAnalysis` URI of `analysis` under `network`.
fn analysis_uri(network: &str, analysis: &str) -> String {
    format!("/global-networks/{network}/route-analyses/{analysis}")
}

/// The request bodies this suite starts analyses from, differing in which endpoint carries
/// an attachment ARN, whether an IP address is given, and the two flags — every shape the
/// records under test can be created in.
fn analysis_bodies(seed: usize) -> Vec<Value> {
    vec![
        json!({
            "Source": { "TransitGatewayAttachmentArn": attachment_arn(seed + 1) },
            "Destination": { "TransitGatewayAttachmentArn": attachment_arn(seed + 2) },
        }),
        json!({
            "Source": {
                "TransitGatewayAttachmentArn": attachment_arn(seed + 3),
                "IpAddress": "10.0.1.7",
            },
            "Destination": { "TransitGatewayAttachmentArn": attachment_arn(seed + 4) },
            "IncludeReturnPath": true,
        }),
        json!({
            "Source": { "IpAddress": "10.0.2.9" },
            "Destination": { "TransitGatewayAttachmentArn": attachment_arn(seed + 5) },
            "UseMiddleboxes": true,
        }),
        json!({
            "Source": { "TransitGatewayAttachmentArn": attachment_arn(seed + 6) },
            "Destination": { "IpAddress": "10.0.3.11" },
            "IncludeReturnPath": true,
            "UseMiddleboxes": true,
        }),
    ]
}

/// One row: how many analyses the global network is made to hold, and how many sites —
/// dependents the caller removes itself — are held beside them.
struct Case {
    name: &'static str,
    analyses: usize,
    sites: usize,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no analysis at all",
            analyses: 0,
            sites: 0,
        },
        Case {
            name: "one analysis",
            analyses: 1,
            sites: 0,
        },
        Case {
            name: "four analyses of every request shape",
            analyses: 4,
            sites: 0,
        },
        Case {
            name: "two analyses beside a site that blocks the delete",
            analyses: 2,
            sites: 1,
        },
        Case {
            name: "four analyses beside two sites that block the delete",
            analyses: 4,
            sites: 2,
        },
    ]
}

/// `StartRouteAnalysis` that must succeed, answering the minted analysis id.
async fn start_analysis(emulator: &Emulator, case: &str, network: &str, body: Value) -> String {
    let (status, created) = emulator.post(&analyses_uri(network), body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartRouteAnalysis expected 200, got {status} with {created}"
    );
    created["RouteAnalysis"]["RouteAnalysisId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RouteAnalysisId in {created}"))
        .to_string()
}

/// Start one analysis per body in `analysis_bodies`, cycling the shapes for a row that asks
/// for more than there are.
async fn start_analyses(
    emulator: &Emulator,
    case: &str,
    network: &str,
    count: usize,
    seed: usize,
) -> Vec<String> {
    let bodies = analysis_bodies(seed);
    let mut started = Vec::new();
    for index in 0..count {
        let body = bodies[index % bodies.len()].clone();
        started.push(start_analysis(emulator, case, network, body).await);
    }
    started
}

/// Assert every analysis of `analyses` reads back through the global network it was started
/// on.
async fn assert_readable(emulator: &Emulator, case: &str, network: &str, analyses: &[String]) {
    for analysis in analyses {
        let (status, body) = emulator.get(&analysis_uri(network, analysis)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRouteAnalysis({analysis}) expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["RouteAnalysis"]["RouteAnalysisId"].as_str(),
            Some(analysis.as_str()),
            "case {case:?}: expected the analysis {analysis}, got {body}"
        );
        assert_eq!(
            body["RouteAnalysis"]["GlobalNetworkId"].as_str(),
            Some(network),
            "case {case:?}: the analysis must report the global network it was started on, \
             got {body}"
        );
    }
}

/// How many route analyses the store still holds for `network`.
///
/// Asked of the store rather than over the wire on purpose: an analysis of a global network
/// that has been deleted is reachable through NO operation, so a surviving one is invisible
/// to every read a client can make.
fn held_by(engine: &engine::InMemoryEngine, network: &str) -> usize {
    let store = state::service_state(engine).expect("the networkmanager store must be reachable");
    let held = store.lock().route_analysis_count_of(network);
    held
}

/// How many route analyses the store holds in total, across every global network.
fn held_in_total(engine: &engine::InMemoryEngine) -> usize {
    let store = state::service_state(engine).expect("the networkmanager store must be reachable");
    let held = store.lock().route_analyses.len();
    held
}

/// `DeleteGlobalNetwork`.
async fn delete_global_network(emulator: &Emulator, network: &str) -> (u16, Value) {
    emulator.delete(&format!("/global-networks/{network}")).await
}

#[tokio::test]
async fn deleting_a_global_network_takes_the_route_analyses_it_holds_with_it() {
    let (emulator, engine) = Emulator::with_engine();

    // A global network no delete below names, holding analyses of its own: the delete must
    // reach exactly the records of the global network it removes.
    let control = emulator.create_global_network("kept global network").await;
    let control_analyses = start_analyses(&emulator, "kept global network", &control, 3, 0x100).await;

    for case in cases() {
        let name = case.name;
        let network = emulator.create_global_network(name).await;
        let analyses = start_analyses(&emulator, name, &network, case.analyses, 0).await;
        let mut sites = Vec::new();
        for index in 0..case.sites {
            sites.push(emulator.create_site(&network, &format!("{name}-{index}")).await);
        }

        assert_readable(&emulator, name, &network, &analyses).await;
        assert_eq!(
            held_by(&engine, &network),
            analyses.len(),
            "case {name:?}: expected the store to hold {} analyses before the delete, got {}",
            analyses.len(),
            held_by(&engine, &network)
        );
        let total_before = held_in_total(&engine);

        if !sites.is_empty() {
            // A site is a dependent the caller removes itself, so it blocks — and a refused
            // delete removes nothing, analyses included.
            let (status, refused) = delete_global_network(&emulator, &network).await;
            assert_eq!(
                status, 400,
                "case {name:?}: a held site must refuse the delete, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {name:?}: expected the dependency ValidationException, got {refused}"
            );
            assert_readable(&emulator, name, &network, &analyses).await;
            assert_eq!(
                held_by(&engine, &network),
                analyses.len(),
                "case {name:?}: a refused delete must leave every analysis stored, got {}",
                held_by(&engine, &network)
            );
            for site in &sites {
                let (status, deleted) = emulator
                    .delete(&format!("/global-networks/{network}/sites/{site}"))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: DeleteSite({site}) expected 200, got {status} with {deleted}"
                );
            }
        }

        // The analyses themselves never block: there is no call that would empty them first.
        let (status, deleted) = delete_global_network(&emulator, &network).await;
        assert_eq!(
            status, 200,
            "case {name:?}: held analyses must not block the delete, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["GlobalNetwork"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record reports DELETING, got {deleted}"
        );

        assert_eq!(
            held_by(&engine, &network),
            0,
            "case {name:?}: the delete must leave no analysis of the removed global network, \
             got {}",
            held_by(&engine, &network)
        );
        assert_eq!(
            held_in_total(&engine),
            total_before - analyses.len(),
            "case {name:?}: the delete must drop exactly the {} analyses it held, got a total \
             of {} where {} were stored before",
            analyses.len(),
            held_in_total(&engine),
            total_before
        );

        // And over the wire the analyses are gone with their parent: the read is refused on
        // the global network the request addressed them through.
        for analysis in &analyses {
            let (status, orphan) = emulator.get(&analysis_uri(&network, analysis)).await;
            assert_eq!(
                status, 404,
                "case {name:?}: GetRouteAnalysis({analysis}) after the delete expected 404, got \
                 {status} with {orphan}"
            );
            assert_eq!(
                error_code(&orphan),
                "ResourceNotFoundException",
                "case {name:?}: expected ResourceNotFoundException for {analysis}, got {orphan}"
            );
            assert_eq!(
                orphan["ResourceType"].as_str(),
                Some(state::GLOBAL_NETWORK_KIND),
                "case {name:?}: the refusal must name the global network as the resource, got \
                 {orphan}"
            );
        }

        // The untouched global network kept every analysis it held.
        assert_readable(&emulator, name, &control, &control_analyses).await;
        assert_eq!(
            held_by(&engine, &control),
            control_analyses.len(),
            "case {name:?}: the delete must reach no analysis of another global network, got {}",
            held_by(&engine, &control)
        );
    }
}

#[tokio::test]
async fn a_create_analyse_delete_round_leaves_the_store_no_bigger_than_it_found_it() {
    let (emulator, engine) = Emulator::with_engine();

    // Rows differing in how many analyses one round starts, so the invariant is driven over
    // a round that holds none, one and several: whatever a round starts, the delete that
    // ends it leaves the store exactly as the round found it.
    for (round, per_round) in [0usize, 1, 4, 2, 4].into_iter().enumerate() {
        let case = format!("round {round} holding {per_round} analyses");
        assert_eq!(
            held_in_total(&engine),
            0,
            "case {case:?}: every earlier round must have left the store empty, got {}",
            held_in_total(&engine)
        );

        let network = emulator.create_global_network(&case).await;
        let analyses = start_analyses(&emulator, &case, &network, per_round, round * 0x10).await;
        assert_eq!(
            held_in_total(&engine),
            per_round,
            "case {case:?}: expected {per_round} analyses stored mid-round, got {}",
            held_in_total(&engine)
        );
        assert_readable(&emulator, &case, &network, &analyses).await;

        let (status, deleted) = delete_global_network(&emulator, &network).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteGlobalNetwork expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            held_in_total(&engine),
            0,
            "case {case:?}: a completed round must leave the store as it found it, got {}",
            held_in_total(&engine)
        );
    }
}

#[tokio::test]
async fn the_per_global_network_bound_still_holds_after_a_delete() {
    let (emulator, engine) = Emulator::with_engine();
    let cap = state::MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK;
    let body = json!({
        "Source": { "TransitGatewayAttachmentArn": attachment_arn(0x201) },
        "Destination": { "TransitGatewayAttachmentArn": attachment_arn(0x202) },
    });

    // Fill one global network to its cap, then delete it: the analyses leave with it, so the
    // store is back to holding none and the next global network has its whole cap available.
    for round in 0..2 {
        let case = format!("filling round {round}");
        let network = emulator.create_global_network(&case).await;
        for index in 0..cap {
            let (status, created) = emulator.post(&analyses_uri(&network), body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: analysis {index} of {cap} expected 200, got {status} with {created}"
            );
        }
        let (status, refused) = emulator.post(&analyses_uri(&network), body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: starting past the cap of {cap} must be refused, got {status} with \
             {refused}"
        );
        assert_eq!(
            held_in_total(&engine),
            cap,
            "case {case:?}: the refused start must record nothing, got {}",
            held_in_total(&engine)
        );

        let (status, deleted) = delete_global_network(&emulator, &network).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteGlobalNetwork expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            held_in_total(&engine),
            0,
            "case {case:?}: a global network filled to its cap must take every analysis with \
             it, got {}",
            held_in_total(&engine)
        );
    }
}
