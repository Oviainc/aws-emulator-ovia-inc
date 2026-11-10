// SPDX-License-Identifier: CC0-1.0

//! `RejectClientVpcConnection` — the target cluster's owner rejecting one of the
//! client (PrivateLink) VPC connections made to that cluster, driven end to end
//! over the same `MultiServiceRouter` the emulator serves from (the shared
//! `kafka_harness`).
//!
//! RULE: a reject takes the ONE connection its `VpcConnectionArn` names, and
//! that connection must be one of the connections made to the cluster its
//! `ClusterArn` names: that connection stops carrying traffic at once
//! (`REJECTING`) and the clock then owes it `REJECTED`, which is the state every
//! reader of the record reports — `DescribeVpcConnection`,
//! `ListVpcConnections` and the cluster's own `ListClientVpcConnections` — while
//! no other connection of that cluster or of any other changes; the response
//! carries nothing, so the state the readers report is where the call is
//! observable. A reference the service cannot act on is refused 400
//! `BadRequestException` naming the member at fault, and a refused reject
//! changes nothing.
//!
//! Each suite asserts that rule over a TABLE of inputs rather than one request:
//! the rows differ along what the rule generalises over (authentication type,
//! how many connections the cluster holds, which of them is rejected, the
//! signing region, how many times the reject is repeated, and which reference
//! is unusable).

use std::collections::BTreeSet;

use service_com_amazonaws_kafka_kafka::state::authorization::caller_principal;
use service_com_amazonaws_kafka_kafka::state::{
    BAD_REQUEST_MESSAGE, CLUSTER_ARN_MEMBER, CLUSTER_ARN_NOT_VALID_MESSAGE,
    VPC_CONNECTION_ARN_MEMBER, VPC_CONNECTION_AUTHENTICATION_TYPES,
};

use crate::kafka_harness::{
    sleep_past_activation, sleep_past_vpc_connection_activation,
    sleep_past_vpc_connection_rejection, ClusterRequest, ConnectionRequest, Emulator, Reply,
    ACCOUNT, REGION,
};

/// The `VpcConnectionState` a connection in service carries.
const AVAILABLE: &str = "AVAILABLE";

/// The `VpcConnectionState` of a connection whose rejection is still being
/// applied, and the one it settles in. Both are values of the model's
/// `VpcConnectionState` enum
/// (`smithy_query(query="shape",shape="VpcConnectionState")`), spelled out here
/// so this suite does not read its expectation off the implementation.
const REJECTING: &str = "REJECTING";

/// See [`REJECTING`].
const REJECTED: &str = "REJECTED";

/// The sentence Amazon MSK refuses a VPC-connection operation on a cluster whose
/// state does not serve one with: fixed text end to end, naming the states it is
/// served from and nothing of the request.
const INVALID_CLUSTER_STATE_MESSAGE: &str =
    "Invalid cluster state. To create, delete or reject a VPC connection, your cluster must be \
     in one of these states: ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER.";

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_CLUSTER_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// A connection id of the same form, for an ARN naming a VPC connection no
/// registry of this emulator holds; the suite that needs a second such ARN uses
/// [`OTHER_GHOST_CONNECTION_ID`].
const GHOST_CONNECTION_ID: &str = "99999999-8888-4777-8666-555555555555-9";

/// See [`GHOST_CONNECTION_ID`].
const OTHER_GHOST_CONNECTION_ID: &str = "12121212-3434-4545-8656-767676767676-3";

/// A well-formed MSK cluster ARN of this account and Region that names no
/// cluster.
fn ghost_cluster_arn() -> String {
    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/ghost-cluster/{GHOST_CLUSTER_ID}")
}

/// A well-formed MSK VPC-connection ARN of this account and Region that names no
/// connection, under the connection id `id`.
fn ghost_connection_arn(id: &str) -> String {
    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/ghost-cluster/{id}")
}

/// The state `DescribeVpcConnection` reports for `arn` at `region`'s endpoint.
async fn described_state(emulator: &Emulator, region: &str, case: &str, arn: &str) -> String {
    let reply = emulator.describe_vpc_connection_in(region, arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeVpcConnection to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/state")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the reader reported no state: {}", reply.body))
        .to_string()
}

