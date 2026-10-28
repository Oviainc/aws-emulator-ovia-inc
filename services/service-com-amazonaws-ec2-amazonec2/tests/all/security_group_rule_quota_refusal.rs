// SPDX-License-Identifier: CC0-1.0

//! The message an authorize is refused with once a security group holds as many rules in one
//! direction as the account's quota allows, driven over the wire the service speaks:
//! `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! ONE rule is under test: the rules-per-security-group quota is a property of the ACCOUNT —
//! it is adjustable per account through Service Quotas — so the refusal that answers it
//! carries the service's fixed message and names no limit value at all. A caller therefore
//! reads the same sentence whatever the quota in force is, and can tell this refusal from
//! another 400 by matching that sentence.
//!
//! The rows differ in the direction the quota was reached in (inbound and outbound, counted
//! apart), in how the group was filled to it (one bulk grant, or the quota reached exactly
//! and then one grant more), and in how far past the quota the refused call asks — the
//! message is the same in every one of them.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The message AWS refuses an over-quota authorize with, written out here rather than read
/// from the crate so a message that lost its wording still fails: entirely fixed text,
/// ending in a period, with NO variable part — the quota in force is adjustable per account
/// (Service Quotas, service code `vpc`, the quota whose name is "Inbound or outbound rules
/// per security group"), and the service does not name its value here.
const AWS_RULES_PER_GROUP_MESSAGE: &str =
    "The maximum number of rules per security group has been reached.";

/// The error code that refusal carries, which a recorded refusal shows and this suite pins
/// beside the message.
const AWS_RULES_PER_GROUP_CODE: &str = "RulesPerSecurityGroupLimitExceeded";

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// `CreateSecurityGroup` in the account's default VPC, answering the new group's id.
async fn create_group(router: &MultiServiceRouter, case: &str, group_name: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "a group filled to its rule quota"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} with body {body}"
    );
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no groupId in {body}"))
}

/// The form fields of one permission naming `count` distinct single-port IPv4 ranges,
/// starting at range number `first` — so two calls on one group never ask for the same rule.
fn permission(count: usize, first: usize) -> Vec<(String, String)> {
    let mut params = vec![
        param("IpPermissions.1.IpProtocol", "tcp"),
        param("IpPermissions.1.FromPort", "443"),
        param("IpPermissions.1.ToPort", "443"),
    ];
    for index in 0..count {
        let number = first + index;
        params.push(param(
            &format!("IpPermissions.1.IpRanges.{}.CidrIp", index + 1),
            &format!("10.{}.{}.0/24", number / 256, number % 256),
        ));
    }
    params
}

/// One authorize in `direction` on `group_id`, asking for `count` ranges from range number
/// `first`.
async fn authorize(
    router: &MultiServiceRouter,
    direction: Direction,
    group_id: &str,
    count: usize,
    first: usize,
) -> (u16, String) {
    let mut params = vec![param("GroupId", group_id)];
    params.extend(permission(count, first));
    super::ec2_query::call(router, REGION, direction.action(), &params).await
}

/// Which side of the group a row authorises on — the quota is published per direction, so
/// each side has its own bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Inbound,
    Outbound,
}

impl Direction {
    /// The operation that authorises on this side.
    fn action(self) -> &'static str {
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

    /// How many rules of this side a freshly created group already holds: "when you create a
    /// security group, it has no inbound rules" and it carries one allow-all outbound rule.
    fn rules_at_creation(self) -> usize {
        match self {
            Self::Inbound => 0,
            Self::Outbound => 1,
        }
    }
}

