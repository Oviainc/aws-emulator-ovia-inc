// SPDX-License-Identifier: CC0-1.0

//! The TEXT the source-API-association refusals carry, end to end over HTTP.
//!
//! Two rules are asserted here, each over a table rather than over the single request
//! that exposed them:
//!
//! 1. an API on a side of an association its `GraphQLApiType` cannot take is refused
//!    400 `BadRequestException` whose message names the type that side requires and the
//!    API that was refused — asserted for both sides (a standard API addressed as the
//!    Merged one, a Merged API addressed as the source one), through both association
//!    operations, and with the API named by its `apiId` and by its ARN, which must not
//!    change the refusal;
//! 2. a modification of a Merged API's source-API associations that is still in
//!    progress refuses the next one 409 `ConcurrentModificationException` whose message
//!    names THAT Merged API — asserted over every association operation that can meet
//!    the conflict (`StartSchemaMerge`, `UpdateSourceApiAssociation`), with the
//!    contended Merged API named by either identifier form;
//! 3. a create that finds the pair already associated is refused 400
//!    `BadRequestException` with the duplicate sentence — never the retryable conflict —
//!    whatever the clock still owes on the association it found, and the association it
//!    found is left where it was.
//!
//! Every row drives the router the production emulator mounts, through the harness the
//! Merged-API-side suite carries, so routing, restJson1 (de)serialization, the handlers
//! and the shared engine are exercised together.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, create_api, create_pair, router, Answer, Api};

/// `AssociateMergedGraphqlApi`'s path: the association addressed from the SOURCE API.
fn from_the_source_side(source_api_identifier: &str) -> String {
    format!("/v1/sourceApis/{source_api_identifier}/mergedApiAssociations")
}

/// `AssociateSourceGraphqlApi`'s path: the same association addressed from the MERGED
/// API.
fn from_the_merged_side(merged_api_identifier: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations")
}

/// `UpdateSourceApiAssociation`'s (and `DisassociateSourceGraphqlApi`'s) path: one
/// association OF a Merged API.
fn association_path(merged_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}")
}

/// `StartSchemaMerge`'s path.
fn merge_path(merged_api_identifier: &str, association_id: &str) -> String {
    format!(
        "/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}/merge"
    )
}

/// `ListSourceApiAssociations`' path, used to prove a refused request stored nothing.
fn list_path(api_id: &str) -> String {
    format!("/v1/apis/{api_id}/sourceApiAssociations")
}

/// Which accepted form of an `…Identifier` member a row names an API by: the two
/// `mergedApiIdentifier` and `sourceApiIdentifier` document ("only require the API ID
/// value or ARN").
#[derive(Debug, Clone, Copy)]
enum Form {
    Id,
    Arn,
}

impl Form {
    /// `api` in this form.
    fn of(self, api: &Api) -> String {
        match self {
            Self::Id => api.id.clone(),
            Self::Arn => api.arn.clone(),
        }
    }
}

/// Which side of the association a row points at an API of the wrong type.
#[derive(Debug, Clone, Copy)]
enum WrongSide {
    /// The Merged side, which only an API of `MERGED` type can take.
    Merged,
    /// The source side, which only an API of `GRAPHQL` type can take.
    Source,
}

/// One row of the api-type table: which side is pointed at the wrong kind of API,
/// which operation the request reaches the service through, and the identifier form it
/// names the refused API by.
#[derive(Debug)]
struct TypeCase {
    label: &'static str,
    side: WrongSide,
    /// Whether the request is `AssociateMergedGraphqlApi` (the source API is in the
    /// path) rather than `AssociateSourceGraphqlApi`.
    from_source_side: bool,
    /// How the BODY member names its API. A path label cannot carry the ARN form (an
    /// ARN holds `/`, which a non-greedy label does not match), so a row that names the
    /// refused API in the path takes [`Form::Id`].
    form: Form,
}

/// The api-type table: both sides of the association, both operations, and both
/// identifier forms for the API that is refused.
fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            label: "standard api as the merged side, by id",
            side: WrongSide::Merged,
            from_source_side: true,
            form: Form::Id,
        },
        TypeCase {
            label: "standard api as the merged side, by arn",
            side: WrongSide::Merged,
            from_source_side: true,
            form: Form::Arn,
        },
        TypeCase {
            label: "standard api as the merged side, addressed from it",
            side: WrongSide::Merged,
            from_source_side: false,
            form: Form::Id,
        },
        TypeCase {
            label: "merged api as the source side, by id",
            side: WrongSide::Source,
            from_source_side: false,
            form: Form::Id,
        },
        TypeCase {
            label: "merged api as the source side, by arn",
            side: WrongSide::Source,
            from_source_side: false,
            form: Form::Arn,
        },
        TypeCase {
            label: "merged api as the source side, addressed from it",
            side: WrongSide::Source,
            from_source_side: true,
            form: Form::Id,
        },
    ]
}

