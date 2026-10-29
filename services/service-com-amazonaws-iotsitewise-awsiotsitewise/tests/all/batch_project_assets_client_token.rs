// SPDX-License-Identifier: CC0-1.0

//! The dimensions of the spent-`clientToken` rule for the two batch project-asset
//! operations — `BatchAssociateProjectAssets` and `BatchDisassociateProjectAssets`
//! — that the sibling token suites do not reach, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule all three suites pin: a `clientToken` suppresses re-execution only for
//! a REPEAT of the request it was spent on. Both of these operations answer with a
//! per-asset error list that reports nothing of the call that spent the token, and
//! neither declares an error that states a token conflict (smithy_query
//! query="operation" operation="BatchAssociateProjectAssets,BatchDisassociateProjectAssets":
//! `InternalFailureException`, `InvalidRequestException`, `LimitExceededException`
//! on the association side, `ResourceNotFoundException`, `ThrottlingException`), so
//! a request differing in any member it is recognised by is a different call: it is
//! carried out, and the membership a later `ListProjectAssets` reports is the one
//! that request asked for. A success of either operation therefore never asserts a
//! membership change the store did not record.
//!
//! What the siblings already cover, and is NOT repeated here: `client_token_replay`
//! pins that a verbatim retry of either operation replays its answer and writes
//! nothing, and `client_token_spent_on_another_request` pins the base case of a
//! second request naming another asset. What these rows add: the LENGTH of the id
//! list, an id repeated inside it, the projectId in the PATH (so the project the
//! first call wrote is shown untouched), a membership the row starts from, the
//! ORDER of the ids, a refused request under a spent token leaving that token
//! spendable, and — on every assertion — the listing and the project's own stored
//! record shown to agree.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`), the request
//! builders of both operations, the project and asset creates, the listing walk and
//! the direct store read are the ones the sibling suites already define, and the
//! token seed comes from the replay suite; all are reused rather than copied.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, router_with_engine};
use super::client_token_replay::token_for;
use super::hierarchies_etags_and_refusals::error_message;
use super::project_asset_disassociation::{disassociate_body, disassociate_uri};
use super::project_assets::{
    associate_body, associate_uri, create_assets, create_project, list_project_assets,
    stored_membership,
};

/// A UUID of the modelled `ID` form (smithy_query query="operation"
/// operation="BatchAssociateProjectAssets": `assetIds` targets `IDs`) that no
/// create in this suite ever mints, for the rows that name an asset this service
/// does not hold.
const ABSENT_ID: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";

/// Which of the two batch project-asset operations a row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Batch {
    Associate,
    Disassociate,
}

impl Batch {
    /// Where a request of this operation goes, and the body it carries: both come
    /// from the suite that owns the operation, so this one grows no second copy of
    /// either wire shape.
    fn uri(self, project_id: &str) -> String {
        match self {
            Batch::Associate => associate_uri(project_id),
            Batch::Disassociate => disassociate_uri(project_id),
        }
    }

    fn body(self, asset_ids: &[String], client_token: Option<&str>) -> Value {
        match self {
            Batch::Associate => associate_body(asset_ids, client_token),
            Batch::Disassociate => disassociate_body(asset_ids, client_token),
        }
    }

    /// The operation's name, for an assertion message.
    fn name(self) -> &'static str {
        match self {
            Batch::Associate => "BatchAssociateProjectAssets",
            Batch::Disassociate => "BatchDisassociateProjectAssets",
        }
    }

    /// The operation that undoes this one on a project's membership.
    fn inverse(self) -> Self {
        match self {
            Batch::Associate => Batch::Disassociate,
            Batch::Disassociate => Batch::Associate,
        }
    }
}

/// Send one request of `batch` and answer `(status, body)`.
async fn send(
    router: &MultiServiceRouter,
    batch: Batch,
    project_id: &str,
    asset_ids: &[String],
    client_token: Option<&str>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &batch.uri(project_id),
        Some(batch.body(asset_ids, client_token)),
    )
    .await
}

