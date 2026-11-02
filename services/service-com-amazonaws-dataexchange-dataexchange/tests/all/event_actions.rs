// SPDX-License-Identifier: CC0-1.0

//! The event-action lifecycle: `CreateEventAction`, `GetEventAction`,
//! `UpdateEventAction`, `DeleteEventAction` and `ListEventActions`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `harness`, `call`, `create_data_set`, `is_modelled_id`,
//! `seconds`) is the one `data_sets.rs` already uses.
//!
//! Three rules are under test.
//!
//! 1. An event action is a stored resource: the create persists what it describes and
//!    every reader — `GetEventAction` and `ListEventActions` — reports that record,
//!    the update replaces its action, and the delete removes it.
//! 2. An event action exports a revision of a data set this account SUBSCRIBES to, so
//!    the data set its event names must exist (404 when it does not) and must carry
//!    `Origin` `ENTITLED` (400 when it carries the other value the model admits), and
//!    a refused create leaves nothing behind.
//! 3. An `EventActionId` that addresses no stored event action is the modelled
//!    `ResourceNotFoundException` (404) — on the read, the update and the delete
//!    alike.
//!
//! The rows span what the rules generalise over: every shape of action the model
//! admits (with and without a key pattern, with each `ServerSideEncryptionTypes`
//! value, with and without tags), both `Origin` values, several shapes of unknown id,
//! and page sizes that split the listing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{
    call, create_data_set, emulator, harness, is_modelled_id, seconds, Answer, ACCOUNT, REGION,
};

/// Well-formed but never created. The model declares `EventActionId` as a plain
/// `__string` (it carries no pattern), so this is simply an id nothing holds.
const NEVER_CREATED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Well-formed per the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` but naming no
/// data set.
const NO_SUCH_DATA_SET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The refusal AWS answers an event action on a data set this account owns with,
/// recorded in `dataexchange.event-action-requires-entitled-dataset-rejected` and
/// stated by `crate::state::event_action_requires_entitled_data_set`.
const ENTITLED_ONLY: &str =
    "Event actions are only compatible with Data Sets where the value for Origin is ENTITLED.";

/// Both values the model's `com.amazonaws.dataexchange#ServerSideEncryptionTypes`
/// enum admits. Source: `smithy_query(query="shape",
/// shape="ServerSideEncryptionTypes")`.
const ENCRYPTION_TYPES: &[&str] = &["aws:kms", "AES256"];

/// One shape of `Action` a request may carry, and how the case is named.
struct ActionCase {
    case: &'static str,
    action: Value,
    tags: Option<Value>,
}

/// The rows span the action shapes the model admits: a destination bucket alone, one
/// with a key pattern, and one under each `ServerSideEncryptionTypes` value — paired
/// with no tags, one tag and several.
fn action_cases() -> Vec<ActionCase> {
    let mut cases = vec![
        ActionCase {
            case: "bucket only, no tags",
            action: json!({
                "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
            }),
            tags: None,
        },
        ActionCase {
            case: "bucket and key pattern, one tag",
            action: json!({
                "ExportRevisionToS3": {
                    "RevisionDestination": {
                        "Bucket": "export-bucket",
                        "KeyPattern": "${Revision.CreatedAt}/${Asset.Name}",
                    }
                }
            }),
            tags: Some(json!({ "team": "data" })),
        },
    ];
    for encryption_type in ENCRYPTION_TYPES {
        let mut encryption = json!({ "Type": encryption_type });
        if *encryption_type == "aws:kms" {
            encryption["KmsKeyArn"] = json!(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/12345678-1234-1234-1234-123456789012"
            ));
        }
        cases.push(ActionCase {
            case: match *encryption_type {
                "aws:kms" => "kms-encrypted export, several tags",
                _ => "s3-managed-key export, several tags",
            },
            action: json!({
                "ExportRevisionToS3": {
                    "RevisionDestination": { "Bucket": "encrypted-bucket" },
                    "Encryption": encryption,
                }
            }),
            tags: Some(json!({ "team": "data", "tier": "gold" })),
        });
    }
    cases
}

/// The `Event` naming `data_set_id` as the published-revision source.
fn revision_published(data_set_id: &str) -> Value {
    json!({ "RevisionPublished": { "DataSetId": data_set_id } })
}

async fn create_event_action(
    router: &MultiServiceRouter,
    event: Value,
    action: Value,
    tags: Option<&Value>,
) -> Answer {
    let mut body = json!({ "Event": event, "Action": action });
    if let Some(tags) = tags {
        body["Tags"] = tags.clone();
    }
    call(router, "POST", "/v1/event-actions", Some(body)).await
}

