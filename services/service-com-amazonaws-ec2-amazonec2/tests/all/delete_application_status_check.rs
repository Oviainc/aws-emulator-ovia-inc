// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteApplicationStatusCheck`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DeleteApplicationStatusCheck`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation DELETES the check the request names: it stamps
//! the stored configuration with "the date and time when the application status check was
//! deleted" (`ApplicationStatusCheckResponseObject$DeletionTime`), which puts the check inside
//! its deletion grace period — `DescribeApplicationStatusChecks` reports it only to a caller
//! that asks for deleted checks, and the account's quota of checks has room again — and
//! "deleting a check automatically removes all of its associations", both the tag pairs the
//! check holds and the per-check results the instances it monitored held. The answer is the
//! record as the delete leaves it, and a request outside the rules the operation publishes is
//! refused before anything is written.
//!
//! So the table below spans that class rather than one request: the rows differ along which
//! aggregation setting and which protocol the check carries (from the model's own catalogs
//! `AGGREGATION_STATUS_ENUM_VALUES` and `NETWORK_PROTOCOL_ENUM_VALUES`), which port (both
//! published bounds of `PortNumber`), and WHAT the check is associated with when it is deleted
//! — nothing, one tag pair, several pairs, one instance, several instances, and both halves at
//! once. The refusal table drives every published rule, and the quota and the bounded
//! deletion history are driven at their bounds.

use aws_smithy_types::DateTime;
use ec2::ops::types::{
    ApplicationStatusCheckResponseObject, AGGREGATION_STATUS_ENUM_VALUES,
    NETWORK_PROTOCOL_ENUM_VALUES,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, APPLICATION_STATUS_CHECK_ID_PREFIX,
    MAX_APPLICATION_STATUS_CHECKS, MAX_CLIENT_TOKEN_CHARS,
    MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
// The XML tree readers every suite of this crate shares, rather than another copy of them.
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. Which check a request deletes is a fact of the
/// id it names rather than of the Region, so one Region is used throughout.
const REGION: &str = "us-west-2";

/// The image every launch here names. Which AMI it is does not matter to a deletion.
const AMI: &str = "ami-0123456789abcdef0";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

// --- the calls ---------------------------------------------------------------

/// One `CreateApplicationStatusCheck` call, as the id of the check it created.
async fn create_check(router: &MultiServiceRouter, extra: &[(String, String)]) -> String {
    let mut params = vec![
        p("Protocol", NETWORK_PROTOCOL_ENUM_VALUES[0]),
        p("Port", "80"),
    ];
    params.extend_from_slice(extra);
    let (status, body) =
        ec2_query::call(router, REGION, "CreateApplicationStatusCheck", &params).await;
    assert_eq!(
        status, 200,
        "CreateApplicationStatusCheck must succeed, got {status}: {body}"
    );
    ec2_query::element(&body, "applicationStatusCheckId")
        .unwrap_or_else(|| panic!("the create must report the id it minted: {body}"))
}

/// Launch one instance and report its id.
async fn launch_one(router: &MultiServiceRouter) -> String {
    let params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
    let (status, body) = ec2_query::call(router, REGION, "RunInstances", &params).await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    ec2_query::elements(&body, "instanceId")
        .pop()
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// One `AssociateApplicationStatusCheck` call that must succeed.
async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    check_id: &str,
    params: &[(String, String)],
) {
    let mut all = vec![p("ApplicationStatusCheckId", check_id)];
    all.extend_from_slice(params);
    let (status, body) =
        ec2_query::call(router, REGION, "AssociateApplicationStatusCheck", &all).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssociateApplicationStatusCheck must succeed, got {status}: {body}"
    );
}

/// One `DeleteApplicationStatusCheck` call carrying `params`, as its HTTP status and body.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    ec2_query::call(router, REGION, "DeleteApplicationStatusCheck", params).await
}

/// The form fields of a `TargetTagAssociations` list, as ec2Query flattens it.
fn tag_fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(nth, (key, value))| {
            let position = nth + 1;
            vec![
                p(&format!("TargetTagAssociation.{position}.Key"), *key),
                p(&format!("TargetTagAssociation.{position}.Value"), *value),
            ]
        })
        .collect()
}

