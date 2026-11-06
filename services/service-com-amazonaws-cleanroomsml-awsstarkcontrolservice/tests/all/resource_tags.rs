// SPDX-License-Identifier: CC0-1.0

//! Clean Rooms ML resource tagging, end to end over HTTP.
//!
//! The rule under test: a resource's tags are part of that resource. `TagResource` writes
//! the keys the request names onto the record its ARN names and leaves every other key as
//! it was, `UntagResource` removes the keys it names and nothing else, and
//! `ListTagsForResource` reports exactly the tag set that record currently holds — for
//! every kind of resource a `TaggableArn` names, in the region whose endpoint holds it. An
//! ARN this endpoint holds no resource for is refused as `ResourceNotFoundException` (404)
//! by all three operations rather than answered with an empty tag set.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows of each table differ along what the rule generalises over: the
//! kind of resource tagged, the tags it was created with, the keys and values a tag
//! carries, how many tags there are, the order of the calls and the region the request is
//! signed for.
//!
//! The resource kinds whose create is still a scaffold stub of its own are seeded straight
//! into the store the handlers read, the way the sibling collaboration suites seed theirs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::ops::types::{
    InferenceOutputConfiguration, InferenceReceiverMember,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    ml_input_channel_arn, trained_model_arn, trained_model_inference_job_arn,
    ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState, MlInputChannel, TrainedModel,
    TrainedModelInferenceJob,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{label, signed_call, Answer};

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "us-east-2";

/// A second endpoint, for the cases that pin that a Clean Rooms ML endpoint serves its own
/// region's resources and no others.
const OTHER_REGION: &str = "eu-central-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// Another account id, for the ARNs that name a resource outside this emulator.
const OTHER_ACCOUNT: &str = "210987654321";

/// An IAM role ARN matching `IamRoleArn`'s `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CleanRoomsMLRole";

/// A membership id matching the modelled `UUID` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, which the
/// membership-nested `TaggableArn` forms carry.
const MEMBERSHIP: &str = "11111111-2222-4333-8444-555555555555";

/// The collaboration that membership belongs to, in the same form.
const COLLABORATION: &str = "99999999-8888-4777-8666-555555555555";

/// The largest key `TagKey` admits (`@length(min: 1, max: 128)`) and the largest value
/// `TagValue` admits (`@length(min: 0, max: 256)`), from
/// `smithy_query(query="shape", shape="TagMap")`.
const MAX_KEY_LENGTH: usize = 128;
const MAX_VALUE_LENGTH: usize = 256;

