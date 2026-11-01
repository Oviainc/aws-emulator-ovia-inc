// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `BatchDeleteUniqueId`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: `BatchDeleteUniqueId` REMOVES from the named rule-based
//! workflow's match index the record each `uniqueId` it carries names, and reports what
//! it did from the index that remains — the ids it removed on `deleted`, an id the
//! workflow holds no such record for on `errors` with the modelled `VALIDATION_ERROR`,
//! and on `disconnectedUniqueIds` the records that survived in a group the call took a
//! member from and that no rule of the workflow joins to another record any more. The
//! deletion is observable through the independent reader (`GetMatchId` stops resolving
//! a record that is gone, and `GenerateMatchId` mints a new group for it). `uniqueIds`
//! is @required and HEADER-serialized, so a list of no elements arrives as no header at
//! all and is refused as the absent @required member, changing nothing.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the size of the group a deleted record belonged to (one, two
//! and three records), how many ids one call names (one, two, the same id twice), which
//! of them the workflow actually holds (all, none, a mix), how many groups one call
//! touches, the `inputSource` scope (absent, naming the record's own source, naming
//! another one), and the wire forms an empty list takes (the header absent, empty and
//! blank). The recorded request — an empty list, then one id the workflow holds — is
//! two rows of these.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, glue_table_arn, role_arn, string_member, Emulator, Reply,
};

/// The schema mapping every workflow here reads its `matchKey` grouping from: `email`
/// carries the `matchKey` the workflows' one rule matches on, so two records sharing an
/// email share a match group.
const SCHEMA_NAME: &str = "people-del";

/// The two Glue tables the workflows name as input sources, so a row can scope a delete
/// to one source and leave a record that arrived from the other alone.
fn first_source() -> String {
    glue_table_arn("people")
}

fn second_source() -> String {
    glue_table_arn("partners")
}

/// The message AWS answers a `BatchDeleteUniqueId` that carries no unique id, as a
/// literal here so this suite pins the text independently of the service crate's own
/// constant. It is the refusal an absent @required member earns — `uniqueIds` is bound
/// to a header, so a list of no elements is no header at all — and it is entirely
/// fixed: no part of the request appears in it.
const NO_UNIQUE_IDS_MESSAGE: &str = "1 validation error detected: Value null at 'uniqueIds' \
                                     failed to satisfy constraint: Member must not be null";

/// The modelled `DeleteUniqueIdErrorType` reported for an id the workflow holds no
/// record for (`smithy_query(query="shape",shape="DeleteUniqueIdErrorType")` →
/// `SERVICE_ERROR | VALIDATION_ERROR`).
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

fn schema_request() -> Value {
    json!({
        "schemaName": SCHEMA_NAME,
        "mappedInputFields": [
            { "fieldName": "id", "type": "UNIQUE_ID" },
            { "fieldName": "email", "type": "EMAIL_ADDRESS", "matchKey": "email" },
        ],
    })
}