/// The form fields of an `InstanceIds` list.
fn instance_fields(instance_ids: &[String]) -> Vec<(String, String)> {
    instance_ids
        .iter()
        .enumerate()
        .map(|(nth, id)| p(&format!("InstanceId.{}", nth + 1), id.clone()))
        .collect()
}

// --- reading the response ----------------------------------------------------

/// One reported `ApplicationStatusCheckResponseObject`: the members a delete can be judged on
/// — its id, the configuration it must leave alone, the instants it carries and the tag
/// associations it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    aggregation: Option<String>,
    protocol: Option<String>,
    port: Option<String>,
    path: Option<String>,
    interval: Option<String>,
    timeout: Option<String>,
    creation: Option<String>,
    deletion: Option<String>,
    target_tags: Vec<(String, String)>,
}

fn row_of(node: roxmltree::Node<'_, '_>) -> Row {
    Row {
        id: child_text(node, "applicationStatusCheckId").unwrap_or_default(),
        aggregation: child_text(node, "aggregation"),
        protocol: child_text(node, "protocol"),
        port: child_text(node, "port"),
        path: child_text(node, "path"),
        interval: child_text(node, "interval"),
        timeout: child_text(node, "timeout"),
        creation: child_text(node, "creationTime"),
        deletion: child_text(node, "deletionTime"),
        target_tags: set_items(node, "targetTagAssociationSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// The one check a `DeleteApplicationStatusCheck` response carries.
fn answered(case: &str, body: &str) -> Row {
    let parsed = document(case, body);
    let check = child(parsed.root_element(), "applicationStatusCheck")
        .unwrap_or_else(|| panic!("case {case:?}: no <applicationStatusCheck> in {body}"));
    row_of(check)
}

/// The checks `DescribeApplicationStatusChecks` reports, narrowed by `params`.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Row> {
    let (status, body) =
        ec2_query::call(router, REGION, "DescribeApplicationStatusChecks", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplicationStatusChecks must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "applicationStatusCheckSet")
        .into_iter()
        .map(row_of)
        .collect()
}

/// The one row the describe reports for `check_id` among `rows`.
fn row_for<'a>(case: &str, rows: &'a [Row], check_id: &str) -> &'a Row {
    rows.iter()
        .find(|row| row.id == check_id)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the describe must report {check_id}, got {:?}",
                rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
            )
        })
}

