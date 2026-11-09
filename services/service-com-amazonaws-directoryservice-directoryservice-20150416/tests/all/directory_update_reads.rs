// SPDX-License-Identifier: CC0-1.0

//! Reading the updates made to a directory's setup, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so nothing here reaches into the state struct
//! and a response that is not computed from persisted state cannot pass.
//!
//! The rules under test, each over a TABLE rather than one recorded call:
//!
//! * a report about a directory RESOLVES it: a `DirectoryId` that satisfies the modelled
//!   pattern and names no stored directory is refused HTTP 400
//!   `DirectoryDoesNotExistException`, with this service's shared sentence for a directory
//!   nothing is stored under and the request-id suffix carrying the id the same answer
//!   reports — whatever `UpdateType` the request asks about;
//! * the refusal is a RESOLUTION failure and comes after the boundary's own constraint
//!   walk, so an `UpdateType` outside the modelled value set is still refused
//!   `ValidationException` first, for a stored directory and an absent one alike;
//! * a directory that resolves is reported from what the store holds for it: a directory
//!   whose setup nobody has updated reports an empty `UpdateActivities` list and no
//!   `NextToken`, for every update type and whether or not a Region is named — and reading
//!   it changes nothing a later caller sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{PAST_ONE_SPAN, REGION};
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// Every value of the modelled `UpdateType` enum
/// (`smithy_query(query="shape",shape="UpdateType")` answers `["OS", "NETWORK", "SIZE"]`),
/// which is what this operation's required `UpdateType` member admits.
const UPDATE_TYPE_CATALOG: [&str; 3] = ["OS", "NETWORK", "SIZE"];

/// Wire values the modelled `UpdateType` does NOT admit: the same letters in another
/// casing, a value of another of this service's enums, and a value of no enum at all.
const UPDATE_TYPES_OUTSIDE_THE_SET: [&str; 4] = ["Os", "os", "Size", "ROTATE"];

/// `UpdateType`'s admissible values as a refusal of this service LISTS them: descending
/// order of the values themselves, which is neither the model's declaration order nor
/// ascending order (`crate::service::stated_enum_values`' rule, pinned for this very enum
/// by `tests/all/refusal_wording.rs`).
const STATED_UPDATE_TYPE_SET: &str = "SIZE, OS, NETWORK";

/// `DirectoryId`s that satisfy the modelled `^d-[0-9a-f]{10}$` and are created in no case
/// here, so the boundary admits them and the handler has to resolve them in the store.
const ABSENT_DIRECTORY_IDS: [&str; 3] = ["d-0123456789", "d-4b7c97286b", "d-ffffffffff"];

