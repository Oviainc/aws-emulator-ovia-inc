// SPDX-License-Identifier: CC0-1.0

//! The cursor scope of a paginated read identifies that read UNIQUELY.
//!
//! Every list of this service builds its cursor over a scope made of the list it
//! pages (with, for a sub-resource list, the parent it was taken under) and the
//! filters in force, and refuses a `nextToken` whose scope is not the scope of
//! the read replaying it — the filters decide the SEQUENCE the cursor points
//! into, so a token followed under another filter set would not walk every entry
//! exactly once.
//!
//! That refusal holds only if the encoding is INJECTIVE: one scope string must
//! come from exactly one list and filter set. A filter value is request text, and
//! the model constrains only some of it —
//! `smithy_query(query="operation",operation="ListOrganizationRecommendationResources",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! shows `status` and `exclusionStatus` as enums and `affectedAccountId` as
//! `length 12..12, pattern ^\d+$`, but `regionCode` as a plain `String` with no
//! constraint at all, and
//! `ListRecommendationsForResource$awsResourceArn` as a pattern ending `([:/].+)*$`,
//! a free-form tail. So a caller can put ANY character, including the ones the
//! encoding delimits on, into a value or into the parent a sub-resource list
//! names.
//!
//! The rules this suite pins, each over a table of inputs:
//!
//! * two different (list, filter set) pairs never encode to the same cursor
//!   scope, whatever characters their values carry — checked over the whole cross
//!   product of a value alphabet built from those delimiters;
//! * a `nextToken` one read issued is refused with 400 `ValidationException` by a
//!   read whose filter values merely SPELL OUT the issuing read's scope, on the
//!   list whose free-form filter precedes a constrained one
//!   (`ListOrganizationRecommendationResources`) and on the list whose scope
//!   embeds a free-form parent ARN (`ListRecommendationsForResource`); and the
//!   issuing read still resumes with it.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use std::collections::HashMap;

use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    aws_resource_arn, list_scope, service_state, SupportPlan,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::organization_recommendation_arn;
use crate::recommendation_loop::{call, emulator_on_plan, error_code, label, resource_arn};

/// The ASCII separator characters a delimited encoding of a cursor scope uses —
/// the record separator between the parts of a scope and the unit separator
/// between a scope and the key it points at. Written out here rather than shared
/// with `src/state.rs`: this suite is a caller forging with the characters such
/// an encoding delimits on, not a second copy of the encoding.
const SCOPE_SEPARATOR: &str = "\u{1e}";
const TOKEN_SEPARATOR: &str = "\u{1f}";

/// The seeded check this suite pages, and the Region two of its resources sit in
/// once [`emulator_with_two_resources_in_one_region`] has run. Source: the `SEED`
/// table in `src/state.rs`, whose `res-ec2-0001` is a `us-east-1` resource of
/// this check's prioritized recommendation.
const CHECK: &str = "low-utilization-ec2-instances";
const REGION: &str = "us-east-1";
const SEEDED_RESOURCE_ID: &str = "res-ec2-0001";

/// One query value as it goes into a request URI.
///
/// `label` covers the alphabet an ARN or a `nextToken` uses; a value this suite
/// forges with is arbitrary request text, so the characters outside that alphabet
/// are percent-encoded here too — a URI carries no raw control character, and a
/// backslash and an ampersand would be read as part of the query's own syntax.
fn forged(value: &str) -> String {
    label(value)
        .replace('\u{1e}', "%1E")
        .replace('\u{1f}', "%1F")
        .replace('\\', "%5C")
        .replace('&', "%26")
}

