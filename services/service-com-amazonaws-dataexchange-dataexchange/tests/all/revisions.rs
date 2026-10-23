// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetRevision`, the single-revision reader.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `call`, `create_data_set`, `seconds`, `is_modelled_id`)
//! is the one `data_sets.rs` already uses.
//!
//! The tables span the classes the rules generalise over — a revision with no
//! comment, with the empty comment the model's `@length(min: 0)` admits, with a
//! comment at the published maximum and with a non-ASCII one; with no tags, one tag
//! and several; across several revisions of one data set and across several data
//! sets — rather than one recorded example.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{
    call, create_data_set, emulator, is_modelled_id, seconds, Answer, ACCOUNT, REGION,
};

/// A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) for a resource this emulator does not
/// hold, so the answer is about the resource rather than about the request.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// The published maximum of the revision `Comment` member
/// (`com.amazonaws.dataexchange#__stringMin0Max16384`, `@length(min: 0, max: 16384)`).
/// Source: `smithy_query(query="operation", operation="GetRevision")`.
const COMMENT_MAX_CHARS: usize = 16_384;

/// The model's `ResourceType` value a revision 404 reports, from the `ResourceType`
/// enum (`ASSET | DATA_GRANT | DATA_SET | EVENT_ACTION | JOB | REVISION`).
const RESOURCE_TYPE_REVISION: &str = "REVISION";

/// The same enum's value for a data set.
const RESOURCE_TYPE_DATA_SET: &str = "DATA_SET";

/// One row of the revision catalog: how the case is named, the `Comment` the
/// revision is created with (`None` meaning the member is omitted entirely) and its
/// `Tags`.
struct RevisionCase {
    case: &'static str,
    comment: Option<String>,
    tags: Option<Value>,
}

/// The rows span what a revision record can carry: comment absent / empty / short /
/// at the published maximum / non-ASCII, paired with no tags, one tag and several.
fn revision_cases() -> Vec<RevisionCase> {
    vec![
        RevisionCase {
            case: "no comment, no tags",
            comment: None,
            tags: None,
        },
        RevisionCase {
            case: "empty comment at the modelled minimum length",
            comment: Some(String::new()),
            tags: Some(json!({ "team": "data" })),
        },
        RevisionCase {
            case: "short comment, several tags",
            comment: Some("first cut of the monthly feed".to_string()),
            tags: Some(json!({ "team": "data", "tier": "gold", "owner": "ops" })),
        },
        RevisionCase {
            case: "comment at the modelled maximum length",
            comment: Some("c".repeat(COMMENT_MAX_CHARS)),
            tags: None,
        },
        RevisionCase {
            case: "non-ascii comment, one tag",
            comment: Some("révision — 改訂 ✅".to_string()),
            tags: Some(json!({ "регион": "eu" })),
        },
    ]
}

/// Create one revision of `data_set_id`, failing the named case if the create itself
/// did not answer 201.
async fn create_revision(
    router: &MultiServiceRouter,
    data_set_id: &str,
    comment: Option<&str>,
    tags: Option<&Value>,
    case: &str,
) -> Answer {
    let mut body = json!({});
    if let Some(comment) = comment {
        body["Comment"] = json!(comment);
    }
    if let Some(tags) = tags {
        body["Tags"] = tags.clone();
    }
    let answer = call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(body),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateRevision, got {} body {:?}",
        answer.status, answer.body
    );
    answer
}

/// Read one revision through `GetRevision`.
async fn get_revision(router: &MultiServiceRouter, data_set_id: &str, revision_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        None,
    )
    .await
}

