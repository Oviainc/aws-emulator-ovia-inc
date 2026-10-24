// SPDX-License-Identifier: CC0-1.0

//! The rule: a deployment takes a configuration update only while its lifecycle is at
//! REST.
//!
//! While a deployment is in a NON-TERMINAL `DeploymentStatus` — one that still owes the
//! clock a step — `UpdateDeployment` is refused with its modelled `ValidationException`
//! (HTTP 400) and the lifecycle under way is left exactly as it was: every step it
//! still owed is taken, in order, and the statuses and events those steps carry are
//! still what the readers report afterwards.
//!
//! The table spans EVERY non-terminal status the model declares for this service's
//! deployment lifecycles (`DeploymentStatus`: `CREATING`, `VALIDATING`, `IN_PROGRESS`,
//! `UPDATE_IN_PROGRESS`, `DELETE_INITIATING`, `DELETE_IN_PROGRESS`), each reached by the
//! call that starts it plus the time the clock needs, and each row is asked with `force`
//! unset AND set, because `force` overrides the schema's validation warnings rather than
//! the state the resource is in. The second suite is the other side of the rule: from
//! each RESTING status (`COMPLETED`, `UPDATE_COMPLETED`) the same request is taken.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::DEPLOYMENT_STATUS_VALUES;

use crate::deployment_lifecycle::{create, get};
use crate::deployment_update::{
    conforming_specifications, error_message, events, one_pattern_per_workload, pattern_data,
    update_body, with_member,
};
use crate::harness::Harness;

/// One clock-owed status step (one `state::TRANSITION_SPAN`) plus slack. The emulator
/// never sleeps; this is the test waiting for the clock.
const ONE_STEP: Duration = Duration::from_millis(600);

/// How many steps a create still owes after the call that accepted it — the number of
/// non-terminal statuses it walks before it comes to rest.
const CREATE_STEPS: u32 = 3;

/// Long enough for every step ANY of these lifecycles still owes to come due.
const SETTLE: Duration = Duration::from_millis(2_100);

/// The lifecycle a row leaves in flight when the update arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    /// The deployment is still being created.
    Create,
    /// An earlier update is still being applied.
    Update,
    /// The deployment is being deleted.
    Delete,
}

impl Lifecycle {
    /// The status this lifecycle comes to rest in, which is what every reader must
    /// report once the clock has taken the steps it owed.
    fn terminal_status(self) -> &'static str {
        match self {
            Self::Create => "COMPLETED",
            Self::Update => "UPDATE_COMPLETED",
            Self::Delete => "DELETED",
        }
    }

    /// The event names this lifecycle appends, oldest first, once it has finished.
    fn events(self) -> &'static [&'static str] {
        match self {
            Self::Create => &[
                "DeploymentCreated",
                "DeploymentValidating",
                "DeploymentInProgress",
                "DeploymentCompleted",
            ],
            Self::Update => &["DeploymentUpdateInProgress", "DeploymentUpdateCompleted"],
            Self::Delete => &["DeleteInitiating", "DeleteInProgress", "DeleteCompleted"],
        }
    }
}

/// One row: what the deployment is doing when the update arrives, how many clock-owed
/// steps of that lifecycle have come due first, and the `DeploymentStatus` the
/// deployment is therefore in.
///
/// Together the rows cover every non-terminal member of the model's `DeploymentStatus`
/// enum this emulator's lifecycles pass through.
const ROWS: [(&str, Lifecycle, u32, &str); 6] = [
    ("a create that was only just accepted", Lifecycle::Create, 0, "CREATING"),
    ("a create whose settings are being checked", Lifecycle::Create, 1, "VALIDATING"),
    ("a create that is provisioning", Lifecycle::Create, 2, "IN_PROGRESS"),
    ("an update that is already under way", Lifecycle::Update, 0, "UPDATE_IN_PROGRESS"),
    ("a delete that was only just accepted", Lifecycle::Delete, 0, "DELETE_INITIATING"),
    ("a delete that is under way", Lifecycle::Delete, 1, "DELETE_IN_PROGRESS"),
];

/// The settings every row submits: the full map the row's pattern publishes, so the
/// request would be applied if the deployment's state allowed it.
async fn settings(harness: &Harness, workload: &str, pattern: &str) -> Map<String, Value> {
    let case = format!("{workload}/{pattern} settings");
    let data = pattern_data(harness, &case, workload, pattern).await;
    conforming_specifications(&case, &data, true)
}

