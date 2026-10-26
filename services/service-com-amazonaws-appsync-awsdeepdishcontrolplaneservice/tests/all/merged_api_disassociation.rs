// SPDX-License-Identifier: CC0-1.0

//! `DisassociateMergedGraphqlApi` end to end over HTTP: the deletion of a source-API
//! association, asked for from the SOURCE API's side and read back through
//! `GetSourceApiAssociation`.
//!
//! The rules asserted here, each over a TABLE or a class of inputs rather than over a
//! single request:
//!
//! 1. the call SCHEDULES the deletion of the association the path addresses, so it
//!    answers the non-terminal `DELETION_SCHEDULED` status and the association is still
//!    readable in that status straight afterwards — whichever side created the link,
//!    whichever accepted identifier form (`apiId` or ARN) names the source API, whatever
//!    `MergeType` the link carries, and whether or not its own scheduled merge was still
//!    owed when the deletion was asked for;
//! 2. one association is addressed from EITHER of its two APIs, so scheduling is
//!    idempotent across the two disassociates too: repeating the call from the source
//!    side, and asking the Merged API's side for the same association, answer the same
//!    status and leave the same one association;
//! 3. the deletion is owed by the CLOCK alone: once its due instant has passed, with no
//!    call in between, the association is gone — the reader answers 404, a further
//!    deletion has nothing to address, and the pair is free to be associated again;
//! 4. an identifier that cannot name an API is a 400 `BadRequestException`, one that
//!    names no API this account holds a 404 `NotFoundException`, an `associationId` that
//!    is no reference the service could have issued a 400, and one of the issued shape
//!    that names no association OF the source API addressed — including the association's
//!    own id addressed by its MERGED API, which is the other side's path — a 404.
//!
//! The harness (`router`, `call`, `create_api`, `create_pair`, `get_path`) is the one
//! the association suites already carry, so this suite drives the same router the
//! production emulator mounts: routing, restJson1 (de)serialization, the handlers and
//! the shared engine together.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{
    associate_path as source_side_path, call, create_api, create_pair, get_path, router, Api,
    PAST_THE_MERGE,
};

/// How long a row that must observe the far side of the scheduled deletion waits: the
/// service's own span plus slack. Only the TEST waits — the handler stores the due
/// instant.
const PAST_THE_DELETION: Duration = Duration::from_millis(250);

/// `AssociateSourceGraphqlApi`'s path, i.e. the association collection of a Merged API.
fn merged_side_path(merged_api_identifier: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations")
}

/// `DisassociateMergedGraphqlApi`'s path: the association addressed from its source API.
fn disassociate_path(source_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/sourceApis/{source_api_identifier}/mergedApiAssociations/{association_id}")
}

/// A value in a NON-GREEDY `@httpLabel`, as an SDK client puts it on the wire: the
/// segment's reserved characters are percent-encoded, so an ARN's `:` and `/` arrive as
/// `%3A` and `%2F` and cannot be mistaken for path structure. The framework
/// percent-decodes the label before the handler sees it.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One row of the deletion table: how the association under test was created and how
/// the deletion is asked for.
#[derive(Debug)]
struct Case {
    /// The row's label, used both in the API names and in every assertion message.
    label: &'static str,
    /// Whether the link was created from the SOURCE API's side
    /// (`AssociateMergedGraphqlApi`) rather than from the Merged API's: AppSync creates
    /// one and the same association either way, so either is the same association to
    /// delete.
    created_from_the_source_side: bool,
    /// Whether the deletion names the source API by its ARN rather than by its `apiId` —
    /// the two forms `sourceApiIdentifier` documents.
    source_by_arn: bool,
    /// The `mergeType` the create named, or `None` when it named none and the
    /// association took the modelled default.
    merge_type: Option<&'static str>,
    /// Whether the row waits for the association's own scheduled merge to come due
    /// before asking for the deletion.
    past_the_merge: bool,
    /// How many times the row asks for the deletion from the SOURCE side: while it is
    /// still owed, every call answers the same scheduled deletion.
    calls: usize,
    /// Whether the row also asks the MERGED API's side (`DisassociateSourceGraphqlApi`)
    /// for the same association, which addresses the very same link.
    also_from_the_merged_side: bool,
}

