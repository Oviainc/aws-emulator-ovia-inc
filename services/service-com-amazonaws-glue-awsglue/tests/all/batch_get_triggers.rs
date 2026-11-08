// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetTriggers`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! RULE: `BatchGetTriggers` answers 200 and reports, for EACH name in `TriggerNames` and in
//! the order the request listed them, either the trigger definition the endpoint's Region
//! holds under that name — the same projection `GetTrigger` reports — or that name in
//! `TriggersNotFound`; a name no trigger holds is not an error, and the call changes nothing
//! a later caller sees.
//!
//! The tables below span the batch along what the rule generalises over: how many names are
//! asked for, how many of them are stored, where the misses sit in the order, whether a name
//! repeats, which Region's endpoint is asked, the trigger TYPE and so which definition
//! members carry a value, and the length of a name.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::batch_get_triggers::MAX_BATCH_TRIGGER_NAMES;
use service_com_amazonaws_glue_awsglue::ops::types::TRIGGER_TYPE_VALUES;
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, emulator_on, error_code, names_in, ok, string_list, REGION};

/// The role ARN every job a trigger here acts on is created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-trigger-role";

/// The job every trigger here runs. A trigger's action names a job of the Region the create
/// is served in, so each suite registers it first.
const JOB: &str = "batch-trigger-job";

/// The job the conditional row's predicate waits on. A conditional trigger may not name
/// one job both in its `Predicate` and in its `Actions` ("Job cannot be a Predicate and
/// Action for Conditional Trigger."), so the predicate waits on a job no action runs.
const PREDICATE_JOB: &str = "batch-trigger-predicate-job";

/// Create the job [`JOB`] through `region`'s endpoint, so an action may name it.
async fn given_job_in(router: &MultiServiceRouter, region: &str) {
    for job in [JOB, PREDICATE_JOB] {
        let (status, body) = call_in_region(
            router,
            region,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreateJob must answer 200 for {job}, got {status} with {body}"
        );
    }
}

/// An `ON_DEMAND` trigger definition that runs [`JOB`], with a description so each stored
/// trigger carries a member distinguishing it from its siblings.
fn trigger_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Type": "ON_DEMAND",
        "Description": format!("trigger {name}"),
        "Actions": [{ "JobName": JOB }],
    })
}

/// One `BatchGetTriggers` call that must succeed, as
/// `(the reported Triggers' names, TriggersNotFound)`.
async fn batch_get(router: &MultiServiceRouter, names: &[&str]) -> (Vec<String>, Vec<String>) {
    let body = ok(router, "BatchGetTriggers", &json!({ "TriggerNames": names })).await;
    (trigger_names(&body), string_list(&body, "TriggersNotFound"))
}

/// The `Name` of every element of the response's `Triggers` list, in order — read through
/// the shared harness helper, so this suite holds no copy of it.
fn trigger_names(body: &Value) -> Vec<String> {
    names_in(body, "Triggers")
}

// ---------------------------------------------------------------------------
// The batch splits the requested names across the two lists
// ---------------------------------------------------------------------------

/// One row of the batch table: the case name, the names the request asks for, the names the
/// response must report in `Triggers`, and the names it must report in `TriggersNotFound`.
type BatchCase<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);

