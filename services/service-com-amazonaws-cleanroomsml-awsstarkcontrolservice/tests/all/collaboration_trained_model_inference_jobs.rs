// SPDX-License-Identifier: CC0-1.0

//! `ListCollaborationTrainedModelInferenceJobs`, end to end over HTTP.
//!
//! The rule under test: the call reports exactly the persisted trained model inference
//! jobs whose collaboration is the one the request names, whose region is the endpoint
//! the request reached, and which ran the trained model and the model version the
//! request filters on — each projected from the stored record as a
//! `CollaborationTrainedModelInferenceJobSummary`, in the one stable order the store's
//! ARN keys give, paged by `maxResults` with `nextToken` resuming. So a collaboration
//! holding none is reported as an empty list rather than refused.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. `StartTrainedModelInferenceJob` is still a scaffold stub with its own
//! issue, so the records each case reports on are written into the SAME store the
//! handler reads through `crate::state::service_state`: this crate's state module,
//! reached from the one shared engine the router was built over.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::ops::types::{
    InferenceOutputConfiguration, InferenceReceiverMember,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    trained_model_arn, trained_model_inference_job_arn,
    ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState, PendingTransition,
    TrainedModelInferenceJob,
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

/// Another account id, for the member that receives the inference output — a 12-digit
/// `AccountId`.
const RECEIVER_ACCOUNT: &str = "123456789012";

/// A third account id, for the member paying for the inference.
const PAYER_ACCOUNT: &str = "210987654321";

/// Every value `TrainedModelInferenceJobStatus` admits, from
/// `smithy_query(query="shape", shape="TrainedModelInferenceJobStatus")`. The cases below
/// report jobs carrying entries the issue never cited, so no handler can special-case one
/// spelling.
const INFERENCE_JOB_STATUSES: &[&str] = &[
    "CREATE_PENDING",
    "CREATE_IN_PROGRESS",
    "CREATE_FAILED",
    "ACTIVE",
    "CANCEL_PENDING",
    "CANCEL_IN_PROGRESS",
    "CANCEL_FAILED",
    "INACTIVE",
];

/// Every value `MetricsStatus` and `LogsStatus` admit, from
/// `smithy_query(query="shape", shape="MetricsStatus,LogsStatus")`.
const PUBLISH_STATUSES: &[&str] = &["PUBLISH_SUCCEEDED", "PUBLISH_FAILED"];

/// The largest page `ListCollaborationTrainedModelInferenceJobsRequest$maxResults` admits
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

/// The seconds since the Unix epoch of a seeded instant, as the wire reports a
/// `date-time` timestamp.
fn epoch_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the seeded instant is past the Unix epoch")
        .as_secs()
}

/// Where an inference job's output goes: `InferenceOutputConfiguration` requires exactly
/// one receiving member (`InferenceReceiverMembers` is `@length(min: 1, max: 1)`).
fn output_configuration(accept: &str, receiver: &str) -> InferenceOutputConfiguration {
    InferenceOutputConfiguration {
        accept: accept.to_string(),
        members: vec![InferenceReceiverMember {
            account_id: receiver.to_string(),
        }],
    }
}

/// One trained model inference job to persist: the required members, with the optional
/// ones absent. A case sets only what it varies.
fn inference_job(
    collaboration: &str,
    membership: &str,
    region: &str,
    id: &str,
) -> TrainedModelInferenceJob {
    TrainedModelInferenceJob {
        arn: trained_model_inference_job_arn(membership, id, region),
        region: region.to_string(),
        membership_identifier: membership.to_string(),
        collaboration_identifier: collaboration.to_string(),
        name: format!("job-{id}"),
        trained_model_arn: trained_model_arn(membership, "model-0", region),
        trained_model_version_identifier: None,
        configured_model_algorithm_association_arn: None,
        output_configuration: output_configuration("application/json", RECEIVER_ACCOUNT),
        description: None,
        creator_account_id: ACCOUNT.to_string(),
        ml_model_inference_payer_account_id: None,
        metrics_status: None,
        metrics_status_details: None,
        logs_status: None,
        logs_status_details: None,
        tags: Default::default(),
        status: "ACTIVE".to_string(),
        pending: None,
        create_time: instant(1_760_000_000),
        update_time: instant(1_760_000_060),
    }
}

