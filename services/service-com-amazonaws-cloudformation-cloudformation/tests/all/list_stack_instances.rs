// SPDX-License-Identifier: CC0-1.0

//! `ListStackInstances` over the awsQuery wire: the stack instances a StackSet carries.
//!
//! The rule every case here holds the service to: **`ListStackInstances` reports the stack
//! instances the StackSet named by the request PERSISTS — resolved by name or by id within
//! the administrator account and Region the request was signed for — one summary per
//! instance with every member read off the instance record, narrowed by
//! `StackInstanceAccount`, `StackInstanceRegion` and each `Filters` entry together, in the
//! order the instances entered the StackSet and one bounded page at a time; a
//! `StackSetName` that resolves to no StackSet is refused with `StackSetNotFoundException`
//! rather than answered with an empty list.**
//!
//! The narrowing table below is one row per selection the request can express — none, each
//! of the two target members, each of the three modelled filter names, filters combined with
//! each other and with a target member, and selections nothing satisfies — over seeded
//! instances spanning three accounts, four target Regions, five detailed statuses, three
//! operation ids and four drift statuses, so a row's expected instances distinguish it from
//! every other row.
//!
//! The store's WRITERS are `CreateStackSet` and `CreateStackInstances`, each still a scaffold
//! with its own stub issue, so the records below are seeded straight into the store through
//! `state::ComAmazonawsCloudformationCloudformationState::record_stack_set` — the same device
//! `hook_result_invocations.rs` and `extension_registry.rs` use for records whose writers are
//! not available. Every READ goes through the real request path (form-encoded body in, XML
//! envelope out), so what is asserted is what a client sees.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, filter_name, instance_status, StoredStackInstance, StoredStackSet,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, DRIFT_STATUS_DRIFTED, DRIFT_STATUS_IN_SYNC, DRIFT_STATUS_NOT_CHECKED,
    DRIFT_STATUS_UNKNOWN, PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router_with_engine, store, xml_blocks, xml_text, ACCOUNT_ID, REGION,
};

/// The StackSet the narrowing table reads, held by the administrator account in [`REGION`].
const STACK_SET: &str = "fleet-baseline";

/// A second StackSet of the same account and Region, carrying no instances: a StackSet that
/// exists answers the empty list, which is a different answer from one that does not exist.
const EMPTY_STACK_SET: &str = "fleet-unused";

/// Another Region's StackSet, created with the SAME name as [`STACK_SET`]: a StackSet name is
/// unique per administrator Region, so this is a second StackSet with its own instances and a
/// read signed for one Region must not resolve the other's.
const OTHER_REGION: &str = "us-west-2";

/// The three target accounts the seeded instances are deployed to (`Account` is
/// `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";
const ACCOUNT_C: &str = "333333333333";

/// The three StackSet operations the seeded instances were last touched by
/// (`LastOperationId` is a `ClientRequestToken`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`).
const OPERATION_ONE: &str = "operation-one";
const OPERATION_TWO: &str = "operation-two";
const OPERATION_THREE: &str = "operation-three";

/// An `OrganizationalUnitId`, `@pattern("^(ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}|r-[a-z0-9]{4,32})$")`:
/// the one seeded instance deployed through service-managed permissions carries one.
const ORGANIZATIONAL_UNIT: &str = "ou-abcd-12345678";

/// The instant the one drift-checked instance was last checked at.
fn last_drift_check() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// That instant as the modelled timestamp renders it, for the row that reads it back.
const LAST_DRIFT_CHECK_TEXT: &str = "2023-11-14T22:13:20Z";

/// The 8-4-4-4-12 suffix of a deployed instance's stack id.
const INSTANCE_STACK_SUFFIX: &str = "11111111-2222-4333-8444-555555555555";

