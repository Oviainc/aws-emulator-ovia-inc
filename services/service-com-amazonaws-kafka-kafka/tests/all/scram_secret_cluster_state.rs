// SPDX-License-Identifier: CC0-1.0

//! The cluster-state pre-condition of the Amazon MSK SCRAM-secret operations,
//! driven end to end over the same router the emulator serves from (the shared
//! `kafka_harness`).
//!
//! RULE under test: a cluster's SASL/SCRAM secret set belongs to brokers that are
//! serving, so `BatchAssociateScramSecret` and `ListScramSecrets` act on it only
//! while the cluster is in one of the states that HAS that set, and the cluster's
//! state is what they answer on BEFORE its client-authentication configuration —
//! so a cluster the service is still provisioning is refused on its state
//! whatever its authentication says. `BatchDisassociateScramSecret` is the one
//! exception: withdrawing a credential is answered on the authentication
//! configuration in every state. Every refusal is HTTP 400 `BadRequestException`
//! naming `ClusterArn`, and leaves the cluster and its stored set exactly as they
//! were.
//!
//! The rows differ along what the rule generalises over: the STATE the cluster is
//! in when the call arrives (`CREATING` from the create, `ACTIVE` once the clock
//! settles it, `UPDATING` from an accepted update), the cluster's client
//! AUTHENTICATION (SASL/SCRAM on, no authentication at all, SCRAM explicitly off,
//! a different SASL mechanism on), its broker count, and which of the three
//! operations is asked for.

use serde_json::json;

// The model's own `ClusterState` enum, so the served set is checked against the
// values the service can actually report rather than a list written out here.
use service_com_amazonaws_kafka_kafka::ops::types::CLUSTER_STATE_VALUES;
// The service's own served-state set and refusal wording, so what the handlers
// raise and what this suite pins cannot drift apart silently.
use service_com_amazonaws_kafka_kafka::state::{
    invalid_cluster_state_for_scram_message, CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM,
    SASL_SCRAM_NOT_ENABLED_MESSAGE, SCRAM_SECRET_ACTION_ASSOCIATE, SCRAM_SECRET_ACTION_LIST,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The sentence Amazon MSK refuses `BatchAssociateScramSecret` with on a cluster
/// whose state does not serve its secret set: fixed text, with nothing of the
/// request in it.
const ASSOCIATE_STATE_REFUSAL: &str = "Invalid cluster state. Cluster must be in the ACTIVE, \
                                       UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state \
                                       to associate secrets.";

/// The same sentence for `ListScramSecrets`, differing only in the verb phrase
/// that names what the operation does.
const LIST_STATE_REFUSAL: &str = "Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, \
                                  MAINTENANCE, HEALING, or REBOOTING_BROKER state to list \
                                  secrets.";

/// The sentence a SCRAM-secret operation is refused with when the cluster's
/// client authentication does not turn SASL/SCRAM on. Also fixed text.
const SASL_SCRAM_REFUSAL: &str = "SASL/SCRAM client authentication must be enabled for the cluster.";

/// The states the served set leaves out, i.e. the states of a cluster that has no
/// secret set to act on: it has no brokers yet, or it is losing them.
const STATES_THAT_DO_NOT_SERVE: [&str; 3] = ["CREATING", "DELETING", "FAILED"];

/// The resource URI all three operations are bound to.
fn secrets_uri(arn: &str) -> String {
    format!("/v1/clusters/{}/scram-secrets", pct(arn))
}

/// One Secrets Manager secret ARN in the form `SecretArnList` carries.
fn secret_arn(name: &str) -> String {
    format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}")
}

async fn associate(emulator: &Emulator, cluster: &str, secrets: &[String]) -> Reply {
    emulator
        .call(
            "POST",
            &secrets_uri(cluster),
            Some(json!({ "secretArnList": secrets })),
        )
        .await
}

async fn disassociate(emulator: &Emulator, cluster: &str, secrets: &[String]) -> Reply {
    emulator
        .call(
            "PATCH",
            &secrets_uri(cluster),
            Some(json!({ "secretArnList": secrets })),
        )
        .await
}

async fn list(emulator: &Emulator, cluster: &str) -> Reply {
    emulator.call("GET", &secrets_uri(cluster), None).await
}

/// The state `DescribeClusterV2` reports for `arn`, or `""` when it reports none.
async fn reported_state(emulator: &Emulator, arn: &str) -> String {
    emulator
        .describe_v2(arn)
        .await
        .at("/clusterInfo/state")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The version `DescribeClusterV2` reports for `arn` — what an update must name.
async fn reported_version(emulator: &Emulator, arn: &str) -> String {
    emulator
        .describe_v2(arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The secrets a successful listing reports.
fn listed(reply: &Reply, case: &str) -> Vec<String> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListScramSecrets to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/secretArnList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected ListScramSecrets to report secretArnList, got {}",
                reply.body
            )
        })
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a listed secret is not a string: {entry}"))
                .to_string()
        })
        .collect()
}

