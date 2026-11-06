// SPDX-License-Identifier: CC0-1.0

//! `ListCollaborationMLInputChannels`, end to end over HTTP.
//!
//! The rule under test: the call reports exactly the persisted ML input channels whose
//! collaboration is the one the request names and whose region is the endpoint the
//! request reached, each projected from the stored record as a
//! `CollaborationMLInputChannelSummary`, in the one stable order the store's ARN keys
//! give, paged by `maxResults` with `nextToken` resuming — so a collaboration holding
//! none is reported as an empty list rather than refused.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. `CreateMLInputChannel` is still a scaffold stub with its own issue, so
//! the records each case reports on are written into the SAME store the handler reads
//! through `crate::state::service_state`: this crate's state module, reached from the one
//! shared engine the router was built over.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::ops::types::PayerConfiguration;
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    ml_input_channel_arn, ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState, MlInputChannel,
    PendingTransition,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{signed_call, timestamp_seconds, Answer};

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "us-west-2";

/// A second endpoint, for the cases that pin that a Clean Rooms ML endpoint reports its
/// own region's resources and no others.
const OTHER_REGION: &str = "ap-southeast-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// Every value `MLInputChannelStatus` admits, from
/// `smithy_query(query="shape", shape="MLInputChannelStatus")`. The cases below report
/// channels carrying entries the issue never cited, so no handler can special-case one
/// spelling.
const CHANNEL_STATUSES: &[&str] = &[
    "CREATE_PENDING",
    "CREATE_IN_PROGRESS",
    "CREATE_FAILED",
    "ACTIVE",
    "DELETE_PENDING",
    "DELETE_IN_PROGRESS",
    "DELETE_FAILED",
    "INACTIVE",
];

/// The largest page `ListCollaborationMLInputChannelsRequest$maxResults` admits
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
/// `index`, so a case can name as many distinct collaborations or memberships as it needs.
fn uuid(index: u32) -> String {
    format!("{index:08x}-1111-4222-8333-{index:012x}")
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

/// One ML input channel to persist: the required members, with the optional ones absent.
/// A case sets only what it varies.
fn channel(collaboration: &str, membership: &str, region: &str, id: &str) -> MlInputChannel {
    MlInputChannel {
        arn: ml_input_channel_arn(membership, id, region),
        region: region.to_string(),
        membership_identifier: membership.to_string(),
        collaboration_identifier: collaboration.to_string(),
        name: format!("channel-{id}"),
        // `ConfiguredModelAlgorithmAssociationArnList` is bounded @length(min: 1, max: 1).
        configured_model_algorithm_associations: vec![association_arn(membership, id, region)],
        description: None,
        payer_configuration: None,
        creator_account_id: ACCOUNT.to_string(),
        tags: Default::default(),
        status: "ACTIVE".to_string(),
        pending: None,
        create_time: instant(1_750_000_000),
        update_time: instant(1_750_000_060),
    }
}

/// Write `channel` into the store the handler reads, and answer the ARN it is keyed by.
fn seed(engine: &engine::InMemoryEngine, channel: MlInputChannel) -> String {
    let arn = channel.arn.clone();
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .ml_input_channels
        .insert(arn.clone(), channel);
    arn
}

/// `ListCollaborationMLInputChannels` on `collaboration`, through `region`'s endpoint,
/// with `query` appended verbatim.
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
        &format!("/collaborations/{collaboration}/ml-input-channels{query}"),
        None,
    )
    .await
}

