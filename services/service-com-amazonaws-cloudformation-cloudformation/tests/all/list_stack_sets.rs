// SPDX-License-Identifier: CC0-1.0

//! `ListStackSets` over the awsQuery wire: the StackSets one Region's administrator account
//! holds.
//!
//! The rule every case here holds the service to: **`ListStackSets` reports one
//! `StackSetSummary` per StackSet the store PERSISTS for the administrator account and the
//! Region the request was signed for, in creation order, every member read off the stored
//! record, narrowed to the one `StackSetStatus` the request names when it names one, and one
//! bounded page at a time — and a read reports what the store holds at that instant without
//! moving it.**
//!
//! The reported-members table below is one row per kind of StackSet a store can hold: its
//! rows span both `StackSetStatus` values and both `PermissionModels` values
//! (`smithy_query(query="shape", shape="StackSetStatus,PermissionModels")`, which
//! `state::stack_sets::status` / `permission_model` name), a StackSet with a description and
//! one without, automatic deployment enabled, disabled and never configured, managed
//! execution on and off, and the three drift aggregates a StackSet's instances can add up to
//! (never checked, every instance in sync, one instance drifted).
//!
//! `CreateStackSet` and `UpdateStackSet` are the store's WRITERS a wire call can drive —
//! `DeleteStackSet` is still a scaffold with a stub issue of its own — so the rows
//! below, which span StackSet shapes no single call sequence produces, are seeded straight
//! into the store through
//! `state::ComAmazonawsCloudformationCloudformationState::record_stack_set`, the same device
//! `list_stack_set_operations.rs` and `list_stack_instances.rs` use. One case drives the real
//! `CreateStackSet` path instead, so what a created StackSet makes the list report is pinned
//! too. Every READ goes through the real request path (form-encoded body in, XML envelope
//! out), so what is asserted is what a client sees.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    permission_model, status, StoredAutoDeployment, StoredStackInstance, StoredStackSet,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, DRIFT_STATUS_DRIFTED, DRIFT_STATUS_IN_SYNC, DRIFT_STATUS_NOT_CHECKED, PAGE_SIZE,
};

use crate::harness::{
    call, call_in, member, router, router_with_engine, store, xml_blocks, xml_text, REGION,
};

/// Another Region, for the cases that hold a StackSet to the Region it was created in.
const OTHER_REGION: &str = "us-west-2";

/// The instant a drift-checked row's check ran at, with the text the modelled `Timestamp`
/// renders it as: a reported instant is compared against an exact string rather than against
/// a re-derivation of the formatting.
const CHECKED_AT: (u64, &str) = (1_700_000_000, "2023-11-14T22:13:20Z");

/// A template body, for the rows that carry one: nothing `StackSetSummary` reports is read
/// off it, so one body serves every row that has one.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// How a row's drift aggregate is seeded: which instances the StackSet holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Drift {
    /// No instance at all: "CloudFormation hasn't checked the StackSet for drift."
    NeverChecked,
    /// Every instance checked and matching: "All the stack instances belonging to the
    /// StackSet match the expected template and parameter configuration."
    EverySyncedInstance,
    /// One instance differing: "One or more of the stack instances belonging to the StackSet
    /// differs from the expected template and parameter configuration."
    OneDriftedInstance,
}

/// One row of the reported-members table: the StackSet the store holds, and every member the
/// summary must report for it.
struct Row {
    case: &'static str,
    stack_set: &'static str,
    description: Option<&'static str>,
    stack_set_status: &'static str,
    permission_model: &'static str,
    /// `AutoDeployment$Enabled` / `$RetainStacksOnAccountRemoval` as the record carries them,
    /// or `None` for a StackSet whose request configured no automatic deployment.
    auto_deployment: Option<(bool, bool)>,
    managed_execution_active: bool,
    drift: Drift,
}

