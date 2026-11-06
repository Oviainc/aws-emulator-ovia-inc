// SPDX-License-Identifier: CC0-1.0

//! Service-linked role DELETION, end to end over the awsQuery wire.
//!
//! `DeleteServiceLinkedRole` submits a deletion for the role a service links to
//! itself and answers the `DeletionTaskId` the deletion is polled through;
//! `GetServiceLinkedRoleDeletionStatus` reports that task. The awsQuery harness
//! (router, call, element) is the one `tests/all/user_lifecycle.rs` owns, so every
//! suite in this crate builds and reads a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! whether AWS publishes a role name for the service principal, the principal's
//! label count and case, whether a `CustomSuffix` is supplied, the case the role
//! name is spelled in on the submit, and — for the refusals — what kind of name or
//! identifier the request carries.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// How long a test waits for a submitted deletion to become due: the span the
/// service holds it in flight for, plus a second so the instant has passed.
fn deletion_span() -> ::std::time::Duration {
    ::std::time::Duration::from_secs(state::SERVICE_LINKED_ROLE_DELETION_SECONDS as u64 + 1)
}

/// One deletion: the service principal the role is created for and the
/// `CustomSuffix` the create carries, if any.
///
/// The role NAME is read back from the create rather than written out here — what
/// name a principal and suffix derive is the subject of
/// `tests/all/service_linked_roles.rs`, and this suite is about what deleting that
/// role does.
struct Case {
    service_principal: &'static str,
    custom_suffix: Option<&'static str>,
}

/// Rows spanning principals whose role name AWS publishes and principals it does
/// not, two-, three- and five-label principals, a principal spelled with a leading
/// capital, and both presence and absence of a `CustomSuffix`.
const CASES: &[Case] = &[
    Case {
        service_principal: "elasticbeanstalk.amazonaws.com",
        custom_suffix: None,
    },
    Case {
        service_principal: "autoscaling.amazonaws.com",
        custom_suffix: Some("debug"),
    },
    Case {
        service_principal: "ops.apigateway.amazonaws.com",
        custom_suffix: Some("1"),
    },
    Case {
        service_principal: "networkmonitor.amazonaws.com",
        custom_suffix: None,
    },
    Case {
        service_principal: "Mediaconvert.amazonaws.com",
        custom_suffix: Some("Fleet_7"),
    },
    Case {
        service_principal: "a.b.c.d.amazonaws.com",
        custom_suffix: None,
    },
];

/// The trust policy a plain (not service-linked) fixture role is created with.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The whole `NoSuchEntity` message IAM words a submit for a role it holds no
/// service-linked role under with: one fixed sentence about the role it was asked to
/// delete, naming nothing of the request, so every refused row reads it identically.
const NO_SERVICE_ROLE_TO_DELETE: &str = "Cannot find the service role to delete.";

/// Create one row's service-linked role, answering the name the service gave it.
async fn create(router: &smithy_server_runtime::MultiServiceRouter, case: &Case) -> String {
    let mut params = vec![("AWSServiceName", case.service_principal)];
    if let Some(suffix) = case.custom_suffix {
        params.push(("CustomSuffix", suffix));
    }
    let (status, body) = call(router, "CreateServiceLinkedRole", &params).await;
    let label = case.service_principal;
    assert_eq!(
        status, 200,
        "case {label:?}: the fixture create must succeed, got {status}: {body}"
    );
    element(&body, "RoleName")
}

