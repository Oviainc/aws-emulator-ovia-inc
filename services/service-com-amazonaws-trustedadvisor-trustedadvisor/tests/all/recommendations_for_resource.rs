// SPDX-License-Identifier: CC0-1.0

//! `ListRecommendationsForResource`, over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The rule under test is not about one call: **the operation reports one
//! `RecommendationForResourceSummary` per stored recommendation resource whose
//! AWS resource is the ARN asked about — the recommendation and its check
//! supplying `recommendationArn`, `checkArn` and `pillars`, the flagged
//! resource supplying `awsResourceArn`, `status`, `exclusionStatus`,
//! `metadata` and `lastUpdatedAt` — kept in recommendation-resource ARN order,
//! cut by the request's `status` / `pillar` / `checkArn` filters, and paged by a
//! cursor bound to that resource and those filters; an ARN no stored
//! recommendation flagged reports an EMPTY list.**
//!
//! So every test is a TABLE whose rows span the class: each seeded AWS resource
//! (three resource types with three different ARN forms, four Regions, all
//! three `ResourceStatus` values and both `ExclusionStatus` values), ARNs that
//! match nothing (a resource no check flagged, the right resource id in the
//! wrong Region, the right id under the wrong resource type, and the finding of
//! a check that is not `resourceArnQueryable`), and a resource flagged by TWO
//! checks, which is the case that makes the answer a list.
//!
//! The expected catalog below is written out by hand — its source is the `SEED`
//! in `src/state.rs` — so a response is compared against stated values rather
//! than against the store it came from.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of this operation (and the refusal of a resource
//! in another account) is pinned in `tests/all/support_level_gate.rs`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    aws_resource_arn, check_arn, service_state, Check, LocalizedText, RecommendationEntry,
    ResourceEntry, SupportPlan, TextCatalog, EXCLUSION_INCLUDED, RESOURCE_STATUS_ERROR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    call, dispatch, emulator_on_plan, error_code, field_of_each, label, recommendation_arn,
    timestamp_secs,
};

/// One seeded AWS resource as this suite states it: the check that flagged it,
/// that check's pillars, the resource's `awsResourceId`, its Region, the ARN
/// form of its resource type, its `ResourceStatus`, its `ExclusionStatus` and
/// its metadata values.
///
/// Source: the `SEED` table in `src/state.rs` (and the per-resource-type ARN
/// forms in `state::aws_resource_arn`).
struct Expected {
    check_id: &'static str,
    pillars: &'static [&'static str],
    aws_resource_id: &'static str,
    region: &'static str,
    /// The ARN's resource part, with `{id}` standing for `aws_resource_id`.
    arn_service: &'static str,
    arn_resource: &'static str,
    status: &'static str,
    exclusion_status: &'static str,
    metadata: &'static [&'static str],
}

impl Expected {
    /// The ARN of the AWS resource this row describes.
    fn arn(&self) -> String {
        format!(
            "arn:aws:{}:{}:{}:{}",
            self.arn_service,
            self.region,
            engine::EMULATOR_ACCOUNT_ID,
            self.arn_resource
        )
    }

    /// The metadata map a `RecommendationForResourceSummary` carries for it:
    /// the resource's values, keyed by column index as a string.
    fn metadata(&self) -> Value {
        Value::Object(
            self.metadata
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), Value::String((*value).to_string())))
                .collect(),
        )
    }
}