/// The table: one StackSet per row, all in the same Region, so one read reports them all.
const ROWS: &[Row] = &[
    Row {
        case: "a self-managed StackSet with nothing configured",
        stack_set: "fleet-minimal",
        description: None,
        stack_set_status: status::ACTIVE,
        permission_model: permission_model::SELF_MANAGED,
        auto_deployment: None,
        managed_execution_active: false,
        drift: Drift::NeverChecked,
    },
    Row {
        case: "a described StackSet with managed execution on",
        stack_set: "fleet-described",
        description: Some("the fleet baseline"),
        stack_set_status: status::ACTIVE,
        permission_model: permission_model::SELF_MANAGED,
        auto_deployment: None,
        managed_execution_active: true,
        drift: Drift::NeverChecked,
    },
    Row {
        case: "service-managed permissions with automatic deployment enabled",
        stack_set: "fleet-auto-deploy-on",
        description: Some("deploys to new accounts of the OU"),
        stack_set_status: status::ACTIVE,
        permission_model: permission_model::SERVICE_MANAGED,
        auto_deployment: Some((true, true)),
        managed_execution_active: false,
        drift: Drift::EverySyncedInstance,
    },
    Row {
        case: "service-managed permissions with automatic deployment disabled",
        stack_set: "fleet-auto-deploy-off",
        description: None,
        stack_set_status: status::ACTIVE,
        permission_model: permission_model::SERVICE_MANAGED,
        auto_deployment: Some((false, false)),
        managed_execution_active: true,
        drift: Drift::OneDriftedInstance,
    },
    Row {
        case: "a StackSet whose instances have all been checked and match",
        stack_set: "fleet-in-sync",
        description: None,
        stack_set_status: status::ACTIVE,
        permission_model: permission_model::SELF_MANAGED,
        auto_deployment: None,
        managed_execution_active: false,
        drift: Drift::EverySyncedInstance,
    },
    Row {
        case: "a deleted StackSet",
        stack_set: "fleet-deleted",
        description: Some("decommissioned"),
        stack_set_status: status::DELETED,
        permission_model: permission_model::SELF_MANAGED,
        auto_deployment: None,
        managed_execution_active: false,
        drift: Drift::NeverChecked,
    },
];

/// One row as the record the store holds, in `region`.
fn stored(row: &Row, region: &str) -> StoredStackSet {
    let mut stack_set = StoredStackSet::new(row.stack_set, region);
    stack_set.description = row.description.map(str::to_string);
    stack_set.status = row.stack_set_status.to_string();
    stack_set.permission_model = row.permission_model.to_string();
    stack_set.template_body = Some(TEMPLATE.to_string());
    stack_set.auto_deployment = row.auto_deployment.map(|(enabled, retain)| StoredAutoDeployment {
        enabled: Some(enabled),
        retain_stacks_on_account_removal: Some(retain),
        depends_on: Vec::new(),
    });
    stack_set.managed_execution_active = row.managed_execution_active;

    let checked_at = SystemTime::UNIX_EPOCH + Duration::from_secs(CHECKED_AT.0);
    let mut instance = |account: &str, drift_status: &str| {
        let mut instance = StoredStackInstance::new(account, region, "operation-seeded");
        instance.drift_status = drift_status.to_string();
        instance.last_drift_check = Some(checked_at);
        stack_set.instances.push(instance);
    };
    match row.drift {
        Drift::NeverChecked => {}
        Drift::EverySyncedInstance => {
            instance("111111111111", DRIFT_STATUS_IN_SYNC);
            instance("222222222222", DRIFT_STATUS_IN_SYNC);
        }
        Drift::OneDriftedInstance => {
            instance("111111111111", DRIFT_STATUS_IN_SYNC);
            instance("222222222222", DRIFT_STATUS_DRIFTED);
        }
    }
    stack_set
}

/// What the row's drift aggregate must be reported as.
fn expected_drift(row: &Row) -> (&'static str, Option<&'static str>) {
    match row.drift {
        // "This value will be NULL for any StackSet that drift detection hasn't yet been
        // performed on."
        Drift::NeverChecked => (DRIFT_STATUS_NOT_CHECKED, None),
        Drift::EverySyncedInstance => (DRIFT_STATUS_IN_SYNC, Some(CHECKED_AT.1)),
        Drift::OneDriftedInstance => (DRIFT_STATUS_DRIFTED, Some(CHECKED_AT.1)),
    }
}

