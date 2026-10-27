// SPDX-License-Identifier: CC0-1.0

//! The organization-wide `includeMemberAccounts` member, end to end over the wire.
//!
//! Nine operations of this service take it — the eight `Export*Recommendations` surfaces and
//! `UpdateEnrollmentStatus` — and every one of them conditions it on the CALLER's role: the
//! member asks for the resources, or the enrollment, of "all member accounts of the
//! organization", which only an organization's management account or delegated administrator
//! has. So a caller that manages no organization asking for them is refused with the modelled
//! 400 `InvalidParameterValueException` and nothing is written, while the same call that does
//! not ask for them is admitted, and a caller whose organization this service records has the
//! member honoured.
//!
//! WHICH accounts an organization holds is recorded by AWS Organizations and no operation of
//! THIS API creates one, so the fixtures record member accounts into the store the way that
//! service would and then drive every call over the same `MultiServiceRouter` dispatch a
//! client uses. What is asserted is the RULE, over a table: one row per operation that takes
//! the member, crossed with the value it carries and with the organizations a caller can
//! manage.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, MemberAccountEnrollment,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_bucket, emulator_with_engine, error_code, opt_in, ACCOUNT};

/// The refusal every operation answers a caller that asks for member accounts while managing
/// no organization with: the service's constant, which names neither the member nor the
/// account — it states the two reasons the caller cannot ask for them. Spelled out here rather
/// than imported so the WORDING is what this suite pins.
const INCLUDE_MEMBER_ACCOUNTS_MESSAGE: &str = "Cannot include member accounts. This could be \
     because you are not an AWS Organization user or you are not the master account in the \
     organization.";

/// The destination bucket the export rows below export to. Compute Optimizer does not create
/// it; the request just names it.
const BUCKET: &str = "member-account-exports";

/// One operation that takes `includeMemberAccounts`, and the rest of the request it needs.
struct Surface {
    /// The operation this row drives.
    operation: &'static str,
    /// The members the request carries BESIDES `includeMemberAccounts`: the required input of
    /// this operation's own surface.
    rest: fn() -> Value,
}

/// The body an export surface carries besides the member: a destination bucket, which "you
/// must create ... before you create the export job".
fn export_rest() -> Value {
    json!({"s3DestinationConfig": {"bucket": BUCKET}})
}

/// The body `UpdateEnrollmentStatus` carries besides the member: the `status` to write, which
/// a client may set to `Active` or `Inactive`.
fn enrollment_rest() -> Value {
    json!({"status": "Active"})
}

/// Every operation whose input declares `includeMemberAccounts`
/// (`smithy_query(query="operation",operation="ExportIdleRecommendations,UpdateEnrollmentStatus")`
/// and its export siblings): the eight export surfaces and the enrollment write.
fn surfaces() -> Vec<Surface> {
    let exports = [
        "ExportAutoScalingGroupRecommendations",
        "ExportEBSVolumeRecommendations",
        "ExportEC2InstanceRecommendations",
        "ExportECSServiceRecommendations",
        "ExportIdleRecommendations",
        "ExportLambdaFunctionRecommendations",
        "ExportLicenseRecommendations",
        "ExportRDSDatabaseRecommendations",
    ];
    let mut surfaces: Vec<Surface> = exports
        .into_iter()
        .map(|operation| Surface {
            operation,
            rest: export_rest,
        })
        .collect();
    surfaces.push(Surface {
        operation: "UpdateEnrollmentStatus",
        rest: enrollment_rest,
    });
    surfaces
}

/// One surface's request, carrying `include_member_accounts` — or, for `None`, not carrying
/// the member at all, which the model defaults to `false`.
fn request(surface: &Surface, include_member_accounts: Option<bool>) -> Value {
    let mut request = (surface.rest)();
    if let Some(include) = include_member_accounts {
        request["includeMemberAccounts"] = json!(include);
    }
    request
}

/// An emulator with the export destination created and the account opted in — what every
/// export call needs before the member itself is judged — and the engine it was built on, so a
/// row can record an organization the way AWS Organizations would.
async fn enrolled_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    (router, engine)
}

/// Record `members` as member accounts of the caller's organization, the way AWS Organizations
/// would: no operation of this API creates one.
fn record_organization(engine: &Arc<engine::InMemoryEngine>, members: &[MemberAccountEnrollment]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for member in members {
        guard.organization_members.insert(member.clone());
    }
}

