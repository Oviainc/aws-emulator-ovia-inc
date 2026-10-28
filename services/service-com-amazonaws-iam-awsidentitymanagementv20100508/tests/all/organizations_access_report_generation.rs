// SPDX-License-Identifier: CC0-1.0

//! Who `GenerateOrganizationsAccessReport` is served to, end to end over the awsQuery wire.
//!
//! The rule under test: the generation is served ONLY to a caller whose account MANAGES the
//! organization the requested entity path names — "to call this operation, you must be
//! signed in using your Organizations management account credentials" — so an account that
//! is in no organization (the standalone account this emulator presents until something
//! records otherwise), a member account, and the management account of another organization
//! than the path names are each refused HTTP 403 `AccessDenied` with no job recorded; the
//! management account of the organization the path names is served, and the job its `JobId`
//! identifies is the one `GetOrganizationsAccessReport` then reports. IAM authorizes its own
//! API first, so the caller's identity-based policies decide the call before the
//! organization is consulted at all.
//!
//! What the job itself is — its `IN_PROGRESS` → `COMPLETED` clock, the concurrency quota and
//! the retention bound — is pinned by
//! `tests/all/boundary_and_provider_writes.rs`; this suite is about the decision in front of
//! it.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the entity
//! path's length (both published bounds) and which kind of Organizations entity it names (a
//! root, an organizational unit, an account), whether the request narrows the report to a
//! service control policy and how that policy id is spelled, which organization the account
//! manages against which the path names, and what the caller's policies allow.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The IAM action this operation is decided on, as a policy statement names it.
const ACTION: &str = "iam:GenerateOrganizationsAccessReport";

/// A 12-digit account that is not the one this emulator's callers speak for, for the row
/// where the organization is managed by somebody else.
const ANOTHER_ACCOUNT: &str = "111122223333";

/// The organization the fixtures use, spelled as `OrganizationIdType` requires
/// (`^o-[a-z0-9]{10,32}$`).
const ORGANIZATION: &str = "o-a1b2c3d4e5";

/// Another organization, for the row whose entity path names one the account does not
/// manage.
const OTHER_ORGANIZATION: &str = "o-z9y8x7w6v5";

/// The `jobStatusType` value of a job whose generation is not due yet, written out here
/// rather than taken from the implementation (`smithy_query(query="shape",
/// shape="jobStatusType")`: `IN_PROGRESS`, `COMPLETED`, `FAILED`).
const JOB_IN_PROGRESS: &str = "IN_PROGRESS";

/// The entity paths the suites span, each labelled by what makes it a distinct row.
///
/// Source: `GenerateOrganizationsAccessReport$EntityPath`
/// (`smithy_query(query="operation", operation="GenerateOrganizationsAccessReport")`) —
/// `organizationsEntityPathType`, len 19..427, pattern
/// `^o-[0-9a-z]{10,32}\/r-[0-9a-z]{4,32}[0-9a-z-\/]*$` — so the rows carry BOTH published
/// length bounds and each kind of entity the documentation names ("root, OU, or account").
fn entity_paths() -> Vec<(&'static str, String)> {
    // The published maximum length, built from the longest organization and root ids the
    // pattern admits plus a tail of the characters it leaves free.
    let longest = {
        let head = format!("o-{}/r-{}", "a".repeat(32), "b".repeat(32));
        let tail = "0".repeat(427 - head.len() - 1);
        format!("{head}/{tail}")
    };
    vec![
        // The published minimum length: the shortest organization and root ids.
        ("the shortest path", "o-0123456789/r-ab12".to_string()),
        (
            "an organization root",
            format!("{ORGANIZATION}/r-f6g7h8i9j0example"),
        ),
        (
            "an organizational unit",
            format!("{ORGANIZATION}/r-f6g7h8i9j0example/ou-rge0-awsabcde"),
        ),
        (
            "an account under an organizational unit",
            format!("{ORGANIZATION}/r-f6g7h8i9j0example/ou-rge0-awsabcde/123456789012"),
        ),
        ("the longest path", longest),
    ]
}

