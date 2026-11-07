// SPDX-License-Identifier: CC0-1.0

//! Enumerating a family's asynchronous analysis jobs: `List*DetectionJobs`.
//!
//! The rule under test is not about one family. A `List*DetectionJobs` reports the
//! persisted records of ITS OWN family and no other's, projected into that family's
//! `*JobProperties` shape, in submission order — reversed only when
//! `Filter.SubmitTimeAfter` is set, which the model documents as "descending order,
//! newest to oldest" — narrowed by the ONE filter member a request may set,
//! truncated to `MaxResults`, with an opaque `NextToken` that resumes exactly after
//! the last record on the page.
//!
//! So every suite below is a TABLE over the families whose `List*DetectionJobs` is
//! implemented × a set of submissions × the page sizes and filters the rule
//! generalises over. `sentiment_jobs.rs` covers the sentiment family's own
//! `List*`/`Stop*` pair in depth; this suite is the family-parametrised half, and
//! the only place the FAMILY-SCOPING half of the rule — one family's page never
//! carries another family's job — is asserted.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background — if a suite in this file ever spawns concurrent work,
// switch to a timer rather than blocking the runtime thread.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::JOB_STATUS_VALUES;

use crate::comprehend_client::{error_code, Comprehend, ACCOUNT_ID, REGION};

/// One analysis-job family, as the model names its operations and shapes.
struct Family {
    /// The operation that submits a job.
    start: &'static str,
    /// The operation that reports ONE submitted job.
    describe: &'static str,
    /// The output member `describe` reports the record under.
    properties: &'static str,
    /// The operation that enumerates this family's jobs.
    list: &'static str,
    /// The output member `list` reports its page under.
    properties_list: &'static str,
    /// The resource type this family's `JobArn` carries, from the `JobArn`
    /// documentation of `start` in the Smithy model (e.g.
    /// `arn::comprehend:::key-phrases-detection-job/`).
    arn_resource_type: &'static str,
    /// Does this family model `LanguageCode` on its request and its properties
    /// shape? The dominant-language family does not — the language is what that job
    /// detects — so a submission to it carries no `LanguageCode` and its records
    /// report none.
    models_language_code: bool,
}

/// Every family whose `List*DetectionJobs` is implemented. The rule holds for each.
const FAMILIES: &[Family] = &[
    Family {
        start: "StartKeyPhrasesDetectionJob",
        describe: "DescribeKeyPhrasesDetectionJob",
        properties: "KeyPhrasesDetectionJobProperties",
        list: "ListKeyPhrasesDetectionJobs",
        properties_list: "KeyPhrasesDetectionJobPropertiesList",
        arn_resource_type: "key-phrases-detection-job",
        models_language_code: true,
    },
    Family {
        start: "StartSentimentDetectionJob",
        describe: "DescribeSentimentDetectionJob",
        properties: "SentimentDetectionJobProperties",
        list: "ListSentimentDetectionJobs",
        properties_list: "SentimentDetectionJobPropertiesList",
        arn_resource_type: "sentiment-detection-job",
        models_language_code: true,
    },
    Family {
        start: "StartDominantLanguageDetectionJob",
        describe: "DescribeDominantLanguageDetectionJob",
        properties: "DominantLanguageDetectionJobProperties",
        list: "ListDominantLanguageDetectionJobs",
        properties_list: "DominantLanguageDetectionJobPropertiesList",
        arn_resource_type: "dominant-language-detection-job",
        models_language_code: false,
    },
];

/// The members a `Start*DetectionJob` persists and the family's `List*` reports
/// back. `OutputDataConfig` is among them because a job whose run was not performed
/// keeps the output location it was submitted with. `LanguageCode` is among them for
/// the families that model it, and absent from both the submission and the record for
/// the family that does not (see [`Family::models_language_code`]), so comparing the
/// member against the body that was sent covers both.
const PERSISTED_MEMBERS: &[&str] = &[
    "JobName",
    "LanguageCode",
    "InputDataConfig",
    "OutputDataConfig",
    "DataAccessRoleArn",
    "VolumeKmsKeyId",
    "VpcConfig",
];

/// The page size a request that sets no `MaxResults` gets, from the modelled
/// `MaxResults` documentation every analysis-job `List*` request shares: "The
/// maximum number of results to return in each page. The default is 100." Written
/// out here rather than imported, so the suite pins the documented default instead
/// of whatever the code currently believes.
const DEFAULT_PAGE_SIZE: usize = 100;