/// Assert one refusal: HTTP 400, the modelled error code, the parameter at fault
/// and the exact sentence.
fn assert_refused_with(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("ClusterArn"),
        "case {case}: expected the refusal to name ClusterArn, got {}",
        reply.body
    );
}

/// One row of the cluster table.
struct ClusterRow {
    case: &'static str,
    /// Whether the cluster authenticates SASL/SCRAM clients, which is what the
    /// authentication test (and only it) is about.
    scram: bool,
    request: ClusterRequest,
}

/// The clusters every phase below walks: the rows differ along the client
/// authentication the create submits and along the cluster's broker count, so
/// neither the state refusal nor the authentication refusal is keyed to one shape
/// of cluster.
fn cluster_rows() -> Vec<ClusterRow> {
    vec![
        ClusterRow {
            case: "scram-enabled",
            scram: true,
            request: ClusterRequest {
                sasl_scram: Some(true),
                brokers: 2,
                ..ClusterRequest::named("scram-state-gate-on")
            },
        },
        ClusterRow {
            case: "no-client-authentication",
            scram: false,
            request: ClusterRequest {
                brokers: 3,
                ..ClusterRequest::named("scram-state-gate-none")
            },
        },
        ClusterRow {
            case: "scram-disabled",
            scram: false,
            request: ClusterRequest {
                sasl_scram: Some(false),
                brokers: 1,
                ..ClusterRequest::named("scram-state-gate-off")
            },
        },
        ClusterRow {
            case: "iam-only",
            scram: false,
            request: ClusterRequest {
                sasl_iam: Some(true),
                brokers: 2,
                ..ClusterRequest::named("scram-state-gate-iam")
            },
        },
    ]
}

/// RULE: the association and the listing are answered on the cluster's STATE
/// before its client authentication, so a cluster that is still being provisioned
/// refuses both on its state whatever its authentication says; the
/// disassociation answers the authentication configuration in that same state;
/// and a refused call leaves the cluster's state and its stored set alone.
#[tokio::test]
async fn the_state_is_answered_before_the_authentication_while_the_cluster_is_created() {
    let emulator = Emulator::new();
    let secrets = vec![secret_arn("AmazonMSK_state-gate-7h8i9j")];

    let rows = cluster_rows();
    let mut arns = Vec::new();
    for row in &rows {
        arns.push(emulator.create_ok(&row.request).await);
    }

    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        assert_eq!(
            reported_state(&emulator, arn).await,
            "CREATING",
            "case {case}: the cluster must read CREATING before the gate is probed"
        );

        // The state answers, not the authentication: the SCRAM-enabled row and
        // every row without SASL/SCRAM get the SAME sentence.
        assert_refused_with(
            &associate(&emulator, arn, &secrets).await,
            case,
            ASSOCIATE_STATE_REFUSAL,
        );
        assert_refused_with(&list(&emulator, arn).await, case, LIST_STATE_REFUSAL);

        // The disassociation is the exception: it answers the authentication
        // configuration in this state too, so only a cluster without SASL/SCRAM
        // is refused — and on the authentication sentence, never the state one.
        let removed = disassociate(&emulator, arn, &secrets).await;
        if row.scram {
            assert_eq!(
                removed.status, 200,
                "case {case}: expected the disassociation to be served while CREATING, got {} {}",
                removed.status, removed.body
            );
        } else {
            assert_refused_with(&removed, case, SASL_SCRAM_REFUSAL);
        }

        // No refused call moved the cluster: the state is the one the clock owes
        // it, not one a refusal wrote.
        assert_eq!(
            reported_state(&emulator, arn).await,
            "CREATING",
            "case {case}: a refused SCRAM-secret call must leave the cluster CREATING"
        );
    }

    // The cluster settles on its own, and nothing the refusals did is visible:
    // the SCRAM-enabled cluster now serves the association, and every other row
    // is refused on its authentication — the state gate has stepped aside.
    sleep_past_activation();
    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        assert_eq!(
            reported_state(&emulator, arn).await,
            "ACTIVE",
            "case {case}: a cluster whose SCRAM calls were refused must still settle ACTIVE"
        );

        let associated = associate(&emulator, arn, &secrets).await;
        if row.scram {
            assert_eq!(
                associated.status, 200,
                "case {case}: expected a settled SCRAM cluster to serve the association, got {} {}",
                associated.status, associated.body
            );
            let stored = listed(&list(&emulator, arn).await, case);
            assert_eq!(
                stored, secrets,
                "case {case}: expected the listing to report the association, got {stored:?}"
            );
        } else {
            assert_refused_with(&associated, case, SASL_SCRAM_REFUSAL);
            assert_refused_with(
                &disassociate(&emulator, arn, &secrets).await,
                case,
                SASL_SCRAM_REFUSAL,
            );
            let stored = listed(&list(&emulator, arn).await, case);
            assert!(
                stored.is_empty(),
                "case {case}: expected a refused association to store nothing, got {stored:?}"
            );
        }
    }
}