/// A `CreateMatchingWorkflow` request for a RULE_MATCHING workflow over both input
/// sources, matching on the `email` key.
fn workflow_request(name: &str) -> Value {
    json!({
        "workflowName": name,
        "inputSourceConfig": [
            { "inputSourceARN": first_source(), "schemaName": SCHEMA_NAME },
            { "inputSourceARN": second_source(), "schemaName": SCHEMA_NAME },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/delete",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": {
            "resolutionType": "RULE_MATCHING",
            "ruleBasedProperties": {
                "rules": [{ "ruleName": "by email", "matchingKeys": ["email"] }],
                "attributeMatchingModel": "ONE_TO_ONE",
            },
            "enableRealTimeMatching": true,
        },
        "roleArn": role_arn(),
    })
}

/// Create the shared schema mapping once, and the workflow `name`.
async fn seed(emulator: &Emulator, name: &str) {
    let schema = emulator.create_schema_mapping(&schema_request()).await;
    assert!(
        schema.status == 200 || schema.error_code.as_deref() == Some("ConflictException"),
        "seeding the schema mapping failed: {schema:?}"
    );
    let created = emulator
        .create_matching_workflow(&workflow_request(name))
        .await;
    assert_eq!(
        created.status, 200,
        "seeding the matching workflow {name:?} failed: {created:?}"
    );
}

/// `GenerateMatchId` one record into `workflow_name`'s index, and answer the `matchId`
/// of the group it joined.
async fn index_record(
    emulator: &Emulator,
    workflow_name: &str,
    unique_id: &str,
    email: &str,
    input_source: &str,
) -> String {
    let reply = emulator
        .call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/generateMatches"),
            Some(&json!({
                "records": [{
                    "inputSourceARN": input_source,
                    "uniqueId": unique_id,
                    "recordAttributeMap": { "email": email },
                }],
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "indexing {unique_id:?} must succeed, got {reply:?}"
    );
    let groups = reply
        .body
        .get("matchGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("matchGroups is not a list in {:?}", reply.body));
    assert_eq!(
        groups.len(),
        1,
        "one submitted record answers one group, got {groups:?}"
    );
    string_member(&groups[0], "matchId")
}

/// `GetMatchId` for the record identified by `email`: the independent reader of the
/// index this suite's deletions change. `None` when the index resolves nothing.
async fn read_match_id(emulator: &Emulator, workflow_name: &str, email: &str) -> Option<String> {
    let reply = emulator
        .call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/matches"),
            Some(&json!({ "record": { "email": email } })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "GetMatchId must succeed, got {reply:?}"
    );
    reply
        .body
        .get("matchId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `DELETE /matchingworkflows/{workflowName}/uniqueids` — `BatchDeleteUniqueId`'s
/// modelled binding (`smithy_query(query="operation",operation="BatchDeleteUniqueId")`).
///
/// `unique_ids` is the `uniqueIds` header VALUE, verbatim, and `None` omits the header
/// entirely: the member is a list bound to a header, so this is the whole wire surface
/// the operation has. `input_source` is the optional `inputSource` header.
async fn batch_delete(
    emulator: &Emulator,
    workflow_name: &str,
    unique_ids: Option<&str>,
    input_source: Option<&str>,
) -> Reply {
    let mut headers: Vec<(&str, String)> = Vec::new();
    if let Some(value) = unique_ids {
        headers.push(("uniqueIds", value.to_string()));
    }
    if let Some(value) = input_source {
        headers.push(("inputSource", value.to_string()));
    }
    emulator
        .call_with_headers(
            "DELETE",
            &format!("/matchingworkflows/{workflow_name}/uniqueids"),
            None,
            &headers,
        )
        .await
}

/// The `uniqueId`s of a `deleted` list, in the order the response lists them.
fn deleted_ids(case: &str, reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("deleted")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: deleted is not a list in {:?}", reply.body))
        .iter()
        .map(|entry| string_member(entry, "uniqueId"))
        .collect()
}

/// The `(uniqueId, errorType)` pairs of an `errors` list, in response order.
fn error_pairs(case: &str, reply: &Reply) -> Vec<(String, String)> {
    reply
        .body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: errors is not a list in {:?}", reply.body))
        .iter()
        .map(|entry| {
            (
                string_member(entry, "uniqueId"),
                string_member(entry, "errorType"),
            )
        })
        .collect()
}

/// The `disconnectedUniqueIds` of a reply, as a set: the list is derived from the stored
/// index, so what it HOLDS is the rule and its order is the index's.
fn disconnected_ids(case: &str, reply: &Reply) -> BTreeSet<String> {
    reply
        .body
        .get("disconnectedUniqueIds")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: disconnectedUniqueIds is not a list in {:?}",
                reply.body
            )
        })
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a disconnected id is not a string"))
                .to_string()
        })
        .collect()
}

/// Assert the call succeeded with a modelled `DeleteUniqueIdStatus`
/// (`COMPLETED | ACCEPTED`).
fn assert_delete_status(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {reply:?}"
    );
    let status = string_member(&reply.body, "status");
    assert!(
        status == "COMPLETED" || status == "ACCEPTED",
        "case {case:?}: expected a modelled DeleteUniqueIdStatus, got {status:?}"
    );
}

