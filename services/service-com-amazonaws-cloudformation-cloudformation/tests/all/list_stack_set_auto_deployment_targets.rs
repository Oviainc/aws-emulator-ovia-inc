// SPDX-License-Identifier: CC0-1.0

//! `ListStackSetAutoDeploymentTargets` over the awsQuery wire: the deployment targets of a
//! StackSet.
//!
//! The rule every case here holds the service to: **`ListStackSetAutoDeploymentTargets`
//! reports the deployment targets the StackSet named by the request PERSISTS — resolved by
//! name or by id within the administrator account and Region the request was signed for —
//! one summary per organization root or organizational unit its stack instances are
//! deployed under, each carrying the distinct Regions it is deployed to there, both in
//! first-seen order and one bounded page at a time; a `StackSetName` that resolves to no
//! StackSet is refused with `StackSetNotFoundException` rather than answered with an empty
//! list, and a StackSet deployed under no organizational unit answers the empty list.**
//!
//! The table below is one row per shape of deployment a StackSet can hold: its rows differ
//! in how many targets the StackSet has (none, one, several), how many Regions each target
//! carries (one, several), both published forms of an `OrganizationalUnitId` (an
//! organization root and an OU — `smithy_query(query="shape",
//! shape="StackSetAutoDeploymentTargetSummary")` gives the member's
//! `^(ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}|r-[a-z0-9]{4,32})$`), a target reached from several
//! accounts in one Region, and a self-managed StackSet whose instances carry no OU at all.
//!
//! The store's remaining WRITERS of a service-managed deployment are scaffolds with stub
//! issues of their own, so the records below are seeded straight into the store through
//! `state::ComAmazonawsCloudformationCloudformationState::record_stack_set` — the same
//! device `list_stack_instances.rs` and `list_stack_set_operations.rs` use for records whose
//! writers are not available. Every READ goes through the real request path (form-encoded
//! body in, XML envelope out), so what is asserted is what a client sees.

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    permission_model, StoredStackInstance, StoredStackSet,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{self, PAGE_SIZE};

use crate::stack_lifecycle::{
    call, call_in, member, router_with_engine, store, xml_blocks, xml_text, xml_texts, REGION,
};

/// Another Region, for the cases that hold a StackSet to the Region it was created in.
const OTHER_REGION: &str = "us-west-2";

/// The StackSet operation the seeded instances were last touched by (`LastOperationId` is a
/// `ClientRequestToken`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`).
const OPERATION: &str = "operation-seeded";

/// One seeded stack instance: the account and Region its stack is deployed in, and the
/// organization root or OU it was deployed under (`None` for a self-managed deployment,
/// whose instances carry no `OrganizationalUnitId`).
struct Instance {
    account: &'static str,
    region: &'static str,
    organizational_unit: Option<&'static str>,
}

/// One row of the table: a StackSet, the instances it holds, and the targets the read must
/// report for it in the order it must report them.
struct Row {
    case: &'static str,
    stack_set: &'static str,
    /// `SERVICE_MANAGED` for a StackSet deployed through Organizations, `SELF_MANAGED` for
    /// one whose instances name accounts directly.
    permission_model: &'static str,
    instances: &'static [Instance],
    targets: &'static [(&'static str, &'static [&'static str])],
}

