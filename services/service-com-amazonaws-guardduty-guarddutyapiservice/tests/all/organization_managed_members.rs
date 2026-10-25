// SPDX-License-Identifier: CC0-1.0

//! The organization precondition on the operations that take a member relationship
//! away from a GuardDuty administrator: `DisassociateMembers` and
//! `StopMonitoringMembers`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! while a detector's stored organization configuration auto-enables GuardDuty for
//! EVERY account of the organization, a call that would end or suspend one of that
//! administrator's member relationships is refused with HTTP 400 under the
//! `InvalidInputException` class and writes nothing — "with
//! `autoEnableOrganizationMembers` configuration for your organization set to `ALL`,
//! you'll receive an error if you attempt to disassociate a member account before
//! removing them from your organization"
//! (`smithy_query(query="doc",shape="DisassociateMembers")`), and the same
//! configuration refuses stopping monitoring
//! (`smithy_query(query="doc",shape="StopMonitoringMembers")`).
//!
//! The cases span it along:
//!
//! 1. the stored `AutoEnableMembers` value — only `ALL` refuses, `NEW` and `NONE`
//!    process, and the two views that write the value agree;
//! 2. the operation — both of the ones the rule covers, each refused with its own
//!    action named and neither moving a stored member;
//! 3. the accounts the request names — one, several, a repeated id, an account that
//!    is no member at all;
//! 4. the detector — the precondition is the NAMED detector's own configuration, so
//!    one administrator's `ALL` does not refuse another's call;
//! 5. the order of the checks — a request the service does not understand is still
//!    refused as such, and a lifted configuration lets the call through again.

use serde_json::{json, Value};

use crate::client_rejections::{invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call_in, error_code, error_shape, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::invitation_listing::enabled_detector_in;
// The organization-configuration harness lives once, in the suite that owns those
// two operations: these are the region-aware forms of the same write and read.
use crate::organization_configuration::{describe_in, update_ok_in};
// The member-call harness lives once, in the member-association suite: these are the
// region-aware forms of the same `CreateMembers` / `GetMembers` calls the other
// member suites make, so no two suites can drift on what a member read looks like.
use crate::member_association::{
    create_members_in, get_members_in, member_of, unprocessed_ids, MALFORMED_ACCOUNT_IDS, MEMBERS,
    NON_MEMBER_ACCOUNT_IDS,
};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// The Region these cases run in: the one the member suites call through.
const REGION: &str = crate::member_association::DEFAULT_REGION;

/// A second Region, which is how a second administrator's detector is reached:
/// GuardDuty holds one detector per account per Region.
const OTHER_REGION: &str = "us-west-2";

/// Every `AutoEnableMembers` member (`smithy_query(shape="AutoEnableMembers")`),
/// paired with whether an organization configured that way manages the GuardDuty
/// relationship of the accounts ALREADY in it. Only `ALL` auto-enables those, so
/// only `ALL` refuses a call that would take one of them away: `NEW` covers the
/// accounts that join from then on, and `NONE` none at all.
const AUTO_ENABLE_MEMBERS: [(&str, bool); 3] = [("NONE", false), ("NEW", false), ("ALL", true)];

/// The operations the precondition spans.
#[derive(Debug, Clone, Copy)]
enum Operation {
    /// `DisassociateMembers`, which ends the association.
    Disassociate,
    /// `StopMonitoringMembers`, which stops monitoring and keeps it.
    StopMonitoring,
}

/// Both of them, so every case below is asserted over the whole class of operations
/// the rule covers rather than over one of them.
const OPERATIONS: [Operation; 2] = [Operation::Disassociate, Operation::StopMonitoring];

impl Operation {
    /// The last path segment the operation is bound at, under
    /// `/detector/{DetectorId}/member`.
    fn path(self) -> &'static str {
        match self {
            Self::Disassociate => "disassociate",
            Self::StopMonitoring => "stop",
        }
    }

    /// The action the refusal names.
    fn action(self) -> &'static str {
        match self {
            Self::Disassociate => "disassociate",
            Self::StopMonitoring => "stop monitoring",
        }
    }

    /// The `relationshipStatus` a member the call PROCESSES ends up with.
    fn moved_to(self) -> &'static str {
        match self {
            Self::Disassociate => "Removed",
            Self::StopMonitoring => "Disabled",
        }
    }
}

/// The message a call that would take a member relationship away is refused with
/// while the organization auto-enables GuardDuty for all of its accounts.
fn organization_managed(action: &str) -> String {
    format!(
        "The request is rejected because the current account cannot {action} the given member account ID while autoEnableOrganizationMembers is set to ALL for the organization. Remove the account from the organization first."
    )
}

/// The `message` an error response carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One of [`OPERATIONS`], called through `region`'s endpoint.
async fn call_operation_in(
    router: &Router,
    region: &str,
    operation: Operation,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/{}", operation.path()),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// Create an enabled detector in `region` and record [`MEMBERS`] against it,
/// returning the detector id.
async fn detector_with_members_in(router: &Router, region: &str) -> String {
    let detector_id = enabled_detector_in(router, region).await;
    let (status, body) = create_members_in(router, region, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "create-members in {region}: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "create-members in {region}: expected every account processed, got {body:?}"
    );
    detector_id
}

/// Every recorded account id, in the order [`MEMBERS`] lists them.
fn all_ids() -> Vec<&'static str> {
    MEMBERS.iter().map(|(id, _)| *id).collect()
}