#[test]
fn every_distinct_list_and_filter_set_encodes_to_its_own_cursor_scope() {
    // The value alphabet: a plain value for each of the three filters, then
    // values carrying each separator character, the backslash an escaping
    // encoding would use, and text that spells out the whole of another filter's
    // part of a delimited scope — so a value and the filter set it imitates are
    // both in the product below. Rows differ along which separator they carry,
    // where it sits, and whether a backslash precedes it.
    let spliced = format!("us-east-1{SCOPE_SEPARATOR}affectedAccountId=111122223333");
    let spliced_after_backslash =
        format!("us-east-1\\{SCOPE_SEPARATOR}affectedAccountId=111122223333");
    let spliced_twice = format!(
        "us-east-1{SCOPE_SEPARATOR}affectedAccountId=111122223333{SCOPE_SEPARATOR}status=ok"
    );
    let values: Vec<&str> = vec![
        "us-east-1",
        "111122223333",
        "ok",
        &spliced,
        "us-east-1\\",
        &spliced_after_backslash,
        &spliced_twice,
        SCOPE_SEPARATOR,
        TOKEN_SEPARATOR,
        "=",
        "affectedAccountId=111122223333",
        "\\\\",
    ];
    // The filter names a handler of this service lists, and lists one of which
    // embeds a parent the caller named: every scope shape the crate builds is one
    // of these two.
    let names = ["regionCode", "affectedAccountId", "status"];
    let spliced_list = format!("organizationRecommendationResources/one{SCOPE_SEPARATOR}status=ok");
    let lists: Vec<&str> = vec![
        "organizationRecommendationResources/one",
        &spliced_list,
        "recommendations-for-resource/arn:aws:ec2:us-east-1:111122223333:instance/i-01\\",
    ];

    let mut options: Vec<Option<&str>> = vec![None];
    options.extend(values.iter().map(|value| Some(*value)));

    // Exhaustive over the cross product: every list crossed with every choice
    // (absent, or each value of the alphabet) for each of the three filters.
    let mut seen: HashMap<String, String> = HashMap::new();
    for list in &lists {
        for first in &options {
            for second in &options {
                for third in &options {
                    let filters = [
                        (names[0], *first),
                        (names[1], *second),
                        (names[2], *third),
                    ];
                    let case = format!("list {list:?} filters {filters:?}");
                    let scope = list_scope(list, &filters);
                    let previous = seen.insert(scope.clone(), case.clone());
                    assert!(
                        previous.is_none(),
                        "case {case}: expected a scope no other filter set can produce, got \
                         {scope:?}, which is also the scope of {previous:?}"
                    );
                }
            }
        }
    }
    assert_eq!(
        seen.len(),
        lists.len() * options.len().pow(3),
        "case the whole cross product: expected one scope per list and filter set, got {}",
        seen.len()
    );
}

/// A mounted emulator whose store holds a SECOND resource of [`CHECK`]'s
/// recommendation in [`REGION`], so a read of that Region's resources for one
/// account matches two entries and its first page therefore carries a
/// `nextToken`.
///
/// Written straight into the store: Trusted Advisor models no operation that
/// creates a check result, so a second finding can only arrive from outside the
/// API, exactly as the sibling suites write one.
fn emulator_with_two_resources_in_one_region() -> MultiServiceRouter {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    let state = service_state(&*engine).expect("the engine carries this service's state");
    let mut state = state.lock();
    let seeded = state
        .recommendation_resources
        .get(&resource_arn(CHECK, SEEDED_RESOURCE_ID))
        .cloned()
        .expect("the seeded store holds this check's first resource");
    assert_eq!(
        seeded.region_code, REGION,
        "case the seeded resource's Region: expected {REGION}, got {}",
        seeded.region_code
    );
    let mut second = seeded.clone();
    second.id = "res-ec2-0009".to_string();
    second.arn = resource_arn(CHECK, "res-ec2-0009");
    second.aws_resource_id = "i-0abc9de23f4567899".to_string();
    state
        .recommendation_resources
        .insert(second.arn.clone(), second);
    drop(state);
    router
}

/// `GET /v1/organization-recommendations/{identifier}/resources` for [`CHECK`]'s
/// organization recommendation.
fn resources_path() -> String {
    format!(
        "/v1/organization-recommendations/{}/resources",
        label(&organization_recommendation_arn(CHECK))
    )
}

#[tokio::test]
async fn a_filter_value_that_spells_out_another_reads_scope_cannot_replay_its_token() {
    let router = emulator_with_two_resources_in_one_region();
    let own = engine::EMULATOR_ACCOUNT_ID;

    // The issuing read's filters, and the replaying read's — whose values spell
    // out the issuing read's scope. Rows differ along how many filters the
    // issuing read holds (so along the shape of the scope forged), along which
    // delimiter the forged value carries, and along whether a backslash precedes
    // it.
    let plain = format!("regionCode={REGION}&affectedAccountId={own}");
    let spliced = format!(
        "regionCode={}",
        forged(&format!(
            "{REGION}{SCOPE_SEPARATOR}affectedAccountId={own}"
        ))
    );
    let cases: &[(&str, String, String)] = &[
        (
            "the account filter's part spliced into regionCode",
            plain.clone(),
            spliced.clone(),
        ),
        (
            "the same splice under a status filter",
            format!("status=warning&{plain}"),
            format!("status=warning&{spliced}"),
        ),
        (
            "the same splice under an exclusionStatus filter",
            format!("exclusionStatus=included&{plain}"),
            format!("exclusionStatus=included&{spliced}"),
        ),
        (
            "a backslash before the spliced separator",
            plain.clone(),
            format!(
                "regionCode={}",
                forged(&format!(
                    "{REGION}\\{SCOPE_SEPARATOR}affectedAccountId={own}"
                ))
            ),
        ),
        (
            "the cursor's own separator spliced into regionCode",
            plain.clone(),
            format!(
                "regionCode={}",
                forged(&format!(
                    "{REGION}{TOKEN_SEPARATOR}affectedAccountId={own}"
                ))
            ),
        ),
        (
            "the account filter dropped and nothing spliced",
            plain.clone(),
            format!("regionCode={REGION}"),
        ),
    ];

    for (case, issuing, replaying) in cases {
        // The replaying read is a well-formed read of this list on its own, so
        // the refusal below is about the TOKEN and not about its input.
        let (status, body) = call(
            &router,
            "GET",
            &format!("{}?{replaying}", resources_path()),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the replaying read's filters to be accepted without a token, \
             got {status} with {body}"
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("{}?{issuing}&maxResults=1", resources_path()),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the issuing read, got {status} with {body}"
        );
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a nextToken on a truncated page, got {body}")
            })
            .to_string();

        let (status, body) = call(
            &router,
            "GET",
            &format!(
                "{}?{replaying}&nextToken={}",
                resources_path(),
                label(&token)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the replaying read to refuse the token with 400, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );

        // And the read that issued the token still resumes with it: making the
        // encoding unambiguous refuses no cursor its own read.
        let (status, body) = call(
            &router,
            "GET",
            &format!(
                "{}?{issuing}&maxResults=1&nextToken={}",
                resources_path(),
                label(&token)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the issuing read to resume with its own token, got {status} \
             with {body}"
        );
        let resumed = body["organizationRecommendationResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no summaries in {body}"))
            .len();
        assert_eq!(
            resumed, 1,
            "case {case}: expected the second page to carry the remaining resource, got {body}"
        );
    }
}

