// SPDX-License-Identifier: CC0-1.0

//! The AWS resource ARN a stored recommendation resource is addressed by, over
//! HTTP through the same `MultiServiceRouter` the emulator serves with and on
//! the derivation itself.
//!
//! The rule: the ARN the emulator derives for a stored resource names the
//! account that OWNS that resource — the account segment of the resource's own
//! ARN (`state::arn_account_id`) — never the calling account, so a query by AWS
//! resource ARN answers with the findings of exactly one account's resources.
//!
//! The store spans the accounts of an AWS Organizations organization
//! (`state::recommendation_resources` is keyed by an ARN whose account segment
//! names the owner), so the rows below are a TABLE over several member accounts
//! holding findings about resources that carry the SAME `awsResourceId` and
//! `regionCode`: the one case where deriving the account from anything but the
//! resource's own ARN makes two accounts' resources collide on one ARN. They
//! span both `resourceArnQueryable` resource types the seeded catalog holds
//! (`AWS::EC2::Instance` and `AWS::RDS::DBInstance`, two different ARN forms),
//! every one of their resources and four 12-digit account values.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler, and the refusal of a resource
//! in another account, are pinned in `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    arn_account_id, aws_resource_arn, service_state, SupportPlan,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::owning_account_from_arn::enrol_member_account;
use crate::recommendation_loop::{
    call, dispatch, emulator_on_plan, error_code, field_of_each, label, recommendation_arn,
};

/// One seeded resource an AWS resource ARN can address: the check that flagged
/// it, its Region and the `{service}:{resource}` halves of the ARN form its
/// resource type is addressed by (the resource part carries the resource's
/// `awsResourceId`).
///
/// Source: the `SEED` table in `src/state.rs` and the per-type ARN forms in
/// `state::aws_resource_arn`.
struct Flagged {
    check_id: &'static str,
    region: &'static str,
    arn_service: &'static str,
    arn_resource: &'static str,
}

impl Flagged {
    /// The ARN of this flagged AWS resource as owned by `account`.
    fn arn(&self, account: &str) -> String {
        format!(
            "arn:aws:{}:{}:{}:{}",
            self.arn_service, self.region, account, self.arn_resource
        )
    }
}

/// Every seeded resource of the two prioritized checks, which are the checks an
/// organization recommendation aggregates member accounts for: three EC2
/// instances in three Regions and two RDS DB instances in two, i.e. two ARN
/// forms (`instance/<id>` and `db:<name>`).
const FLAGGED: &[Flagged] = &[
    Flagged {
        check_id: EC2_CHECK,
        region: "us-east-1",
        arn_service: "ec2",
        arn_resource: "instance/i-0abc1de23f4567890",
    },
    Flagged {
        check_id: EC2_CHECK,
        region: "us-west-2",
        arn_service: "ec2",
        arn_resource: "instance/i-0abc2de23f4567891",
    },
    Flagged {
        check_id: EC2_CHECK,
        region: "eu-west-1",
        arn_service: "ec2",
        arn_resource: "instance/i-0abc3de23f4567892",
    },
    Flagged {
        check_id: RDS_CHECK,
        region: "us-east-1",
        arn_service: "rds",
        arn_resource: "db:reporting-prod",
    },
    Flagged {
        check_id: RDS_CHECK,
        region: "us-west-2",
        arn_service: "rds",
        arn_resource: "db:analytics-staging",
    },
];

/// The checks whose findings the members below hold a copy of: the two
/// prioritized checks, the ones with an organization recommendation to enrol a
/// member account under. Source: the `SEED` table in `src/state.rs`.
const EC2_CHECK: &str = "low-utilization-ec2-instances";
const RDS_CHECK: &str = "rds-idle-db-instances";
const CHECKS: &[&str] = &[EC2_CHECK, RDS_CHECK];

/// The member accounts enrolled beside the emulator's own: two unrelated
/// 12-digit ids and one with leading zeros, so the account each ARN carries is
/// an arbitrary value and not a prefix of the caller's.
const MEMBERS: &[&str] = &["111122223333", "999988887777", "000000000012"];

/// A mounted emulator on a plan that includes the Trusted Advisor API, with the
/// engine it was built on so a test can enrol organization member accounts the
/// way something outside this API would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// A mounted emulator whose store also holds every member account's copy of
/// each check in [`CHECKS`] — the multi-account organization the store is keyed
/// for.
fn emulator_with_members() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator();
    for check in CHECKS {
        for member in MEMBERS {
            enrol_member_account(&engine, check, member);
        }
    }
    (router, engine)
}

/// `GET /v1/recommendations-for-resource/{awsResourceArn}`.
async fn list_for(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v1/recommendations-for-resource/{}", label(arn)),
        None,
    )
    .await
}