/// Every seeded resource that an AWS resource ARN can address: the three
/// resources of the EC2 check, the two of the EBS check and the two of the RDS
/// check. The `iam-access-key-rotation` check is deliberately absent — it is
/// not `resourceArnQueryable`, which the empty-answer table below pins.
const SEEDED: &[Expected] = &[
    Expected {
        check_id: "low-utilization-ec2-instances",
        pillars: &["cost_optimizing"],
        aws_resource_id: "i-0abc1de23f4567890",
        region: "us-east-1",
        arn_service: "ec2",
        arn_resource: "instance/i-0abc1de23f4567890",
        status: "warning",
        exclusion_status: "included",
        metadata: &["us-east-1", "i-0abc1de23f4567890", "m5.xlarge", "72.00"],
    },
    Expected {
        check_id: "low-utilization-ec2-instances",
        pillars: &["cost_optimizing"],
        aws_resource_id: "i-0abc2de23f4567891",
        region: "us-west-2",
        arn_service: "ec2",
        arn_resource: "instance/i-0abc2de23f4567891",
        status: "ok",
        exclusion_status: "included",
        metadata: &["us-west-2", "i-0abc2de23f4567891", "t3.medium", "0.00"],
    },
    Expected {
        check_id: "low-utilization-ec2-instances",
        pillars: &["cost_optimizing"],
        aws_resource_id: "i-0abc3de23f4567892",
        region: "eu-west-1",
        arn_service: "ec2",
        arn_resource: "instance/i-0abc3de23f4567892",
        status: "warning",
        exclusion_status: "included",
        metadata: &["eu-west-1", "i-0abc3de23f4567892", "c5.2xlarge", "148.00"],
    },
    Expected {
        check_id: "amazon-ebs-snapshots",
        pillars: &["fault_tolerance"],
        aws_resource_id: "vol-0ea1b2c3d4e5f6789",
        region: "us-east-1",
        arn_service: "ec2",
        arn_resource: "volume/vol-0ea1b2c3d4e5f6789",
        status: "error",
        exclusion_status: "included",
        metadata: &[
            "us-east-1",
            "vol-0ea1b2c3d4e5f6789",
            "snap-0ea1b2c3d4e5f6789",
            "412",
        ],
    },
    Expected {
        check_id: "amazon-ebs-snapshots",
        pillars: &["fault_tolerance"],
        aws_resource_id: "vol-0ea2b2c3d4e5f6789",
        region: "ap-southeast-1",
        arn_service: "ec2",
        arn_resource: "volume/vol-0ea2b2c3d4e5f6789",
        status: "ok",
        exclusion_status: "excluded",
        metadata: &[
            "ap-southeast-1",
            "vol-0ea2b2c3d4e5f6789",
            "snap-0ea2b2c3d4e5f6789",
            "11",
        ],
    },
    Expected {
        check_id: "rds-idle-db-instances",
        pillars: &["cost_optimizing", "performance"],
        aws_resource_id: "reporting-prod",
        region: "us-east-1",
        arn_service: "rds",
        // An RDS DB instance ARN separates its name with `:`, not `/`.
        arn_resource: "db:reporting-prod",
        status: "warning",
        exclusion_status: "included",
        metadata: &["us-east-1", "reporting-prod", "db.m5.large", "21", "131.40"],
    },
    Expected {
        check_id: "rds-idle-db-instances",
        pillars: &["cost_optimizing", "performance"],
        aws_resource_id: "analytics-staging",
        region: "us-west-2",
        arn_service: "rds",
        arn_resource: "db:analytics-staging",
        status: "ok",
        exclusion_status: "included",
        metadata: &[
            "us-west-2",
            "analytics-staging",
            "db.t3.medium",
            "1",
            "0.00",
        ],
    },
];

/// The check this suite ADDS to the store, so one AWS resource is flagged by
/// two checks: a second `AWS::EC2::Instance` check whose one resource is the
/// same instance `low-utilization-ec2-instances` already flags. Nothing in the
/// seed overlaps, and the answer being a LIST is the operation's whole point.
const SECOND_CHECK_ID: &str = "ec2-imdsv2";
const SECOND_RECOMMENDATION_ID: &str = "ec2-imdsv2-recommendation";
const SECOND_RESOURCE_ID: &str = "res-imdsv2-0001";
const SECOND_CHECK_PILLAR: &str = "security";

/// The instance both checks flag.
fn shared_instance() -> &'static Expected {
    &SEEDED[0]
}