/// The message the tagging path's not-found refusal carries, spelled out here rather than
/// imported so a wrong message in the handler fails.
const TAGGED_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// One emulator: a router carrying only this service, over one shared engine. The engine
/// is handed back so a case can seed the resource kinds whose own create is still a stub.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// An instant that is the same on every run, for the seeded records.
fn instant(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

async fn list_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    signed_call(router, region, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag_resource(router: &MultiServiceRouter, region: &str, arn: &str, tags: Value) -> Answer {
    signed_call(
        router,
        region,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

async fn untag_resource(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    keys: &[String],
) -> Answer {
    // `UntagResourceRequest$tagKeys` is `@httpQuery("tagKeys")`, so each key is one
    // repetition of that parameter.
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect::<Vec<_>>()
        .join("&");
    signed_call(
        router,
        region,
        "DELETE",
        &format!("/tags/{}?{query}", label(arn)),
        None,
    )
    .await
}

/// The tag set an answer reports, ordered so two answers compare by content.
fn reported_tags(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .at("tags")
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A tag set as the wire carries it.
fn tags_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// A tag set from pairs, so a case names only what it varies.
fn tag_set(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One `Dataset`, the only member `CreateTrainingDatasetRequest$trainingData` admits
/// (`@length(min: 1, max: 1)`).
fn training_data() -> Value {
    json!({
        "type": "INTERACTIONS",
        "inputConfig": {
            "schema": [{ "columnName": "user_id", "columnTypes": ["USER_ID"] }],
            "dataSource": {
                "glueDataSource": {
                    "tableName": "interactions",
                    "databaseName": "clean_rooms_demo",
                },
            },
        },
    })
}

/// The ARN a created resource was minted under, from the create's own output member.
fn created_arn(answer: &Answer, member: &str, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the fixture create must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    let arn = answer.at(member).as_str().unwrap_or_default().to_string();
    assert!(
        !arn.is_empty(),
        "case {case:?}: the create must report {member}, got {:?}",
        answer.body
    );
    arn
}

async fn create_training_dataset(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> String {
    let mut request = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "trainingData": [training_data()],
    });
    if let Some(tags) = tags {
        request["tags"] = tags_json(tags);
    }
    let answer = signed_call(router, region, "POST", "/training-dataset", Some(request)).await;
    created_arn(&answer, "trainingDatasetArn", name)
}

async fn create_audience_model(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    training_dataset: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> String {
    let mut request = json!({ "name": name, "trainingDatasetArn": training_dataset });
    if let Some(tags) = tags {
        request["tags"] = tags_json(tags);
    }
    let answer = signed_call(router, region, "POST", "/audience-model", Some(request)).await;
    created_arn(&answer, "audienceModelArn", name)
}

async fn create_configured_model_algorithm(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> String {
    let mut request = json!({ "name": name, "roleArn": ROLE_ARN });
    if let Some(tags) = tags {
        request["tags"] = tags_json(tags);
    }
    let answer = signed_call(
        router,
        region,
        "POST",
        "/configured-model-algorithms",
        Some(request),
    )
    .await;
    created_arn(&answer, "configuredModelAlgorithmArn", name)
}

/// A configured model algorithm association ARN under this suite's membership, the shape
/// `ConfiguredModelAlgorithmAssociationArnList` carries.
fn association_arn(region: &str, id: &str) -> String {
    format!(
        "arn:aws:cleanrooms-ml:{region}:{ACCOUNT}:membership/{MEMBERSHIP}\
         /configured-model-algorithm-association/{id}"
    )
}

/// Persist one ML input channel carrying `tags`, and answer the ARN it is keyed by.
fn seed_ml_input_channel(
    engine: &engine::InMemoryEngine,
    region: &str,
    id: &str,
    tags: BTreeMap<String, String>,
) -> String {
    let arn = ml_input_channel_arn(MEMBERSHIP, id, region);
    let channel = MlInputChannel {
        arn: arn.clone(),
        region: region.to_string(),
        membership_identifier: MEMBERSHIP.to_string(),
        collaboration_identifier: COLLABORATION.to_string(),
        name: format!("channel-{id}"),
        configured_model_algorithm_associations: vec![association_arn(region, id)],
        description: None,
        payer_configuration: None,
        creator_account_id: ACCOUNT.to_string(),
        tags,
        status: "ACTIVE".to_string(),
        pending: None,
        create_time: instant(1_750_000_000),
        update_time: instant(1_750_000_060),
    };
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .ml_input_channels
        .insert(arn.clone(), channel);
    arn
}

/// Persist one trained model carrying `tags`, and answer the ARN it is keyed by.
fn seed_trained_model(
    engine: &engine::InMemoryEngine,
    region: &str,
    id: &str,
    tags: BTreeMap<String, String>,
) -> String {
    let arn = trained_model_arn(MEMBERSHIP, id, region);
    let model = TrainedModel {
        arn: arn.clone(),
        region: region.to_string(),
        membership_identifier: MEMBERSHIP.to_string(),
        collaboration_identifier: COLLABORATION.to_string(),
        name: format!("model-{id}"),
        version_identifier: None,
        incremental_training_data_channels: None,
        configured_model_algorithm_association_arn: association_arn(region, id),
        description: None,
        creator_account_id: ACCOUNT.to_string(),
        ml_model_training_payer_account_id: None,
        tags,
        status: "ACTIVE".to_string(),
        pending: None,
        create_time: instant(1_760_000_000),
        update_time: instant(1_760_000_060),
    };
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .trained_models
        .insert(arn.clone(), model);
    arn
}

/// Persist one trained model inference job carrying `tags`, and answer the ARN it is keyed
/// by.
fn seed_inference_job(
    engine: &engine::InMemoryEngine,
    region: &str,
    id: &str,
    tags: BTreeMap<String, String>,
) -> String {
    let arn = trained_model_inference_job_arn(MEMBERSHIP, id, region);
    let job = TrainedModelInferenceJob {
        arn: arn.clone(),
        region: region.to_string(),
        membership_identifier: MEMBERSHIP.to_string(),
        collaboration_identifier: COLLABORATION.to_string(),
        name: format!("job-{id}"),
        trained_model_arn: trained_model_arn(MEMBERSHIP, id, region),
        trained_model_version_identifier: None,
        configured_model_algorithm_association_arn: None,
        output_configuration: InferenceOutputConfiguration {
            accept: "application/json".to_string(),
            members: vec![InferenceReceiverMember {
                account_id: OTHER_ACCOUNT.to_string(),
            }],
        },
        description: None,
        creator_account_id: ACCOUNT.to_string(),
        ml_model_inference_payer_account_id: None,
        metrics_status: None,
        metrics_status_details: None,
        logs_status: None,
        logs_status_details: None,
        tags,
        status: "ACTIVE".to_string(),
        pending: None,
        create_time: instant(1_770_000_000),
        update_time: instant(1_770_000_060),
    };
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .trained_model_inference_jobs
        .insert(arn.clone(), job);
    arn
}

/// One resource to tag: what it is, the tags it already carries, and the read that reports
/// that same resource when this emulator implements one.
struct Target {
    case: &'static str,
    arn: String,
    initial: BTreeMap<String, String>,
    own_read: Option<String>,
}

/// One target of every kind a `TaggableArn` can name a persisted resource of, each
/// carrying a different starting tag set.
async fn targets(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Vec<Target> {
    let dataset_tags = tag_set(&[("env", "test"), ("team", "ml")]);
    let dataset = create_training_dataset(router, REGION, "tagged-dataset", Some(&dataset_tags)).await;
    let model_tags = tag_set(&[("env", "prod")]);
    let model =
        create_audience_model(router, REGION, "tagged-model", &dataset, Some(&model_tags)).await;
    let algorithm = create_configured_model_algorithm(router, REGION, "tagged-algorithm", None).await;
    let channel_tags = tag_set(&[("owner", "analytics"), ("tier", "gold"), ("note", "")]);
    let channel = seed_ml_input_channel(engine, REGION, "channel-0", channel_tags.clone());
    let trained_tags = tag_set(&[("env", "staging")]);
    let trained = seed_trained_model(engine, REGION, "model-0", trained_tags.clone());
    let job = seed_inference_job(engine, REGION, "job-0", BTreeMap::new());

    vec![
        Target {
            case: "a training dataset created with two tags",
            own_read: Some(format!("/training-dataset/{}", label(&dataset))),
            arn: dataset,
            initial: dataset_tags,
        },
        Target {
            case: "an audience model created with one tag",
            own_read: Some(format!("/audience-model/{}", label(&model))),
            arn: model,
            initial: model_tags,
        },
        Target {
            case: "a configured model algorithm created with no tags",
            own_read: Some(format!("/configured-model-algorithms/{}", label(&algorithm))),
            arn: algorithm,
            initial: BTreeMap::new(),
        },
        Target {
            case: "a membership-nested ML input channel carrying three tags",
            own_read: None,
            arn: channel,
            initial: channel_tags,
        },
        Target {
            case: "a membership-nested trained model",
            own_read: None,
            arn: trained,
            initial: trained_tags,
        },
        Target {
            case: "a membership-nested inference job carrying no tags",
            own_read: None,
            arn: job,
            initial: BTreeMap::new(),
        },
    ]
}

/// `ListTagsForResource` reports the tag set the resource holds, `TagResource` adds to it
/// without disturbing the keys it does not name, and `UntagResource` removes exactly the
/// keys it names — for every kind of resource a `TaggableArn` names.
#[tokio::test]
async fn tags_round_trip_through_every_kind_of_taggable_resource() {
    let (router, engine) = emulator();
    for target in targets(&router, &engine).await {
        let case = target.case;

        // The tag set a resource was created (or persisted) with is what is reported.
        let listed = list_tags(&router, REGION, &target.arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed),
            target.initial,
            "case {case:?}: ListTagsForResource must report the tags the resource carries, \
             got {:?}",
            listed.body
        );

        // A write that adds a key, overwrites one the resource already carries and sets an
        // empty value.
        let mut applied = tag_set(&[("cost-centre", "42"), ("empty-value", "")]);
        if let Some(existing) = target.initial.keys().next() {
            applied.insert(existing.clone(), "rewritten".to_string());
        }
        let tagged = tag_resource(&router, REGION, &target.arn, tags_json(&applied)).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {} with {:?}",
            tagged.status, tagged.body
        );

        let mut expected = target.initial.clone();
        expected.extend(applied.clone());
        let listed = list_tags(&router, REGION, &target.arn).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: every key the request named must carry the value it gave and \
             every other key must be unchanged, got {:?}",
            listed.body
        );

        // The resource's own read reports that same tag set: the tags live on the one
        // record, not in a second copy.
        if let Some(uri) = &target.own_read {
            let own = signed_call(&router, REGION, "GET", uri, None).await;
            assert_eq!(
                own.status, 200,
                "case {case:?}: expected HTTP 200 from the resource's own read, got {} \
                 with {:?}",
                own.status, own.body
            );
            assert_eq!(
                reported_tags(&own),
                expected,
                "case {case:?}: the resource's own read must report the same tags as \
                 ListTagsForResource, got {:?}",
                own.at("tags")
            );
        }

        // Reading tags changes nothing a later caller sees.
        let again = list_tags(&router, REGION, &target.arn).await;
        assert_eq!(
            again.body, listed.body,
            "case {case:?}: reading the tags must not change them, got {:?} after {:?}",
            again.body, listed.body
        );

        // Removing one key the resource carries and one it never carried leaves exactly
        // the rest — the second is what makes the call idempotent.
        let removed = vec!["cost-centre".to_string(), "never-set".to_string()];
        let untagged = untag_resource(&router, REGION, &target.arn, &removed).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: expected HTTP 200 from UntagResource, got {} with {:?}",
            untagged.status, untagged.body
        );
        expected.remove("cost-centre");
        let listed = list_tags(&router, REGION, &target.arn).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: UntagResource must remove only the keys it names, got {:?}",
            listed.body
        );

        // Repeating the same removal is the same answer and the same state.
        let untagged = untag_resource(&router, REGION, &target.arn, &removed).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: repeating UntagResource must answer HTTP 200, got {} with {:?}",
            untagged.status, untagged.body
        );
        let listed = list_tags(&router, REGION, &target.arn).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: repeating UntagResource must leave the tag set as it was, got \
             {:?}",
            listed.body
        );
    }
}

/// Whatever `TagMap` admits round-trips verbatim: the longest key and value, an empty
/// value, punctuation and characters outside ASCII, and a tag set far larger than one row.
#[tokio::test]
async fn a_tag_key_and_value_round_trip_over_what_tagmap_admits() {
    let (router, _engine) = emulator();
    let dataset = create_training_dataset(&router, REGION, "tag-shapes", None).await;

    let longest_key = "k".repeat(MAX_KEY_LENGTH);
    let longest_value = "v".repeat(MAX_VALUE_LENGTH);
    // Rows differ along the shape of the key and of the value, which is what `TagKey` and
    // `TagValue` constrain.
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        (
            "the shortest key TagKey admits",
            tag_set(&[("a", "one-character key")]),
        ),
        (
            "the longest key TagKey admits",
            tag_set(&[(longest_key.as_str(), "longest key")]),
        ),
        (
            "the longest value TagValue admits",
            tag_set(&[("longest-value", longest_value.as_str())]),
        ),
        (
            "an empty value, which TagValue's minimum length admits",
            tag_set(&[("no-value", "")]),
        ),
        (
            "the punctuation AWS tag keys carry",
            tag_set(&[("cost:centre/42+a=b_-.@", "value with spaces")]),
        ),
        (
            "keys and values outside ASCII",
            tag_set(&[("proyecto-ñ", "équipe-数据")]),
        ),
        ("an empty tag map, which changes nothing", BTreeMap::new()),
        (
            "more tags than any other row carries",
            (0..40)
                .map(|index| (format!("bulk-{index:02}"), format!("value-{index}")))
                .collect(),
        ),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (case, tags) in rows {
        let tagged = tag_resource(&router, REGION, &dataset, tags_json(&tags)).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {} with {:?}",
            tagged.status, tagged.body
        );
        expected.extend(tags.clone());

        let listed = list_tags(&router, REGION, &dataset).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: the tag set must be what every call so far wrote, got {:?}",
            listed.body
        );

        // The keys this row wrote go away again, one by one, leaving the rest.
        let keys: Vec<String> = tags.keys().cloned().collect();
        if keys.is_empty() {
            continue;
        }
        let untagged = untag_resource(&router, REGION, &dataset, &keys).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: expected HTTP 200 from UntagResource, got {} with {:?}",
            untagged.status, untagged.body
        );
        for key in &keys {
            expected.remove(key);
        }
        let listed = list_tags(&router, REGION, &dataset).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: removing this row's keys must leave the earlier rows' tags, \
             got {:?}",
            listed.body
        );
    }
}