/// A deployed instance's stack id, in the form CloudFormation gives the stack it creates in
/// the target account (`StackSet-<stack set name>-<id>`).
fn instance_stack_id(account: &str, region: &str) -> String {
    format!(
        "arn:aws:cloudformation:{region}:{account}:stack/StackSet-{STACK_SET}-\
         {INSTANCE_STACK_SUFFIX}/{INSTANCE_STACK_SUFFIX}"
    )
}

/// One seeded instance: the account and Region it is deployed to, its pair of statuses, the
/// operation that last touched it and its drift status.
struct Seeded {
    account: &'static str,
    region: &'static str,
    status: &'static str,
    detailed_status: &'static str,
    operation_id: &'static str,
    drift_status: &'static str,
}

/// The instances [`STACK_SET`] carries, in the order they entered it.
///
/// They span three accounts, four target Regions, five of the eight modelled
/// `StackInstanceDetailedStatus` values, all three `StackInstanceStatus` values, three
/// operation ids and four drift statuses, so every row of the narrowing table below selects a
/// distinguishable subset.
const SEEDED: &[Seeded] = &[
    Seeded {
        account: ACCOUNT_A,
        region: "us-east-1",
        status: instance_status::CURRENT,
        detailed_status: detailed_status::SUCCEEDED,
        operation_id: OPERATION_ONE,
        drift_status: DRIFT_STATUS_IN_SYNC,
    },
    Seeded {
        account: ACCOUNT_A,
        region: "us-west-2",
        status: instance_status::OUTDATED,
        detailed_status: detailed_status::FAILED,
        operation_id: OPERATION_ONE,
        drift_status: DRIFT_STATUS_NOT_CHECKED,
    },
    Seeded {
        account: ACCOUNT_B,
        region: "us-east-1",
        status: instance_status::CURRENT,
        detailed_status: detailed_status::SUCCEEDED,
        operation_id: OPERATION_TWO,
        drift_status: DRIFT_STATUS_DRIFTED,
    },
    Seeded {
        account: ACCOUNT_B,
        region: "eu-west-1",
        status: instance_status::OUTDATED,
        detailed_status: detailed_status::PENDING,
        operation_id: OPERATION_TWO,
        drift_status: DRIFT_STATUS_NOT_CHECKED,
    },
    Seeded {
        account: ACCOUNT_C,
        region: "us-east-1",
        status: instance_status::INOPERABLE,
        detailed_status: detailed_status::INOPERABLE,
        operation_id: OPERATION_THREE,
        drift_status: DRIFT_STATUS_UNKNOWN,
    },
    Seeded {
        account: ACCOUNT_C,
        region: "ap-southeast-2",
        status: instance_status::OUTDATED,
        detailed_status: detailed_status::CANCELLED,
        operation_id: OPERATION_THREE,
        drift_status: DRIFT_STATUS_NOT_CHECKED,
    },
];

/// How a case names one instance: the account and Region pair that identifies it within its
/// StackSet.
fn instance_key(account: &str, region: &str) -> String {
    format!("{account}/{region}")
}

/// The instances one response reports, in the order it reports them.
///
/// Each `<member>` of `<Summaries>` is read as a block and its own `Account` and `Region`
/// taken from inside it, so two summaries cannot be mixed up with each other.
fn reported_instances(body: &str) -> Vec<String> {
    xml_blocks(body, "member")
        .iter()
        .filter_map(|block| {
            let account = xml_text(block, "Account")?;
            let region = xml_text(block, "Region")?;
            Some(instance_key(&account, &region))
        })
        .collect()
}

/// Every seeded instance, in seeding order.
fn every_instance() -> Vec<String> {
    SEEDED
        .iter()
        .map(|seeded| instance_key(seeded.account, seeded.region))
        .collect()
}

/// The seeded instances of the accounts and Regions `keys` names, in seeding order.
fn instances(keys: &[(&str, &str)]) -> Vec<String> {
    keys.iter()
        .map(|(account, region)| instance_key(account, region))
        .collect()
}