/// Send one request of `batch` and assert it was SERVED: HTTP 200 with an empty
/// per-asset error list, which is what "a list of associated error information, if
/// any" states when the whole batch was applied.
async fn send_served(
    router: &MultiServiceRouter,
    case: &str,
    batch: Batch,
    project_id: &str,
    asset_ids: &[String],
    client_token: Option<&str>,
) {
    let (status, answered) = send(router, batch, project_id, asset_ids, client_token).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: {} expected 200, got {status} {answered}",
        batch.name()
    );
    let errors = answered["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an errors list, got {answered}"));
    assert!(
        errors.is_empty(),
        "case {case:?}: expected no per-asset errors, got {answered}"
    );
}

/// The membership of `project_id` as the API reports it AND as the project's own
/// record holds it, asserted to agree, so no row can pass on an answer the store
/// does not back.
async fn membership(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    project_id: &str,
) -> Vec<String> {
    let listed = list_project_assets(router, case, project_id, None).await;
    let stored = stored_membership(engine, project_id);
    assert_eq!(
        listed, stored,
        "case {case:?}: the listing must report the membership stored on the project's \
         record, got {listed:?} against {stored:?}"
    );
    listed
}

/// One row: a request served under a fresh token, and a DIFFERENT request of the
/// same operation sent under that same, now spent, token.
#[derive(Debug)]
struct Row {
    label: &'static str,
    batch: Batch,
    /// How many assets the row creates.
    assets: usize,
    /// The membership each of the row's projects starts from, by index into the
    /// created assets, put there by a call carrying no token at all.
    start: Vec<usize>,
    /// The ids the first request names, by index.
    first: Vec<usize>,
    /// The ids the DIFFERING second request names, by index. An index repeated
    /// here is an id repeated inside one request.
    second: Vec<usize>,
    /// Whether the second request addresses a SECOND project rather than the one
    /// the first request addressed — the member that differs is then the id in the
    /// path, and the first request's project is shown untouched by it.
    second_addresses_another_project: bool,
    /// The membership of the FIRST request's project once it has been served.
    after_first: Vec<usize>,
    /// The membership of the project the SECOND request addressed, once it has
    /// been served.
    after_second: Vec<usize>,
}

/// The rows.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "associate-differing-in-the-length-of-the-list",
            batch: Batch::Associate,
            assets: 3,
            start: Vec::new(),
            first: vec![0],
            second: vec![0, 1, 2],
            second_addresses_another_project: false,
            after_first: vec![0],
            after_second: vec![0, 1, 2],
        },
        Row {
            label: "associate-differing-in-an-id-repeated-inside-the-list",
            batch: Batch::Associate,
            assets: 2,
            start: Vec::new(),
            first: vec![0],
            second: vec![1, 1],
            second_addresses_another_project: false,
            after_first: vec![0],
            after_second: vec![0, 1],
        },
        Row {
            label: "associate-differing-in-the-project-the-path-names",
            batch: Batch::Associate,
            assets: 1,
            start: Vec::new(),
            first: vec![0],
            second: vec![0],
            second_addresses_another_project: true,
            after_first: vec![0],
            after_second: vec![0],
        },
        Row {
            label: "associate-onto-a-membership-the-row-starts-from",
            batch: Batch::Associate,
            assets: 3,
            start: vec![0],
            first: vec![1],
            second: vec![2],
            second_addresses_another_project: false,
            after_first: vec![0, 1],
            after_second: vec![0, 1, 2],
        },
        Row {
            label: "disassociate-differing-in-the-length-of-the-list",
            batch: Batch::Disassociate,
            assets: 3,
            start: vec![0, 1, 2],
            first: vec![0],
            second: vec![0, 1],
            second_addresses_another_project: false,
            after_first: vec![1, 2],
            after_second: vec![2],
        },
        Row {
            label: "disassociate-differing-in-an-id-repeated-inside-the-list",
            batch: Batch::Disassociate,
            assets: 3,
            start: vec![0, 1, 2],
            first: vec![0],
            second: vec![1, 1],
            second_addresses_another_project: false,
            after_first: vec![1, 2],
            after_second: vec![2],
        },
        Row {
            label: "disassociate-differing-in-the-project-the-path-names",
            batch: Batch::Disassociate,
            assets: 2,
            start: vec![0, 1],
            first: vec![0],
            second: vec![0],
            second_addresses_another_project: true,
            after_first: vec![1],
            after_second: vec![1],
        },
    ]
}