/// The `OrganizationsPolicyId` values the suites span: none at all (the entity report),
/// and two spellings of a policy id.
///
/// Source: `GenerateOrganizationsAccessReport$OrganizationsPolicyId`
/// (`organizationsPolicyIdType`, pattern `^p-[0-9a-zA-Z_]{8,128}$`), so the rows carry the
/// whole published alphabet and the published maximum length.
fn policy_ids() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no policy", None),
        ("a policy id", Some("p-Full_AWS_Access9".to_string())),
        (
            "the longest policy id",
            Some(format!("p-{}", "p".repeat(128))),
        ),
    ]
}

/// The organization the entity path `entity_path` names.
fn organization_of(entity_path: &str) -> &str {
    entity_path
        .split('/')
        .next()
        .expect("an entity path carries an organization segment")
}

/// The whole `AccessDenied` message a caller that does not manage `organization_id` is
/// refused with: the requirement the operation's own documentation states, naming the
/// organization the request asked about.
fn not_the_management_account(organization_id: &str) -> String {
    format!(
        "To call this operation, you must be signed in using the Organizations management \
         account credentials of organization {organization_id}."
    )
}

/// Record that `management_account_id` manages `organization_id` in the service's OWN store.
///
/// This is the state AWS Organizations — not IAM — puts an account in: no operation of this
/// service records an organization, so the fact is written into the store the handler reads
/// rather than into a second copy of it, which is how a suite reaches the world in which
/// this account IS a management account.
///
/// `pub(crate)` because every suite that drives a served
/// `GenerateOrganizationsAccessReport` has to put the account in that position
/// (`tests/all/boundary_and_provider_writes.rs`), and it is done one way.
pub(crate) fn record_organization(
    engine: &engine::InMemoryEngine,
    organization_id: &str,
    management_account_id: &str,
) {
    let store = state::service_state(engine).expect("the IAM store");
    store.lock().organization = Some(state::Organization {
        organization_id: organization_id.to_string(),
        management_account_id: management_account_id.to_string(),
    });
}

/// The report jobs the account holds, read straight out of the store: the job ids beside
/// what each report was asked for.
fn held_jobs(engine: &engine::InMemoryEngine) -> Vec<(String, String, Option<String>)> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store
        .lock()
        .organizations_access_reports
        .iter()
        .map(|(job_id, job)| {
            (
                job_id.clone(),
                job.entity_path.clone(),
                job.organizations_policy_id.clone(),
            )
        })
        .collect();
    held
}

/// One `GenerateOrganizationsAccessReport` call, presented as signed by `akid` when a row
/// names one.
async fn generate(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    entity_path: &str,
    organizations_policy_id: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("EntityPath", entity_path)];
    if let Some(policy_id) = organizations_policy_id {
        params.push(("OrganizationsPolicyId", policy_id));
    }
    call_signed_by(router, akid, "GenerateOrganizationsAccessReport", &params).await
}

/// RULE: an account that manages no organization — the standalone account this emulator
/// presents — is refused every entity path, whatever the request narrows the report to, and
/// no job is recorded.
#[tokio::test]
async fn an_account_that_manages_no_organization_is_refused_every_entity_path() {
    for (label, entity_path) in entity_paths() {
        for (policy_label, policy_id) in policy_ids() {
            let (router, engine) = router_with_engine();
            let case = format!("{label} / {policy_label}");

            let (status, body) = generate(&router, None, &entity_path, policy_id.as_deref()).await;

            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403 for a standalone account, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected the error code AccessDenied, got {:?}: {body}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                not_the_management_account(organization_of(&entity_path)),
                "case {case:?}: the refusal must state the requirement, naming the \
                 organization the request asked about"
            );
            assert!(
                held_jobs(&engine).is_empty(),
                "case {case:?}: a refused call must record no job, got {:?}",
                held_jobs(&engine)
            );
        }
    }
}

/// What a row expects of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the caller manages the organization the path names.
    Served,
    /// HTTP 403 `AccessDenied`: it does not.
    Refused,
}

