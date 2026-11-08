// SPDX-License-Identifier: CC0-1.0

//! `UpdateSourceApiAssociation` end to end over HTTP: writing the configuration choices
//! of a stored source-API association, and the refusals a request that names no
//! association of the addressed Merged API meets.
//!
//! Three rules are asserted here, each over a TABLE or a class of inputs rather than
//! over the single request that exposed the defect:
//!
//! 1. an update REPLACES the association's configuration choices with the ones the
//!    request names — a member it omits is gone from the association, and an omitted
//!    `mergeType` leaves it on the modelled default — while everything the caller does
//!    not configure (both sides' ids and ARNs, the association id and ARN, the status,
//!    its detail and the merge date) is left exactly as stored, and the answer is the
//!    body `GetSourceApiAssociation` reports next. Asserted over a table spanning both
//!    `MergeType` values, a config object without a `mergeType`, no config at all,
//!    `description` present, empty, non-ASCII and absent, and associations that were
//!    created with and without either choice;
//! 2. an `associationId` that is not a reference the service could have issued is
//!    refused 400 `BadRequestException` naming the rejected id, while a well-formed one
//!    that resolves to no association of the addressed Merged API is the absent
//!    association it is — 404 `NotFoundException` with the fixed text; an identifier that
//!    cannot name an API at all is a 400, and a Merged API this account does not hold a
//!    404 — over a table spanning an id never minted in either issued form, a reference
//!    of no issued form at all, a real association addressed under another Merged API and
//!    under a standard one, an over-long identifier and an API id nothing holds. A
//!    refused update writes nothing;
//! 3. the update configures the association and nothing else: it neither performs nor
//!    cancels the merge the clock owes, so that merge still comes due with no further
//!    call, and an association whose DELETION the clock owes is being destroyed, so an
//!    update of it is refused 409 `ConcurrentModificationException` and the deletion
//!    the refusal left alone still comes due.
//!
//! The harness (`router`, `call`, `create_api`, `create_pair`, `get_path`) is the one
//! the association suites already carry, so every assertion goes through the router the
//! production emulator mounts: routing, restJson1 (de)serialization, the handlers and
//! the shared engine together.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{
    associate_path, call, create_api, create_pair, get_path, router, Answer, PAST_THE_MERGE,
};

/// Every value of the `MergeType` enum — the whole set a `sourceApiAssociationConfig`
/// can ask an association to merge under.
///
/// Source: `smithy_query(query="shape", shape="MergeType")`, whose documentation names
/// `MANUAL_MERGE` the default of the two.
const MERGE_TYPES: &[&str] = &["MANUAL_MERGE", "AUTO_MERGE"];

/// A description that is not ASCII and carries punctuation, so the row spans the FORM a
/// description takes rather than only its presence.
const UNICODE_DESCRIPTION: &str = "Ventas – producción ⚙";

/// One row of the update table: what the association was created with, what the update
/// asks for, and what the association must then carry.
#[derive(Debug)]
struct Case {
    /// The row's label, used both in the API names and in every assertion message.
    label: &'static str,
    /// The `description` the CREATE asked for, if any.
    created_description: Option<&'static str>,
    /// The `mergeType` the CREATE's `sourceApiAssociationConfig` asked for, if any.
    created_merge_type: Option<&'static str>,
    /// The `description` the UPDATE asks for, or `None` when it names none.
    description: Option<&'static str>,
    /// The `sourceApiAssociationConfig` the UPDATE asks for, or `None` when it names
    /// none.
    config: Option<Value>,
    /// The `description` the association must then report (`Value::Null` when it has
    /// none left).
    expected_description: Value,
    /// The `mergeType` the association must then report.
    expected_merge_type: &'static str,
}