#[tokio::test]
async fn an_api_on_the_wrong_side_is_refused_by_the_type_that_side_requires() {
    let router = router();
    // The pair that CAN associate, plus the two APIs that are on the wrong side of it:
    // a second standard API (no Merged side) and a second Merged API (no source side).
    let (source, merged) = create_pair(&router, "wrong-type").await;
    let standard = create_api(&router, "wrong-type-standard", state::GRAPHQL_API_TYPE).await;
    let other_merged = create_api(&router, "wrong-type-merged", state::MERGED_API_TYPE).await;

    for case in type_cases() {
        // The API that is refused and the type its side requires.
        let (refused_api, required_type) = match case.side {
            WrongSide::Merged => (&standard, state::MERGED_API_TYPE),
            WrongSide::Source => (&other_merged, state::GRAPHQL_API_TYPE),
        };
        // Whichever operation the row takes, the refused API sits on its own side: in
        // the path when that side is the one the operation addresses, in the body
        // otherwise.
        let (path, body) = match (case.side, case.from_source_side) {
            (WrongSide::Merged, true) => (
                from_the_source_side(&source.id),
                json!({"mergedApiIdentifier": case.form.of(refused_api)}),
            ),
            (WrongSide::Merged, false) => (
                from_the_merged_side(&refused_api.id),
                json!({"sourceApiIdentifier": source.id}),
            ),
            (WrongSide::Source, false) => (
                from_the_merged_side(&merged.id),
                json!({"sourceApiIdentifier": case.form.of(refused_api)}),
            ),
            (WrongSide::Source, true) => (
                from_the_source_side(&refused_api.id),
                json!({"mergedApiIdentifier": merged.id}),
            ),
        };

        let answer = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected a 400 BadRequestException, got {} {} {:?}",
            case.label,
            answer.status,
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(state::invalid_api_type_message(required_type, &refused_api.id).as_str()),
            "case {:?}: expected the refusal to name the {required_type} type the side \
             requires and the API refused, got {:?}",
            case.label,
            answer.body["message"]
        );
    }

    // Not one of those requests created anything: every side of every row is still
    // unassociated.
    for (label, api) in [
        ("source", &source),
        ("merged", &merged),
        ("standard", &standard),
        ("other merged", &other_merged),
    ] {
        let listed = call(&router, "GET", &list_path(&api.id), None).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected ListSourceApiAssociations 200, got {} {:?}",
            listed.status, listed.body
        );
        let summaries = listed.body["sourceApiAssociationSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            summaries.is_empty(),
            "case {label:?}: expected a refused association to store nothing, got {summaries:?}"
        );
    }
}

/// Which association operation a row makes its conflicting call through.
///
/// Every row runs into the same modification: the deletion a disassociate scheduled,
/// which leaves the association modifiable by nobody until it comes due.
///
/// The two creates are NOT here: a pair that is already associated leaves a create
/// nothing to do whatever the clock still owes, so it answers the duplicate refusal
/// rather than a conflict (asserted in
/// `a_duplicate_association_is_a_bad_request_rather_than_a_conflict`).
#[derive(Debug, Clone, Copy)]
enum Conflicting {
    /// `StartSchemaMerge`.
    StartMerge,
    /// `UpdateSourceApiAssociation`.
    Update,
}

/// One row of the conflict table: which operation runs into the modification the clock
/// still owes.
#[derive(Debug)]
struct ConflictCase {
    label: &'static str,
    through: Conflicting,
}

/// The conflict table: every operation that can meet a modification of a Merged API's
/// associations already in progress, with the contended Merged API named by either
/// identifier form.
///
/// The merge and the update meet it while the scheduled DELETION is owed, a merge or an
/// update being what the scheduled merge already is or configures.
fn conflict_cases() -> Vec<ConflictCase> {
    vec![
        ConflictCase {
            label: "merge while the deletion is owed",
            through: Conflicting::StartMerge,
        },
        ConflictCase {
            label: "update while the deletion is owed",
            through: Conflicting::Update,
        },
    ]
}

/// Make `case`'s conflicting call against the association given.
async fn conflicting_call(
    router: &MultiServiceRouter,
    case: &ConflictCase,
    merged: &Api,
    association_id: &str,
) -> Answer {
    match case.through {
        Conflicting::StartMerge => {
            call(router, "POST", &merge_path(&merged.id, association_id), None).await
        }
        Conflicting::Update => {
            call(
                router,
                "POST",
                &association_path(&merged.id, association_id),
                Some(json!({"description": "while the deletion is owed"})),
            )
            .await
        }
    }
}