/// One row of [`management_account_rows`]: which organization the account is in, which
/// organization the entity path names, and the answer the rule requires.
struct OrganizationCase {
    case: &'static str,
    /// The organization the store records, as `(organization id, management account id)`,
    /// or `None` for an account that is in no organization at all.
    organization: Option<(String, String)>,
    entity_path: String,
    expect: Expect,
}

/// The organizations the account may be in, against the organization an entity path names.
fn management_account_rows() -> Vec<OrganizationCase> {
    let path = format!("{ORGANIZATION}/r-f6g7h8i9j0example");
    vec![
        OrganizationCase {
            case: "an account in no organization",
            organization: None,
            entity_path: path.clone(),
            expect: Expect::Refused,
        },
        OrganizationCase {
            case: "a member account of the organization the path names",
            organization: Some((ORGANIZATION.to_string(), ANOTHER_ACCOUNT.to_string())),
            entity_path: path.clone(),
            expect: Expect::Refused,
        },
        OrganizationCase {
            case: "the management account of ANOTHER organization",
            organization: Some((OTHER_ORGANIZATION.to_string(), account_id().to_string())),
            entity_path: path.clone(),
            expect: Expect::Refused,
        },
        OrganizationCase {
            case: "the management account of the organization the path names",
            organization: Some((ORGANIZATION.to_string(), account_id().to_string())),
            entity_path: path,
            expect: Expect::Served,
        },
        OrganizationCase {
            case: "the management account, asked about an account under its own root",
            organization: Some((OTHER_ORGANIZATION.to_string(), account_id().to_string())),
            entity_path: format!("{OTHER_ORGANIZATION}/r-f6g7h8i9j0example/123456789012"),
            expect: Expect::Served,
        },
    ]
}

/// RULE: whether the report is generated is decided by whether the CALLER's account manages
/// the organization the entity path NAMES — not by whether it manages some organization, and
/// not by whether the path is well formed.
#[tokio::test]
async fn only_the_management_account_of_the_organization_the_path_names_is_served() {
    for row in management_account_rows() {
        let OrganizationCase {
            case,
            organization,
            entity_path,
            expect,
        } = row;
        let (router, engine) = router_with_engine();
        if let Some((organization_id, management_account_id)) = organization.as_ref() {
            record_organization(&engine, organization_id, management_account_id);
        }

        let (status, body) = generate(&router, None, &entity_path, None).await;

        match expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the generation to be served, got {status}: {body}"
                );
                let job_id = element(&body, "JobId");
                assert_eq!(
                    job_id.len(),
                    36,
                    "case {case:?}: expected the 36-character job id `jobIDType` bounds, \
                     got {job_id:?}"
                );
                let held = held_jobs(&engine);
                assert_eq!(
                    held.len(),
                    1,
                    "case {case:?}: expected exactly the one job the call recorded, got {held:?}"
                );
            }
            Expect::Refused => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {:?}: {body}",
                    error_code(&body)
                );
                assert_eq!(
                    element(&body, "Message"),
                    not_the_management_account(organization_of(&entity_path)),
                    "case {case:?}: the refusal must state the requirement, naming the \
                     organization the request asked about"
                );
                assert!(
                    held_jobs(&engine).is_empty(),
                    "case {case:?}: a refused call must record no job, got {:?}",
                    held_jobs(&engine)
                );
            }
        }
    }
}