/// The rows, one StackSet each so a response carries exactly the targets under test.
const ROWS: &[Row] = &[
    Row {
        case: "one OU deployed to one Region",
        stack_set: "fleet-single-target",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[Instance {
            account: "111111111111",
            region: "us-east-1",
            organizational_unit: Some("ou-abcd-11111111"),
        }],
        targets: &[("ou-abcd-11111111", &["us-east-1"])],
    },
    Row {
        case: "one OU deployed to three Regions",
        stack_set: "fleet-many-regions",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[
            Instance {
                account: "111111111111",
                region: "us-east-1",
                organizational_unit: Some("ou-abcd-22222222"),
            },
            Instance {
                account: "111111111111",
                region: "eu-west-1",
                organizational_unit: Some("ou-abcd-22222222"),
            },
            Instance {
                account: "111111111111",
                region: "ap-southeast-2",
                organizational_unit: Some("ou-abcd-22222222"),
            },
        ],
        targets: &[(
            "ou-abcd-22222222",
            &["us-east-1", "eu-west-1", "ap-southeast-2"],
        )],
    },
    Row {
        case: "three OUs, each with Regions of its own",
        stack_set: "fleet-many-targets",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[
            Instance {
                account: "111111111111",
                region: "us-east-1",
                organizational_unit: Some("ou-abcd-33333333"),
            },
            Instance {
                account: "222222222222",
                region: "eu-west-1",
                organizational_unit: Some("ou-abcd-44444444"),
            },
            Instance {
                account: "222222222222",
                region: "eu-central-1",
                organizational_unit: Some("ou-abcd-44444444"),
            },
            Instance {
                account: "333333333333",
                region: "sa-east-1",
                organizational_unit: Some("ou-efgh-55555555"),
            },
        ],
        targets: &[
            ("ou-abcd-33333333", &["us-east-1"]),
            ("ou-abcd-44444444", &["eu-west-1", "eu-central-1"]),
            ("ou-efgh-55555555", &["sa-east-1"]),
        ],
    },
    Row {
        case: "the organization root as the target",
        stack_set: "fleet-organization-root",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[
            Instance {
                account: "111111111111",
                region: "us-east-2",
                organizational_unit: Some("r-abcd"),
            },
            Instance {
                account: "222222222222",
                region: "us-west-1",
                organizational_unit: Some("r-abcd"),
            },
        ],
        targets: &[("r-abcd", &["us-east-2", "us-west-1"])],
    },
    Row {
        case: "one OU reached from three accounts in the same Region",
        stack_set: "fleet-shared-region",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[
            Instance {
                account: "111111111111",
                region: "us-east-1",
                organizational_unit: Some("ou-abcd-66666666"),
            },
            Instance {
                account: "222222222222",
                region: "us-east-1",
                organizational_unit: Some("ou-abcd-66666666"),
            },
            Instance {
                account: "333333333333",
                region: "us-east-1",
                organizational_unit: Some("ou-abcd-66666666"),
            },
        ],
        // The Region is reported ONCE however many accounts the OU is deployed to there.
        targets: &[("ou-abcd-66666666", &["us-east-1"])],
    },
    Row {
        case: "an OU and the organization root beside it",
        stack_set: "fleet-root-and-ou",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[
            Instance {
                account: "444444444444",
                region: "ca-central-1",
                organizational_unit: Some("r-wxyz"),
            },
            Instance {
                account: "555555555555",
                region: "ap-northeast-1",
                organizational_unit: Some("ou-wxyz-77777777"),
            },
            Instance {
                account: "444444444444",
                region: "ap-northeast-1",
                organizational_unit: Some("r-wxyz"),
            },
        ],
        targets: &[
            ("r-wxyz", &["ca-central-1", "ap-northeast-1"]),
            ("ou-wxyz-77777777", &["ap-northeast-1"]),
        ],
    },
    Row {
        case: "a self-managed StackSet, whose instances carry no OU",
        stack_set: "fleet-self-managed",
        permission_model: permission_model::SELF_MANAGED,
        instances: &[
            Instance {
                account: "111111111111",
                region: "us-east-1",
                organizational_unit: None,
            },
            Instance {
                account: "222222222222",
                region: "eu-west-1",
                organizational_unit: None,
            },
        ],
        targets: &[],
    },
    Row {
        case: "a StackSet nothing is deployed for",
        stack_set: "fleet-undeployed",
        permission_model: permission_model::SERVICE_MANAGED,
        instances: &[],
        targets: &[],
    },
];

/// The StackSet a row describes, as the store holds it.
fn stored(row: &Row, region: &str) -> StoredStackSet {
    let mut stack_set = StoredStackSet::new(row.stack_set, region);
    stack_set.permission_model = row.permission_model.to_string();
    for seeded in row.instances {
        let mut instance =
            StoredStackInstance::new(seeded.account, seeded.region, OPERATION);
        instance.organizational_unit_id = seeded
            .organizational_unit
            .map(|unit| unit.to_string());
        stack_set.record_instance(instance);
    }
    stack_set
}

/// The row's expected targets, as [`reported_targets`] renders what the response carries.
fn expected_targets(row: &Row) -> Vec<(String, Vec<String>)> {
    row.targets
        .iter()
        .map(|(unit, regions)| {
            (
                (*unit).to_string(),
                regions.iter().map(|region| (*region).to_string()).collect(),
            )
        })
        .collect()
}

