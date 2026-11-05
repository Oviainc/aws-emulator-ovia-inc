// SPDX-License-Identifier: CC0-1.0

//! The buckets the rules-per-security-group quota is counted in, driven over the wire the
//! service speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! ONE rule is under test: the published "inbound or outbound rules per security group"
//! quota is a budget per GROUP, per DIRECTION and per IP ADDRESS FAMILY, so a budget that
//! has been reached refuses only the rules counted in it — a group whose inbound IPv4 rules
//! are at the quota still takes an inbound IPv6 rule, an outbound rule of either family, and
//! the IPv4 rule a revoke has just made room for.
//!
//! The rows span the four buckets of one group (both directions crossed with both families),
//! how the budget was filled (one bulk grant, or the quota reached in two calls), and — for
//! a grant that names no address range of its own — the kind of source it names: a security
//! group, which admits traffic of either family, and a prefix list, which admits the family
//! its own `AddressFamily` names.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The error code an over-quota authorize carries.
const AWS_RULES_PER_GROUP_CODE: &str = "RulesPerSecurityGroupLimitExceeded";

/// The message it carries, written out here rather than read from the crate so a message
/// that lost its wording still fails: entirely fixed text, naming no limit value, because
/// the quota in force is adjustable per account.
const AWS_RULES_PER_GROUP_MESSAGE: &str =
    "The maximum number of rules per security group has been reached.";

/// The protocol and port every rule in this suite grants on, so two rules differ in nothing
/// but the source they name.
const PORT: &str = "4433";

/// Which side of a group a rule is on. The quota is published per direction, so each side
/// carries buckets of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Inbound,
    Outbound,
}

impl Direction {
    /// The operation that adds a rule on this side.
    fn authorize_action(self) -> &'static str {
        match self {
            Self::Inbound => "AuthorizeSecurityGroupIngress",
            Self::Outbound => "AuthorizeSecurityGroupEgress",
        }
    }

    /// The operation that takes a rule away on this side.
    fn revoke_action(self) -> &'static str {
        match self {
            Self::Inbound => "RevokeSecurityGroupIngress",
            Self::Outbound => "RevokeSecurityGroupEgress",
        }
    }

    /// The `IsEgress` a rule on this side is reported with.
    fn is_egress(self) -> bool {
        self == Self::Outbound
    }
}

/// The IP address family a grant admits traffic of. The two are counted apart, so each is a
/// bucket of its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Ipv4,
    Ipv6,
}

impl Family {
    /// The `AddressFamily` a managed prefix list of this family is created with — the two
    /// values `CreateManagedPrefixListRequest$AddressFamily` documents ("Valid Values: IPv4
    /// | IPv6").
    fn prefix_list_address_family(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }

    /// Range number `number` of this family, in canonical form: a distinct block per number,
    /// so two grants differ in nothing but the block.
    fn range(self, number: usize) -> String {
        match self {
            Self::Ipv4 => format!("10.{}.{}.0/24", number / 256, number % 256),
            Self::Ipv6 => format!("2001:db8:{:x}:{:x}::/64", number / 256, number % 256),
        }
    }
}

/// One bucket of one group's quota: a direction and an address family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bucket {
    direction: Direction,
    family: Family,
}

/// Every bucket one security group has.
const BUCKETS: [Bucket; 4] = [
    Bucket {
        direction: Direction::Inbound,
        family: Family::Ipv4,
    },
    Bucket {
        direction: Direction::Inbound,
        family: Family::Ipv6,
    },
    Bucket {
        direction: Direction::Outbound,
        family: Family::Ipv4,
    },
    Bucket {
        direction: Direction::Outbound,
        family: Family::Ipv6,
    },
];

/// The one source a rule names: "a CIDR range, security group, or prefix list"
/// (`RevokeSecurityGroupIngress`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Grant {
    /// An address range of one family.
    Range(Family, String),
    /// A managed prefix list, by id.
    PrefixList(String),
    /// Another security group, by id.
    Group(String),
}

