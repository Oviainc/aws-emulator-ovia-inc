// SPDX-License-Identifier: CC0-1.0

//! `CreateSecurityGroup`, end to end over the wire the service actually speaks: `ec2Query`
//! — an `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the create PERSISTS the group and the reporting half reports
//! what was persisted: the name and description the request chose, the VPC it named (or the
//! account's default VPC when it named none), a minted `GroupId`, an ARN derived from it,
//! this account as owner, the documented allow-all outbound rule and no inbound rule. What
//! `DescribeSecurityGroups` SELECTS on — its filters, its narrowing members and its paging
//! — is the subject of `tests/all/describe_security_groups.rs`; this suite uses it only to
//! read a created group back.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! alphabet and length of the name and description, whether a VPC is named, the tag count,
//! and the casing a name is reused under.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, indexed, unescaped};

/// The Region every call here is signed for; it is the Region a created group's ARN names.
const REGION: &str = "us-west-2";

/// One `CreateSecurityGroup` call, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateSecurityGroup", params).await
}

/// One `DescribeSecurityGroups` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecurityGroups", params).await
}

fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One row of a refusal table: the case name, the extra form parameters the row sends, and
/// the EC2 error code the refusal must carry.
type RefusalCase = (&'static str, Vec<(String, String)>, &'static str);

/// How many security groups the store holds — a fresh one already has the default VPC's
/// default group, so a refusal that persists nothing leaves exactly one.
fn held_groups(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().security_groups.len()
}

/// The id of the account's default VPC — the one a create that names no `VpcId` must use.
fn default_vpc_id(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .vpcs
        .values()
        .find(|vpc| vpc.is_default.unwrap_or(false))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has a default VPC")
}

/// Write a second, non-default VPC into the store, the way `CreateVpc` would, so a rule
/// about "unique within the VPC" can be asserted across two VPCs.
fn insert_vpc(engine: &engine::InMemoryEngine, cidr_block: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_vpc(cidr_block, false, "default", "available", Vec::new())
}

/// One `CreateSecurityGroup` request, with the members the row exercises.
struct CreateCase {
    /// What the row is called in an assertion message.
    case: &'static str,
    group_name: &'static str,
    description: &'static str,
    /// Whether the request names a `VpcId`; when it does not, the default VPC is expected.
    name_the_vpc: bool,
    tags: Vec<(&'static str, &'static str)>,
}

impl CreateCase {
    fn params(&self, vpc_id: &str) -> Vec<(String, String)> {
        let mut params = vec![
            param("GroupName", self.group_name),
            param("GroupDescription", self.description),
        ];
        if self.name_the_vpc {
            params.push(param("VpcId", vpc_id));
        }
        if !self.tags.is_empty() {
            params.push(param("TagSpecification.1.ResourceType", "security-group"));
            for (index, (key, value)) in self.tags.iter().enumerate() {
                let position = index + 1;
                params.push(param(
                    &format!("TagSpecification.1.Tag.{position}.Key"),
                    key,
                ));
                params.push(param(
                    &format!("TagSpecification.1.Tag.{position}.Value"),
                    value,
                ));
            }
        }
        params
    }
}

/// The table every suite here is written against: names and descriptions spanning the
/// documented alphabet and lengths, a request that names a VPC and one that does not, and
/// tag counts from none to several.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "the plainest group",
            group_name: "asg-instances",
            description: "instances of the scaling group",
            name_the_vpc: true,
            tags: Vec::new(),
        },
        CreateCase {
            case: "no VpcId, so the default VPC",
            group_name: "default-vpc-group",
            description: "a group in the default VPC",
            name_the_vpc: false,
            tags: Vec::new(),
        },
        CreateCase {
            // Every documented punctuation character except the `;` an
            // `application/x-www-form-urlencoded` body cannot carry unescaped.
            case: "the documented alphabet",
            group_name: "sg_name-0:a/b(c)#d,e@f[g]+h=i&j{k}l!m$n*o.p",
            description: "desc_0:a/b(c)#d,e@f[g]+h=i&j{k}l!m$n*o.p and spaces",
            name_the_vpc: true,
            tags: vec![("env", "dev")],
        },
        CreateCase {
            case: "a one-character name and description",
            group_name: "a",
            description: "d",
            name_the_vpc: true,
            tags: vec![("a", "1"), ("b", "2"), ("c", "3")],
        },
        CreateCase {
            case: "a name and description at the 255-character bound",
            group_name: LONG_NAME,
            description: LONG_DESCRIPTION,
            name_the_vpc: true,
            tags: Vec::new(),
        },
    ]
}