/// Assert that `fetched` reports the record `created` minted, member by member.
fn assert_same_revision(case: &str, created: &Answer, fetched: &Answer) {
    for member in [
        "Arn",
        "Comment",
        "CreatedAt",
        "DataSetId",
        "Finalized",
        "Id",
        "RevocationComment",
        "Revoked",
        "RevokedAt",
        "SourceId",
        "Tags",
        "UpdatedAt",
    ] {
        assert_eq!(
            fetched.member(member),
            created.member(member),
            "case {case}: expected GetRevision's {member} to equal the created record's, \
             got {:?} for {:?}",
            fetched.member(member),
            created.member(member)
        );
    }
}

/// RULE: `GetRevision` answers 200 with the stored revision of the named data set —
/// its `Id`, `Arn`, `DataSetId`, `Comment`, `Tags`, `Finalized`, `Revoked` and
/// timestamps exactly as the create persisted them — for every shape of revision the
/// model admits.
#[tokio::test]
async fn create_then_get_round_trips_every_revision_shape() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "revision-reads", "r", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    for RevisionCase {
        case,
        comment,
        tags,
    } in revision_cases()
    {
        let created = create_revision(
            &router,
            &data_set_id,
            comment.as_deref(),
            tags.as_ref(),
            case,
        )
        .await;
        let revision_id = created.string("Id");

        let fetched = get_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetRevision, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert!(
            is_modelled_id(&fetched.string("Id")),
            "case {case}: expected an Id matching ^[a-zA-Z0-9]{{30,40}}$, got {:?}",
            fetched.member("Id")
        );
        assert_eq!(
            fetched.string("Id"),
            revision_id,
            "case {case}: expected the requested revision, got {:?}",
            fetched.member("Id")
        );
        assert_eq!(
            fetched.string("DataSetId"),
            data_set_id,
            "case {case}: expected the revision to report its data set, got {:?}",
            fetched.member("DataSetId")
        );
        assert_eq!(
            fetched.string("Arn"),
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}/revisions/{revision_id}"
            ),
            "case {case}: expected the revision Arn to name its data set and itself, got {:?}",
            fetched.member("Arn")
        );

        // The comment is reported exactly as stored, and omitted when none was given.
        let expected_comment = comment
            .as_ref()
            .map_or(Value::Null, |text| json!(text));
        assert_eq!(
            fetched.member("Comment"),
            &expected_comment,
            "case {case}: expected the stored Comment, got {:?}",
            fetched.member("Comment")
        );
        let expected_tags = tags.clone().unwrap_or(Value::Null);
        assert_eq!(
            fetched.member("Tags"),
            &expected_tags,
            "case {case}: expected the stored Tags, got {:?}",
            fetched.member("Tags")
        );

        // A revision nothing has finalized or revoked carries both flags false and no
        // revocation members at all (`Finalized` and `Revoked` are `@default: false`).
        assert_eq!(
            fetched.member("Finalized"),
            &json!(false),
            "case {case}: expected Finalized=false on a fresh revision, got {:?}",
            fetched.member("Finalized")
        );
        assert_eq!(
            fetched.member("Revoked"),
            &json!(false),
            "case {case}: expected Revoked=false on a fresh revision, got {:?}",
            fetched.member("Revoked")
        );
        for member in ["RevocationComment", "RevokedAt", "SourceId"] {
            assert_eq!(
                fetched.member(member),
                &Value::Null,
                "case {case}: expected no {member} on a revision nothing revoked, got {:?}",
                fetched.member(member)
            );
        }

        let created_at = seconds(fetched.member("CreatedAt"), case);
        let updated_at = seconds(fetched.member("UpdatedAt"), case);
        assert_eq!(
            created_at, updated_at,
            "case {case}: expected CreatedAt == UpdatedAt on a revision nothing updated, \
             got {created_at} and {updated_at}"
        );
        assert_eq!(
            created_at,
            seconds(created.member("CreatedAt"), case),
            "case {case}: expected the CreatedAt the create reported, got {created_at}"
        );

        assert_same_revision(case, &created, &fetched);
    }
}