/// The modelled `MaxResults` bounds (`@range` 1..500 on `MaxResultsInteger`).
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 500;

/// Longest a suite waits for the clock to owe every job its terminal status. A job
/// walks two phase spans (`crate::state::LIFECYCLE_PHASE_SPAN` each) to a terminal
/// status; this leaves ample head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The terminal status a started job of these families reaches: this emulator does
/// not perform their batch run, so the run reports a failure (see
/// `key_phrases_jobs.rs`).
const TERMINAL_STATUS: &str = "FAILED";

/// A submission body that differs from the others along every member a
/// `Start*DetectionJob` persists: which optional members are present, the language,
/// the input format, the data locations and the tags.
fn submission(index: usize) -> Value {
    match index % 4 {
        0 => json!({
            "JobName": format!("listing-{index}-every-optional-member"),
            "LanguageCode": "en",
            "InputDataConfig": {
                "S3Uri": format!("s3://listing-{index}/in/"),
                "InputFormat": "ONE_DOC_PER_LINE",
            },
            "OutputDataConfig": {
                "S3Uri": format!("s3://listing-{index}/out/"),
                "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/1111-2222",
            },
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ListingRole",
            "VolumeKmsKeyId": "abcd1234-12ab-34cd-56ef-1234567890ab",
            "VpcConfig": {
                "SecurityGroupIds": ["sg-00112233445566778"],
                "Subnets": ["subnet-00112233445566778"],
            },
            "Tags": [{ "Key": "suite", "Value": "listing" }],
        }),
        1 => json!({
            "LanguageCode": "ko",
            "InputDataConfig": { "S3Uri": format!("s3://listing-{index}/corpus/") },
            "OutputDataConfig": { "S3Uri": format!("s3://listing-{index}/results/") },
            "DataAccessRoleArn": "arn:aws:iam::210987654321:role/nested/path/OtherRole",
        }),
        2 => json!({
            "JobName": format!("listing.{index}:2026/04=final+1%"),
            "LanguageCode": "pt",
            "InputDataConfig": {
                "S3Uri": format!("s3://listing-{index}/in"),
                "InputFormat": "ONE_DOC_PER_FILE",
                "DocumentReaderConfig": {
                    "DocumentReadAction": "TEXTRACT_DETECT_DOCUMENT_TEXT",
                },
            },
            "OutputDataConfig": { "S3Uri": format!("s3://listing-{index}/out") },
            "DataAccessRoleArn": "arn:aws:iam::000000000000:role/Third",
        }),
        _ => json!({
            // The modelled `JobName` ceiling is 256 characters.
            "JobName": format!("{index}-{}", "n".repeat(256 - 1 - index.to_string().len())),
            "LanguageCode": "zh-TW",
            "InputDataConfig": { "S3Uri": format!("s3://listing-{index}/deep/prefix/") },
            "OutputDataConfig": { "S3Uri": format!("s3://listing-{index}/deep/out/") },
            "DataAccessRoleArn": "arn:aws-cn:iam::111122223333:role/Fourth",
            "Tags": [],
        }),
    }
}

/// [`submission`], narrowed to the members `family` models: the dominant-language
/// family's request shape declares no `LanguageCode`, so a submission to it carries
/// none.
fn submission_for(family: &Family, index: usize) -> Value {
    let mut body = submission(index);
    if !family.models_language_code {
        if let Some(members) = body.as_object_mut() {
            members.remove("LanguageCode");
        }
    }
    body
}

/// How many jobs each family is seeded with: more than the largest page size the
/// paging suite asks for, so truncation and the cursor are both exercised.
const SEEDED_JOBS: usize = 5;

/// Submit `body` to `family`, returning `(JobId, JobArn)`.
async fn start(comprehend: &Comprehend, family: &Family, case: &str, body: &Value) -> (String, String) {
    let output = comprehend.ok(family.start, body).await;
    let job_id = output["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}/{case:?}: expected a JobId, got {output}", family.start))
        .to_string();
    let job_arn = output["JobArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}/{case:?}: expected a JobArn, got {output}", family.start))
        .to_string();
    (job_id, job_arn)
}