/// Write `job` into the store the handler reads, and answer the ARN it is keyed by.
fn seed(engine: &engine::InMemoryEngine, job: TrainedModelInferenceJob) -> String {
    let arn = job.arn.clone();
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .trained_model_inference_jobs
        .insert(arn.clone(), job);
    arn
}

/// `ListCollaborationTrainedModelInferenceJobs` on `collaboration`, through `region`'s
/// endpoint, with `query` appended verbatim.
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
        &format!("/collaborations/{collaboration}/trained-model-inference-jobs{query}"),
        None,
    )
    .await
}

/// The summaries the call reported, in the order it reported them.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer.body["collaborationTrainedModelInferenceJobs"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListCollaborationTrainedModelInferenceJobs must return a \
                 collaborationTrainedModelInferenceJobs list: {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `trainedModelInferenceJobArn` of each reported summary, in the reported order.
fn listed_arns(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary["trainedModelInferenceJobArn"]
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

/// Before any inference job is started, every collaboration holds none — and a
/// collaboration holding none is reported as an empty list, not refused: the operation
/// models no not-found error for a collaboration.
///
/// The rows differ along why the collaboration holds nothing: an empty store, a store
/// holding other collaborations' jobs only, and a store holding that collaboration's own
/// jobs in another region only.
#[tokio::test]
async fn a_collaboration_holding_no_inference_jobs_reports_an_empty_list() {
    let (router, engine) = emulator();

    let cases: &[(&str, u32)] = &[
        ("empty store", 1),
        ("only other collaborations hold inference jobs", 2),
        ("this collaboration's inference jobs are in another region", 3),
    ];

    for (case, index) in cases.iter().copied() {
        let asked_about = uuid(index);
        match case {
            "only other collaborations hold inference jobs" => {
                for other in [index + 100, index + 200] {
                    seed(
                        &engine,
                        inference_job(&uuid(other), &uuid(other), REGION, &format!("j{other}")),
                    );
                }
            }
            "this collaboration's inference jobs are in another region" => {
                seed(
                    &engine,
                    inference_job(&asked_about, &uuid(index + 300), OTHER_REGION, "elsewhere"),
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

/// An inference job is reported through its OWN collaboration and its own region's
/// endpoint, and through no other: the same store serves each view only what it holds.
///
/// The rows differ in how many jobs a collaboration holds (none, one, several), in how
/// many memberships they are spread over, and in the region they were started in.
#[tokio::test]
async fn inference_jobs_are_reported_only_through_their_own_collaboration_and_region() {
    let (router, engine) = emulator();

    // (case, collaboration, region, how many memberships, jobs per membership)
    let cases: &[(&str, u32, &str, u32, u32)] = &[
        ("one job, one membership", 10, REGION, 1, 1),
        ("three jobs in one membership", 11, REGION, 1, 3),
        ("four jobs across two memberships", 12, REGION, 2, 2),
        ("one job in the other region", 13, OTHER_REGION, 1, 1),
        ("two memberships in the other region", 14, OTHER_REGION, 2, 1),
        ("no jobs at all", 15, REGION, 0, 0),
    ];

    let mut expected: Vec<(&str, String, &str, Vec<String>)> = Vec::new();
    for (case, index, region, memberships, per_membership) in cases.iter().copied() {
        let collaboration = uuid(index);
        let mut arns = Vec::new();
        for membership_index in 0..memberships {
            let membership = uuid(100 * index + membership_index);
            for job_index in 0..per_membership {
                arns.push(seed(
                    &engine,
                    inference_job(
                        &collaboration,
                        &membership,
                        region,
                        &format!("job-{membership_index}-{job_index}"),
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
            "case {case:?}: expected exactly this collaboration's inference jobs, in ARN order"
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
            "case {case:?}: {elsewhere}'s endpoint must report none of {region}'s inference \
             jobs, got {:?}",
            across.body
        );
    }
}

/// Each reported summary is the `CollaborationTrainedModelInferenceJobSummary` projection
/// of the stored record: every member comes back as it was stored, and an optional member
/// the record does not hold is not reported at all.
///
/// The rows differ along what the projection generalises over — the status, whether the
/// description, the model version, the association, the payer account and the metrics and
/// logs statuses are set, the output configuration's accept type and receiver, the name,
/// and the create/update instants.
#[tokio::test]
async fn a_summary_projects_the_stored_record() {
    let (router, engine) = emulator();
    let collaboration = uuid(20);
    let membership = uuid(21);

    let mut expected: Vec<(String, Value)> = Vec::new();
    for (index, status) in INFERENCE_JOB_STATUSES.iter().enumerate() {
        let id = format!("job-{index}");
        let mut record = inference_job(&collaboration, &membership, REGION, &id);
        record.status = (*status).to_string();
        record.create_time = instant(1_700_000_000 + index as u64);
        record.update_time = instant(1_700_000_500 + index as u64);
        record.output_configuration = output_configuration(
            // `InferenceOutputConfiguration$accept` is a 0..256 string; rows alternate so
            // a handler that reports one fixed content type fails here.
            if index % 2 == 0 {
                "application/json"
            } else {
                "text/csv"
            },
            if index % 3 == 0 {
                RECEIVER_ACCOUNT
            } else {
                PAYER_ACCOUNT
            },
        );
        // Every other row carries the optional members, so a handler that always
        // reports them, or never does, fails here.
        if index % 2 == 0 {
            record.description = Some(format!("the {status} job"));
            record.trained_model_version_identifier = Some(uuid(2_000 + index as u32));
            record.configured_model_algorithm_association_arn = Some(format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /configured-model-algorithm-association/assoc-{index}"
            ));
            record.ml_model_inference_payer_account_id = Some(PAYER_ACCOUNT.to_string());
            record.metrics_status = Some(PUBLISH_STATUSES[index % PUBLISH_STATUSES.len()].to_string());
            record.metrics_status_details = Some(format!("metrics of job {index}"));
            record.logs_status = Some(PUBLISH_STATUSES[(index + 1) % PUBLISH_STATUSES.len()].to_string());
            record.logs_status_details = Some(format!("logs of job {index}"));
        }
        if index == 1 {
            // `NameString` admits 1..63 characters; a one-character name is the shortest
            // the model allows.
            record.name = "j".to_string();
        }

        let mut summary = json!({
            "trainedModelInferenceJobArn": &record.arn,
            "membershipIdentifier": &membership,
            "collaborationIdentifier": &collaboration,
            "trainedModelArn": &record.trained_model_arn,
            "name": &record.name,
            "status": &record.status,
            "creatorAccountId": ACCOUNT,
            "outputConfiguration": {
                "accept": &record.output_configuration.accept,
                "members": [{
                    "accountId": &record.output_configuration.members[0].account_id,
                }],
            },
            "createTime": epoch_seconds(record.create_time),
            "updateTime": epoch_seconds(record.update_time),
        });
        for (member, value) in [
            ("description", record.description.as_deref()),
            (
                "trainedModelVersionIdentifier",
                record.trained_model_version_identifier.as_deref(),
            ),
            (
                "configuredModelAlgorithmAssociationArn",
                record.configured_model_algorithm_association_arn.as_deref(),
            ),
            (
                "mlModelInferencePayerAccountId",
                record.ml_model_inference_payer_account_id.as_deref(),
            ),
            ("metricsStatus", record.metrics_status.as_deref()),
            (
                "metricsStatusDetails",
                record.metrics_status_details.as_deref(),
            ),
            ("logsStatus", record.logs_status.as_deref()),
            ("logsStatusDetails", record.logs_status_details.as_deref()),
        ] {
            if let Some(value) = value {
                summary[member] = json!(value);
            }
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
        "expected one summary per stored inference job, got {:?}",
        answer.body
    );
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    for ((arn, want), got) in expected.iter().zip(reported.iter()) {
        assert_summary(arn, want, got);
    }
}

/// The request filters on the trained model, and on the version of that model: a job is
/// reported only when it ran what the request names, and a filter naming something no job
/// ran reports the empty list.
///
/// The rows differ along which filter members the request carries, and along what the
/// store holds for them: two models under two memberships, versioned and unversioned jobs.
#[tokio::test]
async fn the_filters_narrow_the_list_to_the_model_and_version_a_job_ran() {
    let (router, engine) = emulator();
    let collaboration = uuid(70);
    let membership_a = uuid(71);
    let membership_b = uuid(72);
    let model_a = trained_model_arn(&membership_a, "model-a", REGION);
    let model_b = trained_model_arn(&membership_b, "model-b", REGION);
    let version_one = uuid(73);
    let version_two = uuid(74);

    // (id, the membership, the model the job ran, the version it ran)
    let stored: &[(&str, &str, &str, Option<&str>)] = &[
        ("job-a1", &membership_a, &model_a, Some(&version_one)),
        ("job-a2", &membership_a, &model_a, Some(&version_two)),
        ("job-a3", &membership_a, &model_a, None),
        ("job-b1", &membership_b, &model_b, Some(&version_one)),
        ("job-b2", &membership_b, &model_b, None),
    ];
    let mut arns = Vec::new();
    for (id, membership, model, version) in stored.iter().copied() {
        let mut record = inference_job(&collaboration, membership, REGION, id);
        record.trained_model_arn = model.to_string();
        record.trained_model_version_identifier = version.map(str::to_string);
        arns.push((id, seed(&engine, record)));
    }
    let arn_of = |id: &str| -> String {
        arns.iter()
            .find(|(stored_id, _)| *stored_id == id)
            .map(|(_, arn)| arn.clone())
            .expect("every seeded job is in the table")
    };
    let sorted = |ids: &[&str]| -> Vec<String> {
        let mut expected: Vec<String> = ids.iter().copied().map(arn_of).collect();
        expected.sort();
        expected
    };

    // (case, the query the request carries, the jobs it must report)
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter at all",
            String::new(),
            sorted(&["job-a1", "job-a2", "job-a3", "job-b1", "job-b2"]),
        ),
        (
            "one model",
            format!("?trainedModelArn={model_a}"),
            sorted(&["job-a1", "job-a2", "job-a3"]),
        ),
        (
            "the other model",
            format!("?trainedModelArn={model_b}"),
            sorted(&["job-b1", "job-b2"]),
        ),
        (
            "one model and one of its versions",
            format!("?trainedModelArn={model_a}&trainedModelVersionIdentifier={version_two}"),
            sorted(&["job-a2"]),
        ),
        (
            "a version alone, across both models",
            format!("?trainedModelVersionIdentifier={version_one}"),
            sorted(&["job-a1", "job-b1"]),
        ),
        (
            "a model and a version no job of it ran",
            format!("?trainedModelArn={model_b}&trainedModelVersionIdentifier={version_two}"),
            Vec::new(),
        ),
        (
            "a model no job ran",
            format!(
                "?trainedModelArn={}",
                trained_model_arn(&membership_a, "model-never-run", REGION)
            ),
            Vec::new(),
        ),
        (
            "a version no job ran",
            format!("?trainedModelVersionIdentifier={}", uuid(75)),
            Vec::new(),
        ),
    ];

    for (case, query, want) in &cases {
        let answer = list(&router, REGION, &collaboration, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            &listed_arns(&answer),
            want,
            "case {case:?}: expected exactly the jobs the filter admits, in ARN order"
        );

        // The same filter, served one job per page, enumerates the same set.
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=want.len() + 1 {
            let mut paged = format!("{query}{}maxResults=1", if query.is_empty() { '?' } else { '&' });
            if let Some(token) = token.as_deref() {
                paged.push_str(&format!("&nextToken={token}"));
            }
            let answer = list(&router, REGION, &collaboration, &paged).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {page} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            walked.extend(listed_arns(&answer));
            token = answer
                .at("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            &walked, want,
            "case {case:?}: paging a filtered list must enumerate the same jobs"
        );
    }
}

/// `ListCollaborationTrainedModelInferenceJobs` is
/// `@paginated(items: collaborationTrainedModelInferenceJobs)`: whatever `maxResults` it
/// is given, following `nextToken` to the end enumerates every inference job of the
/// collaboration exactly once, in one stable order, and no page is longer than was asked
/// for.
#[tokio::test]
async fn paging_enumerates_every_inference_job_exactly_once() {
    let (router, engine) = emulator();
    let collaboration = uuid(30);
    let mut expected = Vec::new();
    for index in 0..7 {
        expected.push(seed(
            &engine,
            inference_job(
                &collaboration,
                &uuid(31 + index % 2),
                REGION,
                &format!("job-{index}"),
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
            "case {case:?}: following nextToken must enumerate every inference job exactly \
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
        inference_job(&collaboration, &uuid(41), REGION, "job-0"),
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
            "case {accepted:?}: expected the one persisted inference job, got {:?}",
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
/// first page, and a token of the right shape past the last inference job reports the
/// empty tail.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let (router, engine) = emulator();
    let collaboration = uuid(50);
    let membership = uuid(51);
    seed(
        &engine,
        inference_job(&collaboration, &membership, REGION, "job-0"),
    );

    // Rows differ along how the token is wrong: not an ARN at all, an ARN of another
    // resource type in this service, a membership identifier that is not a UUID, a
    // missing trained-model-inference-job segment, an empty resource id and a short
    // account id.
    let cases: &[(&str, String)] = &[
        ("not an ARN at all", "page-2".to_string()),
        (
            "a training-dataset ARN",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/abc"),
        ),
        (
            "a trained model ARN",
            trained_model_arn(&membership, "model-0", REGION),
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
                 /trained-model-inference-job/job-0"
            ),
        ),
        (
            "no trained-model-inference-job segment",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}"),
        ),
        (
            "an empty trained-model-inference-job id",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
                 /trained-model-inference-job/"
            ),
        ),
        (
            "a short account id",
            format!(
                "arn:aws:cleanrooms-ml:{REGION}:00000:membership/{membership}\
                 /trained-model-inference-job/j"
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

    // A token this operation DID issue the shape of — an inference job ARN past every
    // persisted one — is honoured, and reports the empty tail.
    let tail = trained_model_inference_job_arn(&uuid(52), "zzzz", REGION);
    let answer = list(&router, REGION, &collaboration, &format!("?nextToken={tail}")).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed token past the last inference job must be honoured, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        listed_arns(&answer).is_empty(),
        "a token past the last inference job reports no jobs, got {:?}",
        answer.body
    );
}

/// An inference job runs asynchronously, so the status a reader reports is the one the
/// clock owes it now: a transition already due is materialised, one not yet due is not,
/// and reading changes nothing a later caller sees.
///
/// Each attempt reads the collaboration twice over: unfiltered, and narrowed to the model
/// those jobs ran — a job the filter admits carries the same clock-owed status as the same
/// job in the unfiltered page, because the transition was owed whichever call arrived.
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
    let mut ran_model = String::new();
    for (index, (case, stored, due_in, moves_to, reported)) in cases.iter().copied().enumerate() {
        let mut record = inference_job(&collaboration, &membership, REGION, &format!("job-{index}"));
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
        ran_model = record.trained_model_arn.clone();
        expected.push((case, seed(&engine, record), reported));
    }

    let queries = [
        ("unfiltered", String::new()),
        (
            "filtered to the model these jobs ran",
            format!("?trainedModelArn={ran_model}"),
        ),
    ];
    for attempt in 0..2 {
        for (view, query) in &queries {
            let answer = list(&router, REGION, &collaboration, query).await;
            assert_eq!(
                answer.status, 200,
                "view {view:?}: attempt {attempt} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = summaries(&answer);
            for (case, arn, want) in &expected {
                let summary = reported
                    .iter()
                    .find(|summary| {
                        summary["trainedModelInferenceJobArn"].as_str() == Some(arn.as_str())
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: {arn} is missing from the {view} page {:?}",
                            answer.body
                        )
                    });
                assert_eq!(
                    summary["status"].as_str(),
                    Some(*want),
                    "case {case:?}: attempt {attempt} of the {view} page expected status \
                     {want:?}, got {summary:?}"
                );
            }
        }
    }
}
