// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisassociateAdminAccount` and the administrator-slot
//! states it moves the association through.
//!
//! The rule asserted here: only the account that currently holds the Firewall
//! Manager default-administrator association may tear it down — every other
//! caller, including when no association exists at all, is refused with
//! `AccessDeniedException` — and a disassociation the holder asks for is not
//! instantaneous: the role moves to the non-terminal `PENDING_DELETION` and
//! reaches `DELETED` only once the completion instant has passed, which is when
//! the administrator slot is free again.
//!
//! Every test drives the service the way a client does, through the shared
//! awsJson1_1 harness in [`crate::policy_lifecycle`] (`POST /` with an
//! `X-Amz-Target` header and a SigV4 credential scope), so the assertions are
//! about the WIRE response.
//!
//! The class the rule is asserted over is the ACCOUNT the association names:
//! the single account this emulator signs its requests as, and a table of
//! account ids that are not it, differing in length and digits within the
//! model's `AWSAccountId` constraint.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::policy_lifecycle::{call, emulator, error_code};

/// The account every request this emulator serves is signed by, so the only
/// account that can hold an association it is allowed to disassociate
/// (`engine::EMULATOR_ACCOUNT_ID`).
const CALLER: &str = engine::EMULATOR_ACCOUNT_ID;

/// Associated accounts that are NOT the caller. The model constrains
/// `AWSAccountId` only to `^[0-9]+$` with length 1..1024, so these differ in
/// length and digits — including the one-digit and the long forms, which the
/// issue did not cite.
const OTHER_ACCOUNTS: &[&str] = &[
    "123456789012",
    "7",
    "000000000001",
    "99999999999999999999",
];

/// The model's own `AccountRoleStatus` catalog
/// (`com.amazonaws.fms#AccountRoleStatus`), the source of every role status
/// asserted below.
const ROLE_STATUSES: &[&str] =
    service_com_amazonaws_fms_awsfms_20180101::ops::types::ACCOUNT_ROLE_STATUS_VALUES;

/// `name` as the model's catalog spells it, so an expectation below names a
/// value `AccountRoleStatus` actually carries instead of a free-standing string.
fn role_status(name: &str) -> &'static str {
    ROLE_STATUSES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!("{name} must be a modelled AccountRoleStatus, catalog is {ROLE_STATUSES:?}")
        })
}

/// How long a requested disassociation stays pending, from the handler's own
/// constant, plus a margin so a waiting test is not racing it. `pub(crate)` so
/// the sibling suites that drive an operation on the far side of a completed
/// tear-down (`policy_administrator_gate`, `admin_accounts_for_organization`,
/// `list_tags_across_administrator_states`) wait out the SAME span rather than
/// keeping a second copy of the wait rule.
///
/// A blocking `std::thread::sleep` on purpose: this workspace's `tokio` is built
/// without the `time` feature (root `Cargo.toml` enables only `macros`, `rt` and
/// `rt-multi-thread`), so `tokio::time::sleep` is not available here, and a test
/// that waits out one chosen 500 ms span has nothing else to do meanwhile.
pub(crate) fn wait_out_the_disassociation() {
    sleep(
        service_com_amazonaws_fms_awsfms_20180101::state::DISASSOCIATION_SPAN
            + Duration::from_millis(250),
    );
}

/// `GetAdminAccount`'s `(http status, parsed body)` — the body carrying
/// `AdminAccount` and `RoleStatus` on success and the error envelope otherwise.
async fn admin_account(router: &smithy_server_runtime::MultiServiceRouter) -> (u16, Value) {
    call(router, "GetAdminAccount", &json!({})).await
}