/// Seed every family with [`SEEDED_JOBS`] jobs, submitted family by family so the
/// families' submissions interleave in one shared sequence space. Returns, per
/// family index, the `JobId`s in submission order.
async fn seed(comprehend: &Comprehend) -> Vec<Vec<String>> {
    let mut per_family: Vec<Vec<String>> = FAMILIES.iter().map(|_| Vec::new()).collect();
    for index in 0..SEEDED_JOBS {
        for (family_index, family) in FAMILIES.iter().enumerate() {
            let body = submission_for(family, index);
            let (job_id, _) = start(comprehend, family, &format!("seed-{index}"), &body).await;
            per_family[family_index].push(job_id);
        }
    }
    per_family
}

/// One `List*DetectionJobs` page: the records and the `NextToken` it offered.
async fn list_page(comprehend: &Comprehend, family: &Family, request: &Value) -> (Vec<Value>, Option<String>) {
    let output = comprehend.ok(family.list, request).await;
    let page = output[family.properties_list]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {}/{request}: expected a {} array, got {output}",
                family.list, family.properties_list
            )
        });
    let next = output["NextToken"].as_str().map(str::to_string);
    (page, next)
}

/// Every record `family`'s `List*` serves for `filter`, walked through the cursor at
/// `page_size`, with each page's length checked against the page size it asked for.
async fn list_all(
    comprehend: &Comprehend,
    family: &Family,
    filter: Option<&Value>,
    page_size: Option<usize>,
) -> Vec<Value> {
    let case = format!(
        "{} with Filter={} MaxResults={page_size:?}",
        family.list,
        filter.cloned().unwrap_or(Value::Null)
    );
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(filter) = filter {
            request["Filter"] = filter.clone();
        }
        if let Some(page_size) = page_size {
            request["MaxResults"] = json!(page_size);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (page, next) = list_page(comprehend, family, &request).await;
        let ceiling = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        assert!(
            page.len() <= ceiling,
            "case {case:?}: a page carried {} records, more than the {ceiling} asked for",
            page.len()
        );
        collected.extend(page);
        pages += 1;
        assert!(
            pages <= SEEDED_JOBS * FAMILIES.len() + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match next {
            Some(token) => next_token = Some(token),
            None => break,
        }
    }
    collected
}

/// A `Filter` object out of the members it sets.
///
/// A helper rather than `json!`, because the member NAMES are what the suites table
/// over, and `json!` takes only literal keys.
fn filter_of(members: &[(&str, &Value)]) -> Value {
    Value::Object(
        members
            .iter()
            .map(|(member, value)| ((*member).to_string(), (*value).clone()))
            .collect(),
    )
}

/// The `JobId` of each record in a page.
fn ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|record| record["JobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_family_lists_the_jobs_it_persisted_in_submission_order() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend).await;

    for (family_index, family) in FAMILIES.iter().enumerate() {
        let page = list_all(&comprehend, family, None, Some(SEEDED_JOBS * 2)).await;
        assert_eq!(
            ids(&page),
            seeded[family_index],
            "case {}: expected this family's jobs in submission order, got {:?}",
            family.list,
            ids(&page)
        );

        // The FAMILY-SCOPING half of the rule: no other family's job is on the page.
        let foreign: Vec<&String> = seeded
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != family_index)
            .flat_map(|(_, job_ids)| job_ids.iter())
            .filter(|job_id| ids(&page).contains(job_id))
            .collect();
        assert!(
            foreign.is_empty(),
            "case {}: another family's jobs appeared on the page: {foreign:?}",
            family.list
        );
    }
}

