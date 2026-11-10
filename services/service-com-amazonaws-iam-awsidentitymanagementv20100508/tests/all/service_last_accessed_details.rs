// SPDX-License-Identifier: CC0-1.0

//! The service-last-accessed report family, end to end over the awsQuery wire.
//!
//! `GenerateServiceLastAccessedDetails` records a report JOB about the IAM resource its
//! `Arn` names and answers the id that job is read back by;
//! `GetServiceLastAccessedDetails` reports that job's status and, once the generation the
//! clock owes is due, one entry per service the resource's permissions policies allow an
//! action of; `GetServiceLastAccessedDetailsWithEntities` reports, for one such service,
//! the entities (users or roles) the resource's permissions reach. Every policy read here
//! was PERSISTED by an earlier call (`PutUserPolicy`, `PutGroupPolicy`, `PutRolePolicy`,
//! `CreatePolicy` + the attach calls, `AddUserToGroup`), so each assertion is about what a
//! client on the socket sees after a round trip through the store. The awsQuery harness
//! (router, call, element, elements, error_code) is the one `tests/all/user_lifecycle.rs`
//! owns and every suite in this crate shares.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the kind of
//! resource the report is about (user, group, role, managed policy), the form of the action
//! element the grant is written with, which policy carries the grant (inline, attached, or
//! held through a group), the level of detail asked for, the page size, and the way a value
//! fails to name a resource or a job.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{
    account_id, call, element, elements, error_code, router, router_with_engine,
};

/// A trust policy every role this suite creates needs. It is a RESOURCE-based policy — it
/// says who may assume the role, not what the role may do — so it puts no service in the
/// role's reach and must never be reported.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The namespace [`TRUST_POLICY`] would be reported under if it were ever read as a
/// permissions policy.
const TRUST_POLICY_NAMESPACE: &str = "sts";

/// How long a test waits for the generation the clock owes to become due.
fn generation_span() -> ::std::time::Duration {
    ::std::time::Duration::from_secs(state::ACCESS_REPORT_GENERATION_SECONDS as u64 + 1)
}

/// Create a user, answering the ARN the account holds it under.
async fn create_user(router: &MultiServiceRouter, user_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a role, answering the ARN the account holds it under.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a group, answering the ARN the account holds it under.
async fn create_group(router: &MultiServiceRouter, group_name: &str) -> String {
    let (status, body) = call(router, "CreateGroup", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateGroup must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Embed `document` in an entity under `policy_name`, through the put that owns the
/// entity's kind.
async fn put_policy(
    router: &MultiServiceRouter,
    action: &str,
    name_parameter: &str,
    entity_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        action,
        &[
            (name_parameter, entity_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: {action} must succeed, got {status}: {body}"
    );
}

/// Create a managed policy carrying `document`, answering its ARN.
async fn create_managed_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    document: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[("PolicyName", policy_name), ("PolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// One call that must succeed, answering its body.
async fn expect_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status}: {body}"
    );
    body
}

/// Ask for a report about `arn`, at `granularity` when the row names one, answering the
/// `JobId` it was recorded under.
async fn generate(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    granularity: Option<&str>,
) -> String {
    let mut params = vec![("Arn", arn)];
    if let Some(granularity) = granularity {
        params.push(("Granularity", granularity));
    }
    let body = expect_ok(
        router,
        case,
        "GenerateServiceLastAccessedDetails",
        &params,
    )
    .await;
    let job_id = element(&body, "JobId");
    // `jobIDType` is exactly 36 characters.
    assert_eq!(
        job_id.len(),
        36,
        "case {case:?}: expected the 36-character job id `jobIDType` bounds, got {job_id:?}"
    );
    job_id
}

/// Read the report `job_id` names, with any extra parameters the row supplies.
async fn details(
    router: &MultiServiceRouter,
    case: &str,
    job_id: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params = vec![("JobId", job_id)];
    params.extend_from_slice(extra);
    expect_ok(router, case, "GetServiceLastAccessedDetails", &params).await
}

/// Read the entities the report `job_id` names reports for `service_namespace`.
async fn entities(
    router: &MultiServiceRouter,
    case: &str,
    job_id: &str,
    service_namespace: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params = vec![("JobId", job_id), ("ServiceNamespace", service_namespace)];
    params.extend_from_slice(extra);
    expect_ok(
        router,
        case,
        "GetServiceLastAccessedDetailsWithEntities",
        &params,
    )
    .await
}

/// The service names the catalog publishes for `namespaces`, in the same order.
fn service_names(namespaces: &[String]) -> Vec<String> {
    namespaces
        .iter()
        .map(|namespace| {
            state::reported_service(namespace)
                .unwrap_or_else(|| panic!("{namespace} is a service a report covers"))
                .service_name
                .to_string()
        })
        .collect()
}

/// What one row of the services table expects a completed report to list.
enum Expected {
    /// Exactly these namespaces, in this order.
    Only(&'static [&'static str]),
    /// Every service a report covers: the row's grant reaches all of them.
    Every,
    /// Every service a report covers except these.
    EveryExcept(&'static [&'static str]),
}

impl Expected {
    /// The namespaces the row expects, ascending — the order the report is sorted in.
    fn namespaces(&self) -> Vec<String> {
        let every = || {
            state::REPORTED_SERVICES
                .iter()
                .map(|service| service.service_namespace.to_string())
        };
        match self {
            Self::Only(listed) => listed.iter().map(ToString::to_string).collect(),
            Self::Every => every().collect(),
            Self::EveryExcept(excluded) => every()
                .filter(|namespace| !excluded.contains(&namespace.as_str()))
                .collect(),
        }
    }
}

/// One row of the entity table: which report, which service it is asked about, and the
/// entities that service is in reach of through the report's resource, each as the name and
/// the ARN the account holds it under.
struct EntityRow<'a> {
    what: &'a str,
    job_id: &'a str,
    service_namespace: &'a str,
    expected: Vec<(&'a str, &'a String)>,
}

