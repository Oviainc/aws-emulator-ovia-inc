// SPDX-License-Identifier: CC0-1.0

//! `ListCollaborationConfiguredModelAlgorithmAssociations`, end to end over HTTP.
//!
//! The rule under test: the call reports exactly the persisted configured model algorithm
//! associations whose collaboration is the one the request names and whose region is the
//! endpoint the request reached, each projected from the stored record as a
//! `CollaborationConfiguredModelAlgorithmAssociationSummary`, in the one stable order the
//! store's ARN keys give, paged by `maxResults` with `nextToken` resuming — so a
//! collaboration holding none is reported as an empty list rather than refused.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. `CreateConfiguredModelAlgorithmAssociation` is still a scaffold stub with
//! its own issue, so the records each case reports on are written into the SAME store the
//! handler reads through `crate::state::service_state`: this crate's state module, reached
//! from the one shared engine the router was built over.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    configured_model_algorithm_arn, configured_model_algorithm_association_arn,
    ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState, ConfiguredModelAlgorithmAssociation,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{signed_call, timestamp_seconds, Answer};

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "us-east-1";

/// A second endpoint, for the cases that pin that a Clean Rooms ML endpoint reports its
/// own region's resources and no others.
const OTHER_REGION: &str = "ap-northeast-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// The largest page `ListCollaborationConfiguredModelAlgorithmAssociationsRequest$maxResults`
/// admits (`@range(min: 1, max: 100)`), spelled out rather than imported so a wrong
/// ceiling in the handler fails.
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
    format!("{index:08x}-4444-4555-8666-{index:012x}")
}

/// An instant that is the same on every run, so a seeded `createTime` is checked against
/// a value no clock can drift away from.
fn instant(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// One configured model algorithm association to persist: the required members, with the
/// optional one absent. A case sets only what it varies.
fn association(
    collaboration: &str,
    membership: &str,
    region: &str,
    id: &str,
) -> ConfiguredModelAlgorithmAssociation {
    ConfiguredModelAlgorithmAssociation {
        arn: configured_model_algorithm_association_arn(membership, id, region),
        region: region.to_string(),
        membership_identifier: membership.to_string(),
        collaboration_identifier: collaboration.to_string(),
        name: format!("association-{id}"),
        configured_model_algorithm_arn: configured_model_algorithm_arn(id, region),
        description: None,
        privacy_configuration: None,
        creator_account_id: ACCOUNT.to_string(),
        tags: Default::default(),
        create_time: instant(1_762_000_000),
        update_time: instant(1_762_000_060),
    }
}

/// Write `record` into the store the handler reads, and answer the ARN it is keyed by.
fn seed(engine: &engine::InMemoryEngine, record: ConfiguredModelAlgorithmAssociation) -> String {
    let arn = record.arn.clone();
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .configured_model_algorithm_associations
        .insert(arn.clone(), record);
    arn
}

/// `ListCollaborationConfiguredModelAlgorithmAssociations` on `collaboration`, through
/// `region`'s endpoint, with `query` appended verbatim.
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
        &format!("/collaborations/{collaboration}/configured-model-algorithm-associations{query}"),
        None,
    )
    .await
}