/// 255 characters — the documented bound on a group name ("Up to 255 characters in
/// length").
const LONG_NAME: &str = "n255-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// 255 characters — the same bound on a description.
const LONG_DESCRIPTION: &str = "d255-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// Every row creates a group that `DescribeSecurityGroups` then reports identically.
#[tokio::test]
async fn a_created_security_group_round_trips_through_describe_security_groups() {
    let (router, engine) = super::ec2_query::emulator();
    let default_vpc = default_vpc_id(&engine);
    let other_vpc = insert_vpc(&engine, "10.40.0.0/16");
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases() {
        let params = case.params(&other_vpc);
        let (status, created) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {created}",
            case.case
        );

        let group_id = element(&created, "groupId")
            .unwrap_or_else(|| panic!("case {:?}: expected a groupId in {created}", case.case));
        // The id form spelled out here rather than only through the crate's own predicate,
        // so a change to the minter has to be a deliberate change to this expectation too:
        // `sg-` and 17 lowercase hexadecimal digits.
        assert!(
            group_id.len() == 20
                && group_id.starts_with("sg-")
                && group_id[3..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "case {:?}: expected a `sg-` id with 17 lowercase hex digits, got {group_id:?}",
            case.case
        );
        assert!(
            ec2::state::security_group_id_is_well_formed(&group_id),
            "case {:?}: expected a well-formed group id, got {group_id:?}",
            case.case
        );
        assert!(
            !minted.contains(&group_id),
            "case {:?}: group id {group_id:?} was already minted for another group",
            case.case
        );
        minted.push(group_id.clone());

        let expected_vpc = if case.name_the_vpc {
            other_vpc.clone()
        } else {
            default_vpc.clone()
        };
        let expected_arn = format!(
            "arn:aws:ec2:{REGION}:{}:security-group/{group_id}",
            engine::emulator_account_id()
        );
        let reported_arn = element(&created, "securityGroupArn").unwrap_or_default();
        assert_eq!(
            reported_arn, expected_arn,
            "case {:?}: expected the ARN {expected_arn}, got {reported_arn:?}",
            case.case
        );

        // The describe reports the SAME record, member for member.
        let (status, read) = describe(&router, &indexed("GroupId", &[group_id.as_str()])).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the describe, got {status} with body {read}",
            case.case
        );
        let expected_members = [
            ("groupId", group_id.clone()),
            ("groupName", case.group_name.to_string()),
            ("groupDescription", case.description.to_string()),
            ("vpcId", expected_vpc.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("securityGroupArn", expected_arn.clone()),
        ];
        for (member, expected) in &expected_members {
            let actual: Vec<String> = elements(&read, member)
                .iter()
                .map(|value| unescaped(value))
                .collect();
            assert_eq!(
                actual,
                vec![expected.clone()],
                "case {:?}: expected <{member}>{expected}</{member}>, got {read}",
                case.case
            );
        }
        // The tags the request carried, reported by both halves.
        for (key, value) in &case.tags {
            assert!(
                elements(&read, "key").contains(&(*key).to_string()),
                "case {:?}: the describe must report the tag key {key:?}, got {read}",
                case.case
            );
            assert!(
                elements(&read, "value").contains(&(*value).to_string()),
                "case {:?}: the describe must report the tag value {value:?}, got {read}",
                case.case
            );
            assert!(
                elements(&created, "key").contains(&(*key).to_string()),
                "case {:?}: the create must report the tag key {key:?}, got {created}",
                case.case
            );
        }
        if case.tags.is_empty() {
            assert!(
                elements(&created, "key").is_empty(),
                "case {:?}: an untagged group reports no tag, got {created}",
                case.case
            );
        }
    }
}