/// The `Message` of a refusal body, whichever casing the envelope carries it in: a modelled
/// error of this service reports `Message`, the boundary's `ValidationException` reports
/// `message`.
fn message(parsed: &Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// The sentence this service states for a directory nothing is stored under, with the
/// request id the answer itself reported appended.
fn unknown_directory(directory_id: &str, request_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist. : RequestId: {request_id}")
}

/// A Managed Microsoft AD directory past the stage transition its create owes, so it is a
/// directory in service for the reads the cases make.
async fn directory_in_service(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// The `UpdateActivities` of one read, as a list.
fn activities(answer: &Value, case: &str) -> Vec<Value> {
    answer["UpdateActivities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected UpdateActivities, got {answer}"))
        .clone()
}

#[tokio::test]
async fn an_absent_directory_is_refused_for_every_update_type() {
    let router = service();

    // A directory the store really holds, so a refusal below is about the id the request
    // named rather than about an empty store.
    let stored = directory_in_service(&router, "updates-stored.example.com", "the fixture").await;

    for update_type in UPDATE_TYPE_CATALOG {
        for absent in ABSENT_DIRECTORY_IDS {
            for region in [None, Some(REGION)] {
                let case = format!("{update_type} on {absent} in region {region:?}");
                let mut body = json!({ "DirectoryId": absent, "UpdateType": update_type });
                if let Some(region) = region {
                    body["RegionName"] = json!(region);
                }
                let (status, parsed) = call(&router, "DescribeUpdateDirectory", body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the absent directory to be refused, got {status} \
                     with {parsed}"
                );
                let wire_type = parsed["__type"].as_str().unwrap_or_default();
                assert!(
                    wire_type.contains("DirectoryDoesNotExistException"),
                    "case {case:?}: expected DirectoryDoesNotExistException, got {parsed}"
                );
                let reported = string(&parsed, "RequestId", &case);
                assert_eq!(
                    message(&parsed, &case),
                    unknown_directory(absent, &reported),
                    "case {case:?}: expected this service's unknown-directory sentence, got \
                     {parsed}"
                );
                assert!(
                    parsed["UpdateActivities"].is_null(),
                    "case {case:?}: expected no activities on a refusal, got {parsed}"
                );
            }
        }

        // The branch follows stored state: the id the store DOES hold is not refused by it.
        let case = format!("{update_type} on the stored directory");
        let answer = ok(
            &router,
            "DescribeUpdateDirectory",
            json!({ "DirectoryId": &stored, "UpdateType": update_type }),
            &case,
        )
        .await;
        assert!(
            answer["Message"].is_null(),
            "case {case:?}: expected no refusal diagnostic on a served read, got {answer}"
        );
    }
}

#[tokio::test]
async fn an_update_type_outside_the_value_set_is_refused_before_the_directory_is_resolved() {
    let router = service();
    let stored = directory_in_service(&router, "updates-ordering.example.com", "the fixture").await;

    for update_type in UPDATE_TYPES_OUTSIDE_THE_SET {
        // The same answer for a directory the store holds and for one it does not: the
        // boundary's constraint walk runs before anything is resolved, so the two legs
        // cannot be told apart by it.
        for (subject, directory_id) in [
            ("an absent directory", ABSENT_DIRECTORY_IDS[1].to_string()),
            ("a stored directory", stored.clone()),
        ] {
            let case = format!("{update_type:?} against {subject}");
            let (status, parsed) = call(
                &router,
                "DescribeUpdateDirectory",
                json!({ "DirectoryId": &directory_id, "UpdateType": update_type }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the wire value to be refused, got {status} with \
                 {parsed}"
            );
            assert_eq!(
                parsed["__type"],
                json!("ValidationException"),
                "case {case:?}: expected the boundary's validation envelope, got {parsed}"
            );
            assert_eq!(
                message(&parsed, &case),
                format!(
                    "1 validation error detected: Value '{update_type}' at 'updateType' failed \
                     to satisfy constraint: Member must satisfy enum value set: \
                     [{STATED_UPDATE_TYPE_SET}]"
                ),
                "case {case:?}: expected the enum clause rather than a resolution error, got \
                 {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn a_directory_reports_the_update_activities_it_holds() {
    let router = service();
    let directory = directory_in_service(&router, "updates-read.example.com", "the fixture").await;

    for update_type in UPDATE_TYPE_CATALOG {
        // The Region narrows the reads; a request naming none reads every Region the
        // directory is updated in. Neither can report an activity nothing recorded.
        for region in [None, Some(REGION), Some("eu-west-1")] {
            let case = format!("{update_type} in region {region:?}");
            let mut body = json!({ "DirectoryId": &directory, "UpdateType": update_type });
            if let Some(region) = region {
                body["RegionName"] = json!(region);
            }
            let answer = ok(&router, "DescribeUpdateDirectory", body, &case).await;
            assert!(
                activities(&answer, &case).is_empty(),
                "case {case:?}: expected no activity for a directory whose setup nobody has \
                 updated, got {answer}"
            );
            assert!(
                answer["NextToken"].is_null(),
                "case {case:?}: expected no page token on a final page, got {answer}"
            );
        }
    }

    // The read is a REPORT: it changes nothing a later caller sees, neither its own answer
    // nor the directory the stage transition already settled.
    let case = "a repeated read";
    let first = ok(
        &router,
        "DescribeUpdateDirectory",
        json!({ "DirectoryId": &directory, "UpdateType": "OS" }),
        case,
    )
    .await;
    let second = ok(
        &router,
        "DescribeUpdateDirectory",
        json!({ "DirectoryId": &directory, "UpdateType": "OS" }),
        case,
    )
    .await;
    assert_eq!(
        first, second,
        "case {case:?}: expected the second read to answer exactly as the first, got {first} \
         then {second}"
    );
    let described = ok(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [&directory] }),
        case,
    )
    .await;
    assert_eq!(
        described["DirectoryDescriptions"][0]["Stage"],
        json!(crate::directory_lifecycle::STAGE_ACTIVE),
        "case {case:?}: expected the reads to leave the directory as it was, got {described}"
    );
}