/// Whether `arn` names an IAM user rather than a role — the ARN resource type IAM spells a
/// user with.
fn is_user_arn(arn: &str) -> bool {
    arn.contains(":user/")
}

/// RULE: a report is a JOB the account records about the resource the request's `Arn`
/// names — the generate mints its id and the read reports THAT job, moving from
/// `IN_PROGRESS` to `COMPLETED` when the generation the clock owes becomes due, and
/// serving the report only from then on — whichever of the four kinds of resource a report
/// is generated for the ARN names.
#[tokio::test]
async fn a_report_is_recorded_per_resource_and_served_once_the_clock_completes_it() {
    let router = router();
    let case = "the report fixtures";

    // The grant each fixture carries, written so the services it reaches are known: one
    // namespace per fixture, and `sts` never — the roles' trust policy is not a
    // permissions policy.
    let user_arn = create_user(&router, "report-user", "/eng/").await;
    put_policy(
        &router,
        "PutUserPolicy",
        "UserName",
        "report-user",
        "reads-objects",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    )
    .await;
    let role_arn = create_role(&router, "report-role").await;
    put_policy(
        &router,
        "PutRolePolicy",
        "RoleName",
        "report-role",
        "reads-tables",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:Query"],"Resource":"*"}]}"#,
    )
    .await;
    let group_arn = create_group(&router, "report-group").await;
    let group_policy_arn = create_managed_policy(
        &router,
        "group-queues",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sqs:*","Resource":"*"}]}"#,
    )
    .await;
    expect_ok(
        &router,
        case,
        "AttachGroupPolicy",
        &[("GroupName", "report-group"), ("PolicyArn", &group_policy_arn)],
    )
    .await;
    // A user that reaches a service only through the group it belongs to: an identity's
    // own permissions include the policies in force on its groups.
    let member_arn = create_user(&router, "report-member", "/").await;
    expect_ok(
        &router,
        case,
        "AddUserToGroup",
        &[("GroupName", "report-group"), ("UserName", "report-member")],
    )
    .await;

    // The rows: the four kinds of resource a report is generated for, plus the inherited
    // grant — each with the services its permissions policies allow an action of.
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        ("a user's own inline policy", user_arn, vec!["s3"]),
        ("a role's own inline policy", role_arn, vec!["dynamodb"]),
        (
            "a group's attached managed policy",
            group_arn,
            vec!["sqs"],
        ),
        (
            "the managed policy itself",
            group_policy_arn.clone(),
            vec!["sqs"],
        ),
        (
            "a user that holds the grant through its group",
            member_arn,
            vec!["sqs"],
        ),
    ];

    let mut jobs = Vec::new();
    for (what, arn, expected) in &rows {
        let job_id = generate(&router, what, arn, None).await;
        assert!(
            !jobs.iter().any(|(_, held, _): &(&str, String, _)| held == &job_id),
            "case {what:?}: every generate must mint its own job id, got {job_id:?} twice"
        );

        // The job is in flight the instant it is asked for: the generation is not due yet,
        // so the read reports it IN_PROGRESS and serves no report.
        let body = details(&router, what, &job_id, &[]).await;
        assert_eq!(
            element(&body, "JobStatus"),
            state::JOB_STATUS_IN_PROGRESS,
            "case {what:?}: expected the job to be in flight, got {body}"
        );
        assert!(
            elements(&body, "ServiceNamespace").is_empty(),
            "case {what:?}: a report in progress serves no service, got {:?}",
            elements(&body, "ServiceNamespace")
        );
        assert!(
            !elements(&body, "JobCreationDate").is_empty(),
            "case {what:?}: expected the creation instant, got {body}"
        );
        jobs.push((what.to_owned(), job_id, expected.clone()));
    }

    tokio::time::sleep(generation_span()).await;

    for (what, job_id, expected) in &jobs {
        let body = details(&router, what, job_id, &[]).await;
        assert_eq!(
            element(&body, "JobStatus"),
            state::JOB_STATUS_COMPLETED,
            "case {what:?}: expected the generation the clock owed to be done, got {body}"
        );
        assert!(
            !elements(&body, "JobCompletionDate").is_empty(),
            "case {what:?}: expected the completion instant, got {body}"
        );
        // Exactly the services that resource's permissions policies allow an action of —
        // and never the one its trust policy names.
        assert_eq!(
            elements(&body, "ServiceNamespace"),
            expected.iter().map(ToString::to_string).collect::<Vec<String>>(),
            "case {what:?}: expected the services the resource's policies reach, got {:?}",
            elements(&body, "ServiceNamespace")
        );
        assert!(
            !elements(&body, "ServiceNamespace").contains(&TRUST_POLICY_NAMESPACE.to_string()),
            "case {what:?}: a trust policy is not a permissions policy, got {body}"
        );
        // Each entry carries the service's published name beside its namespace.
        assert_eq!(
            elements(&body, "ServiceName"),
            service_names(&elements(&body, "ServiceNamespace")),
            "case {what:?}: expected each service's published name, got {:?}",
            elements(&body, "ServiceName")
        );
        // Nothing in this emulator records a principal reaching a service, so no INSTANT
        // of an attempt is reported for one.
        for withheld in [
            "LastAuthenticated",
            "LastAuthenticatedEntity",
            "LastAuthenticatedRegion",
        ] {
            assert!(
                elements(&body, withheld).is_empty(),
                "case {what:?}: <{withheld}> is reported only for a recorded attempt, got {:?}",
                elements(&body, withheld)
            );
        }
        // The number of principals that have attempted to reach the service is a COUNT,
        // which every entry of a report carries: it is zero for a service nothing has
        // recorded an attempt against, never absent.
        assert_eq!(
            elements(&body, "TotalAuthenticatedEntities"),
            vec!["0".to_string(); elements(&body, "ServiceNamespace").len()],
            "case {what:?}: expected one attempt count of zero per reported service, got {:?}",
            elements(&body, "TotalAuthenticatedEntities")
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {what:?}: expected the whole report in one page, got {body}"
        );

        // A read changes nothing a later caller sees: the same read answers the same
        // report.
        let again = details(&router, what, job_id, &[]).await;
        assert_eq!(
            elements(&again, "ServiceNamespace"),
            elements(&body, "ServiceNamespace"),
            "case {what:?}: a read must not change what a later caller sees, got {again}"
        );
        assert_eq!(
            element(&again, "JobCreationDate"),
            element(&body, "JobCreationDate"),
            "case {what:?}: the creation instant is stored, not read from the clock"
        );
    }

    // The user's report names the service its own grant reaches by the name the catalog
    // publishes for it — the Service Authorization Reference's name for the prefix `s3`.
    let body = details(&router, case, &jobs[0].1, &[]).await;
    assert_eq!(
        element(&body, "ServiceName"),
        "Amazon Simple Storage Service",
        "expected the published name of the service behind the `s3` prefix, got {body}"
    );
}

