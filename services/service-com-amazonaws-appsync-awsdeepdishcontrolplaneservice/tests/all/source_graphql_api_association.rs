// SPDX-License-Identifier: CC0-1.0

//! `AssociateSourceGraphqlApi` end to end over HTTP: the source-API association
//! created from the MERGED API's side, read back through `GetSourceApiAssociation`.
//!
//! AppSync creates one and the same link from either side — this operation addresses
//! the Merged API and names the source API in the body, `AssociateMergedGraphqlApi` the
//! other way round — so the rules asserted here are the rules of that one link,
//! each over a TABLE or a class of inputs rather than over a single request:
//!
//! 1. the call persists ONE association, minted from the service's own state (opaque
//!    `associationId`, ARN under the Merged API) and carrying both sides' own `apiId`
//!    and ARN whichever accepted identifier form the request named them by, with the
//!    `MergeType` the request asked for or the modelled default when it asked for
//!    none — over a table spanning the identifier forms, the whole `MergeType` value
//!    set, a config object without a `mergeType`, and `description` present and absent;
//! 2. creating the association SCHEDULES a merge, so it is reported in the
//!    non-terminal `MERGE_SCHEDULED` status and reaches `MERGE_SUCCESS` once the clock
//!    has made that merge due, with no call in between;
//! 3. the pair is linked at most once no matter which side the request addresses: a
//!    second association of the same pair is refused 400 `BadRequestException` through
//!    EITHER operation, with the service's own duplicate sentence;
//! 4. an identifier that cannot name an API is a 400 `BadRequestException`, one that
//!    names no API this account holds a 404 `NotFoundException`, and an API on the
//!    wrong side of the association a 400.
//!
//! The harness (`router`, `call`, `create_api`) is the one the Merged-API-side suite
//! already carries, so both suites drive the same router the production emulator
//! mounts, exercising routing, restJson1 (de)serialization, the handlers and the
//! shared engine together.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{
    associate_path as merged_side_path, call, create_api, create_pair, get_path, router, Api,
    ACCOUNT, ID_ALPHABET, PAST_THE_MERGE, REGION,
};

/// `AssociateSourceGraphqlApi`'s path for `merged_api_identifier`.
///
/// The label takes the API-id form: an ARN carries `/`, which a non-greedy path label
/// does not match, so the ARN form of an identifier can only reach the service through
/// a body member — which is where this suite exercises it (`sourceApiIdentifier`).
fn associate_path(merged_api_identifier: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations")
}

/// One row of the association table: how the request names the source API and what it
/// asks for, and what the stored association must then report.
#[derive(Debug)]
struct Case {
    /// The row's label, used both in the API names and in every assertion message.
    label: &'static str,
    /// Whether the body names the source API by its ARN rather than by its `apiId` —
    /// the two forms `sourceApiIdentifier` documents.
    source_by_arn: bool,
    description: Option<&'static str>,
    /// The `sourceApiAssociationConfig` the request carries, if any.
    config: Option<Value>,
    /// The `mergeType` the association must then report.
    expected_merge_type: &'static str,
}

/// The association table: the identifier forms `sourceApiIdentifier` accepts, every
/// value of the `MergeType` enum, a config object that names no `mergeType`, and
/// `description` present and absent.
///
/// Source for the `MergeType` value set and its default:
/// `smithy_query(query="shape", shape="SourceApiAssociationConfig")` —
/// "Valid merge types are `MANUAL_MERGE` (default) and `AUTO_MERGE`".
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "ids no config",
            source_by_arn: false,
            description: None,
            config: None,
            expected_merge_type: "MANUAL_MERGE",
        },
        Case {
            label: "source api named by arn",
            source_by_arn: true,
            description: Some("the association the ARN form created"),
            config: Some(json!({"mergeType": "MANUAL_MERGE"})),
            expected_merge_type: "MANUAL_MERGE",
        },
        Case {
            label: "auto merge",
            source_by_arn: false,
            description: Some("subscribed to the source API's changes"),
            config: Some(json!({"mergeType": "AUTO_MERGE"})),
            expected_merge_type: "AUTO_MERGE",
        },
        Case {
            label: "config without a merge type",
            source_by_arn: true,
            description: Some("configuré — déjà vu"),
            config: Some(json!({})),
            expected_merge_type: "MANUAL_MERGE",
        },
    ]
}

/// The `AssociateSourceGraphqlApi` body a row makes: the source API in the form the
/// row names it by, plus whatever else it carries.
fn association_body(case: &Case, source: &Api) -> Value {
    let source_identifier = if case.source_by_arn {
        source.arn.clone()
    } else {
        source.id.clone()
    };
    let mut body = json!({"sourceApiIdentifier": source_identifier});
    if let Some(description) = case.description {
        body["description"] = json!(description);
    }
    if let Some(config) = case.config.clone() {
        body["sourceApiAssociationConfig"] = config;
    }
    body
}