/// One seeded record as it enters the store, with the optional members only some instances
/// carry filled in: a deployed instance has a stack and (once checked) a drift instant, a
/// failed one has a reason, and the one deployed through service-managed permissions has an
/// organizational unit.
fn stored_instance(seeded: &Seeded) -> StoredStackInstance {
    let mut instance =
        StoredStackInstance::new(seeded.account, seeded.region, seeded.operation_id);
    instance.status = seeded.status.to_string();
    instance.detailed_status = seeded.detailed_status.to_string();
    instance.drift_status = seeded.drift_status.to_string();
    if seeded.detailed_status == detailed_status::SUCCEEDED {
        instance.stack_id = Some(instance_stack_id(seeded.account, seeded.region));
    }
    if seeded.detailed_status == detailed_status::FAILED {
        instance.status_reason = Some("The stack could not be created".to_string());
    }
    if seeded.drift_status != DRIFT_STATUS_NOT_CHECKED {
        instance.last_drift_check = Some(last_drift_check());
    }
    if seeded.account == ACCOUNT_B && seeded.region == "us-east-1" {
        instance.organizational_unit_id = Some(ORGANIZATIONAL_UNIT.to_string());
    }
    instance
}

/// The router with the seeded StackSets already in its store, and the id the store minted for
/// [`STACK_SET`] so a case can address it by id as well as by name.
fn router_with_seeded_stack_sets() -> (MultiServiceRouter, String) {
    let (router, engine) = router_with_engine();
    let stack_set_id;
    {
        let state = store(&engine);
        let mut held = state.lock();

        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        for seeded in SEEDED {
            stack_set.record_instance(stored_instance(seeded));
        }
        stack_set_id = stack_set.stack_set_id.clone();
        held.record_stack_set(stack_set);

        // A StackSet of the same account and Region with no instances at all.
        held.record_stack_set(StoredStackSet::new(EMPTY_STACK_SET, REGION));

        // Another Region's StackSet of the SAME name, with one instance of its own.
        let mut elsewhere = StoredStackSet::new(STACK_SET, OTHER_REGION);
        elsewhere.record_instance(StoredStackInstance::new(
            ACCOUNT_A,
            OTHER_REGION,
            OPERATION_ONE,
        ));
        held.record_stack_set(elsewhere);
    }
    (router, stack_set_id)
}