impl Grant {
    /// Whether a rule granting this source is counted in `family`'s bucket of its side: a
    /// range is counted in the bucket of its own family, and a source that names no address
    /// range at all is counted in every bucket.
    fn charges(&self, family: Family) -> bool {
        match self {
            Self::Range(grant_family, _) => *grant_family == family,
            Self::PrefixList(_) | Self::Group(_) => true,
        }
    }
}

/// `count` range grants of `family`, starting at range number `first`.
fn ranges(family: Family, count: usize, first: usize) -> Vec<Grant> {
    (0..count)
        .map(|offset| Grant::Range(family, family.range(first + offset)))
        .collect()
}

/// The grants as the `IpPermissions.1.…` form fields of one permission — the keys the
/// model's `@ec2QueryName`s resolve to, note `Groups.N` for `UserIdGroupPairs`
/// (`smithy_query(query="shape",shape="IpPermission")`).
fn permission_params(grants: &[Grant]) -> Vec<Param> {
    let mut params = vec![
        param("IpPermissions.1.IpProtocol", "tcp"),
        param("IpPermissions.1.FromPort", PORT),
        param("IpPermissions.1.ToPort", PORT),
    ];
    let mut ipv4 = 0usize;
    let mut ipv6 = 0usize;
    let mut lists = 0usize;
    let mut groups = 0usize;
    for grant in grants {
        match grant {
            Grant::Range(Family::Ipv4, cidr) => {
                ipv4 += 1;
                params.push(param(
                    &format!("IpPermissions.1.IpRanges.{ipv4}.CidrIp"),
                    cidr,
                ));
            }
            Grant::Range(Family::Ipv6, cidr) => {
                ipv6 += 1;
                params.push(param(
                    &format!("IpPermissions.1.Ipv6Ranges.{ipv6}.CidrIpv6"),
                    cidr,
                ));
            }
            Grant::PrefixList(prefix_list_id) => {
                lists += 1;
                params.push(param(
                    &format!("IpPermissions.1.PrefixListIds.{lists}.PrefixListId"),
                    prefix_list_id,
                ));
            }
            Grant::Group(group_id) => {
                groups += 1;
                params.push(param(
                    &format!("IpPermissions.1.Groups.{groups}.GroupId"),
                    group_id,
                ));
            }
        }
    }
    params
}

/// One rule as `DescribeSecurityGroupRules` reports it, in the terms the quota counts it in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedRule {
    id: String,
    is_egress: bool,
    grant: Grant,
}

/// `CreateSecurityGroup` in the account's default VPC, answering the new group's id.
async fn create_group(router: &MultiServiceRouter, case: &str, group_name: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param(
                "GroupDescription",
                "a group whose quota buckets are counted apart",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} with body \
         {body}"
    );
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no groupId in {body}"))
}

/// `CreateManagedPrefixList` of `family`, answering the new list's id.
async fn create_prefix_list(router: &MultiServiceRouter, case: &str, family: Family) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateManagedPrefixList",
        &[
            param(
                "PrefixListName",
                &format!("quota-buckets-{}", family.prefix_list_address_family()),
            ),
            param("AddressFamily", family.prefix_list_address_family()),
            param("MaxEntries", "5"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateManagedPrefixList, got {status} with \
         body {body}"
    );
    element(&body, "prefixListId")
        .unwrap_or_else(|| panic!("case {case:?}: no prefixListId in {body}"))
}

/// One authorize on `direction` of `group_id`, granting `grants` as a single permission.
async fn authorize(
    router: &MultiServiceRouter,
    direction: Direction,
    group_id: &str,
    grants: &[Grant],
) -> (u16, String) {
    let mut params = vec![param("GroupId", group_id)];
    params.extend(permission_params(grants));
    super::ec2_query::call(router, REGION, direction.authorize_action(), &params).await
}

