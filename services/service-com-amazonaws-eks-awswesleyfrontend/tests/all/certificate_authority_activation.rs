// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ActivateCertificateAuthority`, driven over the wire
//! through the same `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: activating a certificate authority that the named cluster
//! holds, that is fully distributed and that is not signing, starts ONE
//! asynchronous update that promotes it to be the cluster's signer — the CA reports
//! `ACTIVATING` and the cluster `UPDATING` while that update runs, and when the
//! clock completes it the promoted CA is `IN_USE` with its activation recorded,
//! whichever CA was signing is retired to `NOT_USED` with a rollback window open on
//! it, and the cluster reports the new signer as its active certificate authority.
//! A call whose cluster or certificate authority is in any other state changes
//! nothing and is refused with the class that names what is wrong: a cluster or an
//! id the region does not hold is 404 `ResourceNotFoundException`; a cluster busy
//! with its own lifecycle or another update is 409 `ResourceInUseException`; a CA
//! that already signs for the cluster asks for the state the cluster is already in,
//! so it is 400 `InvalidRequestException`; and a CA that is not fully distributed is
//! the argument-level 400 `InvalidParameterException`.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over (whether an idempotency token is supplied and how two calls'
//! tokens relate, the cluster the CA hangs off, which of a cluster's CAs is
//! activated — including re-activating the one a previous activation retired, which
//! is the rollback the model documents — the region the request is signed for, the
//! cluster's status, and the CA's signing and distribution status). Every assertion
//! names its case and prints the actual value.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs` and the
//! certificate-authority helpers are `certificate_authority`'s: no suite grows a
//! second client. Two rows need a distribution status no producer call can leave a
//! CA in (`FAILED`, `DELETING`), so they write it into the same
//! `ClusterRecord::certificate_authorities` map `CreateCertificateAuthority` writes,
//! through the crate's public `state` module; the call under test is always a real
//! HTTP request.

use std::time::SystemTime;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, CA_DISTRIBUTION_DELAY, CA_ROLLBACK_WINDOW,
    CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, describe_update, epoch,
    is_uuid, same_instant, settle, str_member, REGION,
};
use crate::wire::{call, emulator, emulator_with_engine, encode_label, error_code, Answer};

/// `ActivateCertificateAuthority`:
/// `POST /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}/activate`.
pub(crate) async fn activate_authority(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    authority_id: &str,
    client_request_token: Option<&str>,
) -> Answer {
    let body = match client_request_token {
        Some(token) => serde_json::json!({ "clientRequestToken": token }),
        None => serde_json::json!({}),
    };
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/certificate-authorities/{}/activate",
            encode_label(cluster),
            encode_label(authority_id)
        ),
        &body.to_string(),
    )
    .await
}

/// The id of the certificate authority the cluster reports as its signer
/// (`Cluster$certificateAuthority.active`), which is how a caller learns which CA
/// signs without knowing any id in advance.
async fn signer_of(router: &MultiServiceRouter, region: &str, cluster: &str) -> serde_json::Value {
    describe_cluster(router, region, cluster).await.body["cluster"]["certificateAuthority"]
        ["active"]
        .clone()
}