/// The deletion table: both sides a link can be created from, both identifier forms the
/// deletion can name its source API by, the `MergeType` value set (source:
/// `smithy_query(query="shape", shape="SourceApiAssociationConfig")` — "Valid merge types
/// are `MANUAL_MERGE` (default) and `AUTO_MERGE`") plus the request that names none, a
/// deletion asked for while the merge is still owed and one asked for past it, a single
/// call against a repeated one, and the same association asked for from the other side.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "source side, by api id, default merge type, merge still owed",
            created_from_the_source_side: true,
            source_by_arn: false,
            merge_type: None,
            past_the_merge: false,
            calls: 1,
            also_from_the_merged_side: false,
        },
        Case {
            label: "merged side, by arn, auto merge, merge already performed",
            created_from_the_source_side: false,
            source_by_arn: true,
            merge_type: Some("AUTO_MERGE"),
            past_the_merge: true,
            calls: 1,
            also_from_the_merged_side: false,
        },
        Case {
            label: "source side, by arn, manual merge, repeated deletion past the merge",
            created_from_the_source_side: true,
            source_by_arn: true,
            merge_type: Some("MANUAL_MERGE"),
            past_the_merge: true,
            calls: 2,
            also_from_the_merged_side: false,
        },
        Case {
            label: "merged side, by api id, default merge type, both sides ask while merge owed",
            created_from_the_source_side: false,
            source_by_arn: false,
            merge_type: None,
            past_the_merge: false,
            calls: 3,
            also_from_the_merged_side: true,
        },
    ]
}