/// RULE: a completed report lists one entry per service whose namespace one of the
/// resource's permissions policies ALLOWS an action of, whatever form that policy writes
/// its action element in, and nothing for a service none of them allows an action of.
#[tokio::test]
async fn the_report_lists_every_service_the_resources_policies_allow_an_action_of() {
    let router = router();

    // Rows spanning the forms of an action element the IAM grammar admits, the effect, and
    // a namespace outside the services a report covers.
    let rows: &[(&str, &str, Expected)] = &[
        (
            "one literal action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
            Expected::Only(&["s3"]),
        ),
        (
            "a list of actions of two services",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:PutLogEvents","dynamodb:Query"],"Resource":"*"}]}"#,
            Expected::Only(&["dynamodb", "logs"]),
        ),
        (
            "a wildcard run in the action name",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:Describe*","Resource":"*"}]}"#,
            Expected::Only(&["ec2"]),
        ),
        (
            "a namespace spelled in another case",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"KMS:Decrypt","Resource":"*"}]}"#,
            Expected::Only(&["kms"]),
        ),
        (
            "a wildcard inside the namespace",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3*:Get*","Resource":"*"}]}"#,
            // Every namespace the catalog carries that the run `s3*` reaches.
            Expected::Only(&["s3", "s3-object-lambda", "s3express", "s3outposts", "s3tables"]),
        ),
        (
            "the whole-element wildcard",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
            Expected::Every,
        ),
        (
            "a single statement rather than a list",
            r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Action":"sns:Publish","Resource":"*"}}"#,
            Expected::Only(&["sns"]),
        ),
        (
            "two statements, each reaching one service",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"lambda:InvokeFunction","Resource":"*"},{"Effect":"Allow","Action":"states:StartExecution","Resource":"*"}]}"#,
            Expected::Only(&["lambda", "states"]),
        ),
        (
            "a refusal rather than a grant",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"s3:*","Resource":"*"}]}"#,
            Expected::Only(&[]),
        ),
        (
            "every action but one service's",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":"iam:*","Resource":"*"}]}"#,
            Expected::EveryExcept(&["iam"]),
        ),
        (
            "a service outside the ones a report covers",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"nosuchservicehere:ListModels","Resource":"*"}]}"#,
            Expected::Only(&[]),
        ),
    ];

    let mut jobs = Vec::new();
    for (index, (what, document, expected)) in rows.iter().enumerate() {
        let user_name = format!("grant-user-{index}");
        let arn = create_user(&router, &user_name, "/").await;
        put_policy(
            &router,
            "PutUserPolicy",
            "UserName",
            &user_name,
            "granted",
            document,
        )
        .await;
        jobs.push((
            *what,
            generate(&router, what, &arn, None).await,
            expected.namespaces(),
        ));
    }

    tokio::time::sleep(generation_span()).await;

    for (what, job_id, expected) in &jobs {
        // Every service the row's grant reaches has to fit one page, so the whole report
        // is read in one call.
        let body = details(&router, what, job_id, &[("MaxItems", "1000")]).await;
        assert_eq!(
            elements(&body, "ServiceNamespace"),
            *expected,
            "case {what:?}: expected the services the grant reaches, got {:?}",
            elements(&body, "ServiceNamespace")
        );
        assert_eq!(
            elements(&body, "ServiceName"),
            service_names(expected),
            "case {what:?}: expected each service's published name, got {:?}",
            elements(&body, "ServiceName")
        );
    }
}