/// Assert that a response is the organization refusal of `operation`.
fn assert_organization_refusal(case: &str, operation: Operation, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case}: expected the error class {CLASS}, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case}: expected the error shape {ENVELOPE}, got {body:?}"
    );
    assert_eq!(
        message(body),
        organization_managed(operation.action()),
        "case {case}: expected the organization refusal, got {body:?}"
    );
}

/// Assert that every recorded member still reads back exactly as `expected`
/// describes it: `(relationshipStatus, updatedAt)` per account id.
async fn assert_members_unchanged(
    router: &Router,
    region: &str,
    detector_id: &str,
    case: &str,
    expected: &[(String, String, String)],
) {
    let ids = all_ids();
    let (status, got) = get_members_in(router, region, detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "case {case}: expected get-members to answer 200, got {status} {got:?}"
    );
    for (account_id, status_before, updated_before) in expected {
        let member = member_of(&got, account_id).unwrap_or_else(|| {
            panic!("case {case}: expected a member record for {account_id}, got {got:?}")
        });
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some(status_before.as_str()),
            "case {case}: expected {account_id} to keep relationshipStatus {status_before}, got {member:?}"
        );
        assert_eq!(
            member["updatedAt"].as_str(),
            Some(updated_before.as_str()),
            "case {case}: expected {account_id} to keep updatedAt {updated_before}, got {member:?}"
        );
    }
}