/// The state each listing reports for `arn`, keyed by the listing's name, so a
/// suite can assert the two listings and the reader agree about one record.
async fn listed_states(
    emulator: &Emulator,
    region: &str,
    case: &str,
    cluster_arn: &str,
    arn: &str,
) -> Vec<(&'static str, String)> {
    let client = emulator
        .list_client_vpc_connections_in(region, cluster_arn, "")
        .await;
    let regional = emulator.list_vpc_connections_in(region, "").await;
    let mut found = Vec::new();
    for (name, reply, member) in [
        ("ListClientVpcConnections", client, "clientVpcConnections"),
        ("ListVpcConnections", regional, "vpcConnections"),
    ] {
        assert_eq!(
            reply.status, 200,
            "case {case}: expected {name} to return 200, got {} {}",
            reply.status, reply.body
        );
        let entries = reply
            .at(&format!("/{member}"))
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: {name} reported no {member} array: {}", reply.body)
            })
            .clone();
        let entry = entries
            .iter()
            .find(|entry| entry["vpcConnectionArn"].as_str() == Some(arn))
            .unwrap_or_else(|| panic!("case {case}: {name} did not report {arn}: {entries:?}"));
        found.push((
            name,
            entry["state"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: {name} reported no state: {entry}"))
                .to_string(),
        ));
    }
    found
}

/// Assert the reader and both listings report `expected` for `arn`.
async fn assert_every_reader_reports(
    emulator: &Emulator,
    region: &str,
    case: &str,
    cluster_arn: &str,
    arn: &str,
    expected: &str,
) {
    let described = described_state(emulator, region, case, arn).await;
    assert_eq!(
        described, expected,
        "case {case}: expected DescribeVpcConnection to report {expected}, got {described:?}"
    );
    for (listing, state) in listed_states(emulator, region, case, cluster_arn, arn).await {
        assert_eq!(
            state, expected,
            "case {case}: expected {listing} to report {expected}, got {state:?}"
        );
    }
}

/// Assert a reject succeeded and reported nothing: the model gives
/// `RejectClientVpcConnectionResponse` no members, so an answer carrying one
/// would be reporting something the API does not.
fn assert_accepted(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected RejectClientVpcConnection to return 200, got {} {}",
        reply.status, reply.body
    );
    let reported = reply
        .body
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<String>>())
        .unwrap_or_default();
    assert!(
        reported.is_empty(),
        "case {case}: the response is modelled empty, but it reported {reported:?}"
    );
}

/// Assert a reject was refused as a bad parameter, naming `member` and carrying
/// `message`.
fn assert_bad_parameter(reply: &Reply, case: &str, member: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected the message {message:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(member),
        "case {case}: expected {member} as the parameter at fault, got {}",
        reply.body
    );
}

/// One row of the table below: a cluster holding `connections` connections, each
/// asking for `authentication`, of which the one at `reject_index` is rejected.
#[derive(Debug)]
struct Row {
    case: &'static str,
    region: &'static str,
    authentication: &'static str,
    connections: usize,
    reject_index: usize,
}