/// RULE: `GetRevision` is a read — repeating it answers the identical record, and it
/// leaves what every other reader sees untouched.
#[tokio::test]
async fn repeated_get_is_stable_and_changes_nothing() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "stable-revision", "s", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let created = create_revision(
        &router,
        &data_set_id,
        Some("unchanged by reads"),
        Some(&json!({ "team": "data" })),
        "stable",
    )
    .await;
    let revision_id = created.string("Id");

    let first = get_revision(&router, &data_set_id, &revision_id).await;
    // Rows differ along how many reads preceded this one: a read that moved state
    // would show up as a difference on any of them.
    for round in 1..=3 {
        let case = format!("read {round}");
        let again = get_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected 200, got {} body {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "case {case}: expected the identical record on every read, got {:?} for {:?}",
            again.body, first.body
        );
    }

    // The list reader agrees with the single-revision reader, before and after.
    let listed = call(
        &router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions?maxResults=200"),
        None,
    )
    .await;
    let entries = listed
        .member("Revisions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let entry = entries
        .iter()
        .find(|entry| entry["Id"].as_str() == Some(revision_id.as_str()))
        .unwrap_or_else(|| panic!("case list after reads: the revision is missing from the list"));
    for member in ["Arn", "Comment", "DataSetId", "Finalized", "Id", "Revoked"] {
        assert_eq!(
            &entry[member],
            first.member(member),
            "case list after reads: expected ListDataSetRevisions' {member} to agree with \
             GetRevision's, got {:?} for {:?}",
            entry[member],
            first.member(member)
        );
    }
}

/// RULE: a revision belongs to exactly one data set, so `GetRevision` reports each
/// data set's own revisions and nothing else: the same `RevisionId` under another
/// data set is absent, whatever is stored elsewhere.
#[tokio::test]
async fn a_revision_is_scoped_to_its_own_data_set() {
    let router = emulator();

    // Rows differ along how many revisions each data set holds (one, two, three), so
    // the lookup is exercised where several records live side by side.
    let mut data_sets: Vec<(String, Vec<String>)> = Vec::new();
    for (index, count) in [1usize, 2, 3].into_iter().enumerate() {
        let data_set_id = create_data_set(
            &router,
            &format!("scoped-{index}"),
            "s",
            "S3_SNAPSHOT",
            None,
        )
        .await
        .string("Id");
        let mut revisions = Vec::new();
        for revision in 0..count {
            let case = format!("data set {index} revision {revision}");
            let comment = format!("ds{index}-rev{revision}");
            revisions.push(
                create_revision(&router, &data_set_id, Some(&comment), None, &case)
                    .await
                    .string("Id"),
            );
        }
        data_sets.push((data_set_id, revisions));
    }

    for (index, (data_set_id, revisions)) in data_sets.iter().enumerate() {
        for (position, revision_id) in revisions.iter().enumerate() {
            let case = format!("own revision {position} of data set {index}");
            let fetched = get_revision(&router, data_set_id, revision_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected 200, got {} body {:?}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.member("Comment"),
                &json!(format!("ds{index}-rev{position}")),
                "case {case}: expected this revision's own Comment, got {:?}",
                fetched.member("Comment")
            );
        }

        // Every revision of every OTHER data set is absent under this one.
        for (other_index, (_, other_revisions)) in data_sets.iter().enumerate() {
            if other_index == index {
                continue;
            }
            for revision_id in other_revisions {
                let case =
                    format!("revision of data set {other_index} read under data set {index}");
                let answer = get_revision(&router, data_set_id, revision_id).await;
                assert_eq!(
                    answer.status, 404,
                    "case {case}: expected 404, got {} body {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case}: expected ResourceNotFoundException, got {:?}",
                    answer.error
                );
                assert_eq!(
                    answer.member("ResourceType"),
                    &json!(RESOURCE_TYPE_REVISION),
                    "case {case}: expected the 404 to name the absent revision's type, got {:?}",
                    answer.member("ResourceType")
                );
            }
        }
    }
}

