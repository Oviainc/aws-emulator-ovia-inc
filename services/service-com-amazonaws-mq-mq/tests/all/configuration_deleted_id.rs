// SPDX-License-Identifier: CC0-1.0

//! How a `ConfigurationId` this service ISSUED and has since deleted is answered,
//! driven over the wire through the same dispatch path the front door uses.
//!
//! RULE: a write that would make a new revision of a configuration whose id this
//! service issued and has since deleted is refused as a conflict with that deleted
//! state — HTTP 409, the modelled `ConflictException`, the sentence
//! `Concurrent update to Configuration [<id>]. Retry to create a new revision.` — while
//! an id the service never issued stays an unknown resource (HTTP 404,
//! `NotFoundException`); the READ paths report the deleted id as not found and the
//! listing as absent, exactly as they do an id that was never issued.
//!
//! `UpdateConfiguration` is the only operation this model carries that writes a new
//! revision of a configuration (`smithy_query(query="digest")`: the configuration
//! surface is Create/Update/Delete plus the three reads), so it is the whole surface the
//! conflict half of the rule covers.
//!
//! The table spans what the rule generalises over: every `EngineType` the model
//! declares (`ENGINE_TYPE_VALUES`), configurations deleted with no revision made, with
//! one and with several, the retried write sent with and without a `Description` and
//! carrying each engine's own document form, and the write retried twice so the refusal
//! is shown to be the configuration's state rather than a one-shot answer. The
//! never-issued contrast half spans the id forms `IDS_NAMING_NOTHING` holds.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::ENGINE_TYPE_VALUES;
use service_com_amazonaws_mq_mq::state::FIRST_CONFIGURATION_REVISION;

use crate::broker_lifecycle::{string_at, Mq};
use crate::configuration_name::configuration_body;
use crate::configuration_wire_form::{base64_data, IDS_NAMING_NOTHING};

/// Amazon MQ's sentence for a write that cannot be turned into a new revision of the
/// configuration the id names. The id is the only part that varies.
fn conflict_message(configuration_id: &str) -> String {
    format!("Concurrent update to Configuration [{configuration_id}]. Retry to create a new revision.")
}

/// Amazon MQ's sentence for a `ConfigurationId` that resolves to no configuration.
fn not_found_message(configuration_id: &str) -> String {
    format!("Configuration ID [{configuration_id}] can't be found. Make sure it exists.")
}

/// A document of the form the engine's `Data` member documents: "the base64-encoded XML
/// configuration" for ActiveMQ, "the base64-encoded Cuttlefish configuration" for
/// RabbitMQ (`smithy_query(query="doc", shape="UpdateConfigurationRequest",
/// member="Data")`).
fn document_for(engine_type: &str) -> String {
    match engine_type {
        "RABBITMQ" => base64_data("consumer_timeout = 1800000\n"),
        _ => base64_data(
            "<broker xmlns=\"http://activemq.apache.org/schema/core\"><plugins/></broker>",
        ),
    }
}

/// One row of the table: a configuration of `engine_type`, revised `revisions_before`
/// times, then deleted.
struct Row {
    engine_type: &'static str,
    revisions_before: usize,
    description: Option<&'static str>,
}

/// Every engine crossed with a configuration deleted fresh, deleted after one revision
/// and deleted after several, and the retried write sent with and without a
/// `Description`.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for engine_type in ENGINE_TYPE_VALUES {
        for (index, revisions_before) in [0usize, 1, 3].into_iter().enumerate() {
            rows.push(Row {
                engine_type,
                revisions_before,
                description: if index % 2 == 0 {
                    None
                } else {
                    Some("queued revision push")
                },
            });
        }
    }
    rows
}

/// Create a configuration of `engine_type` under `name`, revise it `revisions` times and
/// delete it, reporting the id it was issued under.
async fn issue_then_delete(mq: &Mq, case: &str, name: &str, row: &Row) -> String {
    let mut body = configuration_body(name);
    body["engineType"] = json!(row.engine_type);
    let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup create expected 200, got {status} {created:?}"
    );
    let id = string_at(&created, "/id");

    let document = document_for(row.engine_type);
    for revision in 0..row.revisions_before {
        let (status, code, answer) = mq
            .update_configuration(&id, &document, Some("before delete"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup revision {revision} expected 200, got {status} {code:?} \
             {answer:?}"
        );
    }

    let (status, code, deleted) = mq.delete_configuration(&id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: delete expected 200, got {status} {code:?} {deleted:?}"
    );
    assert_eq!(
        string_at(&deleted, "/configurationId"),
        id,
        "case {case:?}: the delete must name the configuration it removed, got {deleted:?}"
    );
    id
}

