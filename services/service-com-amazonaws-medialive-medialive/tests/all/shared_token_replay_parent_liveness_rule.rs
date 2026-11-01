// SPDX-License-Identifier: CC0-1.0

//! What an idempotency `RequestId` does NOT do, end to end over the restJson1
//! wire: it never turns a create this service refuses into one it serves.
//!
//! RULE: a create addressed to another resource's collection endpoint is carried
//! out FOR the resource its URI names, so that resource must be one the account
//! still holds and may still change; the replay a `RequestId` already spent
//! there would otherwise answer with resolves that parent through the same
//! liveness the write itself resolves it by, so once the parent's delete has been
//! accepted — inside the transient window as in the terminal state the record
//! keeps — a retry is refused exactly as a first attempt is, with the same
//! status, the same modelled error and the same message, and stores nothing.
//!
//! The suites below are the two shapes that rule takes in this service:
//!
//! * `CreateMultiplexProgram`, whose parent is the multiplex the program is
//!   created in (`POST /prod/multiplexes/{MultiplexId}/programs`) and whose
//!   refusal is the modelled 409 `ConflictException` naming the multiplex's
//!   state;
//! * `CreateNodeRegistrationScript`, whose parent is the cluster the script
//!   registers nodes in (`POST
//!   /prod/clusters/{ClusterId}/nodeRegistrationScript`) and which also names the
//!   node a re-registration script is issued for, both refused with the modelled
//!   400 `BadRequestException`.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the reported create-delete-replay sequence is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_deletion::delete_multiplex;
use crate::multiplex_lifecycle::{create_body, created as created_multiplex, describe_multiplex, settings};
use crate::multiplex_programs::{create_program, describe_program, program_settings};
use crate::node_lifecycle::{cluster, created as created_node, delete_node, node_id};
use crate::wire::{call, error_code, error_message, medialive};

/// The token every row spends and then retries with.
pub(crate) const TOKEN: &str = "token-a";

/// A token this account never spends on anything, which is what each row's
/// refusal is compared against: the FIRST attempt the rule says a retry is
/// answered exactly like.
pub(crate) const NEVER_SPENT: &str = "token-never-spent";

/// The program every doomed multiplex already holds, and the `ProgramNumber` it
/// was created with.
const STORED_PROGRAM: (&str, i64) = ("p1", 9);

/// Where the token the refused call carries had already been spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spent {
    /// On a program of the very multiplex the refused call names — the replay
    /// branch this rule is about.
    OnThisMultiplex,
    /// On a program of ANOTHER multiplex of the account, which the delete below
    /// does not touch.
    OnAnotherMultiplex,
    /// Nowhere at all.
    Never,
}

/// One refusal, as a client reads it: the status, the error code and the
/// message, so two answers can be compared as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) status: u16,
    pub(crate) code: String,
    pub(crate) message: String,
}

pub(crate) fn refusal_of(status: u16, body: &Value) -> Refusal {
    Refusal {
        status,
        code: error_code(status, body),
        message: error_message(body),
    }
}

/// The multiplex `id`, read back through `DescribeMultiplex` with the read
/// asserted to have succeeded and to report `state`.
async fn multiplex_in_state(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    state: &str,
) -> Value {
    let (status, described) = describe_multiplex(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described["state"],
        json!(state),
        "case {case:?}: the row did not reach {state}, it reached {:?}",
        described["state"]
    );
    described
}

