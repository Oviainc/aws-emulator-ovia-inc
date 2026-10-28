// SPDX-License-Identifier: CC0-1.0

//! `DeleteStackSet` over the awsQuery wire: deleting a StackSet of the administrator account.
//!
//! The rule every case here holds the service to: **`DeleteStackSet` deletes the StackSet the
//! request addresses — the record the administrator account holds under that name or that id
//! in the Region the call was signed for — so afterwards nothing resolves it any more, while
//! `ListStackSets` keeps reporting its summary under the `DELETED` status and its name is free
//! for a new StackSet; a StackSet that still holds a stack instance is refused with
//! `StackSetNotEmptyException` and one with a StackSet operation still in flight with
//! `OperationInProgressException`, and a request that addresses no StackSet at all is a
//! SUCCESS, since the operation models no not-found failure and there is nothing left to
//! delete.**
//!
//! The rows span the class that rule generalises over rather than one recorded request: a
//! StackSet addressed by its name and one addressed by its `StackSetId`, a minimal StackSet
//! and one created with a description, tags and capabilities, both `PermissionModels` values,
//! both `CallAs` values, a StackSet that never held an instance and one whose instances were
//! deleted first, instance shapes of one pair, two accounts in one Region and one account
//! across two Regions, and the four ways a request can address no StackSet (a name never
//! created, a name only another Region holds, a name a deletion already took and an id
//! nothing carries).
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out), so
//! what is asserted is what a client sees. The expected refusal wordings are spelled out here
//! rather than read back out of the implementation's own constants.

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in, member, router, wait_for_the_transition, xml_blocks, xml_text, REGION,
};

/// The template every StackSet here is created from: one topic, really created in Amazon SNS
/// by each instance's stack.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"instance-display"}}}}"#;

/// Two target accounts beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";

/// Another Region: the target Region of a deployment, and the Region a call is signed for in
/// the cases that hold a StackSet to the Region it was created in.
const WEST: &str = "us-west-2";

/// How a request ADDRESSES the StackSet it deletes: `StackSetName` is "the name or unique ID
/// of the StackSet that you're deleting".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Address {
    /// By the name the StackSet was created under.
    ByName,
    /// By the `StackSetId` the service minted for it.
    ById,
}

/// `CreateStackSet` with the shared template and whatever else the row states.
async fn create_stack_set(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("TemplateBody", TEMPLATE),
    ];
    members.extend_from_slice(extra);
    call(router, "CreateStackSet", &members).await
}

/// `DeleteStackSet` for one StackSet, signed for `region`, as the row's `CallAs` names it.
async fn delete_stack_set(
    router: &MultiServiceRouter,
    region: &str,
    name_or_id: &str,
    call_as: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackSetName", name_or_id)];
    if let Some(call_as) = call_as {
        members.push(member("CallAs", call_as));
    }
    call_in(router, region, "DeleteStackSet", &members).await
}

/// `DescribeStackSet` for one StackSet, by name or by id.
async fn describe_stack_set(
    router: &MultiServiceRouter,
    region: &str,
    name_or_id: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeStackSet",
        &[member("StackSetName", name_or_id)],
    )
    .await
}

/// `CreateStackInstances` for one product of accounts and Regions.
async fn create_stack_instances(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    members.extend(selection(accounts, regions));
    call(router, "CreateStackInstances", &members).await
}

/// `DeleteStackInstances` for one product of accounts and Regions, deleting their stacks.
async fn delete_stack_instances(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("RetainStacks", "false"),
    ];
    members.extend(selection(accounts, regions));
    call(router, "DeleteStackInstances", &members).await
}