#[tokio::test]
async fn a_listed_record_is_the_submitted_record_and_agrees_with_describe() {
    let comprehend = Comprehend::new();

    for family in FAMILIES {
        // One submission per shape of body, each read back through the family's own
        // List and then through its Describe.
        for index in 0..SEEDED_JOBS {
            let body = submission_for(family, index);
            let case = format!("{}/submission-{index}", family.list);
            let (job_id, job_arn) = start(&comprehend, family, &case, &body).await;

            let page = list_all(&comprehend, family, None, Some(MAX_MAX_RESULTS as usize)).await;
            let listed = page
                .iter()
                .find(|record| record["JobId"].as_str() == Some(job_id.as_str()))
                .unwrap_or_else(|| panic!("case {case:?}: {job_id} is missing from {page:?}"))
                .clone();

            assert_eq!(
                listed["JobArn"].as_str(),
                Some(job_arn.as_str()),
                "case {case:?}: List reported a different JobArn: {listed}"
            );
            assert_eq!(
                job_arn,
                format!(
                    "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:{}/{job_id}",
                    family.arn_resource_type
                ),
                "case {case:?}: unexpected JobArn"
            );
            for member in PERSISTED_MEMBERS {
                let submitted = body.get(member).cloned().unwrap_or(Value::Null);
                let reported = listed.get(member).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    reported, submitted,
                    "case {case:?}: List must report the {member} Start was called with, \
                     got {reported} for {submitted}"
                );
            }
            assert!(
                listed["SubmitTime"].as_f64().is_some_and(|t| t > 0.0),
                "case {case:?}: expected a populated SubmitTime, got {listed}"
            );
            assert!(
                JOB_STATUS_VALUES.contains(&listed["JobStatus"].as_str().unwrap_or_default()),
                "case {case:?}: unexpected JobStatus in {listed}"
            );
            // `Tags` is not a member of the properties shape, so it must not appear.
            assert!(
                listed.get("Tags").is_none(),
                "case {case:?}: {} models no Tags member, got {listed}",
                family.properties
            );
            // Nor is `LanguageCode` for the family whose properties shape declares
            // none: that job DETECTS the language rather than being told it.
            assert!(
                family.models_language_code || listed.get("LanguageCode").is_none(),
                "case {case:?}: {} models no LanguageCode member, got {listed}",
                family.properties
            );

            // List and Describe project the SAME record, so once the clock owes the
            // job its terminal status the two reports agree member for member.
            let started = Instant::now();
            loop {
                let described = comprehend
                    .ok(family.describe, &json!({ "JobId": job_id }))
                    .await[family.properties]
                    .clone();
                let page = list_all(&comprehend, family, None, Some(MAX_MAX_RESULTS as usize)).await;
                let listed = page
                    .iter()
                    .find(|record| record["JobId"].as_str() == Some(job_id.as_str()))
                    .cloned()
                    .unwrap_or(Value::Null);
                if described["JobStatus"].as_str() == Some(TERMINAL_STATUS)
                    && listed["JobStatus"].as_str() == Some(TERMINAL_STATUS)
                {
                    assert_eq!(
                        listed, described,
                        "case {case:?}: List and Describe must report the same record"
                    );
                    break;
                }
                assert!(
                    started.elapsed() < SETTLE_TIMEOUT,
                    "case {case:?}: the job never reached {TERMINAL_STATUS}: {described}"
                );
                sleep(Duration::from_millis(40));
            }
        }
    }
}

#[tokio::test]
async fn every_page_size_serves_the_same_records_exactly_once() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend).await;

    for (family_index, family) in FAMILIES.iter().enumerate() {
        // The modelled bounds, a size below the record count, one at it and one
        // above, and the default (no `MaxResults` at all).
        for page_size in [
            Some(MIN_MAX_RESULTS as usize),
            Some(2),
            Some(SEEDED_JOBS - 1),
            Some(SEEDED_JOBS),
            Some(SEEDED_JOBS + 1),
            Some(MAX_MAX_RESULTS as usize),
            None,
        ] {
            let page = list_all(&comprehend, family, None, page_size).await;
            assert_eq!(
                ids(&page),
                seeded[family_index],
                "case {}/MaxResults={page_size:?}: paging must serve every job exactly \
                 once, in submission order, got {:?}",
                family.list,
                ids(&page)
            );
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let comprehend = Comprehend::new();
    seed(&comprehend).await;

    // `MaxResultsInteger` is `@range` 1..500: each published bound is accepted and
    // one past it is refused before the handler runs.
    for family in FAMILIES {
        for (max_results, accepted) in [
            (MIN_MAX_RESULTS - 1, false),
            (MIN_MAX_RESULTS, true),
            (MAX_MAX_RESULTS, true),
            (MAX_MAX_RESULTS + 1, false),
        ] {
            let (status, body) = comprehend
                .call(family.list, &json!({ "MaxResults": max_results }))
                .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {}/MaxResults={max_results}: expected 200, got {status}: {body}",
                    family.list
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {}/MaxResults={max_results}: expected 400, got {status}: {body}",
                    family.list
                );
            }
        }
    }
}

