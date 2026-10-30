// SPDX-License-Identifier: CC0-1.0

//! The body of the modelled `ResourceNotFoundException`, across every lookup site
//! this crate refuses from.
//!
//! RULE under test: the exception's `Message` is fixed text with no variable part —
//! "The requested resource was not found." — and the identity of the missing thing
//! travels in the exception's own `ResourceId` and `ResourceType` members, which the
//! model documents as "The unique identifier for the resource that couldn't be
//! found." and "The type of resource that couldn't be found."
//! (`smithy_query(query="shape", shape="ResourceNotFoundException")`). So no part of
//! the sentence is taken from the request or from state, whatever the operation,
//! whatever the kind of resource and whatever the identifier.
//!
//! The rows span what the rule generalises over: each resource kind the service
//! addresses by id (data set, revision, job, event action) and the ARN-addressed
//! tagging surface, the read, update, delete and create sites of each, and several
//! forms of identifier — the published minimum and maximum lengths of the modelled
//! `Id` pattern, a mixed-case id in between, and an id outside that pattern on the
//! one member (`EventActionId`) the model declares without it.
//!
//! Every test drives the service through `MultiServiceRouter::dispatch` with a real
//! restJson1 request, so the route, the status, the error code and the serialized
//! body are all exercised.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::data_sets::{call, create_data_set, emulator, ACCOUNT, REGION};
use crate::resource_tags::percent_encode;
use crate::revisions::create_revision;

/// The whole `Message` AWS answers this error class with — the expectation these
/// tests hold, stated here independently of the constant the crate implements it
/// with, so a change to that constant is a failure rather than a tautology.
const NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// Values of the model's `com.amazonaws.dataexchange#ResourceType` enum. Source:
/// `smithy_query(query="shape", shape="ResourceType")` —
/// `DATA_SET | REVISION | ASSET | JOB | EVENT_ACTION | DATA_GRANT`.
const DATA_SET: &str = "DATA_SET";
const REVISION: &str = "REVISION";
const JOB: &str = "JOB";
const EVENT_ACTION: &str = "EVENT_ACTION";

/// An instant for the `date-time` member a `DATA_UPDATE` notification carries.
const AN_INSTANT: &str = "2024-01-01T00:00:00Z";

/// One request that addresses something the emulator does not hold, and the identity
/// its refusal has to report.
struct Lookup {
    label: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// What `ResourceId` must carry: the identifier the request named.
    resource_id: String,
    /// What `ResourceType` must carry, or `None` where the request named no kind this
    /// service can state (an ARN that addresses nothing stored).
    resource_type: Option<&'static str>,
}

/// Identifiers that are well formed per the modelled `Id` pattern
/// `^[a-zA-Z0-9]{30,40}$` but name nothing this emulator holds: the published minimum
/// length, a mixed-case-and-digits id in between, and the published maximum length.
fn absent_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an id at the minimum modelled length", "a".repeat(30)),
        (
            "an id of mixed case and digits",
            "0123456789abcdefABCDEF0123456789".to_string(),
        ),
        ("an id at the maximum modelled length", "Z".repeat(40)),
    ]
}