/// The same authorize, asserting it was served.
async fn expect_accepted(
    router: &MultiServiceRouter,
    case: &str,
    direction: Direction,
    group_id: &str,
    grants: &[Grant],
) {
    let (status, body) = authorize(router, direction, group_id, grants).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 granting {count} rules on the {direction:?} side, \
         got {status} with body {body}",
        count = grants.len()
    );
}

/// The source one reported rule names.
fn grant_of(case: &str, item: roxmltree::Node<'_, '_>) -> Grant {
    if let Some(cidr) = child_text(item, "cidrIpv4") {
        return Grant::Range(Family::Ipv4, cidr);
    }
    if let Some(cidr) = child_text(item, "cidrIpv6") {
        return Grant::Range(Family::Ipv6, cidr);
    }
    if let Some(prefix_list_id) = child_text(item, "prefixListId") {
        return Grant::PrefixList(prefix_list_id);
    }
    if let Some(info) = child(item, "referencedGroupInfo") {
        return Grant::Group(child_text(info, "groupId").unwrap_or_default());
    }
    panic!("case {case:?}: a reported rule names no source: {item:?}");
}

/// Every rule `DescribeSecurityGroupRules` reports for `group_id`, in the order it reported
/// them.
async fn reported_rules(
    router: &MultiServiceRouter,
    case: &str,
    group_id: &str,
) -> Vec<ReportedRule> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroupRules",
        &filters(&[("group-id", &[group_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroupRules, got {status} with \
         body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "item"
                && child_text(*node, "securityGroupRuleId").is_some()
        })
        .map(|item| ReportedRule {
            id: child_text(item, "securityGroupRuleId").unwrap_or_default(),
            is_egress: child_text(item, "isEgress").as_deref() == Some("true"),
            grant: grant_of(case, item),
        })
        .collect()
}

/// How many of the reported rules are counted in `bucket`.
fn in_bucket(rules: &[ReportedRule], bucket: Bucket) -> usize {
    rules
        .iter()
        .filter(|rule| {
            rule.is_egress == bucket.direction.is_egress() && rule.grant.charges(bucket.family)
        })
        .count()
}

/// Assert an authorize was refused as an over-quota one, with nothing stored.
async fn expect_over_quota(
    router: &MultiServiceRouter,
    case: &str,
    direction: Direction,
    group_id: &str,
    grants: &[Grant],
    before: &[ReportedRule],
) {
    let (status, body) = authorize(router, direction, group_id, grants).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 past the quota, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        AWS_RULES_PER_GROUP_CODE,
        "case {case:?}: expected {AWS_RULES_PER_GROUP_CODE}, got {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, AWS_RULES_PER_GROUP_MESSAGE,
        "case {case:?}: expected the message {AWS_RULES_PER_GROUP_MESSAGE:?}, got {message:?}"
    );
    let after = reported_rules(router, case, group_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused authorize must store nothing, got {after:?}"
    );
}