/// The next value of the deterministic generator the sequence case draws from: a 64-bit
/// LCG, so one seed replays the same sequence on every run and on every machine.
fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 11
}

/// The invariant over ANY sequence of writes: after each `TagResource` or `UntagResource`,
/// `ListTagsForResource` reports exactly the map those calls describe — a map merged on
/// tag and narrowed on untag. The sequence is drawn from one printed seed, so a failure
/// replays.
#[tokio::test]
async fn any_sequence_of_tag_and_untag_calls_leaves_the_map_those_calls_describe() {
    /// The keys the sequence draws from, so rows collide, overwrite and miss.
    const ALPHABET: [&str; 6] = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
    /// How many calls the sequence makes — bounded, so the case always terminates.
    const STEPS: usize = 64;
    /// The seed the sequence is drawn from. Named in every assertion, so a failing run
    /// says exactly which sequence to replay.
    const SEED: u64 = 0x1234_5678_9abc_def0;

    let (router, _engine) = emulator();
    let dataset = create_training_dataset(&router, REGION, "tag-sequence", None).await;

    let mut random = SEED;
    let mut model: BTreeMap<String, String> = BTreeMap::new();
    for step in 0..STEPS {
        let draw = next_random(&mut random);
        let count = 1 + (draw % 3) as usize;
        let keys: Vec<String> = (0..count)
            .map(|offset| {
                let index = (next_random(&mut random) as usize + offset) % ALPHABET.len();
                ALPHABET[index].to_string()
            })
            .collect();

        if draw.is_multiple_of(2) {
            let written: BTreeMap<String, String> = keys
                .iter()
                .map(|key| (key.clone(), format!("v{step}")))
                .collect();
            let answer = tag_resource(&router, REGION, &dataset, tags_json(&written)).await;
            assert_eq!(
                answer.status, 200,
                "seed {SEED:#x} step {step}: expected HTTP 200 from TagResource for \
                 {written:?}, got {} with {:?}",
                answer.status, answer.body
            );
            model.extend(written);
        } else {
            let answer = untag_resource(&router, REGION, &dataset, &keys).await;
            assert_eq!(
                answer.status, 200,
                "seed {SEED:#x} step {step}: expected HTTP 200 from UntagResource for \
                 {keys:?}, got {} with {:?}",
                answer.status, answer.body
            );
            for key in &keys {
                model.remove(key);
            }
        }

        let listed = list_tags(&router, REGION, &dataset).await;
        assert_eq!(
            reported_tags(&listed),
            model,
            "seed {SEED:#x} step {step}: the reported tags must be the ones the calls so \
             far describe, got {:?}",
            listed.body
        );
    }
}

