// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateApplicationStatusCheck`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=AssociateApplicationStatusCheck`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation STORES what the request names and reports one
//! row per item: a tag association becomes one of the check's own `TargetTagAssociations`,
//! which `DescribeApplicationStatusChecks` reports, and an instance association starts the
//! health monitoring that `DescribeApplicationStatus` reports as that check's
//! `initializing` result for the instance ("once you create an association, health
//! monitoring automatically begins for the specified instances"). An item that cannot be
//! associated is reported in the unsuccessful set with its reason, and a request outside the
//! rules the operation publishes is refused before anything is stored — the subject id
//! first, a value that is no id of this kind before an id that names no check.
//!
//! So the tables below span those classes rather than one request: the tag rows differ along
//! how many pairs are named, whether a pair repeats, whether its value is empty and whether
//! the check already holds other pairs; the instance rows along how many instances are
//! named, whether one repeats, whether the check aggregates into the instance-level status,
//! and whether an instance is one the account holds, a well-formed id that names nothing, or
//! no instance id at all; and the refusal rows drive every published rule, the per-check tag
//! ceiling at the bound and one past it.

use ec2::state::{
    ASSOCIATION_TYPE_EC2_TAG, ASSOCIATION_TYPE_INSTANCE_ID,
    MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS, MAX_TAG_KEY_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
// The XML tree readers every suite of this crate shares, rather than another copy of them.
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. What a check is associated with is a fact of
/// the check and of the instances, so one Region is used throughout.
const REGION: &str = "us-west-2";

/// The image every launch here names. Which AMI it is does not matter to an association.
const AMI: &str = "ami-0123456789abcdef0";

/// A well-formed instance id the account holds no instance for.
const UNKNOWN_INSTANCE: &str = "i-0123456789abcdef0";

/// A value that is no instance id at all.
const MALFORMED_INSTANCE: &str = "instance-one";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
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

/// One `CreateApplicationStatusCheck` call, as the id of the check it created.
async fn create_check(router: &MultiServiceRouter, extra: &[(String, String)]) -> String {
    let mut params = vec![p("Protocol", "http"), p("Port", "80")];
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
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status}: {body}"
    );
    ec2_query::elements(&body, "instanceId")
        .pop()
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// One `AssociateApplicationStatusCheck` call carrying `params`.
async fn associate(
    router: &MultiServiceRouter,
    check_id: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut all = vec![p("ApplicationStatusCheckId", check_id)];
    all.extend_from_slice(params);
    ec2_query::call(router, REGION, "AssociateApplicationStatusCheck", &all).await
}

// --- reading the response ----------------------------------------------------

/// One reported association result: `(the check id, the association type, its value, the
/// reason an unsuccessful one carries)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    check_id: Option<String>,
    association_type: Option<String>,
    association_value: Option<String>,
    reason: Option<String>,
}

/// The entries of the `set` the response carries.
fn reported(case: &str, body: &str, set: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), set)
        .into_iter()
        .map(|item| Reported {
            check_id: child_text(item, "applicationStatusCheckId"),
            association_type: child_text(item, "associationType"),
            association_value: child_text(item, "associationValue"),
            reason: child_text(item, "reason"),
        })
        .collect()
}

/// A successful row for one item.
fn success(check_id: &str, association_type: &str, value: &str) -> Reported {
    Reported {
        check_id: Some(check_id.to_string()),
        association_type: Some(association_type.to_string()),
        association_value: Some(value.to_string()),
        reason: None,
    }
}