/// What `DescribeApplicationStatus` reports for one instance: `(the instance-level status, the
/// check id of every per-check result it reports)`.
async fn described_application_status(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> (Option<String>, Vec<String>) {
    let (status, body) = ec2_query::call(
        router,
        REGION,
        "DescribeApplicationStatus",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplicationStatus must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let instances = child(parsed.root_element(), "applicationStatusesResponseType")
        .unwrap_or_else(|| panic!("case {case:?}: no application statuses in {body}"));
    let instance = set_items(instances, "instanceSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report {instance_id}: {body}"));
    let application_status = child(instance, "applicationStatus")
        .unwrap_or_else(|| panic!("case {case:?}: no applicationStatus in {body}"));
    let checks = set_items(application_status, "detailSet")
        .into_iter()
        .map(|detail| child_text(detail, "applicationStatusCheckId").unwrap_or_default())
        .collect();
    (child_text(application_status, "status"), checks)
}

// --- the table ---------------------------------------------------------------

/// One row: the check a caller creates, what it is associated with, and the label the
/// assertions name it by.
struct Case {
    label: &'static str,
    aggregation: &'static str,
    protocol: &'static str,
    port: &'static str,
    /// The tag pairs a first call associates the check with, if any.
    tag_pairs: Vec<(&'static str, &'static str)>,
    /// How many launched instances a second call associates the check with.
    instances: usize,
}

fn cases() -> Vec<Case> {
    // The enum-valued members come from the model's own catalogs, and the ports are the two
    // published bounds of `PortNumber` (range 1..65535,
    // `smithy_query(query="operation",operation="CreateApplicationStatusCheck")`).
    let [excluded, included] = [
        AGGREGATION_STATUS_ENUM_VALUES[0],
        AGGREGATION_STATUS_ENUM_VALUES[1],
    ];
    let [http, https] = [
        NETWORK_PROTOCOL_ENUM_VALUES[0],
        NETWORK_PROTOCOL_ENUM_VALUES[1],
    ];
    vec![
        Case {
            label: "a check nothing is associated with",
            aggregation: included,
            protocol: http,
            port: "80",
            tag_pairs: Vec::new(),
            instances: 0,
        },
        Case {
            label: "a check holding one tag association",
            aggregation: included,
            protocol: https,
            port: "443",
            tag_pairs: vec![("Name", "checkout")],
            instances: 0,
        },
        Case {
            label: "a check holding three tag associations",
            aggregation: included,
            protocol: http,
            port: "1",
            tag_pairs: vec![("Tier", "web"), ("Env", "prod"), ("Env", "staging")],
            instances: 0,
        },
        Case {
            label: "a check monitoring one instance",
            aggregation: included,
            protocol: http,
            port: "8080",
            tag_pairs: Vec::new(),
            instances: 1,
        },
        Case {
            label: "a check monitoring two instances",
            aggregation: included,
            protocol: https,
            port: "65535",
            tag_pairs: Vec::new(),
            instances: 2,
        },
        Case {
            label: "a check holding both kinds of association",
            aggregation: included,
            protocol: http,
            port: "8443",
            tag_pairs: vec![("Tier", "web"), ("Env", "prod")],
            instances: 2,
        },
        Case {
            label: "an excluded check holding both kinds of association",
            aggregation: excluded,
            protocol: https,
            port: "9000",
            tag_pairs: vec![("Monitored", "")],
            instances: 1,
        },
    ]
}

/// The delete stamps the check's `DeletionTime`, leaves the rest of its configuration alone,
/// takes every association with it, and hands back the record a describe asking for deleted
/// checks then reports — while the account's own list of checks no longer carries it.
#[tokio::test]
async fn deleting_a_check_stamps_its_deletion_time_and_removes_its_associations() {
    for case in cases() {
        let label = case.label;
        // One account per row: what an instance's application status is derived from is every
        // check associated with it, so a row starts from an account holding neither.
        let (router, _engine) = ec2_query::emulator();
        let mut instances: Vec<String> = Vec::new();
        for _ in 0..case.instances {
            instances.push(launch_one(&router).await);
        }
        let check_id = create_check(
            &router,
            &[
                p("Aggregation", case.aggregation),
                p("Protocol", case.protocol),
                p("Port", case.port),
            ],
        )
        .await;
        if !case.tag_pairs.is_empty() {
            associate(&router, label, &check_id, &tag_fields(&case.tag_pairs)).await;
        }
        if !instances.is_empty() {
            associate(&router, label, &check_id, &instance_fields(&instances)).await;
        }

        // What the account holds BEFORE the delete: an active check carrying the
        // configuration the create was given and the associations the associate made.
        let before_rows = described(&router, label, &[]).await;
        let before = row_for(label, &before_rows, &check_id).clone();
        assert!(
            before.deletion.is_none(),
            "case {label:?}: a check nobody deleted must carry no deletionTime, \
             got {:?}",
            before.deletion
        );
        assert_eq!(
            before.target_tags.len(),
            case.tag_pairs.len(),
            "case {label:?}: the check must hold the pairs the associate named, got {:?}",
            before.target_tags
        );
        for instance_id in &instances {
            let (_, monitored) = described_application_status(&router, label, instance_id).await;
            assert!(
                monitored.contains(&check_id),
                "case {label:?}: {instance_id} must be monitored by {check_id} before the \
                 delete, got {monitored:?}"
            );
        }

        let (status, body) = delete(&router, &[p("ApplicationStatusCheckId", &check_id)]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must succeed, got {status} with body {body}"
        );
        let deleted = answered(label, &body);
        assert_eq!(
            deleted.id, check_id,
            "case {label:?}: the delete must answer with the check it deleted, got {:?}",
            deleted.id
        );
        assert!(
            deleted.deletion.is_some(),
            "case {label:?}: the deleted check must carry a deletionTime, got {deleted:?}"
        );
        assert!(
            deleted.target_tags.is_empty(),
            "case {label:?}: the delete must remove every tag association, got {:?}",
            deleted.target_tags
        );
        // Everything the delete is not about is untouched: the configuration the create
        // stored, and the instant it was created at.
        assert_eq!(
            (
                &deleted.aggregation,
                &deleted.protocol,
                &deleted.port,
                &deleted.path,
                &deleted.interval,
                &deleted.timeout,
                &deleted.creation
            ),
            (
                &before.aggregation,
                &before.protocol,
                &before.port,
                &before.path,
                &before.interval,
                &before.timeout,
                &before.creation
            ),
            "case {label:?}: the delete must leave the configuration alone, got {deleted:?} \
             against {before:?}"
        );

        // "If you omit this parameter or set it to false, the response includes only active
        // checks": the account's list no longer carries the deleted check.
        let active = described(&router, label, &[]).await;
        assert!(
            !active.iter().any(|row| row.id == check_id),
            "case {label:?}: a deleted check must not be one of the account's checks, got {:?}",
            active.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
        );

        // "Recently deleted application status checks that remain available during the deletion
        // grace period": asked for, the deleted check is reported exactly as the delete left
        // it.
        let all = described(&router, label, &[p("IncludeAll", "true")]).await;
        let reported = row_for(label, &all, &check_id);
        assert_eq!(
            reported, &deleted,
            "case {label:?}: the describe must report what the delete answered, got \
             {reported:?} against {deleted:?}"
        );

        // "Deleting a check automatically removes all of its associations": the instances it
        // monitored hold no result for it any more, so an instance no other check applies to
        // is back to `not-applicable`.
        for instance_id in &instances {
            let (instance_status, monitored) =
                described_application_status(&router, label, instance_id).await;
            assert!(
                !monitored.contains(&check_id),
                "case {label:?}: {instance_id} must no longer be monitored by {check_id}, \
                 got {monitored:?}"
            );
            assert_eq!(
                instance_status,
                Some("not-applicable".to_string()),
                "case {label:?}: {instance_id} must have no applicable check left, got \
                 {instance_status:?}"
            );
        }
    }
}

/// "If you retry a request with the same token, the service ignores the request but does not
/// return an error": a check inside its deletion grace period has been deleted once, so a
/// second delete of it answers what the first one answered — the same deletion instant — and
/// deletes nothing again.
#[tokio::test]
async fn deleting_a_deleted_check_answers_the_first_delete_and_writes_nothing_again() {
    let (router, _engine) = ec2_query::emulator();
    let instance_id = launch_one(&router).await;
    let check_id = create_check(&router, &[]).await;
    associate(
        &router,
        "the first delete",
        &check_id,
        &tag_fields(&[("Env", "prod")]),
    )
    .await;
    associate(
        &router,
        "the first delete",
        &check_id,
        &instance_fields(std::slice::from_ref(&instance_id)),
    )
    .await;

    let token = "delete-token-1";
    let (status, body) = delete(
        &router,
        &[
            p("ApplicationStatusCheckId", &check_id),
            p("ClientToken", token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the first delete must succeed, got {status}: {body}"
    );
    let first = answered("the first delete", &body);

    // The same request again, and the same request without the token: both answer the record
    // the first delete left, deletion instant included.
    for (label, params) in [
        (
            "the delete retried with its own token",
            vec![
                p("ApplicationStatusCheckId", &check_id),
                p("ClientToken", token),
            ],
        ),
        (
            "the delete retried with no token at all",
            vec![p("ApplicationStatusCheckId", &check_id)],
        ),
        (
            "the delete retried with another token",
            vec![
                p("ApplicationStatusCheckId", &check_id),
                p("ClientToken", "delete-token-2"),
            ],
        ),
    ] {
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a retried delete must succeed, got {status}: {body}"
        );
        let retried = answered(label, &body);
        assert_eq!(
            retried, first,
            "case {label:?}: a retried delete must answer the first one, got {retried:?} \
             against {first:?}"
        );
    }

    // One deletion, one row: the retries stored no second copy and moved no instant.
    let all = described(&router, "after every retry", &[p("IncludeAll", "true")]).await;
    assert_eq!(
        all.len(),
        1,
        "the account must hold exactly one check after the retries, got {:?}",
        all.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        row_for("after every retry", &all, &check_id),
        &first,
        "the stored check must still be the one the first delete left"
    );
}

/// Every rule the operation publishes, refused with this service's own code — the subject id
/// first, a value that is no id of this kind before an id that names no check — and nothing
/// deleted by a request that was refused.
#[tokio::test]
async fn bad_requests_are_refused_and_delete_nothing() {
    let (router, _engine) = ec2_query::emulator();
    let check_id = create_check(&router, &[]).await;

    /// One refusal row: the request, and the status, code and text it must answer with.
    struct Refusal {
        label: &'static str,
        params: Vec<(String, String)>,
        status: u16,
        code: &'static str,
        names: String,
    }

    let too_long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);

    let refusals = vec![
        // A value that is no application status check id at all, in each of the ways one can
        // fail to be one: another resource's prefix, a digit outside the hexadecimal
        // alphabet, a suffix of neither published length, and no prefix at all.
        Refusal {
            label: "a check id under another prefix",
            params: vec![p(
                "ApplicationStatusCheckId",
                "appstatuscheck-0123456789abcdef0",
            )],
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "appstatuscheck-0123456789abcdef0".to_string(),
        },
        Refusal {
            label: "a check id carrying a digit outside the hexadecimal alphabet",
            params: vec![p("ApplicationStatusCheckId", "asc-0123456789abcdefg")],
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "asc-0123456789abcdefg".to_string(),
        },
        Refusal {
            label: "a check id of neither published length",
            params: vec![p("ApplicationStatusCheckId", "asc-0123456")],
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "asc-0123456".to_string(),
        },
        Refusal {
            label: "a value with no resource prefix at all",
            params: vec![p("ApplicationStatusCheckId", "0123456789abcdef0")],
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "0123456789abcdef0".to_string(),
        },
        // A well-formed id of a check the account does not hold: the OTHER half of the pair,
        // in each published id length.
        Refusal {
            label: "a well-formed check id the account holds no check for",
            params: vec![p("ApplicationStatusCheckId", "asc-0123456789abcdef0")],
            status: 400,
            code: "InvalidApplicationStatusCheckId.NotFound",
            names: "asc-0123456789abcdef0".to_string(),
        },
        Refusal {
            label: "a short well-formed check id the account holds no check for",
            params: vec![p("ApplicationStatusCheckId", "asc-12345678")],
            status: 400,
            code: "InvalidApplicationStatusCheckId.NotFound",
            names: "asc-12345678".to_string(),
        },
        // The id is resolved BEFORE the rest of the request, so a dry run of a request naming
        // a check that does not exist is answered for the id.
        Refusal {
            label: "a dry run naming a check the account holds no check for",
            params: vec![
                p("ApplicationStatusCheckId", "asc-0123456789abcdef0"),
                p("DryRun", "true"),
            ],
            status: 400,
            code: "InvalidApplicationStatusCheckId.NotFound",
            names: "asc-0123456789abcdef0".to_string(),
        },
        Refusal {
            label: "a client token one character above the maximum length",
            params: vec![
                p("ApplicationStatusCheckId", &check_id),
                p("ClientToken", too_long_token.clone()),
            ],
            status: 400,
            code: "InvalidParameterValue",
            names: MAX_CLIENT_TOKEN_CHARS.to_string(),
        },
        // "If you have the required permissions, the error response is DryRunOperation."
        Refusal {
            label: "a dry run of a request that would have succeeded",
            params: vec![
                p("ApplicationStatusCheckId", &check_id),
                p("DryRun", "true"),
            ],
            status: 412,
            code: "DryRunOperation",
            names: "DryRun".to_string(),
        },
    ];

    for refusal in refusals {
        let label = refusal.label;
        let (status, body) = delete(&router, &refusal.params).await;
        assert_eq!(
            status, refusal.status,
            "case {label:?}: expected HTTP {}, got {status} with body {body}",
            refusal.status
        );
        assert_eq!(
            ec2_query::error_code(&body),
            refusal.code,
            "case {label:?}: expected the code {:?}, got body {body}",
            refusal.code
        );
        let message = ec2_query::error_message(&body);
        assert!(
            message.contains(&refusal.names),
            "case {label:?}: expected the message to name {:?}, got {message:?}",
            refusal.names
        );
    }

    // Every one of those requests was refused before anything was written: the check is still
    // one the account holds, and still active.
    let active = described(&router, "after every refusal", &[]).await;
    let row = row_for("after every refusal", &active, &check_id);
    assert!(
        row.deletion.is_none(),
        "a refused delete must delete nothing, got {row:?}"
    );
}

/// "You can create a maximum of 50 application status checks for each account": the check a
/// delete puts inside its deletion grace period is not one the account still holds, so the
/// delete gives its quota slot back.
#[tokio::test]
async fn a_delete_gives_the_accounts_quota_slot_back() {
    let (router, engine) = ec2_query::emulator();
    let store = engine.state::<Ec2State>();
    {
        // The account one slot below its quota, seeded the way every id of this service is
        // minted, so the quota is counted over the id form the store actually holds.
        let mut guard = store.lock();
        for _ in 0..MAX_APPLICATION_STATUS_CHECKS - 1 {
            let id = guard.next_id(APPLICATION_STATUS_CHECK_ID_PREFIX);
            guard.application_status_checks.insert(
                id.clone(),
                ApplicationStatusCheckResponseObject {
                    application_status_check_id: Some(id.clone()),
                    ..ApplicationStatusCheckResponseObject::default()
                },
            );
        }
    }

    // The last slot, taken over the wire, and the create past the quota refused.
    let check_id = create_check(&router, &[]).await;
    let params = vec![
        p("Protocol", NETWORK_PROTOCOL_ENUM_VALUES[0]),
        p("Port", "80"),
    ];
    let (status, body) =
        ec2_query::call(&router, REGION, "CreateApplicationStatusCheck", &params).await;
    assert_eq!(
        status, 400,
        "a create past the quota must be refused, got {status}: {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "ResourceLimitExceeded",
        "expected ResourceLimitExceeded, got {body}"
    );

    let (status, body) = delete(&router, &[p("ApplicationStatusCheckId", &check_id)]).await;
    assert_eq!(status, 200, "the delete must succeed, got {status}: {body}");

    let (status, body) =
        ec2_query::call(&router, REGION, "CreateApplicationStatusCheck", &params).await;
    assert_eq!(
        status, 200,
        "a create must succeed once a check is deleted, got {status}: {body}"
    );
    let minted = ec2_query::element(&body, "applicationStatusCheckId")
        .unwrap_or_else(|| panic!("the create must report the id it minted: {body}"));
    assert_ne!(
        minted, check_id,
        "the create must mint a fresh id rather than reuse the deleted check's"
    );
}

/// The deletion grace period is finite: the store keeps at most
/// `MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS` deleted checks, and the
/// oldest-deleted one leaves for good to make room for a newer deletion.
#[tokio::test]
async fn the_deletion_history_is_bounded_and_drops_the_oldest_deletion_first() {
    let (router, engine) = ec2_query::emulator();
    let store = engine.state::<Ec2State>();

    // A full history of deletions, each dated one second after the last, so which one is the
    // oldest is unambiguous.
    let mut deleted_ids: Vec<String> = Vec::new();
    {
        let mut guard = store.lock();
        for nth in 0..MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS {
            let id = guard.next_id(APPLICATION_STATUS_CHECK_ID_PREFIX);
            let deleted_at = DateTime::from_secs(1_767_225_600 + nth as i64);
            guard.application_status_checks.insert(
                id.clone(),
                ApplicationStatusCheckResponseObject {
                    application_status_check_id: Some(id.clone()),
                    deletion_time: Some(deleted_at),
                    ..ApplicationStatusCheckResponseObject::default()
                },
            );
            deleted_ids.push(id);
        }
    }
    let oldest = deleted_ids
        .first()
        .cloned()
        .unwrap_or_else(|| unreachable!("the history holds more than one deletion"));

    // One more deletion, performed over the wire.
    let check_id = create_check(&router, &[]).await;
    let (status, body) = delete(&router, &[p("ApplicationStatusCheckId", &check_id)]).await;
    assert_eq!(status, 200, "the delete must succeed, got {status}: {body}");

    let all = described(&router, "a full deletion history", &[p("IncludeAll", "true")]).await;
    let reported: Vec<String> = all.iter().map(|row| row.id.clone()).collect();
    assert_eq!(
        reported.len(),
        MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS,
        "the store must keep at most {MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS} \
         deletions, got {reported:?}"
    );
    assert!(
        reported.contains(&check_id),
        "the newest deletion must be one the store keeps, got {reported:?}"
    );
    assert!(
        !reported.contains(&oldest),
        "the oldest deletion must have left the store, got {reported:?}"
    );
    // Gone for good, not merely hidden: the id names no check of the store any more.
    let held = store
        .lock()
        .application_status_checks
        .contains_key(&oldest);
    assert!(
        !held,
        "the store must hold no record for the dropped deletion {oldest}"
    );
    for kept in deleted_ids.iter().skip(1) {
        assert!(
            reported.contains(kept),
            "a deletion inside the bound must be kept, got {reported:?} without {kept}"
        );
    }
}