/// RULE: the services a report covers are a catalog keyed by service namespace, so no
/// namespace is reported twice and the catalog's order IS the namespace order the report is
/// sorted in.
#[test]
fn the_services_a_report_covers_are_a_sorted_catalog_of_distinct_namespaces() {
    let namespaces: Vec<&str> = state::REPORTED_SERVICES
        .iter()
        .map(|service| service.service_namespace)
        .collect();
    let mut sorted = namespaces.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        namespaces, sorted,
        "the catalog must be ascending by namespace and hold each namespace once, got \
         {namespaces:?}"
    );
    for service in state::REPORTED_SERVICES {
        assert!(
            !service.service_name.is_empty(),
            "case {:?}: every service a report covers carries its published name",
            service.service_namespace
        );
        // The lookup folds case, because an action element may spell a prefix in any.
        assert_eq!(
            state::reported_service(&service.service_namespace.to_ascii_uppercase())
                .map(|found| found.service_namespace),
            Some(service.service_namespace),
            "case {:?}: the lookup must not be case-sensitive",
            service.service_namespace
        );
    }
    // Two entries written out, so the names the catalog publishes are asserted rather
    // than echoed. Source: the Service Authorization Reference, which files the prefix
    // `s3` under "Amazon Simple Storage Service" and `iam` under "AWS Identity and Access
    // Management".
    for (namespace, service_name) in [
        ("s3", "Amazon Simple Storage Service"),
        ("iam", "AWS Identity and Access Management"),
    ] {
        assert_eq!(
            state::reported_service(namespace).map(|found| found.service_name),
            Some(service_name),
            "case {namespace:?}: expected the published name of that service"
        );
    }
    // And a prefix the catalog does not carry names no service a report covers.
    assert!(
        state::reported_service("nosuchservicehere").is_none(),
        "a namespace outside the catalog names no service a report covers"
    );
    // The catalog is the published breadth of AWS service prefixes rather than a
    // shortlist: a report of it covers hundreds of services, which is what a
    // least-privilege review of a wide grant reads.
    assert!(
        state::REPORTED_SERVICES.len() > 100,
        "the services a report covers must be the published catalog, not a shortlist, got \
         {}",
        state::REPORTED_SERVICES.len()
    );
}

/// RULE: the `JobType` a report reports is the level of detail the generate asked for — and
/// an omitted `Granularity` asks for service data — while the tracked actions within a
/// service are reported only for an attempt something recorded, so neither level reports
/// one here.
#[tokio::test]
async fn the_job_type_is_the_level_of_detail_the_generate_asked_for() {
    let router = router();
    let arn = create_user(&router, "granularity-user", "/").await;
    put_policy(
        &router,
        "PutUserPolicy",
        "UserName",
        "granularity-user",
        "reads-objects",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:Get*","Resource":"*"}]}"#,
    )
    .await;

    // The rows: no `Granularity` at all, and each value the enum declares.
    let rows: &[(&str, Option<&str>, &str)] = &[
        (
            "no granularity asked for",
            None,
            state::USAGE_GRANULARITY_SERVICE_LEVEL,
        ),
        (
            "service data",
            Some(state::USAGE_GRANULARITY_SERVICE_LEVEL),
            state::USAGE_GRANULARITY_SERVICE_LEVEL,
        ),
        (
            "service and action data",
            Some(state::USAGE_GRANULARITY_ACTION_LEVEL),
            state::USAGE_GRANULARITY_ACTION_LEVEL,
        ),
    ];

    let mut jobs = Vec::new();
    for (what, granularity, expected) in rows {
        jobs.push((
            *what,
            generate(&router, what, &arn, *granularity).await,
            *expected,
        ));
    }

    tokio::time::sleep(generation_span()).await;

    for (what, job_id, expected) in &jobs {
        let body = details(&router, what, job_id, &[]).await;
        assert_eq!(
            element(&body, "JobType"),
            *expected,
            "case {what:?}: expected the level of detail the generate asked for, got {body}"
        );
        assert_eq!(
            elements(&body, "ServiceNamespace"),
            vec!["s3".to_string()],
            "case {what:?}: the level of detail does not change which services are \
             reported, got {:?}",
            elements(&body, "ServiceNamespace")
        );
        assert!(
            elements(&body, "ActionName").is_empty(),
            "case {what:?}: a tracked action is reported only for a recorded attempt, got \
             {:?}",
            elements(&body, "ActionName")
        );
    }
}