/// RULE: the reject moves the named connection out of service — `REJECTING` at
/// once, `REJECTED` once the clock owes it — and leaves every other connection
/// of the cluster exactly as it was, whichever connection of however many was
/// named.
#[tokio::test]
async fn reject_moves_the_named_connection_out_of_service() {
    let mut rows = vec![
        Row {
            case: "one-connection",
            region: REGION,
            authentication: "SASL_IAM",
            connections: 1,
            reject_index: 0,
        },
        Row {
            case: "three-connections-reject-the-first",
            region: REGION,
            authentication: "SASL_SCRAM",
            connections: 3,
            reject_index: 0,
        },
        Row {
            case: "three-connections-reject-the-last",
            region: REGION,
            authentication: "TLS",
            connections: 3,
            reject_index: 2,
        },
        Row {
            // Another signing region, where the cluster, its connections and the
            // listings that report them all live.
            case: "two-connections-reject-the-second-in-another-region",
            region: "eu-central-1",
            authentication: "SASL_IAM",
            connections: 2,
            reject_index: 1,
        },
    ];
    // Every authentication type the service publishes is covered, including any
    // this file did not spell out above.
    for authentication in VPC_CONNECTION_AUTHENTICATION_TYPES {
        if !rows.iter().any(|row| row.authentication == authentication) {
            rows.push(Row {
                case: "a-published-authentication-type-not-spelled-out-above",
                region: REGION,
                authentication,
                connections: 1,
                reject_index: 0,
            });
        }
    }

    for row in &rows {
        // A cluster and an engine of this row's own, so the rows cannot interfere.
        let emulator = Emulator::new();
        let cluster = emulator
            .vpc_connection_target_in(row.region, "reject-target")
            .await;
        let mut arns = Vec::new();
        for _ in 0..row.connections {
            arns.push(
                emulator
                    .create_vpc_connection_ok_in(
                        row.region,
                        &ConnectionRequest {
                            authentication: row.authentication,
                            ..ConnectionRequest::to(&cluster)
                        },
                    )
                    .await,
            );
        }
        sleep_past_vpc_connection_activation();
        // The control: every connection is in service before the reject, so the
        // rows below are a transition rather than a state they already held.
        for arn in &arns {
            assert_every_reader_reports(
                &emulator,
                row.region,
                row.case,
                &cluster,
                arn,
                AVAILABLE,
            )
            .await;
        }

        let rejected = arns[row.reject_index].clone();
        let reply = emulator
            .reject_client_vpc_connection_in(row.region, &cluster, &rejected)
            .await;
        assert_accepted(&reply, row.case);

        // Immediately: the connection has left service, and the clock still owes
        // it the settled state.
        assert_every_reader_reports(
            &emulator,
            row.region,
            row.case,
            &cluster,
            &rejected,
            REJECTING,
        )
        .await;
        sleep_past_vpc_connection_rejection();
        assert_every_reader_reports(
            &emulator,
            row.region,
            row.case,
            &cluster,
            &rejected,
            REJECTED,
        )
        .await;

        // Every other connection of the cluster is untouched.
        for arn in arns.iter().filter(|arn| **arn != rejected) {
            assert_every_reader_reports(
                &emulator,
                row.region,
                row.case,
                &cluster,
                arn,
                AVAILABLE,
            )
            .await;
        }
    }
}

/// RULE: the call asks for a state rather than for a step, so repeating it
/// changes nothing — a connection already rejected keeps the rejection it
/// carries, whether that rejection is still in flight or already settled.
#[tokio::test]
async fn repeating_the_reject_changes_nothing() {
    // The rows differ along how many times the reject is repeated and whether
    // the repeats come before or after the clock owes the settled state.
    let rows: [(&str, usize, bool); 4] = [
        ("repeated-once-while-rejecting", 1, false),
        ("repeated-twice-while-rejecting", 2, false),
        ("repeated-once-after-settling", 1, true),
        ("repeated-three-times-after-settling", 3, true),
    ];

    for (case, repeats, after_settling) in rows {
        let emulator = Emulator::new();
        let cluster = emulator.vpc_connection_target("repeat-target").await;
        let arn = emulator
            .create_vpc_connection_ok(&ConnectionRequest::to(&cluster))
            .await;
        sleep_past_vpc_connection_activation();
        assert_accepted(
            &emulator.reject_client_vpc_connection(&cluster, &arn).await,
            case,
        );
        if after_settling {
            sleep_past_vpc_connection_rejection();
        }
        let expected = if after_settling { REJECTED } else { REJECTING };
        for repeat in 0..repeats {
            let reply = emulator.reject_client_vpc_connection(&cluster, &arn).await;
            assert_accepted(&reply, case);
            let state = described_state(&emulator, REGION, case, &arn).await;
            assert_eq!(
                state, expected,
                "case {case}: repeat {repeat} must leave the state at {expected}, got {state:?}"
            );
        }
        // However many times it was asked for, the rejection settles once and
        // stays settled.
        sleep_past_vpc_connection_rejection();
        assert_every_reader_reports(&emulator, REGION, case, &cluster, &arn, REJECTED).await;
    }
}