/// One record indexed before a delete: its `uniqueId`, the email that decides its group,
/// and the input source it arrived from.
struct Indexed {
    unique_id: &'static str,
    email: &'static str,
    /// `true` for [`first_source`], `false` for [`second_source`].
    from_first_source: bool,
}

/// One deletion: the records the workflow holds, the `uniqueIds` header the call
/// carries, and what the rule says comes back.
struct DeletionCase {
    case: &'static str,
    records: Vec<Indexed>,
    /// The `uniqueIds` header value, i.e. the comma-separated list an SDK sends.
    delete: &'static str,
    /// The `inputSource` header, when the call scopes itself to one source.
    input_source: Option<bool>,
    deleted: Vec<&'static str>,
    errors: Vec<&'static str>,
    disconnected: Vec<&'static str>,
}

fn record(unique_id: &'static str, email: &'static str) -> Indexed {
    Indexed {
        unique_id,
        email,
        from_first_source: true,
    }
}

/// The table: group sizes of one, two and three; one id, two ids and a repeated id per
/// call; ids the workflow holds, ids it does not and a mix; one group touched and two;
/// and the `inputSource` scope over absent, the record's own source and another source.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            case: "the only record of a one-record group",
            records: vec![record("r1", "a@example.com")],
            delete: "r1",
            input_source: None,
            deleted: vec!["r1"],
            errors: vec![],
            disconnected: vec![],
        },
        DeletionCase {
            case: "one record of a two-record group leaves the other connected to nothing",
            records: vec![record("r1", "a@example.com"), record("r2", "a@example.com")],
            delete: "r1",
            input_source: None,
            deleted: vec!["r1"],
            errors: vec![],
            disconnected: vec!["r2"],
        },
        DeletionCase {
            case: "one record of a three-record group leaves the rest matching each other",
            records: vec![
                record("r1", "a@example.com"),
                record("r2", "a@example.com"),
                record("r3", "a@example.com"),
            ],
            delete: "r1",
            input_source: None,
            deleted: vec!["r1"],
            errors: vec![],
            disconnected: vec![],
        },
        DeletionCase {
            case: "both records of a two-record group, so nothing survives to disconnect",
            records: vec![record("r1", "a@example.com"), record("r2", "a@example.com")],
            delete: "r1,r2",
            input_source: None,
            deleted: vec!["r1", "r2"],
            errors: vec![],
            disconnected: vec![],
        },
        DeletionCase {
            case: "a uniqueId the workflow never indexed",
            records: vec![record("r1", "a@example.com")],
            delete: "ghost",
            input_source: None,
            deleted: vec![],
            errors: vec!["ghost"],
            disconnected: vec![],
        },
        DeletionCase {
            case: "a repeated uniqueId names one record",
            records: vec![record("r1", "a@example.com")],
            delete: "r1,r1",
            input_source: None,
            deleted: vec!["r1"],
            errors: vec![],
            disconnected: vec![],
        },
        DeletionCase {
            case: "one id the workflow holds and one it does not, in one call",
            records: vec![record("r1", "a@example.com"), record("r2", "b@example.com")],
            delete: "r1,ghost",
            input_source: None,
            deleted: vec!["r1"],
            errors: vec!["ghost"],
            disconnected: vec![],
        },
        DeletionCase {
            case: "one id from each of two groups",
            records: vec![
                record("r1", "a@example.com"),
                record("r2", "a@example.com"),
                record("r3", "b@example.com"),
                record("r4", "b@example.com"),
            ],
            delete: "r1,r3",
            input_source: None,
            deleted: vec!["r1", "r3"],
            errors: vec![],
            disconnected: vec!["r2", "r4"],
        },
        DeletionCase {
            case: "inputSource naming the source the record arrived from",
            records: vec![Indexed {
                unique_id: "r1",
                email: "a@example.com",
                from_first_source: false,
            }],
            delete: "r1",
            input_source: Some(false),
            deleted: vec!["r1"],
            errors: vec![],
            disconnected: vec![],
        },
        DeletionCase {
            case: "inputSource naming another source leaves the record alone",
            records: vec![Indexed {
                unique_id: "r1",
                email: "a@example.com",
                from_first_source: false,
            }],
            delete: "r1",
            input_source: Some(true),
            deleted: vec![],
            errors: vec!["r1"],
            disconnected: vec![],
        },
    ]
}