/// RULE: the entities a report reports for one service are the users and roles the
/// report's resource puts that service in reach of — its members for a group, the
/// identities it is in force on for a managed policy, and itself for a user or a role — and
/// none at all for a service the resource's policies allow no action of.
#[tokio::test]
async fn the_entities_reported_are_the_ones_the_reports_resource_reaches() {
    let router = router();
    let case = "the entity fixtures";

    // A group whose own policy reaches one service, holding two of the account's three
    // users.
    let group_arn = create_group(&router, "entity-group").await;
    put_policy(
        &router,
        "PutGroupPolicy",
        "GroupName",
        "entity-group",
        "reads-objects",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    )
    .await;
    let alice_arn = create_user(&router, "alice", "/eng/").await;
    let bob_arn = create_user(&router, "Bob", "/").await;
    let carol_arn = create_user(&router, "carol", "/sales/").await;
    for member in ["alice", "Bob"] {
        expect_ok(
            &router,
            case,
            "AddUserToGroup",
            &[("GroupName", "entity-group"), ("UserName", member)],
        )
        .await;
    }

    // A managed policy reaching another service, in force on a role, on a user directly,
    // and on a second group's member.
    let policy_arn = create_managed_policy(
        &router,
        "entity-policy",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:Query","Resource":"*"}]}"#,
    )
    .await;
    let role_arn = create_role(&router, "entity-role").await;
    expect_ok(
        &router,
        case,
        "AttachRolePolicy",
        &[("RoleName", "entity-role"), ("PolicyArn", &policy_arn)],
    )
    .await;
    expect_ok(
        &router,
        case,
        "AttachUserPolicy",
        &[("UserName", "carol"), ("PolicyArn", &policy_arn)],
    )
    .await;
    create_group(&router, "entity-second-group").await;
    expect_ok(
        &router,
        case,
        "AttachGroupPolicy",
        &[
            ("GroupName", "entity-second-group"),
            ("PolicyArn", &policy_arn),
        ],
    )
    .await;
    expect_ok(
        &router,
        case,
        "AddUserToGroup",
        &[("GroupName", "entity-second-group"), ("UserName", "Bob")],
    )
    .await;

    // A job is in flight when it is asked for, so the entities are served only once its
    // generation is due.
    let group_job = generate(&router, "the group's report", &group_arn, None).await;
    let body = entities(&router, "the group's report in flight", &group_job, "s3", &[]).await;
    assert_eq!(
        element(&body, "JobStatus"),
        state::JOB_STATUS_IN_PROGRESS,
        "expected the job to be in flight, got {body}"
    );
    assert!(
        elements(&body, "Arn").is_empty(),
        "a report in progress serves no entity, got {:?}",
        elements(&body, "Arn")
    );

    let policy_job = generate(&router, "the policy's report", &policy_arn, None).await;
    let user_job = generate(&router, "a user's report", &alice_arn, None).await;
    let role_job = generate(&router, "a role's report", &role_arn, None).await;

    tokio::time::sleep(generation_span()).await;

    // The rows: which resource the report is about, which service is asked about, and the
    // entities that service is in reach of through that resource.
    let rows: Vec<EntityRow> = vec![
        EntityRow {
            what: "a group's members use the group's policy",
            job_id: &group_job,
            service_namespace: "s3",
            expected: vec![("alice", &alice_arn), ("Bob", &bob_arn)],
        },
        EntityRow {
            what: "a service the group's policy allows no action of",
            job_id: &group_job,
            service_namespace: "dynamodb",
            expected: vec![],
        },
        EntityRow {
            what: "the identities a managed policy is in force on",
            job_id: &policy_job,
            service_namespace: "dynamodb",
            expected: vec![
                ("entity-role", &role_arn),
                ("Bob", &bob_arn),
                ("carol", &carol_arn),
            ],
        },
        EntityRow {
            what: "a user's own report is about that user",
            job_id: &user_job,
            service_namespace: "s3",
            expected: vec![("alice", &alice_arn)],
        },
        EntityRow {
            what: "a role's own report is about that role",
            job_id: &role_job,
            service_namespace: "dynamodb",
            expected: vec![("entity-role", &role_arn)],
        },
        EntityRow {
            what: "a service a role's policies allow no action of",
            job_id: &role_job,
            service_namespace: "s3",
            expected: vec![],
        },
    ];

    for row in rows {
        let what = row.what;
        let body = entities(
            &router,
            what,
            row.job_id,
            row.service_namespace,
            &[("MaxItems", "1000")],
        )
        .await;
        assert_eq!(
            element(&body, "JobStatus"),
            state::JOB_STATUS_COMPLETED,
            "case {what:?}: expected the generation the clock owed to be done, got {body}"
        );
        // Roles before users, each ascending by the case-folded name — one total order, so
        // a single `Marker` can page over the list.
        let mut expected = row.expected.clone();
        expected.sort_by_key(|(name, arn)| (is_user_arn(arn), name.to_ascii_lowercase()));
        let expected_names: Vec<String> =
            expected.iter().map(|(name, _)| (*name).to_string()).collect();
        let expected_arns: Vec<String> =
            expected.iter().map(|(_, arn)| (*arn).clone()).collect();
        assert_eq!(
            elements(&body, "Name"),
            expected_names,
            "case {what:?}: expected the entities the resource reaches, got {:?}",
            elements(&body, "Name")
        );
        assert_eq!(
            elements(&body, "Arn"),
            expected_arns,
            "case {what:?}: expected each entity's own ARN, got {:?}",
            elements(&body, "Arn")
        );
        // "The type of entity (user or role)", and the identifier and path the account
        // holds that entity under.
        assert_eq!(
            elements(&body, "Type"),
            expected_arns
                .iter()
                .map(|arn| if is_user_arn(arn) { "USER" } else { "ROLE" }.to_string())
                .collect::<Vec<String>>(),
            "case {what:?}: expected each entity's type, got {:?}",
            elements(&body, "Type")
        );
        for (identifier, arn) in elements(&body, "Id").iter().zip(&expected_arns) {
            // The prefixes the account mints an entity's own identifier with
            // (`User$UserId`, `Role$RoleId`), so what is asserted is the STORED id rather
            // than one this read invented.
            let prefix = if is_user_arn(arn) { "AIDA" } else { "AROA" };
            assert!(
                identifier.starts_with(prefix),
                "case {what:?}: expected the stored identifier of {arn}, got {identifier:?}"
            );
        }
        assert_eq!(
            elements(&body, "Path").len(),
            expected_arns.len(),
            "case {what:?}: expected one path per entity, got {:?}",
            elements(&body, "Path")
        );
        // Nothing records a principal reaching a service, so no entity reports an attempt.
        assert!(
            elements(&body, "LastAuthenticated").is_empty(),
            "case {what:?}: an access instant is reported only for a recorded attempt, got \
             {:?}",
            elements(&body, "LastAuthenticated")
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {what:?}: expected the whole list in one page, got {body}"
        );
    }

}