async fn get_event_action(router: &MultiServiceRouter, event_action_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/event-actions/{event_action_id}"),
        None,
    )
    .await
}

async fn update_event_action(
    router: &MultiServiceRouter,
    event_action_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/v1/event-actions/{event_action_id}"),
        Some(body),
    )
    .await
}

async fn delete_event_action(router: &MultiServiceRouter, event_action_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/event-actions/{event_action_id}"),
        None,
    )
    .await
}

/// `ListEventActions`, optionally narrowed to one event source and one page size.
async fn list_event_actions(
    router: &MultiServiceRouter,
    event_source_id: Option<&str>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(source) = event_source_id {
        query.push(format!("eventSourceId={source}"));
    }
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        "/v1/event-actions".to_string()
    } else {
        format!("/v1/event-actions?{}", query.join("&"))
    };
    call(router, "GET", &uri, None).await
}

/// The ids a listing answered, in the order it answered them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .member("EventActions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["Id"].as_str().map(str::to_string))
        .collect()
}

/// The refusal message an answer carries, or the empty string when it carries none.
fn message(answer: &Answer) -> String {
    answer
        .member("Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Turn a data set into the entitled copy of someone else's — the state an event
/// action requires of the data set its event watches.
///
/// No operation this emulator implements mints an entitled data set (AWS does it by
/// accepting a data grant or an AWS Marketplace subscription, and `AcceptDataGrant` is
/// still a scaffold), so the test writes that state through the shared engine, the way
/// `data_set_notifications.rs` writes a data set's Marketplace product. It is also
/// what proves the handler's answer comes from the stored record rather than from the
/// request.
pub(crate) fn entitle_data_set(engine: &Arc<engine::InMemoryEngine>, data_set_id: &str) {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    let data_set = store
        .data_sets
        .get_mut(data_set_id)
        .expect("the data set the test just created");
    data_set.origin = dataexchange::state::ORIGIN_ENTITLED.to_string();
    data_set.origin_details = Some(dataexchange::ops::types::OriginDetails {
        product_id: None,
        data_grant_id: Some("dgrant7890123456789012345678901".to_string()),
    });
}

/// How many event actions state holds.
fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    assert_eq!(
        store.event_actions.len(),
        store.event_action_order.len(),
        "case store invariant: the record map and its order must hold the same ids"
    );
    store.event_actions.len()
}

/// An entitled data set, ready to be watched by an event action.
async fn entitled_data_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> String {
    let data_set_id = create_data_set(router, name, "e", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    entitle_data_set(engine, &data_set_id);
    data_set_id
}

/// RULE: the create persists the event action it describes and every reader reports
/// that record — for every shape of action the model admits.
#[tokio::test]
async fn create_then_read_round_trips_every_action_shape() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "event-action-reads").await;

    let mut created_ids: Vec<String> = Vec::new();
    for ActionCase { case, action, tags } in action_cases() {
        let created = create_event_action(
            &router,
            revision_published(&data_set_id),
            action.clone(),
            tags.as_ref(),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateEventAction, got {} body {:?}",
            created.status, created.body
        );
        let event_action_id = created.string("Id");
        assert!(
            is_modelled_id(&event_action_id),
            "case {case}: expected an Id matching ^[a-zA-Z0-9]{{30,40}}$, got {:?}",
            created.member("Id")
        );
        assert_eq!(
            created.string("Arn"),
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:event-actions/{event_action_id}"),
            "case {case}: expected the event action Arn to name itself, got {:?}",
            created.member("Arn")
        );
        assert_eq!(
            created.member("Action"),
            &action,
            "case {case}: expected the action as described, got {:?}",
            created.member("Action")
        );
        assert_eq!(
            created.member("Event"),
            &revision_published(&data_set_id),
            "case {case}: expected the event to name its data set, got {:?}",
            created.member("Event")
        );
        assert_eq!(
            created.member("Tags"),
            &tags.clone().unwrap_or(Value::Null),
            "case {case}: expected the tags as given, got {:?}",
            created.member("Tags")
        );
        assert_eq!(
            seconds(created.member("CreatedAt"), case),
            seconds(created.member("UpdatedAt"), case),
            "case {case}: expected CreatedAt == UpdatedAt on a record nothing has updated"
        );

        // Round trip: the single-resource reader reports what the create persisted.
        let fetched = get_event_action(&router, &event_action_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetEventAction, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body, created.body,
            "case {case}: expected GetEventAction to answer the created record, got {:?} \
             for {:?}",
            fetched.body, created.body
        );

        // And repeating the read changes nothing a later caller sees.
        let again = get_event_action(&router, &event_action_id).await;
        assert_eq!(
            again.body, fetched.body,
            "case {case}: expected the identical record on every read, got {:?} for {:?}",
            again.body, fetched.body
        );

        created_ids.push(event_action_id);
    }

    // The listing agrees with the single-resource reader, newest first.
    let listed = list_event_actions(&router, Some(&data_set_id), None, None).await;
    assert_eq!(
        listed.status, 200,
        "case listing: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    let expected: Vec<String> = created_ids.iter().rev().cloned().collect();
    assert_eq!(
        listed_ids(&listed),
        expected,
        "case listing: expected every created event action, newest first, got {:?}",
        listed_ids(&listed)
    );
    assert_eq!(
        stored_count(&engine),
        created_ids.len(),
        "case listing: expected state to hold exactly the created records"
    );
}

/// RULE: the data set an event watches must exist and must be an entitled copy, so a
/// create naming an owned one is a 400 and a create naming no data set at all is a
/// 404 — and neither leaves an event action behind.
#[tokio::test]
async fn a_create_is_refused_unless_its_data_set_is_entitled() {
    let (router, engine) = harness();
    let owned = create_data_set(&router, "owned-source", "o", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let entitled = entitled_data_set(&router, &engine, "entitled-source").await;
    let action = json!({
        "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
    });

    // Rows differ along WHY the create cannot hold: the data set this account owns
    // (both `Origin` values are covered, since the entitled one succeeds below), a
    // data set id naming nothing, and an event naming no source at all.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an owned data set",
            revision_published(&owned),
            400,
            "ValidationException",
        ),
        (
            "a data set id naming nothing",
            revision_published(NO_SUCH_DATA_SET),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an event naming no source",
            json!({}),
            400,
            "ValidationException",
        ),
    ];

    for (case, event, status, code) in rows {
        let before = stored_count(&engine);
        let refused = create_event_action(&router, event, action.clone(), None).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected {status}, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some(code),
            "case {case}: expected {code}, got {:?}",
            refused.error
        );
        assert_eq!(
            stored_count(&engine),
            before,
            "case {case}: expected a refused create to persist nothing"
        );
        for source in [owned.as_str(), NO_SUCH_DATA_SET] {
            let listed = list_event_actions(&router, Some(source), None, None).await;
            assert_eq!(
                listed_ids(&listed),
                Vec::<String>::new(),
                "case {case}: expected no event action for source {source}, got {:?}",
                listed_ids(&listed)
            );
        }
    }

    // The owned-data-set refusal states the constraint in AWS's own words.
    let refused =
        create_event_action(&router, revision_published(&owned), action.clone(), None).await;
    assert_eq!(
        message(&refused),
        ENTITLED_ONLY,
        "case owned data set wording: expected the recorded refusal, got {:?}",
        message(&refused)
    );

    // The same request against the entitled data set is accepted, which is what makes
    // the refusals above about `Origin` rather than about the request's shape.
    let created = create_event_action(&router, revision_published(&entitled), action, None).await;
    assert_eq!(
        created.status, 201,
        "case entitled data set: expected 201, got {} body {:?}",
        created.status, created.body
    );
    assert_eq!(
        listed_ids(&list_event_actions(&router, Some(&entitled), None, None).await),
        vec![created.string("Id")],
        "case entitled data set: expected the created event action under its source"
    );
}