/// RULE: a reject whose `ClusterArn` or `VpcConnectionArn` names nothing this
/// endpoint can act on — including a connection made to a DIFFERENT cluster — is
/// refused 400 `BadRequestException` naming the member at fault, and changes no
/// connection.
#[tokio::test]
async fn reject_refuses_a_reference_it_cannot_act_on() {
    let emulator = Emulator::new();
    let cluster = emulator.vpc_connection_target("refusal-target").await;
    let other_cluster = emulator.vpc_connection_target("refusal-other").await;
    let connection = emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&cluster))
        .await;
    let other_connection = emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&other_cluster))
        .await;
    sleep_past_vpc_connection_activation();

    // The rows walk both references through every form the service cannot act
    // on: a value of no ARN shape, an ARN of the wrong kind, a well-formed ARN
    // naming nothing, and a connection belonging to another cluster.
    let rows: Vec<(&str, String, String, &str, &str)> = vec![
        (
            "a-cluster-arn-that-is-not-an-arn",
            "not-an-arn".to_string(),
            connection.clone(),
            CLUSTER_ARN_MEMBER,
            CLUSTER_ARN_NOT_VALID_MESSAGE,
        ),
        (
            "a-cluster-arn-of-another-resource-kind",
            connection.clone(),
            connection.clone(),
            CLUSTER_ARN_MEMBER,
            CLUSTER_ARN_NOT_VALID_MESSAGE,
        ),
        (
            "a-cluster-no-registry-holds",
            ghost_cluster_arn(),
            connection.clone(),
            CLUSTER_ARN_MEMBER,
            BAD_REQUEST_MESSAGE,
        ),
        (
            "a-connection-arn-that-is-not-an-arn",
            cluster.clone(),
            "not-an-arn".to_string(),
            VPC_CONNECTION_ARN_MEMBER,
            BAD_REQUEST_MESSAGE,
        ),
        (
            "a-connection-arn-of-another-resource-kind",
            cluster.clone(),
            cluster.clone(),
            VPC_CONNECTION_ARN_MEMBER,
            BAD_REQUEST_MESSAGE,
        ),
        (
            "a-connection-no-registry-holds",
            cluster.clone(),
            ghost_connection_arn(GHOST_CONNECTION_ID),
            VPC_CONNECTION_ARN_MEMBER,
            BAD_REQUEST_MESSAGE,
        ),
        (
            "a-connection-made-to-another-cluster",
            cluster.clone(),
            other_connection.clone(),
            VPC_CONNECTION_ARN_MEMBER,
            BAD_REQUEST_MESSAGE,
        ),
    ];

    for (case, cluster_arn, connection_arn, member, message) in &rows {
        let reply = emulator
            .reject_client_vpc_connection(cluster_arn, connection_arn)
            .await;
        assert_bad_parameter(&reply, case, member, message);
        // Nothing moved: neither the connection the row named nor the other one.
        for (owner, arn) in [(&cluster, &connection), (&other_cluster, &other_connection)] {
            assert_every_reader_reports(&emulator, REGION, case, owner, arn, AVAILABLE).await;
        }
    }

    // A regional endpoint acts only on its own Region's clusters, so the same
    // live pair is unusable from another Region's endpoint — and the refusal is
    // about the cluster, which is what that endpoint cannot resolve.
    let elsewhere = emulator
        .reject_client_vpc_connection_in("eu-central-1", &cluster, &connection)
        .await;
    assert_bad_parameter(
        &elsewhere,
        "another-regions-endpoint",
        CLUSTER_ARN_MEMBER,
        BAD_REQUEST_MESSAGE,
    );
    assert_every_reader_reports(
        &emulator,
        REGION,
        "another-regions-endpoint",
        &cluster,
        &connection,
        AVAILABLE,
    )
    .await;

    // The resource-scoped decision comes before every row above: a cluster of
    // another account reaches no grant this caller holds, so such a call is
    // refused as unauthorized rather than answered about either reference.
    let foreign =
        format!("arn:aws:kafka:{REGION}:210987654321:cluster/foreign/{GHOST_CLUSTER_ID}");
    let denied = emulator
        .reject_client_vpc_connection(&foreign, &connection)
        .await;
    assert_eq!(
        (denied.status, denied.error_code()),
        (403, "AccessDeniedException"),
        "case another-accounts-cluster: expected 403 AccessDeniedException, got {} {}",
        denied.status,
        denied.body
    );
    let expected = format!(
        "User: {} is not authorized to perform: kafka:RejectClientVpcConnection on resource: \
         {foreign} because no resource-based policy allows the \
         kafka:RejectClientVpcConnection action",
        caller_principal()
    );
    assert_eq!(
        denied.error_message(),
        expected,
        "case another-accounts-cluster: expected {expected:?}, got {:?}",
        denied.error_message()
    );

    // The control that makes the rows above references rather than a blanket no:
    // the live pair is rejected.
    assert_accepted(
        &emulator
            .reject_client_vpc_connection(&cluster, &connection)
            .await,
        "control",
    );
}

