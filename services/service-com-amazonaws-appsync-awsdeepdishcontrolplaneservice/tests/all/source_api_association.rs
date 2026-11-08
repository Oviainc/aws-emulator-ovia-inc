// SPDX-License-Identifier: CC0-1.0

//! Source-API associations, end to end over HTTP: `AssociateMergedGraphqlApi` and the
//! `GetSourceApiAssociation` that reads the association back.
//!
//! Four rules are asserted here, each over a TABLE or a class of inputs rather than
//! over the single request that exposed the defect:
//!
//! 1. associating a source API with a Merged API persists ONE association, minted from
//!    the service's own state (opaque `associationId`, ARN under the Merged API) and
//!    carrying both sides' own `apiId` and ARN whichever accepted identifier form the
//!    request named them by, with the `MergeType` the request asked for or the
//!    modelled default when it asked for none — asserted over a table spanning the
//!    identifier forms, the whole `MergeType` value set, a config object without a
//!    `mergeType`, and `description` present and absent;
//! 2. creating the association SCHEDULES a merge, so it is reported in the
//!    non-terminal `MERGE_SCHEDULED` status and reaches `MERGE_SUCCESS` once the clock
//!    has made that merge due — with no call in between, and with a merge date that
//!    does not move on a later read;
//! 3. a source API is linked to a Merged API at most once: a second association of the
//!    same pair is refused 400 `BadRequestException` with the service's own duplicate
//!    sentence, whatever the clock still owes on the existing association — so racing
//!    callers leave exactly one winner and one association;
//! 4. an identifier that cannot name an API is a 400 `BadRequestException`, one that
//!    names no API this account holds (or an association under the wrong Merged API) a
//!    404 `NotFoundException`, and an API on the wrong side of the association a 400.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together. The suite reads the error-type header, which the lifecycle suite's
//! helpers do not carry, so it keeps its own `call`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the service reads it off the credential scope, so it
/// must appear in every ARN it mints.
pub(crate) const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// How long a row that must observe the far side of the scheduled merge waits: the
/// service's own span plus slack. Only the TEST waits — the handler stores the due
/// instant.
pub(crate) const PAST_THE_MERGE: Duration = Duration::from_millis(250);

/// The alphabet the service mints opaque ids over (`state::mint_id`): RFC 4648
/// base-32, lowercased.
pub(crate) const ID_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz234567";

/// One answer from the emulator: the parts a client branches on.
///
/// Shared with the suite that asserts the same association rule from the Merged-API
/// side, so both read the status, the error shape and the body the same way.
#[derive(Debug, Clone)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

/// A router with only this service mounted, over a fresh engine.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and return what the emulator answered.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// One created GraphQL API: the two forms an `…Identifier` member accepts.
#[derive(Debug, Clone)]
pub(crate) struct Api {
    pub(crate) id: String,
    pub(crate) arn: String,
}

/// Create one GraphQL API of `api_type` and return its id and ARN.
pub(crate) async fn create_api(router: &MultiServiceRouter, name: &str, api_type: &str) -> Api {
    let answer = call(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": name,
            "apiType": api_type,
            "authenticationType": "API_KEY",
            "queryDepthLimit": 0,
            "resolverCountLimit": 0,
            "xrayEnabled": false,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create {name:?} ({api_type}): expected 200, got {} {:?}",
        answer.status, answer.body
    );
    Api {
        id: answer.body["graphqlApi"]["apiId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        arn: answer.body["graphqlApi"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Create the pair an association needs: a standard source API and a Merged API.
pub(crate) async fn create_pair(router: &MultiServiceRouter, label: &str) -> (Api, Api) {
    let source = create_api(router, &format!("{label}-source"), state::GRAPHQL_API_TYPE).await;
    let merged = create_api(router, &format!("{label}-merged"), state::MERGED_API_TYPE).await;
    (source, merged)
}

/// `AssociateMergedGraphqlApi`'s path for `source_api_identifier`.
pub(crate) fn associate_path(source_api_identifier: &str) -> String {
    format!("/v1/sourceApis/{source_api_identifier}/mergedApiAssociations")
}

/// `GetSourceApiAssociation`'s path.
pub(crate) fn get_path(merged_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}")
}

/// One row of the association table: how the request names the Merged API and what it
/// asks for, and what the stored association must then report.
#[derive(Debug)]
struct Case {
    /// The row's label, used both in the API names and in every assertion message.
    label: &'static str,
    /// Whether the body names the Merged API by its ARN rather than by its `apiId` —
    /// the two forms `mergedApiIdentifier` documents.
    merged_by_arn: bool,
    description: Option<&'static str>,
    /// The `sourceApiAssociationConfig` the request carries, if any.
    config: Option<Value>,
    /// The `mergeType` the association must then report.
    expected_merge_type: &'static str,
}

/// The association table: the identifier forms `mergedApiIdentifier` accepts, every
/// value of the `MergeType` enum, a config object that names no `mergeType`, and
/// `description` present and absent.
///
/// Source for the `MergeType` value set and its default:
/// `smithy_query(query="shape", shape="SourceApiAssociationConfig")` —
/// "Valid merge types are `MANUAL_MERGE` (default) and `AUTO_MERGE`".
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "ids, no config",
            merged_by_arn: false,
            description: None,
            config: None,
            expected_merge_type: "MANUAL_MERGE",
        },
        Case {
            label: "merged api named by arn",
            merged_by_arn: true,
            description: Some("the association the ARN form created"),
            config: Some(json!({"mergeType": "MANUAL_MERGE"})),
            expected_merge_type: "MANUAL_MERGE",
        },
        Case {
            label: "auto merge",
            merged_by_arn: false,
            description: Some("subscribed to the source API's changes"),
            config: Some(json!({"mergeType": "AUTO_MERGE"})),
            expected_merge_type: "AUTO_MERGE",
        },
        Case {
            label: "config without a merge type",
            merged_by_arn: true,
            description: Some("configuré — déjà vu"),
            config: Some(json!({})),
            expected_merge_type: "MANUAL_MERGE",
        },
    ]
}