/// Only the holder of the association may disassociate it: with nothing
/// associated, and with ANY other account associated, the call is refused with
/// `AccessDeniedException` and changes nothing.
#[tokio::test]
async fn disassociate_is_refused_for_a_caller_that_does_not_hold_the_association() {
    // Row 0 is the no-association case the recorded unit exercises; the rest
    // associate an account that is not this caller.
    let mut rows: Vec<Option<&str>> = vec![None];
    rows.extend(OTHER_ACCOUNTS.iter().map(|account| Some(*account)));

    for associated in rows {
        let case = associated.unwrap_or("<none>");
        let router = emulator();

        if let Some(account) = associated {
            let (status, body) = call(
                &router,
                "AssociateAdminAccount",
                &json!({ "AdminAccount": account }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected AssociateAdminAccount to answer 200, got {status} with {body}"
            );
        }

        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {case:?}: a caller that does not hold the association must be refused, \
             got {status} {body}"
        );

        // The refusal changed nothing: the slot is exactly as it was.
        let (status, got) = admin_account(&router).await;
        match associated {
            None => assert_eq!(
                (status, error_code(&got).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: the refusal must not have created an association, \
                 got {status} {got}"
            ),
            Some(account) => assert_eq!(
                (status, got["AdminAccount"].as_str(), got["RoleStatus"].as_str()),
                (200, Some(account), Some(role_status("READY"))),
                "case {case:?}: the refused disassociation must have left the association \
                 READY, got {status} {got}"
            ),
        }
    }
}

/// The holder's own disassociation is accepted, moves the role to
/// `PENDING_DELETION` for a non-zero span, reaches `DELETED` after it, and frees
/// the administrator slot for the next association — whichever account takes it.
#[tokio::test]
async fn the_holder_disassociates_and_the_role_is_deleted_after_the_pending_span() {
    // The next holder of the freed slot: the same account again, and every
    // other account in the table.
    let mut next_holders: Vec<&str> = vec![CALLER];
    next_holders.extend(OTHER_ACCOUNTS.iter().copied());

    for next in next_holders {
        let case = format!("{CALLER}->{next}");
        let router = emulator();

        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": CALLER }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected AssociateAdminAccount to answer 200, got {status} with {body}"
        );

        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: the holder's disassociation must be accepted, got {status} with {body}"
        );

        // The span is never zero: a caller reading straight after the call still
        // sees the non-terminal status.
        let (status, got) = admin_account(&router).await;
        assert_eq!(
            (status, got["AdminAccount"].as_str(), got["RoleStatus"].as_str()),
            (200, Some(CALLER), Some(role_status("PENDING_DELETION"))),
            "case {case}: right after the disassociation the role must be pending deletion, \
             got {status} {got}"
        );

        // Asking again while it is pending is idempotent and does not push the
        // completion instant out.
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: a repeated disassociation must be idempotent, got {status} with {body}"
        );
        let (_, got) = admin_account(&router).await;
        assert_eq!(
            got["RoleStatus"].as_str(),
            Some(role_status("PENDING_DELETION")),
            "case {case}: the repeated call must not have changed the pending status, got {got}"
        );

        wait_out_the_disassociation();

        // Completed: the role is DELETED, and repeated reads all answer the same.
        let (status, got) = admin_account(&router).await;
        assert_eq!(
            (status, got["AdminAccount"].as_str(), got["RoleStatus"].as_str()),
            (200, Some(CALLER), Some(role_status("DELETED"))),
            "case {case}: once the pending span has passed the role must be DELETED, \
             got {status} {got}"
        );
        for round in 1..4 {
            let (again_status, again) = admin_account(&router).await;
            assert_eq!(
                (again_status, &again),
                (status, &got),
                "case {case}: read {round} of GetAdminAccount must answer identically, \
                 got {again_status} {again}"
            );
        }

        // Nobody holds the association now, so disassociating again is refused.
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {case}: disassociating a deleted association must be refused, \
             got {status} {body}"
        );

        // And the slot is free: the next account becomes the administrator.
        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": next }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the freed slot must accept a new association, got {status} with {body}"
        );
        let (status, got) = admin_account(&router).await;
        assert_eq!(
            (status, got["AdminAccount"].as_str(), got["RoleStatus"].as_str()),
            (200, Some(next), Some(role_status("READY"))),
            "case {case}: the new association must be reported READY, got {status} {got}"
        );
    }
}

/// While a disassociation is still pending there is no association to make: the
/// account is on its way out, so `AssociateAdminAccount` is refused with the
/// modelled `InvalidOperationException` — for the same account and for another.
#[tokio::test]
async fn associating_over_a_pending_disassociation_is_refused() {
    let mut candidates: Vec<&str> = vec![CALLER];
    candidates.extend(OTHER_ACCOUNTS.iter().copied());

    for candidate in candidates {
        let router = emulator();
        call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": CALLER }),
        )
        .await;
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {candidate:?}: the holder's disassociation must be accepted, \
             got {status} with {body}"
        );

        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": candidate }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {candidate:?}: associating while the disassociation is pending must be \
             refused, got {status} {body}"
        );

        let (_, got) = admin_account(&router).await;
        assert_eq!(
            (got["AdminAccount"].as_str(), got["RoleStatus"].as_str()),
            (Some(CALLER), Some(role_status("PENDING_DELETION"))),
            "case {candidate:?}: the refused association must not have changed the slot, \
             got {got}"
        );
    }
}