/// An ARN this endpoint holds no resource for is refused by all three tagging operations
/// with the tagging path's own not-found: HTTP 404, the error code `NotFoundException` and
/// the fixed message that names no ARN — not the control plane's modelled
/// `ResourceNotFoundException`, which is what the resource's own read answers.
#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_by_every_tagging_operation() {
    let (router, _engine) = emulator();
    // One real, tagged resource exists, so a miss is a miss on a populated store.
    let present_tags = tag_set(&[("env", "test")]);
    let present =
        create_training_dataset(&router, REGION, "present-dataset", Some(&present_tags)).await;
    let present_id = present.rsplit('/').next().unwrap_or_default().to_string();

    // Rows differ along the resource kind the ARN names, the shape of its id, the account
    // and the region — every one satisfies `TaggableArn` and names nothing this endpoint
    // holds. The kinds are the ones that pattern admits, from
    // `smithy_query(query="shape", shape="TaggableArn")`.
    let cases: Vec<(&str, String)> = vec![
        (
            "a training dataset id this emulator never minted",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an audience model id this emulator never minted",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/never-minted"),
        ),
        (
            "a configured model algorithm id of the dots and slashes the pattern admits",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:configured-model-algorithm/team.ml/algorithms/trainer.v2"),
        ),
        (
            "a kind this emulator persists no resource of at all",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-generation-job/job-1"),
        ),
        (
            "a configured audience model, another kind nothing is persisted under",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:configured-audience-model/cam-1"),
        ),
        (
            "a membership-nested trained model nothing was created under",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{MEMBERSHIP}/trained-model/absent"),
        ),
        (
            "a membership-nested ML input channel nothing was created under",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{MEMBERSHIP}/ml-input-channel/absent"),
        ),
        (
            "the persisted resource's id under another account",
            format!("arn:aws:cleanrooms-ml:{REGION}:{OTHER_ACCOUNT}:training-dataset/{present_id}"),
        ),
        (
            "the persisted resource read through another region's endpoint",
            present.clone(),
        ),
    ];

    for (case, arn) in &cases {
        // The last row is the real ARN asked of a different endpoint; every other row is
        // asked of the endpoint that holds the store.
        let region = if arn == &present { OTHER_REGION } else { REGION };
        let keys = vec!["env".to_string()];
        let answers = [
            ("ListTagsForResource", list_tags(&router, region, arn).await),
            (
                "TagResource",
                tag_resource(&router, region, arn, json!({ "added": "1" })).await,
            ),
            (
                "UntagResource",
                untag_resource(&router, region, arn, &keys).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404 from {operation} for {arn:?}, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case:?}: expected the tagging path's NotFoundException from \
                 {operation}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.at("message"),
                json!(TAGGED_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the tagging path's fixed refusal message from \
                 {operation}, got {:?}",
                answer.at("message")
            );
        }
    }

    // The control plane's own read of a missing resource is the other refusal: the
    // modelled `ResourceNotFoundException`, which names the ARN it was asked about. The
    // two paths answer one absence with two codes, and neither borrows the other's.
    let unknown =
        format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/never-minted");
    let own_read = signed_call(
        &router,
        REGION,
        "GET",
        &format!("/training-dataset/{}", label(&unknown)),
        None,
    )
    .await;
    assert_eq!(
        own_read.error_code(),
        "ResourceNotFoundException",
        "the resource's own read must keep its modelled refusal, got {:?}",
        own_read.body
    );

    // None of the refused calls touched the one resource that does exist.
    let listed = list_tags(&router, REGION, &present).await;
    assert_eq!(
        reported_tags(&listed),
        present_tags,
        "the persisted resource's tags must be unchanged by the refused calls, got {:?}",
        listed.body
    );
}