/// Create the row's association and answer its `associationId`.
async fn associate(router: &MultiServiceRouter, case: &Case, source: &Api, merged: &Api) -> String {
    let (path, mut body) = if case.created_from_the_source_side {
        (
            source_side_path(&source.id),
            json!({"mergedApiIdentifier": merged.id}),
        )
    } else {
        (
            merged_side_path(&merged.id),
            json!({"sourceApiIdentifier": source.id}),
        )
    };
    if let Some(merge_type) = case.merge_type {
        body["sourceApiAssociationConfig"] = json!({"mergeType": merge_type});
    }

    let created = call(router, "POST", &path, Some(body)).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the association to be created, got {} {:?}",
        case.label, created.status, created.body
    );
    created.body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_disassociate_from_the_source_side_schedules_the_deletion_the_clock_then_performs() {
    let router = router();

    for case in cases() {
        let (source, merged) = create_pair(&router, case.label).await;
        let association_id = associate(&router, &case, &source, &merged).await;
        // The source API named the way the row asks for it: either accepted identifier
        // form addresses the same API, so either must address the same association.
        let source_identifier = if case.source_by_arn {
            label(&source.arn)
        } else {
            source.id.clone()
        };
        let path = disassociate_path(&source_identifier, &association_id);
        // The association's reader takes the Merged API's path: the deletion addresses
        // the very resource `GetSourceApiAssociation` reports.
        let read_path = get_path(&merged.id, &association_id);

        if case.past_the_merge {
            tokio::time::sleep(state::MERGE_SPAN + PAST_THE_MERGE).await;
        }

        // Asking for the deletion SCHEDULES it, and asking again while it is still owed
        // schedules nothing more: every call answers the same non-terminal status.
        for attempt in 1..=case.calls {
            let deleted = call(&router, "DELETE", &path, None).await;
            assert_eq!(
                deleted.status, 200,
                "case {:?} (call {attempt}): expected DisassociateMergedGraphqlApi 200, got {} \
                 {:?}",
                case.label, deleted.status, deleted.body
            );
            assert_eq!(
                deleted.body["sourceApiAssociationStatus"].as_str(),
                Some(state::DELETION_SCHEDULED),
                "case {:?} (call {attempt}): expected status {:?}, got {:?}",
                case.label,
                state::DELETION_SCHEDULED,
                deleted.body["sourceApiAssociationStatus"]
            );
        }

        // One link, addressed from either of its two APIs: the other side's delete
        // reaches the same association and reports the same scheduled deletion.
        if case.also_from_the_merged_side {
            let from_merged = call(&router, "DELETE", &read_path, None).await;
            assert_eq!(
                (
                    from_merged.status,
                    from_merged.body["sourceApiAssociationStatus"].as_str()
                ),
                (200, Some(state::DELETION_SCHEDULED)),
                "case {:?}: expected the Merged API's side to reach the same scheduled \
                 association, got {} {:?}",
                case.label,
                from_merged.status,
                from_merged.body
            );
        }

        // The deletion is only SCHEDULED, so the association is still there, in the
        // status the call reported and with the detail that states where it stands.
        let read = call(&router, "GET", &read_path, None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the scheduled association to still be readable, got {} {:?}",
            case.label, read.status, read.body
        );
        let association = read.body["sourceApiAssociation"].clone();
        assert_eq!(
            association["sourceApiAssociationStatus"].as_str(),
            Some(state::DELETION_SCHEDULED),
            "case {:?}: expected the reader to report status {:?}, got {:?}",
            case.label,
            state::DELETION_SCHEDULED,
            association["sourceApiAssociationStatus"]
        );
        assert!(
            association["sourceApiAssociationStatusDetail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty()),
            "case {:?}: expected a status detail, got {:?}",
            case.label,
            association["sourceApiAssociationStatusDetail"]
        );
        assert_eq!(
            association["associationId"].as_str(),
            Some(association_id.as_str()),
            "case {:?}: expected the addressed association back, got {:?}",
            case.label,
            association["associationId"]
        );

        // Nothing is called in between: the transition below is owed by the clock.
        tokio::time::sleep(state::DELETION_SPAN + PAST_THE_DELETION).await;

        let gone = call(&router, "GET", &read_path, None).await;
        assert_eq!(
            (gone.status, gone.error_type.as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected the association to be gone past the due instant, got {} {} {:?}",
            case.label,
            gone.status,
            gone.error_type,
            gone.body
        );
        let deleted_again = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            (deleted_again.status, deleted_again.error_type.as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected a deletion of the gone association to be a not-found, got {} {} \
             {:?}",
            case.label,
            deleted_again.status,
            deleted_again.error_type,
            deleted_again.body
        );

        // The link is gone from the store, not merely re-labelled: the pair can be
        // associated again, which a stored association of the same pair refuses.
        let reassociated = call(
            &router,
            "POST",
            &source_side_path(&source.id),
            Some(json!({"mergedApiIdentifier": merged.id})),
        )
        .await;
        assert_eq!(
            reassociated.status, 200,
            "case {:?}: expected the pair to be associable again, got {} {:?}",
            case.label, reassociated.status, reassociated.body
        );
        let fresh_id = reassociated.body["sourceApiAssociation"]["associationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            fresh_id, association_id,
            "case {:?}: expected a fresh associationId for the new association, got the deleted \
             one {association_id:?}",
            case.label
        );
    }
}

/// One refusal row: the request to make and the answer it must take.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    /// `{source}`, `{other}`, `{merged}` and `{association}` are substituted with the
    /// fixture's ids before the call.
    path: &'static str,
    status: u16,
    error_type: &'static str,
    /// Whether the refusal's text is the fixed not-found one rather than a malformed
    /// request's, which names the value it refused.
    not_found_text: bool,
    /// The value a malformed request's text must name, empty on a not-found row.
    refused: &'static str,
}

/// Every way a `DisassociateMergedGraphqlApi` request can address something it cannot
/// delete: an identifier that is no identifier at all (400), one that names nothing this
/// account holds (404), an `associationId` no AppSync service could have issued (400),
/// one of the issued shape this account does not hold (404), an association that belongs
/// to ANOTHER source API (404), and the association addressed by its MERGED API, which is
/// the other disassociate's path rather than this one's (404).
///
/// The over-long identifier is one character past the 26 an AppSync API id has
/// (`state::API_ID_LEN`), the bound `state::is_api_id` publishes; the absent ones are of
/// exactly that length, so they are well-formed values this account simply does not hold.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "source identifier too long to be one",
            path: "/v1/sourceApis/aaaaaaaaaaaaaaaaaaaaaaaaaaa/mergedApiAssociations/{association}",
            status: 400,
            error_type: "BadRequestException",
            not_found_text: false,
            refused: "aaaaaaaaaaaaaaaaaaaaaaaaaaa",
        },
        Refusal {
            label: "source api this account does not hold",
            path: "/v1/sourceApis/abcdefghijklmnopqrstuvwxyz/mergedApiAssociations/{association}",
            status: 404,
            error_type: "NotFoundException",
            not_found_text: true,
            refused: "",
        },
        Refusal {
            label: "association reference the service could not have issued",
            path: "/v1/sourceApis/{source}/mergedApiAssociations/source-api-association-77",
            status: 400,
            error_type: "BadRequestException",
            not_found_text: false,
            refused: "source-api-association-77",
        },
        Refusal {
            label: "association this account does not hold",
            path: "/v1/sourceApis/{source}/mergedApiAssociations/abcdefghijklmnopqrstuvwxyz",
            status: 404,
            error_type: "NotFoundException",
            not_found_text: true,
            refused: "",
        },
        Refusal {
            label: "association of another source api",
            path: "/v1/sourceApis/{other}/mergedApiAssociations/{association}",
            status: 404,
            error_type: "NotFoundException",
            not_found_text: true,
            refused: "",
        },
        Refusal {
            label: "the association's merged api addressed as its source api",
            path: "/v1/sourceApis/{merged}/mergedApiAssociations/{association}",
            status: 404,
            error_type: "NotFoundException",
            not_found_text: true,
            refused: "",
        },
    ]
}