/// The rule: the quota is a budget per group, direction and address family. A bucket filled
/// to the quota refuses one more rule of its own, while every OTHER bucket of the same group
/// still takes one — and a revoke in the filled bucket gives the refused rule its slot back.
#[tokio::test]
async fn every_direction_and_family_pair_is_its_own_quota_bucket() {
    let quota = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;
    // The rows: each bucket of a group, filled either in one bulk grant or in two calls —
    // what the rule generalises over is the bucket, not how it came to be full.
    let rows: Vec<(Bucket, bool)> = BUCKETS
        .iter()
        .flat_map(|bucket| [(*bucket, false), (*bucket, true)])
        .collect();

    for (bucket, split) in rows {
        let filled_in = if split { "two calls" } else { "one grant" };
        let case = format!(
            "{:?} {:?} filled in {filled_in}",
            bucket.direction, bucket.family
        );
        let case = case.as_str();
        let (router, _engine) = super::ec2_query::emulator();
        let group_id = create_group(&router, case, "quota-buckets").await;

        // What the group already holds in that bucket: "when you create a security group, it
        // has no inbound rules", and it carries one allow-all OUTBOUND rule, over IPv4.
        let at_creation = reported_rules(&router, case, &group_id).await;
        let held = in_bucket(&at_creation, bucket);
        let to_grant = quota - held;
        assert!(
            to_grant >= 2,
            "case {case:?}: the rows below fill the bucket in up to two calls, so it must \
             start with room for at least two rules, got {to_grant}"
        );

        // Fill the bucket to exactly the quota.
        let mut next = 1usize;
        if split {
            expect_accepted(
                &router,
                case,
                bucket.direction,
                &group_id,
                &ranges(bucket.family, to_grant - 1, next),
            )
            .await;
            next += to_grant - 1;
            expect_accepted(
                &router,
                case,
                bucket.direction,
                &group_id,
                &ranges(bucket.family, 1, next),
            )
            .await;
            next += 1;
        } else {
            expect_accepted(
                &router,
                case,
                bucket.direction,
                &group_id,
                &ranges(bucket.family, to_grant, next),
            )
            .await;
            next += to_grant;
        }
        let at_quota = reported_rules(&router, case, &group_id).await;
        assert_eq!(
            in_bucket(&at_quota, bucket),
            quota,
            "case {case:?}: expected the bucket filled to the quota of {quota}, got the \
             rules {at_quota:?}"
        );

        // One more rule of that bucket is refused, and nothing is stored for it.
        let refused = ranges(bucket.family, 1, next);
        next += 1;
        expect_over_quota(
            &router,
            case,
            bucket.direction,
            &group_id,
            &refused,
            &at_quota,
        )
        .await;

        // Every other bucket of the same group still takes a rule: the other family of this
        // direction, and both families of the other direction.
        for other in BUCKETS.iter().filter(|other| **other != bucket) {
            let granted = ranges(other.family, 1, next);
            next += 1;
            let (status, body) = authorize(&router, other.direction, &group_id, &granted).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {:?} {:?} is a bucket of its own, so expected HTTP 200, got \
                 {status} with body {body}",
                other.direction, other.family
            );
        }

        // The quota is a LIVE count, not a high-water mark: a revoke in the filled bucket
        // gives its slot back, and the very grant that was refused is then served.
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission_params(&ranges(bucket.family, 1, 1)));
        let (status, body) =
            super::ec2_query::call(&router, REGION, bucket.direction.revoke_action(), &params)
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the revoke, got {status} with body {body}"
        );
        expect_accepted(&router, case, bucket.direction, &group_id, &refused).await;
    }
}

/// Assert an authorize naming a prefix list the account cannot see was refused with that
/// resource's own not-found class, with nothing stored.
async fn expect_unknown_prefix_list(
    router: &MultiServiceRouter,
    case: &str,
    direction: Direction,
    group_id: &str,
    grants: &[Grant],
    before: &[ReportedRule],
) {
    let (status, body) = authorize(router, direction, group_id, grants).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a prefix list the account cannot see, got \
         {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_PREFIX_LIST_ID_NOT_FOUND,
        "case {case:?}: expected the prefix list's not-found code, got {body}"
    );
    let after = reported_rules(router, case, group_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused authorize must store nothing, got {after:?}"
    );
}