/// The update table: both `MergeType` values, a config object that names no `mergeType`
/// and a request that names no config at all, a description set, replaced, emptied,
/// written in another script and dropped, over associations created with and without
/// either choice.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "sets both choices on an association created with neither",
            created_description: None,
            created_merge_type: None,
            description: Some("the inventory source api"),
            config: Some(json!({"mergeType": "AUTO_MERGE"})),
            expected_description: json!("the inventory source api"),
            expected_merge_type: "AUTO_MERGE",
        },
        Case {
            label: "replaces both choices",
            created_description: Some("the first description"),
            created_merge_type: Some("AUTO_MERGE"),
            description: Some("the second description"),
            config: Some(json!({"mergeType": "MANUAL_MERGE"})),
            expected_description: json!("the second description"),
            expected_merge_type: "MANUAL_MERGE",
        },
        Case {
            label: "a description the request omits is gone",
            created_description: Some("the first description"),
            created_merge_type: Some("AUTO_MERGE"),
            description: None,
            config: Some(json!({"mergeType": "AUTO_MERGE"})),
            expected_description: Value::Null,
            expected_merge_type: "AUTO_MERGE",
        },
        Case {
            label: "a config the request omits leaves the modelled default",
            created_description: Some("the first description"),
            created_merge_type: Some("AUTO_MERGE"),
            description: Some("kept"),
            config: None,
            expected_description: json!("kept"),
            expected_merge_type: state::DEFAULT_MERGE_TYPE,
        },
        Case {
            label: "a config without a mergeType leaves the modelled default",
            created_description: None,
            created_merge_type: Some("AUTO_MERGE"),
            description: Some("kept"),
            config: Some(json!({})),
            expected_description: json!("kept"),
            expected_merge_type: state::DEFAULT_MERGE_TYPE,
        },
        Case {
            label: "an empty description is a description",
            created_description: Some("the first description"),
            created_merge_type: None,
            description: Some(""),
            config: Some(json!({"mergeType": "AUTO_MERGE"})),
            expected_description: json!(""),
            expected_merge_type: "AUTO_MERGE",
        },
        Case {
            label: "a description outside ASCII",
            created_description: None,
            created_merge_type: Some("MANUAL_MERGE"),
            description: Some(UNICODE_DESCRIPTION),
            config: Some(json!({"mergeType": "AUTO_MERGE"})),
            expected_description: json!(UNICODE_DESCRIPTION),
            expected_merge_type: "AUTO_MERGE",
        },
        Case {
            label: "an update that asks for nothing clears both choices",
            created_description: Some("the first description"),
            created_merge_type: Some("AUTO_MERGE"),
            description: None,
            config: None,
            expected_description: Value::Null,
            expected_merge_type: state::DEFAULT_MERGE_TYPE,
        },
    ]
}

/// The body an update asks with, carrying only the members the row names.
fn update_body(description: Option<&str>, config: Option<Value>) -> Value {
    let mut body = json!({});
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if let Some(config) = config {
        body["sourceApiAssociationConfig"] = config;
    }
    body
}

/// Associate the pair `label` names, with the choices the create asks for, and return
/// the Merged API id and the association id.
async fn associate(
    router: &MultiServiceRouter,
    label: &str,
    description: Option<&str>,
    merge_type: Option<&str>,
) -> (String, String) {
    let (source, merged) = create_pair(router, label).await;
    let mut body = json!({"mergedApiIdentifier": merged.id});
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if let Some(merge_type) = merge_type {
        body["sourceApiAssociationConfig"] = json!({"mergeType": merge_type});
    }
    let created = call(router, "POST", &associate_path(&source.id), Some(body)).await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: expected the association fixture to be created, got {} {:?}",
        created.status, created.body
    );
    let association_id = created.body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (merged.id, association_id)
}