#[tokio::test]
async fn each_filter_member_narrows_and_orders_as_its_documentation_states() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend).await;
    // 2100-01-01T00:00:00Z: later than any job this suite submits.
    let far_future = 4_102_444_800_f64;

    for (family_index, family) in FAMILIES.iter().enumerate() {
        let expected = &seeded[family_index];
        let newest_first: Vec<String> = expected.iter().rev().cloned().collect();

        // The two submit-time members state OPPOSITE orders: `SubmitTimeBefore`
        // "ascending order, oldest to newest", `SubmitTimeAfter` "descending order,
        // newest to oldest". A bound that excludes everything selects nothing, and
        // both bounds are read strictly.
        for (case, filter, want) in [
            (
                "SubmitTimeBefore in the far future",
                json!({ "SubmitTimeBefore": far_future }),
                expected.clone(),
            ),
            (
                "SubmitTimeAfter at the epoch",
                json!({ "SubmitTimeAfter": 0.0 }),
                newest_first.clone(),
            ),
            (
                "SubmitTimeAfter in the far future",
                json!({ "SubmitTimeAfter": far_future }),
                Vec::new(),
            ),
            (
                "SubmitTimeBefore at the epoch",
                json!({ "SubmitTimeBefore": 0.0 }),
                Vec::new(),
            ),
        ] {
            // Whole page, then one record at a time through the cursor: the order
            // and the membership must be the same either way.
            for page_size in [Some(1), Some(2), Some(MAX_MAX_RESULTS as usize)] {
                let page = list_all(&comprehend, family, Some(&filter), page_size).await;
                assert_eq!(
                    ids(&page),
                    want,
                    "case {}/{case:?} with MaxResults={page_size:?}: wrong records or \
                     order, got {:?}",
                    family.list,
                    ids(&page)
                );
            }
        }

        // `JobName` "Filters on the name of the job": it selects exactly the jobs
        // carrying that name, and a name no job carries selects nothing.
        for (index, job_id) in expected.iter().enumerate() {
            let Some(name) = submission_for(family, index)
                .get("JobName")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let page = list_all(
                &comprehend,
                family,
                Some(&json!({ "JobName": name })),
                Some(MAX_MAX_RESULTS as usize),
            )
            .await;
            assert_eq!(
                ids(&page),
                vec![job_id.clone()],
                "case {}/Filter.JobName of submission {index}: expected exactly that \
                 job, got {:?}",
                family.list,
                ids(&page)
            );
        }
        for absent in ["listing", "no-such-job", ""] {
            let (status, body) = comprehend
                .call(family.list, &json!({ "Filter": { "JobName": absent } }))
                .await;
            if absent.is_empty() {
                // `JobName` is `@length` 1..256, so the empty string never reaches
                // the handler.
                assert_eq!(
                    status, 400,
                    "case {}/Filter.JobName={absent:?}: expected 400, got {status}: {body}",
                    family.list
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {}/Filter.JobName={absent:?}: expected 200, got {status}: {body}",
                family.list
            );
            let page = body[family.properties_list].as_array().cloned().unwrap_or_default();
            assert!(
                page.is_empty() && body["NextToken"].is_null(),
                "case {}/Filter.JobName={absent:?}: expected an empty last page, got {body}",
                family.list
            );
        }

        // `JobStatus` "Returns only jobs with the specified status": every modelled
        // status either selects jobs that carry it or selects none. The catalog is
        // the model's own `JobStatus` enum.
        for status in JOB_STATUS_VALUES {
            let page = list_all(
                &comprehend,
                family,
                Some(&json!({ "JobStatus": status })),
                Some(MAX_MAX_RESULTS as usize),
            )
            .await;
            for record in &page {
                assert_eq!(
                    record["JobStatus"].as_str(),
                    Some(*status),
                    "case {}/Filter.JobStatus={status:?}: returned a job with another \
                     status: {record}",
                    family.list
                );
            }
        }

        // And the positive case, made fail-able: once the clock owes every job its
        // terminal status, that status selects all of them and no other any.
        let started = Instant::now();
        loop {
            let page = list_all(
                &comprehend,
                family,
                Some(&json!({ "JobStatus": TERMINAL_STATUS })),
                Some(MAX_MAX_RESULTS as usize),
            )
            .await;
            if ids(&page) == *expected {
                break;
            }
            assert!(
                started.elapsed() < SETTLE_TIMEOUT,
                "case {}/Filter.JobStatus={TERMINAL_STATUS:?}: expected {:?} once the \
                 clock owed every job its terminal status, got {:?}",
                family.list,
                expected,
                ids(&page)
            );
            sleep(Duration::from_millis(40));
        }
        for status in JOB_STATUS_VALUES
            .iter()
            .filter(|status| **status != TERMINAL_STATUS)
        {
            let page = list_all(
                &comprehend,
                family,
                Some(&json!({ "JobStatus": status })),
                Some(MAX_MAX_RESULTS as usize),
            )
            .await;
            assert!(
                page.is_empty(),
                "case {}/Filter.JobStatus={status:?}: every job is {TERMINAL_STATUS}, so \
                 this must select none, got {:?}",
                family.list,
                ids(&page)
            );
        }

        // Reading never changes what a later caller sees: two lists of a settled
        // store are identical.
        let once = list_all(&comprehend, family, None, Some(MAX_MAX_RESULTS as usize)).await;
        let twice = list_all(&comprehend, family, None, Some(MAX_MAX_RESULTS as usize)).await;
        assert_eq!(
            once, twice,
            "case {}: listing a settled store twice must report the same records",
            family.list
        );
    }
}

