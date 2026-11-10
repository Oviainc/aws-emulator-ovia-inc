// SPDX-License-Identifier: CC0-1.0

//! The Map Runs an execution is ALREADY running, as the walk that CONTINUES that
//! execution's attempt reads them.
//!
//! "When you redrive an execution, it continues the failed execution from the
//! unsuccessful step and uses the same input. Step Functions preserves the results and
//! execution history of the successful steps, and doesn't rerun these steps"
//! (`RedriveExecution`), and for a Distributed Map "you must redrive the parent
//! workflow. The parent workflow redrives all the unsuccessful states, including a
//! failed Map Run."
//!
//! So a redriven attempt enters the same `Map` state again, and two things about it are
//! facts of the EXECUTION rather than of the walk: the Map Run it is a second attempt
//! of — which keeps the ARN a client is holding, so `DescribeMapRun` follows the same
//! resource across the redrive — and which of its items already have a child workflow
//! execution that SUCCEEDED, because those are the ones that are not run again.
//!
//! Both live in this service's own store, so they are handed to the walk as the one
//! closure it asks them through ([`super::integration::MapRuns`]) — the shape
//! [`super::task_authorization::role_gate`], [`super::http_task::connections`] and
//! [`super::callbacks::handovers`] reach a store through: the walk knows nothing of the
//! store and only asks.

use ::std::sync::Arc;

use super::integration::{FinishedChild, MapRuns, ResumedMapRun};

/// What the Map Runs of `execution_arn` answer a continued attempt of it with; `None`
/// for an engine carrying no store of this service, where there is nothing to read.
///
/// The Map Run is matched by the `Map` state's own LABEL — "a user-defined or an
/// auto-generated string that identifies a Map state" — and, among the Map Runs that
/// label has, by WHICH entry into that state the walk is making: one Distributed Map
/// nested inside another is entered once per outer item, and each of those entries is a
/// Map Run in its own right. A state the execution holds no Map Run for answers
/// nothing, so the walk starts one of its own.
pub fn resumed<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    execution_arn: &str,
) -> Option<MapRuns> {
    super::service_state(&**engine)?;
    let engine = Arc::clone(engine);
    let execution_arn = execution_arn.to_string();
    Some(Arc::new(move |occurrence: usize, label: &str| {
        let state = super::service_state(&*engine)?;
        let store = state.lock();
        // The Map Runs this execution has, in the order their states were entered,
        // which is the order a second entry into the same state follows.
        let arn = store
            .map_runs_by_execution
            .get(&execution_arn)?
            .iter()
            .filter(|arn| {
                store
                    .map_runs
                    .get(*arn)
                    .is_some_and(|map_run| map_run.label == label)
            })
            .nth(occurrence)?
            .clone();
        // The children are listed in the order their items were STARTED, which is the
        // order of the items themselves, so the position in this list is the item's own
        // index. An item whose child did not succeed is left empty: it is one of the
        // items the continued attempt runs again.
        let items = store
            .executions_by_map_run
            .get(&arn)
            .map(|children| {
                children
                    .iter()
                    .map(|child| succeeded_child(&store, child))
                    .collect()
            })
            .unwrap_or_default();
        Some(ResumedMapRun { arn, items })
    }))
}

/// The child workflow execution `arn` names, for one that SUCCEEDED; `None` for a
/// child that failed, one the store no longer holds, and one whose outcome the clock
/// has still to make due — which is not a success yet, so the item it ran is run again.
fn succeeded_child(
    store: &super::ComAmazonawsSfnAwsstepfunctionsState,
    arn: &str,
) -> Option<FinishedChild> {
    let child = store.executions.get(arn)?;
    if child.status != super::EXECUTION_STATUS_SUCCEEDED {
        return None;
    }
    Some(FinishedChild {
        name: child.name.clone(),
        arn: child.arn.clone(),
        input: child.input.clone(),
        output: child.output.clone(),
        start_date: child.start_date,
        // An execution the store reports SUCCEEDED has stopped, so it carries the
        // instant it stopped at; the start stands in for a record that somehow does
        // not, rather than an instant nothing reached being invented for it.
        stop_date: child.stop_date.unwrap_or(child.start_date),
        // "The total number of items processed in a child workflow execution": the
        // count that execution already reports, which does not change by being kept.
        item_count: child.item_count.unwrap_or(1),
    })
}