/// RULE: a well-formed id naming nothing is a 404 `ResourceNotFoundException` whose
/// `ResourceType` names which resource was absent — the data set when the data set
/// is unknown, the revision when only the revision is.
#[tokio::test]
async fn an_absent_data_set_or_revision_is_a_404() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "absent-reads", "a", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let revision_id = create_revision(&router, &data_set_id, None, None, "setup")
        .await
        .string("Id");

    // Rows differ along WHICH named resource is absent: the data set, the revision,
    // or both.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "unknown data set, unknown revision",
            ABSENT_ID.to_string(),
            ABSENT_ID.to_string(),
            RESOURCE_TYPE_DATA_SET,
        ),
        (
            "unknown data set, a revision that exists elsewhere",
            ABSENT_ID.to_string(),
            revision_id.clone(),
            RESOURCE_TYPE_DATA_SET,
        ),
        (
            "known data set, unknown revision",
            data_set_id.clone(),
            ABSENT_ID.to_string(),
            RESOURCE_TYPE_REVISION,
        ),
        (
            "known data set, a data set id used as a revision id",
            data_set_id.clone(),
            data_set_id.clone(),
            RESOURCE_TYPE_REVISION,
        ),
    ];

    for (case, data_set, revision, resource_type) in &rows {
        let answer = get_revision(&router, data_set, revision).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.member("ResourceType"),
            &json!(resource_type),
            "case {case}: expected ResourceType {resource_type}, got {:?}",
            answer.member("ResourceType")
        );
    }

    // And once the data set is deleted, its revision is gone with it.
    let deleted = call(&router, "DELETE", &format!("/v1/data-sets/{data_set_id}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case delete setup: expected 204 from DeleteDataSet, got {} body {:?}",
        deleted.status, deleted.body
    );
    let after_delete = get_revision(&router, &data_set_id, &revision_id).await;
    assert_eq!(
        after_delete.status, 404,
        "case get after the data set was deleted: expected 404, got {} body {:?}",
        after_delete.status, after_delete.body
    );
    assert_eq!(
        after_delete.member("ResourceType"),
        &json!(RESOURCE_TYPE_DATA_SET),
        "case get after the data set was deleted: expected the data set to be the absent \
         resource, got {:?}",
        after_delete.member("ResourceType")
    );
}

/// RULE: an id outside the modelled `^[a-zA-Z0-9]{30,40}$` pattern is a 400
/// `ValidationException`, not a 404 — in EITHER label position; an id inside the
/// pattern is acceptable, so the answer is then about the resource.
#[tokio::test]
async fn a_malformed_id_in_either_position_is_a_400() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "malformed-reads", "m", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Rows differ along HOW the id breaks the pattern — too short, too long, a
    // disallowed character — plus both published bounds, which are well formed.
    let ids = [
        ("one short of the minimum length", "a".repeat(29)),
        ("at the minimum length", "a".repeat(30)),
        ("at the maximum length", "a".repeat(40)),
        ("one past the maximum length", "a".repeat(41)),
        (
            "an underscore is not alphanumeric",
            format!("{}_{}", "a".repeat(16), "b".repeat(15)),
        ),
    ];

    for (how, id) in ids {
        for position in ["DataSetId", "RevisionId"] {
            let case = format!("{position} {how}");
            let answer = match position {
                "DataSetId" => get_revision(&router, &id, ABSENT_ID).await,
                _ => get_revision(&router, &data_set_id, &id).await,
            };
            if is_modelled_id(&id) {
                assert_eq!(
                    answer.status, 404,
                    "case {case}: expected 404 for a well-formed but unknown id {id:?}, \
                     got {} body {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400 for {id:?}, got {} body {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ValidationException"),
                    "case {case}: expected ValidationException for {id:?}, got {:?}",
                    answer.error
                );
            }
        }
    }
}
