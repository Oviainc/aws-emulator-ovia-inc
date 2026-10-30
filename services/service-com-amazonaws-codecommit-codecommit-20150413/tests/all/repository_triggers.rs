// SPDX-License-Identifier: CC0-1.0

//! A repository's trigger configuration over the wire: `PutRepositoryTriggers` and
//! `GetRepositoryTriggers`, driven as awsJson1_1 requests through the router that
//! serves the real emulator.
//!
//! The rule under test is that a put REPLACES the repository's whole trigger
//! configuration and a get reports exactly what the last put stored — every member of
//! every trigger, under the `configurationId` that write minted — while a
//! configuration the repository cannot hold is refused with the error the model
//! declares for the member at fault and leaves the stored configuration alone. The
//! tables below span that class (trigger counts, event sets, branch scopes, custom
//! data, name and destination forms, both published bounds and one past each) rather
//! than one recorded request.

use serde_json::{json, Value};
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    MAX_BRANCHES_PER_TRIGGER, MAX_TRIGGERS_PER_REPOSITORY, MAX_TRIGGER_CUSTOM_DATA_CHARACTERS,
    MAX_TRIGGER_NAME_CHARACTERS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, error, is_uuid, ok, string_at, REGION};

/// Every value `RepositoryTriggerEventEnum` declares
/// (`smithy_query(query="shape",shape="RepositoryTriggerEventEnum")`), which is the
/// catalog a trigger's `events` is drawn from.
const EVERY_EVENT: &[&str] = &["all", "updateReference", "createReference", "deleteReference"];

/// The sentence `InvalidRepositoryTriggerEventsException` carries for a list that
/// names `all` beside another event type — fixed text, ending without a period, as
/// the service writes it.
const ALL_COMBINED_MESSAGE: &str =
    "Repository trigger events cannot contain 'all' and additional event types simultaneously";

/// The sentence the same code carries for an event name the service does not
/// recognise: the other reason a list is refused, worded for the NAME rather than for
/// the combination.
const UNRECOGNISED_EVENT_MESSAGE: &str =
    "One or more events specified for the trigger is not valid. Check to make sure that all \
     events specified match the requirements for allowed events.";

/// The ARN of an Amazon SNS topic in `region` — a destination a repository trigger
/// may target.
fn topic_arn(region: &str, topic: &str) -> String {
    format!(
        "arn:aws:sns:{region}:{}:{topic}",
        engine::emulator_account_id()
    )
}

/// The ARN of a Lambda function in `region`, the other destination a trigger may
/// target. Its resource portion carries a further colon, so it also covers an ARN
/// whose last field is multi-segment.
fn function_arn(region: &str, function: &str) -> String {
    format!(
        "arn:aws:lambda:{region}:{}:function:{function}",
        engine::emulator_account_id()
    )
}

/// A trigger carrying only the three members the model requires.
fn trigger(name: &str, destination: &str, events: &[&str]) -> Value {
    json!({ "name": name, "destinationArn": destination, "events": events })
}

/// Create `repository`, so a configuration can be written for it.
async fn seed(router: &MultiServiceRouter, repository: &str) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
}

/// Store `triggers` on `repository` and answer the `configurationId` the write
/// minted, asserting it is the form this service mints an id as.
async fn put(router: &MultiServiceRouter, repository: &str, triggers: &Value) -> String {
    let written = ok(
        router,
        "PutRepositoryTriggers",
        &json!({ "repositoryName": repository, "triggers": triggers }),
    )
    .await;
    let configuration_id = string_at(&written, "/configurationId");
    assert!(
        is_uuid(&configuration_id),
        "a put must mint a configurationId, got {configuration_id:?} in {written}"
    );
    configuration_id
}