/// RULE: a read that names ONE service by its `ServiceNamespace` is served for a namespace
/// the report family covers a service under — with an empty list when that service is in
/// nobody's reach — and refused as an invalid input, naming the value it was given, for a
/// namespace that covers no service at all.
#[tokio::test]
async fn a_service_namespace_covering_no_service_is_refused_as_an_invalid_input() {
    let router = router();
    let case = "the namespace fixtures";
    let arn = create_user(&router, "namespace-user", "/").await;
    put_policy(
        &router,
        "PutUserPolicy",
        "UserName",
        "namespace-user",
        "reads-objects",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    )
    .await;
    let job_id = generate(&router, case, &arn, None).await;
    tokio::time::sleep(generation_span()).await;

    // SOURCE of which namespaces name a service at all: `state::REPORTED_SERVICES`, the
    // services a report covers — so the rows that must be SERVED are taken from it, the
    // first and the last entry included, rather than written out here.
    let first = state::REPORTED_SERVICES
        .first()
        .expect("a report covers at least one service")
        .service_namespace;
    let last = state::REPORTED_SERVICES
        .last()
        .expect("a report covers at least one service")
        .service_namespace;

    // Rows spanning the ways a namespace names a service the report covers: the one the
    // grant reaches, the same one spelled in another case (a service prefix is published in
    // lower case, and one namespace names one service however it is spelled), a covered
    // service the grant does not reach, and the two ends of the catalog.
    for (what, namespace, reaches) in [
        ("the service the grant reaches", "s3", true),
        ("that service spelled in another case", "S3", true),
        ("a covered service the grant does not reach", "dynamodb", false),
        ("the first service the catalog covers", first, false),
        ("the last service the catalog covers", last, false),
    ] {
        let body = entities(&router, what, &job_id, namespace, &[("MaxItems", "1000")]).await;
        let expected = if reaches {
            vec!["namespace-user".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            elements(&body, "Name"),
            expected,
            "case {what:?}: expected the entities that service is in reach of, got {:?}",
            elements(&body, "Name")
        );
    }

    // Rows spanning the ways a namespace covers no service: a value naming no service at
    // all, a published service prefix the report family covers nothing for, one that merely
    // carries a covered namespace as a prefix, and the two ends of `serviceNamespaceType`'s
    // own length bounds (1..64), so the refusal is the handler's rather than the boundary
    // validator's. Each clears that shape, so each reaches the handler.
    let longest = "n".repeat(64);
    for (what, namespace) in [
        ("a value naming no service at all", "not-a-real-aws-service-namespace"),
        ("a published prefix the report covers nothing for", "mechanicalturk"),
        ("a prefix that merely extends a covered one", "iotjobsdata"),
        ("the shortest namespace the shape admits", "x"),
        ("the longest namespace the shape admits", longest.as_str()),
    ] {
        let (status, body) = call(
            &router,
            "GetServiceLastAccessedDetailsWithEntities",
            &[("JobId", job_id.as_str()), ("ServiceNamespace", namespace)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("Service namepace {namespace} is either invalid or unsupported."),
            "case {what:?}: expected the refusal to name the namespace it was given, got {:?}",
            element(&body, "Message")
        );
    }
}

/// RULE: both reads page through their own list exactly once — at most `MaxItems` items per
/// call, each page resuming strictly after the item the previous `Marker` names — and the
/// pages together are the whole list in its one order.
#[tokio::test]
async fn a_report_pages_through_its_services_and_entities_exactly_once() {
    let router = router();
    let case = "the pagination fixtures";

    // A grant reaching five services, so a page size below five truncates.
    let policy_arn = create_managed_policy(
        &router,
        "paged-policy",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject","sqs:SendMessage","sns:Publish","kms:Decrypt","logs:PutLogEvents"],"Resource":"*"}]}"#,
    )
    .await;
    let every_service = vec![
        "kms".to_string(),
        "logs".to_string(),
        "s3".to_string(),
        "sns".to_string(),
        "sqs".to_string(),
    ];
    // Five users the policy is in force on, so the entity list pages too.
    let mut every_user: Vec<String> = Vec::new();
    for index in 0..5 {
        let user_name = format!("paged-user-{index}");
        create_user(&router, &user_name, "/").await;
        expect_ok(
            &router,
            case,
            "AttachUserPolicy",
            &[("UserName", &user_name), ("PolicyArn", &policy_arn)],
        )
        .await;
        every_user.push(user_name);
    }
    every_user.sort();

    let job_id = generate(&router, case, &policy_arn, None).await;
    tokio::time::sleep(generation_span()).await;

    // The rows: page sizes below, at and above the length of each list.
    for page_size in [1_usize, 2, 4, 5, 1000] {
        let what = format!("{page_size} services per page");
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut extra = vec![("MaxItems", size.as_str())];
            if let Some(marker) = &marker {
                extra.push(("Marker", marker.as_str()));
            }
            let body = details(&router, &what, &job_id, &extra).await;
            let page = elements(&body, "ServiceNamespace");
            assert!(
                page.len() <= page_size,
                "case {what:?}: a page may not exceed MaxItems, got {page:?}"
            );
            collected.extend(page);
            if element(&body, "IsTruncated") != "true" {
                assert!(
                    elements(&body, "Marker").is_empty(),
                    "case {what:?}: a page that is not truncated hands out no marker, got \
                     {body}"
                );
                break;
            }
            marker = Some(element(&body, "Marker"));
        }
        assert_eq!(
            collected, every_service,
            "case {what:?}: the pages together are the whole report, once each, in its \
             own order"
        );

        let what = format!("{page_size} entities per page");
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut extra = vec![("MaxItems", size.as_str())];
            if let Some(marker) = &marker {
                extra.push(("Marker", marker.as_str()));
            }
            let body = entities(&router, &what, &job_id, "s3", &extra).await;
            let page = elements(&body, "Name");
            assert!(
                page.len() <= page_size,
                "case {what:?}: a page may not exceed MaxItems, got {page:?}"
            );
            collected.extend(page);
            if element(&body, "IsTruncated") != "true" {
                break;
            }
            marker = Some(element(&body, "Marker"));
        }
        assert_eq!(
            collected, every_user,
            "case {what:?}: the pages together are the whole entity list, once each"
        );
    }

    // A marker this service did not issue for THIS list is refused as an invalid input,
    // rather than read as the start of it.
    let foreign = state::encode_marker(
        &state::service_last_accessed_marker_scope("another-job"),
        "s3",
    );
    for (what, action, params) in [
        (
            "a marker issued for another report",
            "GetServiceLastAccessedDetails",
            vec![("JobId", job_id.as_str()), ("Marker", foreign.as_str())],
        ),
        (
            "a marker nothing issued",
            "GetServiceLastAccessedDetails",
            vec![("JobId", job_id.as_str()), ("Marker", "not-a-marker")],
        ),
        (
            "a services marker handed to the entity list",
            "GetServiceLastAccessedDetailsWithEntities",
            vec![
                ("JobId", job_id.as_str()),
                ("ServiceNamespace", "s3"),
                ("Marker", foreign.as_str()),
            ],
        ),
    ] {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: a value that is not the ARN of one of the resources a report is generated for is
/// refused as an invalid input, an ARN naming no such resource of this account is the
/// modelled not-found, and so is a job id the account holds no report under — and a refused
/// generate records nothing.
#[tokio::test]
async fn an_arn_or_job_id_naming_nothing_is_refused() {
    // The engine is held beside the router so the suite can seed the report of ANOTHER
    // kind below, and read back that the refused generates recorded nothing.
    let (router, engine) = router_with_engine();
    let account = account_id();

    // Rows spanning the ways a value fails to be the ARN of a user, group, role or managed
    // policy: another service's resource, another IAM resource type, and a value that is
    // no ARN at all (past `arnType`'s 20-character minimum, so the boundary validator
    // passes it).
    for (what, arn) in [
        (
            "another service's resource",
            format!("arn:aws:sqs:us-east-1:{account}:a-queue"),
        ),
        (
            "an IAM resource of another type",
            format!("arn:aws:iam::{account}:instance-profile/a-profile"),
        ),
        (
            "an IAM resource with no resource type",
            format!("arn:aws:iam::{account}:a-user"),
        ),
        ("no ARN at all", "not-an-arn-but-long-enough".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "GenerateServiceLastAccessedDetails",
            &[("Arn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!(
                "ARN {arn} is not valid: Arn must be the ARN of an IAM user, group, role or \
                 managed policy"
            ),
            "case {what:?}: expected the refusal to name the value it was given"
        );
    }

    // Rows spanning the resources a report IS generated for, each naming one this account
    // does not hold — including one held under another account's id.
    create_user(&router, "held-user", "/").await;
    for (what, arn) in [
        (
            "a user the account does not hold",
            format!("arn:aws:iam::{account}:user/never-created"),
        ),
        (
            "a group the account does not hold",
            format!("arn:aws:iam::{account}:group/never-created"),
        ),
        (
            "a role the account does not hold",
            format!("arn:aws:iam::{account}:role/never-created"),
        ),
        (
            "a managed policy the account does not hold",
            format!("arn:aws:iam::{account}:policy/never-created"),
        ),
        (
            "a user of another account",
            "arn:aws:iam::999999999999:user/held-user".to_string(),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GenerateServiceLastAccessedDetails",
            &[("Arn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {what:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("ARN {arn} does not exist."),
            "case {what:?}: expected the refusal to name the ARN asked for"
        );
    }

    // A job id the account holds no report under is the modelled not-found — including the
    // id of an Organizations access report, which is a report of another kind read back by
    // another operation. That report is written straight into the service's own store:
    // `GenerateOrganizationsAccessReport` serves only the Organizations management
    // account's credentials, which this unsigned harness does not present.
    let organizations_job = "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_string();
    {
        let held = state::service_state(&*engine).expect("the IAM store");
        let created = state::now();
        held.lock().organizations_access_reports.insert(
            organizations_job.clone(),
            state::OrganizationsAccessReportJob {
                entity_path: "o-abcdefghij/r-root".to_string(),
                organizations_policy_id: None,
                created,
                completes: created,
            },
        );
    }
    for (what, job_id) in [
        (
            "a job id nothing minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the shape of a job id with another value",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "the job id of a report of another kind",
            organizations_job.clone(),
        ),
    ] {
        // Every reader of a report job, so one missing job cannot be described two ways.
        for action in [
            "GetServiceLastAccessedDetails",
            "GetServiceLastAccessedDetailsWithEntities",
            "GetOrganizationsAccessReport",
        ] {
            // The one job this account DOES hold is a report of the other kind, which its
            // own reader serves rather than refuses.
            if action == "GetOrganizationsAccessReport" && job_id == organizations_job {
                continue;
            }
            let mut params = vec![("JobId", job_id.as_str())];
            if action.ends_with("WithEntities") {
                params.push(("ServiceNamespace", "s3"));
            }
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, 404,
                "case {what:?}: {action} expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {what:?}: {action} expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                format!("Job {job_id} does not exist."),
                "case {what:?}: {action} must name the job id asked for, got {:?}",
                element(&body, "Message")
            );
        }
    }

    // And the refused generates recorded nothing: the account holds no service-last-accessed
    // report at all, while the report of the other kind it does hold is untouched.
    let held = state::service_state(&*engine).expect("the IAM store");
    let store = held.lock();
    assert!(
        store.service_last_accessed_reports.is_empty(),
        "a refused generate records nothing, got {:?}",
        store.service_last_accessed_reports.keys().collect::<Vec<&String>>()
    );
    assert_eq!(
        store
            .organizations_access_reports
            .keys()
            .cloned()
            .collect::<Vec<String>>(),
        vec![organizations_job],
        "the report of the other kind must be left as it was"
    );
}