/// Read the association back, asserting the read itself succeeded.
async fn read(
    router: &MultiServiceRouter,
    label: &str,
    merged_api_id: &str,
    association_id: &str,
) -> Value {
    let answer = call(
        router,
        "GET",
        &get_path(merged_api_id, association_id),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected GetSourceApiAssociation 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["sourceApiAssociation"].clone()
}

/// The members an update does not configure, which it therefore has to leave alone.
const UNCONFIGURED_MEMBERS: &[&str] = &[
    "associationId",
    "associationArn",
    "sourceApiId",
    "sourceApiArn",
    "mergedApiId",
    "mergedApiArn",
    "sourceApiAssociationStatus",
    "sourceApiAssociationStatusDetail",
    "lastSuccessfulMergeDate",
];

#[tokio::test]
async fn an_update_replaces_the_configuration_choices_and_leaves_the_rest() {
    let router = router();

    for case in cases() {
        let (merged_api_id, association_id) = associate(
            &router,
            case.label,
            case.created_description,
            case.created_merge_type,
        )
        .await;

        // Let the merge the create scheduled come due first, so the association sits in
        // a terminal state: every difference the update then makes is the update's own,
        // and none of it is the clock's.
        tokio::time::sleep(state::MERGE_SPAN + PAST_THE_MERGE).await;
        let before = read(&router, case.label, &merged_api_id, &association_id).await;
        assert_eq!(
            before["sourceApiAssociationStatus"].as_str(),
            Some(state::MERGE_SUCCESS),
            "case {:?}: expected the create's merge to have been performed, got {:?}",
            case.label,
            before["sourceApiAssociationStatus"]
        );

        let updated = call(
            &router,
            "POST",
            // The update takes the SAME URI as the read (the model binds both to
            // /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId});
            // only the method differs, so the read's helper builds it.
            &get_path(&merged_api_id, &association_id),
            Some(update_body(case.description, case.config.clone())),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected UpdateSourceApiAssociation 200, got {} {:?}",
            case.label, updated.status, updated.body
        );
        let association = updated.body["sourceApiAssociation"].clone();

        assert_eq!(
            association["description"], case.expected_description,
            "case {:?}: expected the description {:?}, got {:?}",
            case.label, case.expected_description, association["description"]
        );
        let merge_type = association["sourceApiAssociationConfig"]["mergeType"].clone();
        assert_eq!(
            merge_type,
            json!(case.expected_merge_type),
            "case {:?}: expected the mergeType {:?}, got {merge_type:?}",
            case.label,
            case.expected_merge_type
        );
        assert!(
            merge_type
                .as_str()
                .is_some_and(|value| MERGE_TYPES.contains(&value)),
            "case {:?}: expected a modelled MergeType out of {MERGE_TYPES:?}, got {merge_type:?}",
            case.label
        );

        // Nothing the caller did not configure moved: the association is the same
        // resource, in the same state, with the same merge behind it.
        for member in UNCONFIGURED_MEMBERS {
            assert_eq!(
                association[member], before[member],
                "case {:?}: expected {member} to be left at {:?}, got {:?}",
                case.label, before[member], association[member]
            );
        }

        // The answer is the STORED association: the read reports the same body, and a
        // repeated identical update is the same write again, so it reports it too.
        let stored = read(&router, case.label, &merged_api_id, &association_id).await;
        assert_eq!(
            stored, association,
            "case {:?}: expected the read to report the updated association {association:?}, got \
             {stored:?}",
            case.label
        );

        let again = call(
            &router,
            "POST",
            &get_path(&merged_api_id, &association_id),
            Some(update_body(case.description, case.config.clone())),
        )
        .await;
        assert_eq!(
            (again.status, again.body["sourceApiAssociation"].clone()),
            (200, association.clone()),
            "case {:?}: expected the same update to leave the same association {association:?}, \
             got {} {:?}",
            case.label,
            again.status,
            again.body
        );
    }
}

#[tokio::test]
async fn an_update_leaves_the_merge_the_clock_owes_where_it_was() {
    let router = router();
    let label = "owed-merge";
    let (merged_api_id, association_id) = associate(&router, label, None, Some("AUTO_MERGE")).await;

    // The update arrives while the merge the create scheduled is still owed. It
    // configures the association and nothing else, so the status it answers is one the
    // service's own state machine is in, and the merge still comes due afterwards with
    // no further call.
    let updated = call(
        &router,
        "POST",
        &get_path(&merged_api_id, &association_id),
        Some(update_body(Some("configured while merging"), None)),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {label:?}: expected UpdateSourceApiAssociation 200 while the merge is owed, got {} \
         {:?}",
        updated.status, updated.body
    );
    let status = updated.body["sourceApiAssociation"]["sourceApiAssociationStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        [state::MERGE_SCHEDULED, state::MERGE_SUCCESS].contains(&status.as_str()),
        "case {label:?}: expected the status the create's merge left ({:?}) or the one the clock \
         carried it to ({:?}), got {status:?}",
        state::MERGE_SCHEDULED,
        state::MERGE_SUCCESS
    );

    tokio::time::sleep(state::MERGE_SPAN + PAST_THE_MERGE).await;
    let settled = read(&router, label, &merged_api_id, &association_id).await;
    assert_eq!(
        settled["sourceApiAssociationStatus"].as_str(),
        Some(state::MERGE_SUCCESS),
        "case {label:?}: expected the owed merge to come due past its span, got {:?}",
        settled["sourceApiAssociationStatus"]
    );
    assert!(
        settled["lastSuccessfulMergeDate"]
            .as_f64()
            .is_some_and(|epoch| epoch > 0.0),
        "case {label:?}: expected a merge date once the owed merge ran, got {:?}",
        settled["lastSuccessfulMergeDate"]
    );
    // The configuration the update wrote survived the clock's own transition.
    assert_eq!(
        settled["description"].as_str(),
        Some("configured while merging"),
        "case {label:?}: expected the description the update wrote, got {:?}",
        settled["description"]
    );
}

/// `DisassociateSourceGraphqlApi`'s path — the call that schedules the association's
/// deletion, which an update that arrives afterwards has to reckon with.
fn disassociate_path(merged_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}")
}