#[tokio::test]
async fn a_deletion_the_request_cannot_address_is_refused() {
    let router = router();
    // The fixtures every row is written against: one association, plus a second source
    // API that does not take part in it.
    let (source, merged) = create_pair(&router, "merged-disassociate-refuse").await;
    let other = create_api(
        &router,
        "merged-disassociate-refuse-other-source",
        state::GRAPHQL_API_TYPE,
    )
    .await;
    let association_id = associate(
        &router,
        &Case {
            label: "merged-disassociate-refuse",
            created_from_the_source_side: true,
            source_by_arn: false,
            merge_type: None,
            past_the_merge: false,
            calls: 1,
            also_from_the_merged_side: false,
        },
        &source,
        &merged,
    )
    .await;

    for refusal in refusals() {
        let path = refusal
            .path
            .replace("{source}", &source.id)
            .replace("{other}", &other.id)
            .replace("{merged}", &merged.id)
            .replace("{association}", &association_id);
        let answer = call(&router, "DELETE", &path, None).await;
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
        if refusal.not_found_text {
            // A missing resource names no identifier at all, so the same fixed text
            // answers whichever side of the request was the one that resolved to nothing.
            assert!(
                answer.body["message"].as_str().is_some_and(|text| {
                    text == state::API_NOT_FOUND_MESSAGE
                        || text == state::ASSOCIATION_NOT_FOUND_MESSAGE
                }),
                "case {:?}: expected one of the rule's not-found texts ({:?}, {:?}), got {:?}",
                refusal.label,
                state::API_NOT_FOUND_MESSAGE,
                state::ASSOCIATION_NOT_FOUND_MESSAGE,
                answer.body["message"]
            );
        } else {
            // A malformed request names the value that was refused, which is what keeps
            // the two answers apart.
            assert!(
                answer.body["message"]
                    .as_str()
                    .is_some_and(|text| text.ends_with(refusal.refused)),
                "case {:?}: expected the refusal to end with the rejected value {:?}, got {:?}",
                refusal.label,
                refusal.refused,
                answer.body["message"]
            );
        }
    }

    // None of the refused calls touched the association: it is still readable, and still
    // in the status its create left it in.
    let read = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
    assert_eq!(
        read.body["sourceApiAssociation"]["sourceApiAssociationStatus"].as_str(),
        Some(state::MERGE_SCHEDULED),
        "case refused: expected the untouched association in status {:?}, got {:?}",
        state::MERGE_SCHEDULED,
        read.body["sourceApiAssociation"]
    );
}
