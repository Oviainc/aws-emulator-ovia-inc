// SPDX-License-Identifier: CC0-1.0

//! What the merged-API association surface REFUSES, end to end over HTTP.
//!
//! Four rules, each asserted over a TABLE or a class of inputs rather than over the one
//! request that exposed it:
//!
//! 1. a pair is associated at most once, so a create that finds the pair already
//!    associated is refused 400 `BadRequestException` with the duplicate sentence —
//!    whichever side the request addresses, whichever identifier form it names the other
//!    side by, and whatever the clock still owes on the association it found (a create
//!    never answers the retryable conflict for a request that can never succeed);
//! 2. a source API belongs to ONE Merged API, so a create that would give it a second
//!    one is refused 400 `BadRequestException` with the multiple-Merged-APIs sentence —
//!    again from either side and in either identifier form — and the same create succeeds
//!    once the first association is gone, so the refusal follows the stored topology
//!    rather than the request;
//! 3. an `associationId` is split on its FORM by every operation that addresses an
//!    association by one: a value of no form the service issues is a 400
//!    `BadRequestException` naming the rejected value, while a well-formed one that names
//!    no association of the API addressed — never issued, issued for another resource, or
//!    issued and since removed — is 404 `NotFoundException` with the fixed text;
//! 4. an association cannot outlive either of its APIs, so a `DeleteGraphqlApi` naming an
//!    API that is still a SOURCE API of a Merged API is refused 400 `BadRequestException`
//!    with the fixed sentence, while the Merged API side and an API in no association are
//!    deleted — and once the source API has been disassociated its deletion goes through,
//!    which `GetGraphqlApi` then reports as the absent API it is.
//!
//! The harness (`router`, `call`, `create_api`, `create_pair`, `get_path`) is the one the
//! association suites already carry, so every row drives the router the production
//! emulator mounts: routing, restJson1 (de)serialization, the handlers and the shared
//! engine together.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{
    call, create_api, create_pair, get_path, router, Answer, Api,
};

/// How long a row that must observe the far side of a scheduled span waits: the
/// service's own span plus slack. Only the TEST waits — the handlers store the due
/// instant.
const PAST_THE_SPAN: Duration = Duration::from_millis(250);

/// The fixed sentence a create answers for a pair that is already associated.
///
/// Written out here rather than read off the service's own constant, so a change of
/// wording is caught rather than followed.
const ALREADY_EXISTS: &str = "SourceApiAssociation already exists.";

/// The fixed sentence a create answers for a source API that already has a Merged API.
const MULTIPLE_MERGED_APIS: &str = "Source Api can not be associated with multiple Merged APIs.";

/// The fixed sentence `DeleteGraphqlApi` answers for an API that is still a source API
/// of a Merged API.
const ASSOCIATED_TO_A_MERGED_API: &str = "API can not be deleted as it is associated to a Merged API";

/// The fixed text every operation answers for an association this account does not hold.
const NO_ASSOCIATION: &str = "No source API association was found";

/// Which side of the association a create addresses, i.e. which of the two operations
/// the request reaches: AppSync creates ONE link either way.
#[derive(Debug, Clone, Copy)]
enum Side {
    /// `AssociateMergedGraphqlApi`: the source API is in the path, the Merged API in the
    /// body.
    FromSource,
    /// `AssociateSourceGraphqlApi`: the Merged API is in the path, the source API in the
    /// body.
    FromMerged,
}

/// Which accepted form of an `…Identifier` member a row names the BODY's API by — the
/// two `mergedApiIdentifier` and `sourceApiIdentifier` document ("only require the API
/// ID value or ARN").
#[derive(Debug, Clone, Copy)]
enum Form {
    Id,
    Arn,
}

impl Form {
    fn of(self, api: &Api) -> String {
        match self {
            Self::Id => api.id.clone(),
            Self::Arn => api.arn.clone(),
        }
    }
}