#[tokio::test]
async fn more_than_one_filter_member_is_refused() {
    let comprehend = Comprehend::new();
    seed(&comprehend).await;

    // "You can only set one filter at a time": every pair of members, and the full
    // set, is refused; each member on its own is not.
    let members: [(&str, Value); 4] = [
        ("JobName", json!("listing-0-every-optional-member")),
        ("JobStatus", json!("SUBMITTED")),
        ("SubmitTimeBefore", json!(4_102_444_800_f64)),
        ("SubmitTimeAfter", json!(0.0)),
    ];

    for family in FAMILIES {
        for (index, (member, value)) in members.iter().enumerate() {
            let (status, body) = comprehend
                .call(
                    family.list,
                    &json!({ "Filter": filter_of(&[(member, value)]) }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {}/one filter member {member}: expected 200, got {status}: {body}",
                family.list
            );

            for (other, other_value) in members.iter().skip(index + 1) {
                let (status, body) = comprehend
                    .call(
                        family.list,
                        &json!({
                            "Filter": filter_of(&[(member, value), (other, other_value)]),
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {}/{member}+{other}: expected 400, got {status}: {body}",
                    family.list
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidFilterException",
                    "case {}/{member}+{other}: expected InvalidFilterException, got {body}",
                    family.list
                );
            }
        }

        let every: Vec<(&str, &Value)> = members
            .iter()
            .map(|(member, value)| (*member, value))
            .collect();
        let (status, body) = comprehend
            .call(family.list, &json!({ "Filter": filter_of(&every) }))
            .await;
        assert_eq!(
            status, 400,
            "case {}/every filter member: expected 400, got {status}: {body}",
            family.list
        );
        assert_eq!(
            error_code(&body),
            "InvalidFilterException",
            "case {}/every filter member: expected InvalidFilterException, got {body}",
            family.list
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend).await;

    for family in FAMILIES {
        for token in [
            "not-a-token",
            "00",
            "deadbeef",
            "0011223344556677889900",
            // Well-formed, but names no page this service ever issued: a cursor is a
            // submission sequence number, which starts at 1 and never runs ahead of
            // the newest submission.
            "0000000000000000",
            "00000000000003e8",
        ] {
            let (status, body) = comprehend
                .call(family.list, &json!({ "NextToken": token }))
                .await;
            assert_eq!(
                status, 400,
                "case {}/NextToken={token:?}: expected 400, got {status}: {body}",
                family.list
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {}/NextToken={token:?}: expected InvalidRequestException, got {body}",
                family.list
            );
        }
    }

    // A token this service DID issue resumes exactly after the last record of the
    // page that offered it, at every page size.
    for (family_index, family) in FAMILIES.iter().enumerate() {
        for page_size in [1usize, 2, SEEDED_JOBS - 1] {
            let (first, next) = list_page(
                &comprehend,
                family,
                &json!({ "MaxResults": page_size }),
            )
            .await;
            let token = next.unwrap_or_else(|| {
                panic!(
                    "case {}/MaxResults={page_size}: expected a NextToken after {} of {} \
                     records",
                    family.list,
                    first.len(),
                    SEEDED_JOBS
                )
            });
            let (second, _) = list_page(
                &comprehend,
                family,
                &json!({ "MaxResults": page_size, "NextToken": token }),
            )
            .await;
            let resumed = ids(&second);
            let expected: Vec<String> = seeded[family_index]
                .iter()
                .skip(first.len())
                .take(page_size)
                .cloned()
                .collect();
            assert_eq!(
                resumed, expected,
                "case {}/MaxResults={page_size}: the second page must resume after the \
                 first, got {resumed:?}",
                family.list
            );
        }
    }
}