/// `arn:aws:trustedadvisor::<account>:recommendation/ec2-imdsv2-recommendation`.
fn second_recommendation_arn() -> String {
    format!(
        "arn:aws:trustedadvisor::{}:recommendation/{SECOND_RECOMMENDATION_ID}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A mounted emulator on an entitled plan.
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// A mounted emulator whose store also holds [`SECOND_CHECK_ID`], a second
/// `resourceArnQueryable` check on `AWS::EC2::Instance` that flagged the same
/// instance as the seeded EC2 check.
///
/// Written straight into the store the way the seed is: Trusted Advisor models
/// no operation that creates a check or a recommendation, so a second finding
/// can only arrive from outside the API.
fn emulator_with_two_checks_on_one_instance() -> MultiServiceRouter {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    let state = service_state(&*engine).expect("the engine carries this service's state");
    let mut state = state.lock();

    let text = TextCatalog::new(
        LocalizedText {
            name: "Amazon EC2 instances with IMDSv1 enabled".to_string(),
            description: "Checks whether your Amazon EC2 instances require IMDSv2.".to_string(),
        },
        std::iter::empty(),
    );
    let check_arn = check_arn(SECOND_CHECK_ID);
    let recommendation_arn = second_recommendation_arn();
    let resource_arn = format!(
        "arn:aws:trustedadvisor::{}:recommendation-resource/{SECOND_RECOMMENDATION_ID}/\
         {SECOND_RESOURCE_ID}",
        engine::EMULATOR_ACCOUNT_ID
    );
    // The same instant for both of the new entry's timestamps, so the row's
    // `lastUpdatedAt` is a stated value.
    let last_updated_at_millis = 1_735_689_600_000;

    state.checks.insert(
        SECOND_CHECK_ID.to_string(),
        Check {
            id: SECOND_CHECK_ID.to_string(),
            arn: check_arn.clone(),
            text: text.clone(),
            pillars: vec![SECOND_CHECK_PILLAR.to_string()],
            aws_services: vec!["ec2".to_string()],
            source: "ta_check".to_string(),
            metadata: BTreeMap::from([
                ("0".to_string(), "Region".to_string()),
                ("1".to_string(), "Instance ID".to_string()),
            ]),
            resource_arn_queryable: true,
            aws_resource_types: vec!["AWS::EC2::Instance".to_string()],
            check_granularity: Some("resource".to_string()),
            recommendation_id: Some(SECOND_RECOMMENDATION_ID.to_string()),
        },
    );
    state.recommendations.insert(
        recommendation_arn.clone(),
        RecommendationEntry {
            id: SECOND_RECOMMENDATION_ID.to_string(),
            arn: recommendation_arn.clone(),
            text,
            check_arn: Some(check_arn),
            r#type: "standard".to_string(),
            status: RESOURCE_STATUS_ERROR.to_string(),
            lifecycle_stage: None,
            pillars: vec![SECOND_CHECK_PILLAR.to_string()],
            source: "ta_check".to_string(),
            aws_services: vec!["ec2".to_string()],
            created_at_millis: last_updated_at_millis,
            last_updated_at_millis,
            resolved_at_millis: None,
            update_reason: None,
            update_reason_code: None,
        },
    );
    state.recommendation_resources.insert(
        resource_arn.clone(),
        ResourceEntry {
            id: SECOND_RESOURCE_ID.to_string(),
            arn: resource_arn,
            recommendation_arn,
            aws_resource_id: shared_instance().aws_resource_id.to_string(),
            region_code: shared_instance().region.to_string(),
            status: RESOURCE_STATUS_ERROR.to_string(),
            metadata: BTreeMap::from([
                ("0".to_string(), shared_instance().region.to_string()),
                (
                    "1".to_string(),
                    shared_instance().aws_resource_id.to_string(),
                ),
            ]),
            last_updated_at_millis,
            exclusion_status: EXCLUSION_INCLUDED.to_string(),
            estimated_monthly_spend: None,
        },
    );
    drop(state);
    router
}

/// `GET /v1/recommendations-for-resource/{awsResourceArn}{query}`.
async fn list_for(router: &MultiServiceRouter, arn: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v1/recommendations-for-resource/{}{query}", label(arn)),
        None,
    )
    .await
}

#[tokio::test]
async fn every_flagged_resource_reports_the_recommendation_that_flagged_it() {
    let router = emulator();

    for expected in SEEDED {
        let arn = expected.arn();
        let case = arn.clone();
        let (status, body) = list_for(&router, &arn, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let rows = body["recommendationForResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no recommendationForResourceSummaries: {body}"));
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected exactly one summary, got {rows:?}"
        );
        let row = &rows[0];
        assert_eq!(
            row["awsResourceArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected awsResourceArn {arn:?}, got {:?}",
            row["awsResourceArn"]
        );
        assert_eq!(
            row["checkArn"].as_str(),
            Some(check_arn(expected.check_id).as_str()),
            "case {case:?}: expected the check of {:?}, got {:?}",
            expected.check_id,
            row["checkArn"]
        );
        assert_eq!(
            row["recommendationArn"].as_str(),
            Some(recommendation_arn(expected.check_id).as_str()),
            "case {case:?}: expected the recommendation of {:?}, got {:?}",
            expected.check_id,
            row["recommendationArn"]
        );
        assert_eq!(
            row["status"].as_str(),
            Some(expected.status),
            "case {case:?}: expected status {:?}, got {:?}",
            expected.status,
            row["status"]
        );
        assert_eq!(
            row["exclusionStatus"].as_str(),
            Some(expected.exclusion_status),
            "case {case:?}: expected exclusionStatus {:?}, got {:?}",
            expected.exclusion_status,
            row["exclusionStatus"]
        );
        assert_eq!(
            row["pillars"],
            json!(expected.pillars),
            "case {case:?}: expected pillars {:?}, got {:?}",
            expected.pillars,
            row["pillars"]
        );
        assert_eq!(
            row["metadata"],
            expected.metadata(),
            "case {case:?}: expected metadata {}, got {}",
            expected.metadata(),
            row["metadata"]
        );
        assert!(
            row["lastUpdatedAt"] != Value::Null,
            "case {case:?}: expected a lastUpdatedAt, got {row}"
        );
    }
}

/// The row reports the SAME resource `ListRecommendationResources` reports
/// under the owning recommendation: one stored resource, two views of it.
#[tokio::test]
async fn a_row_agrees_with_the_recommendation_s_own_resource_listing() {
    let router = emulator();

    for expected in SEEDED {
        let arn = expected.arn();
        let case = arn.clone();
        let (_, by_resource) = list_for(&router, &arn, "").await;
        let row = &by_resource["recommendationForResourceSummaries"][0];

        let (status, by_recommendation) = call(
            &router,
            "GET",
            &format!(
                "/v1/recommendations/{}/resources",
                label(&recommendation_arn(expected.check_id))
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListRecommendationResources, got {status} with \
             {by_recommendation}"
        );
        let summary = by_recommendation["recommendationResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no summaries: {by_recommendation}"))
            .iter()
            .find(|summary| summary["awsResourceId"].as_str() == Some(expected.aws_resource_id))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {} is not listed: {by_recommendation}", expected.aws_resource_id)
            });

        for member in ["status", "exclusionStatus", "metadata"] {
            assert_eq!(
                row[member], summary[member],
                "case {case:?}: the two views disagree on {member}: {} and {}",
                row[member], summary[member]
            );
        }
        assert_eq!(
            timestamp_secs(&row["lastUpdatedAt"]),
            timestamp_secs(&summary["lastUpdatedAt"]),
            "case {case:?}: the two views disagree on lastUpdatedAt: {} and {}",
            row["lastUpdatedAt"],
            summary["lastUpdatedAt"]
        );
    }
}