/// RULE: an `Action` that names no action to perform describes nothing to do, so the
/// create and the update both refuse it as a 400 `ValidationException`.
#[tokio::test]
async fn an_action_that_names_nothing_is_refused() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "empty-action").await;
    let good = json!({
        "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
    });

    let refused =
        create_event_action(&router, revision_published(&data_set_id), json!({}), None).await;
    assert_eq!(
        refused.status, 400,
        "case create with an empty action: expected 400, got {} body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case create with an empty action: expected ValidationException, got {:?}",
        refused.error
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "case create with an empty action: expected nothing to be persisted"
    );

    let event_action_id = create_event_action(
        &router,
        revision_published(&data_set_id),
        good.clone(),
        None,
    )
    .await
    .string("Id");
    let refused = update_event_action(&router, &event_action_id, json!({ "Action": {} })).await;
    assert_eq!(
        refused.status, 400,
        "case update with an empty action: expected 400, got {} body {:?}",
        refused.status, refused.body
    );
    let unchanged = get_event_action(&router, &event_action_id).await;
    assert_eq!(
        unchanged.member("Action"),
        &good,
        "case update with an empty action: expected the stored action to survive, got {:?}",
        unchanged.member("Action")
    );
}

/// RULE: the update replaces the stored action and moves `UpdatedAt`, leaving the
/// event and the identity alone; an update carrying no action changes nothing.
#[tokio::test]
async fn an_update_replaces_the_action_it_carries() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "event-action-updates").await;
    let cases = action_cases();
    let created = create_event_action(
        &router,
        revision_published(&data_set_id),
        cases[0].action.clone(),
        Some(&json!({ "team": "data" })),
    )
    .await;
    let event_action_id = created.string("Id");

    // Rows walk every OTHER action shape in the catalog through the update, so each is
    // exercised as a replacement as well as at create.
    for ActionCase { case, action, .. } in cases.iter().skip(1) {
        let before = get_event_action(&router, &event_action_id).await;
        let updated =
            update_event_action(&router, &event_action_id, json!({ "Action": action })).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateEventAction, got {} body {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.member("Action"),
            action,
            "case {case}: expected the replacement action, got {:?}",
            updated.member("Action")
        );
        assert_eq!(
            updated.member("Event"),
            before.member("Event"),
            "case {case}: expected the event to be untouched, got {:?}",
            updated.member("Event")
        );
        assert_eq!(
            updated.string("Id"),
            event_action_id,
            "case {case}: expected the same identity, got {:?}",
            updated.member("Id")
        );
        assert_eq!(
            seconds(updated.member("CreatedAt"), case),
            seconds(before.member("CreatedAt"), case),
            "case {case}: expected CreatedAt to stay where the create left it"
        );
        assert!(
            seconds(updated.member("UpdatedAt"), case) >= seconds(before.member("UpdatedAt"), case),
            "case {case}: expected UpdatedAt to move forward, got {} for {}",
            seconds(updated.member("UpdatedAt"), case),
            seconds(before.member("UpdatedAt"), case)
        );

        // Round trip: the reader reports what the update persisted, tags and all.
        let fetched = get_event_action(&router, &event_action_id).await;
        assert_eq!(
            fetched.member("Action"),
            action,
            "case {case}: expected GetEventAction to report the replaced action, got {:?}",
            fetched.member("Action")
        );
        assert_eq!(
            fetched.member("Tags"),
            &json!({ "team": "data" }),
            "case {case}: expected the tags to survive an action update, got {:?}",
            fetched.member("Tags")
        );
    }

    // An update carrying no action asks for no change.
    let before = get_event_action(&router, &event_action_id).await;
    let unchanged = update_event_action(&router, &event_action_id, json!({})).await;
    assert_eq!(
        unchanged.status, 200,
        "case update with no action: expected 200, got {} body {:?}",
        unchanged.status, unchanged.body
    );
    assert_eq!(
        seconds(unchanged.member("UpdatedAt"), "update with no action"),
        seconds(before.member("UpdatedAt"), "update with no action"),
        "case update with no action: expected UpdatedAt to stay put"
    );
    assert_eq!(
        unchanged.member("Action"),
        before.member("Action"),
        "case update with no action: expected the stored action to survive, got {:?}",
        unchanged.member("Action")
    );
}