#[tokio::test]
async fn an_association_created_from_the_merged_api_side_is_persisted_as_configured() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();

    for case in cases() {
        let (source, merged) = create_pair(&router, case.label).await;
        let body = association_body(&case, &source);

        let answer = call(
            &router,
            "POST",
            &associate_path(&merged.id),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected AssociateSourceGraphqlApi 200, got {} {:?}",
            case.label, answer.status, answer.body
        );

        let association = answer.body["sourceApiAssociation"].clone();
        let association_id = association["associationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The id is minted by the service, from its own state: opaque, of the shape
        // this service's ids take, and never a function of the request.
        assert_eq!(
            association_id.chars().count(),
            state::API_ID_LEN,
            "case {:?}: expected a {}-character associationId, got {association_id:?}",
            case.label,
            state::API_ID_LEN
        );
        assert!(
            association_id
                .chars()
                .all(|character| ID_ALPHABET.contains(character)),
            "case {:?}: expected an associationId over {ID_ALPHABET:?}, got {association_id:?}",
            case.label
        );
        assert!(
            !minted_ids.contains(&association_id),
            "case {:?}: expected a fresh associationId, got the already-minted \
             {association_id:?} (minted so far: {minted_ids:?})",
            case.label
        );
        minted_ids.push(association_id.clone());

        // Both sides are reported by their OWN id and ARN, whichever form the request
        // named them by, and the association's ARN sits under the Merged API.
        let expected_arn = format!(
            "arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{}/sourceApiAssociations/{association_id}",
            merged.id
        );
        for (member, expected) in [
            ("associationArn", expected_arn.as_str()),
            ("sourceApiId", source.id.as_str()),
            ("sourceApiArn", source.arn.as_str()),
            ("mergedApiId", merged.id.as_str()),
            ("mergedApiArn", merged.arn.as_str()),
        ] {
            assert_eq!(
                association[member].as_str(),
                Some(expected),
                "case {:?}: expected {member} {expected:?}, got {:?}",
                case.label,
                association[member]
            );
        }

        assert_eq!(
            association["description"].as_str(),
            case.description,
            "case {:?}: expected description {:?}, got {:?}",
            case.label,
            case.description,
            association["description"]
        );
        assert_eq!(
            association["sourceApiAssociationConfig"]["mergeType"].as_str(),
            Some(case.expected_merge_type),
            "case {:?}: expected mergeType {:?}, got {:?}",
            case.label,
            case.expected_merge_type,
            association["sourceApiAssociationConfig"]
        );

        // The create SCHEDULES the merge: a non-terminal status, a detail that states
        // where the association stands, and no merge performed yet.
        assert_eq!(
            association["sourceApiAssociationStatus"].as_str(),
            Some(state::MERGE_SCHEDULED),
            "case {:?}: expected status {:?}, got {:?}",
            case.label,
            state::MERGE_SCHEDULED,
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
        assert!(
            association["lastSuccessfulMergeDate"].is_null(),
            "case {:?}: expected no merge date before the scheduled merge, got {:?}",
            case.label,
            association["lastSuccessfulMergeDate"]
        );

        // Round trip: the reader answers the SAME body the create answered.
        let read = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetSourceApiAssociation 200, got {} {:?}",
            case.label, read.status, read.body
        );
        assert_eq!(
            read.body["sourceApiAssociation"], association,
            "case {:?}: expected the stored association back, got {:?}",
            case.label, read.body["sourceApiAssociation"]
        );

        // The pair is linked at most once, so a second association of it has nothing
        // to create — asked for from THIS side and from the other one, since both
        // create the same link.
        let again = call(
            &router,
            "POST",
            &associate_path(&merged.id),
            Some(body.clone()),
        )
        .await;
        let from_the_other_side = call(
            &router,
            "POST",
            &merged_side_path(&source.id),
            Some(json!({"mergedApiIdentifier": merged.id})),
        )
        .await;
        for (side, refused) in [
            ("AssociateSourceGraphqlApi", &again),
            ("AssociateMergedGraphqlApi", &from_the_other_side),
        ] {
            assert_eq!(
                (refused.status, refused.error_type.as_str()),
                (400, "BadRequestException"),
                "case {:?}: expected a 400 BadRequestException for the second \
                 association through {side}, got {} {} {:?}",
                case.label,
                refused.status,
                refused.error_type,
                refused.body
            );
            // The refusal states the association that is in the way, whichever side
            // the request addressed it from.
            assert_eq!(
                refused.body["message"].as_str(),
                Some("SourceApiAssociation already exists."),
                "case {:?}: expected the duplicate-association text from {side}, got {:?}",
                case.label,
                refused.body["message"]
            );
        }
    }
}