/// Rule: a program is created IN the multiplex the request names, so that
/// multiplex must be one the account may still change — a `RequestId` cannot
/// turn a request this operation refuses into one it serves, so a create aimed
/// at a multiplex whose delete has been accepted is refused with the modelled
/// 409 `ConflictException` naming the multiplex and the state it is in, exactly
/// as a first attempt carrying a token nothing ever spent is, and stores
/// nothing.
///
/// The rows differ along what the rule generalises over: where the token the
/// call carries was spent before (on a program of this multiplex, on another
/// multiplex's program, nowhere), which half of the delete lifecycle the call
/// lands in, and whether the call asks for the stored program's own name and
/// number or for ones no program of the multiplex has.
#[tokio::test]
async fn a_program_create_in_a_multiplex_whose_delete_was_accepted_is_refused_whatever_token_it_carries(
) {
    // (case, where the token was spent, whether the clock settles the delete,
    //  the program name and number the refused call asks for)
    let rows: [(&str, Spent, bool, (&str, i64)); 6] = [
        (
            "a token already spent on a program of this multiplex, inside the DELETING window",
            Spent::OnThisMultiplex,
            false,
            STORED_PROGRAM,
        ),
        (
            "a token already spent on a program of this multiplex, once the delete settled",
            Spent::OnThisMultiplex,
            true,
            STORED_PROGRAM,
        ),
        (
            "the spent token asking for another program name and number",
            Spent::OnThisMultiplex,
            true,
            ("second", 12),
        ),
        (
            "a token spent on another multiplex's program, inside the DELETING window",
            Spent::OnAnotherMultiplex,
            false,
            STORED_PROGRAM,
        ),
        (
            "a token this account has never spent, inside the DELETING window",
            Spent::Never,
            false,
            ("after-the-delete", 3),
        ),
        (
            "a token this account has never spent, once the delete settled",
            Spent::Never,
            true,
            ("after-the-delete", 3),
        ),
    ];

    for (case, spent, wait, (asked_name, asked_number)) in rows {
        let router = medialive();
        let doomed = created_multiplex(
            &router,
            case,
            create_body("doomed", "token-doomed", settings(10_000_000, 1)),
        )
        .await;
        let doomed = doomed["id"].as_str().unwrap_or_default().to_string();

        // The multiplex already holds a program, created either with the token
        // the refused call carries or with one of its own.
        let (stored_name, stored_number) = STORED_PROGRAM;
        let stored_token = if spent == Spent::OnThisMultiplex {
            TOKEN
        } else {
            "token-p1"
        };
        let (status, stored) = create_program(
            &router,
            &doomed,
            stored_name,
            stored_token,
            program_settings(stored_number),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram: expected 201, got {status}: {stored}"
        );

        // A second multiplex, whose own program the same token minted, so a row
        // that spends it elsewhere shows the delete below ends ONE multiplex's
        // window rather than the map's.
        if spent == Spent::OnAnotherMultiplex {
            let other = created_multiplex(
                &router,
                case,
                create_body("elsewhere", "token-elsewhere", settings(3_000_000, 2)),
            )
            .await;
            let other = other["id"].as_str().unwrap_or_default().to_string();
            let (status, elsewhere) = create_program(
                &router,
                &other,
                stored_name,
                TOKEN,
                program_settings(stored_number),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateMultiplexProgram on the other multiplex: expected 201, got \
                 {status}: {elsewhere}"
            );
        }

        // While the multiplex is LIVE the spent token IS a replay: it answers
        // with the program it minted whatever the body asks for, and stores
        // nothing. That is the answer the delete below takes away.
        if spent == Spent::OnThisMultiplex {
            let (status, replayed) = create_program(
                &router,
                &doomed,
                asked_name,
                TOKEN,
                program_settings(asked_number),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the live replay to answer 201, got {status}: {replayed}"
            );
            assert_eq!(
                replayed, stored,
                "case {case:?}: expected the live token to replay its own program, got {replayed}"
            );
        }

        // A multiplex is deleted from the stopped state, so the create's own
        // transition is let come due first.
        wait_for_the_clock();
        let (status, deleting) = delete_multiplex(&router, &doomed).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
        );
        if wait {
            wait_for_the_clock();
        }
        let phase = if wait { "DELETED" } else { "DELETING" };
        let before = multiplex_in_state(&router, case, &doomed, phase).await;
        let (status, stored_before) = describe_program(&router, &doomed, stored_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {stored_before}"
        );

        // The first attempt: the same request, carrying a token nothing ever
        // spent. This is what the rule says the retry is answered exactly like.
        let (status, control) = create_program(
            &router,
            &doomed,
            asked_name,
            NEVER_SPENT,
            program_settings(asked_number),
        )
        .await;
        let control = refusal_of(status, &control);
        assert_eq!(
            control,
            Refusal {
                status: 409,
                code: "ConflictException".to_string(),
                message: format!("Multiplex {doomed} is {phase}; no program can be created in it"),
            },
            "case {case:?}: expected the modelled 409 naming the multiplex and its state, got \
             {control:?}"
        );

        // The retry, carrying the row's own token.
        let (status, refused) = create_program(
            &router,
            &doomed,
            asked_name,
            TOKEN,
            program_settings(asked_number),
        )
        .await;
        let refused = refusal_of(status, &refused);
        assert_eq!(
            refused, control,
            "case {case:?}: expected the retry to be refused exactly as a first attempt is, got \
             {refused:?}"
        );

        // Neither call stored anything: the multiplex reads back as it did, its
        // program with it, and the name the refused calls asked for names no
        // program of the multiplex.
        assert_eq!(
            multiplex_in_state(&router, case, &doomed, phase).await,
            before,
            "case {case:?}: expected the refusals to leave the {phase} multiplex alone"
        );
        let (status, stored_after) = describe_program(&router, &doomed, stored_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram after the refusals: expected 200, got \
             {status}: {stored_after}"
        );
        assert_eq!(
            stored_after, stored_before,
            "case {case:?}: expected the stored program to be left alone, got {stored_after}"
        );
        if asked_name != stored_name {
            let (status, missing) = describe_program(&router, &doomed, asked_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused name to name no program, got {status}: \
                 {missing}"
            );
        }
    }
}

/// The registration-script sub-resource of one cluster, which
/// `CreateNodeRegistrationScript` is bound to (`smithy_query(query="operation",
/// operation="CreateNodeRegistrationScript")`: `POST
/// /prod/clusters/{ClusterId}/nodeRegistrationScript`).
fn script_uri(cluster_id: &str) -> String {
    format!("/prod/clusters/{cluster_id}/nodeRegistrationScript")
}