/// Create the association between `source` and `merged`, addressed from `side` and
/// naming the body's API in `form`.
async fn associate(
    router: &MultiServiceRouter,
    side: Side,
    form: Form,
    source: &Api,
    merged: &Api,
) -> Answer {
    let (path, body) = match side {
        Side::FromSource => (
            format!("/v1/sourceApis/{}/mergedApiAssociations", source.id),
            json!({"mergedApiIdentifier": form.of(merged)}),
        ),
        Side::FromMerged => (
            format!("/v1/mergedApis/{}/sourceApiAssociations", merged.id),
            json!({"sourceApiIdentifier": form.of(source)}),
        ),
    };
    call(router, "POST", &path, Some(body)).await
}

/// The `associationId` an associate answered with.
fn minted_id(answer: &Answer) -> String {
    answer.body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DisassociateMergedGraphqlApi`'s path: the association addressed from the source API.
fn from_the_source_side(source_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/sourceApis/{source_api_identifier}/mergedApiAssociations/{association_id}")
}

/// `StartSchemaMerge`'s path.
fn merge_path(merged_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}/merge")
}

/// `GetGraphqlApi`'s and `DeleteGraphqlApi`'s path.
fn api_path(api_id: &str) -> String {
    format!("/v1/apis/{api_id}")
}

/// One row of the duplicate table: which side the second create addresses, how it names
/// the body's API, and whether the clock has already made the first create's merge due.
#[derive(Debug)]
struct Duplicate {
    label: &'static str,
    side: Side,
    form: Form,
    /// Whether the row waits for the scheduled merge to come due before the duplicate.
    /// A duplicate is permanent, so the answer may not depend on it.
    past_the_merge: bool,
}

/// The duplicate table: both operations, both identifier forms for the body's API, and
/// both sides of the instant the clock makes the first create's merge due.
fn duplicates() -> Vec<Duplicate> {
    vec![
        Duplicate {
            label: "from the source side, by id, while the merge is owed",
            side: Side::FromSource,
            form: Form::Id,
            past_the_merge: false,
        },
        Duplicate {
            label: "from the source side, by arn, while the merge is owed",
            side: Side::FromSource,
            form: Form::Arn,
            past_the_merge: false,
        },
        Duplicate {
            label: "from the merged side, by id, while the merge is owed",
            side: Side::FromMerged,
            form: Form::Id,
            past_the_merge: false,
        },
        Duplicate {
            label: "from the merged side, by arn, past the merge",
            side: Side::FromMerged,
            form: Form::Arn,
            past_the_merge: true,
        },
        Duplicate {
            label: "from the source side, by id, past the merge",
            side: Side::FromSource,
            form: Form::Id,
            past_the_merge: true,
        },
    ]
}

#[tokio::test]
async fn a_pair_that_is_already_associated_is_refused_as_the_duplicate_it_is() {
    let router = router();

    for case in duplicates() {
        // Each row gets its own pair, so the first create always succeeds and the
        // refusal below is the row's own.
        let (source, merged) = create_pair(&router, case.label).await;
        let created = associate(&router, Side::FromSource, Form::Id, &source, &merged).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the first association to be created, got {} {:?}",
            case.label, created.status, created.body
        );
        let association_id = minted_id(&created);

        if case.past_the_merge {
            tokio::time::sleep(state::MERGE_SPAN + PAST_THE_SPAN).await;
        }

        let refused = associate(&router, case.side, case.form, &source, &merged).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException for the duplicate, got {} {} {:?}",
            case.label,
            refused.status,
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(ALREADY_EXISTS),
            "case {:?}: expected {ALREADY_EXISTS:?}, got {:?}",
            case.label,
            refused.body["message"]
        );

        // The refused duplicate created nothing: the association the first create
        // minted is still the one the Merged API holds.
        let read = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the first association to still be readable, got {} {:?}",
            case.label, read.status, read.body
        );
        assert_eq!(
            read.body["sourceApiAssociation"]["associationId"].as_str(),
            Some(association_id.as_str()),
            "case {:?}: expected the association the first create minted, got {:?}",
            case.label,
            read.body["sourceApiAssociation"]
        );
    }
}

/// One row of the second-Merged-API table: which side the second create addresses and
/// how it names the body's API.
#[derive(Debug)]
struct SecondMergedApi {
    label: &'static str,
    side: Side,
    form: Form,
}

/// The second-Merged-API table: both operations and both identifier forms.
fn second_merged_apis() -> Vec<SecondMergedApi> {
    vec![
        SecondMergedApi {
            label: "from the source side, by id",
            side: Side::FromSource,
            form: Form::Id,
        },
        SecondMergedApi {
            label: "from the source side, by arn",
            side: Side::FromSource,
            form: Form::Arn,
        },
        SecondMergedApi {
            label: "from the merged side, by id",
            side: Side::FromMerged,
            form: Form::Id,
        },
        SecondMergedApi {
            label: "from the merged side, by arn",
            side: Side::FromMerged,
            form: Form::Arn,
        },
    ]
}

#[tokio::test]
async fn a_source_api_cannot_take_a_second_merged_api() {
    let router = router();

    for case in second_merged_apis() {
        let (source, merged) = create_pair(&router, case.label).await;
        let second = create_api(
            &router,
            &format!("{}-second-merged", case.label),
            state::MERGED_API_TYPE,
        )
        .await;
        let created = associate(&router, Side::FromMerged, Form::Id, &source, &merged).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the first association to be created, got {} {:?}",
            case.label, created.status, created.body
        );
        let association_id = minted_id(&created);

        let refused = associate(&router, case.side, case.form, &source, &second).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException for the second Merged API, got {} {} \
             {:?}",
            case.label,
            refused.status,
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(MULTIPLE_MERGED_APIS),
            "case {:?}: expected {MULTIPLE_MERGED_APIS:?}, got {:?}",
            case.label,
            refused.body["message"]
        );

        // The second Merged API holds nothing, so nothing was created for it.
        let listed = call(
            &router,
            "GET",
            &format!("/v1/apis/{}/sourceApiAssociations", second.id),
            None,
        )
        .await;
        let summaries = listed.body["sourceApiAssociationSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            summaries.is_empty(),
            "case {:?}: expected the refused Merged API to hold no association, got \
             {summaries:?}",
            case.label
        );

        // The refusal follows the STORED topology: once the first association is gone,
        // the very same create succeeds.
        let disassociated = call(
            &router,
            "DELETE",
            &get_path(&merged.id, &association_id),
            None,
        )
        .await;
        assert_eq!(
            disassociated.status, 200,
            "case {:?}: expected the disassociate to be scheduled, got {} {:?}",
            case.label, disassociated.status, disassociated.body
        );
        tokio::time::sleep(state::DELETION_SPAN + PAST_THE_SPAN).await;

        let accepted = associate(&router, case.side, case.form, &source, &second).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the source API to be associable once it has no Merged API, \
             got {} {:?}",
            case.label,
            accepted.status,
            accepted.body
        );
        assert_eq!(
            accepted.body["sourceApiAssociation"]["mergedApiId"].as_str(),
            Some(second.id.as_str()),
            "case {:?}: expected the new association to name the second Merged API, got {:?}",
            case.label,
            accepted.body["sourceApiAssociation"]
        );
    }
}

/// Which operation a reference row addresses the association through.
#[derive(Debug, Clone, Copy)]
enum Through {
    /// `GetSourceApiAssociation`.
    Read,
    /// `UpdateSourceApiAssociation`.
    Update,
    /// `StartSchemaMerge`.
    Merge,
    /// `DisassociateSourceGraphqlApi`: addressed by the Merged API.
    DisassociateByMergedApi,
    /// `DisassociateMergedGraphqlApi`: the same association addressed by the source API.
    DisassociateBySourceApi,
}

impl Through {
    /// Make this operation's call for `reference`, against the pair given.
    async fn call(
        self,
        router: &MultiServiceRouter,
        source: &Api,
        merged: &Api,
        reference: &str,
    ) -> Answer {
        match self {
            Self::Read => call(router, "GET", &get_path(&merged.id, reference), None).await,
            Self::Update => {
                call(
                    router,
                    "POST",
                    &get_path(&merged.id, reference),
                    Some(json!({"description": "a reference that names no association"})),
                )
                .await
            }
            Self::Merge => call(router, "POST", &merge_path(&merged.id, reference), None).await,
            Self::DisassociateByMergedApi => {
                call(router, "DELETE", &get_path(&merged.id, reference), None).await
            }
            Self::DisassociateBySourceApi => {
                call(
                    router,
                    "DELETE",
                    &from_the_source_side(&source.id, reference),
                    None,
                )
                .await
            }
        }
    }
}

/// Every operation that addresses one association by `associationId`, so the split is
/// asserted over the whole class rather than over the operation the defect surfaced in.
const EVERY_OPERATION: &[Through] = &[
    Through::Read,
    Through::Update,
    Through::Merge,
    Through::DisassociateByMergedApi,
    Through::DisassociateBySourceApi,
];

/// Which reference a row carries.
#[derive(Debug, Clone, Copy)]
enum Reference {
    /// A value the row writes out itself.
    Literal(&'static str),
    /// The `associationId` the service minted for an association it has since removed.
    Removed,
}

/// One reference row: the value to address and which side of the split it falls on.
#[derive(Debug)]
struct Case {
    label: &'static str,
    reference: Reference,
    /// Whether the value is of no form the service issues, which is what makes the
    /// REQUEST malformed rather than the association missing.
    malformed: bool,
}

/// The reference table.
///
/// An `associationId` is "the ID generated by the AppSync service for the source API
/// association" (`smithy_query(query="doc", shape="DisassociateSourceGraphqlApiRequest",
/// member="associationId")`), which the service issues in the canonical hyphenated UUID
/// form, beside the 26-character base-32 opaque ids this emulator's own counter mints
/// (`state::API_ID_LEN`, `state::is_association_id`). The well-formed rows span both
/// forms and an id that was really issued and removed; the malformed rows span values of
/// neither form: a caller-invented reference, a UUID a character short, a UUID in
/// uppercase, and a single character.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the all-zero uuid",
            reference: Reference::Literal("00000000-0000-0000-0000-000000000000"),
            malformed: false,
        },
        Case {
            label: "a uuid nothing was issued under",
            reference: Reference::Literal("8c0c4f6e-2a1d-47a6-9f0b-7d9c1e5a3b24"),
            malformed: false,
        },
        Case {
            label: "the opaque form, nothing issued under it",
            reference: Reference::Literal("abcdefghijklmnopqrstuvwxyz"),
            malformed: false,
        },
        Case {
            label: "issued for this pair and since removed",
            reference: Reference::Removed,
            malformed: false,
        },
        Case {
            label: "a caller-invented reference",
            reference: Reference::Literal("assoc-does-not-exist-1234"),
            malformed: true,
        },
        Case {
            label: "a uuid one character short",
            reference: Reference::Literal("8c0c4f6e-2a1d-47a6-9f0b-7d9c1e5a3b2"),
            malformed: true,
        },
        Case {
            label: "a uuid in uppercase",
            reference: Reference::Literal("8C0C4F6E-2A1D-47A6-9F0B-7D9C1E5A3B24"),
            malformed: true,
        },
        Case {
            label: "a single character",
            reference: Reference::Literal("x"),
            malformed: true,
        },
    ]
}

