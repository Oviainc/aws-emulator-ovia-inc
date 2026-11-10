// SPDX-License-Identifier: CC0-1.0

//! WHICH repeat of an `UpdateFramework` is the one that takes no action.
//!
//! The rule under test is the member's own: `UpdateFramework$IdempotencyToken`
//! is "a customer-chosen string that you can use to distinguish between
//! otherwise identical calls to UpdateFrameworkInput. Retrying a successful
//! request with the same idempotency token results in a success message with no
//! action taken" (`smithy_query` operation `UpdateFramework`). A call is
//! therefore a retry — and takes no action — only when its token is the one the
//! update that last succeeded ON THAT FRAMEWORK carried. Any other token is a
//! new action and writes: one another framework's update used, the one this
//! framework's create used, a token no earlier call used, and no token at all.
//!
//! What tells "acted" from "took no action" apart is what the framework reports
//! afterwards through `DescribeFramework`: a call that acted leaves the
//! description it carried and the non-terminal deployment status a write
//! re-enters, while a retry leaves the description and the settled, terminal
//! status the previous update already left.
//!
//! The request harness is the shared one in `vault_and_backup_job.rs`, the
//! framework helpers are `framework.rs`'s and the update binding is
//! `framework_update.rs`'s, so there is ONE wire path for this service's suites.

use serde_json::{json, Value};

use crate::framework::{
    create_framework, describe_framework, text, DEPLOYMENT_COMPLETED, PAST_FRAMEWORK_SETTLE,
};
use crate::framework_update::{
    create_request as framework_create_request, created_controls, update_framework,
    DEPLOYMENT_UPDATE_IN_PROGRESS,
};
use crate::vault_and_backup_job::router;

/// The description the update that succeeds BEFORE each row's probe writes.
const FIRST_DESCRIPTION: &str = "the update that last succeeded";

/// The description each row's probe update carries, so a probe that acted is
/// distinguishable from one that did not by what the framework reports.
const PROBE_DESCRIPTION: &str = "what the probe would write";

/// A second framework, updated with a token of its own: the row that probes a
/// token another framework's update used repeats THIS token.
const NEIGHBOUR: &str = "NeighbourFramework";
const NEIGHBOUR_CREATE_TOKEN: &str = "neighbour-create-token";
const NEIGHBOUR_UPDATE_TOKEN: &str = "neighbour-update-token";
const NEIGHBOUR_DESCRIPTION: &str = "the neighbour's own update";

/// Where the token a row's probe update carries comes from.
#[derive(Debug, Clone, Copy)]
enum ProbeToken {
    /// The token the update that last succeeded on this framework carried.
    OwnLastUpdate,
    /// The token another framework's successful update carried.
    AnotherFrameworksUpdate,
    /// The token this framework's own create carried.
    OwnCreate,
    /// A token no earlier call used.
    Unused,
    /// No token at all.
    Absent,
}

/// The rows: (case, whether the update before the probe carried a token, where
/// the probe's token comes from, whether the rule makes that probe a retry that
/// takes no action). The rows differ in the PROVENANCE of the probe's token,
/// which is what the rule generalises over.
fn rows() -> Vec<(&'static str, bool, ProbeToken, bool)> {
    vec![
        (
            "the token the update that last succeeded on this framework carried",
            true,
            ProbeToken::OwnLastUpdate,
            true,
        ),
        (
            "a token another framework's successful update carried",
            true,
            ProbeToken::AnotherFrameworksUpdate,
            false,
        ),
        (
            "the token this framework's create carried",
            false,
            ProbeToken::OwnCreate,
            false,
        ),
        (
            "no token on the probe and none on the update before it",
            false,
            ProbeToken::Absent,
            false,
        ),
        (
            "a token no earlier call used",
            true,
            ProbeToken::Unused,
            false,
        ),
    ]
}

/// The framework row `index` is probed on.
fn row_name(index: usize) -> String {
    format!("TokenRow{index}")
}

/// The `IdempotencyToken` row `index`'s CREATE carries — distinct per row, since
/// a repeated create token replays the framework that token made rather than
/// making a second one.
fn create_token(index: usize) -> String {
    format!("create-token-{index}")
}

/// The `IdempotencyToken` the update before row `index`'s probe carries, when
/// that row's update carries one.
fn own_update_token(index: usize) -> String {
    format!("own-update-token-{index}")
}

/// The token a row's probe update carries, resolved from its provenance.
fn probe_token(kind: ProbeToken, index: usize) -> Option<String> {
    match kind {
        ProbeToken::OwnLastUpdate => Some(own_update_token(index)),
        ProbeToken::AnotherFrameworksUpdate => Some(NEIGHBOUR_UPDATE_TOKEN.to_string()),
        ProbeToken::OwnCreate => Some(create_token(index)),
        ProbeToken::Unused => Some(format!("unused-token-{index}")),
        ProbeToken::Absent => None,
    }
}