/// RULE: the delete removes the event action — 204, then no reader finds it — and a
/// second delete of the same id is the same 404 a never-created id answers with.
#[tokio::test]
async fn a_delete_removes_the_event_action() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "event-action-deletes").await;
    let action = json!({
        "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
    });

    // Rows differ along how many event actions share the source, so the delete is
    // exercised where records live side by side as well as alone.
    let mut ids: Vec<String> = Vec::new();
    for index in 0..3 {
        ids.push(
            create_event_action(
                &router,
                revision_published(&data_set_id),
                action.clone(),
                None,
            )
            .await
            .string("Id"),
        );
        assert_eq!(
            stored_count(&engine),
            index + 1,
            "case create {index}: expected the store to grow by one"
        );
    }

    for (index, event_action_id) in ids.iter().enumerate() {
        let case = format!("delete {index}");
        let deleted = delete_event_action(&router, event_action_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204, got {} body {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            stored_count(&engine),
            ids.len() - index - 1,
            "case {case}: expected the store to shrink by one"
        );

        let fetched = get_event_action(&router, event_action_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected the reader to answer 404 after the delete, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert!(
            !listed_ids(&list_event_actions(&router, Some(&data_set_id), None, None).await)
                .contains(event_action_id),
            "case {case}: expected the listing to drop the deleted event action"
        );

        let again = delete_event_action(&router, event_action_id).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected a second delete to answer 404, got {} body {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            again.error
        );
    }
}

/// RULE: an `EventActionId` that addresses no stored event action is a 404
/// `ResourceNotFoundException` naming the absent resource — on the read, the update
/// and the delete alike, whatever the id looks like.
#[tokio::test]
async fn an_absent_event_action_is_a_404_on_every_operation() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "absent-event-actions").await;
    let action = json!({
        "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "any-bucket" } }
    });
    // One event action DOES exist, so a 404 below is about the id rather than about an
    // empty store.
    let live = create_event_action(
        &router,
        revision_published(&data_set_id),
        action.clone(),
        None,
    )
    .await
    .string("Id");

    // Rows differ along the shape of the absent id: the recorded 38-character one, a
    // short one, one carrying characters the `Id` pattern would refuse (the model
    // declares this member as a plain `__string`), and a data set id — which names a
    // resource of another kind.
    let ids: Vec<(&str, String)> = vec![
        ("thirty-eight characters", NEVER_CREATED.to_string()),
        ("a short id", "nope".to_string()),
        ("an id with a hyphen", "event-action-0001".to_string()),
        ("a data set id", data_set_id.clone()),
    ];

    for (case, event_action_id) in &ids {
        for (operation, answer) in [
            ("GetEventAction", get_event_action(&router, event_action_id).await),
            (
                "UpdateEventAction",
                update_event_action(&router, event_action_id, json!({ "Action": action })).await,
            ),
            (
                "DeleteEventAction",
                delete_event_action(&router, event_action_id).await,
            ),
        ] {
            let label = format!("{operation} on {case}");
            assert_eq!(
                answer.status, 404,
                "case {label}: expected 404, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label}: expected ResourceNotFoundException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.member("ResourceType"),
                &json!("EVENT_ACTION"),
                "case {label}: expected the 404 to name the absent resource's type, got {:?}",
                answer.member("ResourceType")
            );
        }
    }

    // The one that does exist is untouched by all of that.
    let fetched = get_event_action(&router, &live).await;
    assert_eq!(
        fetched.status, 200,
        "case the live event action: expected 200, got {} body {:?}",
        fetched.status, fetched.body
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case the live event action: expected the store to still hold exactly it"
    );
}