/// RULE: a served call records the job its `JobId` names — what the report was asked FOR —
/// and that id is the handle `GetOrganizationsAccessReport` reports that job by.
#[tokio::test]
async fn a_served_call_records_the_job_its_job_id_names() {
    for (label, entity_path) in entity_paths() {
        for (policy_label, policy_id) in policy_ids() {
            // A fresh account per row: the concurrency quota is about jobs running at once,
            // and this test is about what ONE served call records.
            let (router, engine) = router_with_engine();
            record_organization(&engine, organization_of(&entity_path), account_id());
            let case = format!("{label} / {policy_label}");

            let (status, body) = generate(&router, None, &entity_path, policy_id.as_deref()).await;

            assert_eq!(
                status, 200,
                "case {case:?}: expected the generation to be served, got {status}: {body}"
            );
            let job_id = element(&body, "JobId");

            // The account holds exactly the job that was asked for, under the id the caller
            // was handed.
            let held = held_jobs(&engine);
            let (_, held_path, held_policy) = held
                .iter()
                .find(|(held_id, _, _)| held_id == &job_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the answered JobId {job_id:?} names no recorded job")
                });
            assert_eq!(
                held_path, &entity_path,
                "case {case:?}: the job must record the entity path the request carried"
            );
            assert_eq!(
                held_policy, &policy_id,
                "case {case:?}: the job must record the Organizations policy the request \
                 named, and none when it named none"
            );

            // And the read addressed by that id reports THAT job: in progress, because the
            // generation the clock owes is not due yet.
            let (status, body) =
                call_signed_by(&router, None, "GetOrganizationsAccessReport", &[("JobId", &job_id)])
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read of the job must be served, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "JobStatus"),
                JOB_IN_PROGRESS,
                "case {case:?}: expected the job the generate started to be in flight, got {body}"
            );
            assert!(
                !elements(&body, "JobCreationDate").is_empty(),
                "case {case:?}: expected the creation instant on the job, got {body}"
            );
        }
    }
}

/// One row of [`policy_rows`]: what the signing user's documents say about this action, and
/// whether the call reaches the organization at all.
struct PolicyCase {
    case: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs, each put on the caller with `PutUserPolicy`.
    policies: Vec<(String, String)>,
    expect: Expect,
}

/// The identity-policy rows.
fn policy_rows() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "an Allow naming the action",
            policies: vec![("allow".to_string(), statement("Allow", ACTION, "*"))],
            expect: Expect::Served,
        },
        PolicyCase {
            case: "an Allow whose action element is a wildcard run",
            policies: vec![(
                "allow-generate".to_string(),
                statement("Allow", "iam:Generate*", "*"),
            )],
            expect: Expect::Served,
        },
        PolicyCase {
            case: "no policy at all",
            policies: Vec::new(),
            expect: Expect::Refused,
        },
        PolicyCase {
            case: "an Allow and an explicit Deny",
            policies: vec![
                ("allow".to_string(), statement("Allow", ACTION, "*")),
                ("deny".to_string(), statement("Deny", ACTION, "*")),
            ],
            expect: Expect::Refused,
        },
    ]
}

/// RULE: IAM authorizes its own API, so the signing principal's identity-based policies
/// decide this call — and they decide it BEFORE the organization is consulted, with the
/// refusal AWS words for an authorization decision rather than the one the management
/// account requirement carries.
#[tokio::test]
async fn identity_policies_decide_the_call_before_the_organization_is_consulted() {
    let entity_path = format!("{ORGANIZATION}/r-f6g7h8i9j0example");

    for (row, policy_case) in policy_rows().into_iter().enumerate() {
        let PolicyCase {
            case,
            policies,
            expect,
        } = policy_case;
        let (router, engine) = router_with_engine();
        record_organization(&engine, ORGANIZATION, account_id());
        let user_name = format!("reporter{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;

        let (status, body) = generate(&router, Some(&access_key_id), &entity_path, None).await;

        match expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the generation to be served, got {status}: {body}"
                );
                assert_eq!(
                    held_jobs(&engine).len(),
                    1,
                    "case {case:?}: the served call must record its job, got {:?}",
                    held_jobs(&engine).len()
                );
            }
            Expect::Refused => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {:?}: {body}",
                    error_code(&body)
                );
                let reason = if policies.is_empty() {
                    format!("because no identity-based policy allows the {ACTION} action")
                } else {
                    "with an explicit deny in an identity-based policy".to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    ACTION,
                    &entity_path,
                    &reason,
                );
                assert!(
                    held_jobs(&engine).is_empty(),
                    "case {case:?}: a refused call must record no job, got {:?}",
                    held_jobs(&engine).len()
                );
            }
        }
    }
}