/// Every lookup site that answers about `absent` — an identifier nothing is stored
/// under — with `data_set_id` standing in for a data set that DOES exist, so the
/// revision sites are reached.
fn lookups(how: &str, absent: &str, data_set_id: &str) -> Vec<Lookup> {
    let export_action = json!({
        "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
    });
    let mut rows = vec![
        Lookup {
            label: format!("GetDataSet on {how}"),
            method: "GET",
            uri: format!("/v1/data-sets/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("UpdateDataSet on {how}"),
            method: "PATCH",
            uri: format!("/v1/data-sets/{absent}"),
            body: Some(json!({ "Name": "renamed" })),
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("DeleteDataSet on {how}"),
            method: "DELETE",
            uri: format!("/v1/data-sets/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("ListDataSetRevisions on {how}"),
            method: "GET",
            uri: format!("/v1/data-sets/{absent}/revisions"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("CreateRevision under {how}"),
            method: "POST",
            uri: format!("/v1/data-sets/{absent}/revisions"),
            body: Some(json!({ "Comment": "nothing to revise" })),
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("SendDataSetNotification about {how}"),
            method: "POST",
            uri: format!("/v1/data-sets/{absent}/notification"),
            body: Some(json!({
                "Type": "DATA_UPDATE",
                "Details": { "DataUpdate": { "DataUpdatedAt": AN_INSTANT } },
            })),
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("CreateEventAction whose event source is {how}"),
            method: "POST",
            uri: "/v1/event-actions".to_string(),
            body: Some(json!({
                "Event": { "RevisionPublished": { "DataSetId": absent } },
                "Action": export_action,
            })),
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("CreateJob whose details name {how}"),
            method: "POST",
            uri: "/v1/jobs".to_string(),
            body: Some(json!({
                "Type": "EXPORT_ASSET_TO_SIGNED_URL",
                "Details": { "ExportAssetToSignedUrl": {
                    "AssetId": absent,
                    "DataSetId": absent,
                    "RevisionId": absent,
                }},
            })),
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("GetRevision on {how} of a stored data set"),
            method: "GET",
            uri: format!("/v1/data-sets/{data_set_id}/revisions/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(REVISION),
        },
        Lookup {
            label: format!("UpdateRevision on {how} of a stored data set"),
            method: "PATCH",
            uri: format!("/v1/data-sets/{data_set_id}/revisions/{absent}"),
            body: Some(json!({ "Comment": "nothing to comment on" })),
            resource_id: absent.to_string(),
            resource_type: Some(REVISION),
        },
        Lookup {
            label: format!("ListRevisionAssets under {how}"),
            method: "GET",
            uri: format!("/v1/data-sets/{absent}/revisions/{absent}/assets"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(DATA_SET),
        },
        Lookup {
            label: format!("ListRevisionAssets under {how} of a stored data set"),
            method: "GET",
            uri: format!("/v1/data-sets/{data_set_id}/revisions/{absent}/assets"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(REVISION),
        },
        Lookup {
            label: format!("CreateJob whose details name {how} of a stored data set"),
            method: "POST",
            uri: "/v1/jobs".to_string(),
            body: Some(json!({
                "Type": "EXPORT_ASSET_TO_SIGNED_URL",
                "Details": { "ExportAssetToSignedUrl": {
                    "AssetId": absent,
                    "DataSetId": data_set_id,
                    "RevisionId": absent,
                }},
            })),
            resource_id: absent.to_string(),
            resource_type: Some(REVISION),
        },
        Lookup {
            label: format!("GetJob on {how}"),
            method: "GET",
            uri: format!("/v1/jobs/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(JOB),
        },
        Lookup {
            label: format!("StartJob on {how}"),
            method: "PATCH",
            uri: format!("/v1/jobs/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(JOB),
        },
        Lookup {
            label: format!("CancelJob on {how}"),
            method: "DELETE",
            uri: format!("/v1/jobs/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(JOB),
        },
        Lookup {
            label: format!("GetEventAction on {how}"),
            method: "GET",
            uri: format!("/v1/event-actions/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(EVENT_ACTION),
        },
        Lookup {
            label: format!("UpdateEventAction on {how}"),
            method: "PATCH",
            uri: format!("/v1/event-actions/{absent}"),
            body: Some(json!({ "Action": export_action })),
            resource_id: absent.to_string(),
            resource_type: Some(EVENT_ACTION),
        },
        Lookup {
            label: format!("DeleteEventAction on {how}"),
            method: "DELETE",
            uri: format!("/v1/event-actions/{absent}"),
            body: None,
            resource_id: absent.to_string(),
            resource_type: Some(EVENT_ACTION),
        },
    ];

    // The ARN-addressed tagging surface: an ARN that addresses nothing stored names no
    // kind this service can state, so only `ResourceId` is reported there.
    let arn = format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{absent}");
    rows.push(Lookup {
        label: format!("ListTagsForResource on the ARN of {how}"),
        method: "GET",
        uri: format!("/tags/{}", percent_encode(&arn)),
        body: None,
        resource_id: arn.clone(),
        resource_type: None,
    });
    rows.push(Lookup {
        label: format!("TagResource on the ARN of {how}"),
        method: "POST",
        uri: format!("/tags/{}", percent_encode(&arn)),
        body: Some(json!({ "tags": { "team": "data" } })),
        resource_id: arn.clone(),
        resource_type: None,
    });
    rows.push(Lookup {
        label: format!("UntagResource on the ARN of {how}"),
        method: "DELETE",
        uri: format!("/tags/{}?tagKeys=team", percent_encode(&arn)),
        body: None,
        resource_id: arn,
        resource_type: None,
    });
    rows
}

/// RULE: a `ResourceNotFoundException` carries the fixed `Message` "The requested
/// resource was not found." and reports the missing resource in `ResourceId` and
/// `ResourceType` — at every lookup site, for every resource kind, whatever the
/// identifier the request named.
#[tokio::test]
async fn a_not_found_refusal_carries_the_fixed_message_and_the_identity_in_its_members() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "stored", "for the revision sites", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Every `Message` seen, so the sentence can be shown to be the SAME one across
    // kinds, sites and identifiers rather than merely right row by row.
    let mut messages: BTreeSet<String> = BTreeSet::new();
    let mut rows: Vec<Lookup> = Vec::new();
    for (how, absent) in absent_ids() {
        rows.extend(lookups(how, &absent, &data_set_id));
    }
    // `EventActionId` is modelled as a plain `__string` — it carries no pattern, unlike
    // `Id` (`smithy_query(query="operation", operation="GetEventAction")`) — so an id
    // outside that pattern is still a lookup, and still this refusal.
    let unpatterned = "no-such-event-action";
    for (operation, method, body) in [
        ("GetEventAction", "GET", None),
        (
            "UpdateEventAction",
            "PATCH",
            Some(json!({ "Action": {
                "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
            }})),
        ),
        ("DeleteEventAction", "DELETE", None),
    ] {
        rows.push(Lookup {
            label: format!("{operation} on an id outside the modelled Id pattern"),
            method,
            uri: format!("/v1/event-actions/{unpatterned}"),
            body,
            resource_id: unpatterned.to_string(),
            resource_type: Some(EVENT_ACTION),
        });
    }

    for row in &rows {
        let case = &row.label;
        let answer = call(&router, row.method, &row.uri, row.body.clone()).await;
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
        let message = answer.string("Message");
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {case}: expected the fixed refusal message, got {message:?}"
        );
        assert!(
            !message.contains(&row.resource_id),
            "case {case}: expected no request value in the message, got {message:?}"
        );
        assert_eq!(
            answer.member("ResourceId").as_str(),
            Some(row.resource_id.as_str()),
            "case {case}: expected ResourceId to report the resource asked about, got {:?}",
            answer.member("ResourceId")
        );
        assert_eq!(
            answer.member("ResourceType").as_str(),
            row.resource_type,
            "case {case}: expected ResourceType {:?}, got {:?}",
            row.resource_type,
            answer.member("ResourceType")
        );
        messages.insert(message);
    }

    assert_eq!(
        messages.iter().cloned().collect::<Vec<String>>(),
        vec![NOT_FOUND_MESSAGE.to_string()],
        "expected one fixed message across all {} cases, got {messages:?}",
        rows.len()
    );
}

/// RULE: the same fixed `Message` answers a resource that existed and was deleted as
/// answers one that was never created — the sentence reports no history and no
/// identifier, on the data set the delete removed and on the revisions it owned.
#[tokio::test]
async fn a_deleted_resource_is_refused_with_the_same_fixed_message() {
    let router = emulator();

    let surviving = create_data_set(&router, "surviving", "keeps its id", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let surviving_revision = create_revision(&router, &surviving, Some("kept"), None, "setup")
        .await
        .string("Id");
    let doomed = create_data_set(&router, "doomed", "loses its id", "S3_DATA_ACCESS", None)
        .await
        .string("Id");
    let orphaned_revision = create_revision(&router, &doomed, None, None, "setup")
        .await
        .string("Id");

    let deleted = call(&router, "DELETE", &format!("/v1/data-sets/{doomed}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case DeleteDataSet: expected 204 from the delete, got {} body {:?}",
        deleted.status, deleted.body
    );

    // Rows differ along WHICH site now answers about a resource the delete removed:
    // the data set itself, the revision it owned read through its own data set, and
    // the surviving data set asked for the revision it never held.
    let gone: Vec<(&str, String, Option<&str>, String)> = vec![
        (
            "GetDataSet on a deleted data set",
            format!("/v1/data-sets/{doomed}"),
            Some(DATA_SET),
            doomed.clone(),
        ),
        (
            "GetRevision on a revision whose data set was deleted",
            format!("/v1/data-sets/{doomed}/revisions/{orphaned_revision}"),
            Some(DATA_SET),
            doomed.clone(),
        ),
        (
            "GetRevision for a revision of another data set",
            format!("/v1/data-sets/{surviving}/revisions/{orphaned_revision}"),
            Some(REVISION),
            orphaned_revision.clone(),
        ),
    ];

    for (case, uri, resource_type, resource_id) in &gone {
        let answer = call(&router, "GET", uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404 after the delete, got {} body {:?}",
            answer.status, answer.body
        );
        let message = answer.string("Message");
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {case}: expected the fixed refusal message, got {message:?}"
        );
        assert_eq!(
            answer.member("ResourceId").as_str(),
            Some(resource_id.as_str()),
            "case {case}: expected ResourceId to report the deleted resource, got {:?}",
            answer.member("ResourceId")
        );
        assert_eq!(
            answer.member("ResourceType").as_str(),
            *resource_type,
            "case {case}: expected ResourceType {resource_type:?}, got {:?}",
            answer.member("ResourceType")
        );
    }

    // The resource that was NOT deleted still reads back, so the refusals above are
    // about the deleted resources rather than about the store.
    let still_there = call(&router, "GET", &format!("/v1/data-sets/{surviving}"), None).await;
    assert_eq!(
        still_there.status, 200,
        "case the surviving data set: expected 200, got {} body {:?}",
        still_there.status, still_there.body
    );
    let kept = call(
        &router,
        "GET",
        &format!("/v1/data-sets/{surviving}/revisions/{surviving_revision}"),
        None,
    )
    .await;
    assert_eq!(
        kept.status, 200,
        "case the surviving revision: expected 200, got {} body {:?}",
        kept.status, kept.body
    );
}