/// The ids `indices` name, sorted: a membership is a SET ordered by id, and that
/// is the order both the listing and the stored record enumerate.
fn ids(assets: &[String], indices: &[usize]) -> Vec<String> {
    let mut named: Vec<String> = indices.iter().map(|at| assets[*at].clone()).collect();
    named.sort();
    named.dedup();
    named
}

#[tokio::test]
async fn a_batch_project_asset_request_differing_from_the_call_its_token_names_is_carried_out() {
    for row in rows() {
        let label = row.label;
        let (router, engine) = router_with_engine();
        let first_project = create_project(&router, label, &format!("{label}-project")).await;
        let second_project = if row.second_addresses_another_project {
            Some(create_project(&router, label, &format!("{label}-other-project")).await)
        } else {
            None
        };
        let addressed_second = second_project.as_ref().unwrap_or(&first_project).clone();
        let assets = create_assets(&router, label, row.assets).await;

        // The membership each addressed project starts from, put there by a call
        // carrying no token: the row's token is spent by the first request below.
        let start = ids(&assets, &row.start);
        if !start.is_empty() {
            for project_id in [Some(&first_project), second_project.as_ref()]
                .into_iter()
                .flatten()
            {
                send_served(&router, label, Batch::Associate, project_id, &start, None).await;
            }
        }

        let token = token_for(label);
        let first_named: Vec<String> = row.first.iter().map(|at| assets[*at].clone()).collect();
        let second_named: Vec<String> = row.second.iter().map(|at| assets[*at].clone()).collect();

        // The first request spends the token.
        send_served(
            &router,
            label,
            row.batch,
            &first_project,
            &first_named,
            Some(&token),
        )
        .await;
        let after_first = ids(&assets, &row.after_first);
        let reported = membership(&router, &engine, label, &first_project).await;
        assert_eq!(
            reported, after_first,
            "case {label:?}: after the first call the project must hold {after_first:?}, \
             got {reported:?}"
        );

        // A DIFFERENT request under that same, now spent, token: a different call,
        // which has to be carried out rather than answered from the first one.
        send_served(
            &router,
            label,
            row.batch,
            &addressed_second,
            &second_named,
            Some(&token),
        )
        .await;
        let after_second = ids(&assets, &row.after_second);
        let reported = membership(&router, &engine, label, &addressed_second).await;
        assert_eq!(
            reported, after_second,
            "case {label:?}: the differing request must leave the project holding \
             {after_second:?}, got {reported:?}"
        );

        // A second project is addressed by the differing request ALONE, so the
        // membership the first call wrote is untouched by it.
        if second_project.is_some() {
            let reported = membership(&router, &engine, label, &first_project).await;
            assert_eq!(
                reported, after_first,
                "case {label:?}: the project the first call addressed must still hold \
                 {after_first:?}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_request_under_a_spent_token_changes_nothing_and_leaves_the_token_spendable() {
    // A request naming an asset this service does not hold is refused in full, and a
    // refused call applied nothing — so it neither moves the membership nor consumes
    // the token it carried, whatever that token was already spent on.
    let cases = [
        (Batch::Associate, "refused-association"),
        (Batch::Disassociate, "refused-disassociation"),
    ];
    for (batch, label) in cases {
        let (router, engine) = router_with_engine();
        let project_id = create_project(&router, label, &format!("{label}-project")).await;
        let assets = create_assets(&router, label, 2).await;
        let token = token_for(label);

        // A disassociation needs the assets in the project to begin with.
        if batch == Batch::Disassociate {
            send_served(&router, label, Batch::Associate, &project_id, &assets, None).await;
        }

        // The call that spends the token.
        send_served(
            &router,
            label,
            batch,
            &project_id,
            &assets[..1],
            Some(&token),
        )
        .await;
        let settled = membership(&router, &engine, label, &project_id).await;

        // A DIFFERING request under that spent token, naming an asset this service
        // does not hold: refused in full.
        let absent = vec![ABSENT_ID.to_string()];
        let (status, refused) = send(&router, batch, &project_id, &absent, Some(&token)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: an absent asset is the modelled 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {label:?}: the modelled 404 code, got {refused}"
        );
        // The refusal names the id THIS request sent, so the caller can tell which
        // of the ids it listed the service does not hold. The exact wording every
        // not-found refusal of this service carries is pinned by the suites that own
        // each refusal, not re-asserted here.
        assert!(
            error_message(&refused).contains(ABSENT_ID),
            "case {label:?}: the refusal must name the id the request sent, got {refused}"
        );
        let reported = membership(&router, &engine, label, &project_id).await;
        assert_eq!(
            reported, settled,
            "case {label:?}: a refused request changes no membership, expected \
             {settled:?}, got {reported:?}"
        );

        // The refusal spent nothing, so a further differing request under the same
        // token is still served — and applied.
        send_served(
            &router,
            label,
            batch,
            &project_id,
            &assets[1..],
            Some(&token),
        )
        .await;
        let expected: Vec<String> = match batch {
            Batch::Associate => {
                let mut both = assets.clone();
                both.sort();
                both
            }
            Batch::Disassociate => Vec::new(),
        };
        let reported = membership(&router, &engine, label, &project_id).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: the further differing request must leave the project \
             holding {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_order_of_the_ids_is_a_member_a_batch_project_asset_request_is_recognised_by() {
    // `assetIds` is a LIST, which is ordered, so two orderings of the same ids are
    // two requests. A membership is a SET, so the two can only be told apart by
    // changing the membership in between: the row applies the call, puts ONE id
    // back with a call carrying no token, and then sends the SAME ids in the
    // REVERSE order under the spent token. A replay would leave the membership the
    // untokened call made; a request carried out restores what the first call left.
    let cases = [
        (Batch::Associate, "reversed-ids-of-an-association"),
        (Batch::Disassociate, "reversed-ids-of-a-disassociation"),
    ];
    for (batch, label) in cases {
        let (router, engine) = router_with_engine();
        let project_id = create_project(&router, label, &format!("{label}-project")).await;
        let assets = create_assets(&router, label, 2).await;
        let token = token_for(label);

        // A disassociation needs both assets in the project to begin with.
        if batch == Batch::Disassociate {
            send_served(&router, label, Batch::Associate, &project_id, &assets, None).await;
        }

        send_served(&router, label, batch, &project_id, &assets, Some(&token)).await;
        let applied = membership(&router, &engine, label, &project_id).await;

        send_served(
            &router,
            label,
            batch.inverse(),
            &project_id,
            &assets[..1],
            None,
        )
        .await;
        let between = membership(&router, &engine, label, &project_id).await;
        assert_ne!(
            between, applied,
            "case {label:?}: the untokened call must itself move the membership, or \
             this row could not tell a replay from a call carried out, got \
             {between:?}"
        );

        let reversed: Vec<String> = assets.iter().rev().cloned().collect();
        send_served(&router, label, batch, &project_id, &reversed, Some(&token)).await;
        let reported = membership(&router, &engine, label, &project_id).await;
        assert_eq!(
            reported, applied,
            "case {label:?}: the same ids in the reverse order are a different \
             request and must be carried out, leaving {applied:?}; a replay would \
             have left {between:?}, got {reported:?}"
        );
    }
}