/// A cluster with a successor certificate authority appended and fully
/// distributed — the precondition the model states for an activation ("must already
/// exist on the cluster and have a distributionStatus of COMPLETE") — returning
/// `(the signer's id, the successor's id)`.
pub(crate) async fn cluster_with_a_distributed_successor(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) -> (String, String) {
    let created = create_cluster(router, REGION, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateCluster, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let signer = signer_of(router, REGION, cluster).await;
    let signer_id = str_member(case, "cluster.certificateAuthority.active.id", &signer["id"]);

    let appended = append_authority(router, REGION, cluster, Some("tok-append")).await;
    assert_eq!(
        appended.status, 200,
        "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    let successor_id = str_member(
        case,
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );
    settle(CA_DISTRIBUTION_DELAY).await;
    (signer_id, successor_id)
}

// ---------------------------------------------------------------------------
// RULE: activating a distributed successor starts one asynchronous update that
// promotes it to the cluster's signer; while that update runs the CA is
// ACTIVATING and the cluster UPDATING, and when the clock completes it the
// promoted CA is IN_USE with its activation recorded, the outgoing CA is retired
// to NOT_USED with a rollback window, and the cluster reports the new signer.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activating_a_successor_promotes_it_and_retires_the_outgoing_authority() {
    let router = emulator();

    // Rows differ along whether an idempotency token is supplied and along the
    // cluster the certificate authorities hang off.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("no idempotency token", "act-plain", None),
        ("an idempotency token", "act-token", Some("tok-activate")),
        ("another cluster name shape", "Act9_mixed-Name", None),
    ];

    for (case, cluster, token) in &cases {
        let (signer_id, successor_id) =
            cluster_with_a_distributed_successor(&router, case, cluster).await;

        let activated = activate_authority(&router, REGION, cluster, &successor_id, *token).await;
        assert_eq!(
            activated.status, 200,
            "case {case:?}: expected 200 from ActivateCertificateAuthority, got {activated:?}"
        );

        // The answer is about the CA being activated, in the transient status the
        // promotion puts it in: the span is never zero, so this is observable.
        let summary = &activated.body["certificateAuthority"];
        assert_eq!(
            summary["id"],
            serde_json::json!(successor_id),
            "case {case:?}: the answer names the CA being activated, got {:?}",
            summary["id"]
        );
        assert_eq!(
            summary["signingStatus"],
            serde_json::json!("ACTIVATING"),
            "case {case:?}: a CA whose promotion is in flight is ACTIVATING, got {:?}",
            summary["signingStatus"]
        );
        assert_eq!(
            summary["distributionStatus"],
            serde_json::json!("COMPLETE"),
            "case {case:?}: activating does not disturb the trust distribution, got {:?}",
            summary["distributionStatus"]
        );

        // The asynchronous update the operation returns.
        let update = &activated.body["update"];
        let update_id = str_member(case, "update.id", &update["id"]);
        assert!(
            is_uuid(&update_id),
            "case {case:?}: expected a UUID-shaped update id, got {update_id:?}"
        );
        assert_eq!(
            update["type"],
            serde_json::json!("CertificateAuthorityUpdate"),
            "case {case:?}: expected the CertificateAuthorityUpdate type, got {:?}",
            update["type"]
        );
        assert_eq!(
            update["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: the promotion is in progress, got {:?}",
            update["status"]
        );
        assert_eq!(
            update["params"],
            serde_json::json!([
                { "type": "CertificateAuthorityId", "value": successor_id },
                { "type": "SigningStatus", "value": "IN_USE" },
            ]),
            "case {case:?}: the update names the CA it promotes and the signing \
             status it promotes it to, got {:?}",
            update["params"]
        );
        let update_created_at = epoch(case, "update.createdAt", &update["createdAt"]);

        // `DescribeUpdate` reads back the same object, so the update is a resource
        // of the cluster rather than a value in this one response.
        let read_update = describe_update(&router, REGION, cluster, &update_id).await;
        assert_eq!(
            read_update.body["update"], *update,
            "case {case:?}: DescribeUpdate must answer with the update the \
             activation returned, got {:?}",
            read_update.body["update"]
        );

        // Until the promotion is due the outgoing CA still signs: the cluster is
        // under an update and reports the old signer.
        let during = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            during.body["cluster"]["status"],
            serde_json::json!("UPDATING"),
            "case {case:?}: the cluster is UPDATING while the promotion runs, got {:?}",
            during.body["cluster"]["status"]
        );
        assert_eq!(
            during.body["cluster"]["certificateAuthority"]["active"]["id"],
            serde_json::json!(signer_id),
            "case {case:?}: the outgoing CA signs until the promotion is due, got {:?}",
            during.body["cluster"]["certificateAuthority"]["active"]["id"]
        );

        settle(UPDATE_COMPLETION_DELAY).await;

        // The promoted CA is the signer, and its activation is recorded at the
        // instant the promotion was DUE — not at the instant some read noticed it.
        let promoted = describe_authority(&router, REGION, cluster, &successor_id).await;
        let promoted = &promoted.body["certificateAuthority"];
        assert_eq!(
            promoted["signingStatus"],
            serde_json::json!("IN_USE"),
            "case {case:?}: the activated CA signs once the promotion is due, got {:?}",
            promoted["signingStatus"]
        );
        assert_eq!(
            promoted["activatedBy"],
            serde_json::json!("CUSTOMER"),
            "case {case:?}: the caller asked for this activation, got {:?}",
            promoted["activatedBy"]
        );
        let activated_at = epoch(case, "activatedAt", &promoted["activatedAt"]);
        assert!(
            same_instant(
                activated_at,
                update_created_at + UPDATE_COMPLETION_DELAY.as_secs_f64()
            ),
            "case {case:?}: the activation is recorded at the instant it was owed \
             ({}), got {activated_at}",
            update_created_at + UPDATE_COMPLETION_DELAY.as_secs_f64()
        );
        assert_eq!(
            promoted["rollbackAvailable"],
            serde_json::json!(false),
            "case {case:?}: the CA that signs now is not one to revert to, got {:?}",
            promoted["rollbackAvailable"]
        );

        // The outgoing CA is retired but still on the cluster — "the outgoing CA
        // remains in the cluster's trust bundle but no longer signs certificates" —
        // and reverting to it is available for a limited period.
        let retired = describe_authority(&router, REGION, cluster, &signer_id).await;
        assert_eq!(
            retired.status, 200,
            "case {case:?}: the retired CA remains in the trust bundle, got {retired:?}"
        );
        let retired = &retired.body["certificateAuthority"];
        assert_eq!(
            retired["signingStatus"],
            serde_json::json!("NOT_USED"),
            "case {case:?}: the outgoing CA no longer signs, got {:?}",
            retired["signingStatus"]
        );
        assert_eq!(
            retired["rollbackAvailable"],
            serde_json::json!(true),
            "case {case:?}: reverting to the outgoing CA is available right after \
             the activation, got {:?}",
            retired["rollbackAvailable"]
        );
        assert_eq!(
            retired["activatedBy"],
            serde_json::json!("EKS"),
            "case {case:?}: retiring a CA does not rewrite who activated it, got {:?}",
            retired["activatedBy"]
        );

        // The update completed and handed the cluster back, which now reports the
        // promoted CA as the one signing for it.
        let read_update = describe_update(&router, REGION, cluster, &update_id).await;
        assert_eq!(
            read_update.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {case:?}: the promotion's update is successful once it is due, got {:?}",
            read_update.body["update"]["status"]
        );
        let after = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            after.body["cluster"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: the cluster is ACTIVE again once the promotion completed, got {:?}",
            after.body["cluster"]["status"]
        );
        assert_eq!(
            after.body["cluster"]["certificateAuthority"]["active"],
            serde_json::json!({ "id": successor_id, "activatedBy": "CUSTOMER" }),
            "case {case:?}: the cluster reports the CA that signs for it, got {:?}",
            after.body["cluster"]["certificateAuthority"]["active"]
        );

        // Reading again does not move anything on.
        let twice = describe_authority(&router, REGION, cluster, &successor_id).await;
        assert_eq!(
            twice.body["certificateAuthority"], *promoted,
            "case {case:?}: a second read must answer identically"
        );

        // The rollback window closes on its own: it was owed at the stored instant
        // whether or not anybody called.
        settle(CA_ROLLBACK_WINDOW).await;
        let lapsed = describe_authority(&router, REGION, cluster, &signer_id).await;
        assert_eq!(
            lapsed.body["certificateAuthority"]["rollbackAvailable"],
            serde_json::json!(false),
            "case {case:?}: once the rollback window has passed no rollback is \
             available, got {:?}",
            lapsed.body["certificateAuthority"]["rollbackAvailable"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the same promotion applies to ANY of a cluster's certificate authorities
// that is distributed and not signing — including the one a previous activation
// retired, which is how a rotation is reverted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_retired_authority_can_be_activated_again() {
    let router = emulator();

    // Rows differ along the cluster and along whether the reverting call carries an
    // idempotency token.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("revert with a token", "revert-token", Some("tok-revert")),
        ("revert without one", "revert-plain", None),
    ];

    for (case, cluster, token) in &cases {
        let (signer_id, successor_id) =
            cluster_with_a_distributed_successor(&router, case, cluster).await;

        let forward = activate_authority(&router, REGION, cluster, &successor_id, None).await;
        assert_eq!(
            forward.status, 200,
            "case {case:?}: expected 200 from the first activation, got {forward:?}"
        );
        settle(UPDATE_COMPLETION_DELAY).await;

        // Now the other way round: the CA the first activation retired becomes the
        // signer again, and the one it promoted is retired in its turn.
        let back = activate_authority(&router, REGION, cluster, &signer_id, *token).await;
        assert_eq!(
            back.status, 200,
            "case {case:?}: expected 200 reverting to the outgoing CA, got {back:?}"
        );
        assert_eq!(
            back.body["certificateAuthority"]["signingStatus"],
            serde_json::json!("ACTIVATING"),
            "case {case:?}: the reverted-to CA is ACTIVATING, got {:?}",
            back.body["certificateAuthority"]["signingStatus"]
        );
        settle(UPDATE_COMPLETION_DELAY).await;

        let signer = signer_of(&router, REGION, cluster).await;
        assert_eq!(
            signer["id"],
            serde_json::json!(signer_id),
            "case {case:?}: the cluster signs with the CA it reverted to, got {signer:?}"
        );
        assert_eq!(
            signer["activatedBy"],
            serde_json::json!("CUSTOMER"),
            "case {case:?}: the caller asked for the revert, got {signer:?}"
        );
        let retired = describe_authority(&router, REGION, cluster, &successor_id).await;
        assert_eq!(
            retired.body["certificateAuthority"]["signingStatus"],
            serde_json::json!("NOT_USED"),
            "case {case:?}: the CA the revert replaced no longer signs, got {:?}",
            retired.body["certificateAuthority"]["signingStatus"]
        );
        assert_eq!(
            retired.body["certificateAuthority"]["rollbackAvailable"],
            serde_json::json!(true),
            "case {case:?}: the revert opens a rollback window on what it replaced, got {:?}",
            retired.body["certificateAuthority"]["rollbackAvailable"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `clientRequestToken` is an `@idempotencyToken` — a second call carrying
// the same non-blank token is the SAME request and is answered with the update
// the first one started; any other pair of calls is two requests, and the second
// is judged on the state the first one left.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_with_the_same_idempotency_token_is_absorbed() {
    let router = emulator();

    /// One row: the case name, its cluster, the two calls' tokens, and whether the
    /// second call is a retry of the first.
    struct Row {
        case: &'static str,
        cluster: &'static str,
        first_token: Option<&'static str>,
        second_token: Option<&'static str>,
        absorbed: bool,
    }

    // Rows differ along the relationship between the two calls' tokens: equal,
    // different, both absent, and both blank (a token that identifies nothing).
    let cases = vec![
        Row {
            case: "the same token twice",
            cluster: "act-idem-same",
            first_token: Some("tok"),
            second_token: Some("tok"),
            absorbed: true,
        },
        Row {
            case: "two different tokens",
            cluster: "act-idem-different",
            first_token: Some("tok-a"),
            second_token: Some("tok-b"),
            absorbed: false,
        },
        Row {
            case: "no token on either call",
            cluster: "act-idem-absent",
            first_token: None,
            second_token: None,
            absorbed: false,
        },
        Row {
            case: "a blank token on both calls",
            cluster: "act-idem-blank",
            first_token: Some("   "),
            second_token: Some("   "),
            absorbed: false,
        },
    ];

    for row in &cases {
        let (case, cluster) = (row.case, row.cluster);
        let (_, successor_id) = cluster_with_a_distributed_successor(&router, case, cluster).await;

        let first =
            activate_authority(&router, REGION, cluster, &successor_id, row.first_token).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first activation succeeds, got {first:?}"
        );
        let first_update = str_member(case, "update.id", &first.body["update"]["id"]);

        // A retry that arrives while the promotion is still running is absorbed:
        // idempotency does not wait for the work to finish.
        let during =
            activate_authority(&router, REGION, cluster, &successor_id, row.second_token).await;
        if row.absorbed {
            assert_eq!(
                during.body["update"]["id"],
                serde_json::json!(first_update),
                "case {case:?}: an in-flight retry reports the original update, got {during:?}"
            );
            assert_eq!(
                during.body["certificateAuthority"]["id"],
                serde_json::json!(successor_id),
                "case {case:?}: an in-flight retry names the same CA, got {during:?}"
            );
        } else {
            // Not a retry, so it is a second request — and the cluster is still
            // running the first one's update.
            assert_eq!(
                (during.status, error_code(&during).as_str()),
                (409, "ResourceInUseException"),
                "case {case:?}: a call that is not a retry is refused while the \
                 promotion runs, got {during:?}"
            );
        }

        settle(UPDATE_COMPLETION_DELAY).await;

        let after =
            activate_authority(&router, REGION, cluster, &successor_id, row.second_token).await;
        if row.absorbed {
            assert_eq!(
                after.body["update"]["id"],
                serde_json::json!(first_update),
                "case {case:?}: an absorbed retry still reports the original update, got {after:?}"
            );
            assert_eq!(
                after.body["update"]["status"],
                serde_json::json!("Successful"),
                "case {case:?}: the absorbed retry reports the promotion it finished, got {:?}",
                after.body["update"]["status"]
            );
        } else {
            // The CA the first call promoted now signs, so activating it again asks
            // for the state the cluster is already in.
            assert_eq!(
                (after.status, error_code(&after).as_str()),
                (400, "InvalidRequestException"),
                "case {case:?}: the promoted CA already signs, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a call whose cluster or certificate authority is not in a state to take
// an activation writes nothing and is refused with the class that names what is
// wrong.
// ---------------------------------------------------------------------------

/// What a refusal row expects: the HTTP status and the error code a client
/// branches on.
struct Refusal {
    case: &'static str,
    region: &'static str,
    cluster: String,
    authority_id: String,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn activation_is_refused_unless_the_cluster_and_the_authority_are_ready() {
    let (router, engine) = emulator_with_engine();
    const OTHER_REGION: &str = "eu-central-1";

    // A ready cluster with a distributed successor, which the rows that are about
    // something OTHER than the CA's own state point at.
    let (ready_signer, ready_successor) =
        cluster_with_a_distributed_successor(&router, "ready", "act-ready").await;

    // The same name in two regions is two independent clusters, each with its own
    // certificate authorities.
    let created = create_cluster(&router, OTHER_REGION, "act-ready").await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 creating the other region's cluster, got {created:?}"
    );

    // Two certificate authorities parked in a distribution status no producer call
    // leaves one in: the model's `FAILED` and `DELETING`.
    let mut parked = Vec::new();
    for (case, cluster, status) in [
        ("a CA whose distribution failed", "act-failed", "FAILED"),
        ("a CA being deleted", "act-deleting", "DELETING"),
    ] {
        let (_, successor) = cluster_with_a_distributed_successor(&router, case, cluster).await;
        set_distribution_status(&engine, cluster, &successor, status);
        parked.push((case, cluster, successor));
    }

    // A cluster with a ready successor that is NOT `ACTIVE`: a cluster being
    // created takes no append, so the status is parked on the record the appends
    // wrote, which is the one state the activation gate reads.
    let (_, parked_cluster_successor) =
        cluster_with_a_distributed_successor(&router, "not active", "act-not-active").await;
    park_cluster_status(&engine, "act-not-active", "CREATING");

    // The two rows whose precondition is TRANSIENT are set up last, so nothing
    // sleeps between standing them up and the calls that read them: a cluster whose
    // successor's trust distribution is still running (the append's own update still
    // in flight), and a cluster running an unrelated update with a ready successor.
    let (_, busy_successor) =
        cluster_with_a_distributed_successor(&router, "busy", "act-busy").await;
    let (_, distributing) =
        append_and_leave_in_flight(&router, "distributing", "act-distributing").await;
    let busy_update = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label("act-busy")),
        &serde_json::json!({ "deletionProtection": true }).to_string(),
    )
    .await;
    assert_eq!(
        busy_update.status, 200,
        "setup: expected 200 from UpdateClusterConfig, got {busy_update:?}"
    );

    let mut cases = vec![
        Refusal {
            case: "a cluster already running another update",
            region: REGION,
            cluster: "act-busy".to_string(),
            authority_id: busy_successor,
            status: 409,
            code: "ResourceInUseException",
        },
        Refusal {
            case: "a successor whose trust distribution is still running",
            region: REGION,
            cluster: "act-distributing".to_string(),
            authority_id: distributing,
            status: 409,
            code: "ResourceInUseException",
        },
        Refusal {
            case: "a cluster that is not ACTIVE",
            region: REGION,
            cluster: "act-not-active".to_string(),
            authority_id: parked_cluster_successor,
            status: 409,
            code: "ResourceInUseException",
        },
        Refusal {
            case: "an unknown cluster",
            region: REGION,
            cluster: "no-such-cluster".to_string(),
            authority_id: ready_successor.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a well-formed but unused CA id",
            region: REGION,
            cluster: "act-ready".to_string(),
            authority_id: "00000000-0000-4000-8000-000000000000".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "an id that is not a UUID at all",
            region: REGION,
            cluster: "act-ready".to_string(),
            authority_id: "not-an-id".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a CA of the same-named cluster in another region",
            region: OTHER_REGION,
            cluster: "act-ready".to_string(),
            authority_id: ready_successor.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "the CA that already signs for the cluster",
            region: REGION,
            cluster: "act-ready".to_string(),
            authority_id: ready_signer.clone(),
            status: 400,
            code: "InvalidRequestException",
        },
    ];
    for (case, cluster, authority_id) in parked {
        cases.push(Refusal {
            case,
            region: REGION,
            cluster: cluster.to_string(),
            authority_id,
            status: 400,
            code: "InvalidParameterException",
        });
    }

    for row in &cases {
        let case = row.case;
        let refused = activate_authority(
            &router,
            row.region,
            &row.cluster,
            &row.authority_id,
            Some("tok-refused"),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (row.status, row.code),
            "case {case:?}: expected {} {}, got {refused:?}",
            row.status,
            row.code
        );
    }

    // The refusals wrote nothing: the ready cluster still signs with the CA it
    // signed with, its successor is still a successor, and no update was started.
    let signer = signer_of(&router, REGION, "act-ready").await;
    assert_eq!(
        signer["id"],
        serde_json::json!(ready_signer),
        "a refused activation must not change the signer, got {signer:?}"
    );
    let successor = describe_authority(&router, REGION, "act-ready", &ready_successor).await;
    assert_eq!(
        successor.body["certificateAuthority"]["signingStatus"],
        serde_json::json!("NOT_USED"),
        "a refused activation must not move the CA it named, got {:?}",
        successor.body["certificateAuthority"]["signingStatus"]
    );
    let cluster = describe_cluster(&router, REGION, "act-ready").await;
    assert_eq!(
        cluster.body["cluster"]["status"],
        serde_json::json!("ACTIVE"),
        "a refused activation must not take the cluster into an update, got {:?}",
        cluster.body["cluster"]["status"]
    );

    // And the still-ready cluster takes the activation once it is asked for
    // properly, which is what makes the refusals above about state and not about
    // the call itself.
    let activated =
        activate_authority(&router, REGION, "act-ready", &ready_successor, None).await;
    assert_eq!(
        activated.status, 200,
        "the ready cluster takes the activation, got {activated:?}"
    );
}

/// A cluster with a successor appended and its trust distribution left RUNNING,
/// returning `(the signer's id, the successor's id)`.
async fn append_and_leave_in_flight(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) -> (String, String) {
    let created = create_cluster(router, REGION, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateCluster, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;
    let signer = signer_of(router, REGION, cluster).await;
    let signer_id = str_member(case, "cluster.certificateAuthority.active.id", &signer["id"]);
    let appended = append_authority(router, REGION, cluster, Some("tok-in-flight")).await;
    assert_eq!(
        appended.status, 200,
        "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    (
        signer_id,
        str_member(
            case,
            "certificateAuthority.id",
            &appended.body["certificateAuthority"]["id"],
        ),
    )
}

/// Park `authority` of `cluster` in `status`, a `CertificateAuthorityDistribution
/// Status` member no producer call in this emulator leaves a CA in.
///
/// It is written into the same `ClusterRecord::certificate_authorities` map
/// `CreateCertificateAuthority` writes and `DescribeCertificateAuthority` reads,
/// and the instant the clock owes the CA its `COMPLETE` distribution is cleared so
/// the status it is parked in is the one the call under test sees.
fn set_distribution_status(
    engine: &engine::InMemoryEngine,
    cluster: &str,
    authority: &str,
    status: &str,
) {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .expect("setup: the cluster was created above")
        .certificate_authority_mut(authority)
        .expect("setup: the certificate authority was appended above");
    record.distribution_status = status.to_string();
    record.distribution_complete_at = None;
}

/// Park `cluster` in `status`, a cluster status no call can leave a cluster that
/// already holds a distributed successor in.
///
/// It is written into the same `ClusterRecord` every operation reads, and the
/// instant the clock owes the cluster its `ACTIVE` status is cleared so the status
/// it is parked in is the one the call under test sees.
fn park_cluster_status(engine: &engine::InMemoryEngine, cluster: &str, status: &str) {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    // Materialise everything the clock already owes the store first: an append's
    // update completing is what hands its cluster back to `ACTIVE`, and that is owed
    // whether or not anybody has called yet — so it would otherwise be applied ON TOP
    // of the parked status by the very call under test.
    guard.materialize_due_transitions(SystemTime::now());
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .expect("setup: the cluster was created above");
    record.status = status.to_string();
    record.active_at = None;
}