/// One row of the narrowing table: the case name, the members the request carries beyond
/// `StackSetName`, and the instances the response must report.
type NarrowingCase = (&'static str, Vec<(String, String)>, Vec<String>);

/// `Filters.member.<index>.{Name,Values}`, as an AWS SDK form-encodes one filter.
fn filter(index: usize, name: &str, values: &str) -> Vec<(String, String)> {
    vec![
        member(&format!("Filters.member.{index}.Name"), name),
        member(&format!("Filters.member.{index}.Values"), values),
    ]
}

#[tokio::test]
async fn each_narrowing_reports_the_instances_it_names() {
    let (router, stack_set_id) = router_with_seeded_stack_sets();

    let cases: Vec<NarrowingCase> = vec![
        (
            "no narrowing: every instance the StackSet carries",
            vec![member("StackSetName", STACK_SET)],
            every_instance(),
        ),
        (
            "the StackSet addressed by its unique id rather than its name",
            vec![member("StackSetName", &stack_set_id)],
            every_instance(),
        ),
        (
            "StackInstanceAccount: one account's instances",
            vec![
                member("StackSetName", STACK_SET),
                member("StackInstanceAccount", ACCOUNT_A),
            ],
            instances(&[(ACCOUNT_A, "us-east-1"), (ACCOUNT_A, "us-west-2")]),
        ),
        (
            "StackInstanceAccount: another account's instances",
            vec![
                member("StackSetName", STACK_SET),
                member("StackInstanceAccount", ACCOUNT_C),
            ],
            instances(&[(ACCOUNT_C, "us-east-1"), (ACCOUNT_C, "ap-southeast-2")]),
        ),
        (
            "StackInstanceRegion: one target Region's instances",
            vec![
                member("StackSetName", STACK_SET),
                member("StackInstanceRegion", "us-east-1"),
            ],
            instances(&[
                (ACCOUNT_A, "us-east-1"),
                (ACCOUNT_B, "us-east-1"),
                (ACCOUNT_C, "us-east-1"),
            ]),
        ),
        (
            "StackInstanceRegion: a Region one instance is deployed to",
            vec![
                member("StackSetName", STACK_SET),
                member("StackInstanceRegion", "ap-southeast-2"),
            ],
            instances(&[(ACCOUNT_C, "ap-southeast-2")]),
        ),
        (
            "StackInstanceAccount and StackInstanceRegion: one instance",
            vec![
                member("StackSetName", STACK_SET),
                member("StackInstanceAccount", ACCOUNT_A),
                member("StackInstanceRegion", "us-west-2"),
            ],
            instances(&[(ACCOUNT_A, "us-west-2")]),
        ),
        (
            "StackInstanceAccount and StackInstanceRegion: a pair with no instance",
            vec![
                member("StackSetName", STACK_SET),
                member("StackInstanceAccount", ACCOUNT_A),
                member("StackInstanceRegion", "eu-west-1"),
            ],
            vec![],
        ),
        (
            "DETAILED_STATUS: the instances in that status",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::SUCCEEDED),
            ]
            .concat(),
            instances(&[(ACCOUNT_A, "us-east-1"), (ACCOUNT_B, "us-east-1")]),
        ),
        (
            "DETAILED_STATUS: another status, carried by one instance",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::PENDING),
            ]
            .concat(),
            instances(&[(ACCOUNT_B, "eu-west-1")]),
        ),
        (
            "DETAILED_STATUS: a modelled status no instance carries",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(
                    1,
                    filter_name::DETAILED_STATUS,
                    detailed_status::SKIPPED_SUSPENDED_ACCOUNT,
                ),
            ]
            .concat(),
            vec![],
        ),
        (
            "LAST_OPERATION_ID: the instances one operation last touched",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::LAST_OPERATION_ID, OPERATION_TWO),
            ]
            .concat(),
            instances(&[(ACCOUNT_B, "us-east-1"), (ACCOUNT_B, "eu-west-1")]),
        ),
        (
            "LAST_OPERATION_ID: an operation that touched no instance",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::LAST_OPERATION_ID, "operation-never-run"),
            ]
            .concat(),
            vec![],
        ),
        (
            "DRIFT_STATUS: the drifted instance",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DRIFT_STATUS, DRIFT_STATUS_DRIFTED),
            ]
            .concat(),
            instances(&[(ACCOUNT_B, "us-east-1")]),
        ),
        (
            "DRIFT_STATUS: the instances no detection has run against",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DRIFT_STATUS, DRIFT_STATUS_NOT_CHECKED),
            ]
            .concat(),
            instances(&[
                (ACCOUNT_A, "us-west-2"),
                (ACCOUNT_B, "eu-west-1"),
                (ACCOUNT_C, "ap-southeast-2"),
            ]),
        ),
        (
            "two filters: both must hold",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::SUCCEEDED),
                filter(2, filter_name::DRIFT_STATUS, DRIFT_STATUS_IN_SYNC),
            ]
            .concat(),
            instances(&[(ACCOUNT_A, "us-east-1")]),
        ),
        (
            "three filters, the most the model admits: all three must hold",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::SUCCEEDED),
                filter(2, filter_name::DRIFT_STATUS, DRIFT_STATUS_DRIFTED),
                filter(3, filter_name::LAST_OPERATION_ID, OPERATION_TWO),
            ]
            .concat(),
            instances(&[(ACCOUNT_B, "us-east-1")]),
        ),
        (
            "three filters no one instance satisfies together",
            [
                vec![member("StackSetName", STACK_SET)],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::SUCCEEDED),
                filter(2, filter_name::DRIFT_STATUS, DRIFT_STATUS_IN_SYNC),
                filter(3, filter_name::LAST_OPERATION_ID, OPERATION_TWO),
            ]
            .concat(),
            vec![],
        ),
        (
            "a filter and a target member: both narrow the same answer",
            [
                vec![
                    member("StackSetName", STACK_SET),
                    member("StackInstanceAccount", ACCOUNT_B),
                ],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::SUCCEEDED),
            ]
            .concat(),
            instances(&[(ACCOUNT_B, "us-east-1")]),
        ),
        (
            "a filter and a target member nothing satisfies together",
            [
                vec![
                    member("StackSetName", STACK_SET),
                    member("StackInstanceAccount", ACCOUNT_C),
                ],
                filter(1, filter_name::DETAILED_STATUS, detailed_status::SUCCEEDED),
            ]
            .concat(),
            vec![],
        ),
        (
            "a filter naming no value narrows nothing",
            vec![
                member("StackSetName", STACK_SET),
                member("Filters.member.1.Name", filter_name::DETAILED_STATUS),
            ],
            every_instance(),
        ),
        (
            "a StackSet of this Region that carries no instance at all",
            vec![member("StackSetName", EMPTY_STACK_SET)],
            vec![],
        ),
    ];

    for (case, members, expected) in &cases {
        let (status, body) = call(&router, "ListStackInstances", members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            &reported_instances(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds every match, so no NextToken, got {body}"
        );
    }

    // The read reports, it does not consume: every row answers the same instances the second
    // time round, and the unnarrowed read is unchanged by every narrowed one above.
    for (case, members, expected) in &cases {
        let (status, body) = call(&router, "ListStackInstances", members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 on the repeat read, got {status} {body}"
        );
        assert_eq!(
            &reported_instances(&body),
            expected,
            "case {case:?}: the repeat read must report {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn one_summary_carries_every_member_the_instance_record_holds() {
    let (router, stack_set_id) = router_with_seeded_stack_sets();

    // The one instance deployed through service-managed permissions, drifted and last
    // touched by the second operation: the record with the most members set.
    let (status, body) = call(
        &router,
        "ListStackInstances",
        &[
            member("StackSetName", STACK_SET),
            member("StackInstanceAccount", ACCOUNT_B),
            member("StackInstanceRegion", "us-east-1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"one instance's members\": expected 200, got {status} {body}"
    );

    for (tag, expected) in [
        ("StackSetId", stack_set_id.clone()),
        ("Account", ACCOUNT_B.to_string()),
        ("Region", "us-east-1".to_string()),
        ("StackId", instance_stack_id(ACCOUNT_B, "us-east-1")),
        ("Status", instance_status::CURRENT.to_string()),
        ("DetailedStatus", detailed_status::SUCCEEDED.to_string()),
        ("DriftStatus", DRIFT_STATUS_DRIFTED.to_string()),
        ("LastDriftCheckTimestamp", LAST_DRIFT_CHECK_TEXT.to_string()),
        ("LastOperationId", OPERATION_TWO.to_string()),
        ("OrganizationalUnitId", ORGANIZATIONAL_UNIT.to_string()),
    ] {
        assert_eq!(
            xml_text(&body, tag).as_deref(),
            Some(expected.as_str()),
            "case {tag:?}: expected {expected:?}, got {body}"
        );
    }
    // A member this instance does not carry is absent rather than empty.
    assert_eq!(
        xml_text(&body, "StatusReason"),
        None,
        "case \"StatusReason\": absent for an instance whose status has no reason, got {body}"
    );

    // The instance whose deployment failed carries its reason and no stack, and the one
    // nothing has drift checked carries no drift instant: each member comes off the record.
    let (status, failed) = call(
        &router,
        "ListStackInstances",
        &[
            member("StackSetName", STACK_SET),
            member("StackInstanceAccount", ACCOUNT_A),
            member("StackInstanceRegion", "us-west-2"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the failed instance\": expected 200, got {status} {failed}"
    );
    assert_eq!(
        xml_text(&failed, "StatusReason").as_deref(),
        Some("The stack could not be created"),
        "case \"StatusReason\": the record's own reason, got {failed}"
    );
    assert_eq!(
        xml_text(&failed, "Status").as_deref(),
        Some(instance_status::OUTDATED),
        "case \"Status\": the record's own status, got {failed}"
    );
    for tag in ["StackId", "LastDriftCheckTimestamp", "OrganizationalUnitId"] {
        assert_eq!(
            xml_text(&failed, tag),
            None,
            "case {tag:?}: absent for an instance that carries none, got {failed}"
        );
    }
}

#[tokio::test]
async fn a_stackset_this_region_does_not_hold_is_refused() {
    let (router, stack_set_id) = router_with_seeded_stack_sets();

    // Each row names a `StackSetName` that resolves to no StackSet of the administrator
    // account and Region the request is signed for.
    let cases: Vec<(&str, String)> = vec![
        ("a name no StackSet carries", "no-such-stack-set".to_string()),
        (
            "an id no StackSet carries",
            format!("{STACK_SET}:00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an instance's account, which addresses no StackSet",
            ACCOUNT_A.to_string(),
        ),
        (
            "the name of a StackSet of this Region, with its case changed",
            STACK_SET.to_uppercase(),
        ),
    ];

    for (case, stack_set_name) in &cases {
        let (status, body) = call(
            &router,
            "ListStackInstances",
            &[member("StackSetName", stack_set_name)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("StackSetNotFoundException"),
            "case {case:?}: expected the modelled code, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("StackSet {stack_set_name} not found").as_str()),
            "case {case:?}: expected the StackSet-not-found message, got {body}"
        );
    }

    // A StackSet is addressable only in the Region it was created in: the id the store minted
    // for this Region's StackSet resolves nothing when the read is signed for another one,
    // while the NAME resolves that other Region's own StackSet and its own instances.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackInstances",
        &[member("StackSetName", &stack_set_id)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"another Region's StackSet id\": expected 404, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("StackSetNotFoundException"),
        "case \"another Region's StackSet id\": expected the modelled code, got {body}"
    );

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackInstances",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the same name in another Region\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_instances(&body),
        instances(&[(ACCOUNT_A, OTHER_REGION)]),
        "case \"the same name in another Region\": that Region's own instance, got {body}"
    );

    // And a StackSet of the signed-for Region is not reachable from another one either.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackInstances",
        &[member("StackSetName", EMPTY_STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"another Region's StackSet name\": expected 404, got {status} {body}"
    );
}

#[tokio::test]
async fn an_undeclared_filter_name_is_refused_before_the_handler() {
    let (router, _) = router_with_seeded_stack_sets();

    // `StackInstanceFilterName` admits exactly three values, so a fourth is a bad request
    // rather than a filter that selects nothing.
    let (status, body) = call(
        &router,
        "ListStackInstances",
        &[
            vec![member("StackSetName", STACK_SET)],
            filter(1, "NOT_A_FILTER_NAME", detailed_status::SUCCEEDED),
        ]
        .concat(),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an undeclared filter name\": expected 400, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"an undeclared filter name\": expected a refusal, got {body}"
    );
}

/// How many instances the paginated case seeds: more than one default page, so the default
/// read must hand out a token and the resumed read must finish the collection.
const PAGINATED_INSTANCES: usize = PAGE_SIZE + 23;

/// The router with ONE StackSet carrying [`PAGINATED_INSTANCES`] instances, one per account,
/// in seeding order — more than a bound's worth of wire calls could create.
fn router_with_a_paginated_stack_set() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        for index in 0..PAGINATED_INSTANCES {
            stack_set.record_instance(StoredStackInstance::new(
                paginated_account(index),
                "us-east-1",
                OPERATION_ONE,
            ));
        }
        held.record_stack_set(stack_set);
    }
    router
}

/// The account of the `index`th paginated instance (`Account` is `@pattern("^[0-9]{12}$")`).
fn paginated_account(index: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + index as u64)
}

#[tokio::test]
async fn the_collection_is_paged_and_a_token_resumes_just_past_the_page() {
    let router = router_with_a_paginated_stack_set();
    let expected: Vec<String> = (0..PAGINATED_INSTANCES)
        .map(|index| instance_key(&paginated_account(index), "us-east-1"))
        .collect();

    // A request naming no `MaxResults` gets this service's own page size, and a token.
    let (status, first) = call(
        &router,
        "ListStackInstances",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the default page\": expected 200, got {status} {first}"
    );
    assert_eq!(
        reported_instances(&first),
        &expected[..PAGE_SIZE],
        "case \"the default page\": the first {PAGE_SIZE} instances, got {first}"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the default page\": expected a NextToken, got {first}")
    });

    let (status, second) = call(
        &router,
        "ListStackInstances",
        &[
            member("StackSetName", STACK_SET),
            member("NextToken", &token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the resumed page\": expected 200, got {status} {second}"
    );
    assert_eq!(
        reported_instances(&second),
        &expected[PAGE_SIZE..],
        "case \"the resumed page\": the rest of the collection, got {second}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no token, got {second}"
    );

    // `MaxResults` is "the maximum number of results to be returned with a single call": a
    // walk in pages of that size reports the whole collection exactly once, in order, and a
    // caller may change the maximum between calls.
    for max_results in [1usize, 7, PAGE_SIZE] {
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("MaxResults", &max_results.to_string()),
            ];
            if let Some(token) = &next {
                members.push(member("NextToken", token));
            }
            let (status, body) = call(&router, "ListStackInstances", &members).await;
            assert_eq!(
                status, 200,
                "case \"MaxResults {max_results}\": expected 200, got {status} {body}"
            );
            let page = reported_instances(&body);
            assert!(
                page.len() <= max_results,
                "case \"MaxResults {max_results}\": a page may not exceed the maximum, got \
                 {} in {body}",
                page.len()
            );
            walked.extend(page);
            next = xml_text(&body, "NextToken");
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case \"MaxResults {max_results}\": the walk must report the collection once, got \
             {walked:?}"
        );
    }

    // A token this service did not issue for THIS collection is a bad request, not an empty
    // page: one past the end of the collection, and one issued for another collection.
    let beyond_the_end =
        state::encode_next_token(state::PageOf::StackInstanceSummaries, PAGINATED_INSTANCES + 1);
    let other_collection = state::encode_next_token(state::PageOf::StackSummaries, 0);
    for (case, token) in [
        ("a cursor past the end of the collection", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
        ("a token this service never issued", "not-a-token".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "ListStackInstances",
            &[
                member("StackSetName", STACK_SET),
                member("NextToken", &token),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("{token} is not a valid NextToken").as_str()),
            "case {case:?}: expected the invalid-token refusal, got {body}"
        );
    }
}

/// The emulator's own account id is not what an instance's account is: a stack instance is
/// deployed to a TARGET account the request names, so the summary reports the instance's
/// account rather than the account the call was signed for.
#[tokio::test]
async fn an_instance_reports_its_target_account_not_the_administrator_account() {
    let (router, _) = router_with_seeded_stack_sets();

    let (status, body) = call(
        &router,
        "ListStackInstances",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the target accounts\": expected 200, got {status} {body}"
    );
    assert!(
        !reported_instances(&body)
            .iter()
            .any(|key| key.starts_with(ACCOUNT_ID)),
        "case \"the target accounts\": no instance was deployed to {ACCOUNT_ID}, got {body}"
    );
}