/// The `(key, value)` pairs the check `check_id` is associated with, read back through
/// `DescribeApplicationStatusChecks`.
async fn described_target_tags(
    router: &MultiServiceRouter,
    case: &str,
    check_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = ec2_query::call(
        router,
        REGION,
        "DescribeApplicationStatusChecks",
        &[p("ApplicationStatusCheckId.1", check_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplicationStatusChecks must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let check = set_items(parsed.root_element(), "applicationStatusCheckSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report {check_id}: {body}"));
    set_items(check, "targetTagAssociationSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// Whether `DescribeApplicationStatusChecks` reports a `lastUpdatedAt` for `check_id`.
async fn described_last_updated(
    router: &MultiServiceRouter,
    case: &str,
    check_id: &str,
) -> Option<String> {
    let (status, body) = ec2_query::call(
        router,
        REGION,
        "DescribeApplicationStatusChecks",
        &[p("ApplicationStatusCheckId.1", check_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplicationStatusChecks must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let check = set_items(parsed.root_element(), "applicationStatusCheckSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report {check_id}: {body}"));
    child_text(check, "lastUpdatedAt")
}

/// What `DescribeApplicationStatus` reports for one instance: `(the instance-level status,
/// one (check id, detail status, aggregation) per reported detail)`.
type ReportedStatus = (Option<String>, Vec<(String, String, String)>);

async fn described_application_status(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> ReportedStatus {
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
    let details = set_items(application_status, "detailSet")
        .into_iter()
        .map(|detail| {
            (
                child_text(detail, "applicationStatusCheckId").unwrap_or_default(),
                child_text(detail, "status").unwrap_or_default(),
                child_text(detail, "aggregation").unwrap_or_default(),
            )
        })
        .collect();
    (child_text(application_status, "status"), details)
}

// --- the tag half ------------------------------------------------------------

/// One row of the tag table: the pairs the request names, the pairs the check must then be
/// associated with, and the values the successful set must report.
struct TagCase {
    label: &'static str,
    /// Pairs associated by a FIRST call, so a row can start from a check that already holds
    /// associations.
    already: Vec<(&'static str, &'static str)>,
    asked: Vec<(&'static str, &'static str)>,
    /// The distinct pairs of `asked`, in request order: what the successful set reports.
    reported: Vec<(&'static str, &'static str)>,
    /// What the check must be associated with afterwards.
    stored: Vec<(&'static str, &'static str)>,
}

fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            label: "one pair",
            already: Vec::new(),
            asked: vec![("Name", "checkout")],
            reported: vec![("Name", "checkout")],
            stored: vec![("Name", "checkout")],
        },
        TagCase {
            label: "two pairs at once",
            already: Vec::new(),
            asked: vec![("Tier", "web"), ("Env", "prod")],
            reported: vec![("Tier", "web"), ("Env", "prod")],
            stored: vec![("Tier", "web"), ("Env", "prod")],
        },
        TagCase {
            label: "two values of one key",
            already: Vec::new(),
            asked: vec![("Env", "prod"), ("Env", "staging")],
            reported: vec![("Env", "prod"), ("Env", "staging")],
            stored: vec![("Env", "prod"), ("Env", "staging")],
        },
        TagCase {
            label: "a pair whose value is empty",
            already: Vec::new(),
            asked: vec![("Monitored", "")],
            reported: vec![("Monitored", "")],
            stored: vec![("Monitored", "")],
        },
        TagCase {
            label: "the same pair named twice is one association",
            already: Vec::new(),
            asked: vec![("Name", "checkout"), ("Name", "checkout")],
            reported: vec![("Name", "checkout")],
            stored: vec![("Name", "checkout")],
        },
        TagCase {
            label: "a pair added to a check that already holds another",
            already: vec![("Tier", "web")],
            asked: vec![("Env", "prod")],
            reported: vec![("Env", "prod")],
            stored: vec![("Tier", "web"), ("Env", "prod")],
        },
        TagCase {
            label: "a pair the check already holds, named again",
            already: vec![("Tier", "web")],
            asked: vec![("Tier", "web")],
            reported: vec![("Tier", "web")],
            stored: vec![("Tier", "web")],
        },
    ]
}

/// `(key, value)` pairs as the response and the describe report them.
fn owned_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// "The tags to associate the application status check with": each pair becomes one of the
/// check's `TargetTagAssociations`, reported as `EC2TAG` with the value `key=value`, and the
/// same pair twice is one association.
#[tokio::test]
async fn tag_associations_are_stored_on_the_check_and_reported_per_pair() {
    let (router, _engine) = ec2_query::emulator();

    for case in tag_cases() {
        let label = case.label;
        let check_id = create_check(&router, &[]).await;
        assert_eq!(
            described_last_updated(&router, label, &check_id).await,
            None,
            "case {label:?}: a check nobody has associated must report no lastUpdatedAt"
        );

        if !case.already.is_empty() {
            let (status, body) =
                associate(&router, &check_id, &tag_fields(&case.already)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the first associate must succeed, got {status}: {body}"
            );
        }

        let (status, body) = associate(&router, &check_id, &tag_fields(&case.asked)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let wanted: Vec<Reported> = case
            .reported
            .iter()
            .map(|(key, value)| {
                success(&check_id, ASSOCIATION_TYPE_EC2_TAG, &format!("{key}={value}"))
            })
            .collect();
        let got = reported(label, &body, "successfulResultSet");
        assert_eq!(
            got, wanted,
            "case {label:?}: expected the successful set {wanted:?}, got {got:?} in {body}"
        );
        let failed = reported(label, &body, "unsuccessfulResultSet");
        assert!(
            failed.is_empty(),
            "case {label:?}: expected no unsuccessful rows, got {failed:?} in {body}"
        );

        let stored = described_target_tags(&router, label, &check_id).await;
        assert_eq!(
            stored,
            owned_pairs(&case.stored),
            "case {label:?}: expected the check to be associated with {:?}, got {stored:?}",
            owned_pairs(&case.stored)
        );
        assert!(
            described_last_updated(&router, label, &check_id)
                .await
                .is_some(),
            "case {label:?}: an associate updates the check, so it must report a lastUpdatedAt"
        );
    }
}

// --- the instance half -------------------------------------------------------

/// One row of the instance table: how many of the launched instances the request names, how
/// many times, and what the response and the status describe must then report.
struct InstanceCase {
    label: &'static str,
    /// Which launched instances the request names, by index, in request order.
    named: Vec<usize>,
    /// The aggregation the check is created with.
    aggregation: &'static str,
    /// The instance-level status each associated instance must then report.
    instance_status: &'static str,
    /// Whether the request also names ids the account holds no instance for.
    with_unusable: bool,
}

fn instance_cases() -> Vec<InstanceCase> {
    vec![
        InstanceCase {
            label: "one instance, an included check",
            named: vec![0],
            aggregation: "included",
            // "initializing – At least one included check is initializing, and no included
            // check is impaired."
            instance_status: "initializing",
            with_unusable: false,
        },
        InstanceCase {
            label: "two instances at once",
            named: vec![0, 1],
            aggregation: "included",
            instance_status: "initializing",
            with_unusable: false,
        },
        InstanceCase {
            label: "one instance named twice is one association",
            named: vec![1, 1],
            aggregation: "included",
            instance_status: "initializing",
            with_unusable: false,
        },
        InstanceCase {
            label: "an excluded check does not move the instance-level status",
            named: vec![0],
            aggregation: "excluded",
            // "Checks with Aggregation set to excluded do not affect this value", so no
            // INCLUDED check applies to the instance.
            instance_status: "not-applicable",
            with_unusable: false,
        },
        InstanceCase {
            label: "one usable instance beside ids the account cannot use",
            named: vec![1],
            aggregation: "included",
            instance_status: "initializing",
            with_unusable: true,
        },
    ]
}

/// "The IDs of the instances to associate with the application status check": each one the
/// account holds starts being monitored by the check — the result
/// `DescribeApplicationStatus` then reports for it — while an id the account cannot use is
/// reported in the unsuccessful set with the reason, and the rest of the request still runs.
#[tokio::test]
async fn instance_associations_begin_monitoring_and_report_one_row_per_instance() {
    for case in instance_cases() {
        let label = case.label;
        // One account per row: what an instance's status is derived from is every check
        // associated with it, so a row starts from instances no check applies to.
        let (router, _engine) = ec2_query::emulator();
        let instances = [launch_one(&router).await, launch_one(&router).await];
        let check_id = create_check(&router, &[p("Aggregation", case.aggregation)]).await;
        let named: Vec<String> = case
            .named
            .iter()
            .map(|index| instances[*index].clone())
            .collect();
        let mut params = instance_fields(&named);
        if case.with_unusable {
            let position = named.len();
            params.push(p(
                &format!("InstanceId.{}", position + 1),
                UNKNOWN_INSTANCE,
            ));
            params.push(p(
                &format!("InstanceId.{}", position + 2),
                MALFORMED_INSTANCE,
            ));
        }

        let (status, body) = associate(&router, &check_id, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );

        // One row per DISTINCT instance the account holds, in request order.
        let mut distinct: Vec<String> = Vec::new();
        for instance_id in &named {
            if !distinct.contains(instance_id) {
                distinct.push(instance_id.clone());
            }
        }
        let wanted: Vec<Reported> = distinct
            .iter()
            .map(|instance_id| success(&check_id, ASSOCIATION_TYPE_INSTANCE_ID, instance_id))
            .collect();
        let got = reported(label, &body, "successfulResultSet");
        assert_eq!(
            got, wanted,
            "case {label:?}: expected the successful set {wanted:?}, got {got:?} in {body}"
        );

        let failed = reported(label, &body, "unsuccessfulResultSet");
        if case.with_unusable {
            let values: Vec<Option<String>> = failed
                .iter()
                .map(|row| row.association_value.clone())
                .collect();
            assert_eq!(
                values,
                vec![
                    Some(UNKNOWN_INSTANCE.to_string()),
                    Some(MALFORMED_INSTANCE.to_string())
                ],
                "case {label:?}: expected an unsuccessful row per unusable id, got {failed:?}"
            );
            let reasons: Vec<String> = failed
                .iter()
                .map(|row| row.reason.clone().unwrap_or_default())
                .collect();
            assert!(
                reasons[0].contains(UNKNOWN_INSTANCE) && reasons[0].contains("does not exist"),
                "case {label:?}: expected the reason to say the instance does not exist, \
                 got {:?}",
                reasons[0]
            );
            assert!(
                reasons[1].contains(MALFORMED_INSTANCE) && reasons[1].contains("Invalid id"),
                "case {label:?}: expected the reason to say the value is no instance id, \
                 got {:?}",
                reasons[1]
            );
        } else {
            assert!(
                failed.is_empty(),
                "case {label:?}: expected no unsuccessful rows, got {failed:?} in {body}"
            );
        }

        // Health monitoring has begun for every associated instance, and for nobody else.
        for (index, instance_id) in instances.iter().enumerate() {
            let (instance_status, details) =
                described_application_status(&router, label, instance_id).await;
            let monitored = distinct.contains(instance_id);
            let wanted_detail = (
                check_id.clone(),
                "initializing".to_string(),
                case.aggregation.to_string(),
            );
            if monitored {
                assert!(
                    details.contains(&wanted_detail),
                    "case {label:?}: instance {index} must report {wanted_detail:?}, \
                     got {details:?}"
                );
                assert_eq!(
                    instance_status,
                    Some(case.instance_status.to_string()),
                    "case {label:?}: instance {index} must report the status {:?}, got {:?}",
                    case.instance_status,
                    instance_status
                );
            } else {
                assert!(
                    !details.iter().any(|(reported, _, _)| reported == &check_id),
                    "case {label:?}: instance {index} was not associated, so it must report \
                     no result for {check_id}, got {details:?}"
                );
            }
        }

        // The tag half is untouched by an instance association.
        let tags = described_target_tags(&router, label, &check_id).await;
        assert!(
            tags.is_empty(),
            "case {label:?}: an instance association must associate no tag, got {tags:?}"
        );
        // An association of either half updates the check.
        assert!(
            described_last_updated(&router, label, &check_id)
                .await
                .is_some(),
            "case {label:?}: an associate updates the check, so it must report a lastUpdatedAt"
        );
    }
}

/// "If you retry a request with the same token, the service ignores the request but does not
/// return an error": associating what is already associated answers the same way and stores
/// no second copy, whichever half the request named.
#[tokio::test]
async fn associating_twice_answers_the_same_way_and_stores_one_association() {
    let (router, _engine) = ec2_query::emulator();
    let instance_id = launch_one(&router).await;
    let check_id = create_check(&router, &[]).await;

    for (label, params) in [
        (
            "a tag association",
            {
                let mut params = tag_fields(&[("Env", "prod")]);
                params.push(p("ClientToken", "token-a"));
                params
            },
        ),
        (
            "an instance association",
            {
                let mut params = instance_fields(std::slice::from_ref(&instance_id));
                params.push(p("ClientToken", "token-b"));
                params
            },
        ),
    ] {
        let (status, first) = associate(&router, &check_id, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first call must succeed, got {status}: {first}"
        );
        let (status, second) = associate(&router, &check_id, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the retry must succeed, got {status}: {second}"
        );
        assert_eq!(
            reported(label, &second, "successfulResultSet"),
            reported(label, &first, "successfulResultSet"),
            "case {label:?}: the retry must report what the first call did, got {second}"
        );
    }

    let tags = described_target_tags(&router, "after the retries", &check_id).await;
    assert_eq!(
        tags,
        owned_pairs(&[("Env", "prod")]),
        "the retry must store no second pair, got {tags:?}"
    );
    let (_, details) =
        described_application_status(&router, "after the retries", &instance_id).await;
    assert_eq!(
        details.len(),
        1,
        "the retry must store no second result for the instance, got {details:?}"
    );
}

/// The two `AssociationType` spellings the association response objects carry are the ones
/// the model documents — "valid values: EC2TAG and INSTANCE_ID" — rather than whatever this
/// emulator happens to put on the wire.
#[test]
fn the_reported_association_types_are_the_documented_spellings() {
    assert_eq!(
        ASSOCIATION_TYPE_EC2_TAG, "EC2TAG",
        "a tag association is reported as EC2TAG, got {ASSOCIATION_TYPE_EC2_TAG:?}"
    );
    assert_eq!(
        ASSOCIATION_TYPE_INSTANCE_ID, "INSTANCE_ID",
        "an instance association is reported as INSTANCE_ID, got {ASSOCIATION_TYPE_INSTANCE_ID:?}"
    );
}

/// "You can create a maximum of 50 tag associations for each application status check":
/// driven AT the documented ceiling, which is accepted and stored, and one pair past it,
/// which is refused — while a pair the check already holds takes no room at all.
#[tokio::test]
async fn the_documented_ceiling_bounds_how_many_tag_associations_one_check_holds() {
    let (router, _engine) = ec2_query::emulator();
    let check_id = create_check(&router, &[]).await;

    let at_the_ceiling: Vec<(String, String)> = (0
        ..MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS)
        .flat_map(|index| {
            let position = index + 1;
            vec![
                p(&format!("TargetTagAssociation.{position}.Key"), "Env"),
                p(
                    &format!("TargetTagAssociation.{position}.Value"),
                    format!("value-{index}"),
                ),
            ]
        })
        .collect();
    let (status, body) = associate(&router, &check_id, &at_the_ceiling).await;
    assert_eq!(
        status, 200,
        "the ceiling itself must be accepted, got {status}: {body}"
    );
    let stored = described_target_tags(&router, "at the ceiling", &check_id).await;
    assert_eq!(
        stored.len(),
        MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS,
        "the check must hold every pair of the ceiling, got {stored:?}"
    );

    // A pair the check already holds adds nothing, so it is still at the ceiling and not
    // past it.
    let (status, body) = associate(
        &router,
        &check_id,
        &[
            p("TargetTagAssociation.1.Key", "Env"),
            p("TargetTagAssociation.1.Value", "value-0"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a pair the check already holds must be accepted at the ceiling, got {status}: {body}"
    );

    let (status, body) = associate(
        &router,
        &check_id,
        &[
            p("TargetTagAssociation.1.Key", "Env"),
            p("TargetTagAssociation.1.Value", "one-too-many"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "one pair past the ceiling must be refused, got {status}: {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "ResourceLimitExceeded",
        "expected ResourceLimitExceeded, got {body}"
    );
    let message = ec2_query::error_message(&body);
    assert!(
        message.contains(&MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS.to_string()),
        "expected the refusal to name the ceiling of \
         {MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS}, got {message:?}"
    );
    let stored = described_target_tags(&router, "past the ceiling", &check_id).await;
    assert_eq!(
        stored.len(),
        MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS,
        "a refused associate must store nothing, got {stored:?}"
    );

    // The same bound on ONE request: a call naming more pairs than a check may hold can never
    // be carried out, whatever the check holds already.
    let fresh = create_check(&router, &[]).await;
    let one_too_many: Vec<(String, String)> = (0
        ..=MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS)
        .flat_map(|index| {
            let position = index + 1;
            vec![
                p(&format!("TargetTagAssociation.{position}.Key"), "Env"),
                p(
                    &format!("TargetTagAssociation.{position}.Value"),
                    format!("value-{index}"),
                ),
            ]
        })
        .collect();
    let (status, body) = associate(&router, &fresh, &one_too_many).await;
    assert_eq!(
        status, 400,
        "one request past the ceiling must be refused, got {status}: {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "ResourceLimitExceeded",
        "expected ResourceLimitExceeded, got {body}"
    );
    let stored = described_target_tags(&router, "one request past the ceiling", &fresh).await;
    assert!(
        stored.is_empty(),
        "a refused associate must store nothing, got {stored:?}"
    );
}

/// Every rule the operation publishes, refused with this service's own code: the subject id
#[tokio::test]
async fn bad_requests_are_refused_with_the_services_own_codes() {
    let (router, _engine) = ec2_query::emulator();
    let instance_id = launch_one(&router).await;
    let check_id = create_check(&router, &[]).await;

    /// One refusal row: the request, and the status, code and text it must answer with.
    struct Refusal {
        label: &'static str,
        check_id: String,
        params: Vec<(String, String)>,
        status: u16,
        code: &'static str,
        names: String,
    }

    let one_instance = instance_fields(std::slice::from_ref(&instance_id));
    let too_long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);

    let refusals = vec![
        // A value that is no application status check id at all, in each of the ways one can
        // fail to be one: another resource's prefix, a digit outside the hexadecimal
        // alphabet, a suffix of neither published length, and no prefix at all.
        Refusal {
            label: "a check id under another prefix",
            check_id: "appstatuscheck-0123456789abcdef0".to_string(),
            params: one_instance.clone(),
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "appstatuscheck-0123456789abcdef0".to_string(),
        },
        Refusal {
            label: "a check id carrying a digit outside the hexadecimal alphabet",
            check_id: "asc-0123456789abcdefg".to_string(),
            params: one_instance.clone(),
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "asc-0123456789abcdefg".to_string(),
        },
        Refusal {
            label: "a check id of neither published length",
            check_id: "asc-0123456".to_string(),
            params: one_instance.clone(),
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "asc-0123456".to_string(),
        },
        Refusal {
            label: "a value with no resource prefix at all",
            check_id: "0123456789abcdef0".to_string(),
            params: one_instance.clone(),
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "0123456789abcdef0".to_string(),
        },
        // The id's FORM is read before the rest of the request, so a request that is wrong in
        // two ways is answered for the id.
        Refusal {
            label: "a malformed check id beside both target members",
            check_id: "appstatuscheck-0123456789abcdef0".to_string(),
            params: {
                let mut params = one_instance.clone();
                params.extend(tag_fields(&[("Env", "prod")]));
                params
            },
            status: 400,
            code: "InvalidApplicationStatusCheckId.Malformed",
            names: "appstatuscheck-0123456789abcdef0".to_string(),
        },
        // A well-formed id of a check the account does not hold: the OTHER half of the pair,
        // answered before the rest of the request is read.
        Refusal {
            label: "a well-formed check id the account holds no check for",
            check_id: "asc-0123456789abcdef0".to_string(),
            params: one_instance.clone(),
            status: 400,
            code: "InvalidApplicationStatusCheckId.NotFound",
            names: "asc-0123456789abcdef0".to_string(),
        },
        Refusal {
            label: "an unknown check id with no target member at all",
            check_id: "asc-12345678".to_string(),
            params: Vec::new(),
            status: 400,
            code: "InvalidApplicationStatusCheckId.NotFound",
            names: "asc-12345678".to_string(),
        },
        // "You must specify either TargetTagAssociations or InstanceIds, but not both."
        Refusal {
            label: "both target members",
            check_id: check_id.clone(),
            params: {
                let mut params = one_instance.clone();
                params.extend(tag_fields(&[("Env", "prod")]));
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
            names: "InstanceIds".to_string(),
        },
        Refusal {
            label: "neither target member",
            check_id: check_id.clone(),
            params: Vec::new(),
            status: 400,
            code: "MissingParameter",
            names: "TargetTagAssociations".to_string(),
        },
        // "You must not leave tag keys blank."
        Refusal {
            label: "a blank tag key",
            check_id: check_id.clone(),
            params: tag_fields(&[("", "prod")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "tag key".to_string(),
        },
        Refusal {
            label: "a tag key one character above the documented maximum length",
            check_id: check_id.clone(),
            params: vec![
                p("TargetTagAssociation.1.Key", too_long_key.clone()),
                p("TargetTagAssociation.1.Value", "prod"),
            ],
            status: 400,
            code: "InvalidParameterValue",
            names: MAX_TAG_KEY_CHARS.to_string(),
        },
        Refusal {
            label: "a dry run of a request that would have succeeded",
            check_id: check_id.clone(),
            params: {
                let mut params = one_instance.clone();
                params.push(p("DryRun", "true"));
                params
            },
            status: 412,
            code: "DryRunOperation",
            names: "DryRun".to_string(),
        },
    ];

    for refusal in refusals {
        let label = refusal.label;
        let (status, body) = associate(&router, &refusal.check_id, &refusal.params).await;
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

    // Nothing any refused request named was stored.
    let tags = described_target_tags(&router, "after every refusal", &check_id).await;
    assert!(
        tags.is_empty(),
        "a refused associate must store no tag association, got {tags:?}"
    );
    let (instance_status, details) =
        described_application_status(&router, "after every refusal", &instance_id).await;
    assert!(
        details.is_empty(),
        "a refused associate must begin no monitoring, got {details:?}"
    );
    assert_eq!(
        instance_status,
        Some("not-applicable".to_string()),
        "an instance no check applies to must stay not-applicable"
    );
}