/// The organizations a management account can be recorded with, spanning what the rule
/// generalises over — the organization's SIZE and the enrollment its members are in, neither of
/// which the member itself depends on: one member account, several, and one whose enrollment is
/// already settled at a status.
fn organizations() -> Vec<(&'static str, Vec<MemberAccountEnrollment>)> {
    let settled = |account_id: &str, status: &str| {
        let mut member = MemberAccountEnrollment::new(account_id);
        member.status = status.to_string();
        member
    };
    vec![
        (
            "one member account",
            vec![MemberAccountEnrollment::new("200000000001")],
        ),
        (
            "three member accounts",
            vec![
                MemberAccountEnrollment::new("200000000001"),
                MemberAccountEnrollment::new("200000000002"),
                MemberAccountEnrollment::new("200000000003"),
            ],
        ),
        (
            "a member account already opted in",
            vec![settled("200000000004", "Active")],
        ),
        (
            "a member account opted out",
            vec![settled("200000000005", "Inactive")],
        ),
    ]
}

/// The job ids `DescribeRecommendationExportJobs` reports, in the order it reported them.
async fn reported_jobs(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "DescribeRecommendationExportJobs", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the export jobs back: got {status}: {body}"
    );
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a job list, got {body}"))
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The account's own enrollment, as `GetEnrollmentStatus` reports it.
async fn enrollment(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = call(router, "GetEnrollmentStatus", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the enrollment back: got {status}: {body}"
    );
    body
}

/// Every operation that takes `includeMemberAccounts` refuses `true` from a caller that manages
/// no organization — with the same wording, the modelled 400, and without writing anything a
/// later caller could see.
#[tokio::test]
async fn member_accounts_are_refused_to_a_caller_that_manages_no_organization() {
    let (router, _engine) = enrolled_emulator().await;
    let before = enrollment(&router, "before any refusal").await;

    for surface in surfaces() {
        let asked = request(&surface, Some(true));
        let case = format!("{} with {asked}", surface.operation);
        let (status, body) = call(&router, surface.operation, asked).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INCLUDE_MEMBER_ACCOUNTS_MESSAGE),
            "case {case:?}: expected the member-accounts refusal, got {body}"
        );
        assert!(
            body["jobId"].is_null(),
            "case {case:?}: a refused call answers no job id, got {body}"
        );
        assert!(
            body["status"].is_null(),
            "case {case:?}: a refused call answers no enrollment status, got {body}"
        );
    }

    // Not one of those calls wrote: no export job was queued and the account's own enrollment
    // is exactly where the opt-in left it.
    let jobs = reported_jobs(&router, "after every refusal").await;
    assert!(
        jobs.is_empty(),
        "a refused export records no job, got {jobs:?}"
    );
    let after = enrollment(&router, "after every refusal").await;
    assert_eq!(
        after, before,
        "a refused enrollment write must not move the stored record, got {after}"
    );
    assert_eq!(
        after["memberAccountsEnrolled"],
        json!(false),
        "a refused call never records member accounts as enrolled, got {after}"
    );
}

/// The same operations admit the call that does NOT ask for member accounts — `false`, and the
/// member left out altogether, which the model defaults to `false` — so the refusal is about
/// what the request asks for and not about the member being present.
#[tokio::test]
async fn a_call_that_asks_for_no_member_accounts_is_admitted() {
    for include_member_accounts in [Some(false), None] {
        for surface in surfaces() {
            // One emulator per admitted export: only one export job of a resource type may be
            // in progress per Region, so two rows of one surface cannot share a store.
            let (router, _engine) = enrolled_emulator().await;
            let asked = request(&surface, include_member_accounts);
            let case = format!("{} with {asked}", surface.operation);

            let (status, body) = call(&router, surface.operation, asked).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be admitted, got {status}: {body}"
            );

            let after = enrollment(&router, &case).await;
            assert_eq!(
                after["memberAccountsEnrolled"],
                json!(false),
                "case {case:?}: a call that asks for no member accounts records none as \
                 enrolled, got {after}"
            );
        }
    }
}