#[tokio::test]
async fn a_conflict_on_an_association_names_the_merged_api_it_is_scoped_to() {
    let router = router();
    let mut named: Vec<String> = Vec::new();

    for case in conflict_cases() {
        // Each row gets its OWN pair, so the Merged API the refusal must name differs
        // from row to row and a constant message cannot satisfy them all.
        let (source, merged) = create_pair(&router, case.label).await;
        let created = call(
            &router,
            "POST",
            &from_the_source_side(&source.id),
            Some(json!({"mergedApiIdentifier": merged.id})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the first association to be created, got {} {:?}",
            case.label, created.status, created.body
        );
        let association_id = created.body["sourceApiAssociation"]["associationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The deletion the row's conflicting call runs into: scheduled, not performed,
        // so the clock still owes it while that call is made.
        let deleted = call(
            &router,
            "DELETE",
            &association_path(&merged.id, &association_id),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected the deletion to be scheduled, got {} {:?}",
            case.label, deleted.status, deleted.body
        );

        let refused = conflicting_call(&router, &case, &merged, &association_id).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (409, "ConcurrentModificationException"),
            "case {:?}: expected a 409 ConcurrentModificationException, got {} {} {:?}",
            case.label,
            refused.status,
            refused.error_type,
            refused.body
        );
        let message = refused.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            state::associations_being_updated_message(&merged.id),
            "case {:?}: expected the refusal to name the contended Merged API {:?}, got {:?}",
            case.label,
            merged.id,
            refused.body["message"]
        );
        assert!(
            !message.contains(&source.id),
            "case {:?}: expected the refusal to name the Merged API alone, got {message:?} \
             naming the source API {:?}",
            case.label,
            source.id
        );
        assert!(
            !named.contains(&message.to_string()),
            "case {:?}: expected a refusal scoped to this row's Merged API, got the text \
             {message:?} an earlier row already answered",
            case.label
        );
        named.push(message.to_string());
    }
}

/// A create that finds the pair already associated is the DUPLICATE it is rather than a
/// conflict, whatever the clock still owes on the existing association: the refusal is
/// asserted both while the scheduled merge is owed and once it has been performed, so a
/// permanently duplicate request is never answered with a retryable signal.
#[tokio::test]
async fn a_duplicate_association_is_a_bad_request_rather_than_a_conflict() {
    let router = router();
    let (source, merged) = create_pair(&router, "settled").await;
    let created = call(
        &router,
        "POST",
        &from_the_source_side(&source.id),
        Some(json!({"mergedApiIdentifier": merged.id})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case settled: expected the first association to be created, got {} {:?}",
        created.status, created.body
    );

    // Twice: once while the create's own scheduled merge is still owed, and once past
    // the instant the clock made it due.
    for case in ["while the merge is owed", "once the merge is done"] {
        if case == "once the merge is done" {
            tokio::time::sleep(state::MERGE_SPAN + std::time::Duration::from_millis(250)).await;
        }
        let again = call(
            &router,
            "POST",
            &from_the_source_side(&source.id),
            Some(json!({"mergedApiIdentifier": merged.id})),
        )
        .await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException for the duplicate, got {} {} {:?}",
            again.status,
            again.error_type,
            again.body
        );
        let message: &str = again.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, "SourceApiAssociation already exists.",
            "case {case:?}: expected the duplicate-association text, got {message:?}"
        );
        assert!(
            !message.contains(&merged.id) && !message.contains(&source.id),
            "case {case:?}: expected the duplicate refusal to name neither API of the request, \
             got {message:?}"
        );
    }
}

/// The duplicate refusal leaves the association it found where it was, so a listing of
/// the Merged API still reports exactly the one association.
#[tokio::test]
async fn a_refused_duplicate_leaves_the_one_association_it_found() {
    let router = router();
    let (source, merged) = create_pair(&router, "stored-under").await;
    let created = call(
        &router,
        "POST",
        &from_the_merged_side(&merged.id),
        Some(json!({"sourceApiIdentifier": source.arn})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case stored-under: expected the association to be created, got {} {:?}",
        created.status, created.body
    );

    // The same pair, named by the other accepted identifier form: one association
    // either way, so the duplicate is recognised whichever form the request used.
    let refused = call(
        &router,
        "POST",
        &from_the_merged_side(&merged.id),
        Some(json!({"sourceApiIdentifier": source.id})),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_type.as_str()),
        (400, "BadRequestException"),
        "case stored-under: expected a 400 BadRequestException, got {} {} {:?}",
        refused.status,
        refused.error_type,
        refused.body
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some("SourceApiAssociation already exists."),
        "case stored-under: expected the duplicate-association text, got {:?}",
        refused.body["message"]
    );

    let listed = call(&router, "GET", &list_path(&merged.id), None).await;
    let summaries: Vec<Value> = listed.body["sourceApiAssociationSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "case stored-under: expected the one association the duplicate was refused against, got \
         {summaries:?}"
    );
    assert_eq!(
        summaries[0]["mergedApiId"].as_str(),
        Some(merged.id.as_str()),
        "case stored-under: expected the stored association to name the Merged API, got {:?}",
        summaries[0]
    );
}