/// The targets one response reports, in the order it reports them: each summary's
/// `OrganizationalUnitId` paired with the `Regions` list beside it.
///
/// Both members are read as the document order of the whole `Summaries` element, so the
/// pairing follows the summaries rather than the order the members happen to be serialized
/// in within one summary.
fn reported_targets(body: &str) -> Vec<(String, Vec<String>)> {
    let summaries = xml_blocks(body, "Summaries");
    let Some(block) = summaries.first() else {
        return Vec::new();
    };
    let units = xml_texts(block, "OrganizationalUnitId");
    let regions: Vec<Vec<String>> = xml_blocks(block, "Regions")
        .iter()
        .map(|regions| xml_texts(regions, "member"))
        .collect();
    assert_eq!(
        units.len(),
        regions.len(),
        "every reported target carries both its OU and its Regions, got {body}"
    );
    units.into_iter().zip(regions).collect()
}

#[tokio::test]
async fn the_targets_a_stackset_is_deployed_under_are_reported() {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in ROWS {
            held.record_stack_set(stored(row, REGION));
        }
    }

    for row in ROWS {
        let case = row.case;
        let (status, body) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
            &[member("StackSetName", row.stack_set)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let reported = reported_targets(&body);
        assert_eq!(
            reported,
            expected_targets(row),
            "case {case:?}: the targets the StackSet's own deployments are under, got \
             {reported:?} in {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds the whole collection, so it carries no token, \
             got {body}"
        );

        // A read may not change what a later caller sees: the same request answers the
        // same targets.
        let (status, again) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
            &[member("StackSetName", row.stack_set)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 on the second read, got {status} {again}"
        );
        assert_eq!(
            reported_targets(&again),
            reported,
            "case {case:?}: a second read reports the same targets, got {again}"
        );
    }
}