#[tokio::test]
async fn an_association_reference_is_split_on_its_form_by_every_operation() {
    let router = router();
    let (source, merged) = create_pair(&router, "reference-split").await;

    // The reference a row must carry for an association the service really issued and
    // has since removed: a second source API is associated, disassociated, and the clock
    // alone then performs the deletion.
    let gone_source = create_api(&router, "reference-split-gone", state::GRAPHQL_API_TYPE).await;
    let gone = associate(&router, Side::FromMerged, Form::Id, &gone_source, &merged).await;
    assert_eq!(
        gone.status, 200,
        "case removed fixture: expected the association to be created, got {} {:?}",
        gone.status, gone.body
    );
    let removed = minted_id(&gone);
    let scheduled = call(&router, "DELETE", &get_path(&merged.id, &removed), None).await;
    assert_eq!(
        scheduled.status, 200,
        "case removed fixture: expected the deletion to be scheduled, got {} {:?}",
        scheduled.status, scheduled.body
    );
    tokio::time::sleep(state::DELETION_SPAN + PAST_THE_SPAN).await;

    // The pair whose LIVE association no row may touch, so a refusal cannot be mistaken
    // for a deletion.
    let live = associate(&router, Side::FromMerged, Form::Id, &source, &merged).await;
    assert_eq!(
        live.status, 200,
        "case live fixture: expected the association to be created, got {} {:?}",
        live.status, live.body
    );
    let live_association = minted_id(&live);

    for case in cases() {
        let reference = match case.reference {
            Reference::Literal(value) => value.to_string(),
            Reference::Removed => removed.clone(),
        };
        // The two answers the one rule gives, stated from the row's class alone.
        let (status, error_type, message) = if case.malformed {
            (
                400,
                "BadRequestException",
                format!("Invalid SourceApi Association ID {reference}"),
            )
        } else {
            (404, "NotFoundException", NO_ASSOCIATION.to_string())
        };

        for through in EVERY_OPERATION {
            let answer = through.call(&router, &source, &merged, &reference).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (status, error_type),
                "case {:?} through {through:?} ({reference:?}): expected {status} {error_type}, \
                 got {} {} {:?}",
                case.label,
                answer.status,
                answer.error_type,
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(message.as_str()),
                "case {:?} through {through:?} ({reference:?}): expected {message:?}, got {:?}",
                case.label,
                answer.body["message"]
            );
        }
    }

    // Not one of the refused calls touched the live association.
    let read = call(
        &router,
        "GET",
        &get_path(&merged.id, &live_association),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case live association: expected it to still be readable, got {} {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn an_associated_source_api_cannot_be_deleted_until_it_is_disassociated() {
    let router = router();
    let (source, merged) = create_pair(&router, "delete-gate").await;
    // An API in no association at all, so the gate is not simply "every API is
    // undeletable".
    let unassociated = create_api(&router, "delete-gate-free", state::GRAPHQL_API_TYPE).await;

    let created = associate(&router, Side::FromMerged, Form::Id, &source, &merged).await;
    assert_eq!(
        created.status, 200,
        "case delete gate: expected the association to be created, got {} {:?}",
        created.status, created.body
    );
    let association_id = minted_id(&created);

    // The source side of a live association cannot be deleted — while the merge the
    // create scheduled is owed and once the clock has made it due.
    for case in ["while the merge is owed", "past the merge"] {
        if case == "past the merge" {
            tokio::time::sleep(state::MERGE_SPAN + PAST_THE_SPAN).await;
        }
        let refused = call(&router, "DELETE", &api_path(&source.id), None).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException for the associated source API, got \
             {} {} {:?}",
            refused.status,
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(ASSOCIATED_TO_A_MERGED_API),
            "case {case:?}: expected {ASSOCIATED_TO_A_MERGED_API:?}, got {:?}",
            refused.body["message"]
        );
        // The refusal deleted nothing: the API and its association are both still there.
        let read = call(&router, "GET", &api_path(&source.id), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the refused API to still be readable, got {} {:?}",
            read.status, read.body
        );
        let association = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
        assert_eq!(
            association.status, 200,
            "case {case:?}: expected the association to survive the refused delete, got {} {:?}",
            association.status, association.body
        );
    }

    // An API that is a side of nothing is deleted, and is then the absent API it is.
    let deleted = call(&router, "DELETE", &api_path(&unassociated.id), None).await;
    assert_eq!(
        deleted.status, 200,
        "case unassociated api: expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let read = call(&router, "GET", &api_path(&unassociated.id), None).await;
    assert_eq!(
        (read.status, read.error_type.as_str()),
        (404, "NotFoundException"),
        "case unassociated api: expected the deleted API to be gone, got {} {} {:?}",
        read.status,
        read.error_type,
        read.body
    );

    // Disassociating the source API — from the SOURCE side, the way the merged-API
    // teardown does it — leaves the deletion nothing to block on: the association is on
    // its way out, and the status says so.
    let disassociated = call(
        &router,
        "DELETE",
        &from_the_source_side(&source.id, &association_id),
        None,
    )
    .await;
    assert_eq!(
        disassociated.status, 200,
        "case disassociate: expected 200, got {} {:?}",
        disassociated.status, disassociated.body
    );
    assert_eq!(
        disassociated.body["sourceApiAssociationStatus"].as_str(),
        Some(state::DELETION_SCHEDULED),
        "case disassociate: expected the scheduled-deletion status, got {:?}",
        disassociated.body
    );

    let gone = call(&router, "DELETE", &api_path(&source.id), None).await;
    assert_eq!(
        gone.status, 200,
        "case delete after disassociate: expected 200, got {} {:?}",
        gone.status, gone.body
    );
    let read = call(&router, "GET", &api_path(&source.id), None).await;
    assert_eq!(
        (read.status, read.error_type.as_str()),
        (404, "NotFoundException"),
        "case delete after disassociate: expected the deleted API to be gone, got {} {} {:?}",
        read.status,
        read.error_type,
        read.body
    );

    // The Merged API side is not what the gate holds: it is deleted with the links it
    // was a side of.
    let merged_deleted = call(&router, "DELETE", &api_path(&merged.id), None).await;
    assert_eq!(
        merged_deleted.status, 200,
        "case delete the merged api: expected 200, got {} {:?}",
        merged_deleted.status, merged_deleted.body
    );
}

#[tokio::test]
async fn the_merged_api_side_of_an_association_is_torn_down_by_the_id_the_create_minted() {
    let router = router();
    let (source, merged) = create_pair(&router, "merged-side-teardown").await;
    let created = associate(&router, Side::FromSource, Form::Id, &source, &merged).await;
    assert_eq!(
        created.status, 200,
        "case teardown: expected the association to be created, got {} {:?}",
        created.status, created.body
    );
    let association_id = minted_id(&created);

    // The id the create answered with resolves from the SOURCE API's side, and the
    // disassociate schedules the deletion rather than performing it.
    let disassociated = call(
        &router,
        "DELETE",
        &from_the_source_side(&source.id, &association_id),
        None,
    )
    .await;
    assert_eq!(
        (disassociated.status, disassociated.error_type.as_str()),
        (200, ""),
        "case teardown: expected 200 for the id the create minted, got {} {} {:?}",
        disassociated.status,
        disassociated.error_type,
        disassociated.body
    );
    assert_eq!(
        disassociated.body["sourceApiAssociationStatus"].as_str(),
        Some(state::DELETION_SCHEDULED),
        "case teardown: expected {:?}, got {:?}",
        state::DELETION_SCHEDULED,
        disassociated.body
    );

    // While the deletion is merely scheduled the association still reports that status,
    // and the clock alone then removes it.
    let scheduled = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
    assert_eq!(
        scheduled.body["sourceApiAssociation"]["sourceApiAssociationStatus"].as_str(),
        Some(state::DELETION_SCHEDULED),
        "case teardown: expected the scheduled deletion to be reported, got {:?}",
        scheduled.body
    );
    tokio::time::sleep(state::DELETION_SPAN + PAST_THE_SPAN).await;
    let removed = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
    assert_eq!(
        (removed.status, removed.error_type.as_str()),
        (404, "NotFoundException"),
        "case teardown: expected the association to be gone past the due instant, got {} {} {:?}",
        removed.status,
        removed.error_type,
        removed.body
    );

    // And the teardown then completes: both APIs of the pair are deletable.
    for (label, api) in [("source", &source), ("merged", &merged)] {
        let deleted = call(&router, "DELETE", &api_path(&api.id), None).await;
        assert_eq!(
            deleted.status, 200,
            "case teardown {label}: expected 200, got {} {:?}",
            deleted.status, deleted.body
        );
    }
}