/// A group `CreateSecurityGroup` minted has NO inbound rule — "when you create a security
/// group, it has no inbound rules" — and the one documented allow-all outbound rule.
#[tokio::test]
async fn a_created_group_starts_with_no_inbound_rule_and_one_outbound_rule() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&engine);
    let params = vec![
        param("GroupName", "fresh-group"),
        param("GroupDescription", "a brand new group"),
        param("VpcId", &vpc_id),
    ];
    let (status, created) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200, got {status} with body {created}"
    );
    let group_id = element(&created, "groupId").unwrap_or_default();

    let group = engine
        .state::<Ec2State>()
        .lock()
        .security_groups
        .get(&group_id)
        .cloned()
        .unwrap_or_else(|| panic!("the group {group_id:?} must be persisted"));
    assert_eq!(
        group.ip_permissions.as_deref().unwrap_or_default().len(),
        0,
        "a new group has no inbound rule, got {:?}",
        group.ip_permissions
    );
    let egress = group.ip_permissions_egress.clone().unwrap_or_default();
    assert_eq!(
        egress.len(),
        1,
        "a new group has exactly one outbound rule, got {egress:?}"
    );
    assert_eq!(
        egress[0].ip_protocol.as_deref(),
        Some("-1"),
        "the outbound rule allows every protocol, got {:?}",
        egress[0].ip_protocol
    );
    assert_eq!(
        egress[0]
            .ip_ranges
            .iter()
            .flatten()
            .filter_map(|range| range.cidr_ip.as_deref())
            .collect::<Vec<&str>>(),
        vec!["0.0.0.0/0"],
        "the outbound rule allows every destination, got {egress:?}"
    );
}

// ---------------------------------------------------------------------------
// Names are unique within the VPC, and case-insensitive
// ---------------------------------------------------------------------------

/// "You can't have two security groups for the same VPC with the same name", and "names
/// are case-insensitive" — so a second create under any casing is refused, while the same
/// name in ANOTHER VPC is accepted.
#[tokio::test]
async fn a_name_is_unique_within_the_vpc_and_not_distinguished_by_case() {
    let (router, engine) = super::ec2_query::emulator();
    let first_vpc = default_vpc_id(&engine);
    let second_vpc = insert_vpc(&engine, "10.41.0.0/16");

    let held = vec![
        param("GroupName", "Shared-Group"),
        param("GroupDescription", "the first one"),
        param("VpcId", &first_vpc),
    ];
    let (status, body) = create(&router, &held).await;
    assert_eq!(status, 200, "the first create must succeed, got {body}");

    for name in [
        "Shared-Group",
        "shared-group",
        "SHARED-GROUP",
        "sHaReD-gRoUp",
    ] {
        let params = vec![
            param("GroupName", name),
            param("GroupDescription", "a duplicate"),
            param("VpcId", &first_vpc),
        ];
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidGroup.Duplicate",
            "case {name:?}: expected InvalidGroup.Duplicate, got {body}"
        );
    }

    // The same name in another VPC is a different group, so it is accepted.
    let elsewhere = vec![
        param("GroupName", "Shared-Group"),
        param("GroupDescription", "the same name, another VPC"),
        param("VpcId", &second_vpc),
    ];
    let (status, body) = create(&router, &elsewhere).await;
    assert_eq!(
        status, 200,
        "the same name in another VPC must be accepted, got {status} with body {body}"
    );
    // And a distinct name in the first VPC is still accepted.
    let distinct = vec![
        param("GroupName", "Another-Group"),
        param("GroupDescription", "a distinct name"),
        param("VpcId", &first_vpc),
    ];
    let (status, body) = create(&router, &distinct).await;
    assert_eq!(
        status, 200,
        "a distinct name must still be accepted, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A name or description outside the documented constraints is refused, and nothing is
/// persisted.
#[tokio::test]
async fn a_name_or_description_outside_its_constraints_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&engine);
    let over_long = "x".repeat(256);

    // Rows: the empty value, one past the 255-character bound, a character outside the
    // documented alphabet, and the `sg-` prefix a NAME may not start with (but a
    // DESCRIPTION may).
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("empty name", "", "a description", "MissingParameter"),
        (
            "name past the bound",
            over_long.as_str(),
            "a description",
            "InvalidParameterValue",
        ),
        (
            "name outside the alphabet",
            "group<name>",
            "a description",
            "InvalidParameterValue",
        ),
        (
            "name shaped like a group id",
            "sg-0123456789abcdef0",
            "a description",
            "InvalidParameterValue",
        ),
        ("empty description", "ok-name", "", "MissingParameter"),
        (
            "description past the bound",
            "ok-name",
            over_long.as_str(),
            "InvalidParameterValue",
        ),
        (
            "description outside the alphabet",
            "ok-name",
            "a <description>",
            "InvalidParameterValue",
        ),
    ];
    for (case, group_name, description, expected_code) in cases {
        let params = vec![
            param("GroupName", group_name),
            param("GroupDescription", description),
            param("VpcId", &vpc_id),
        ];
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            element(&body, "groupId").is_none(),
            "case {case:?}: a refused create reports no group, got {body}"
        );
    }
    assert_eq!(
        held_groups(&engine),
        1,
        "a refused create must persist nothing beyond the default group"
    );

    // A DESCRIPTION may start with `sg-`: the rule is about names, which are ids' near
    // neighbours.
    let allowed = vec![
        param("GroupName", "ok-name"),
        param("GroupDescription", "sg-0123456789abcdef0 is a group id"),
        param("VpcId", &vpc_id),
    ];
    let (status, body) = create(&router, &allowed).await;
    assert_eq!(
        status, 200,
        "a description starting with sg- must be accepted, got {status} with body {body}"
    );
}