/// The `<member>` block of each summary one response reports, in wire order.
///
/// The `Summaries` list is read INSIDE its own block, so a `<member>` of another list the
/// envelope carries cannot be swept up with it.
fn summary_blocks(body: &str) -> Vec<String> {
    match xml_blocks(body, "Summaries").into_iter().next() {
        Some(block) => xml_blocks(&block, "member"),
        None => Vec::new(),
    }
}

/// The StackSet names one response reports, in the order it reports them.
fn reported_names(body: &str) -> Vec<String> {
    summary_blocks(body)
        .iter()
        .filter_map(|summary| xml_text(summary, "StackSetName"))
        .collect()
}

/// One response's summary for `stack_set`, or `None` when it reports none.
fn summary_for(body: &str, stack_set: &str) -> Option<String> {
    summary_blocks(body).into_iter().find(|summary| {
        xml_text(summary, "StackSetName").as_deref() == Some(stack_set)
    })
}

/// One `CreateStackSet` the list must reflect: what the row is, the StackSet it creates and
/// the members the call states beyond `StackSetName`.
struct Create {
    case: &'static str,
    stack_set_name: &'static str,
    request: Vec<(String, String)>,
}

/// `ListStackSets` in `region`, with whatever members the case names.
async fn list_stack_sets(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_in(router, region, "ListStackSets", members).await
}

#[tokio::test]
async fn every_member_is_read_off_the_stackset_record() {
    let (router, engine) = router_with_engine();
    let mut ids: HashMap<&str, String> = HashMap::new();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in ROWS {
            let stack_set = stored(row, REGION);
            ids.insert(row.stack_set, stack_set.stack_set_id.clone());
            held.record_stack_set(stack_set);
        }
    }

    let (http_status, body) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected 200, got {http_status} {body}"
    );

    for row in ROWS {
        let case = row.case;
        let summary = summary_for(&body, row.stack_set).unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary for {}, got {body}", row.stack_set)
        });
        assert_eq!(
            xml_text(&summary, "StackSetId").as_deref(),
            ids.get(row.stack_set).map(String::as_str),
            "case {case:?}: the id the store minted for the record, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "Description").as_deref(),
            row.description,
            "case {case:?}: the record's own description, absent when it carries none, got \
             {summary}"
        );
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some(row.stack_set_status),
            "case {case:?}: the record's own status, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "PermissionModel").as_deref(),
            Some(row.permission_model),
            "case {case:?}: the record's own permission model, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "Active").as_deref(),
            Some(if row.managed_execution_active {
                "true"
            } else {
                "false"
            }),
            "case {case:?}: managed execution as the record carries it, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "Enabled"),
            row.auto_deployment
                .map(|(enabled, _)| enabled.to_string()),
            "case {case:?}: automatic deployment as the record carries it, absent when the \
             record configures none, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "RetainStacksOnAccountRemoval"),
            row.auto_deployment.map(|(_, retain)| retain.to_string()),
            "case {case:?}: the retention the record carries, got {summary}"
        );
        let (drift_status, last_check) = expected_drift(row);
        assert_eq!(
            xml_text(&summary, "DriftStatus").as_deref(),
            Some(drift_status),
            "case {case:?}: the drift aggregate of the record's instances, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "LastDriftCheckTimestamp").as_deref(),
            last_check,
            "case {case:?}: the most recent check of the record's instances, got {summary}"
        );

        // The same record read through `DescribeStackSet`: the two reads cannot disagree
        // about the StackSet's identity, status, permission model or drift.
        let (http_status, described) = call(
            &router,
            "DescribeStackSet",
            &[member("StackSetName", row.stack_set)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSet to answer 200, got {http_status} \
             {described}"
        );
        for tag in [
            "StackSetId",
            "StackSetName",
            "Status",
            "PermissionModel",
            "DriftStatus",
        ] {
            assert_eq!(
                xml_text(&summary, tag),
                xml_text(&described, tag),
                "case {case:?}: {tag} must agree with DescribeStackSet, got {summary} and \
                 {described}"
            );
        }
    }
}