/// The over-quota refusal carries the service's FIXED message — no limit value anywhere in
/// it — in every direction, however the group was filled to the quota and however far past
/// it the refused call asks.
#[tokio::test]
async fn the_over_quota_refusal_names_no_limit_value() {
    // The quota in force for this account, read from the one place the emulator holds it:
    // the rows below are built from it so that nothing here depends on its value.
    let quota = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;

    // Each row: the direction, whether the quota is reached in one bulk grant or in two
    // calls, and how many ranges the refused call then asks for.
    let cases: Vec<(String, Direction, bool, usize)> = vec![
        (
            "inbound, filled in one grant, one rule over".to_string(),
            Direction::Inbound,
            false,
            1,
        ),
        (
            "inbound, the last rule granted on its own, one rule over".to_string(),
            Direction::Inbound,
            true,
            1,
        ),
        (
            "inbound, filled in one grant, a whole quota over".to_string(),
            Direction::Inbound,
            false,
            quota,
        ),
        (
            "outbound, filled in one grant, one rule over".to_string(),
            Direction::Outbound,
            false,
            1,
        ),
        (
            "outbound, the last rule granted on its own, two rules over".to_string(),
            Direction::Outbound,
            true,
            2,
        ),
    ];

    for (case, direction, split, over) in &cases {
        let (router, _engine) = super::ec2_query::emulator();
        let group_id = create_group(&router, case, "at-its-quota").await;

        // Fill the direction to exactly the quota, counting the rules the group was created
        // with.
        let to_grant = quota - direction.rules_at_creation();
        let granted = if *split {
            let (status, body) = authorize(&router, *direction, &group_id, to_grant - 1, 1).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 filling the quota, got {status} with \
                 body {body}"
            );
            let (status, body) =
                authorize(&router, *direction, &group_id, 1, to_grant).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for the last rule under the quota, got \
                 {status} with body {body}"
            );
            to_grant
        } else {
            let (status, body) = authorize(&router, *direction, &group_id, to_grant, 1).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 filling the quota, got {status} with \
                 body {body}"
            );
            to_grant
        };

        // One rule more than the quota admits, on the same side.
        let (status, body) =
            authorize(&router, *direction, &group_id, *over, granted + 1).await;
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
            "case {case:?}: expected the message {AWS_RULES_PER_GROUP_MESSAGE:?}, got \
             {message:?}"
        );
        // The quota in force is the account's, so the refusal discloses no value at all —
        // not the emulator's own, and not any other number.
        assert!(
            !message.chars().any(char::is_numeric),
            "case {case:?}: the refusal must name no limit value, got {message:?}"
        );

        // The other direction is a bucket of its own: a side at its quota does not refuse
        // the first rule of the other side.
        let other = match direction {
            Direction::Inbound => Direction::Outbound,
            Direction::Outbound => Direction::Inbound,
        };
        let (status, body) = authorize(&router, other, &group_id, 1, granted + quota + 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the other direction has its own bucket, so expected HTTP 200, \
             got {status} with body {body}"
        );
    }
}

/// The quota is a LIVE COUNT of the rules a group holds in one direction, not a high-water
/// mark: a revoke gives the slot back, so the very grant that was refused is served once a
/// rule has been revoked — and the grant after it is refused again, with the same fixed
/// message. The rows differ in how many rules are revoked and then re-granted.
///
/// Inbound only: `RevokeSecurityGroupEgress` is still a scaffold in this emulator, so there
/// is no call that gives an outbound slot back to assert against.
#[tokio::test]
async fn a_revoke_gives_back_the_slot_the_quota_refused() {
    let quota = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;
    let direction = Direction::Inbound;

    // Each row: how many rules are revoked before granting again.
    for freed in [1_usize, 3, 10] {
        let case = format!("inbound, {freed} rules revoked");
        let case = case.as_str();
        let (router, _engine) = super::ec2_query::emulator();
        let group_id = create_group(&router, case, "at-its-quota").await;
        let to_grant = quota - direction.rules_at_creation();
        let (status, body) = authorize(&router, direction, &group_id, to_grant, 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 filling the quota, got {status} with body {body}"
        );

        // At the quota, one more rule is refused.
        let (status, body) = authorize(&router, direction, &group_id, 1, to_grant + 1).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 at the quota, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            AWS_RULES_PER_GROUP_MESSAGE,
            "case {case:?}: expected the message {AWS_RULES_PER_GROUP_MESSAGE:?}, got {body}"
        );

        // Give `freed` slots back, naming the rules by the same properties they were
        // granted with.
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission(freed, 1));
        let (status, body) =
            super::ec2_query::call(&router, REGION, direction.revoke_action(), &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the revoke, got {status} with body {body}"
        );

        // Exactly the freed slots are available again: that many rules are granted, and one
        // more is refused with the same fixed message.
        let (status, body) = authorize(&router, direction, &group_id, freed, to_grant + 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the {freed} freed slots to be grantable, got {status} \
             with body {body}"
        );
        let (status, body) =
            authorize(&router, direction, &group_id, 1, to_grant + freed + 1).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 back at the quota, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            AWS_RULES_PER_GROUP_MESSAGE,
            "case {case:?}: expected the message {AWS_RULES_PER_GROUP_MESSAGE:?}, got {body}"
        );
    }
}