/// `CreateNodeRegistrationScript` on one cluster: its status and body.
pub(crate) async fn create_script(
    router: &MultiServiceRouter,
    cluster_id: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &script_uri(cluster_id), Some(body)).await
}

/// `DeleteCluster` (`DELETE /prod/clusters/{ClusterId}`), asserted to have been
/// accepted with the modelled 202.
pub(crate) async fn delete_cluster(router: &MultiServiceRouter, case: &str, cluster_id: &str) {
    let (status, deleting) = call(
        router,
        "DELETE",
        &format!("/prod/clusters/{cluster_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteCluster: expected 202, got {status}: {deleting}"
    );
}

/// Which resource the script was issued for has its delete accepted in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deleted {
    /// The cluster the script registers nodes in.
    Cluster,
    /// The node a re-registration script was issued for.
    Node,
}

/// Rule: a registration script is issued FOR the cluster the request names and,
/// when it asks for a re-registration, for a node that cluster still holds, so
/// both must be ones the account may still change — a `RequestId` cannot turn a
/// request this operation refuses into one it serves, so a retry sent once the
/// delete of either has been accepted is refused with the modelled 400
/// `BadRequestException` naming that resource, exactly as a first attempt
/// carrying a token nothing ever spent is.
///
/// The rows differ along what the rule generalises over: WHICH of the two
/// resources the script was issued for is taken away, and which half of that
/// resource's delete lifecycle the retry lands in.
#[tokio::test]
async fn a_registration_script_retry_for_a_resource_whose_delete_was_accepted_is_refused() {
    // (case, which resource is deleted, whether the clock settles that delete)
    let rows: [(&str, Deleted, bool); 4] = [
        (
            "the cluster the script registers nodes in, inside its DELETING window",
            Deleted::Cluster,
            false,
        ),
        (
            "the cluster the script registers nodes in, once its delete settled",
            Deleted::Cluster,
            true,
        ),
        (
            "the node the script re-registers, inside its DEREGISTERING window",
            Deleted::Node,
            false,
        ),
        (
            "the node the script re-registers, once its deregistration settled",
            Deleted::Node,
            true,
        ),
    ];

    for (case, deleted, wait) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "anywhere").await;

        // A cluster is deleted only once it holds no node, so only the rows
        // about a NODE create one — and those are the re-registration rows.
        let node = if deleted == Deleted::Node {
            let node = created_node(&router, case, &cluster_id, json!({ "name": "unit-1" })).await;
            Some(node_id(case, &node))
        } else {
            None
        };
        let body = |token: &str| match node.as_deref() {
            Some(node) => json!({ "requestId": token, "id": node }),
            None => json!({ "requestId": token, "name": "anywhere-$hn" }),
        };

        let (status, issued) = create_script(&router, &cluster_id, body(TOKEN)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateNodeRegistrationScript: expected 200, got {status}: {issued}"
        );

        // While both resources are live the token IS a replay: it answers with
        // the script it issued. That is the answer the delete below takes away.
        let (status, replayed) = create_script(&router, &cluster_id, body(TOKEN)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the live replay to answer 200, got {status}: {replayed}"
        );
        assert_eq!(
            replayed, issued,
            "case {case:?}: expected the live token to replay its own script, got {replayed}"
        );

        let expected_message = match (deleted, node.as_deref()) {
            (Deleted::Cluster, _) => {
                delete_cluster(&router, case, &cluster_id).await;
                format!("Cluster {cluster_id} not found")
            }
            (Deleted::Node, Some(node)) => {
                let (status, deleting) = delete_node(&router, &cluster_id, node).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: DeleteNode: expected 202, got {status}: {deleting}"
                );
                format!("Node {node} already deleted")
            }
            (Deleted::Node, None) => unreachable!("a node row creates its node"),
        };
        if wait {
            wait_for_the_clock();
        }

        // The first attempt: the same request, carrying a token nothing ever
        // spent.
        let (status, control) = create_script(&router, &cluster_id, body(NEVER_SPENT)).await;
        let control = refusal_of(status, &control);
        assert_eq!(
            control,
            Refusal {
                status: 400,
                code: "BadRequestException".to_string(),
                message: expected_message,
            },
            "case {case:?}: expected the modelled 400 naming the resource that is gone, got \
             {control:?}"
        );

        // The retry, carrying the token the script was issued under.
        let (status, refused) = create_script(&router, &cluster_id, body(TOKEN)).await;
        let refused = refusal_of(status, &refused);
        assert_eq!(
            refused, control,
            "case {case:?}: expected the retry to be refused exactly as a first attempt is, got \
             {refused:?}"
        );
        assert!(
            !refused.message.is_empty(),
            "case {case:?}: expected the refusal to carry a message, got {refused:?}"
        );
    }
}