/// Submit a deletion for `role_name`, asserting it is accepted, and answer the
/// `DeletionTaskId`.
async fn submit(
    router: &smithy_server_runtime::MultiServiceRouter,
    label: &str,
    role_name: &str,
) -> String {
    let (status, body) = call(
        router,
        "DeleteServiceLinkedRole",
        &[("RoleName", role_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the deletion to be submitted, got {status}: {body}"
    );
    element(&body, "DeletionTaskId")
}

/// The `Status` the read reports for `deletion_task_id`.
async fn status_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    label: &str,
    deletion_task_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "GetServiceLinkedRoleDeletionStatus",
        &[("DeletionTaskId", deletion_task_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the deletion status to be reported, got {status}: {body}"
    );
    element(&body, "Status")
}

/// The role names the account holds, as `ListRoles` reports them.
async fn listed_role_names(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListRoles", &[]).await;
    assert_eq!(status, 200, "ListRoles must succeed, got {status}: {body}");
    elements(&body, "RoleName")
}

/// Assert `deletion_task_id` is spelled as `DeleteServiceLinkedRole$DeletionTaskId`
/// documents: "task/aws-service-role/<service-principal-name>/<role-name>/<task-uuid>".
fn assert_task_id_names_the_role(
    label: &str,
    deletion_task_id: &str,
    service_principal: &str,
    role_name: &str,
) {
    // `DeletionTaskIdType` is len 1..1000
    // (`smithy_query(query="shape", shape="DeletionTaskIdType")`).
    assert!(
        (1..=1000).contains(&deletion_task_id.chars().count()),
        "case {label:?}: expected a DeletionTaskId of 1..1000 characters, got \
         {deletion_task_id:?}"
    );
    let expected_head = format!("task/aws-service-role/{service_principal}/{role_name}/");
    assert!(
        deletion_task_id.starts_with(&expected_head),
        "case {label:?}: expected the DeletionTaskId to begin {expected_head:?}, got \
         {deletion_task_id:?}"
    );
    let uuid = &deletion_task_id[expected_head.len()..];
    assert!(
        state::is_uuid(uuid),
        "case {label:?}: expected the DeletionTaskId to end in a task uuid, got {uuid:?}"
    );
}

/// RULE: `DeleteServiceLinkedRole` SUBMITS a deletion of the service-linked role the
/// request names — it records a task identified by
/// `task/aws-service-role/<service-principal-name>/<role-name>/<task-uuid>`, removes
/// the role from the account, and reports that task `IN_PROGRESS` until the deletion
/// the clock owes is due and `SUCCEEDED` from then on.
#[tokio::test]
async fn a_submitted_deletion_names_its_role_removes_it_and_reports_its_progress() {
    let router = router();
    let mut submitted: Vec<(&str, String, String)> = Vec::new();

    for case in CASES {
        let label = case.service_principal;
        let role_name = create(&router, case).await;
        assert!(
            listed_role_names(&router).await.contains(&role_name),
            "case {label:?}: the fixture role must be held before the deletion, got {:?}",
            listed_role_names(&router).await
        );

        let deletion_task_id = submit(&router, label, &role_name).await;
        assert_task_id_names_the_role(label, &deletion_task_id, label, &role_name);
        assert!(
            !submitted
                .iter()
                .any(|(_, _, held): &(&str, String, String)| held == &deletion_task_id),
            "case {label:?}: every submit must mint its own task id, got {deletion_task_id:?} \
             twice"
        );

        // The account no longer holds the role: the submit is what deletes it.
        let held = listed_role_names(&router).await;
        assert!(
            !held.contains(&role_name),
            "case {label:?}: expected the role {role_name:?} to be gone, got {held:?}"
        );

        // The deletion is in flight the instant it is submitted: it is not due yet.
        let reported = status_of(&router, label, &deletion_task_id).await;
        assert_eq!(
            reported,
            state::DELETION_TASK_STATUS_IN_PROGRESS,
            "case {label:?}: expected the deletion to be in flight, got {reported:?}"
        );

        submitted.push((label, role_name, deletion_task_id));
    }

    tokio::time::sleep(deletion_span()).await;

    for (label, role_name, deletion_task_id) in &submitted {
        let reported = status_of(&router, label, deletion_task_id).await;
        assert_eq!(
            reported,
            state::DELETION_TASK_STATUS_SUCCEEDED,
            "case {label:?}: expected the deletion the clock owed to be done, got {reported:?}"
        );

        // The deletion is complete and the role is gone, so a further submit for that
        // name references no entity at all.
        let (status, body) = call(
            &router,
            "DeleteServiceLinkedRole",
            &[("RoleName", role_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected a submit for the deleted role to be 404, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {label:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: a role has ONE deletion in flight at a time — "if you call this operation
/// more than once for the same service-linked role and an earlier deletion task is
/// not complete, then the DeletionTaskId of the earlier request is returned" —
/// however the repeated request spells the role name, since an IAM entity name is not
/// distinguished by case. A role CREATED again under that name is a different role, so
/// its deletion is a task of its own.
#[tokio::test]
async fn a_repeated_submit_while_the_deletion_is_in_flight_answers_the_earlier_task_id() {
    let router = router();

    for case in CASES {
        let label = case.service_principal;
        let role_name = create(&router, case).await;
        let first = submit(&router, label, &role_name).await;

        // Rows: the name as the service gave it, lower-cased, and upper-cased — three
        // spellings of one role.
        for spelling in [
            role_name.clone(),
            role_name.to_ascii_lowercase(),
            role_name.to_ascii_uppercase(),
        ] {
            let repeat = submit(&router, label, &spelling).await;
            assert_eq!(
                repeat, first,
                "case {label:?}/{spelling:?}: expected the earlier task id {first:?}, got \
                 {repeat:?}"
            );
            let reported = status_of(&router, label, &repeat).await;
            assert_eq!(
                reported,
                state::DELETION_TASK_STATUS_IN_PROGRESS,
                "case {label:?}/{spelling:?}: expected the one task to still be in flight, \
                 got {reported:?}"
            );
        }

        // The same name, created again while that task is still in flight: a new role,
        // and its own deletion — so a new task id, and the new role is deleted too.
        let recreated = create(&router, case).await;
        assert_eq!(
            recreated, role_name,
            "case {label:?}: expected the second create to name the same role, got \
             {recreated:?}"
        );
        let second = submit(&router, label, &recreated).await;
        assert_ne!(
            second, first,
            "case {label:?}: expected the re-created role's deletion to be its own task, \
             got {second:?} twice"
        );
        let held = listed_role_names(&router).await;
        assert!(
            !held.contains(&recreated),
            "case {label:?}: expected the re-created role {recreated:?} to be gone, got \
             {held:?}"
        );
        for (which, deletion_task_id) in [("the first", &first), ("the second", &second)] {
            let reported = status_of(&router, label, deletion_task_id).await;
            assert_eq!(
                reported,
                state::DELETION_TASK_STATUS_IN_PROGRESS,
                "case {label:?}: expected {which} task to be in flight, got {reported:?}"
            );
        }
    }
}

/// RULE: this operation deletes a SERVICE-LINKED role of the account, so a name the
/// account holds no such role under — one it holds nothing under, one it holds a role
/// that is linked to no service under, or one only the owning service's own
/// provisioned role answers to — "referenced a resource entity that does not exist"
/// and is refused with `NoSuchEntity` (HTTP 404).
#[tokio::test]
async fn a_name_the_account_holds_no_service_linked_role_under_is_not_found() {
    let router = router();

    // A plain role of the account's, which is linked to no service.
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "plain-role"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture create must succeed, got {status}: {body}"
    );
    // A role of the account's filed at a path of its own, so the refusal does not turn
    // on the name alone.
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "AWSServiceRoleForNothing"),
            ("Path", "/eng/"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture create must succeed, got {status}: {body}"
    );

    // Rows: a name nothing was ever created under, a near-miss of one that was, the two
    // roles above (held, but linked to no service), a name AWS publishes for a
    // service-linked role the account never created (the provisioned role
    // `tests/all/service_provisioned_roles.rs` covers, which `GetRole` reports and this
    // submit does not delete), and a name at the longest length `roleNameType` admits.
    let longest = "r".repeat(state::MAX_ROLE_NAME_CHARACTERS);
    for absent in [
        "nobody",
        "plain-rol",
        "plain-role",
        "AWSServiceRoleForNothing",
        "AWSServiceRoleForECS",
        longest.as_str(),
    ] {
        let (status, body) = call(
            &router,
            "DeleteServiceLinkedRole",
            &[("RoleName", absent)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        // IAM words this refusal as one fixed sentence about the role it was asked to
        // delete, naming nothing of the request — so every row reads the same message.
        assert_eq!(
            element(&body, "Message"),
            NO_SERVICE_ROLE_TO_DELETE,
            "case {absent:?}: expected {NO_SERVICE_ROLE_TO_DELETE:?}, got {:?}",
            element(&body, "Message")
        );
    }

    // And neither refused submit deleted the role it named.
    let held = listed_role_names(&router).await;
    for name in ["plain-role", "AWSServiceRoleForNothing"] {
        assert!(
            held.contains(&name.to_string()),
            "case {name:?}: a refused submit deleted the role anyway, got {held:?}"
        );
    }
}

/// RULE: `GetServiceLinkedRoleDeletionStatus` reports the task the submit recorded
/// under the identifier it answered with, so a value outside the format that
/// identifier is returned in is refused as an invalid input (HTTP 400) and a
/// well-formed identifier the account holds no task under is `NoSuchEntity` (HTTP
/// 404).
#[tokio::test]
async fn a_task_id_outside_the_documented_format_is_invalid_and_an_unknown_one_is_not_found() {
    let router = router();

    // Rows: a bare word, the prefix alone, the prefix with too few segments, with too
    // many, with an empty principal, with an empty role name, a tail that is not a uuid,
    // an identifier of another kind the account mints, and one that only contains the
    // prefix.
    let invalid = [
        "nobody",
        "task/aws-service-role/",
        "task/aws-service-role/svc.amazonaws.com/AWSServiceRoleForSvc",
        "task/aws-service-role/svc.amazonaws.com/AWSServiceRoleForSvc/11111111-2222-3333-4444-555555555555/extra",
        "task/aws-service-role//AWSServiceRoleForSvc/11111111-2222-3333-4444-555555555555",
        "task/aws-service-role/svc.amazonaws.com//11111111-2222-3333-4444-555555555555",
        "task/aws-service-role/svc.amazonaws.com/AWSServiceRoleForSvc/not-a-uuid",
        "task/aws-service-role/svc.amazonaws.com/AWSServiceRoleForSvc/11111111-2222-3333-4444-55555555555",
        "11111111-2222-3333-4444-555555555555",
        "prefixed-task/aws-service-role/svc.amazonaws.com/AWSServiceRoleForSvc/11111111-2222-3333-4444-555555555555",
    ];
    for deletion_task_id in invalid {
        let (status, body) = call(
            &router,
            "GetServiceLinkedRoleDeletionStatus",
            &[("DeletionTaskId", deletion_task_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {deletion_task_id:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {deletion_task_id:?}: expected InvalidInput, got {:?}",
            error_code(&body)
        );
    }

    // Rows: a well-formed identifier for a role the account never held, and one that
    // differs from a task the account DOES hold only in its uuid.
    let role_name = create(&router, &CASES[0]).await;
    let held = submit(&router, CASES[0].service_principal, &role_name).await;
    let near_miss = {
        let (head, _) = held
            .rsplit_once('/')
            .expect("a DeletionTaskId carries a task uuid");
        format!("{head}/99999999-8888-7777-6666-555555555555")
    };
    for deletion_task_id in [
        "task/aws-service-role/svc.amazonaws.com/AWSServiceRoleForSvc/11111111-2222-3333-4444-555555555555"
            .to_string(),
        near_miss,
    ] {
        let (status, body) = call(
            &router,
            "GetServiceLinkedRoleDeletionStatus",
            &[("DeletionTaskId", deletion_task_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {deletion_task_id:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {deletion_task_id:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        // IAM's deletion-status surface words a task it holds nothing under with a
        // sentence that interpolates NOTHING, so both rows get the same text
        // (`crate::state::deletion_task_not_found`).
        let expected = "Cannot find deletion status for given id.";
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {deletion_task_id:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
    }

    // The task the account does hold is still reported, so no refused read disturbed it.
    let reported = status_of(&router, CASES[0].service_principal, &held).await;
    assert_eq!(
        reported,
        state::DELETION_TASK_STATUS_IN_PROGRESS,
        "expected the held task to still be in flight, got {reported:?}"
    );
}