/// The `Accounts` and `Regions` members selecting one product of targets.
fn selection(accounts: &[&str], regions: &[&str]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        members.push(member(&format!("Accounts.member.{}", index + 1), account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    members
}

/// `ListStackSets`, narrowed to one `StackSetStatus` when the case names one.
async fn list_stack_sets(
    router: &MultiServiceRouter,
    region: &str,
    status: Option<&str>,
) -> (u16, String) {
    let members = match status {
        Some(status) => vec![member("Status", status)],
        None => Vec::new(),
    };
    call_in(router, region, "ListStackSets", &members).await
}

/// One response's `StackSetSummary` for `stack_set_name`, or `None` when it reports none.
///
/// The `Summaries` list is read INSIDE its own block, so a `<member>` of another list the
/// envelope carries cannot be swept up with it.
fn summary_for(body: &str, stack_set_name: &str) -> Option<String> {
    let summaries = match xml_blocks(body, "Summaries").into_iter().next() {
        Some(block) => xml_blocks(&block, "member"),
        None => Vec::new(),
    };
    summaries
        .into_iter()
        .find(|summary| xml_text(summary, "StackSetName").as_deref() == Some(stack_set_name))
}

/// The `<Code>` a refusal was answered under, and its `<Message>`.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// The `StackSetId` the service minted for one StackSet, read back through `DescribeStackSet`.
async fn stack_set_id(case: &str, router: &MultiServiceRouter, stack_set_name: &str) -> String {
    let (http_status, body) = describe_stack_set(router, REGION, stack_set_name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackSet({stack_set_name}) to succeed, got \
         {http_status} {body}"
    );
    xml_text(&body, "StackSetId").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeStackSet answered no StackSetId: {body}")
    })
}

/// The `StackSetARN` one StackSet reports, which the single-flight refusal names.
async fn stack_set_arn(case: &str, router: &MultiServiceRouter, stack_set_name: &str) -> String {
    let (http_status, body) = describe_stack_set(router, REGION, stack_set_name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackSet({stack_set_name}) to succeed, got \
         {http_status} {body}"
    );
    xml_text(&body, "StackSetARN").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeStackSet answered no StackSetARN: {body}")
    })
}

/// One row of the deletion table: the StackSet it creates, how the deletion addresses it, and
/// the instances it deploys and takes back out before deleting the StackSet.
struct Row {
    case: &'static str,
    stack_set_name: &'static str,
    /// The `CreateStackSet` members beyond `StackSetName` and `TemplateBody`.
    create_extra: Vec<(String, String)>,
    address: Address,
    /// `CallAs`: "whether you are acting as an account administrator in the organization's
    /// management account or as a delegated administrator in a member account" — this
    /// emulator holds one administrator account's StackSets, so either value deletes from the
    /// same store.
    call_as: Option<&'static str>,
    /// The accounts and Regions deployed to and then deleted again before the StackSet is
    /// deleted, for a row whose StackSet held instances once.
    emptied_of: Option<(Vec<&'static str>, Vec<&'static str>)>,
}

/// The table: each row is a different StackSet, addressed differently, deleted the same way.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a StackSet that never held an instance, addressed by name",
            stack_set_name: "delete-minimal",
            create_extra: Vec::new(),
            address: Address::ByName,
            call_as: None,
            emptied_of: None,
        },
        Row {
            case: "a StackSet addressed by its StackSetId",
            stack_set_name: "delete-by-id",
            create_extra: Vec::new(),
            address: Address::ById,
            call_as: None,
            emptied_of: None,
        },
        Row {
            case: "a described, tagged and acknowledged StackSet",
            stack_set_name: "delete-configured",
            create_extra: vec![
                member("Description", "the fleet baseline"),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "prod"),
                member("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
            address: Address::ByName,
            call_as: Some("SELF"),
            emptied_of: None,
        },
        Row {
            case: "a service-managed StackSet deleted by a delegated administrator",
            stack_set_name: "delete-service-managed",
            create_extra: vec![member("PermissionModel", "SERVICE_MANAGED")],
            address: Address::ById,
            call_as: Some("DELEGATED_ADMIN"),
            emptied_of: None,
        },
        Row {
            case: "a StackSet whose one instance was deleted first",
            stack_set_name: "delete-emptied-one-pair",
            create_extra: Vec::new(),
            address: Address::ByName,
            call_as: None,
            emptied_of: Some((vec![ACCOUNT_A], vec![REGION])),
        },
        Row {
            case: "a StackSet whose instances in two Regions were deleted first",
            stack_set_name: "delete-emptied-two-regions",
            create_extra: Vec::new(),
            address: Address::ById,
            call_as: None,
            emptied_of: Some((vec![ACCOUNT_B], vec![REGION, WEST])),
        },
    ]
}