/// The event names of `id`, oldest first.
async fn event_names(harness: &Harness, case: &str, id: &str) -> Vec<String> {
    events(harness, case, id)
        .await
        .iter()
        .map(|event| event["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn an_update_of_a_deployment_whose_lifecycle_is_in_flight_is_refused() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let conforming = settings(&harness, &workload, &pattern).await;

    // Phase one: every row is driven into its status and refused there. The two force
    // values share the row's deployment, because a refusal changes nothing.
    let mut rows = Vec::new();
    for (index, (label, lifecycle, steps, expected)) in ROWS.into_iter().enumerate() {
        let case = format!("{label} ({workload}/{pattern})");
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("in-flight-{index}"),
        )
        .await;

        // The lifecycle this row is about is started, then exactly `steps` of the
        // steps it owes are let through.
        match lifecycle {
            Lifecycle::Create => {}
            Lifecycle::Update => {
                std::thread::sleep(ONE_STEP * CREATE_STEPS);
                harness
                    .ok(&case, "/updateDeployment", update_body(&id, &conforming))
                    .await;
            }
            Lifecycle::Delete => {
                std::thread::sleep(ONE_STEP * CREATE_STEPS);
                harness
                    .ok(&case, "/deleteDeployment", json!({"deploymentId": id}))
                    .await;
            }
        }
        if steps > 0 {
            std::thread::sleep(ONE_STEP * steps);
        }

        let before = get(&harness, &case, &id).await;
        let status = before["status"].as_str().unwrap_or_default().to_string();
        assert!(
            DEPLOYMENT_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: status {status:?} is not a DeploymentStatus value"
        );
        assert_eq!(
            status, expected,
            "case {case:?}: the row must reach the status it is about, got {status:?}"
        );

        for forced in [false, true] {
            let case = format!("{case} with force={forced}");
            let refused = harness
                .refused(
                    &case,
                    "/updateDeployment",
                    with_member(&update_body(&id, &conforming), "force", json!(forced)),
                    400,
                    "ValidationException",
                )
                .await;
            let message = error_message(&refused);
            assert!(
                !message.is_empty(),
                "case {case:?}: a refused update must carry a message, got {refused}"
            );
            // The refusal says what state the deployment is in, never which id was
            // asked about: a deployment id is caller input and the existence of one is
            // not disclosed through the error channel.
            assert!(
                !message.contains(id.as_str()),
                "case {case:?}: the message must not render the requested id, got {message:?}"
            );
            let after = get(&harness, &case, &id).await;
            assert_eq!(
                after["specifications"], before["specifications"],
                "case {case:?}: a refused update must persist nothing, got {}",
                after["specifications"]
            );
        }
        rows.push((case, id, lifecycle));
    }

    // Phase two: ONE wait, after which every row's lifecycle has taken the steps it
    // still owed — the refused update neither cancelled them nor replaced them with an
    // update's own.
    std::thread::sleep(SETTLE);
    for (case, id, lifecycle) in rows {
        let settled = get(&harness, &case, &id).await;
        assert_eq!(
            settled["status"].as_str(),
            Some(lifecycle.terminal_status()),
            "case {case:?}: the in-flight lifecycle must still reach {:?}, got {}",
            lifecycle.terminal_status(),
            settled["status"]
        );
        let names = event_names(&harness, &case, &id).await;
        // Every event that lifecycle owed is in the history, in order, and the refused
        // update added none of its own.
        let expected_tail: Vec<String> = lifecycle
            .events()
            .iter()
            .map(ToString::to_string)
            .collect();
        let tail: Vec<String> = names
            .iter()
            .skip(names.len().saturating_sub(expected_tail.len()))
            .cloned()
            .collect();
        assert_eq!(
            tail, expected_tail,
            "case {case:?}: the lifecycle's own steps must be the history's last events, \
             got {names:?}"
        );
        if lifecycle != Lifecycle::Update {
            assert!(
                !names.iter().any(|name| name.starts_with("DeploymentUpdate")),
                "case {case:?}: a refused update must append no event of its own, got {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_is_taken_from_every_status_the_lifecycle_comes_to_rest_in() {
    let harness = Harness::new();
    // One row per workload the catalogue holds, so the rule is not read off a single
    // pattern, and per resting status: straight after the create has settled, and
    // after an earlier UPDATE has settled.
    let mut rows = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let conforming = settings(&harness, &workload, &pattern).await;
        for (step, resting) in [(0u32, "COMPLETED"), (1, "UPDATE_COMPLETED")] {
            let case = format!("{workload}/{pattern} at rest in {resting}");
            let id = create(
                &harness,
                &case,
                &workload,
                &pattern,
                &format!("at-rest-{index}-{step}"),
            )
            .await;
            rows.push((case, id, resting, conforming.clone(), step));
        }
    }
    std::thread::sleep(SETTLE);

    for (case, id, resting, conforming, step) in &rows {
        // The second row needs an earlier update of its own to have settled, which is
        // the status it is about.
        if *step == 1 {
            harness
                .ok(case, "/updateDeployment", update_body(id, conforming))
                .await;
            std::thread::sleep(ONE_STEP * 2);
        }
        let before = get(&harness, case, id).await;
        assert_eq!(
            before["status"].as_str(),
            Some(*resting),
            "case {case:?}: the row must start from the resting status it is about, got {}",
            before["status"]
        );

        let answered = harness
            .ok(case, "/updateDeployment", update_body(id, conforming))
            .await;
        assert_eq!(
            answered["deployment"]["status"].as_str(),
            Some("UPDATE_IN_PROGRESS"),
            "case {case:?}: a deployment at rest must take the update, got {answered}"
        );
        let read = get(&harness, case, id).await;
        assert_eq!(
            read["specifications"],
            Value::Object(conforming.clone()),
            "case {case:?}: the submitted settings must be the stored ones, got {}",
            read["specifications"]
        );
    }
}