#[tokio::test]
async fn the_targets_are_reported_for_the_stackset_the_request_resolves() {
    // `StackSetName` is "the name or unique ID of the StackSet", and a StackSet name is
    // unique per administrator Region, so the same name in another Region is a second
    // StackSet with deployments of its own.
    let here = &ROWS[2];
    let elsewhere = &ROWS[0];
    let (router, engine) = router_with_engine();
    let stack_set_id;
    {
        let state = store(&engine);
        let mut held = state.lock();
        let held_here = stored(here, REGION);
        stack_set_id = held_here.stack_set_id.clone();
        held.record_stack_set(held_here);

        // Another Region's StackSet of the SAME name, deployed under a target of its own.
        let mut other = stored(elsewhere, OTHER_REGION);
        other.stack_set_name = here.stack_set.to_string();
        held.record_stack_set(other);
    }

    for (case, name) in [
        ("the StackSet addressed by its name", here.stack_set.to_string()),
        ("the StackSet addressed by its id", stack_set_id.clone()),
    ] {
        let (status, body) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
            &[member("StackSetName", &name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_targets(&body),
            expected_targets(here),
            "case {case:?}: this Region's own targets, got {body}"
        );
    }

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackSetAutoDeploymentTargets",
        &[member("StackSetName", here.stack_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the same name in another Region\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_targets(&body),
        expected_targets(elsewhere),
        "case \"the same name in another Region\": that Region's own targets, got {body}"
    );

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackSetAutoDeploymentTargets",
        &[member("StackSetName", &stack_set_id)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"another Region's StackSet id\": expected 404, got {status} {body}"
    );
}

#[tokio::test]
async fn a_stackset_this_region_does_not_hold_is_refused() {
    let row = &ROWS[0];
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.record_stack_set(stored(row, REGION));
    }

    // Each case names a `StackSetName` that resolves to no StackSet of the administrator
    // account and Region the request is signed for.
    let cases: Vec<(&str, String)> = vec![
        ("a name no StackSet carries", "no-such-stack-set".to_string()),
        (
            "an id no StackSet carries",
            format!("{}:00000000-0000-4000-8000-000000000000", row.stack_set),
        ),
        (
            "the name of a StackSet of this Region, with its case changed",
            row.stack_set.to_uppercase(),
        ),
    ];
    for (case, stack_set_name) in &cases {
        let (status, body) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
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
}

/// How many targets the paginated case seeds: more than one default page, so the default
/// read must hand out a token and the resumed read must finish the collection.
const PAGINATED_TARGETS: usize = PAGE_SIZE + 17;

/// The `OrganizationalUnitId` of the `index`th seeded target, in the OU form of the
/// member's published pattern (`^(ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}|r-[a-z0-9]{4,32})$`).
fn paginated_unit(index: usize) -> String {
    format!("ou-page-{index:08}")
}

/// The target account of the `index`th seeded instance (`Account` is 12 digits).
fn paginated_account(index: usize) -> String {
    format!("{:012}", index + 1)
}

#[tokio::test]
async fn the_collection_is_paged_and_a_token_resumes_just_past_the_page() {
    const STACK_SET: &str = "fleet-paged";
    const TARGET_REGION: &str = "us-east-1";
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.permission_model = permission_model::SERVICE_MANAGED.to_string();
        for index in 0..PAGINATED_TARGETS {
            let mut instance = StoredStackInstance::new(
                paginated_account(index),
                TARGET_REGION,
                OPERATION,
            );
            instance.organizational_unit_id = Some(paginated_unit(index));
            stack_set.record_instance(instance);
        }
        held.record_stack_set(stack_set);
    }

    // First-seen order, which for these instances is the order they were deployed in.
    let expected: Vec<String> = (0..PAGINATED_TARGETS).map(paginated_unit).collect();

    // A request naming no `MaxResults` gets this service's own page size, and a token.
    let (status, first) = call(
        &router,
        "ListStackSetAutoDeploymentTargets",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the default page\": expected 200, got {status} {first}"
    );
    let page: Vec<String> = reported_targets(&first)
        .into_iter()
        .map(|(unit, _)| unit)
        .collect();
    assert_eq!(
        page,
        expected[..PAGE_SIZE],
        "case \"the default page\": the first {PAGE_SIZE} targets, got {first}"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the default page\": expected a NextToken, got {first}")
    });

    let (status, second) = call(
        &router,
        "ListStackSetAutoDeploymentTargets",
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
    let page: Vec<String> = reported_targets(&second)
        .into_iter()
        .map(|(unit, _)| unit)
        .collect();
    assert_eq!(
        page,
        expected[PAGE_SIZE..],
        "case \"the resumed page\": the rest of the collection, got {second}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no token, got {second}"
    );

    // `MaxResults` is "the maximum number of results to be returned with a single call"
    // (range 1..100): a walk in pages of that size reports the whole collection exactly
    // once, in order, and a caller may change the maximum between calls. Both published
    // bounds are walked, and one past the maximum is refused by the modelled constraint.
    for max_results in [1usize, 9, PAGE_SIZE] {
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
            let (status, body) =
                call(&router, "ListStackSetAutoDeploymentTargets", &members).await;
            assert_eq!(
                status, 200,
                "case \"MaxResults {max_results}\": expected 200, got {status} {body}"
            );
            let page: Vec<String> = reported_targets(&body)
                .into_iter()
                .map(|(unit, _)| unit)
                .collect();
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
            "case \"MaxResults {max_results}\": the walk must report the collection once, \
             got {walked:?}"
        );
    }

    for (case, max_results) in [
        ("one below the published minimum", "0"),
        ("one above the published maximum", "101"),
    ] {
        let (status, body) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
            &[
                member("StackSetName", STACK_SET),
                member("MaxResults", max_results),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside the modelled range is refused, got \
             {status} {body}"
        );
    }

    // A token this service did not issue for THIS collection is a bad request, not an empty
    // page: one past the end of the collection, and one issued for another collection.
    let beyond_the_end = state::encode_next_token(
        state::PageOf::StackSetAutoDeploymentTargetSummaries,
        PAGINATED_TARGETS + 1,
    );
    let other_collection = state::encode_next_token(state::PageOf::StackInstanceSummaries, 0);
    for (case, token) in [
        ("a cursor past the end of the collection", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
        ("a token this service never issued", "not-a-token".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
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

#[tokio::test]
async fn either_call_as_reads_the_same_administrator_account() {
    // `CallAs` chooses whether the caller is the management account or a delegated
    // administrator (`smithy_query(query="shape", shape="CallAs")`); this emulator holds
    // one administrator account's StackSets, so both values read the same targets.
    let row = &ROWS[2];
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.record_stack_set(stored(row, REGION));
    }

    for call_as in ["SELF", "DELEGATED_ADMIN"] {
        let (status, body) = call(
            &router,
            "ListStackSetAutoDeploymentTargets",
            &[
                member("StackSetName", row.stack_set),
                member("CallAs", call_as),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {call_as:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_targets(&body),
            expected_targets(row),
            "case {call_as:?}: the StackSet's own targets, got {body}"
        );
    }
}