/// RULE: `ListEventActions` reports the stored event actions, narrowed to one source
/// when the request names one, and pages them under the size the request asks for.
#[tokio::test]
async fn the_listing_filters_by_source_and_pages() {
    let (router, engine) = harness();
    let action = json!({
        "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
    });

    // Rows differ along how many event actions each source holds: zero, one, three.
    let mut sources: Vec<(String, Vec<String>)> = Vec::new();
    for (index, count) in [0usize, 1, 3].into_iter().enumerate() {
        let data_set_id = entitled_data_set(&router, &engine, &format!("source-{index}")).await;
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(
                create_event_action(
                    &router,
                    revision_published(&data_set_id),
                    action.clone(),
                    None,
                )
                .await
                .string("Id"),
            );
        }
        sources.push((data_set_id, ids));
    }

    for (index, (data_set_id, ids)) in sources.iter().enumerate() {
        let case = format!("source {index} holding {} event actions", ids.len());
        let listed = list_event_actions(&router, Some(data_set_id), None, None).await;
        let expected: Vec<String> = ids.iter().rev().cloned().collect();
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case}: expected this source's own event actions, newest first, got {:?}",
            listed_ids(&listed)
        );
    }

    // An unnarrowed listing reports every one of them.
    let all: Vec<String> = sources
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .rev()
        .collect();
    let listed = list_event_actions(&router, None, None, None).await;
    assert_eq!(
        listed_ids(&listed),
        all,
        "case every source: expected every stored event action, newest first, got {:?}",
        listed_ids(&listed)
    );
    assert_eq!(
        listed.member("NextToken"),
        &Value::Null,
        "case every source: expected no NextToken when one page exhausted the collection"
    );

    // Rows differ along the page size: each one splits the collection differently, and
    // following the tokens visits every record exactly once, in the same order.
    for page_size in [1i32, 2, 3, 4] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page =
                list_event_actions(&router, None, Some(page_size), token.as_deref()).await;
            assert_eq!(
                page.status, 200,
                "case {case}: expected 200, got {} body {:?}",
                page.status, page.body
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size as usize,
                "case {case}: expected at most {page_size} entries, got {}",
                ids.len()
            );
            seen.extend(ids);
            token = page
                .member("NextToken")
                .as_str()
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case}: expected the pages to walk every event action once, got {seen:?}"
        );
    }
}