/// An ARN no stored recommendation flagged reports an EMPTY list and HTTP 200 —
/// never a 404, which this operation models no error for. The rows differ along
/// WHY nothing matches.
#[tokio::test]
async fn an_arn_no_recommendation_flagged_reports_an_empty_list() {
    let router = emulator();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let cases = [
        (
            "an EC2 instance no check flagged",
            format!("arn:aws:ec2:us-east-1:{account}:instance/i-0fffffffffffffff0"),
        ),
        (
            "a flagged instance id in another Region",
            format!("arn:aws:ec2:eu-central-1:{account}:instance/i-0abc1de23f4567890"),
        ),
        (
            "a flagged volume id under the instance resource type",
            format!("arn:aws:ec2:us-east-1:{account}:instance/vol-0ea1b2c3d4e5f6789"),
        ),
        (
            "a flagged DB instance name with the `/` separator ARN form",
            format!("arn:aws:rds:us-east-1:{account}:db/reporting-prod"),
        ),
        (
            // `iam-access-key-rotation` is seeded `resourceArnQueryable: false`:
            // "Indicates whether this check is supported by the
            // ListRecommendationsForResource API".
            "the finding of a check that is not resourceArnQueryable",
            format!("arn:aws:iam::{account}:user/AKIAIOSFODNN7EXAMPLE"),
        ),
        (
            "a resource type carrying no account segment at all",
            "arn:aws:s3:::trusted-advisor-probe-bucket".to_string(),
        ),
    ];

    for (case, arn) in cases {
        let (status, body) = list_for(&router, &arn, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["recommendationForResourceSummaries"],
            json!([]),
            "case {case:?}: expected no summaries, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: expected no nextToken, got {body}"
        );
    }
}