/// The summaries the call reported, in the order it reported them.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer.body["collaborationMLInputChannelsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListCollaborationMLInputChannels must return a \
                 collaborationMLInputChannelsList: {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `mlInputChannelArn` of each reported summary, in the reported order.
fn listed_arns(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary["mlInputChannelArn"]
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
        let reported = observed_members
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: member {member:?} is missing from {observed:?}"));
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

/// Before any ML input channel is created, every collaboration holds none — and a
/// collaboration holding none is reported as an empty list, not refused: the operation
/// models no not-found error for a collaboration.
///
/// The rows differ along why the collaboration holds nothing: the store is empty, the
/// store holds channels of other collaborations only, and the store holds this
/// collaboration's channels in another region only.
#[tokio::test]
async fn a_collaboration_holding_no_channels_reports_an_empty_list() {
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

    // Channels of two OTHER collaborations, in the region being asked.
    for other in [2_u32, 3] {
        seed(
            &engine,
            channel(&uuid(other), &uuid(other), REGION, &format!("c{other}")),
        );
    }
    // And this collaboration's own channel, in another region.
    seed(
        &engine,
        channel(&asked_about, &uuid(4), OTHER_REGION, "elsewhere"),
    );

    for case in [
        "only other collaborations hold channels",
        "this collaboration's channels are in another region",
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

/// A channel is reported through its OWN collaboration and its own region's endpoint, and
/// through no other: the same store serves each view only what it holds.
///
/// The rows differ in how many channels a collaboration holds (none, one, several), in
/// how many memberships they are spread over, and in the region they were created in.
#[tokio::test]
async fn channels_are_reported_only_through_their_own_collaboration_and_region() {
    let (router, engine) = emulator();

    // (case, collaboration, region, how many memberships, channels per membership)
    let cases: &[(&str, u32, &str, u32, u32)] = &[
        ("one channel, one membership", 10, REGION, 1, 1),
        ("three channels in one membership", 11, REGION, 1, 3),
        ("four channels across two memberships", 12, REGION, 2, 2),
        ("one channel in the other region", 13, OTHER_REGION, 1, 1),
        ("two memberships in the other region", 14, OTHER_REGION, 2, 1),
        ("no channels at all", 15, REGION, 0, 0),
    ];

    let mut expected: Vec<(&str, String, &str, Vec<String>)> = Vec::new();
    for (case, index, region, memberships, per_membership) in cases.iter().copied() {
        let collaboration = uuid(index);
        let mut arns = Vec::new();
        for membership_index in 0..memberships {
            let membership = uuid(100 * index + membership_index);
            for channel_index in 0..per_membership {
                arns.push(seed(
                    &engine,
                    channel(
                        &collaboration,
                        &membership,
                        region,
                        &format!("channel-{membership_index}-{channel_index}"),
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
            "case {case:?}: expected exactly this collaboration's channels, in ARN order"
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
            "case {case:?}: {elsewhere}'s endpoint must report none of {region}'s channels, got \
             {:?}",
            across.body
        );
    }
}

/// Each reported summary is the `CollaborationMLInputChannelSummary` projection of the
/// stored record: every member comes back as it was stored, and an optional member the
/// record does not hold is not reported at all.
///
/// The rows differ along what the projection generalises over — the status, whether the
/// description and the payer configuration are set, the name, and the create/update
/// instants.
#[tokio::test]
async fn a_summary_projects_the_stored_record() {
    let (router, engine) = emulator();
    let collaboration = uuid(20);
    let membership = uuid(21);

    let mut expected: Vec<(String, Value)> = Vec::new();
    for (index, status) in CHANNEL_STATUSES.iter().enumerate() {
        let id = format!("channel-{index}");
        let mut record = channel(&collaboration, &membership, REGION, &id);
        record.status = (*status).to_string();
        record.create_time = instant(1_700_000_000 + index as u64);
        record.update_time = instant(1_700_000_500 + index as u64);
        // Every other row carries the optional members, so a handler that always
        // reports them, or never does, fails here.
        if index % 2 == 0 {
            record.description = Some(format!("the {status} channel"));
            record.payer_configuration = Some(PayerConfiguration {
                compute_payer_account_id: Some(ACCOUNT.to_string()),
                synthetic_data_payer_account_id: None,
            });
        }
        if index == 1 {
            // `NameString` admits 1..63 characters; a one-character name is the shortest
            // the model allows.
            record.name = "c".to_string();
        }

        let mut summary = json!({
            "mlInputChannelArn": &record.arn,
            "membershipIdentifier": &membership,
            "collaborationIdentifier": &collaboration,
            "name": &record.name,
            "configuredModelAlgorithmAssociations": &record.configured_model_algorithm_associations,
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
        if record.payer_configuration.is_some() {
            summary["payerConfiguration"] = json!({ "computePayerAccountId": ACCOUNT });
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
        "expected one summary per stored channel, got {:?}",
        answer.body
    );
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    for ((arn, want), got) in expected.iter().zip(reported.iter()) {
        assert_summary(arn, want, got);
    }
}

/// `ListCollaborationMLInputChannels` is `@paginated(items:
/// collaborationMLInputChannelsList)`: whatever `maxResults` it is given, following
/// `nextToken` to the end enumerates every channel of the collaboration exactly once, in
/// one stable order, and no page is longer than was asked for.
#[tokio::test]
async fn paging_enumerates_every_channel_exactly_once() {
    let (router, engine) = emulator();
    let collaboration = uuid(30);
    let mut expected = Vec::new();
    for index in 0..7 {
        expected.push(seed(
            &engine,
            channel(
                &collaboration,
                &uuid(31 + index % 2),
                REGION,
                &format!("channel-{index}"),
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
            "case {case:?}: following nextToken must enumerate every channel exactly once, in \
             the unpaged order"
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
        channel(&collaboration, &uuid(41), REGION, "channel-0"),
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
            "case {accepted:?}: expected the one persisted channel, got {:?}",
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
/// first page, and a token of the right shape past the last channel reports the empty
/// tail.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let (router, engine) = emulator();
    let collaboration = uuid(50);
    let membership = uuid(51);
    seed(
        &engine,
        channel(&collaboration, &membership, REGION, "channel-0"),
    );

    // Rows differ along how the token is wrong: not an ARN at all, an ARN of another
    // resource type in this service, a membership identifier that is not a UUID, a
    // missing channel segment, and an empty resource id.
    let cases: &[(&str, String)] = &[
        ("not an ARN at all", "page-2".to_string()),
        (
            "a training-dataset ARN",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/abc"),
        ),
        (
            "a membership that is not a UUID",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/not-a-uuid\
                 /ml-input-channel/channel-0"
            ),
        ),
        (
            "no channel segment",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}"),
        ),
        (
            "an empty channel id",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /ml-input-channel/"
            ),
        ),
        (
            "a short account id",
            format!("arn:aws:cleanrooms-ml:{REGION}:00000:membership/{membership}/ml-input-channel/c"),
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

    // A token this operation DID issue the shape of — a channel ARN past every persisted
    // one — is honoured, and reports the empty tail.
    let tail = ml_input_channel_arn(&uuid(52), "zzzz", REGION);
    let answer = list(&router, REGION, &collaboration, &format!("?nextToken={tail}")).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed token past the last channel must be honoured, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        listed_arns(&answer).is_empty(),
        "a token past the last channel reports no channels, got {:?}",
        answer.body
    );
}

/// An ML input channel is created asynchronously, so the status a reader reports is the
/// one the clock owes it now: a transition already due is materialised, one not yet due
/// is not, and reading changes nothing a later caller sees.
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
            "a delete not yet due",
            "DELETE_PENDING",
            Some(Duration::from_secs(3_600)),
            "INACTIVE",
            "DELETE_PENDING",
        ),
    ];

    let mut expected = Vec::new();
    for (index, (case, stored, due_in, moves_to, reported)) in cases.iter().copied().enumerate() {
        let mut record = channel(
            &collaboration,
            &membership,
            REGION,
            &format!("channel-{index}"),
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
                .find(|summary| summary["mlInputChannelArn"].as_str() == Some(arn.as_str()))
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