#[tokio::test]
async fn a_created_stackset_is_reported_by_the_list() {
    let router = router();

    // Each row is a `CreateStackSet` the list must then reflect, in the order the calls
    // created them.
    let creates: Vec<Create> = vec![
        Create {
            case: "the minimal create",
            stack_set_name: "set-created-minimal",
            request: vec![member("TemplateBody", TEMPLATE)],
        },
        Create {
            case: "a described, service-managed create with automatic deployment",
            stack_set_name: "set-created-service-managed",
            request: vec![
                member("TemplateBody", TEMPLATE),
                member("Description", "created over the wire"),
                member("PermissionModel", permission_model::SERVICE_MANAGED),
                member("AutoDeployment.Enabled", "true"),
                member("AutoDeployment.RetainStacksOnAccountRemoval", "false"),
                member("ManagedExecution.Active", "true"),
            ],
        },
    ];

    // Before any create, the Region holds no StackSet at all.
    let (http_status, body) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(http_status, 200, "expected 200, got {http_status} {body}");
    assert_eq!(
        reported_names(&body),
        Vec::<String>::new(),
        "a Region holding no StackSet reports none, got {body}"
    );
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "an empty collection carries no token, got {body}"
    );

    let mut created_ids: Vec<String> = Vec::new();
    for Create {
        case,
        stack_set_name,
        request,
    } in &creates
    {
        let mut members = vec![member("StackSetName", stack_set_name)];
        members.extend_from_slice(request);
        let (http_status, created) = call(&router, "CreateStackSet", &members).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStackSet to answer 200, got {http_status} {created}"
        );
        created_ids.push(xml_text(&created, "StackSetId").unwrap_or_else(|| {
            panic!("case {case:?}: expected CreateStackSet to answer an id, got {created}")
        }));
    }

    let (http_status, body) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(http_status, 200, "expected 200, got {http_status} {body}");
    let expected_names: Vec<String> = creates
        .iter()
        .map(|create| create.stack_set_name.to_string())
        .collect();
    assert_eq!(
        reported_names(&body),
        expected_names,
        "the created StackSets, in the order they were created, got {body}"
    );
    for (create, created_id) in creates.iter().zip(&created_ids) {
        let (case, stack_set_name) = (create.case, create.stack_set_name);
        let summary = summary_for(&body, stack_set_name).unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary for {stack_set_name}, got {body}")
        });
        assert_eq!(
            xml_text(&summary, "StackSetId").as_deref(),
            Some(created_id.as_str()),
            "case {case:?}: the id the create answered, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some(status::ACTIVE),
            "case {case:?}: a StackSet that exists is ACTIVE, got {summary}"
        );
    }

    // The configuration the second create stated is what its summary reports.
    let summary = summary_for(&body, "set-created-service-managed")
        .unwrap_or_else(|| panic!("expected a summary for the service-managed create, got {body}"));
    for (tag, value) in [
        ("Description", "created over the wire"),
        ("PermissionModel", permission_model::SERVICE_MANAGED),
        ("Enabled", "true"),
        ("RetainStacksOnAccountRemoval", "false"),
        ("Active", "true"),
    ] {
        assert_eq!(
            xml_text(&summary, tag).as_deref(),
            Some(value),
            "case \"the configuration the create stated\": {tag} as the create stated it, got \
             {summary}"
        );
    }
}

#[tokio::test]
async fn the_status_filter_narrows_the_set_to_that_status() {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in ROWS {
            held.record_stack_set(stored(row, REGION));
        }
    }

    // The catalog the filter ranges over is the model's own `StackSetStatus`
    // (`smithy_query(query="shape", shape="StackSetStatus")`): ACTIVE and DELETED.
    for wanted in [status::ACTIVE, status::DELETED] {
        let (http_status, body) =
            list_stack_sets(&router, REGION, &[member("Status", wanted)]).await;
        assert_eq!(
            http_status, 200,
            "case {wanted:?}: expected 200, got {http_status} {body}"
        );
        let expected: Vec<String> = ROWS
            .iter()
            .filter(|row| row.stack_set_status == wanted)
            .map(|row| row.stack_set.to_string())
            .collect();
        assert_eq!(
            reported_names(&body),
            expected,
            "case {wanted:?}: only the StackSets of that status, got {body}"
        );
        for summary in summary_blocks(&body) {
            assert_eq!(
                xml_text(&summary, "Status").as_deref(),
                Some(wanted),
                "case {wanted:?}: every reported summary carries the asked-for status, got \
                 {summary}"
            );
        }
    }

    // A request naming no status gets every StackSet of the Region, whatever its status.
    let (http_status, body) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(
        http_status, 200,
        "case \"no status\": expected 200, got {http_status} {body}"
    );
    let every: Vec<String> = ROWS.iter().map(|row| row.stack_set.to_string()).collect();
    assert_eq!(
        reported_names(&body),
        every,
        "case \"no status\": every StackSet of the Region, got {body}"
    );
}