/// A `VpcId` that names no VPC of the account is `InvalidVpcID.NotFound`, and nothing is
/// persisted.
#[tokio::test]
async fn a_vpc_id_that_names_no_vpc_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    for vpc_id in ["vpc-0ffffffffffffffff", "vpc-0abc1234", "not-a-vpc-id"] {
        let params = vec![
            param("GroupName", "orphan-group"),
            param("GroupDescription", "a group with no VPC"),
            param("VpcId", vpc_id),
        ];
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {vpc_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcID.NotFound",
            "case {vpc_id:?}: expected InvalidVpcID.NotFound, got {body}"
        );
    }
    assert_eq!(
        held_groups(&engine),
        1,
        "a refused create must persist nothing beyond the default group"
    );
}

/// A request that names no `VpcId` in an account with no DEFAULT VPC has no VPC to fall
/// back on, so it is refused with `VPCIdNotSpecified` rather than creating a group
/// belonging to nothing.
#[tokio::test]
async fn no_vpc_id_and_no_default_vpc_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let default_vpc = default_vpc_id(&engine);

    // The account stops having a default VPC — the state a Region that never got one is in.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        if let Some(vpc) = guard.vpcs.get_mut(&default_vpc) {
            vpc.is_default = Some(false);
        }
    }

    let params = vec![
        param("GroupName", "no-default-vpc"),
        param("GroupDescription", "there is no default VPC to use"),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "VPCIdNotSpecified",
        "expected VPCIdNotSpecified, got {body}"
    );
    assert_eq!(
        held_groups(&engine),
        1,
        "a refused create must persist nothing beyond the default group"
    );

    // Naming the VPC explicitly is still accepted: the refusal is about the request having
    // no VPC to resolve, not about the VPC itself.
    let named = vec![
        param("GroupName", "no-default-vpc"),
        param("GroupDescription", "there is no default VPC to use"),
        param("VpcId", &default_vpc),
    ];
    let (status, body) = create(&router, &named).await;
    assert_eq!(
        status, 200,
        "naming the VPC must still be accepted, got {status} with body {body}"
    );
}