#[test]
fn the_derived_arn_carries_the_account_given_for_every_resource_type() {
    // Rows: the per-type ARN forms `state::aws_resource_arn` states, each
    // written out by hand, plus a resource type the table does not name (no ARN
    // form the emulator knows, so no ARN at all).
    let forms: &[(&str, Option<&str>)] = &[
        (
            "AWS::EC2::Instance",
            Some("arn:aws:ec2:eu-west-1:{account}:instance/the-id"),
        ),
        (
            "AWS::EC2::Volume",
            Some("arn:aws:ec2:eu-west-1:{account}:volume/the-id"),
        ),
        (
            "AWS::RDS::DBInstance",
            Some("arn:aws:rds:eu-west-1:{account}:db:the-id"),
        ),
        ("AWS::Lambda::Function", None),
        ("", None),
    ];
    // The account the ARN is formed for spans the emulator's own account and
    // every member account value, so no form is tied to one account.
    let accounts: Vec<&str> = std::iter::once(engine::EMULATOR_ACCOUNT_ID)
        .chain(MEMBERS.iter().copied())
        .collect();

    for account in accounts {
        for (resource_type, form) in forms {
            let expected = form.map(|form| form.replace("{account}", account));
            let got = aws_resource_arn(resource_type, account, "eu-west-1", "the-id");
            assert_eq!(
                got,
                expected,
                "case {resource_type:?}/{account}: expected {expected:?}, got {got:?}"
            );
        }
    }
}

#[test]
fn every_stored_resource_is_addressed_by_an_arn_in_the_account_that_owns_it() {
    let (_router, engine) = emulator_with_members();
    let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
    let state = state.lock();

    // Every stored resource of every account, including the emulator's own:
    // the ARN it is addressed by names the account its OWN ARN names, whatever
    // the calling account is.
    let mut checked = 0usize;
    for (key, resource) in &state.recommendation_resources {
        let Some(derived) = state.aws_resource_arn_of(resource) else {
            // A finding of a check that is not `resourceArnQueryable` is
            // addressable by no resource ARN at all.
            continue;
        };
        let owner = arn_account_id(&resource.arn);
        let in_derived = arn_account_id(&derived);
        assert_eq!(
            in_derived,
            owner,
            "case {key:?}: expected the derived ARN {derived:?} to name the owning account \
             {owner:?}, got {in_derived:?}"
        );
        checked += 1;
    }
    // The table is only a test of the rule if it actually spanned the member
    // accounts' resources as well as the emulator's own.
    let expected_rows = (MEMBERS.len() + 1) * FLAGGED.len();
    assert!(
        checked >= expected_rows,
        "expected at least {expected_rows} addressable resources across the organization, \
         checked {checked}"
    );
}

#[tokio::test]
async fn a_query_by_resource_arn_reports_only_the_calling_account_s_own_findings() {
    let (router, _engine) = emulator_with_members();

    for flagged in FLAGGED {
        let arn = flagged.arn(engine::EMULATOR_ACCOUNT_ID);
        let case = arn.clone();
        let (status, body) = list_for(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        // Exactly the calling account's own finding: one row, naming the
        // calling account's recommendation. A member account's copy of the same
        // `awsResourceId` in the same Region is a resource of ANOTHER account's
        // ARN, so it is reported by no ARN of this one.
        let got = field_of_each(&body, "recommendationForResourceSummaries", "recommendationArn");
        let expected = vec![recommendation_arn(flagged.check_id)];
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
        let rows = field_of_each(&body, "recommendationForResourceSummaries", "awsResourceArn");
        assert_eq!(
            rows,
            vec![arn.clone()],
            "case {case:?}: expected every row to be about {arn:?}, got {rows:?}"
        );
    }
}

#[tokio::test]
async fn enrolling_member_accounts_changes_no_answer_the_calling_account_reads() {
    let (bare, _bare_engine) = emulator();
    let (with_members, _engine) = emulator_with_members();

    // One store holds the seeded account's findings only, the other also every
    // member account's copy of them; a query by the calling account's resource
    // ARN answers identically, member for member.
    for flagged in FLAGGED {
        let arn = flagged.arn(engine::EMULATOR_ACCOUNT_ID);
        let case = arn.clone();
        let (bare_status, bare_body) = list_for(&bare, &arn).await;
        let (status, body) = list_for(&with_members, &arn).await;
        assert_eq!(
            status, bare_status,
            "case {case:?}: expected status {bare_status}, got {status} with {body}"
        );
        assert_eq!(
            body, bare_body,
            "case {case:?}: expected the seeded answer {bare_body}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_member_account_s_own_resource_arn_is_refused_for_the_calling_account() {
    let (router, _engine) = emulator_with_members();

    // The true ARN of a member account's resource is an ARN in another account,
    // which no support plan grants this caller access to: 403
    // `AccessDeniedException`, naming the resource that was refused — and never
    // a row of another account's findings.
    for flagged in FLAGGED {
        for member in MEMBERS {
            let arn = flagged.arn(member);
            let case = arn.clone();
            let (status, errortype, body) = dispatch(
                &router,
                "GET",
                &format!("/v1/recommendations-for-resource/{}", label(&arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status} with {body}"
            );
            assert_eq!(
                errortype.as_deref(),
                Some("AccessDeniedException"),
                "case {case:?}: expected x-amzn-errortype AccessDeniedException, got \
                 {errortype:?} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: expected __type AccessDeniedException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(format!("Access denied to resource: {arn}")),
                "case {case:?}: expected the refusal to name the resource, got {body}"
            );
        }
    }
}