/// Tags belong to one resource: writing or removing them on one leaves every other
/// resource's tags exactly as they were, including a resource of the same kind and name in
/// another region.
#[tokio::test]
async fn tagging_one_resource_leaves_every_other_resources_tags_alone() {
    let (router, engine) = emulator();
    let first_tags = tag_set(&[("env", "test"), ("team", "ml")]);
    let first = create_training_dataset(&router, REGION, "one-name", Some(&first_tags)).await;
    let second_tags = tag_set(&[("env", "prod")]);
    let second = create_training_dataset(&router, REGION, "other-name", Some(&second_tags)).await;
    // One name is free in every other region, so this is the same kind and name on another
    // endpoint.
    let elsewhere_tags = tag_set(&[("env", "dev")]);
    let elsewhere =
        create_training_dataset(&router, OTHER_REGION, "one-name", Some(&elsewhere_tags)).await;
    let seeded_tags = tag_set(&[("owner", "analytics")]);
    let seeded = seed_trained_model(&engine, REGION, "untouched", seeded_tags.clone());

    let tagged = tag_resource(
        &router,
        REGION,
        &first,
        json!({ "env": "rewritten", "extra": "x" }),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "the write under test must answer HTTP 200, got {} with {:?}",
        tagged.status, tagged.body
    );
    let untagged = untag_resource(&router, REGION, &first, &["team".to_string()]).await;
    assert_eq!(
        untagged.status, 200,
        "the removal under test must answer HTTP 200, got {} with {:?}",
        untagged.status, untagged.body
    );

    let cases = [
        ("the resource that was written", REGION, &first, tag_set(&[("env", "rewritten"), ("extra", "x")])),
        ("another resource in the same region", REGION, &second, second_tags.clone()),
        ("the same name in another region", OTHER_REGION, &elsewhere, elsewhere_tags.clone()),
        ("a resource of another kind", REGION, &seeded, seeded_tags.clone()),
    ];
    for (case, region, arn, expected) in cases {
        let listed = list_tags(&router, region, arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: expected the tags {expected:?}, got {:?}",
            listed.body
        );
    }
}