/// One resource flagged by two checks reports one row per check, in
/// recommendation-resource ARN order.
#[tokio::test]
async fn a_resource_flagged_by_two_checks_reports_both() {
    let router = emulator_with_two_checks_on_one_instance();
    let arn = shared_instance().arn();

    let (status, body) = list_for(&router, &arn, "").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    let got = field_of_each(&body, "recommendationForResourceSummaries", "recommendationArn");
    // `ec2-imdsv2-recommendation` sorts before `low-utilization-…` in the
    // recommendation-resource ARN order the store keys resources by.
    let expected = vec![
        second_recommendation_arn(),
        recommendation_arn(shared_instance().check_id),
    ];
    assert_eq!(
        got, expected,
        "expected both recommendations in ARN order {expected:?}, got {got:?}"
    );
    for row in body["recommendationForResourceSummaries"]
        .as_array()
        .expect("recommendationForResourceSummaries")
    {
        assert_eq!(
            row["awsResourceArn"].as_str(),
            Some(arn.as_str()),
            "every row is about the resource asked for, got {row}"
        );
    }
}

/// Each filter keeps exactly the rows whose stored value it names: `status` the
/// flagged resource's status, `pillar` a member of the recommendation's
/// pillars, `checkArn` the check behind it.
#[tokio::test]
async fn every_filter_keeps_exactly_the_rows_it_names() {
    let router = emulator_with_two_checks_on_one_instance();
    let arn = shared_instance().arn();
    let seeded = recommendation_arn(shared_instance().check_id);
    let second = second_recommendation_arn();

    // Rows differ along the member filtered on, the number of matches (both,
    // one, none) and the combination of two filters.
    let cases: Vec<(String, Vec<String>)> = vec![
        (String::new(), vec![second.clone(), seeded.clone()]),
        ("?status=warning".to_string(), vec![seeded.clone()]),
        ("?status=error".to_string(), vec![second.clone()]),
        ("?status=ok".to_string(), vec![]),
        (
            "?pillar=cost_optimizing".to_string(),
            vec![seeded.clone()],
        ),
        ("?pillar=security".to_string(), vec![second.clone()]),
        ("?pillar=service_limits".to_string(), vec![]),
        (
            format!("?checkArn={}", label(&check_arn(shared_instance().check_id))),
            vec![seeded.clone()],
        ),
        (
            format!("?checkArn={}", label(&check_arn(SECOND_CHECK_ID))),
            vec![second.clone()],
        ),
        (
            format!("?checkArn={}", label(&check_arn("rds-idle-db-instances"))),
            vec![],
        ),
        (
            format!(
                "?pillar=cost_optimizing&checkArn={}",
                label(&check_arn(shared_instance().check_id))
            ),
            vec![seeded.clone()],
        ),
        (
            format!(
                "?pillar=security&checkArn={}",
                label(&check_arn(shared_instance().check_id))
            ),
            vec![],
        ),
        // `language` selects localized text, of which this output has none, so
        // it must change no row.
        (
            "?language=ja".to_string(),
            vec![second.clone(), seeded.clone()],
        ),
        ("?language=pt_BR&status=error".to_string(), vec![second]),
    ];

    for (query, expected) in cases {
        let (status, body) = list_for(&router, &arn, &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let got = field_of_each(&body, "recommendationForResourceSummaries", "recommendationArn");
        assert_eq!(
            got, expected,
            "case {query:?}: expected {expected:?}, got {got:?}"
        );
    }
}

/// `nextToken` walks every row exactly once at any page size, and the token is
/// bound to this resource and this filter set.
#[tokio::test]
async fn paginating_walks_every_row_exactly_once() {
    let router = emulator_with_two_checks_on_one_instance();
    let arn = shared_instance().arn();
    let all = vec![
        second_recommendation_arn(),
        recommendation_arn(shared_instance().check_id),
    ];

    // Rows differ along the page size: one row per page, exactly the whole
    // list, and a page larger than it (up to the modelled maximum).
    for page_size in [1usize, 2, 3, 600] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One round more than the number of pages the size implies, so a token
        // that never clears is caught as a loop rather than hanging.
        for _ in 0..=all.len() + 1 {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = list_for(&router, &arn, &query).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} with {body}"
            );
            let page = field_of_each(&body, "recommendationForResourceSummaries", "recommendationArn");
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} rows, got {page:?}"
            );
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case page size {page_size}: expected the cursor to clear, got {token:?}"
        );
        assert_eq!(
            seen, all,
            "case page size {page_size}: expected every row exactly once in order {all:?}, got \
             {seen:?}"
        );
    }
}