#[tokio::test]
async fn a_deleted_stackset_is_resolved_by_nothing_and_listed_as_deleted() {
    let router = router();

    for row in rows() {
        let case = row.case;
        let name = row.stack_set_name;
        let (http_status, body) = create_stack_set(&router, name, &row.create_extra).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStackSet to succeed, got {http_status} {body}"
        );
        let id = stack_set_id(case, &router, name).await;

        // A row whose StackSet held instances really deploys them and really takes them back
        // out, so the deletion below is of a StackSet the store emptied rather than of one
        // nothing was ever deployed to.
        if let Some((accounts, regions)) = &row.emptied_of {
            let (http_status, body) =
                create_stack_instances(&router, name, accounts, regions).await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected CreateStackInstances to succeed, got {http_status} \
                 {body}"
            );
            wait_for_the_transition().await;
            let (http_status, body) =
                delete_stack_instances(&router, name, accounts, regions).await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected DeleteStackInstances to succeed, got {http_status} \
                 {body}"
            );
            wait_for_the_transition().await;
        }

        let addressed = match row.address {
            Address::ByName => name.to_string(),
            Address::ById => id.clone(),
        };
        let (http_status, body) =
            delete_stack_set(&router, REGION, &addressed, row.call_as).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DeleteStackSet({addressed}) to succeed, got {http_status} \
             {body}"
        );
        // `DeleteStackSetOutput` models no member: the deletion is immediate, so the response
        // carries no operation for a caller to monitor and no refusal.
        assert_eq!(
            xml_text(&body, "OperationId"),
            None,
            "case {case:?}: expected no OperationId in the empty output, got {body}"
        );
        assert_eq!(
            refusal(&body),
            (None, None),
            "case {case:?}: expected a success envelope, got {body}"
        );

        // Nothing resolves the StackSet any more, by either of the two ways it was
        // addressable: "the specified StackSet doesn't exist".
        for probe in [name.to_string(), id.clone()] {
            let (http_status, body) = describe_stack_set(&router, REGION, &probe).await;
            assert_eq!(
                http_status, 404,
                "case {case:?}: expected DescribeStackSet({probe}) to be refused with 404, got \
                 {http_status} {body}"
            );
            let (code, message) = refusal(&body);
            assert_eq!(
                code.as_deref(),
                Some("StackSetNotFoundException"),
                "case {case:?}: expected the modelled refusal for {probe}, got {body}"
            );
            assert_eq!(
                message.as_deref(),
                Some(format!("StackSet {probe} not found").as_str()),
                "case {case:?}: expected the refusal to name {probe}, got {message:?}"
            );
        }

        // Its summary stays, under the status the model declares for a deleted StackSet, with
        // the id the store minted for it: "Summary information for stacks that have been
        // deleted is kept" is how this service reports what it no longer resolves.
        let (http_status, body) = list_stack_sets(&router, REGION, None).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected ListStackSets to succeed, got {http_status} {body}"
        );
        let summary = summary_for(&body, name).unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary for {name}, got {body}")
        });
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some("DELETED"),
            "case {case:?}: expected the deleted status, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "StackSetId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the id the store minted, got {summary}"
        );
        for (status, listed) in [("DELETED", true), ("ACTIVE", false)] {
            let (http_status, body) = list_stack_sets(&router, REGION, Some(status)).await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected ListStackSets({status}) to succeed, got {http_status} \
                 {body}"
            );
            assert_eq!(
                summary_for(&body, name).is_some(),
                listed,
                "case {case:?}: expected ListStackSets({status}) to report {name}: {listed}, \
                 got {body}"
            );
        }

        // A second deletion of the same StackSet has nothing left to delete, so it answers
        // exactly as the first did rather than refusing.
        let (http_status, body) =
            delete_stack_set(&router, REGION, &addressed, row.call_as).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected a repeated DeleteStackSet({addressed}) to succeed, got \
             {http_status} {body}"
        );

        // "The name must be unique in the Region where you create your StackSet" is about the
        // StackSets that exist: the deleted name is free, and the new StackSet is a different
        // record with an id of its own.
        let (http_status, body) = create_stack_set(&router, name, &row.create_extra).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the name to be free for a new StackSet, got {http_status} \
             {body}"
        );
        let recreated = stack_set_id(case, &router, name).await;
        assert_ne!(
            recreated, id,
            "case {case:?}: expected a new StackSetId for the re-created StackSet, got \
             {recreated}"
        );
    }
}