/// The rule, for the sources that name no address range of their own: a prefix list is
/// counted in the bucket of the family its own `AddressFamily` names, while a referenced
/// security group — which admits that group's traffic of either family — is counted in every
/// bucket. A prefix list this account holds no record of is never counted at all: it grants
/// nothing, so it is refused with that resource's own not-found class before any bucket is
/// asked about (the suite `security_group_rule_prefix_lists.rs` is that rule's own).
#[tokio::test]
async fn a_source_that_names_no_range_is_counted_in_the_buckets_it_admits() {
    let quota = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;
    // A well-formed prefix list id this account holds no list for.
    const UNKNOWN_PREFIX_LIST: &str = "pl-0abcd1234ef567890";
    /// What the authorize of the row's source must answer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Outcome {
        /// The bucket the source charges has room, so the rule is stored.
        Served,
        /// The bucket the source charges is full.
        OverQuota,
        /// The source names a prefix list the account cannot see.
        UnknownList,
    }
    // Each row: the family whose inbound bucket is filled to the quota, the kind of source
    // the next rule names, and what the authorize of it answers.
    let rows: [(Family, &str, Outcome); 8] = [
        (Family::Ipv4, "an IPv6 prefix list", Outcome::Served),
        (Family::Ipv4, "an IPv4 prefix list", Outcome::OverQuota),
        (
            Family::Ipv4,
            "a referenced security group",
            Outcome::OverQuota,
        ),
        (
            Family::Ipv4,
            "an unknown prefix list",
            Outcome::UnknownList,
        ),
        (Family::Ipv6, "an IPv4 prefix list", Outcome::Served),
        (Family::Ipv6, "an IPv6 prefix list", Outcome::OverQuota),
        (
            Family::Ipv6,
            "a referenced security group",
            Outcome::OverQuota,
        ),
        (
            Family::Ipv6,
            "an unknown prefix list",
            Outcome::UnknownList,
        ),
    ];

    for (full, source, outcome) in rows {
        let case = format!("{full:?} inbound at the quota, granting {source}");
        let case = case.as_str();
        let (router, _engine) = super::ec2_query::emulator();
        let group_id = create_group(&router, case, "quota-buckets-sources").await;
        let ipv4_list = create_prefix_list(&router, case, Family::Ipv4).await;
        let ipv6_list = create_prefix_list(&router, case, Family::Ipv6).await;
        let grant = match source {
            "an IPv4 prefix list" => Grant::PrefixList(ipv4_list),
            "an IPv6 prefix list" => Grant::PrefixList(ipv6_list),
            "an unknown prefix list" => Grant::PrefixList(UNKNOWN_PREFIX_LIST.to_string()),
            _ => Grant::Group(group_id.clone()),
        };

        // Fill one inbound bucket to the quota with ranges of its own family, which leaves
        // the other family's bucket empty.
        let bucket = Bucket {
            direction: Direction::Inbound,
            family: full,
        };
        expect_accepted(
            &router,
            case,
            Direction::Inbound,
            &group_id,
            &ranges(full, quota, 1),
        )
        .await;
        let at_quota = reported_rules(&router, case, &group_id).await;
        assert_eq!(
            in_bucket(&at_quota, bucket),
            quota,
            "case {case:?}: expected the bucket filled to the quota of {quota}, got the \
             rules {at_quota:?}"
        );

        if outcome == Outcome::Served {
            expect_accepted(
                &router,
                case,
                Direction::Inbound,
                &group_id,
                std::slice::from_ref(&grant),
            )
            .await;
            // Round trip: the rule the authorize accepted is one a reader reports.
            let after = reported_rules(&router, case, &group_id).await;
            let added: Vec<&ReportedRule> = after
                .iter()
                .filter(|rule| !at_quota.iter().any(|held| held.id == rule.id))
                .collect();
            assert_eq!(
                added.len(),
                1,
                "case {case:?}: expected exactly one rule added, got {added:?}"
            );
            assert_eq!(
                added[0].grant, grant,
                "case {case:?}: expected the added rule to name {grant:?}, got {:?}",
                added[0]
            );
        } else if outcome == Outcome::UnknownList {
            expect_unknown_prefix_list(
                &router,
                case,
                Direction::Inbound,
                &group_id,
                &[grant],
                &at_quota,
            )
            .await;
        } else {
            expect_over_quota(
                &router,
                case,
                Direction::Inbound,
                &group_id,
                &[grant],
                &at_quota,
            )
            .await;
        }
    }
}