/// The create every framework in this suite is made with: the fixture the sibling
/// suite starts its frameworks from, with this framework's own token on it, since
/// a repeated create token replays the framework that token made rather than
/// making a second one.
fn create_request(name: &str, token: &str) -> Value {
    let mut request = framework_create_request(name);
    request["IdempotencyToken"] = json!(token);
    request
}

/// An `UpdateFramework` body carrying `description`, and `token` when the row
/// gives it one — an absent member rather than a null one, which is how a client
/// that chose no token sends the call.
fn update_body(description: &str, token: Option<&str>) -> Value {
    let mut body = json!({"FrameworkDescription": description});
    if let Some(token) = token {
        body["IdempotencyToken"] = json!(token);
    }
    body
}

/// An update is a retry that takes no action only when its `IdempotencyToken` is
/// the one the update that last succeeded on THAT framework carried; a token of
/// any other provenance is a new action that writes. In both cases the answer is
/// a success reporting the stored framework's own identifiers, and no probe
/// writes a framework other than the one it named.
#[tokio::test]
async fn an_update_token_is_a_retry_only_of_that_frameworks_last_update() {
    let router = router();
    let rows = rows();

    // Every framework is created up front, so ONE wait settles every create's
    // deployment rather than one wait per row.
    let (status, body) = create_framework(
        &router,
        &create_request(NEIGHBOUR, NEIGHBOUR_CREATE_TOKEN),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the neighbour's CreateFramework 200, got {status} {body:?}"
    );
    let mut created = Vec::new();
    for (index, &(case, ..)) in rows.iter().enumerate() {
        let (status, body) =
            create_framework(&router, &create_request(&row_name(index), &create_token(index))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {body:?}"
        );
        created.push(body);
    }
    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    // The updates the probes below repeat the tokens of: the neighbour's, and
    // each row's own.
    let (status, body) = update_framework(
        &router,
        NEIGHBOUR,
        &update_body(NEIGHBOUR_DESCRIPTION, Some(NEIGHBOUR_UPDATE_TOKEN)),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the neighbour's UpdateFramework 200, got {status} {body:?}"
    );
    for (index, &(case, first_tokened, ..)) in rows.iter().enumerate() {
        let token = first_tokened.then(|| own_update_token(index));
        let (status, body) = update_framework(
            &router,
            &row_name(index),
            &update_body(FIRST_DESCRIPTION, token.as_deref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first UpdateFramework 200, got {status} {body:?}"
        );
    }
    // That update's own deployment is settled, so what the probe meets is a
    // framework no action is in flight on: a probe that does nothing leaves the
    // terminal status, and one that acts re-enters the non-terminal one.
    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    for (index, &(case, _, probe, takes_no_action)) in rows.iter().enumerate() {
        let name = row_name(index);
        let token = probe_token(probe, index);
        let (status, answered) = update_framework(
            &router,
            &name,
            &update_body(PROBE_DESCRIPTION, token.as_deref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateFramework 200, got {status} {answered:?}"
        );
        assert!(
            !text(&answered, "FrameworkName").is_empty()
                && !text(&answered, "FrameworkArn").is_empty(),
            "case {case:?}: the answer names the framework it wrote, got {answered:?}"
        );
        assert_eq!(
            (
                answered.get("FrameworkName"),
                answered.get("FrameworkArn"),
            ),
            (
                created[index].get("FrameworkName"),
                created[index].get("FrameworkArn"),
            ),
            "case {case:?}: the answer reports the stored framework's identifiers, got {answered:?}"
        );

        let (expected_description, expected_deployment) = if takes_no_action {
            (FIRST_DESCRIPTION, DEPLOYMENT_COMPLETED)
        } else {
            (PROBE_DESCRIPTION, DEPLOYMENT_UPDATE_IN_PROGRESS)
        };
        let (status, read) = describe_framework(&router, &name).await;
        assert_eq!(
            (
                status,
                text(&read, "FrameworkDescription").as_str(),
                text(&read, "DeploymentStatus").as_str(),
            ),
            (200, expected_description, expected_deployment),
            "case {case:?}: expected the framework to report {expected_description:?} at \
             {expected_deployment}, got {status} {read:?}"
        );
    }

    // No probe wrote the framework whose token it repeated: the neighbour still
    // reports its own update, settled, with the controls it was created with.
    let (status, neighbour) = describe_framework(&router, NEIGHBOUR).await;
    assert_eq!(
        (
            status,
            text(&neighbour, "FrameworkDescription").as_str(),
            text(&neighbour, "DeploymentStatus").as_str(),
            neighbour.get("FrameworkControls"),
        ),
        (
            200,
            NEIGHBOUR_DESCRIPTION,
            DEPLOYMENT_COMPLETED,
            Some(&created_controls()),
        ),
        "an update of one framework must not write another, got {status} {neighbour:?}"
    );
}