#[tokio::test]
async fn the_stacksets_reported_are_the_ones_of_the_requests_region() {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        // The SAME names in two Regions, plus one name only the other Region holds: a
        // StackSet name is unique per Region, so each Region has its own StackSets.
        for row in ROWS {
            held.record_stack_set(stored(row, REGION));
        }
        for row in &ROWS[..2] {
            held.record_stack_set(stored(row, OTHER_REGION));
        }
        held.record_stack_set(StoredStackSet::new("fleet-elsewhere-only", OTHER_REGION));
    }

    let (http_status, here) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(
        http_status, 200,
        "case \"the signed-for Region\": expected 200, got {http_status} {here}"
    );
    let expected: Vec<String> = ROWS.iter().map(|row| row.stack_set.to_string()).collect();
    assert_eq!(
        reported_names(&here),
        expected,
        "case \"the signed-for Region\": that Region's own StackSets, got {here}"
    );

    let (http_status, there) = list_stack_sets(&router, OTHER_REGION, &[]).await;
    assert_eq!(
        http_status, 200,
        "case \"another Region\": expected 200, got {http_status} {there}"
    );
    let expected: Vec<String> = ROWS[..2]
        .iter()
        .map(|row| row.stack_set.to_string())
        .chain(["fleet-elsewhere-only".to_string()])
        .collect();
    assert_eq!(
        reported_names(&there),
        expected,
        "case \"another Region\": that Region's own StackSets, got {there}"
    );

    // The two Regions' records of the same name are different StackSets, with ids of their
    // own.
    let here_id = summary_for(&here, ROWS[0].stack_set).and_then(|summary| {
        xml_text(&summary, "StackSetId")
    });
    let there_id = summary_for(&there, ROWS[0].stack_set).and_then(|summary| {
        xml_text(&summary, "StackSetId")
    });
    assert!(
        here_id.is_some() && there_id.is_some() && here_id != there_id,
        "case \"one name in two Regions\": each Region holds a StackSet of its own, got \
         {here_id:?} and {there_id:?}"
    );

    // A Region holding nothing reports the empty list rather than another Region's records.
    let empty_region = "eu-central-1";
    let (http_status, body) = list_stack_sets(&router, empty_region, &[]).await;
    assert_eq!(
        http_status, 200,
        "case \"a Region holding none\": expected 200, got {http_status} {body}"
    );
    assert_eq!(
        reported_names(&body),
        Vec::<String>::new(),
        "case \"a Region holding none\": no StackSet, got {body}"
    );
}

/// How many StackSets the paginated case seeds: more than one default page, so the default
/// read must hand out a token and the resumed read must finish the collection.
const PAGINATED_STACK_SETS: usize = PAGE_SIZE + 23;

/// The name of the `index`th seeded StackSet, numbered so creation order is also name order.
fn paginated_stack_set_name(index: usize) -> String {
    format!("fleet-{index:05}")
}