/// RULE: a state the cluster's brokers are serving in serves the secret set, so
/// an update in flight — which does not take the brokers away — leaves both the
/// association and the listing answering, and what is stored while the update is
/// in flight is still stored once it lands.
#[tokio::test]
async fn an_update_in_flight_still_serves_the_secret_set() {
    let emulator = Emulator::new();

    let rows = cluster_rows();
    let mut arns = Vec::new();
    for row in &rows {
        arns.push(emulator.create_ok(&row.request).await);
    }
    sleep_past_activation();

    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        let version = reported_version(&emulator, arn).await;
        let started = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/storage", pct(arn)),
                Some(json!({ "currentVersion": version, "volumeSizeGB": 400 })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case}: expected the update that puts the cluster into UPDATING to be accepted, \
             got {} {}",
            started.status, started.body
        );
        assert_eq!(
            reported_state(&emulator, arn).await,
            "UPDATING",
            "case {case}: an accepted update must leave the cluster UPDATING"
        );

        let secrets = vec![secret_arn(&format!("AmazonMSK_updating-{case}"))];
        let associated = associate(&emulator, arn, &secrets).await;
        if row.scram {
            assert_eq!(
                associated.status, 200,
                "case {case}: expected UPDATING to serve the association, got {} {}",
                associated.status, associated.body
            );
            let stored = listed(&list(&emulator, arn).await, case);
            assert_eq!(
                stored, secrets,
                "case {case}: expected UPDATING to serve the listing, got {stored:?}"
            );
        } else {
            // The state serves, so the authentication is what answers — which is
            // the whole point of the ordering: the sentence names the
            // configuration, not the state.
            assert_refused_with(&associated, case, SASL_SCRAM_REFUSAL);
            assert!(
                listed(&list(&emulator, arn).await, case).is_empty(),
                "case {case}: expected a cluster without SASL/SCRAM to hold no secrets"
            );
        }

        // The update lands because the clock came due, and the stored set is
        // exactly what was stored while it was in flight.
        sleep_past_update();
        assert_eq!(
            reported_state(&emulator, arn).await,
            "ACTIVE",
            "case {case}: the in-flight update must still land"
        );
        let after = listed(&list(&emulator, arn).await, case);
        let expected: Vec<String> = if row.scram { secrets } else { Vec::new() };
        assert_eq!(
            after, expected,
            "case {case}: expected the stored set to survive the update, got {after:?}"
        );
    }
}

/// RULE: the served set is the published one — the five `ClusterState` values the
/// refusal sentence names — and each refusal sentence is that list plus the
/// operation's own verb phrase, with nothing of a request in it.
///
/// The set's source is the sentence Amazon MSK answers a refused SCRAM-secret
/// call with; every entry must also be a value of the model's `ClusterState`
/// enum (`smithy_query(query="shape",shape="ClusterState")`, mirrored by
/// [`CLUSTER_STATE_VALUES`]), and the values it leaves out are exactly the states
/// in which a cluster has no secret set.
#[tokio::test]
async fn the_served_states_and_the_refusal_sentences_are_the_published_ones() {
    assert_eq!(
        CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM.as_slice(),
        ["ACTIVE", "UPDATING", "MAINTENANCE", "HEALING", "REBOOTING_BROKER"].as_slice(),
        "expected the five published states in the order the sentence lists them, got {:?}",
        CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM
    );

    for state in CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM {
        assert!(
            CLUSTER_STATE_VALUES.contains(&state),
            "case {state}: a served state must be a modelled ClusterState value, and \
             {CLUSTER_STATE_VALUES:?} does not hold it"
        );
    }

    // Every modelled state is either served or one of the three that cannot be:
    // the set and its complement together cover the enum, so a state added to the
    // model cannot be silently unaccounted for.
    for state in CLUSTER_STATE_VALUES {
        let served = CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM.contains(state);
        assert_eq!(
            served,
            !STATES_THAT_DO_NOT_SERVE.contains(state),
            "case {state}: expected served={} for this modelled state",
            !STATES_THAT_DO_NOT_SERVE.contains(state)
        );
    }

    for (case, action, expected) in [
        (
            "associate",
            SCRAM_SECRET_ACTION_ASSOCIATE,
            ASSOCIATE_STATE_REFUSAL,
        ),
        ("list", SCRAM_SECRET_ACTION_LIST, LIST_STATE_REFUSAL),
    ] {
        let message = invalid_cluster_state_for_scram_message(action);
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }

    assert_eq!(
        SASL_SCRAM_NOT_ENABLED_MESSAGE, SASL_SCRAM_REFUSAL,
        "expected the SASL/SCRAM sentence AWS answers, got {SASL_SCRAM_NOT_ENABLED_MESSAGE:?}"
    );
}
