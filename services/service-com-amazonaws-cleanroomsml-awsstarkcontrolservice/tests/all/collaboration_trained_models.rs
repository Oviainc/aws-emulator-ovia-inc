// SPDX-License-Identifier: CC0-1.0

//! `ListCollaborationTrainedModels`, end to end over HTTP.
//!
//! The rule under test: the call reports exactly the persisted trained models whose
//! collaboration is the one the request names and whose region is the endpoint the
//! request reached, each projected from the stored record as a
//! `CollaborationTrainedModelSummary`, in the one stable order the store's ARN keys
//! give, paged by `maxResults` with `nextToken` resuming — so a collaboration holding
//! none is reported as an empty list rather than refused.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. `CreateTrainedModel` is still a scaffold stub with its own issue, so the
//! records each case reports on are written into the SAME store the handler reads through
//! `crate::state::service_state`: this crate's state module, reached from the one shared
//! engine the router was built over.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::ops::types::IncrementalTrainingDataChannelOutput;
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    trained_model_arn, ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState, PendingTransition,
    TrainedModel,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{signed_call, timestamp_seconds, Answer};

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "us-east-2";

/// A second endpoint, for the cases that pin that a Clean Rooms ML endpoint reports its
/// own region's resources and no others.
const OTHER_REGION: &str = "eu-central-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// Another account id, for the payer member — a 12-digit `AccountId`.
const PAYER_ACCOUNT: &str = "210987654321";

/// Every value `TrainedModelStatus` admits, from
/// `smithy_query(query="shape", shape="TrainedModelStatus")`. The cases below report
/// models carrying entries the issue never cited, so no handler can special-case one
/// spelling.
const TRAINED_MODEL_STATUSES: &[&str] = &[
    "CREATE_PENDING",
    "CREATE_IN_PROGRESS",
    "CREATE_FAILED",
    "ACTIVE",
    "DELETE_PENDING",
    "DELETE_IN_PROGRESS",
    "DELETE_FAILED",
    "INACTIVE",
    "CANCEL_PENDING",
    "CANCEL_IN_PROGRESS",
    "CANCEL_FAILED",
];

/// The largest page `ListCollaborationTrainedModelsRequest$maxResults` admits
/// (`@range(min: 1, max: 100)`), spelled out rather than imported so a wrong ceiling in
/// the handler fails.
const MAX_RESULTS_CEILING: i64 = 100;

/// One emulator: a router carrying only this service, over one shared engine, and the
/// handle to that engine — the store the handler reads is the store a case seeds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A `UUID` (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) built from
/// `index`, so a case can name as many distinct collaborations, memberships or versions
/// as it needs.
fn uuid(index: u32) -> String {
    format!("{index:08x}-2222-4333-8444-{index:012x}")
}