/// One row of the not-empty table: the instances the StackSet holds when the deletion is
/// asked for.
struct NotEmpty {
    case: &'static str,
    stack_set_name: &'static str,
    accounts: Vec<&'static str>,
    regions: Vec<&'static str>,
}

#[tokio::test]
async fn a_stackset_that_still_holds_an_instance_is_not_deleted() {
    let router = router();
    let rows = vec![
        NotEmpty {
            case: "one instance in the Region the StackSet itself lives in",
            stack_set_name: "not-empty-one-pair",
            accounts: vec![ACCOUNT_A],
            regions: vec![REGION],
        },
        NotEmpty {
            case: "two accounts in one target Region",
            stack_set_name: "not-empty-two-accounts",
            accounts: vec![ACCOUNT_A, ACCOUNT_B],
            regions: vec![REGION],
        },
        NotEmpty {
            case: "one account across two target Regions",
            stack_set_name: "not-empty-two-regions",
            accounts: vec![ACCOUNT_A],
            regions: vec![REGION, WEST],
        },
    ];

    // Each row's StackSet is its own, so the deployments the rows start all run at once and
    // one wait covers them.
    for row in &rows {
        let case = row.case;
        let (http_status, body) = create_stack_set(&router, row.stack_set_name, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStackSet to succeed, got {http_status} {body}"
        );
        let (http_status, body) =
            create_stack_instances(&router, row.stack_set_name, &row.accounts, &row.regions).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStackInstances to succeed, got {http_status} {body}"
        );
    }
    wait_for_the_transition().await;

    for row in &rows {
        let case = row.case;
        let name = row.stack_set_name;
        let (http_status, body) = delete_stack_set(&router, REGION, name, None).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected the deletion to be refused with 409, got {http_status} \
             {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("StackSetNotEmptyException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some("StackSet is not empty"),
            "case {case:?}: expected the service's own wording, got {message:?}"
        );

        // A refused deletion deletes nothing: the StackSet is still there, still ACTIVE, and
        // still holding every instance it held.
        let (http_status, body) = describe_stack_set(&router, REGION, name).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the refused StackSet to still be described, got \
             {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some("ACTIVE"),
            "case {case:?}: expected the StackSet to be untouched by the refusal, got {body}"
        );
    }

    // "Before you can delete a StackSet, all its member stack instances must be deleted": the
    // same deletion is served once they are.
    for row in &rows {
        let case = row.case;
        let (http_status, body) =
            delete_stack_instances(&router, row.stack_set_name, &row.accounts, &row.regions).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DeleteStackInstances to succeed, got {http_status} {body}"
        );
    }
    wait_for_the_transition().await;

    for row in &rows {
        let case = row.case;
        let name = row.stack_set_name;
        let (http_status, body) = delete_stack_set(&router, REGION, name, None).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the emptied StackSet to be deleted, got {http_status} \
             {body}"
        );
        let (http_status, body) = describe_stack_set(&router, REGION, name).await;
        assert_eq!(
            http_status, 404,
            "case {case:?}: expected the deleted StackSet to resolve to nothing, got \
             {http_status} {body}"
        );
    }
}