/// RULE: `ListEventActions` is a read — a source naming nothing selects nothing
/// rather than failing, and the listing leaves the store where it was.
#[tokio::test]
async fn the_listing_changes_nothing_and_never_404s_on_its_filter() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "stable-listing").await;
    create_event_action(
        &router,
        revision_published(&data_set_id),
        json!({ "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "b" } } }),
        None,
    )
    .await;

    // Rows differ along what the filter names: a source nothing watches, an id naming
    // no data set at all, and the source that does hold one.
    let rows: Vec<(&str, String, usize)> = vec![
        ("a data set id naming nothing", NO_SUCH_DATA_SET.to_string(), 0),
        ("an id of another form", "not-a-data-set".to_string(), 0),
        ("the source that holds one", data_set_id.clone(), 1),
    ];

    let before = stored_count(&engine);
    for (case, source, expected) in rows {
        for round in 1..=2 {
            let listed = list_event_actions(&router, Some(&source), None, None).await;
            assert_eq!(
                listed.status, 200,
                "case {case} (read {round}): expected 200, got {} body {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                listed_ids(&listed).len(),
                expected,
                "case {case} (read {round}): expected {expected} entries, got {:?}",
                listed_ids(&listed)
            );
        }
    }
    assert_eq!(
        stored_count(&engine),
        before,
        "case reads: expected the listing to change nothing in state"
    );
}

/// RULE: an event action is a taggable resource of this service, so the tagging
/// surface addresses it by the ARN the create minted — the tags the create carried
/// are the tags the tag reader reports.
#[tokio::test]
async fn the_tag_reader_finds_an_event_action_by_its_arn() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "tagged-event-actions").await;

    // Rows differ along the tag map the create carried: none, one, several.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no tags", None),
        ("one tag", Some(json!({ "team": "data" }))),
        (
            "several tags",
            Some(json!({ "team": "data", "tier": "gold", "owner": "ops" })),
        ),
    ];

    for (case, tags) in rows {
        let created = create_event_action(
            &router,
            revision_published(&data_set_id),
            json!({ "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "b" } } }),
            tags.as_ref(),
        )
        .await;
        let arn = created.string("Arn");
        // The whole ARN is ONE `@httpLabel` segment, so its `:` and `/` travel encoded.
        let encoded = arn.replace('%', "%25").replace(':', "%3A").replace('/', "%2F");
        let listed = call(&router, "GET", &format!("/tags/{encoded}"), None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from ListTagsForResource, got {} body {:?}",
            listed.status, listed.body
        );
        // `ListTagsForResource`'s output member carries `@jsonName("tags")`.
        assert_eq!(
            listed.member("tags"),
            &tags.unwrap_or(json!({})),
            "case {case}: expected the tags the create carried, got {:?}",
            listed.member("tags")
        );
    }
}

/// RULE: a page size outside the modelled `@range(min: 1, max: 200)` is refused as a
/// 400 `ValidationException`; a value at either published bound is accepted.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = emulator();

    // Rows: each published bound, and one past it on each side.
    let rows: Vec<(&str, i32, u16)> = vec![
        ("one below the minimum", 0, 400),
        ("at the minimum", 1, 200),
        ("at the maximum", 200, 200),
        ("one past the maximum", 201, 400),
    ];

    for (case, max_results, status) in rows {
        let answer = list_event_actions(&router, None, Some(max_results), None).await;
        assert_eq!(
            answer.status, status,
            "case {case} (maxResults={max_results}): expected {status}, got {} body {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {:?}",
                answer.error
            );
        }
    }
}

/// RULE: a `NextToken` this service never issued is a 400 `ValidationException`, not a
/// silently different page.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let router = emulator();

    // Rows differ along how the token is wrong: not hex at all, hex that decodes to
    // nothing this service encodes, and a token longer than one it can issue.
    let tokens = [
        ("not hex", "zzzz".to_string()),
        ("hex of a foreign encoding", hex::encode("9|999999999999|0|x")),
        ("far too long", "ab".repeat(200)),
    ];

    for (case, token) in tokens {
        let answer = list_event_actions(&router, None, None, Some(&token)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            answer.error
        );
    }
}