#[tokio::test]
async fn an_update_of_an_association_being_deleted_is_refused() {
    let router = router();
    let label = "deleting";
    let (merged_api_id, association_id) = associate(&router, label, None, None).await;

    let deleted = call(
        &router,
        "DELETE",
        &disassociate_path(&merged_api_id, &association_id),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case {label:?}: expected DisassociateSourceGraphqlApi 200, got {} {:?}",
        deleted.status, deleted.body
    );

    // While the deletion the call scheduled is still owed, the association is being
    // destroyed: that is a modification in progress, so an update of it is refused with
    // the status the model gives that error and the text that scopes the conflict to the
    // Merged API whose associations are being updated.
    let refused = call(
        &router,
        "POST",
        &get_path(&merged_api_id, &association_id),
        Some(update_body(Some("too late"), None)),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_type.as_str()),
        (409, "ConcurrentModificationException"),
        "case {label:?}: expected a 409 ConcurrentModificationException while the deletion is \
         owed, got {} {} {:?}",
        refused.status,
        refused.error_type,
        refused.body
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some(state::associations_being_updated_message(&merged_api_id).as_str()),
        "case {label:?}: expected the association-scoped concurrent-modification text, got {:?}",
        refused.body["message"]
    );

    // The refusal left the deletion where it was, so the clock still carries it out —
    // and the association the update could not configure stops existing, which the next
    // update of the same reference reports as the absent association it now is.
    tokio::time::sleep(state::DELETION_SPAN + PAST_THE_MERGE).await;
    let gone = call(
        &router,
        "POST",
        &get_path(&merged_api_id, &association_id),
        Some(update_body(Some("later still"), None)),
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_type.as_str()),
        (404, "NotFoundException"),
        "case deleted: expected a 404 NotFoundException once the deletion came due, got {} {} \
         {:?}",
        gone.status,
        gone.error_type,
        gone.body
    );
    assert_eq!(
        gone.body["message"].as_str(),
        Some(state::ASSOCIATION_NOT_FOUND_MESSAGE),
        "case deleted: expected the fixed not-found text, got {:?}",
        gone.body["message"]
    );
}

/// One refusal row: the pair to address and the answer it must take.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    /// `{merged}`, `{other}`, `{graphql}`, `{association}` are substituted with the
    /// fixture's ids before the call.
    merged_api_identifier: &'static str,
    association_id: &'static str,
    status: u16,
    error_type: &'static str,
    /// The text the refusal must carry, written out here rather than read off the
    /// service's own constant, so a change of wording is caught rather than followed.
    /// `{association}` is substituted the same way.
    message: Option<&'static str>,
}