/// An instant that is the same on every run, so a seeded `createTime` is checked against
/// a value no clock can drift away from.
fn instant(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A `ConfiguredModelAlgorithmAssociationArn` under `membership`, matching that shape's
/// `…:membership/{uuid}/configured-model-algorithm-association/[-a-zA-Z0-9_/.]+` pattern.
fn association_arn(membership: &str, id: &str, region: &str) -> String {
    format!(
        "arn:aws:cleanrooms-ml:{region}:{ACCOUNT}:membership/{membership}\
         /configured-model-algorithm-association/{id}"
    )
}

/// One trained model to persist: the required members, with the optional ones absent. A
/// case sets only what it varies.
fn trained_model(collaboration: &str, membership: &str, region: &str, id: &str) -> TrainedModel {
    TrainedModel {
        arn: trained_model_arn(membership, id, region),
        region: region.to_string(),
        membership_identifier: membership.to_string(),
        collaboration_identifier: collaboration.to_string(),
        name: format!("model-{id}"),
        version_identifier: None,
        incremental_training_data_channels: None,
        configured_model_algorithm_association_arn: association_arn(membership, id, region),
        description: None,
        creator_account_id: ACCOUNT.to_string(),
        ml_model_training_payer_account_id: None,
        status: "ACTIVE".to_string(),
        pending: None,
        create_time: instant(1_760_000_000),
        update_time: instant(1_760_000_060),
    }
}

/// Write `model` into the store the handler reads, and answer the ARN it is keyed by.
fn seed(engine: &engine::InMemoryEngine, model: TrainedModel) -> String {
    let arn = model.arn.clone();
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .trained_models
        .insert(arn.clone(), model);
    arn
}

/// `ListCollaborationTrainedModels` on `collaboration`, through `region`'s endpoint, with
/// `query` appended verbatim.
async fn list(
    router: &MultiServiceRouter,
    region: &str,
    collaboration: &str,
    query: &str,
) -> Answer {
    signed_call(
        router,
        region,
        "GET",
        &format!("/collaborations/{collaboration}/trained-models{query}"),
        None,
    )
    .await
}

/// The summaries the call reported, in the order it reported them.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer.body["collaborationTrainedModels"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListCollaborationTrainedModels must return a collaborationTrainedModels list: \
                 {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `trainedModelArn` of each reported summary, in the reported order.
fn listed_arns(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary["trainedModelArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert `observed` is exactly the summary `expected` describes: every member reported
/// with the stored value, and no member the record does not hold.
fn assert_summary(case: &str, expected: &Value, observed: &Value) {
    let expected_members = expected
        .as_object()
        .expect("a case describes a summary as a JSON object");
    let observed_members = observed
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary object, got {observed:?}"));
    for (member, value) in expected_members {
        let reported = observed_members.get(member).unwrap_or_else(|| {
            panic!("case {case:?}: member {member:?} is missing from {observed:?}")
        });
        if member.ends_with("Time") {
            let expected_seconds = timestamp_seconds(value);
            let reported_seconds = timestamp_seconds(reported);
            assert!(
                (expected_seconds - reported_seconds).abs() < 0.001,
                "case {case:?}: member {member:?} was stored as {value:?} but is reported as \
                 {reported:?}"
            );
            continue;
        }
        assert_eq!(
            reported, value,
            "case {case:?}: member {member:?} was stored as {value:?} but is reported as \
             {reported:?}"
        );
    }
    let extra: Vec<&String> = observed_members
        .keys()
        .filter(|member| !expected_members.contains_key(*member))
        .collect();
    assert!(
        extra.is_empty(),
        "case {case:?}: the summary reports {extra:?}, which the stored record does not hold: \
         {observed:?}"
    );
}

/// Before any trained model is created, every collaboration holds none — and a
/// collaboration holding none is reported as an empty list, not refused: the operation
/// models no not-found error for a collaboration.
///
/// The rows differ along why the collaboration holds nothing: the store is empty, the
/// store holds models of other collaborations only, and the store holds this
/// collaboration's models in another region only.
#[tokio::test]
async fn a_collaboration_holding_no_trained_models_reports_an_empty_list() {
    let (router, engine) = emulator();
    let asked_about = uuid(1);

    let answer = list(&router, REGION, &asked_about, "").await;
    assert_eq!(
        answer.status, 200,
        "case \"empty store\": expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        summaries(&answer).is_empty(),
        "case \"empty store\": expected no summaries, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.at("nextToken"),
        Value::Null,
        "case \"empty store\": a page that ends the list carries no nextToken, got {:?}",
        answer.body
    );

    // Models of two OTHER collaborations, in the region being asked.
    for other in [2_u32, 3] {
        seed(
            &engine,
            trained_model(&uuid(other), &uuid(other), REGION, &format!("m{other}")),
        );
    }
    // And this collaboration's own model, in another region.
    seed(
        &engine,
        trained_model(&asked_about, &uuid(4), OTHER_REGION, "elsewhere"),
    );

    for case in [
        "only other collaborations hold trained models",
        "this collaboration's trained models are in another region",
    ] {
        let answer = list(&router, REGION, &asked_about, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            summaries(&answer).is_empty(),
            "case {case:?}: expected no summaries, got {:?}",
            answer.body
        );
    }
}

/// A trained model is reported through its OWN collaboration and its own region's
/// endpoint, and through no other: the same store serves each view only what it holds.
///
/// The rows differ in how many models a collaboration holds (none, one, several), in how
/// many memberships they are spread over, and in the region they were created in.
#[tokio::test]
async fn trained_models_are_reported_only_through_their_own_collaboration_and_region() {
    let (router, engine) = emulator();

    // (case, collaboration, region, how many memberships, models per membership)
    let cases: &[(&str, u32, &str, u32, u32)] = &[
        ("one model, one membership", 10, REGION, 1, 1),
        ("three models in one membership", 11, REGION, 1, 3),
        ("four models across two memberships", 12, REGION, 2, 2),
        ("one model in the other region", 13, OTHER_REGION, 1, 1),
        ("two memberships in the other region", 14, OTHER_REGION, 2, 1),
        ("no models at all", 15, REGION, 0, 0),
    ];

    let mut expected: Vec<(&str, String, &str, Vec<String>)> = Vec::new();
    for (case, index, region, memberships, per_membership) in cases.iter().copied() {
        let collaboration = uuid(index);
        let mut arns = Vec::new();
        for membership_index in 0..memberships {
            let membership = uuid(100 * index + membership_index);
            for model_index in 0..per_membership {
                arns.push(seed(
                    &engine,
                    trained_model(
                        &collaboration,
                        &membership,
                        region,
                        &format!("model-{membership_index}-{model_index}"),
                    ),
                ));
            }
        }
        arns.sort();
        expected.push((case, collaboration, region, arns));
    }

    for (case, collaboration, region, arns) in &expected {
        let answer = list(&router, region, collaboration, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            &listed_arns(&answer),
            arns,
            "case {case:?}: expected exactly this collaboration's trained models, in ARN order"
        );

        // The other endpoint reports none of them, unless it holds its own.
        let elsewhere = if *region == REGION { OTHER_REGION } else { REGION };
        let across = list(&router, elsewhere, collaboration, "").await;
        assert_eq!(
            across.status, 200,
            "case {case:?}: expected HTTP 200 from {elsewhere}, got {} with {:?}",
            across.status, across.body
        );
        assert!(
            listed_arns(&across).is_empty(),
            "case {case:?}: {elsewhere}'s endpoint must report none of {region}'s trained \
             models, got {:?}",
            across.body
        );
    }
}

/// Each reported summary is the `CollaborationTrainedModelSummary` projection of the
/// stored record: every member comes back as it was stored, and an optional member the
/// record does not hold is not reported at all.
///
/// The rows differ along what the projection generalises over — the status, whether the
/// description, the version identifier, the incremental training data channels and the
/// payer account are set, the name, and the create/update instants.
#[tokio::test]
async fn a_summary_projects_the_stored_record() {
    let (router, engine) = emulator();
    let collaboration = uuid(20);
    let membership = uuid(21);

    let mut expected: Vec<(String, Value)> = Vec::new();
    for (index, status) in TRAINED_MODEL_STATUSES.iter().enumerate() {
        let id = format!("model-{index}");
        let mut record = trained_model(&collaboration, &membership, REGION, &id);
        record.status = (*status).to_string();
        record.create_time = instant(1_700_000_000 + index as u64);
        record.update_time = instant(1_700_000_500 + index as u64);
        // Every other row carries the optional members, so a handler that always
        // reports them, or never does, fails here.
        if index % 2 == 0 {
            record.description = Some(format!("the {status} model"));
            record.version_identifier = Some(uuid(2_000 + index as u32));
            record.ml_model_training_payer_account_id = Some(PAYER_ACCOUNT.to_string());
            record.incremental_training_data_channels =
                Some(vec![IncrementalTrainingDataChannelOutput {
                    channel_name: format!("channel-{index}"),
                    version_identifier: Some(uuid(3_000 + index as u32)),
                    model_name: format!("base-model-{index}"),
                }]);
        }
        if index == 1 {
            // `NameString` admits 1..63 characters; a one-character name is the shortest
            // the model allows.
            record.name = "m".to_string();
        }

        let mut summary = json!({
            "trainedModelArn": &record.arn,
            "membershipIdentifier": &membership,
            "collaborationIdentifier": &collaboration,
            "name": &record.name,
            "configuredModelAlgorithmAssociationArn":
                &record.configured_model_algorithm_association_arn,
            "status": &record.status,
            "creatorAccountId": ACCOUNT,
            "createTime": record
                .create_time
                .duration_since(UNIX_EPOCH)
                .expect("the seeded instant is past the Unix epoch")
                .as_secs(),
            "updateTime": record
                .update_time
                .duration_since(UNIX_EPOCH)
                .expect("the seeded instant is past the Unix epoch")
                .as_secs(),
        });
        if let Some(description) = record.description.as_deref() {
            summary["description"] = json!(description);
        }
        if let Some(version) = record.version_identifier.as_deref() {
            summary["versionIdentifier"] = json!(version);
        }
        if let Some(payer) = record.ml_model_training_payer_account_id.as_deref() {
            summary["mlModelTrainingPayerAccountId"] = json!(payer);
        }
        if let Some(channels) = record.incremental_training_data_channels.as_ref() {
            summary["incrementalTrainingDataChannels"] = Value::Array(
                channels
                    .iter()
                    .map(|channel| {
                        json!({
                            "channelName": &channel.channel_name,
                            "versionIdentifier": &channel.version_identifier,
                            "modelName": &channel.model_name,
                        })
                    })
                    .collect(),
            );
        }
        expected.push((seed(&engine, record), summary));
    }

    let answer = list(&router, REGION, &collaboration, "").await;
    assert_eq!(
        answer.status, 200,
        "listing the collaboration must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = summaries(&answer);
    assert_eq!(
        reported.len(),
        expected.len(),
        "expected one summary per stored trained model, got {:?}",
        answer.body
    );
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    for ((arn, want), got) in expected.iter().zip(reported.iter()) {
        assert_summary(arn, want, got);
    }
}

/// `ListCollaborationTrainedModels` is `@paginated(items: collaborationTrainedModels)`:
/// whatever `maxResults` it is given, following `nextToken` to the end enumerates every
/// trained model of the collaboration exactly once, in one stable order, and no page is
/// longer than was asked for.
#[tokio::test]
async fn paging_enumerates_every_trained_model_exactly_once() {
    let (router, engine) = emulator();
    let collaboration = uuid(30);
    let mut expected = Vec::new();
    for index in 0..7 {
        expected.push(seed(
            &engine,
            trained_model(
                &collaboration,
                &uuid(31 + index % 2),
                REGION,
                &format!("model-{index}"),
            ),
        ));
    }
    expected.sort();

    // Rows differ in the page size: none at all, one, a size that does not divide the
    // total, the exact total, and the ceiling the model admits.
    let cases: &[(&str, Option<i64>)] = &[
        ("no maxResults at all", None),
        ("one at a time", Some(1)),
        ("two at a time", Some(2)),
        ("three at a time, which does not divide seven", Some(3)),
        ("exactly the number persisted", Some(7)),
        ("the ceiling the model admits", Some(MAX_RESULTS_CEILING)),
    ];

    for (case, max_results) in cases.iter().copied() {
        let page_limit = max_results.unwrap_or(MAX_RESULTS_CEILING);
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more page than the smallest size could need, so a token that never
        // terminates fails here instead of hanging.
        for page in 0..=expected.len() + 1 {
            let mut query = String::new();
            if let Some(max) = max_results {
                query.push_str(&format!("?maxResults={max}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list(&router, REGION, &collaboration, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {page} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page_arns = listed_arns(&answer);
            assert!(
                i64::try_from(page_arns.len()).expect("a page fits in i64") <= page_limit,
                "case {case:?}: page {page} is longer than {page_limit}, got {page_arns:?}"
            );
            walked.extend(page_arns);
            token = answer
                .at("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: paging must terminate, still holding {token:?} after {} pages",
            expected.len() + 2
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: following nextToken must enumerate every trained model exactly \
             once, in the unpaged order"
        );
    }
}

/// `maxResults` is modelled `@range(min: 1, max: 100)`: each published bound is served
/// and a value past either one is refused.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let (router, engine) = emulator();
    let collaboration = uuid(40);
    seed(
        &engine,
        trained_model(&collaboration, &uuid(41), REGION, "model-0"),
    );

    for accepted in [1, MAX_RESULTS_CEILING] {
        let answer = list(
            &router,
            REGION,
            &collaboration,
            &format!("?maxResults={accepted}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {accepted:?}: a published bound must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_arns(&answer).len(),
            1,
            "case {accepted:?}: expected the one persisted trained model, got {:?}",
            answer.body
        );
    }

    for refused in [0, MAX_RESULTS_CEILING + 1] {
        let answer = list(
            &router,
            REGION,
            &collaboration,
            &format!("?maxResults={refused}"),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {refused:?}: a value past the modelled range must be refused with HTTP 400, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {refused:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

/// A `nextToken` this operation never issued is refused rather than silently serving the
/// first page, and a token of the right shape past the last trained model reports the
/// empty tail.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let (router, engine) = emulator();
    let collaboration = uuid(50);
    let membership = uuid(51);
    seed(
        &engine,
        trained_model(&collaboration, &membership, REGION, "model-0"),
    );

    // Rows differ along how the token is wrong: not an ARN at all, an ARN of another
    // resource type in this service, a membership identifier that is not a UUID, a
    // missing trained-model segment, an empty resource id and a short account id.
    let cases: &[(&str, String)] = &[
        ("not an ARN at all", "page-2".to_string()),
        (
            "a training-dataset ARN",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/abc"),
        ),
        (
            "an ML input channel ARN",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /ml-input-channel/channel-0"
            ),
        ),
        (
            "a membership that is not a UUID",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/not-a-uuid\
                 /trained-model/model-0"
            ),
        ),
        (
            "no trained-model segment",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}"),
        ),
        (
            "an empty trained-model id",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /trained-model/"
            ),
        ),
        (
            "a short account id",
            format!("arn:aws:cleanrooms-ml:{REGION}:00000:membership/{membership}/trained-model/m"),
        ),
    ];

    for (case, token) in cases {
        let answer = list(
            &router,
            REGION,
            &collaboration,
            &format!("?nextToken={token}"),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued nextToken must be refused with HTTP 400, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // A token this operation DID issue the shape of — a trained model ARN past every
    // persisted one — is honoured, and reports the empty tail.
    let tail = trained_model_arn(&uuid(52), "zzzz", REGION);
    let answer = list(&router, REGION, &collaboration, &format!("?nextToken={tail}")).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed token past the last trained model must be honoured, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        listed_arns(&answer).is_empty(),
        "a token past the last trained model reports no models, got {:?}",
        answer.body
    );
}

/// A trained model is trained asynchronously, so the status a reader reports is the one
/// the clock owes it now: a transition already due is materialised, one not yet due is
/// not, and reading changes nothing a later caller sees.
#[tokio::test]
async fn the_reported_status_is_the_one_the_clock_owes() {
    let (router, engine) = emulator();
    let collaboration = uuid(60);
    let membership = uuid(61);
    let now = SystemTime::now();

    // (case, the stored status, when the transition falls due, the status it moves to,
    //  the status the call must report)
    let cases: &[(&str, &str, Option<Duration>, &str, &str)] = &[
        ("no transition owed", "ACTIVE", None, "ACTIVE", "ACTIVE"),
        (
            "a transition already due",
            "CREATE_PENDING",
            Some(Duration::from_secs(0)),
            "ACTIVE",
            "ACTIVE",
        ),
        (
            "a transition not yet due",
            "CREATE_PENDING",
            Some(Duration::from_secs(3_600)),
            "ACTIVE",
            "CREATE_PENDING",
        ),
        (
            "a cancel not yet due",
            "CANCEL_PENDING",
            Some(Duration::from_secs(3_600)),
            "INACTIVE",
            "CANCEL_PENDING",
        ),
    ];

    let mut expected = Vec::new();
    for (index, (case, stored, due_in, moves_to, reported)) in cases.iter().copied().enumerate() {
        let mut record = trained_model(
            &collaboration,
            &membership,
            REGION,
            &format!("model-{index}"),
        );
        record.status = stored.to_string();
        record.pending = due_in.map(|due_in| PendingTransition {
            // A transition "already due" is one whose instant has passed, so it is owed
            // whether or not anybody called.
            due: if due_in.is_zero() {
                now - Duration::from_secs(1)
            } else {
                now + due_in
            },
            status: moves_to.to_string(),
        });
        expected.push((case, seed(&engine, record), reported));
    }

    for attempt in 0..2 {
        let answer = list(&router, REGION, &collaboration, "").await;
        assert_eq!(
            answer.status, 200,
            "attempt {attempt}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = summaries(&answer);
        for (case, arn, want) in &expected {
            let summary = reported
                .iter()
                .find(|summary| summary["trainedModelArn"].as_str() == Some(arn.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {arn} is missing from {:?}", answer.body)
                });
            assert_eq!(
                summary["status"].as_str(),
                Some(*want),
                "case {case:?}: attempt {attempt} expected status {want:?}, got {summary:?}"
            );
        }
    }
}