/// RULE, conflict half: a write against an id this service issued and has since deleted
/// is the modelled `ConflictException`, with Amazon MQ's sentence for it, however the
/// configuration was revised before it was deleted and whatever the write carries.
#[tokio::test]
async fn a_write_against_a_deleted_configuration_id_is_a_conflict() {
    let mq = Mq::new("us-east-1");

    for (index, row) in rows().iter().enumerate() {
        let case = format!(
            "{}/{} revisions before delete/description={:?}",
            row.engine_type, row.revisions_before, row.description
        );
        let id = issue_then_delete(&mq, &case, &format!("cfg-retired-{index}"), row).await;
        let document = document_for(row.engine_type);

        // The write is refused the same way however many times it is retried: what it
        // conflicts with is the configuration's state, not this one call.
        for attempt in 0..2 {
            let (status, code, answer) = mq
                .update_configuration(&id, &document, row.description)
                .await;
            assert_eq!(
                status, 409,
                "case {case:?} attempt {attempt}: expected 409, got {status} {code:?} \
                 {answer:?}"
            );
            assert_eq!(
                code, "ConflictException",
                "case {case:?} attempt {attempt}: expected ConflictException, got {code:?} \
                 {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/message"),
                conflict_message(&id),
                "case {case:?} attempt {attempt}: expected Amazon MQ's sentence for this \
                 refusal, got {answer:?}"
            );
        }

        // The refused write stored nothing: the id is still a configuration nobody can
        // read, on every read path, and the listing still does not hold it.
        for (operation, answer) in [
            ("DescribeConfiguration", mq.describe_configuration(&id).await),
            (
                "ListConfigurationRevisions",
                mq.list_configuration_revisions(&id, "").await,
            ),
            (
                "DescribeConfigurationRevision",
                mq.describe_configuration_revision(&id, &FIRST_CONFIGURATION_REVISION.to_string())
                    .await,
            ),
        ] {
            let (status, code, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} on a deleted id expected 404, got {status} {body:?}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: {operation} on a deleted id expected NotFoundException, got \
                 {code:?} {body:?}"
            );
        }
        assert_eq!(
            string_at(&mq.describe_configuration(&id).await.2, "/message"),
            not_found_message(&id),
            "case {case:?}: the read path keeps Amazon MQ's not-found sentence"
        );

        let (status, listed) = mq
            .call("GET", "/v1/configurations?maxResults=100", None)
            .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {listed:?}");
        let ids: Vec<String> = listed
            .pointer("/configurations")
            .and_then(Value::as_array)
            .map(|entries| entries.iter().map(|e| string_at(e, "/id")).collect())
            .unwrap_or_default();
        assert!(
            !ids.contains(&id),
            "case {case:?}: a deleted configuration must not be listed, got {ids:?}"
        );
    }
}

/// RULE, never-issued half: the write path keeps the modelled `NotFoundException` for an
/// id this service never issued, whatever form that id takes — so a client can tell a
/// lost race against a deleted configuration from an id that is not its own.
#[tokio::test]
async fn a_write_against_an_id_this_service_never_issued_is_not_found() {
    let mq = Mq::new("us-east-1");
    let document = document_for(ENGINE_TYPE_VALUES[0]);

    // One configuration exists, so the refusals below are about the id asked for rather
    // than about an empty store.
    let (status, _code, created) = mq.create_configuration("cfg-never-issued").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");

    for id in IDS_NAMING_NOTHING {
        let case = format!("UpdateConfiguration on {id:?}");
        let (status, code, answer) = mq.update_configuration(id, &document, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {code:?} {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            not_found_message(id),
            "case {case:?}: expected Amazon MQ's not-found sentence, got {answer:?}"
        );
    }

    // And the live configuration is still writable: the refusals above changed nothing.
    let id = string_at(&created, "/id");
    let (status, code, updated) = mq.update_configuration(&id, &document, None).await;
    assert_eq!(
        status, 200,
        "a live configuration must still take a revision, got {status} {code:?} {updated:?}"
    );
}

/// A deleted configuration's NAME is free again, and the configuration created under it
/// is a different resource: it is issued its own id, which is writable, while the
/// retired id keeps conflicting. An id this service issued is never issued twice.
#[tokio::test]
async fn recreating_a_deleted_configuration_issues_a_new_writable_id() {
    let mq = Mq::new("us-east-1");
    let name = "cfg-recycled";
    let document = document_for(ENGINE_TYPE_VALUES[0]);

    let (status, _code, first) = mq.create_configuration(name).await;
    assert_eq!(status, 200, "setup: expected 200, got {first:?}");
    let retired = string_at(&first, "/id");
    let (status, _code, deleted) = mq.delete_configuration(&retired).await;
    assert_eq!(status, 200, "setup delete: expected 200, got {deleted:?}");

    let (status, code, second) = mq.create_configuration(name).await;
    assert_eq!(
        status, 200,
        "the deleted name must be free again, got {status} {code:?} {second:?}"
    );
    let live = string_at(&second, "/id");
    assert_ne!(
        live, retired,
        "a recreated configuration must be issued its own id, got {live:?}"
    );

    let (status, code, answer) = mq.update_configuration(&live, &document, None).await;
    assert_eq!(
        status, 200,
        "case {live:?}: the live configuration must take a revision, got {status} {code:?} \
         {answer:?}"
    );
    assert_eq!(
        answer
            .pointer("/latestRevision/revision")
            .and_then(Value::as_i64),
        Some(i64::from(FIRST_CONFIGURATION_REVISION) + 1),
        "case {live:?}: the write must make the next revision of the new configuration, got \
         {answer:?}"
    );

    let (status, code, answer) = mq.update_configuration(&retired, &document, None).await;
    assert_eq!(
        status, 409,
        "case {retired:?}: the retired id must still conflict, got {status} {code:?} {answer:?}"
    );
    assert_eq!(
        code, "ConflictException",
        "case {retired:?}: expected ConflictException, got {code:?} {answer:?}"
    );
    assert_eq!(
        string_at(&answer, "/message"),
        conflict_message(&retired),
        "case {retired:?}: the sentence must name the id the write targeted, got {answer:?}"
    );
}