/// A mounted emulator whose store holds a SECOND finding on ONE EC2 instance —
/// two resources of [`CHECK`]'s recommendation naming the same AWS resource — so
/// `ListRecommendationsForResource` on that instance's ARN has two rows to page,
/// together with that ARN.
fn emulator_with_two_findings_on_one_instance() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    let state = service_state(&*engine).expect("the engine carries this service's state");
    let mut state = state.lock();
    let seeded = state
        .recommendation_resources
        .get(&resource_arn(CHECK, SEEDED_RESOURCE_ID))
        .cloned()
        .expect("the seeded store holds this check's first resource");
    let mut second = seeded.clone();
    second.id = "res-ec2-0009".to_string();
    second.arn = resource_arn(CHECK, "res-ec2-0009");
    state
        .recommendation_resources
        .insert(second.arn.clone(), second);
    drop(state);
    // The ARN of the instance both findings flag, derived the one way the
    // service derives it (`state::aws_resource_arn`), from the seeded resource's
    // own type, account, Region and id.
    let instance_arn = aws_resource_arn(
        "AWS::EC2::Instance",
        engine::EMULATOR_ACCOUNT_ID,
        &seeded.region_code,
        &seeded.aws_resource_id,
    )
    .expect("an EC2 instance has an ARN form");
    (router, instance_arn)
}

/// `GET /v1/recommendations-for-resource/{awsResourceArn}{query}`.
fn for_resource_path(arn: &str, query: &str) -> String {
    format!("/v1/recommendations-for-resource/{}{query}", forged(arn))
}

#[tokio::test]
async fn a_parent_arn_that_spells_out_another_reads_scope_cannot_replay_its_token() {
    let (router, instance_arn) = emulator_with_two_findings_on_one_instance();

    // The issuing read holds a filter whose part of the scope the replaying
    // read's ARN spells out instead. Rows differ along the filter the token was
    // issued under and along whether a backslash precedes the splice.
    let cases: &[(&str, &str, String)] = &[
        (
            "the status filter's part spliced into the resource ARN",
            "?status=warning&maxResults=1",
            format!("{instance_arn}{SCOPE_SEPARATOR}status=warning"),
        ),
        (
            "the pillar filter's part spliced into the resource ARN",
            "?pillar=cost_optimizing&maxResults=1",
            format!("{instance_arn}{SCOPE_SEPARATOR}pillar=cost_optimizing"),
        ),
        (
            "a backslash before the spliced separator",
            "?status=warning&maxResults=1",
            format!("{instance_arn}\\{SCOPE_SEPARATOR}status=warning"),
        ),
    ];

    for (case, issuing, replaying_arn) in cases {
        // The replaying read is a well-formed read on its own: that ARN names no
        // flagged resource, so it answers an empty list.
        let (status, body) = call(&router, "GET", &for_resource_path(replaying_arn, ""), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the replaying read's ARN to be accepted without a token, got \
             {status} with {body}"
        );

        let (status, body) =
            call(&router, "GET", &for_resource_path(&instance_arn, issuing), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the issuing read, got {status} with {body}"
        );
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a nextToken on a truncated page, got {body}")
            })
            .to_string();

        let (status, body) = call(
            &router,
            "GET",
            &for_resource_path(replaying_arn, &format!("?nextToken={}", label(&token))),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the replaying read to refuse the token with 400, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );

        let (status, body) = call(
            &router,
            "GET",
            &for_resource_path(
                &instance_arn,
                &format!("{issuing}&nextToken={}", label(&token)),
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the issuing read to resume with its own token, got {status} \
             with {body}"
        );
        let resumed = body["recommendationForResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no summaries in {body}"))
            .len();
        assert_eq!(
            resumed, 1,
            "case {case}: expected the second page to carry the remaining row, got {body}"
        );
    }
}