/// Every way an update can name no association: an `associationId` that was never
/// minted — in each form the service issues one in — and a real association addressed
/// under the wrong Merged API, whether another Merged API or a standard one, each the
/// absent association it is; a reference of no issued form at all, which is a bad
/// request naming the rejected value; an identifier that can name no API at all, also a
/// bad request; and a Merged API this account does not hold, which is the absent
/// resource every operation reports as one.
///
/// The over-long identifier is one character past the 26 an AppSync API id has
/// (`state::API_ID_LEN`), the bound `state::is_api_id` publishes.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "association id never minted, opaque form",
            merged_api_identifier: "{merged}",
            association_id: "abcdefghijklmnopqrstuvwxyz",
            status: 404,
            error_type: "NotFoundException",
            message: Some("No source API association was found"),
        },
        Refusal {
            label: "association id never minted, uuid form",
            merged_api_identifier: "{merged}",
            association_id: "3f1b9c7e-5a24-4d8f-9b61-0c2e7d84af53",
            status: 404,
            error_type: "NotFoundException",
            message: Some("No source API association was found"),
        },
        Refusal {
            label: "association id of no form the service issues",
            merged_api_identifier: "{merged}",
            association_id: "assoc-does-not-exist-1234",
            status: 400,
            error_type: "BadRequestException",
            message: Some("Invalid SourceApi Association ID assoc-does-not-exist-1234"),
        },
        Refusal {
            label: "a real association under another merged api",
            merged_api_identifier: "{other}",
            association_id: "{association}",
            status: 404,
            error_type: "NotFoundException",
            message: Some("No source API association was found"),
        },
        Refusal {
            label: "a real association under a standard api",
            merged_api_identifier: "{graphql}",
            association_id: "{association}",
            status: 404,
            error_type: "NotFoundException",
            message: Some("No source API association was found"),
        },
        Refusal {
            label: "merged identifier too long to be one",
            merged_api_identifier: "aaaaaaaaaaaaaaaaaaaaaaaaaaa",
            association_id: "{association}",
            status: 400,
            error_type: "BadRequestException",
            message: Some("Invalid API ID aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        },
        Refusal {
            label: "merged api this account does not hold",
            merged_api_identifier: "abcdefghijklmnopqrstuvwxyz",
            association_id: "{association}",
            status: 404,
            error_type: "NotFoundException",
            message: Some("API not found."),
        },
    ]
}

#[tokio::test]
async fn a_pair_naming_no_association_is_refused_and_writes_nothing() {
    let router = router();
    let label = "refuse";
    let (merged_api_id, association_id) =
        associate(&router, label, Some("the stored description"), Some("AUTO_MERGE")).await;
    // The fixtures the rows are written against: a second Merged API that holds no
    // association, and a standard API that can hold none at all.
    let other = create_api(&router, "refuse-other-merged", state::MERGED_API_TYPE).await;
    let graphql = create_api(&router, "refuse-standard", state::GRAPHQL_API_TYPE).await;

    // Let the merge the create scheduled come due first, so the association sits in a
    // state a refused call could only change by writing to it.
    tokio::time::sleep(state::MERGE_SPAN + PAST_THE_MERGE).await;
    let before = read(&router, label, &merged_api_id, &association_id).await;

    let fill = |template: &str| {
        template
            .replace("{merged}", &merged_api_id)
            .replace("{other}", &other.id)
            .replace("{graphql}", &graphql.id)
            .replace("{association}", &association_id)
    };

    for refusal in refusals() {
        let answer: Answer = call(
            &router,
            "POST",
            &get_path(
                &fill(refusal.merged_api_identifier),
                &fill(refusal.association_id),
            ),
            Some(update_body(
                Some("a description nothing may keep"),
                Some(json!({"mergeType": "MANUAL_MERGE"})),
            )),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (refusal.status, refusal.error_type),
            "case {:?}: expected {} {}, got {} {} {:?}",
            refusal.label,
            refusal.status,
            refusal.error_type,
            answer.status,
            answer.error_type,
            answer.body
        );
        match refusal.message {
            Some(message) => {
                let expected = fill(message);
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(expected.as_str()),
                    "case {:?}: expected the message {expected:?}, got {:?}",
                    refusal.label,
                    answer.body["message"]
                );
            }
            None => assert!(
                answer.body["message"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {:?}: expected a message stating why, got {:?}",
                refusal.label,
                answer.body["message"]
            ),
        }
    }

    // A refused update wrote nothing: the association is where the clock left it,
    // description, merge type and all.
    let after = read(&router, label, &merged_api_id, &association_id).await;
    assert_eq!(
        after, before,
        "case refused writes nothing: expected the association to be untouched ({before:?}), got \
         {after:?}"
    );
}