#[tokio::test]
async fn a_deletion_while_an_operation_is_in_flight_names_the_stackset_and_the_operation() {
    let router = router();
    let case = "a deletion asked for while a deployment is in flight";
    let name = "delete-single-flight";
    let (http_status, body) = create_stack_set(&router, name, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateStackSet to succeed, got {http_status} {body}"
    );
    let arn = stack_set_arn(case, &router, name).await;
    let (http_status, body) = create_stack_instances(&router, name, &[ACCOUNT_A], &[REGION]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateStackInstances to succeed, got {http_status} {body}"
    );
    let blocking = xml_text(&body, "OperationId")
        .unwrap_or_else(|| panic!("case {case:?}: no OperationId in {body}"));

    // The fixed words are "Another Operation on StackSet" and "is in progress:"; the ARN is
    // the StackSet's own and the id is the operation that still holds it.
    let expected = format!("Another Operation on StackSet {arn} is in progress: {blocking}");
    let (http_status, body) = delete_stack_set(&router, REGION, name, None).await;
    assert_eq!(
        http_status, 409,
        "case {case:?}: expected the deletion to be refused with 409, got {http_status} {body}"
    );
    let (code, message) = refusal(&body);
    assert_eq!(
        code.as_deref(),
        Some("OperationInProgressException"),
        "case {case:?}: expected the modelled refusal, got {body}"
    );
    assert_eq!(
        message.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected {expected:?}, got {message:?}"
    );

    // Once that operation has concluded the StackSet is no longer busy — but it now holds the
    // instance the deployment created, so the deletion is refused for THAT reason instead.
    wait_for_the_transition().await;
    let (http_status, body) = delete_stack_set(&router, REGION, name, None).await;
    assert_eq!(
        http_status, 409,
        "case {case:?}: expected the deployed StackSet to be refused with 409, got \
         {http_status} {body}"
    );
    assert_eq!(
        refusal(&body).0.as_deref(),
        Some("StackSetNotEmptyException"),
        "case {case:?}: expected the deployed StackSet's own refusal, got {body}"
    );

    let (http_status, body) = delete_stack_instances(&router, name, &[ACCOUNT_A], &[REGION]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DeleteStackInstances to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let (http_status, body) = delete_stack_set(&router, REGION, name, None).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the emptied StackSet to be deleted, got {http_status} {body}"
    );
}

/// One row of the addresses-nothing table: the Region the deletion is signed for and the
/// `StackSetName` it names.
struct Nothing {
    case: &'static str,
    region: &'static str,
    name_or_id: String,
}

#[tokio::test]
async fn a_request_that_addresses_no_stackset_is_a_success() {
    let router = router();
    // One StackSet that SURVIVES every call below: it is held to the Region it was created
    // in, so a deletion signed for another Region addresses no StackSet at all.
    let survivor = "delete-other-region";
    let (http_status, body) = create_stack_set(&router, survivor, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet({survivor}) to succeed, got {http_status} {body}"
    );
    // And one that a deletion has already taken, so its name addresses nothing either.
    let gone = "delete-already-deleted";
    let (http_status, body) = create_stack_set(&router, gone, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet({gone}) to succeed, got {http_status} {body}"
    );
    let gone_id = stack_set_id("the StackSet a deletion already took", &router, gone).await;
    let (http_status, body) = delete_stack_set(&router, REGION, gone, None).await;
    assert_eq!(
        http_status, 200,
        "expected DeleteStackSet({gone}) to succeed, got {http_status} {body}"
    );

    let rows = vec![
        Nothing {
            case: "a name no StackSet was ever created under",
            region: REGION,
            name_or_id: "delete-never-created".to_string(),
        },
        Nothing {
            case: "a StackSet of another Region, addressed in this one",
            region: WEST,
            name_or_id: survivor.to_string(),
        },
        Nothing {
            case: "a name whose StackSet a deletion already took",
            region: REGION,
            name_or_id: gone.to_string(),
        },
        Nothing {
            case: "the id of a StackSet a deletion already took",
            region: REGION,
            name_or_id: gone_id,
        },
        Nothing {
            case: "an id no StackSet of this account carries",
            region: REGION,
            name_or_id: "delete-ghost:00000000-0000-0000-0000-000000000000".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let addressed = row.name_or_id.as_str();
        let (http_status, body) =
            delete_stack_set(&router, row.region, addressed, None).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DeleteStackSet({addressed}) in {} to succeed, got \
             {http_status} {body}",
            row.region
        );
        assert_eq!(
            refusal(&body),
            (None, None),
            "case {case:?}: expected a success envelope, got {body}"
        );
    }

    // None of those calls touched a StackSet: the one another Region holds is still there,
    // still ACTIVE.
    let (http_status, body) = describe_stack_set(&router, REGION, survivor).await;
    assert_eq!(
        http_status, 200,
        "expected {survivor} to survive a deletion signed for another Region, got \
         {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some("ACTIVE"),
        "expected {survivor} to be untouched, got {body}"
    );
}