#[tokio::test]
async fn an_association_is_persisted_as_the_request_configured_it() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();

    for case in cases() {
        let (source, merged) = create_pair(&router, case.label).await;
        let merged_identifier = if case.merged_by_arn {
            merged.arn.clone()
        } else {
            merged.id.clone()
        };

        let mut body = json!({"mergedApiIdentifier": merged_identifier});
        if let Some(description) = case.description {
            body["description"] = json!(description);
        }
        if let Some(config) = case.config.clone() {
            body["sourceApiAssociationConfig"] = config;
        }

        let answer = call(
            &router,
            "POST",
            &associate_path(&source.id),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected AssociateMergedGraphqlApi 200, got {} {:?}",
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

        // The create SCHEDULES the merge: a non-terminal status, no merge performed
        // yet, and a detail that states where the association stands.
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

        // Round trip: the reader answers the SAME body the create answered. The
        // Merged API is addressed by its `apiId` here — the ARN form carries `:` and
        // `/`, which this path label does not take, so the id form is the addressable
        // one; the ARN form is exercised through the body member above.
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

        // A second association of the same pair has nothing to create: the pair is
        // already associated, whatever the clock still owes on that association.
        let again = call(
            &router,
            "POST",
            &associate_path(&source.id),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected a 400 BadRequestException for the second \
             association, got {} {} {:?}",
            case.label,
            again.status,
            again.error_type,
            again.body
        );
        // The refusal states the association that is in the way and names nothing of
        // the request.
        assert_eq!(
            again.body["message"].as_str(),
            Some("SourceApiAssociation already exists."),
            "case {:?}: expected the duplicate-association text, got {:?}",
            case.label,
            again.body["message"]
        );
    }
}