/// RULE: a cluster whose state does not serve a VPC-connection operation does
/// not serve the reject either, refused with MSK's one fixed sentence for that
/// condition — decided before the connection the request names is resolved, so
/// the answer does not depend on it.
#[tokio::test]
async fn reject_is_refused_while_the_cluster_does_not_manage_its_connections() {
    let emulator = Emulator::new();
    // A settled cluster with a live connection on it comes FIRST, so waiting for
    // it cannot settle the cluster under test: that one is created last and left
    // in the state its create answered.
    let settled_elsewhere = emulator.vpc_connection_target("state-gate-other").await;
    let live_elsewhere = emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&settled_elsewhere))
        .await;
    // `CREATING`, which is not one of the states MSK manages a cluster's VPC
    // connections from.
    let creating = emulator
        .create_ok(&ClusterRequest::vpc_connection_target("state-gate"))
        .await;

    // The rows differ along the connection reference, which the gate must not
    // consult: every well-formed reference is told the same thing, whether or
    // not it names a connection at all.
    let rows: Vec<(&str, String)> = vec![
        (
            "a-connection-no-registry-holds",
            ghost_connection_arn(GHOST_CONNECTION_ID),
        ),
        (
            "another-connection-no-registry-holds",
            ghost_connection_arn(OTHER_GHOST_CONNECTION_ID),
        ),
        ("a-connection-made-to-another-cluster", live_elsewhere),
    ];
    let mut answered: BTreeSet<String> = BTreeSet::new();
    for (case, connection_arn) in &rows {
        let reply = emulator
            .reject_client_vpc_connection(&creating, connection_arn)
            .await;
        assert_bad_parameter(&reply, case, CLUSTER_ARN_MEMBER, INVALID_CLUSTER_STATE_MESSAGE);
        answered.insert(reply.error_message().to_string());
    }
    let distinct: Vec<&String> = answered.iter().collect();
    assert_eq!(
        distinct,
        vec![&INVALID_CLUSTER_STATE_MESSAGE.to_string()],
        "expected one fixed cluster-state refusal across every row, got {distinct:?}"
    );

    // Once the clock owes the cluster `ACTIVE`, the gate opens: the same request
    // is answered about the CONNECTION it names instead, and a connection the
    // cluster really holds is rejected.
    sleep_past_activation();
    let after = emulator
        .reject_client_vpc_connection(&creating, &ghost_connection_arn(GHOST_CONNECTION_ID))
        .await;
    assert_bad_parameter(
        &after,
        "control after the cluster settles",
        VPC_CONNECTION_ARN_MEMBER,
        BAD_REQUEST_MESSAGE,
    );
    let connection = emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&creating))
        .await;
    assert_accepted(
        &emulator
            .reject_client_vpc_connection(&creating, &connection)
            .await,
        "control after the cluster settles",
    );
}