/// The summaries the call reported, in the order it reported them.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer.body["collaborationConfiguredModelAlgorithmAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListCollaborationConfiguredModelAlgorithmAssociations must return a \
                 collaborationConfiguredModelAlgorithmAssociations list: {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `configuredModelAlgorithmAssociationArn` of each reported summary, in the reported
/// order.
fn listed_arns(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary["configuredModelAlgorithmAssociationArn"]
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

/// Before any association is created, every collaboration holds none — and a collaboration
/// holding none is reported as an empty list, not refused: the operation models no
/// not-found error for a collaboration.
///
/// The rows differ along why the collaboration holds nothing, and each row is a distinct
/// input: an empty store, a store holding other collaborations' associations only, and a
/// store holding that collaboration's own associations in another region only.
#[tokio::test]
async fn a_collaboration_holding_no_associations_reports_an_empty_list() {
    let (router, engine) = emulator();

    // (case, the collaboration asked about)
    let cases: &[(&str, u32)] = &[
        ("empty store", 1),
        ("only other collaborations hold associations", 2),
        ("this collaboration's associations are in another region", 3),
    ];

    for (case, index) in cases.iter().copied() {
        let asked_about = uuid(index);
        match case {
            "only other collaborations hold associations" => {
                for other in [index + 100, index + 200] {
                    seed(
                        &engine,
                        association(&uuid(other), &uuid(other), REGION, &format!("a{other}")),
                    );
                }
            }
            "this collaboration's associations are in another region" => {
                seed(
                    &engine,
                    association(&asked_about, &uuid(index + 300), OTHER_REGION, "elsewhere"),
                );
            }
            _ => {}
        }

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
        assert_eq!(
            answer.at("nextToken"),
            Value::Null,
            "case {case:?}: a page that ends the list carries no nextToken, got {:?}",
            answer.body
        );
    }
}

/// An association is reported through its OWN collaboration and its own region's
/// endpoint, and through no other: the same store serves each view only what it holds.
///
/// The rows differ in how many associations a collaboration holds (none, one, several), in
/// how many memberships they are spread over, and in the region they were created in.
#[tokio::test]
async fn associations_are_reported_only_through_their_own_collaboration_and_region() {
    let (router, engine) = emulator();

    // (case, collaboration, region, how many memberships, associations per membership)
    let cases: &[(&str, u32, &str, u32, u32)] = &[
        ("one association, one membership", 10, REGION, 1, 1),
        ("three associations in one membership", 11, REGION, 1, 3),
        ("four associations across two memberships", 12, REGION, 2, 2),
        ("one association in the other region", 13, OTHER_REGION, 1, 1),
        ("two memberships in the other region", 14, OTHER_REGION, 2, 1),
        ("no associations at all", 15, REGION, 0, 0),
    ];

    let mut expected: Vec<(&str, String, &str, Vec<String>)> = Vec::new();
    for (case, index, region, memberships, per_membership) in cases.iter().copied() {
        let collaboration = uuid(index);
        let mut arns = Vec::new();
        for membership_index in 0..memberships {
            let membership = uuid(100 * index + membership_index);
            for association_index in 0..per_membership {
                arns.push(seed(
                    &engine,
                    association(
                        &collaboration,
                        &membership,
                        region,
                        &format!("assoc-{membership_index}-{association_index}"),
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
            "case {case:?}: expected exactly this collaboration's associations, in ARN order"
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
            "case {case:?}: {elsewhere}'s endpoint must report none of {region}'s associations, \
             got {:?}",
            across.body
        );
    }
}

/// Each reported summary is the `CollaborationConfiguredModelAlgorithmAssociationSummary`
/// projection of the stored record: every member comes back as it was stored, and the
/// optional member a record does not hold is not reported at all.
///
/// The rows differ along what the projection generalises over — whether the description is
/// set, the length of the name (`NameString` admits 1..63 characters), the resource id the
/// ARNs carry, the membership, and the create/update instants.
#[tokio::test]
async fn a_summary_projects_the_stored_record() {
    let (router, engine) = emulator();
    let collaboration = uuid(20);

    // (case, the association's resource id, the name, the description)
    let cases: Vec<(&str, &str, String, Option<&str>)> = vec![
        (
            "no description",
            "assoc-0",
            "first association".to_string(),
            None,
        ),
        (
            "a description",
            "assoc-1",
            "second association".to_string(),
            Some("the association under test"),
        ),
        // `NameString` is @length(min: 1, max: 63): both published bounds are stored and
        // reported unchanged.
        ("the shortest name", "assoc-2", "a".to_string(), Some("x")),
        ("the longest name", "assoc-3", "n".repeat(63), None),
        // `ConfiguredModelAlgorithmAssociationArn` admits `[-a-zA-Z0-9_/.]+` in its
        // resource id, so a nested id is as valid as a flat one.
        (
            "a nested resource id",
            "team/assoc.4_v1",
            "fourth".to_string(),
            None,
        ),
    ];

    let mut expected: Vec<(String, Value, &str)> = Vec::new();
    for (index, (case, id, name, description)) in cases.iter().enumerate() {
        let membership = uuid(21 + index as u32 % 2);
        let mut record = association(&collaboration, &membership, REGION, id);
        record.name = name.clone();
        record.description = description.map(std::string::ToString::to_string);
        record.create_time = instant(1_700_000_000 + index as u64);
        record.update_time = instant(1_700_000_500 + index as u64);

        let mut summary = json!({
            "configuredModelAlgorithmAssociationArn": &record.arn,
            "membershipIdentifier": &membership,
            "collaborationIdentifier": &collaboration,
            "name": &record.name,
            "configuredModelAlgorithmArn": &record.configured_model_algorithm_arn,
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
        expected.push((seed(&engine, record), summary, case));
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
        "expected one summary per stored association, got {:?}",
        answer.body
    );
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    for ((arn, want, case), got) in expected.iter().zip(reported.iter()) {
        assert_summary(&format!("{case} ({arn})"), want, got);
    }
}

/// `ListCollaborationConfiguredModelAlgorithmAssociations` is
/// `@paginated(items: collaborationConfiguredModelAlgorithmAssociations)`: whatever
/// `maxResults` it is given, following `nextToken` to the end enumerates every association
/// of the collaboration exactly once, in one stable order, and no page is longer than was
/// asked for.
#[tokio::test]
async fn paging_enumerates_every_association_exactly_once() {
    let (router, engine) = emulator();
    let collaboration = uuid(30);
    let mut expected = Vec::new();
    for index in 0..7 {
        expected.push(seed(
            &engine,
            association(
                &collaboration,
                &uuid(31 + index % 2),
                REGION,
                &format!("assoc-{index}"),
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
            "case {case:?}: following nextToken must enumerate every association exactly once, \
             in the unpaged order"
        );
    }
}

/// `maxResults` is modelled `@range(min: 1, max: 100)`: each published bound is served and
/// a value past either one is refused.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let (router, engine) = emulator();
    let collaboration = uuid(40);
    seed(
        &engine,
        association(&collaboration, &uuid(41), REGION, "assoc-0"),
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
            "case {accepted:?}: expected the one persisted association, got {:?}",
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
/// first page, and a token of the right shape past the last association reports the empty
/// tail.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let (router, engine) = emulator();
    let collaboration = uuid(50);
    let membership = uuid(51);
    seed(
        &engine,
        association(&collaboration, &membership, REGION, "assoc-0"),
    );

    // Rows differ along how the token is wrong: not an ARN at all, an ARN of another
    // resource type in this service, a membership identifier that is not a UUID, a
    // missing association segment, an empty resource id and a short account id.
    let cases: &[(&str, String)] = &[
        ("not an ARN at all", "page-2".to_string()),
        (
            "a configured-model-algorithm ARN",
            configured_model_algorithm_arn("algorithm-0", REGION),
        ),
        (
            "a trained-model ARN",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /trained-model/model-0"
            ),
        ),
        (
            "a membership that is not a UUID",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/not-a-uuid\
                 /configured-model-algorithm-association/assoc-0"
            ),
        ),
        (
            "no association segment",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}"),
        ),
        (
            "an empty association id",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /configured-model-algorithm-association/"
            ),
        ),
        (
            "a short account id",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:00000:membership/{membership}\
                 /configured-model-algorithm-association/a"
            ),
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

    // A token this operation DID issue the shape of — an association ARN past every
    // persisted one — is honoured, and reports the empty tail.
    let tail = configured_model_algorithm_association_arn(&uuid(52), "zzzz", REGION);
    let answer = list(&router, REGION, &collaboration, &format!("?nextToken={tail}")).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed token past the last association must be honoured, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        listed_arns(&answer).is_empty(),
        "a token past the last association reports no associations, got {:?}",
        answer.body
    );
}

/// The call only reports: reading a collaboration leaves the store exactly as it was, so
/// every later caller — paged or not — sees the same associations.
#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let collaboration = uuid(60);
    let mut expected = Vec::new();
    for index in 0..4 {
        expected.push(seed(
            &engine,
            association(
                &collaboration,
                &uuid(61 + index),
                REGION,
                &format!("assoc-{index}"),
            ),
        ));
    }
    expected.sort();

    // Rows differ in the query the repeated read carries, so neither an unpaged read nor
    // a paged one may consume what it reports.
    for (case, query) in [
        ("unpaged", String::new()),
        ("paged by one", "?maxResults=1".to_string()),
        ("paged by three", "?maxResults=3".to_string()),
    ] {
        for attempt in 0..3 {
            let answer = list(&router, REGION, &collaboration, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: attempt {attempt} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = listed_arns(&answer);
            let wanted: Vec<String> = match case {
                "paged by one" => expected[..1].to_vec(),
                "paged by three" => expected[..3].to_vec(),
                _ => expected.clone(),
            };
            assert_eq!(
                reported, wanted,
                "case {case:?}: attempt {attempt} must report the same associations as every \
                 other attempt"
            );
        }
    }

    // And the store still holds exactly what was seeded.
    let held: Vec<String> = engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .configured_model_algorithm_associations
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        held, expected,
        "reading must leave the store holding exactly the seeded associations"
    );
}