/// A caller whose organization this service records has `includeMemberAccounts=true` honoured
/// by every one of those operations, whatever that organization holds: the export is queued and
/// reported, and the enrollment write records the member accounts as enrolled and reaches every
/// one of them.
#[tokio::test]
async fn member_accounts_are_honoured_for_a_caller_that_manages_an_organization() {
    for (organization, members) in organizations() {
        let (router, engine) = enrolled_emulator().await;
        record_organization(&engine, &members);

        let mut admitted: Vec<String> = Vec::new();
        for surface in surfaces() {
            let asked = request(&surface, Some(true));
            let case = format!("{organization}: {} with {asked}", surface.operation);
            let (status, body) = call(&router, surface.operation, asked).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be admitted, got {status}: {body}"
            );
            if let Some(job_id) = body["jobId"].as_str() {
                admitted.push(job_id.to_string());
            }
        }

        // The admitted exports are in the store — one per export surface, each the job its
        // call answered.
        let jobs = reported_jobs(&router, organization).await;
        assert_eq!(
            jobs, admitted,
            "case {organization:?}: the store holds exactly the admitted export jobs, got \
             {jobs:?}"
        );

        // The admitted enrollment write recorded the member accounts as enrolled, and the
        // status it asked for reached every member account the organization holds.
        let after = enrollment(&router, organization).await;
        assert_eq!(
            after["memberAccountsEnrolled"],
            json!(true),
            "case {organization:?}: an honoured call records the member accounts as enrolled, \
             got {after}"
        );
        let (status, body) = call(
            &router,
            "GetEnrollmentStatusesForOrganization",
            json!({"maxResults": 100}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {organization:?}: reading the organization back: got {status}: {body}"
        );
        let rows = body["accountEnrollmentStatuses"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {organization:?}: the reader reports a row list, got {body}")
            });
        assert_eq!(
            rows.len(),
            members.len(),
            "case {organization:?}: one row per member account, got {body}"
        );
        for row in rows {
            let reported = row["status"].as_str().unwrap_or_default();
            assert!(
                reported == "Active" || reported == "Pending",
                "case {organization:?}: the enrollment asked for reaches every member account \
                 (through Pending), got {row}"
            );
        }
    }
}

/// The member-accounts refusal precedes the enrollment gate every export surface applies: it is
/// about what the request asks for, not about what the store holds for the caller, so an
/// account that never opted in gets the same 400 rather than the 403 its own export gets.
#[tokio::test]
async fn the_member_accounts_refusal_precedes_the_opt_in_gate() {
    let (router, _engine) = emulator_with_engine();
    create_bucket(&router, BUCKET).await;

    for surface in surfaces() {
        if surface.operation == "UpdateEnrollmentStatus" {
            // The enrollment write is what opts an account IN, so it has no opt-in gate to
            // precede; its own refusal is asserted by the rows above.
            continue;
        }
        let asked = request(&surface, Some(true));
        let case = format!("not opted in: {} with {asked}", surface.operation);
        let (status, body) = call(&router, surface.operation, asked).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the member-accounts refusal, got {status}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INCLUDE_MEMBER_ACCOUNTS_MESSAGE),
            "case {case:?}: expected the member-accounts refusal, got {body}"
        );
    }

    // Enrolling is what admits an export — and what admits the reader that reports them: the
    // store the refused calls left alone is empty.
    opt_in(&router).await;
    let jobs = reported_jobs(&router, "after every refusal").await;
    assert!(
        jobs.is_empty(),
        "a refused export records no job, got {jobs:?}"
    );
}

/// An `accountIds` naming the caller's own account is still mutually exclusive with the member:
/// "This parameter can't be specified together with the include member accounts parameter. The
/// parameters are mutually exclusive" — refused as a conflict rather than as the member's own
/// organization rule, whichever organization the caller manages.
#[tokio::test]
async fn the_mutual_exclusion_is_judged_before_the_organization() {
    for (organization, members) in organizations() {
        let (router, engine) = enrolled_emulator().await;
        record_organization(&engine, &members);

        for surface in surfaces() {
            let mut asked = request(&surface, Some(true));
            if surface.operation == "UpdateEnrollmentStatus" {
                // `accountIds` is not a member of this surface at all.
                continue;
            }
            asked["accountIds"] = json!([ACCOUNT]);
            let case = format!("{organization}: {} with {asked}", surface.operation);

            let (status, body) = call(&router, surface.operation, asked).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the conflict to be refused, got {status}: {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(
                    "The value supplied for parameter accountIds is not valid: accountIds and \
                     includeMemberAccounts are mutually exclusive."
                ),
                "case {case:?}: expected the mutual-exclusion refusal, got {body}"
            );
        }
    }
}