#[tokio::test]
async fn the_merge_this_side_scheduled_is_owed_by_the_clock_alone() {
    let router = router();
    let (source, merged) = create_pair(&router, "merged-side-clock").await;

    let created = call(
        &router,
        "POST",
        &associate_path(&merged.id),
        Some(json!({"sourceApiIdentifier": source.id})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case schedule: expected 200, got {} {:?}",
        created.status, created.body
    );
    let association_id = created.body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Nothing was called in between: the transition below is owed by the clock.
    tokio::time::sleep(state::MERGE_SPAN + PAST_THE_MERGE).await;

    let settled = call(&router, "GET", &get_path(&merged.id, &association_id), None).await;
    assert_eq!(
        settled.status, 200,
        "case settled: expected 200, got {} {:?}",
        settled.status, settled.body
    );
    let association = settled.body["sourceApiAssociation"].clone();
    assert_eq!(
        association["sourceApiAssociationStatus"].as_str(),
        Some(state::MERGE_SUCCESS),
        "case settled: expected status {:?} past the due instant, got {:?}",
        state::MERGE_SUCCESS,
        association["sourceApiAssociationStatus"]
    );
    let merge_date = association["lastSuccessfulMergeDate"].clone();
    assert!(
        merge_date.as_f64().is_some_and(|epoch| epoch > 0.0),
        "case settled: expected a merge date past the due instant, got {merge_date:?}"
    );

    // The pair is associated and no modification is in progress any more, so a further
    // association of it is simply a bad request.
    let again = call(
        &router,
        "POST",
        &associate_path(&merged.id),
        Some(json!({"sourceApiIdentifier": source.arn})),
    )
    .await;
    assert_eq!(
        (again.status, again.error_type.as_str()),
        (400, "BadRequestException"),
        "case settled duplicate: expected a 400 BadRequestException, got {} {} {:?}",
        again.status,
        again.error_type,
        again.body
    );
}

/// One refusal row: the request to make and the answer it must take.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    /// `{source}`, `{merged}`, `{graphql}` are substituted with the fixture's ids
    /// before the call.
    path: &'static str,
    body: &'static str,
    status: u16,
    error_type: &'static str,
}

/// Every way an `AssociateSourceGraphqlApi` request can name something that cannot be
/// associated: an identifier that is no identifier at all (400), one that names nothing
/// this account holds, in either form (404), and an API on the wrong side of the
/// association (400).
///
/// The over-long identifier is one character past the 26 an AppSync API id has
/// (`state::API_ID_LEN`), the bound `state::is_api_id` publishes; the absent ones are
/// of exactly that length, so they are well-formed ids this account simply does not
/// hold.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "merged identifier too long to be one",
            path: "/v1/mergedApis/aaaaaaaaaaaaaaaaaaaaaaaaaaa/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "{source}"}"#,
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "source identifier too long to be one",
            path: "/v1/mergedApis/{merged}/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "aaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "merged api this account does not hold",
            path: "/v1/mergedApis/abcdefghijklmnopqrstuvwxyz/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "{source}"}"#,
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "source api this account does not hold",
            path: "/v1/mergedApis/{merged}/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "abcdefghijklmnopqrstuvwxyz"}"#,
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "source api arn this account does not hold",
            path: "/v1/mergedApis/{merged}/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "arn:aws:appsync:eu-west-1:000000000000:apis/abcdefghijklmnopqrstuvwxyz"}"#,
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "a standard api cannot be the merged side",
            path: "/v1/mergedApis/{graphql}/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "{source}"}"#,
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "a merged api cannot be the source side",
            path: "/v1/mergedApis/{merged}/sourceApiAssociations",
            body: r#"{"sourceApiIdentifier": "{merged}"}"#,
            status: 400,
            error_type: "BadRequestException",
        },
    ]
}

#[tokio::test]
async fn an_association_this_side_cannot_name_is_refused() {
    let router: MultiServiceRouter = router();
    // The fixtures every row is written against: the pair that CAN associate, plus a
    // second standard API that is on the wrong side of it.
    let (source, merged) = create_pair(&router, "merged-side-refuse").await;
    let graphql = create_api(&router, "merged-side-refuse-standard", state::GRAPHQL_API_TYPE).await;

    let fill = |template: &str| {
        template
            .replace("{source}", &source.id)
            .replace("{merged}", &merged.id)
            .replace("{graphql}", &graphql.id)
    };

    for refusal in refusals() {
        let body =
            serde_json::from_str::<Value>(&fill(refusal.body)).expect("the row's body is JSON");
        let answer = call(&router, "POST", &fill(refusal.path), Some(body)).await;
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
    }
}