/// The configuration `repository` holds: the id and the triggers a get reports.
async fn read(router: &MultiServiceRouter, repository: &str) -> (Option<String>, Value) {
    let got = ok(
        router,
        "GetRepositoryTriggers",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let configuration_id = got
        .get("configurationId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let triggers = got
        .get("triggers")
        .cloned()
        .unwrap_or_else(|| panic!("no triggers in {got}"));
    (configuration_id, triggers)
}

#[tokio::test]
async fn a_put_replaces_the_whole_configuration_and_a_get_reports_it() {
    let router = emulator();
    seed(&router, "trigger-roundtrip").await;

    // A repository nobody has configured holds no trigger and no configuration id:
    // nothing has been written for one to identify.
    let (id, triggers) = read(&router, "trigger-roundtrip").await;
    assert_eq!(
        (id.as_deref(), &triggers),
        (None, &json!([])),
        "an unconfigured repository must report no trigger and no configurationId"
    );

    // (case, the configuration as sent). Each row is written over the one before it,
    // so every row also checks that a put REPLACES rather than merges: what the row
    // sends is the whole configuration afterwards.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one trigger on every event, scoped to no branch",
            json!([trigger(
                "all-events",
                &topic_arn(REGION, "releases"),
                &["all"]
            )]),
        ),
        (
            "one trigger on two events, scoped to two branches, with custom data",
            json!([{
                "name": "ref-changes",
                "destinationArn": topic_arn(REGION, "ref-changes"),
                "events": ["createReference", "deleteReference"],
                "branches": ["main", "release/2026"],
                "customData": "team=platform",
            }]),
        ),
        (
            "two triggers, one per destination kind, with an empty branch list",
            json!([
                {
                    "name": "to-a-topic",
                    "destinationArn": topic_arn(REGION, "pushes"),
                    "events": ["updateReference"],
                    "branches": [],
                },
                {
                    "name": "to-a-function",
                    "destinationArn": function_arn(REGION, "on-push"),
                    "events": ["all"],
                    "customData": "{\"pipeline\":\"build\"}",
                },
            ]),
        ),
        (
            "the published maximum number of triggers",
            Value::Array(
                (0..MAX_TRIGGERS_PER_REPOSITORY)
                    .map(|index| {
                        trigger(
                            &format!("filling-{index:02}"),
                            &topic_arn(REGION, "filling"),
                            &["all"],
                        )
                    })
                    .collect(),
            ),
        ),
        (
            "no trigger at all, which is how every trigger is deleted",
            json!([]),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, sent) in rows {
        let configuration_id = put(&router, "trigger-roundtrip", &sent).await;
        let (read_id, read_triggers) = read(&router, "trigger-roundtrip").await;
        assert_eq!(
            read_id.as_deref(),
            Some(configuration_id.as_str()),
            "case {case:?}: a get must report the id the put minted, got {read_id:?}"
        );
        assert_eq!(
            read_triggers, sent,
            "case {case:?}: a get must report exactly the configuration stored, got \
             {read_triggers}"
        );
        assert!(
            !minted.contains(&configuration_id),
            "case {case:?}: each write identifies itself, but {configuration_id} was minted \
             twice ({minted:?})"
        );
        minted.push(configuration_id);
    }
}

#[tokio::test]
async fn every_modelled_event_is_accepted_and_all_stands_alone() {
    let router = emulator();
    seed(&router, "trigger-events").await;

    // Each value the model's enum declares, one per row: a trigger on that single
    // event is stored and read back.
    for event in EVERY_EVENT {
        let sent = json!([trigger(
            "one-event",
            &topic_arn(REGION, "events"),
            &[event]
        )]);
        put(&router, "trigger-events", &sent).await;
        let (_, read_triggers) = read(&router, "trigger-events").await;
        assert_eq!(
            read_triggers, sent,
            "case {event:?}: a trigger on a modelled event must be stored, got {read_triggers}"
        );
    }

    // Every non-`all` value is also accepted BESIDE another one, so the refusal
    // below is about `all` rather than about a trigger naming two events.
    let combined: Vec<&str> = EVERY_EVENT
        .iter()
        .copied()
        .filter(|event| *event != "all")
        .collect();
    let sent = json!([trigger(
        "several-events",
        &topic_arn(REGION, "events"),
        &combined
    )]);
    put(&router, "trigger-events", &sent).await;
    let (_, read_triggers) = read(&router, "trigger-events").await;
    assert_eq!(
        read_triggers, sent,
        "every modelled event other than all must combine, got {read_triggers}"
    );

    // "The valid value \"all\" cannot be used with any other values." The refusal is
    // worded for the COMBINATION rather than for an event the service does not
    // recognise, which is the sentence the sibling test below pins.
    for other in &combined {
        let (code, message) = error_with_message(
            &router,
            "PutRepositoryTriggers",
            &json!({
                "repositoryName": "trigger-events",
                "triggers": [trigger("all-and-more", &topic_arn(REGION, "events"), &["all", other])],
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidRepositoryTriggerEventsException",
            "case {other:?}: all beside another event must be refused, got {code}: {message}"
        );
        assert_eq!(
            message, ALL_COMBINED_MESSAGE,
            "case {other:?}: expected the combination sentence, got {message:?}"
        );
    }

    // And the configuration that was already stored is still the one it holds.
    let (_, read_triggers) = read(&router, "trigger-events").await;
    assert_eq!(
        read_triggers, sent,
        "a refused put must leave the stored configuration alone, got {read_triggers}"
    );
}

/// The two reasons an `events` list is refused carry the SAME modelled code and
/// DIFFERENT sentences, so a caller can tell an illegal combination from an event
/// name the service does not recognise.
///
/// The rows below span what that rule generalises over: where `all` sits in the list
/// and how many other events stand beside it (before, after, among two others, among
/// every other value) for the combination sentence, and why a name is not an event
/// (a value outside the enum, one differing only in case, one differing only in
/// spacing, and the same value named twice) for the generic one. The accepted rows
/// are `all` alone and each other value alone, which is what makes these refusals
/// about the list rather than about the trigger.
#[tokio::test]
async fn the_two_reasons_an_events_list_is_refused_are_worded_apart() {
    let router = emulator();
    seed(&router, "trigger-event-wording").await;
    // The configuration a refusal must leave in place.
    let stored = json!([trigger(
        "already-stored",
        &topic_arn(REGION, "events"),
        &["all"]
    )]);
    put(&router, "trigger-event-wording", &stored).await;

    let combination: Vec<(&str, Vec<&str>)> = vec![
        ("all before another event", vec!["all", "updateReference"]),
        ("all after another event", vec!["createReference", "all"]),
        (
            "all among two other events",
            vec!["createReference", "all", "deleteReference"],
        ),
        ("all among every other event", EVERY_EVENT.to_vec()),
    ];
    let unrecognised: Vec<(&str, Vec<&str>)> = vec![
        ("an event outside the enum", vec!["pushReference"]),
        (
            "an event differing only in case",
            vec!["UpdateReference"],
        ),
        (
            "an event differing only in spacing",
            vec![" updateReference"],
        ),
        (
            "a recognised event named twice",
            vec!["createReference", "createReference"],
        ),
        (
            "an unrecognised event beside a recognised one",
            vec!["createReference", "pushReference"],
        ),
        (
            "all beside the same event twice: a list naming one event twice names\
             nothing new, which is refused for the name rather than for the\
             combination",
            vec!["all", "updateReference", "updateReference"],
        ),
    ];

    for (rows, expected, label) in [
        (combination, ALL_COMBINED_MESSAGE, "the combination"),
        (unrecognised, UNRECOGNISED_EVENT_MESSAGE, "the event name"),
    ] {
        for (case, events) in rows {
            let (code, message) = error_with_message(
                &router,
                "PutRepositoryTriggers",
                &json!({
                    "repositoryName": "trigger-event-wording",
                    "triggers": [trigger("bad-trigger", &topic_arn(REGION, "events"), &events)],
                }),
                400,
            )
            .await;
            assert_eq!(
                code, "InvalidRepositoryTriggerEventsException",
                "case {case:?}: expected the modelled events error, got {code}: {message}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the sentence for {label}, got {message:?}"
            );
            // Neither sentence interpolates anything: not the trigger, not the
            // repository, not the offending value.
            for named in events
                .iter()
                .copied()
                .chain(["bad-trigger", "trigger-event-wording"])
            {
                let named = named.trim();
                if named == "all" {
                    // The combination sentence names the one value that cannot be
                    // combined, which is part of its fixed text.
                    continue;
                }
                assert!(
                    !message.contains(named),
                    "case {case:?}: the message must not carry {named:?}, got {message:?}"
                );
            }
        }
    }

    // Every refused put left the stored configuration alone, and the list it holds is
    // still accepted on its own.
    let (_, read_triggers) = read(&router, "trigger-event-wording").await;
    assert_eq!(
        read_triggers, stored,
        "a refused put must leave the stored configuration alone, got {read_triggers}"
    );
}

#[tokio::test]
async fn a_configuration_the_repository_cannot_hold_is_refused() {
    let router = emulator();
    seed(&router, "trigger-refusals").await;
    // The configuration a refusal must leave in place.
    let stored = json!([trigger(
        "already-stored",
        &topic_arn(REGION, "stored"),
        &["all"]
    )]);
    let stored_id = put(&router, "trigger-refusals", &stored).await;

    let long_name = "n".repeat(MAX_TRIGGER_NAME_CHARACTERS + 1);
    let long_custom_data = "d".repeat(MAX_TRIGGER_CUSTOM_DATA_CHARACTERS + 1);
    let many_branches: Vec<String> = (0..=MAX_BRANCHES_PER_TRIGGER)
        .map(|index| format!("branch/{index:02}"))
        .collect();
    let many_triggers: Vec<Value> = (0..=MAX_TRIGGERS_PER_REPOSITORY)
        .map(|index| {
            trigger(
                &format!("too-many-{index:02}"),
                &topic_arn(REGION, "too-many"),
                &["all"],
            )
        })
        .collect();

    // (case, the `triggers` value as sent, the modelled error it must be refused with)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a trigger carrying no name member",
            json!([{ "destinationArn": topic_arn(REGION, "t"), "events": ["all"] }]),
            "RepositoryTriggerNameRequiredException",
        ),
        (
            "a trigger whose name is empty",
            json!([trigger("", &topic_arn(REGION, "t"), &["all"])]),
            "RepositoryTriggerNameRequiredException",
        ),
        (
            "a trigger whose name is blank",
            json!([trigger("   ", &topic_arn(REGION, "t"), &["all"])]),
            "RepositoryTriggerNameRequiredException",
        ),
        (
            "a name one character past the bound",
            json!([trigger(&long_name, &topic_arn(REGION, "t"), &["all"])]),
            "InvalidRepositoryTriggerNameException",
        ),
        (
            "two triggers sharing one name",
            json!([
                trigger("same-name", &topic_arn(REGION, "t"), &["all"]),
                trigger("same-name", &function_arn(REGION, "f"), &["all"]),
            ]),
            "InvalidRepositoryTriggerNameException",
        ),
        (
            "a trigger carrying no destinationArn member",
            json!([{ "name": "no-destination", "events": ["all"] }]),
            "RepositoryTriggerDestinationArnRequiredException",
        ),
        (
            "a trigger whose destination is empty",
            json!([trigger("empty-destination", "", &["all"])]),
            "RepositoryTriggerDestinationArnRequiredException",
        ),
        (
            "a destination that is not an ARN at all",
            json!([trigger("not-an-arn", "releases", &["all"])]),
            "InvalidRepositoryTriggerDestinationArnException",
        ),
        (
            "a destination ARN of a service a trigger cannot run",
            json!([trigger(
                "wrong-service",
                &format!("arn:aws:s3:::{}-bucket", engine::emulator_account_id()),
                &["all"]
            )]),
            "InvalidRepositoryTriggerDestinationArnException",
        ),
        (
            "a destination ARN whose account is not an account id",
            json!([trigger(
                "bad-account",
                "arn:aws:sns:us-west-2:not-an-account:releases",
                &["all"]
            )]),
            "InvalidRepositoryTriggerDestinationArnException",
        ),
        (
            "a destination in another Region than the repository",
            json!([trigger(
                "other-region",
                &topic_arn("eu-central-1", "releases"),
                &["all"]
            )]),
            "InvalidRepositoryTriggerRegionException",
        ),
        (
            "a trigger carrying no events member",
            json!([{ "name": "no-events", "destinationArn": topic_arn(REGION, "t") }]),
            "RepositoryTriggerEventsListRequiredException",
        ),
        (
            "a trigger naming no event",
            json!([trigger("empty-events", &topic_arn(REGION, "t"), &[])]),
            "RepositoryTriggerEventsListRequiredException",
        ),
        (
            "an event outside the modelled enum",
            json!([trigger(
                "unknown-event",
                &topic_arn(REGION, "t"),
                &["pushReference"]
            )]),
            "InvalidRepositoryTriggerEventsException",
        ),
        (
            "an event differing only in case from a modelled one",
            json!([trigger(
                "miscased-event",
                &topic_arn(REGION, "t"),
                &["CreateReference"]
            )]),
            "InvalidRepositoryTriggerEventsException",
        ),
        (
            "the same event named twice",
            json!([trigger(
                "repeated-event",
                &topic_arn(REGION, "t"),
                &["createReference", "createReference"]
            )]),
            "InvalidRepositoryTriggerEventsException",
        ),
        (
            "one branch past the bound for a single trigger",
            json!([{
                "name": "too-many-branches",
                "destinationArn": topic_arn(REGION, "t"),
                "events": ["all"],
                "branches": many_branches,
            }]),
            "MaximumBranchesExceededException",
        ),
        (
            "a branch name no repository can hold",
            json!([{
                "name": "bad-branch",
                "destinationArn": topic_arn(REGION, "t"),
                "events": ["all"],
                "branches": ["not a branch"],
            }]),
            "InvalidRepositoryTriggerBranchNameException",
        ),
        (
            "custom data one character past the bound",
            json!([{
                "name": "too-much-data",
                "destinationArn": topic_arn(REGION, "t"),
                "events": ["all"],
                "customData": long_custom_data,
            }]),
            "InvalidRepositoryTriggerCustomDataException",
        ),
        (
            "one trigger past the bound for a repository",
            Value::Array(many_triggers),
            "MaximumRepositoryTriggersExceededException",
        ),
    ];

    for (case, triggers, expected) in rows {
        let (code, message) = error_with_message(
            &router,
            "PutRepositoryTriggers",
            &json!({ "repositoryName": "trigger-refusals", "triggers": triggers }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}: {message}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: {code} must say what is wrong, got an empty message"
        );
    }

    // A request carrying no trigger list at all is the list's own requirement.
    let (code, message) = error_with_message(
        &router,
        "PutRepositoryTriggers",
        &json!({ "repositoryName": "trigger-refusals" }),
        400,
    )
    .await;
    assert_eq!(
        code, "RepositoryTriggersListRequiredException",
        "a put carrying no triggers member must be refused for the list, got {code}: {message}"
    );

    // Not one of those refusals touched the configuration the repository holds.
    let (read_id, read_triggers) = read(&router, "trigger-refusals").await;
    assert_eq!(
        (read_id.as_deref(), &read_triggers),
        (Some(stored_id.as_str()), &stored),
        "a refused put must leave the stored configuration alone"
    );
}

#[tokio::test]
async fn the_bounds_are_the_published_ones_and_per_subject() {
    let router = emulator();
    seed(&router, "trigger-bounds").await;

    // Each bound AT its published value: a name of exactly the bound, custom data of
    // exactly the bound, and a trigger naming exactly as many branches as one may.
    let at_the_bound = json!([{
        "name": "b".repeat(MAX_TRIGGER_NAME_CHARACTERS),
        "destinationArn": topic_arn(REGION, "bounds"),
        "events": ["all"],
        "branches": (0..MAX_BRANCHES_PER_TRIGGER)
            .map(|index| format!("branch/{index:02}"))
            .collect::<Vec<String>>(),
        "customData": "c".repeat(MAX_TRIGGER_CUSTOM_DATA_CHARACTERS),
    }]);
    put(&router, "trigger-bounds", &at_the_bound).await;
    let (_, read_triggers) = read(&router, "trigger-bounds").await;
    assert_eq!(
        read_triggers, at_the_bound,
        "a configuration at every bound must be stored, got {read_triggers}"
    );

    // The trigger bound is what ONE repository holds: another repository has its own
    // configuration, and a configuration on one is not reported for the other.
    seed(&router, "trigger-bounds-other").await;
    let other = json!([trigger(
        "elsewhere",
        &function_arn(REGION, "elsewhere"),
        &["createReference"]
    )]);
    put(&router, "trigger-bounds-other", &other).await;
    let (_, read_other) = read(&router, "trigger-bounds-other").await;
    assert_eq!(
        read_other, other,
        "each repository holds its own configuration, got {read_other}"
    );
    let (_, read_triggers) = read(&router, "trigger-bounds").await;
    assert_eq!(
        read_triggers, at_the_bound,
        "writing one repository's configuration must not touch another's, got {read_triggers}"
    );
}

#[tokio::test]
async fn a_repository_the_store_does_not_hold_is_refused() {
    let router = emulator();
    seed(&router, "trigger-gone").await;
    put(
        &router,
        "trigger-gone",
        &json!([trigger(
            "doomed",
            &topic_arn(REGION, "doomed"),
            &["all"]
        )]),
    )
    .await;
    ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "trigger-gone" }),
    )
    .await;

    // (case, repositoryName): a name no repository ever had, and the name of one that
    // was deleted — the refusal follows the store, not the shape of the name. Both
    // operations answer it, and a deleted repository's configuration is gone with it.
    let rows: Vec<(&str, &str)> = vec![
        ("a name no repository ever had", "never-created"),
        ("the name of a deleted repository", "trigger-gone"),
    ];
    for (case, repository) in rows {
        for operation in ["PutRepositoryTriggers", "GetRepositoryTriggers"] {
            let code = error(
                &router,
                operation,
                &json!({
                    "repositoryName": repository,
                    "triggers": [trigger("t", &topic_arn(REGION, "t"), &["all"])],
                }),
                400,
            )
            .await;
            assert_eq!(
                code, "RepositoryDoesNotExistException",
                "case {case:?} on {operation}: expected RepositoryDoesNotExistException, got \
                 {code}"
            );
        }
    }

    // A repository created again under a deleted repository's name starts with no
    // configuration: the triggers went with the repository.
    seed(&router, "trigger-gone").await;
    let (id, triggers) = read(&router, "trigger-gone").await;
    assert_eq!(
        (id.as_deref(), &triggers),
        (None, &json!([])),
        "a recreated repository must hold no trigger configuration"
    );
}