#[tokio::test]
async fn the_collection_is_paged_and_a_token_resumes_just_past_the_page() {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..PAGINATED_STACK_SETS {
            held.record_stack_set(StoredStackSet::new(
                paginated_stack_set_name(index),
                REGION,
            ));
        }
    }

    let expected: Vec<String> = (0..PAGINATED_STACK_SETS)
        .map(paginated_stack_set_name)
        .collect();

    // A request naming no `MaxResults` gets this service's own page size, and a token.
    let (http_status, first) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(
        http_status, 200,
        "case \"the default page\": expected 200, got {http_status} {first}"
    );
    assert_eq!(
        reported_names(&first),
        expected[..PAGE_SIZE],
        "case \"the default page\": the first {PAGE_SIZE} StackSets, got {first}"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the default page\": expected a NextToken, got {first}")
    });

    let (http_status, second) =
        list_stack_sets(&router, REGION, &[member("NextToken", &token)]).await;
    assert_eq!(
        http_status, 200,
        "case \"the resumed page\": expected 200, got {http_status} {second}"
    );
    assert_eq!(
        reported_names(&second),
        expected[PAGE_SIZE..],
        "case \"the resumed page\": the rest of the collection, got {second}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no token, got {second}"
    );

    // `MaxResults` is "the maximum number of results to be returned with a single call": a
    // walk in pages of that size reports the whole collection exactly once, in order, and a
    // caller may change the maximum between calls. The rows are the modelled range's own
    // bounds (`MaxResults`, range 1..100) and a size between them.
    for max_results in [1usize, 7, PAGE_SIZE] {
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut members = vec![member("MaxResults", &max_results.to_string())];
            if let Some(token) = &next {
                members.push(member("NextToken", token));
            }
            let (http_status, body) = list_stack_sets(&router, REGION, &members).await;
            assert_eq!(
                http_status, 200,
                "case \"MaxResults {max_results}\": expected 200, got {http_status} {body}"
            );
            let page = reported_names(&body);
            assert!(
                page.len() <= max_results,
                "case \"MaxResults {max_results}\": a page may not exceed the maximum, got {} \
                 in {body}",
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

    // A `MaxResults` past the modelled range is refused before the handler runs, so no page
    // is answered for it at all.
    for (case, max_results) in [("below the modelled minimum", "0"), ("above the modelled maximum", "101")] {
        let (http_status, body) =
            list_stack_sets(&router, REGION, &[member("MaxResults", max_results)]).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected 400, got {http_status} {body}"
        );
    }

    // A token this service did not issue for THIS collection is a bad request, not an empty
    // page: one past the end of the collection, and one issued for another collection.
    let beyond_the_end =
        state::encode_next_token(state::PageOf::StackSetSummaries, PAGINATED_STACK_SETS + 1);
    let other_collection = state::encode_next_token(state::PageOf::StackSetOperationSummaries, 0);
    for (case, token) in [
        ("a cursor past the end of the collection", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
        ("a token this service never issued", "not-a-token".to_string()),
    ] {
        let (http_status, body) =
            list_stack_sets(&router, REGION, &[member("NextToken", &token)]).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected 400, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("{token} is not a valid NextToken").as_str()),
            "case {case:?}: expected the invalid-token refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_store_without_changing_it() {
    let (router, engine) = router_with_engine();
    let mut ids: Vec<String> = Vec::new();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in ROWS {
            let stack_set = stored(row, REGION);
            ids.push(stack_set.stack_set_id.clone());
            held.record_stack_set(stack_set);
        }
    }

    // Reading the list, however many times and whatever page size, reports the same records
    // and leaves the store holding exactly them.
    let (_, first) = list_stack_sets(&router, REGION, &[]).await;
    let (_, with_filter) =
        list_stack_sets(&router, REGION, &[member("Status", status::ACTIVE)]).await;
    let (http_status, second) = list_stack_sets(&router, REGION, &[]).await;
    assert_eq!(
        http_status, 200,
        "case \"the second read\": expected 200, got {http_status} {second}"
    );
    assert_eq!(
        reported_names(&second),
        reported_names(&first),
        "case \"the second read\": a read must not change what a later caller sees, got \
         {second}"
    );
    assert!(
        !reported_names(&with_filter).is_empty(),
        "case \"the filtered read\": the ACTIVE StackSets, got {with_filter}"
    );

    let state = store(&engine);
    let held = state.lock();
    let held_ids: Vec<String> = held
        .stack_sets_in(REGION)
        .map(|stack_set| stack_set.stack_set_id.clone())
        .collect();
    assert_eq!(
        held_ids, ids,
        "the store holds exactly the records it held before the reads, got {held_ids:?}"
    );
}