#[tokio::test]
async fn the_scheduled_merge_is_owed_by_the_clock_alone() {
    let router = router();
    let (source, merged) = create_pair(&router, "clock").await;

    let created = call(
        &router,
        "POST",
        &associate_path(&source.id),
        Some(json!({"mergedApiIdentifier": merged.id})),
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

    let settled = call(
        &router,
        "GET",
        &get_path(&merged.id, &association_id),
        None,
    )
    .await;
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

    // A read does not move it: the date is the instant the merge was due, not the
    // instant somebody looked.
    let reread = call(
        &router,
        "GET",
        &get_path(&merged.id, &association_id),
        None,
    )
    .await;
    assert_eq!(
        reread.body["sourceApiAssociation"], association,
        "case reread: expected the same settled association, got {:?}",
        reread.body["sourceApiAssociation"]
    );

    // The pair is associated, so a further association of it is a bad request naming
    // the association that already exists.
    let again = call(
        &router,
        "POST",
        &associate_path(&source.id),
        Some(json!({"mergedApiIdentifier": merged.id})),
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

/// How many callers race to associate one pair. More than the two a "both of them won"
/// bug needs, so a handler that lets several through is caught.
const RACERS: usize = 4;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_callers_leave_exactly_one_association() {
    let router = Arc::new(router());
    let (source, merged) = create_pair(&router, "race").await;

    let mut racers = Vec::with_capacity(RACERS);
    for _ in 0..RACERS {
        let router = Arc::clone(&router);
        let path = associate_path(&source.id);
        let body = json!({"mergedApiIdentifier": merged.id});
        racers.push(tokio::spawn(async move {
            call(&router, "POST", &path, Some(body)).await
        }));
    }

    let mut answers = Vec::with_capacity(RACERS);
    for racer in racers {
        answers.push(racer.await.expect("the racer task completes"));
    }

    let winners: Vec<&Answer> = answers
        .iter()
        .filter(|answer| answer.status == 200)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "case race: expected exactly one winner out of {RACERS}, got {answers:?}"
    );
    for loser in answers.iter().filter(|answer| answer.status != 200) {
        assert_eq!(
            (loser.status, loser.error_type.as_str()),
            (400, "BadRequestException"),
            "case race: expected every loser to be a 400 BadRequestException, got \
             {} {} {:?}",
            loser.status,
            loser.error_type,
            loser.body
        );
        assert_eq!(
            loser.body["message"].as_str(),
            Some("SourceApiAssociation already exists."),
            "case race: expected the duplicate-association text, got {:?}",
            loser.body["message"]
        );
    }

    // The winner's association is the one that is stored, and it is the only one.
    let association_id = winners[0].body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let read = call(
        &router,
        "GET",
        &get_path(&merged.id, &association_id),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case race: expected the winner's association to be readable, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["sourceApiAssociation"]["sourceApiId"].as_str(),
        Some(source.id.as_str()),
        "case race: expected the stored association to name the source API, got {:?}",
        read.body["sourceApiAssociation"]
    );
}

/// One refusal row: the request to make and the answer it must take.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    method: &'static str,
    /// `{source}`, `{merged}`, `{graphql}`, `{association}` are substituted with the
    /// fixture's ids before the call.
    path: &'static str,
    body: Option<&'static str>,
    status: u16,
    error_type: &'static str,
}

/// Every way an association request can name something that cannot be associated:
/// an identifier that is no identifier at all (400), one that names nothing (404), an
/// association under the wrong Merged API (404), and an API on the wrong side of the
/// association (400).
///
/// The over-long identifier is one character past the 26 an AppSync API id has
/// (`state::API_ID_LEN`), the bound `state::is_api_id` publishes.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "source identifier too long to be one",
            method: "POST",
            path: "/v1/sourceApis/aaaaaaaaaaaaaaaaaaaaaaaaaaa/mergedApiAssociations",
            body: Some(r#"{"mergedApiIdentifier": "{merged}"}"#),
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "merged identifier too long to be one",
            method: "POST",
            path: "/v1/sourceApis/{source}/mergedApiAssociations",
            body: Some(r#"{"mergedApiIdentifier": "aaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#),
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "source api this account does not hold",
            method: "POST",
            path: "/v1/sourceApis/abcdefghijklmnopqrstuvwxyz/mergedApiAssociations",
            body: Some(r#"{"mergedApiIdentifier": "{merged}"}"#),
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "merged api this account does not hold",
            method: "POST",
            path: "/v1/sourceApis/{source}/mergedApiAssociations",
            body: Some(r#"{"mergedApiIdentifier": "abcdefghijklmnopqrstuvwxyz"}"#),
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "merged api arn this account does not hold",
            method: "POST",
            path: "/v1/sourceApis/{source}/mergedApiAssociations",
            body: Some(
                r#"{"mergedApiIdentifier": "arn:aws:appsync:eu-west-1:000000000000:apis/abcdefghijklmnopqrstuvwxyz"}"#,
            ),
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "a standard api cannot be the merged side",
            method: "POST",
            path: "/v1/sourceApis/{source}/mergedApiAssociations",
            body: Some(r#"{"mergedApiIdentifier": "{graphql}"}"#),
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "a merged api cannot be the source side",
            method: "POST",
            path: "/v1/sourceApis/{merged}/mergedApiAssociations",
            body: Some(r#"{"mergedApiIdentifier": "{merged}"}"#),
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "read with an identifier too long to be one",
            method: "GET",
            path: "/v1/mergedApis/aaaaaaaaaaaaaaaaaaaaaaaaaaa/sourceApiAssociations/{association}",
            body: None,
            status: 400,
            error_type: "BadRequestException",
        },
        Refusal {
            label: "read under a merged api this account does not hold",
            method: "GET",
            path: "/v1/mergedApis/abcdefghijklmnopqrstuvwxyz/sourceApiAssociations/{association}",
            body: None,
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "read under another merged api",
            method: "GET",
            path: "/v1/mergedApis/{graphql}/sourceApiAssociations/{association}",
            body: None,
            status: 404,
            error_type: "NotFoundException",
        },
        Refusal {
            label: "read an association that was never created",
            method: "GET",
            path: "/v1/mergedApis/{merged}/sourceApiAssociations/abcdefghijklmnopqrstuvwxyz",
            body: None,
            status: 404,
            error_type: "NotFoundException",
        },
    ]
}

#[tokio::test]
async fn an_association_nothing_can_name_is_refused() {
    let router = router();
    // The fixtures every row is written against: the pair that CAN associate, plus a
    // second standard API that is on the wrong side of it.
    let (source, merged) = create_pair(&router, "refuse").await;
    let graphql = create_api(&router, "refuse-standard", state::GRAPHQL_API_TYPE).await;

    let created = call(
        &router,
        "POST",
        &associate_path(&source.id),
        Some(json!({"mergedApiIdentifier": merged.id})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case refusal fixture: expected 200, got {} {:?}",
        created.status, created.body
    );
    let association_id = created.body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let fill = |template: &str| {
        template
            .replace("{source}", &source.id)
            .replace("{merged}", &merged.id)
            .replace("{graphql}", &graphql.id)
            .replace("{association}", &association_id)
    };

    for refusal in refusals() {
        let body = refusal.body.map(|template| {
            serde_json::from_str::<Value>(&fill(template)).expect("the row's body is JSON")
        });
        let answer = call(&router, refusal.method, &fill(refusal.path), body).await;
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