/// The `(accountId, relationshipStatus, updatedAt)` of every recorded member, as a
/// read reports them now.
async fn member_snapshot(
    router: &Router,
    region: &str,
    detector_id: &str,
) -> Vec<(String, String, String)> {
    let ids = all_ids();
    let (status, got) = get_members_in(router, region, detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "snapshot: expected get-members to answer 200, got {status} {got:?}"
    );
    ids.iter()
        .map(|account_id| {
            let member = member_of(&got, account_id).unwrap_or_else(|| {
                panic!("snapshot: expected a member record for {account_id}, got {got:?}")
            });
            (
                (*account_id).to_string(),
                member["relationshipStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                member["updatedAt"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1 + 2: the stored auto-enablement decides, for every operation the rule
// covers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_all_refuses_a_call_that_takes_a_member_relationship_away() {
    for operation in OPERATIONS {
        for (value, managed) in AUTO_ENABLE_MEMBERS {
            let case = format!("{operation:?} with autoEnableOrganizationMembers {value}");
            // One store per row: the configuration and the member relationships are
            // both per-detector, so each row starts from a detector of its own.
            let router = new_router();
            let detector_id = detector_with_members_in(&router, REGION).await;
            update_ok_in(
                &router,
                REGION,
                &detector_id,
                json!({"autoEnableOrganizationMembers": value}),
            )
            .await;
            let before = member_snapshot(&router, REGION, &detector_id).await;
            let ids = all_ids();

            let (status, body) =
                call_operation_in(&router, REGION, operation, &detector_id, &ids).await;
            if managed {
                assert_organization_refusal(&case, operation, status, &body);
                assert_members_unchanged(&router, REGION, &detector_id, &case, &before).await;
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            assert!(
                unprocessed_ids(&body).is_empty(),
                "case {case}: expected every member processed, got {body:?}"
            );
            let (status, got) = get_members_in(&router, REGION, &detector_id, &ids).await;
            assert_eq!(
                status, 200,
                "case {case}: expected get-members to answer 200, got {status} {got:?}"
            );
            for account_id in &ids {
                let member = member_of(&got, account_id).unwrap_or_else(|| {
                    panic!("case {case}: expected a member record for {account_id}, got {got:?}")
                });
                assert_eq!(
                    member["relationshipStatus"].as_str(),
                    Some(operation.moved_to()),
                    "case {case}: expected {account_id} to move to {}, got {member:?}",
                    operation.moved_to()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the two views that write the auto-enablement agree about the
// precondition, because they write ONE stored value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_deprecated_boolean_view_auto_enables_only_the_accounts_that_join() {
    for operation in OPERATIONS {
        let case = format!("{operation:?} with the deprecated autoEnable=true");
        let router = new_router();
        let detector_id = detector_with_members_in(&router, REGION).await;
        update_ok_in(&router, REGION, &detector_id, json!({"autoEnable": true})).await;

        // The boolean cannot say "ALL": it is about the accounts that join, which is
        // what `NEW` means, so it never raises the precondition.
        let got = describe_in(&router, REGION, &detector_id).await;
        assert_eq!(
            got["autoEnableOrganizationMembers"].as_str(),
            Some("NEW"),
            "case {case}: expected the boolean view to store NEW, got {got:?}"
        );

        let ids = all_ids();
        let (status, body) =
            call_operation_in(&router, REGION, operation, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: expected every member processed, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the refusal does not depend on which accounts the request names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_refusal_holds_whatever_accounts_the_request_names() {
    let first_member = MEMBERS[0].0;
    let non_member = NON_MEMBER_ACCOUNT_IDS[0].0;
    // Request shapes differing along the count, the repetition and whether the named
    // account is a member of this administrator at all.
    let requests: [(&str, Vec<&str>); 5] = [
        ("every recorded member", all_ids()),
        ("a single member", vec![first_member]),
        ("one member named twice", vec![first_member, first_member]),
        ("an account that is no member", vec![non_member]),
        (
            "a member and a non-member together",
            vec![first_member, non_member],
        ),
    ];

    for operation in OPERATIONS {
        let router = new_router();
        let detector_id = detector_with_members_in(&router, REGION).await;
        update_ok_in(
            &router,
            REGION,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;
        let before = member_snapshot(&router, REGION, &detector_id).await;

        for (shape, account_ids) in &requests {
            let case = format!("{operation:?} naming {shape}");
            let (status, body) =
                call_operation_in(&router, REGION, operation, &detector_id, account_ids).await;
            assert_organization_refusal(&case, operation, status, &body);
            // A refused call reports nothing per account either: the whole request is
            // refused, so there is no `unprocessedAccounts` list at all.
            assert!(
                body["unprocessedAccounts"].is_null(),
                "case {case}: expected no unprocessedAccounts on a refusal, got {body:?}"
            );
            assert_members_unchanged(&router, REGION, &detector_id, &case, &before).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the precondition is the NAMED detector's own organization.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_administrators_configuration_does_not_refuse_anothers_call() {
    for operation in OPERATIONS {
        let case = format!("{operation:?} on a detector of its own");
        let router = new_router();
        let refusing = detector_with_members_in(&router, REGION).await;
        let permitting = detector_with_members_in(&router, OTHER_REGION).await;
        update_ok_in(
            &router,
            REGION,
            &refusing,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;

        let ids = all_ids();
        let (status, body) =
            call_operation_in(&router, OTHER_REGION, operation, &permitting, &ids).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 for the detector whose organization is untouched, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: expected every member processed, got {body:?}"
        );

        let (status, body) =
            call_operation_in(&router, REGION, operation, &refusing, &ids).await;
        assert_organization_refusal(&case, operation, status, &body);
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the order of the checks, and lifting the configuration.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_the_service_does_not_understand_is_still_refused_as_such() {
    for operation in OPERATIONS {
        let router = new_router();
        let detector_id = detector_with_members_in(&router, REGION).await;
        update_ok_in(
            &router,
            REGION,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;

        // A value that cannot be an account id at all is refused for that, not for
        // the organization: the parameter is checked before anything is looked up.
        for (account_id, why) in MALFORMED_ACCOUNT_IDS {
            let case = format!("{operation:?} with {why}");
            let (status, body) =
                call_operation_in(&router, REGION, operation, &detector_id, &[account_id]).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                message(&body),
                invalid_parameter("accountId"),
                "case {case}: expected the malformed-account refusal, got {body:?}"
            );
        }

        // A detector this account/Region does not own is refused for that: the
        // organization configuration read is the resolved detector's own, so there is
        // none to read until the detector resolves.
        let case = format!("{operation:?} on a detector this account does not own");
        let (status, body) = call_operation_in(
            &router,
            REGION,
            operation,
            ABSENT_DETECTOR_ID,
            &[MEMBERS[0].0],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            message(&body),
            not_owned("detectorId"),
            "case {case}: expected the unowned-detector refusal, got {body:?}"
        );
    }
}

#[tokio::test]
async fn lifting_the_configuration_lets_the_call_through_and_restoring_it_refuses_again() {
    for operation in OPERATIONS {
        let router = new_router();
        let detector_id = detector_with_members_in(&router, REGION).await;
        let ids = all_ids();

        update_ok_in(
            &router,
            REGION,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;
        let before = member_snapshot(&router, REGION, &detector_id).await;
        let case = format!("{operation:?} while the organization auto-enables ALL");
        let (status, body) =
            call_operation_in(&router, REGION, operation, &detector_id, &ids).await;
        assert_organization_refusal(&case, operation, status, &body);
        assert_members_unchanged(&router, REGION, &detector_id, &case, &before).await;

        // Removing the auto-enablement removes the precondition: the SAME call now
        // processes every member.
        update_ok_in(
            &router,
            REGION,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "NONE"}),
        )
        .await;
        let case = format!("{operation:?} after the auto-enablement was lifted");
        let (status, body) =
            call_operation_in(&router, REGION, operation, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: expected every member processed, got {body:?}"
        );

        // And restoring it refuses the call again, even though it would now change
        // nothing: the precondition is checked before the members are processed.
        update_ok_in(
            &router,
            REGION,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;
        let processed = member_snapshot(&router, REGION, &detector_id).await;
        let case = format!("{operation:?} after the auto-enablement was restored");
        let (status, body) =
            call_operation_in(&router, REGION, operation, &detector_id, &ids).await;
        assert_organization_refusal(&case, operation, status, &body);
        assert_members_unchanged(&router, REGION, &detector_id, &case, &processed).await;
    }
}