#[tokio::test]
async fn a_delete_removes_exactly_the_records_it_names_and_reports_what_is_left() {
    let emulator = Emulator::new();

    for (index, case) in deletion_cases().into_iter().enumerate() {
        let label = case.case;
        // Each row gets its own workflow, so no row's index can leak into another's.
        let workflow = format!("del-{index}");
        seed(&emulator, &workflow).await;

        let mut group_of: Vec<(&str, String)> = Vec::new();
        for held in &case.records {
            let source = if held.from_first_source {
                first_source()
            } else {
                second_source()
            };
            let match_id =
                index_record(&emulator, &workflow, held.unique_id, held.email, &source).await;
            group_of.push((held.email, match_id));
        }

        let scope: Option<String> = case.input_source.map(|first| {
            if first {
                first_source()
            } else {
                second_source()
            }
        });
        let reply = batch_delete(&emulator, &workflow, Some(case.delete), scope.as_deref()).await;
        assert_delete_status(label, &reply);

        assert_eq!(
            deleted_ids(label, &reply),
            case.deleted,
            "case {label:?}: expected {:?} deleted, got {:?}",
            case.deleted,
            reply.body
        );
        let expected_errors: Vec<(String, String)> = case
            .errors
            .iter()
            .map(|id| ((*id).to_string(), VALIDATION_ERROR.to_string()))
            .collect();
        assert_eq!(
            error_pairs(label, &reply),
            expected_errors,
            "case {label:?}: expected the errors {expected_errors:?}, got {:?}",
            reply.body
        );
        let expected_disconnected: BTreeSet<String> = case
            .disconnected
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        assert_eq!(
            disconnected_ids(label, &reply),
            expected_disconnected,
            "case {label:?}: expected {expected_disconnected:?} disconnected, got {:?}",
            reply.body
        );

        // The round trip through the independent reader: a group that still holds a
        // record resolves to the SAME matchId it was given, and a group whose every
        // record this call deleted resolves to nothing at all.
        let deleted: BTreeSet<&str> = case.deleted.iter().copied().collect();
        for held in &case.records {
            let survives = case
                .records
                .iter()
                .any(|other| other.email == held.email && !deleted.contains(other.unique_id));
            let read = read_match_id(&emulator, &workflow, held.email).await;
            let expected = group_of
                .iter()
                .find(|(email, _)| *email == held.email)
                .map(|(_, match_id)| match_id.clone());
            if survives {
                assert_eq!(
                    read, expected,
                    "case {label:?}: {:?} must still resolve to its group",
                    held.email
                );
            } else {
                assert_eq!(
                    read, None,
                    "case {label:?}: every record of {:?} was deleted, so the reader must \
                     resolve nothing, got {read:?}",
                    held.email
                );
            }
        }
    }
}

/// The wire forms a list of no elements takes: `uniqueIds` is bound to a header, so an
/// empty list cannot be spelled on the wire at all.
fn empty_list_forms() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("the uniqueIds header absent, as an empty list is sent", None),
        ("the uniqueIds header present and empty", Some("")),
        ("the uniqueIds header holding only whitespace", Some("   ")),
    ]
}

