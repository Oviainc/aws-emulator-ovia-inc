// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreatePlacementGroup` and `DescribePlacementGroups`, driven over
//! the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreatePlacementGroup`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that `CreatePlacementGroup` CREATES THE PLACEMENT GROUP THE
//! REQUEST DESCRIBES and answers it from the store: the group carries the requested name,
//! the requested strategy and the strategy's own members (`PartitionCount` only for
//! `partition`, `SpreadLevel` only for `spread`), is minted with a service-owned `pg-` id
//! and an ARN naming the signed Region and the group's name, is `available`, and reads back
//! unchanged through `DescribePlacementGroups` by id, by name and through every filter that
//! operation documents. A name the account already holds is refused, and a member that
//! belongs to another strategy is refused rather than stored.
//!
//! Every table row is a distinct input along what that generalises over — the strategy (one
//! row per member of the model's own `PlacementStrategy` catalog), the name's value, length
//! and character set, the partition count (each published bound and one past it), the
//! spread level, the tag count and the signed Region — so the one case a recording happens
//! to contain is one row among many rather than the specification.

use std::collections::BTreeMap;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_PLACEMENT_GROUPS, PlacementGroupSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::error_code;
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region most calls here are signed for.
const REGION: &str = "us-east-1";

/// Every member of the model's own `PlacementStrategy` catalog
/// (`crate::ops::types::PLACEMENT_STRATEGY_VALUES`, from
/// `smithy_query(query="shape",shape="PlacementStrategy")`), so the create table spans the
/// whole published strategy set rather than the one strategy an issue cited.
const STRATEGIES: [&str; 4] = ["cluster", "partition", "precision-time", "spread"];

/// Every member of the model's own `SpreadLevel` catalog
/// (`crate::ops::types::SPREAD_LEVEL_VALUES`).
const SPREAD_LEVELS: [&str; 2] = ["host", "rack"];

/// The account every ARN this emulator mints names.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreatePlacementGroup` call carrying `params`, signed for `region`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, "CreatePlacementGroup", params).await
}

/// One `CreatePlacementGroup` call carrying `params`, signed for [`REGION`].
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    create_in(router, REGION, params).await
}

/// `CreatePlacementGroup` requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreatePlacementGroup, got {status} \
         with body {body}"
    );
    body
}

/// One `DescribePlacementGroups` call carrying `params`, signed for [`REGION`].
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribePlacementGroups", params).await
}

/// `DescribePlacementGroups` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePlacementGroups, got {status} \
         with body {body}"
    );
    body
}

/// One placement group as a response reports it: its scalar members by their ec2Query
/// element name, and its tags in the order they were reported.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    members: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl Group {
    /// The text of member `name`, or `None` when the group reports none.
    fn member(&self, name: &str) -> Option<&str> {
        self.members.get(name).map(String::as_str)
    }

    /// The text of member `name`, or a panic naming the case.
    fn required(&self, case: &str, name: &str) -> &str {
        self.member(name).unwrap_or_else(|| {
            panic!("case {case:?}: no <{name}> on the placement group {self:?}")
        })
    }
}

/// The element names of the sets nested inside a placement group: a scalar member of the
/// group is never read from inside one of these.
const NESTED_SETS: [&str; 1] = ["tagSet"];

/// Every placement group a response carries, in the order it reported them.
///
/// A group is the element with a `<groupId>` child — the `<placementGroup>` of a create and
/// each `<item>` of a describe's `<placementGroupSet>` — read through `roxmltree` so nesting
/// is resolved by the document rather than by string surgery.
fn groups(case: &str, body: &str) -> Vec<Group> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "groupId").is_some())
        .map(|node| {
            let members = node
                .children()
                .filter(|inner| {
                    inner.is_element() && !NESTED_SETS.contains(&inner.tag_name().name())
                })
                .map(|inner| {
                    (
                        inner.tag_name().name().to_string(),
                        inner.text().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            let tags = child(node, "tagSet")
                .into_iter()
                .flat_map(|set| set.children().collect::<Vec<_>>())
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect();
            Group { members, tags }
        })
        .collect()
}

/// The one placement group a response carries, or a panic naming the case.
fn one_group(case: &str, body: &str) -> Group {
    let found = groups(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one placement group, got {found:?} from {body}"
    );
    found.into_iter().next().expect("one group")
}

/// The `GroupId`s a describe reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    groups(case, body)
        .into_iter()
        .map(|group| group.required(case, "groupId").to_string())
        .collect()
}

/// The form fields for a `TagSpecification.1` naming the placement group resource type and
/// `tags`.
fn tag_fields(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut fields = Vec::new();
    if tags.is_empty() {
        return fields;
    }
    fields.push(param("TagSpecification.1.ResourceType", "placement-group"));
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    fields
}

/// One row of the create table: the case, the group's name, its strategy, the extra form
/// fields the request carries, the tags it assigns, and the members the group must report
/// beyond its name and strategy (element name, expected text).
type CreateCase = (
    &'static str,
    String,
    &'static str,
    Vec<Param>,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, String)>,
);