/// Each requested name is reported in exactly one of the two lists, in the order asked.
///
/// The rows differ along the batch's size, how many of its names are stored, where the
/// misses sit (first, last, interleaved), and whether a name repeats.
#[tokio::test]
async fn every_requested_name_is_reported_exactly_once() {
    let router = emulator();
    given_job_in(&router, REGION).await;
    for stored in ["alpha", "beta", "gamma"] {
        ok(&router, "CreateTrigger", &trigger_body(stored)).await;
    }

    // (case, requested names, expected Triggers names, expected TriggersNotFound)
    let cases: Vec<BatchCase> = vec![
        ("one stored name", vec!["alpha"], vec!["alpha"], vec![]),
        (
            // The recorded reference case: a batch whose every name is stored.
            "every name stored",
            vec!["alpha", "beta", "gamma"],
            vec!["alpha", "beta", "gamma"],
            vec![],
        ),
        (
            // The request's order, not the store's: a batch is not a listing.
            "every name stored, asked out of name order",
            vec!["gamma", "alpha", "beta"],
            vec!["gamma", "alpha", "beta"],
            vec![],
        ),
        (
            "no name stored",
            vec!["absent", "missing"],
            vec![],
            vec!["absent", "missing"],
        ),
        (
            "the miss comes first",
            vec!["absent", "beta"],
            vec!["beta"],
            vec!["absent"],
        ),
        (
            "the miss comes last",
            vec!["beta", "absent"],
            vec!["beta"],
            vec!["absent"],
        ),
        (
            "misses interleaved with hits",
            vec!["absent", "alpha", "missing", "gamma", "nope"],
            vec!["alpha", "gamma"],
            vec!["absent", "missing", "nope"],
        ),
        (
            // `Trigger$Name` carries no documented case fold, so a different casing is a
            // different trigger — and so a miss.
            "a stored name in another casing",
            vec!["ALPHA", "alpha"],
            vec!["alpha"],
            vec!["ALPHA"],
        ),
        (
            // One answer per requested name, not per distinct name.
            "a repeated stored name",
            vec!["beta", "beta"],
            vec!["beta", "beta"],
            vec![],
        ),
        (
            "a repeated unknown name",
            vec!["absent", "absent"],
            vec![],
            vec!["absent", "absent"],
        ),
        (
            // A batch with nothing in it: both lists are present and empty.
            "an empty batch",
            vec![],
            vec![],
            vec![],
        ),
    ];

    for (case, requested, expected_triggers, expected_missing) in cases {
        let body = ok(
            &router,
            "BatchGetTriggers",
            &json!({ "TriggerNames": requested }),
        )
        .await;

        let reported = trigger_names(&body);
        assert_eq!(
            reported, expected_triggers,
            "case {case:?}: expected Triggers {expected_triggers:?}, got {reported:?} in {body}"
        );
        let missing = string_list(&body, "TriggersNotFound");
        assert_eq!(
            missing, expected_missing,
            "case {case:?}: expected TriggersNotFound {expected_missing:?}, got {missing:?} in \
             {body}"
        );
        // The two lists together account for every requested name, once each.
        assert_eq!(
            reported.len() + missing.len(),
            requested.len(),
            "case {case:?}: every requested name must be reported exactly once, got {body}"
        );

        // A read does not change what the next caller sees: the same batch answers the same
        // thing, and each stored name is still readable through `GetTrigger`.
        let again = ok(
            &router,
            "BatchGetTriggers",
            &json!({ "TriggerNames": requested }),
        )
        .await;
        assert_eq!(
            again, body,
            "case {case:?}: BatchGetTriggers must be repeatable, got {again}"
        );
        for stored in ["alpha", "beta", "gamma"] {
            let read = ok(&router, "GetTrigger", &json!({ "Name": stored })).await;
            assert_eq!(
                read.get("Trigger").and_then(|trigger| trigger.get("Name")),
                Some(&json!(stored)),
                "case {case:?}: the batch read must not have touched {stored:?}, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What it reports is what the store holds
// ---------------------------------------------------------------------------

/// A trigger the batch reports is byte-for-byte the definition `GetTrigger` reports for the
/// same name — the batch is a different addressing mode over one stored resource, not a
/// second projection of it.
///
/// The rows cover every value of `TriggerType` (source:
/// `smithy_query(query="shape",shape="TriggerType")`, whose published list the crate's
/// `TRIGGER_TYPE_VALUES` carries), so the comparison spans the definition members each type
/// requires — a `Schedule`, a `Predicate`, an `EventBatchingCondition` — and the two states
/// a create can leave a trigger in.
#[tokio::test]
async fn a_batched_trigger_is_the_trigger_get_trigger_reports() {
    let router = emulator();
    given_job_in(&router, REGION).await;
    ok(
        &router,
        "CreateWorkflow",
        &json!({ "Name": "batch-trigger-workflow" }),
    )
    .await;

    let creates: Vec<(&str, Value)> = vec![
        (
            "ON_DEMAND, required members only",
            json!({
                "Name": "on-demand",
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "SCHEDULED, inside a workflow",
            json!({
                "Name": "scheduled",
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "WorkflowName": "batch-trigger-workflow",
                "Description": "the nightly load",
                "Actions": [{
                    "JobName": JOB,
                    "Arguments": { "--mode": "full" },
                    "Timeout": 30,
                    "NotificationProperty": { "NotifyDelayAfter": 10 },
                }],
            }),
        ),
        (
            "CONDITIONAL, started on creation",
            json!({
                "Name": "conditional",
                "Type": "CONDITIONAL",
                "StartOnCreation": true,
                "Actions": [{ "JobName": JOB }],
                "Predicate": {
                    "Logical": "ANY",
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": PREDICATE_JOB,
                        "State": "SUCCEEDED",
                    }],
                },
            }),
        ),
        (
            "EVENT, with a batching condition",
            json!({
                "Name": "event",
                "Type": "EVENT",
                "Actions": [{ "JobName": JOB }],
                "EventBatchingCondition": { "BatchSize": 5, "BatchWindow": 120 },
                "Tags": { "owner": "data-platform" },
            }),
        ),
    ];

    // Every modelled trigger type is represented above, so no type goes untested.
    for trigger_type in TRIGGER_TYPE_VALUES {
        assert!(
            creates
                .iter()
                .any(|(_, body)| body["Type"].as_str() == Some(*trigger_type)),
            "case {trigger_type:?}: every modelled TriggerType must have a row"
        );
    }

    let mut names = Vec::new();
    for (case, request) in &creates {
        let created = ok(&router, "CreateTrigger", request).await;
        assert!(
            created.get("Name").is_some(),
            "case {case:?}: CreateTrigger must answer with the trigger's name, got {created}"
        );
        names.push(
            request["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every row names its trigger"))
                .to_string(),
        );
    }

    let requested: Vec<&str> = names.iter().map(String::as_str).collect();
    let body = ok(
        &router,
        "BatchGetTriggers",
        &json!({ "TriggerNames": requested }),
    )
    .await;
    let reported = body
        .get("Triggers")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("BatchGetTriggers must report a Triggers list, got {body}"))
        .clone();

    for ((case, _), name) in creates.iter().zip(names.iter()) {
        let single = ok(&router, "GetTrigger", &json!({ "Name": name })).await;
        let expected = single
            .get("Trigger")
            .unwrap_or_else(|| panic!("case {case:?}: GetTrigger must report a Trigger, got {single}"));
        let actual = reported
            .iter()
            .find(|trigger| trigger.get("Name") == Some(&json!(name)))
            .unwrap_or_else(|| panic!("case {case:?}: the batch must report {name:?}, got {body}"));
        assert_eq!(
            actual, expected,
            "case {case:?}: the batched trigger must be the trigger GetTrigger reports, got \
             {actual}"
        );
        // `Tags` is not a member of the `Trigger` shape, so a batched trigger carries none.
        assert!(
            actual.get("Tags").is_none(),
            "case {case:?}: Tags is not a member of Trigger, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// The batch reads the store on every call
// ---------------------------------------------------------------------------

/// The batch reports what the store holds AT THE TIME OF THE CALL: a definition an
/// `UpdateTrigger` overwrote is reported as overwritten, a state a `StopTrigger` moved is
/// reported as moved, and a name whose trigger leaves the store moves to `TriggersNotFound`
/// and back when it is created again.
///
/// This is the recorded reference flow — a create, an update, then a batch read — with the
/// rows differing along which mutation preceded the batch. The trigger leaves the store
/// directly rather than through `DeleteTrigger`, so this case is about the batch read
/// alone.
#[tokio::test]
async fn the_batch_follows_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    given_job_in(&router, REGION).await;
    ok(&router, "CreateTrigger", &trigger_body("transient")).await;

    /// The `Trigger` the batch reports for the one name it is given.
    async fn only_trigger(router: &MultiServiceRouter, case: &str) -> Value {
        let body = ok(
            router,
            "BatchGetTriggers",
            &json!({ "TriggerNames": ["transient"] }),
        )
        .await;
        let reported = body
            .get("Triggers")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a Triggers list, got {body}"));
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one trigger, got {body}"
        );
        reported[0].clone()
    }

    let created = only_trigger(&router, "created").await;
    assert_eq!(
        created["Description"].as_str(),
        Some("trigger transient"),
        "case \"created\": the batch reports the created definition, got {created}"
    );

    // An update "updates the previous trigger definition by overwriting it completely", so
    // the batch reports the replacement and not the description the create left.
    ok(
        &router,
        "UpdateTrigger",
        &json!({
            "Name": "transient",
            "TriggerUpdate": {
                "Description": "overwritten by the update",
                "Actions": [{ "JobName": JOB, "Arguments": { "--mode": "delta" } }],
            },
        }),
    )
    .await;
    let updated = only_trigger(&router, "updated").await;
    assert_eq!(
        updated["Description"].as_str(),
        Some("overwritten by the update"),
        "case \"updated\": the batch reports the overwritten definition, got {updated}"
    );
    assert_eq!(
        updated["Actions"][0]["Arguments"]["--mode"].as_str(),
        Some("delta"),
        "case \"updated\": the batch reports the update's actions, got {updated}"
    );
    let single = ok(&router, "GetTrigger", &json!({ "Name": "transient" })).await;
    assert_eq!(
        single["Trigger"], updated,
        "case \"updated\": the batch and GetTrigger must report one record, got {single}"
    );

    // A state the service moved is reported too: the batch is a read of the live record.
    ok(&router, "StopTrigger", &json!({ "Name": "transient" })).await;
    let stopped = only_trigger(&router, "stopped").await;
    // "DEACTIVATED" is the modelled `TriggerState` of a stopped trigger (source:
    // `smithy_query(query="shape",shape="TriggerState")`).
    assert_eq!(
        stopped["State"].as_str(),
        Some("DEACTIVATED"),
        "case \"stopped\": the state must come from the store, got {stopped}"
    );
    let after_stop = ok(&router, "GetTrigger", &json!({ "Name": "transient" })).await;
    assert_eq!(
        after_stop["Trigger"], stopped,
        "case \"stopped\": the batch and GetTrigger must report one record, got {after_stop}"
    );

    // Gone from the store: the name becomes a miss.
    engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .triggers
        .get_mut(REGION)
        .expect("the Region's triggers")
        .remove("transient");
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert!(
        found.is_empty(),
        "case \"removed\": a trigger no longer stored is not reported, got {found:?}"
    );
    assert_eq!(
        missing,
        vec!["transient".to_string()],
        "case \"removed\": a trigger no longer stored is reported missing, got {missing:?}"
    );

    // Created again: the name is served again.
    ok(&router, "CreateTrigger", &trigger_body("transient")).await;
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"re-created\": a re-created trigger is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"re-created\": a re-created trigger is not missing, got {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// Only the endpoint's own triggers are reachable
// ---------------------------------------------------------------------------

/// A trigger is served only through the endpoint that created it, so one Region's batch
/// reports another Region's trigger as a miss rather than reporting it.
///
/// The rows span four Regions' endpoints, so nothing here depends on one Region name.
#[tokio::test]
async fn a_batch_reports_only_the_endpoints_own_triggers() {
    let router = emulator();
    let regions = [REGION, "us-west-2", "eu-west-1", "ap-southeast-1"];

    for region in regions {
        given_job_in(&router, region).await;
        let (status, body) = call_in_region(
            &router,
            region,
            "CreateTrigger",
            &trigger_body(&format!("trigger-{region}")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreateTrigger must answer 200, got {status} with {body}"
        );
    }

    let all: Vec<String> = regions
        .iter()
        .map(|region| format!("trigger-{region}"))
        .collect();
    for region in regions {
        let names: Vec<&str> = all.iter().map(String::as_str).collect();
        let (status, body) = call_in_region(
            &router,
            region,
            "BatchGetTriggers",
            &json!({ "TriggerNames": names }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: BatchGetTriggers must answer 200, got {status} with {body}"
        );
        assert_eq!(
            trigger_names(&body),
            vec![format!("trigger-{region}")],
            "case {region:?}: an endpoint reports its own triggers and no other Region's, got \
             {body}"
        );
        let expected_missing: Vec<String> = all
            .iter()
            .filter(|name| *name != &format!("trigger-{region}"))
            .cloned()
            .collect();
        assert_eq!(
            string_list(&body, "TriggersNotFound"),
            expected_missing,
            "case {region:?}: another Region's triggers are misses, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled constraint on a name is refused, not reported as a miss
// ---------------------------------------------------------------------------

/// A `TriggerNames` element outside the modelled `length 1..255` is refused at the request
/// boundary rather than reported in `TriggersNotFound` — a name the service cannot address
/// is a bad request, not a trigger that is absent — and the required member itself must be
/// present.
///
/// Source for the bound: `smithy_query(query="shape",shape="TriggerNameList")`, whose member
/// targets `NameString` with `length 1..255`. Both published bounds are exercised, plus one
/// past each.
#[tokio::test]
async fn a_name_outside_the_modeled_bounds_is_refused() {
    let router = emulator();
    given_job_in(&router, REGION).await;
    ok(&router, "CreateTrigger", &trigger_body("alpha")).await;

    // At each published bound the name is a legal address — so it is simply unknown, and
    // reported as a miss rather than refused.
    for length in [1usize, 255] {
        let name = "n".repeat(length);
        let body = ok(
            &router,
            "BatchGetTriggers",
            &json!({ "TriggerNames": [name.clone()] }),
        )
        .await;
        assert_eq!(
            string_list(&body, "TriggersNotFound"),
            vec![name.clone()],
            "case name of {length} characters: a legal but unknown name is a miss, got {body}"
        );
        assert!(
            trigger_names(&body).is_empty(),
            "case name of {length} characters: no trigger is stored under it, got {body}"
        );
    }

    // One past each bound is refused.
    for (case, names) in [
        ("an empty name", json!([""])),
        ("a name of 256 characters", json!(["n".repeat(256)])),
        (
            "a legal name beside an over-long one",
            json!(["alpha", "n".repeat(256)]),
        ),
    ] {
        let (status, answer) =
            call(&router, "BatchGetTriggers", &json!({ "TriggerNames": names })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            !error_code(&answer).is_empty(),
            "case {case:?}: the refusal must name a modeled error, got {answer}"
        );
    }

    // `TriggerNames` is `@required`, so a batch that names it not at all is refused too.
    let (status, answer) = call(&router, "BatchGetTriggers", &json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no TriggerNames member\": expected 400, got {status} with {answer}"
    );
    assert!(
        !error_code(&answer).is_empty(),
        "case \"no TriggerNames member\": the refusal must name a modeled error, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// The batch's own size bound
// ---------------------------------------------------------------------------

/// A batch is served up to `MAX_BATCH_TRIGGER_NAMES` names and refused past it — the answer
/// carries one whole trigger projection per listed name, repeats included, so the bound is
/// on the request's name COUNT, which the model leaves unconstrained.
///
/// The rows differ along the batch's size (one, a middling batch, the bound exactly, one
/// past it) and along what the names are: a batch at the bound of distinct stored triggers,
/// one of the same stored name repeated, and one of unknown names.
#[tokio::test]
async fn a_batch_past_the_name_ceiling_is_refused() {
    let router = emulator();
    given_job_in(&router, REGION).await;
    let stored: Vec<String> = (0..MAX_BATCH_TRIGGER_NAMES)
        .map(|index| format!("batched-{index:03}"))
        .collect();
    for name in &stored {
        ok(&router, "CreateTrigger", &trigger_body(name)).await;
    }

    // At and below the bound the batch is served, whatever its names are.
    for size in [1usize, 25, MAX_BATCH_TRIGGER_NAMES] {
        let distinct: Vec<&str> = stored.iter().take(size).map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &distinct).await;
        assert_eq!(
            found, distinct,
            "case {size} distinct stored names: expected them all reported, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} distinct stored names: none may miss, got {missing:?}"
        );

        let repeated: Vec<&str> = vec!["batched-000"; size];
        let (found, missing) = batch_get(&router, &repeated).await;
        assert_eq!(
            found.len(),
            size,
            "case {size} copies of one stored name: one entry per requested name, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} copies of one stored name: none may miss, got {missing:?}"
        );

        let unknown: Vec<String> = (0..size).map(|index| format!("absent-{index}")).collect();
        let unknown: Vec<&str> = unknown.iter().map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &unknown).await;
        assert!(
            found.is_empty(),
            "case {size} unknown names: nothing may be reported, got {found:?}"
        );
        assert_eq!(
            missing, unknown,
            "case {size} unknown names: all of them miss, got {missing:?}"
        );
    }

    // One past the bound is refused with the operation's modelled input error, and the
    // refusal does not depend on whether the names are stored.
    let over = MAX_BATCH_TRIGGER_NAMES + 1;
    let batches: Vec<(&str, Vec<String>)> = vec![
        (
            "one name too many, all stored",
            stored
                .iter()
                .cloned()
                .chain(std::iter::once("batched-000".to_string()))
                .collect(),
        ),
        (
            "one name too many, none stored",
            (0..over).map(|index| format!("absent-{index}")).collect(),
        ),
        (
            "one name too many, the same name repeated",
            vec!["batched-000".to_string(); over],
        ),
    ];
    for (case, names) in batches {
        assert_eq!(
            names.len(),
            over,
            "case {case:?}: the batch must be one past the bound"
        );
        let (status, answer) =
            call(&router, "BatchGetTriggers", &json!({ "TriggerNames": names })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {answer}"
        );
    }

    // The refusals changed nothing: a batch at the bound is still served.
    let distinct: Vec<&str> = stored.iter().map(String::as_str).collect();
    let (found, _) = batch_get(&router, &distinct).await;
    assert_eq!(
        found.len(),
        MAX_BATCH_TRIGGER_NAMES,
        "a batch at the bound is still served after a refused one, got {found:?}"
    );
}