#[tokio::test]
async fn a_request_carrying_no_unique_id_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let workflow = "del-empty";
    seed(&emulator, workflow).await;
    let match_id = index_record(
        &emulator,
        workflow,
        "rec-1",
        "kept@example.com",
        &first_source(),
    )
    .await;

    let mut texts = BTreeSet::new();
    for (label, header) in empty_list_forms() {
        let refused = batch_delete(&emulator, workflow, header, None).await;
        assert_error_shape(label, &refused, 400, "ValidationException");
        let message = string_member(&refused.body, "message");
        assert_eq!(
            message, NO_UNIQUE_IDS_MESSAGE,
            "case {label:?}: expected the refusal {NO_UNIQUE_IDS_MESSAGE:?}, got {message:?}"
        );
        texts.insert(message);

        // A refused call mutates nothing: the record it did not delete still resolves.
        assert_eq!(
            read_match_id(&emulator, workflow, "kept@example.com").await,
            Some(match_id.clone()),
            "case {label:?}: a refused delete must leave the index untouched"
        );
    }

    assert_eq!(
        texts.len(),
        1,
        "the refusal text is fixed, so every empty form must answer the same one, saw \
         {texts:?}"
    );
}

#[tokio::test]
async fn a_deleted_record_is_gone_from_the_index_rather_than_hidden() {
    let emulator = Emulator::new();
    let workflow = "del-regenerate";
    seed(&emulator, workflow).await;

    let first = index_record(
        &emulator,
        workflow,
        "rec-1",
        "again@example.com",
        &first_source(),
    )
    .await;
    let removed = batch_delete(&emulator, workflow, Some("rec-1"), None).await;
    assert_delete_status("a record re-indexed after its delete", &removed);
    assert_eq!(
        read_match_id(&emulator, workflow, "again@example.com").await,
        None,
        "the reader must resolve nothing once the only record is deleted"
    );

    // Submitting the same record again opens a NEW group: the previous one was removed
    // from the index, not merely withheld from the reader.
    let second = index_record(
        &emulator,
        workflow,
        "rec-1",
        "again@example.com",
        &first_source(),
    )
    .await;
    assert_ne!(
        second, first,
        "a record indexed after its delete must open a new group, got the deleted \
         group's id {first:?} back"
    );
    assert_eq!(
        read_match_id(&emulator, workflow, "again@example.com").await,
        Some(second),
        "the reader must resolve the group the re-indexed record opened"
    );
}

#[tokio::test]
async fn a_workflow_this_operation_is_not_defined_over_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator, "del-refusals").await;

    // A name the store holds no workflow for: there is no index to delete from.
    let absent = batch_delete(&emulator, "del-nosuchworkflow", Some("rec-1"), None).await;
    assert_error_shape(
        "a workflowName naming no workflow",
        &absent,
        404,
        "ResourceNotFoundException",
    );

    // A workflow that is not rule-based holds no match index at all, exactly as it
    // serves neither `GenerateMatchId` nor `GetMatchId`.
    let ml = emulator
        .create_matching_workflow(&json!({
            "workflowName": "del-ml",
            "inputSourceConfig": [
                { "inputSourceARN": first_source(), "schemaName": SCHEMA_NAME },
            ],
            "outputSourceConfig": [{
                "outputS3Path": "s3://er-output/delete",
                "output": [{ "name": "id" }],
            }],
            "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
            "roleArn": role_arn(),
        }))
        .await;
    assert_eq!(
        ml.status, 200,
        "seeding the ML_MATCHING workflow failed: {ml:?}"
    );
    let refused = batch_delete(&emulator, "del-ml", Some("rec-1"), None).await;
    assert_error_shape(
        "a workflow that is not rule-based",
        &refused,
        400,
        "ValidationException",
    );
}