/// One row of a refusal table: the case, the request's form fields, the EC2 error code the
/// service answers and the HTTP status it answers with.
type RefusalCase = (&'static str, Vec<Param>, &'static str, u16);

/// A name of `length` ASCII characters, built so that no two rows share a name.
fn name_of_length(seed: &str, length: usize) -> String {
    let mut name = seed.to_string();
    while name.chars().count() < length {
        name.push('a');
    }
    name.chars().take(length).collect()
}

/// The create table: one row per `PlacementStrategy` member, plus rows varying the name's
/// length and character set, the partition count at each published bound, both spread
/// levels and the tag count.
fn create_cases() -> Vec<CreateCase> {
    let mut cases: Vec<CreateCase> = Vec::new();
    for strategy in STRATEGIES {
        cases.push((
            "bare strategy",
            format!("bare-{strategy}"),
            strategy,
            Vec::new(),
            Vec::new(),
            match strategy {
                // A partition group the request names no count for still has partitions,
                // and a spread group with no level named gets `rack` — the level with "no
                // usage restrictions" (`CreatePlacementGroupRequest$SpreadLevel`).
                "partition" => vec![("partitionCount", "2".to_string())],
                "spread" => vec![("spreadLevel", "rack".to_string())],
                _ => Vec::new(),
            },
        ));
    }
    // "A partition placement group can have a maximum of seven partitions per Availability
    // Zone" (Amazon EC2 User Guide, "Partition placement groups"); one partition is the
    // smallest count there can be. Both bounds are rows here; one past each is a refusal
    // row below.
    for count in [1, 3, 7] {
        cases.push((
            "partition count",
            format!("partitions-{count}"),
            "partition",
            vec![param("PartitionCount", &count.to_string())],
            Vec::new(),
            vec![("partitionCount", count.to_string())],
        ));
    }
    for level in SPREAD_LEVELS {
        cases.push((
            "spread level",
            format!("spread-{level}"),
            "spread",
            vec![param("SpreadLevel", level)],
            Vec::new(),
            vec![("spreadLevel", level.to_string())],
        ));
    }
    // "Up to 255 ASCII characters" (`CreatePlacementGroupRequest$GroupName`): the published
    // bound itself is a row, and so is a one-character name and one carrying the ASCII
    // punctuation a name may hold.
    cases.push((
        "shortest name",
        "x".to_string(),
        "cluster",
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    cases.push((
        "longest name",
        name_of_length("longest-", 255),
        "cluster",
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    cases.push((
        "punctuated name",
        "Group_1.2-3 (test)".to_string(),
        "spread",
        Vec::new(),
        Vec::new(),
        vec![("spreadLevel", "rack".to_string())],
    ));
    for tags in [
        vec![],
        vec![("Owner", "TeamA")],
        vec![("Owner", "TeamB"), ("Env", "prod"), ("Empty", "")],
    ] {
        cases.push((
            "tag count",
            format!("tagged-{}", tags.len()),
            "cluster",
            Vec::new(),
            tags,
            Vec::new(),
        ));
    }
    cases
}

#[tokio::test]
async fn create_stores_the_group_the_request_describes() {
    let router = emulator();
    for (label, group_name, strategy, extra, tags, expected) in create_cases() {
        let case = format!("{label}: {group_name} / {strategy}");
        let mut params = vec![param("GroupName", &group_name), param("Strategy", strategy)];
        params.extend(extra);
        params.extend(tag_fields(&tags));

        let created = create_ok(&case, &router, &params).await;
        let group = one_group(&case, &created);

        let group_id = group.required(&case, "groupId").to_string();
        assert!(
            is_long_id("pg", &group_id),
            "case {case:?}: expected a pg- long id, got {group_id:?}"
        );
        assert_eq!(
            group.member("groupName"),
            Some(group_name.as_str()),
            "case {case:?}: expected groupName {group_name:?}, got {:?}",
            group.member("groupName")
        );
        assert_eq!(
            group.member("strategy"),
            Some(strategy),
            "case {case:?}: expected strategy {strategy:?}, got {:?}",
            group.member("strategy")
        );
        // "A logical grouping of instances" (`CreatePlacementGroup`) has nothing to
        // provision, so it is usable as soon as it exists.
        assert_eq!(
            group.member("state"),
            Some("available"),
            "case {case:?}: expected state \"available\", got {:?}",
            group.member("state")
        );
        let expected_arn = format!(
            "arn:aws:ec2:{REGION}:{account}:placement-group/{group_name}",
            account = account_id()
        );
        assert_eq!(
            group.member("groupArn"),
            Some(expected_arn.as_str()),
            "case {case:?}: expected groupArn {expected_arn:?}, got {:?}",
            group.member("groupArn")
        );
        for (member, value) in &expected {
            assert_eq!(
                group.member(member),
                Some(value.as_str()),
                "case {case:?}: expected <{member}> {value:?}, got {:?}",
                group.member(member)
            );
        }
        // A member that belongs to another strategy is not reported at all, rather than
        // reported empty: `PartitionCount` is "valid only if strategy is set to partition"
        // (`PlacementGroup$PartitionCount`), and only a spread group has a spread level.
        if strategy != "partition" {
            assert_eq!(
                group.member("partitionCount"),
                None,
                "case {case:?}: a {strategy} group must report no partitionCount, got {:?}",
                group.member("partitionCount")
            );
        }
        if strategy != "spread" {
            assert_eq!(
                group.member("spreadLevel"),
                None,
                "case {case:?}: a {strategy} group must report no spreadLevel, got {:?}",
                group.member("spreadLevel")
            );
        }
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            group.tags, expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            group.tags
        );

        // The round trip: the group reads back out of the store, by id and by name, exactly
        // as the create answered it.
        let by_id = describe_ok(&case, &router, &[param("GroupId.1", &group_id)]).await;
        assert_eq!(
            one_group(&case, &by_id),
            group,
            "case {case:?}: DescribePlacementGroups by id must report the created group \
             unchanged, got {by_id}"
        );
        let by_name = describe_ok(&case, &router, &[param("GroupName.1", &group_name)]).await;
        assert_eq!(
            one_group(&case, &by_name),
            group,
            "case {case:?}: DescribePlacementGroups by name must report the created group \
             unchanged, got {by_name}"
        );
    }
}

#[tokio::test]
async fn the_arn_names_the_region_the_request_was_signed_for() {
    // The ARN is built from the Region in the request's SigV4 credential scope, so a group
    // created in another Region reports that Region — not the emulator's default.
    for region in ["us-east-1", "eu-west-3", "ap-southeast-2"] {
        let case = format!("region {region}");
        let router = emulator();
        let group_name = format!("regional-{region}");
        let (status, body) = create_in(
            &router,
            region,
            &[param("GroupName", &group_name), param("Strategy", "cluster")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let expected = format!(
            "arn:aws:ec2:{region}:{account}:placement-group/{group_name}",
            account = account_id()
        );
        let group = one_group(&case, &body);
        assert_eq!(
            group.member("groupArn"),
            Some(expected.as_str()),
            "case {case:?}: expected groupArn {expected:?}, got {:?}",
            group.member("groupArn")
        );
    }
}

#[tokio::test]
async fn a_cluster_group_can_name_a_parent_group() {
    let router = emulator();
    let case = "parent group";
    let parent = create_ok(
        case,
        &router,
        &[param("GroupName", "parent"), param("Strategy", "cluster")],
    )
    .await;
    let parent_id = one_group(case, &parent).required(case, "groupId").to_string();

    let child_body = create_ok(
        case,
        &router,
        &[
            param("GroupName", "child"),
            param("Strategy", "cluster"),
            param("ParentGroupId", &parent_id),
        ],
    )
    .await;
    let group = one_group(case, &child_body);
    assert_eq!(
        group.member("parentGroupId"),
        Some(parent_id.as_str()),
        "case {case:?}: expected parentGroupId {parent_id:?}, got {:?}",
        group.member("parentGroupId")
    );

    // And it reads back with the parent it was created under.
    let read_back = describe_ok(
        case,
        &router,
        &[param("GroupName.1", "child")],
    )
    .await;
    assert_eq!(
        one_group(case, &read_back),
        group,
        "case {case:?}: the child group must read back unchanged, got {read_back}"
    );
}

#[tokio::test]
async fn a_group_never_reports_a_reserved_member() {
    // `LinkedGroupId` is "reserved for future use" and `Operator` is "reserved for internal
    // use", so a group an account created for itself reports neither — a caller cannot mark
    // its own group as managed by a service provider, and the refusal table below pins that
    // setting either is refused rather than ignored.
    let router = emulator();
    let case = "reserved members";
    let body = create_ok(
        case,
        &router,
        &[param("GroupName", "plain"), param("Strategy", "cluster")],
    )
    .await;
    let parsed = document(case, &body);
    let group = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "groupId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no placement group in {body}"));
    for member in ["operator", "linkedGroupId"] {
        assert_eq!(
            child(group, member).map(|node| node.tag_name().name().to_string()),
            None,
            "case {case:?}: a group must report no <{member}>, got {body}"
        );
    }
}

/// The refusal table for `CreatePlacementGroup`: every member that belongs to one strategy
/// is refused with another, every published bound is refused one past it, and a name the
/// account already holds is refused.
fn create_refusals() -> Vec<RefusalCase> {
    vec![
        (
            "no name",
            vec![param("Strategy", "cluster")],
            "MissingParameter",
            400,
        ),
        (
            "empty name",
            vec![param("GroupName", ""), param("Strategy", "cluster")],
            "MissingParameter",
            400,
        ),
        (
            "no strategy",
            vec![param("GroupName", "no-strategy")],
            "MissingParameter",
            400,
        ),
        // The modelled `PlacementStrategy` enum is refused by the boundary validator,
        // before this handler runs, so an undocumented strategy answers that refusal's own
        // code rather than one the handler chose.
        (
            "unknown strategy",
            vec![param("GroupName", "bad-strategy"), param("Strategy", "herd")],
            "ValidationError",
            400,
        ),
        (
            "name one character past the bound",
            vec![
                param("GroupName", &name_of_length("too-long-", 256)),
                param("Strategy", "cluster"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "non-ASCII name",
            vec![param("GroupName", "grüppe"), param("Strategy", "cluster")],
            "InvalidParameterValue",
            400,
        ),
        (
            "partition count below the bound",
            vec![
                param("GroupName", "partitions-0"),
                param("Strategy", "partition"),
                param("PartitionCount", "0"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "partition count past the bound",
            vec![
                param("GroupName", "partitions-8"),
                param("Strategy", "partition"),
                param("PartitionCount", "8"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "partition count on a cluster group",
            vec![
                param("GroupName", "clustered-partitions"),
                param("Strategy", "cluster"),
                param("PartitionCount", "3"),
            ],
            "InvalidParameterCombination",
            400,
        ),
        (
            "partition count on a spread group",
            vec![
                param("GroupName", "spread-partitions"),
                param("Strategy", "spread"),
                param("PartitionCount", "3"),
            ],
            "InvalidParameterCombination",
            400,
        ),
        // `SpreadLevel` is a modelled enum too, so the boundary validator refuses a value
        // outside it for the reason given above.
        (
            "unknown spread level",
            vec![
                param("GroupName", "bad-spread"),
                param("Strategy", "spread"),
                param("SpreadLevel", "cabinet"),
            ],
            "ValidationError",
            400,
        ),
        (
            "spread level on a cluster group",
            vec![
                param("GroupName", "clustered-spread"),
                param("Strategy", "cluster"),
                param("SpreadLevel", "rack"),
            ],
            "InvalidParameterCombination",
            400,
        ),
        (
            "parent group on a spread group",
            vec![
                param("GroupName", "spread-parented"),
                param("Strategy", "spread"),
                param("ParentGroupId", "pg-00000000000000001"),
            ],
            "InvalidParameterCombination",
            400,
        ),
        (
            "unknown parent group",
            vec![
                param("GroupName", "orphan"),
                param("Strategy", "cluster"),
                param("ParentGroupId", "pg-0000000000000dead"),
            ],
            "InvalidPlacementGroup.Unknown",
            400,
        ),
        (
            "reserved LinkedGroupId",
            vec![
                param("GroupName", "linked"),
                param("Strategy", "cluster"),
                param("LinkedGroupId", "pg-00000000000000001"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "reserved Operator",
            vec![
                param("GroupName", "managed"),
                param("Strategy", "cluster"),
                param("Operator.Principal", "ec2.amazonaws.com"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "tag specification naming another resource type",
            vec![
                param("GroupName", "mistagged"),
                param("Strategy", "cluster"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Owner"),
                param("TagSpecification.1.Tag.1.Value", "TeamA"),
            ],
            "InvalidParameterValue",
            400,
        ),
    ]
}

#[tokio::test]
async fn create_refuses_what_it_cannot_store() {
    let router = emulator();
    for (case, params, expected_code, expected_status) in create_refusals() {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} \
             with body {body}"
        );
    }

    // Nothing above was stored, so a group may still be created under each of those names.
    let listed = describe_ok("after the refusals", &router, &[]).await;
    assert!(
        groups("after the refusals", &listed).is_empty(),
        "a refused create must store nothing, got {listed}"
    );
}

#[tokio::test]
async fn a_name_the_account_already_holds_is_refused() {
    let router = emulator();
    // The name is what must be unique, not the rest of the request: a second create is
    // refused whatever strategy it names.
    for strategy in STRATEGIES {
        let case = format!("duplicate of a {strategy} group");
        let group_name = format!("taken-{strategy}");
        create_ok(
            &case,
            &router,
            &[param("GroupName", &group_name), param("Strategy", strategy)],
        )
        .await;
        for second in STRATEGIES {
            let (status, body) = create(
                &router,
                &[param("GroupName", &group_name), param("Strategy", second)],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a second create as {second} must be refused, got {status} \
                 with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidPlacementGroup.Duplicate",
                "case {case:?}: a second create as {second} must answer \
                 InvalidPlacementGroup.Duplicate, got {code:?} with body {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_dry_run_that_would_have_succeeded_stores_nothing() {
    let router = emulator();
    let case = "dry run";
    let (status, body) = create(
        &router,
        &[
            param("GroupName", "dry"),
            param("Strategy", "cluster"),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 from a dry run, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {code:?} with body {body}"
    );

    // The group was not created, so asking for it by name is an unknown group and the real
    // create still works.
    let (status, body) = describe(&router, &[param("GroupName.1", "dry")]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a dry run must store nothing, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "InvalidPlacementGroup.Unknown",
        "case {case:?}: expected InvalidPlacementGroup.Unknown, got {code:?} with {body}"
    );
    create_ok(
        case,
        &router,
        &[param("GroupName", "dry"), param("Strategy", "cluster")],
    )
    .await;
}

/// The groups the filter and selection tests run against: one per strategy, tagged so the
/// tag filters have something to select on. Answers each group's name and the id it was
/// minted with, in creation order.
async fn seeded(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut created = Vec::new();
    for strategy in STRATEGIES {
        let case = format!("seed {strategy}");
        let group_name = format!("seed-{strategy}");
        let body = create_ok(
            &case,
            router,
            &[
                param("GroupName", &group_name),
                param("Strategy", strategy),
                param("TagSpecification.1.ResourceType", "placement-group"),
                param("TagSpecification.1.Tag.1.Key", "Strategy"),
                param("TagSpecification.1.Tag.1.Value", strategy),
            ],
        )
        .await;
        let group_id = one_group(&case, &body).required(&case, "groupId").to_string();
        created.push((group_name, group_id));
    }
    created
}

#[tokio::test]
async fn describe_selects_on_every_documented_filter() {
    let router = emulator();
    let seeds = seeded(&router).await;
    let id_of = |name: &str| -> String {
        seeds
            .iter()
            .find(|(group_name, _)| group_name == name)
            .map(|(_, group_id)| group_id.clone())
            .unwrap_or_else(|| panic!("no seeded group named {name}"))
    };

    // Each row names a filter `DescribePlacementGroupsRequest$Filters` documents, the
    // values it carries, and the group names it must select.
    let cases: Vec<(&str, &str, Vec<String>, Vec<&str>)> = vec![
        (
            "group-name selects one",
            "group-name",
            vec!["seed-spread".to_string()],
            vec!["seed-spread"],
        ),
        (
            "group-name is joined with OR across its values",
            "group-name",
            vec!["seed-spread".to_string(), "seed-cluster".to_string()],
            vec!["seed-cluster", "seed-spread"],
        ),
        (
            "group-name takes EC2 wildcards",
            "group-name",
            vec!["seed-c*".to_string()],
            vec!["seed-cluster"],
        ),
        (
            "group-arn selects the group it names",
            "group-arn",
            vec![format!(
                "arn:aws:ec2:{REGION}:{account}:placement-group/seed-partition",
                account = account_id()
            )],
            vec!["seed-partition"],
        ),
        (
            "strategy selects every group with it",
            "strategy",
            vec!["precision-time".to_string()],
            vec!["seed-precision-time"],
        ),
        (
            "state selects every group, all of which are available",
            "state",
            vec!["available".to_string()],
            vec![
                "seed-cluster",
                "seed-partition",
                "seed-precision-time",
                "seed-spread",
            ],
        ),
        (
            "spread-level selects only the spread group",
            "spread-level",
            vec!["rack".to_string()],
            vec!["seed-spread"],
        ),
        (
            "spread-level a group does not carry selects nothing",
            "spread-level",
            vec!["host".to_string()],
            vec![],
        ),
        (
            "tag-key selects every tagged group",
            "tag-key",
            vec!["Strategy".to_string()],
            vec![
                "seed-cluster",
                "seed-partition",
                "seed-precision-time",
                "seed-spread",
            ],
        ),
        (
            "tag-key no group carries selects nothing",
            "tag-key",
            vec!["Owner".to_string()],
            vec![],
        ),
        (
            "tag:<key> selects on the value",
            "tag:Strategy",
            vec!["cluster".to_string()],
            vec!["seed-cluster"],
        ),
        (
            "tag:<key> with a value no group carries selects nothing",
            "tag:Strategy",
            vec!["herd".to_string()],
            vec![],
        ),
    ];

    for (case, name, values, expected_names) in cases {
        let mut params = vec![param("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            params.push(param(&format!("Filter.1.Value.{}", index + 1), value));
        }
        let body = describe_ok(case, &router, &params).await;
        let expected: Vec<String> = {
            let mut ids: Vec<String> = expected_names.iter().map(|name| id_of(name)).collect();
            // The store reports in `GroupId` order, which is the order the ids were minted.
            ids.sort();
            ids
        };
        let actual = reported_ids(case, &body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the groups {expected_names:?} ({expected:?}), got \
             {actual:?} from {body}"
        );
    }
}

#[tokio::test]
async fn describe_joins_its_filters_with_and() {
    let router = emulator();
    let seeds = seeded(&router).await;
    let case = "two filters that cannot both hold";
    let body = describe_ok(
        case,
        &router,
        &[
            param("Filter.1.Name", "strategy"),
            param("Filter.1.Value.1", "cluster"),
            param("Filter.2.Name", "tag:Strategy"),
            param("Filter.2.Value.1", "spread"),
        ],
    )
    .await;
    let actual = reported_ids(case, &body);
    assert!(
        actual.is_empty(),
        "case {case:?}: filters are joined with AND, so nothing may match, got {actual:?}"
    );

    let case = "two filters that both hold";
    let body = describe_ok(
        case,
        &router,
        &[
            param("Filter.1.Name", "strategy"),
            param("Filter.1.Value.1", "cluster"),
            param("Filter.2.Name", "tag:Strategy"),
            param("Filter.2.Value.1", "cluster"),
        ],
    )
    .await;
    let expected: Vec<String> = seeds
        .iter()
        .filter(|(name, _)| name == "seed-cluster")
        .map(|(_, group_id)| group_id.clone())
        .collect();
    let actual = reported_ids(case, &body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?} from {body}"
    );
}

#[tokio::test]
async fn describe_reports_every_group_when_the_request_narrows_nothing() {
    let router = emulator();
    let seeds = seeded(&router).await;
    let case = "no selectors";
    let body = describe_ok(case, &router, &[]).await;
    let mut expected: Vec<String> = seeds.iter().map(|(_, id)| id.clone()).collect();
    expected.sort();
    let actual = reported_ids(case, &body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected every group {expected:?}, got {actual:?} from {body}"
    );
}

#[tokio::test]
async fn describe_refuses_what_it_cannot_resolve() {
    let router = emulator();
    seeded(&router).await;
    let refusals: Vec<RefusalCase> = vec![
        (
            "an id the account holds no group under",
            vec![param("GroupId.1", "pg-0000000000000dead")],
            "InvalidPlacementGroup.Unknown",
            400,
        ),
        (
            "a name the account holds no group under",
            vec![param("GroupName.1", "nothing-here")],
            "InvalidPlacementGroup.Unknown",
            400,
        ),
        (
            "a known name beside an unknown one",
            vec![
                param("GroupName.1", "seed-cluster"),
                param("GroupName.2", "nothing-here"),
            ],
            "InvalidPlacementGroup.Unknown",
            400,
        ),
        (
            "a filter name the operation does not document",
            vec![
                param("Filter.1.Name", "partition-count"),
                param("Filter.1.Value.1", "2"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "a tag filter with no key",
            vec![param("Filter.1.Name", "tag:"), param("Filter.1.Value.1", "x")],
            "InvalidParameterValue",
            400,
        ),
    ];
    for (case, params, expected_code, expected_status) in refusals {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} \
             with body {body}"
        );
    }

    let case = "a dry run reads nothing";
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {code:?} with body {body}"
    );
}

#[tokio::test]
async fn a_read_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let seeds = seeded(&router).await;
    let case = "repeated reads";
    let first = describe_ok(case, &router, &[]).await;
    let first_groups = groups(case, &first);
    for _ in 0..3 {
        let again = describe_ok(case, &router, &[]).await;
        assert_eq!(
            groups(case, &again),
            first_groups,
            "case {case:?}: a read must not change what a later caller sees, got {again}"
        );
    }
    assert_eq!(
        first_groups.len(),
        seeds.len(),
        "case {case:?}: expected {} groups, got {first_groups:?}",
        seeds.len()
    );
}

#[tokio::test]
async fn describe_takes_the_union_of_its_two_selector_lists() {
    // Each entry of `GroupIds` and of `GroupNames` must resolve, so a request carrying both
    // asked for every group it named: the two lists are a union. Intersecting them would
    // report nothing for a request naming one group by id and another by name.
    let router = emulator();
    let seeds = seeded(&router).await;
    let case = "an id and a different group's name";
    let by_id = &seeds[0];
    let by_name = &seeds[3];
    let body = describe_ok(
        case,
        &router,
        &[
            param("GroupId.1", &by_id.1),
            param("GroupName.1", &by_name.0),
        ],
    )
    .await;
    let mut expected = vec![by_id.1.clone(), by_name.1.clone()];
    expected.sort();
    let actual = reported_ids(case, &body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected both {expected:?}, got {actual:?} from {body}"
    );

    let case = "an id and that same group's name";
    let body = describe_ok(
        case,
        &router,
        &[param("GroupId.1", &by_id.1), param("GroupName.1", &by_id.0)],
    )
    .await;
    let actual = reported_ids(case, &body);
    assert_eq!(
        actual,
        vec![by_id.1.clone()],
        "case {case:?}: one group named twice is reported once, got {actual:?} from {body}"
    );
}

#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused() {
    // The count ceiling, exercised at the store seam rather than through 500 wire calls:
    // the store is filled to `MAX_PLACEMENT_GROUPS`, and the next create is refused with
    // EC2's own quota code instead of growing the store.
    let (router, engine) = super::ec2_query::emulator();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for index in 0..MAX_PLACEMENT_GROUPS {
            guard
                .insert_placement_group(PlacementGroupSpec {
                    group_name: format!("seeded-{index}"),
                    strategy: "cluster",
                    partition_count: None,
                    spread_level: None,
                    parent_group_id: None,
                    region: REGION.to_string(),
                    tags: Vec::new(),
                })
                .expect("the store holds the seeded placement group");
        }
        assert!(
            !guard.has_placement_group_capacity(),
            "a store holding {MAX_PLACEMENT_GROUPS} groups must report no capacity"
        );
    }

    let case = "one past the ceiling";
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreatePlacementGroup",
        &[param("GroupName", "one-too-many"), param("Strategy", "cluster")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "PlacementGroupLimitExceeded",
        "case {case:?}: expected PlacementGroupLimitExceeded, got {code:?} with {body}"
    );

    // And nothing was stored for the refused name.
    let stored = engine
        .state::<Ec2State>()
        .lock()
        .placement_group_id_for_name("one-too-many");
    assert_eq!(
        stored, None,
        "case {case:?}: a refused create must store nothing, got {stored:?}"
    );
}