/// A `nextToken` this list did not issue is refused with the modelled
/// `ValidationException` (HTTP 400): the rows differ along what makes it
/// foreign — not a token at all, and a token issued for the same resource under
/// another filter set.
#[tokio::test]
async fn a_token_this_list_did_not_issue_is_refused() {
    let router = emulator_with_two_checks_on_one_instance();
    let arn = shared_instance().arn();

    let (status, body) = list_for(&router, &arn, "?maxResults=1").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    let issued = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a two-row list paged at 1 must issue a token: {body}"))
        .to_string();

    let cases = [
        ("not a token this service issued", "not-a-token".to_string()),
        (
            "a token of the unfiltered list replayed on a filtered one",
            format!("{}&pillar=security", label(&issued)),
        ),
    ];

    for (case, token) in cases {
        let query = format!("?maxResults=1&nextToken={token}");
        let (status, errortype, body) = dispatch(
            &router,
            "GET",
            &format!("/v1/recommendations-for-resource/{}{query}", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            errortype.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected x-amzn-errortype ValidationException, got {errortype:?} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected __type ValidationException, got {body}"
        );
    }
}

/// The ARN form a resource type is addressed by, as
/// `state::aws_resource_arn` states it: a type the table names gets that
/// type's ARN, and a type it does not name gets none (so no ARN can address
/// that finding).
#[tokio::test]
async fn the_arn_form_of_a_resource_type_is_the_type_s_own() {
    // Source: the per-type forms in `state::aws_resource_arn`, plus a type the
    // table does not name.
    let cases = [
        (
            "AWS::EC2::Instance",
            Some("arn:aws:ec2:us-east-1:{account}:instance/the-id"),
        ),
        (
            "AWS::EC2::Volume",
            Some("arn:aws:ec2:us-east-1:{account}:volume/the-id"),
        ),
        (
            "AWS::RDS::DBInstance",
            Some("arn:aws:rds:us-east-1:{account}:db:the-id"),
        ),
        ("AWS::Lambda::Function", None),
        ("", None),
    ];

    for (resource_type, expected) in cases {
        let expected = expected
            .map(|form| form.replace("{account}", engine::EMULATOR_ACCOUNT_ID));
        let got = aws_resource_arn(resource_type, "us-east-1", "the-id");
        assert_eq!(
            got, expected,
            "case {resource_type:?}: expected {expected:?}, got {got:?}"
        );
    }
}