/// Past the number of security groups this store holds, a create is refused with
/// `SecurityGroupLimitExceeded` rather than growing memory without bound.
#[tokio::test]
async fn a_create_past_the_store_s_ceiling_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&engine);

    // Fill the store to its ceiling the way a creating operation would, through the store's
    // own writer, so the ceiling is reached by records and not by a flag.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let mut index = 0;
        while guard.has_security_group_capacity() {
            index += 1;
            guard.insert_security_group(ec2::state::SecurityGroupSpec {
                vpc_id: vpc_id.clone(),
                group_name: format!("filler-{index}"),
                description: "one of the groups filling the store".to_string(),
                region: REGION.to_string(),
                tags: Vec::new(),
                ip_permissions: Vec::new(),
                ip_permissions_egress: vec![ec2::state::allow_all_egress()],
            });
        }
    }
    let held = held_groups(&engine);
    assert_eq!(
        held,
        ec2::state::MAX_SECURITY_GROUPS,
        "the store must be at its ceiling before the create under test"
    );

    let params = vec![
        param("GroupName", "one-too-many"),
        param("GroupDescription", "past the ceiling"),
        param("VpcId", &vpc_id),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "SecurityGroupLimitExceeded",
        "expected SecurityGroupLimitExceeded, got {body}"
    );
    assert_eq!(
        held_groups(&engine),
        held,
        "a refused create must persist nothing"
    );
}

/// A tag specification naming a resource type this action does not tag is refused, and so
/// is one naming none at all, a reserved key, and one key given twice.
#[tokio::test]
async fn a_tag_specification_this_action_cannot_honour_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&engine);

    let cases: Vec<RefusalCase> = vec![
        (
            "another resource type",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "env"),
                param("TagSpecification.1.Tag.1.Value", "dev"),
            ],
            "InvalidParameterValue",
        ),
        (
            "no resource type",
            vec![
                param("TagSpecification.1.Tag.1.Key", "env"),
                param("TagSpecification.1.Tag.1.Value", "dev"),
            ],
            "MissingParameter",
        ),
        (
            "a reserved tag key",
            vec![
                param("TagSpecification.1.ResourceType", "security-group"),
                param("TagSpecification.1.Tag.1.Key", "aws:created-by"),
                param("TagSpecification.1.Tag.1.Value", "me"),
            ],
            "InvalidParameterValue",
        ),
        (
            "one key given twice",
            vec![
                param("TagSpecification.1.ResourceType", "security-group"),
                param("TagSpecification.1.Tag.1.Key", "env"),
                param("TagSpecification.1.Tag.1.Value", "dev"),
                param("TagSpecification.1.Tag.2.Key", "env"),
                param("TagSpecification.1.Tag.2.Value", "prod"),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (case, extra, expected_code) in cases {
        let mut params = vec![
            param("GroupName", "tagged-group"),
            param("GroupDescription", "a tagged group"),
            param("VpcId", &vpc_id),
        ];
        params.extend(extra);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
    assert_eq!(
        held_groups(&engine),
        1,
        "a refused create must persist nothing beyond the default group"
    );
}

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and creates
/// nothing; a dry run of a request that would FAIL reports the failure instead; and
/// `DryRun=false` is an ordinary create.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_creating() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&engine);

    let params = vec![
        param("GroupName", "dry-run-group"),
        param("GroupDescription", "nothing is created"),
        param("VpcId", &vpc_id),
        param("DryRun", "true"),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        held_groups(&engine),
        1,
        "DryRun=true must create nothing"
    );

    // The default VPC already holds a group named `default`, so this request would be
    // refused — and a dry run of it reports THAT, not DryRunOperation.
    let refused = vec![
        param("GroupName", "default"),
        param("GroupDescription", "a duplicate of the default group"),
        param("VpcId", &vpc_id),
        param("DryRun", "true"),
    ];
    let (status, body) = create(&router, &refused).await;
    assert_eq!(
        status, 400,
        "a dry run of a failing request reports the failure, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidGroup.Duplicate",
        "a dry run of a failing request reports its code, got {body}"
    );

    let allowed = vec![
        param("GroupName", "dry-run-group"),
        param("GroupDescription", "nothing is created"),
        param("VpcId", &vpc_id),
        param("DryRun", "false"),
    ];
    let (status, body) = create(&router, &allowed).await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected an ordinary create, got {status} with body {body}"
    );
    assert_eq!(
        held_groups(&engine),
        2,
        "DryRun=false must create the group"
    );
}
