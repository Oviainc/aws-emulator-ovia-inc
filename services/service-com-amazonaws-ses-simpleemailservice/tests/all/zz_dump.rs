// SPDX-License-Identifier: CC0-1.0

//! Service-wide protocol checks. The `zz_` prefix keeps this suite sorted after
//! the per-operation ones; what it asserts is about the whole SES surface rather
//! than one operation's behaviour:
//!
//! * `GetSendQuota` is ROUTED. The scaffold generator declined it (its modeled
//!   input is `smithy.api#Unit`), so before this slice it was not in the
//!   service's operation table at all and the framework answered it through the
//!   unroutable-operation path. It is hand-authored now, and this pins that it
//!   is reached as a modeled operation.
//! * the operations this slice did NOT implement still answer HTTP 501
//!   `NotImplemented` — never an empty 200 that a caller could mistake for real
//!   behaviour.
//! * the operations the generator still declines stay on the
//!   unroutable-operation path, which is the honest answer for an operation
//!   this service does not serve.

use crate::harness::{call, emulator, field};
use smithy_server_runtime::MultiServiceRouter;

/// One awsQuery call carrying `action` plus the given extra form fields.
async fn probe(router: &MultiServiceRouter, action: &str, extra: &[(&str, &str)]) -> (u16, String) {
    let params: Vec<_> = extra
        .iter()
        .map(|(name, value)| field(*name, *value))
        .collect();
    call(router, action, &params).await
}

#[tokio::test]
async fn the_hand_authored_get_send_quota_is_a_routed_operation() {
    let router = emulator();
    let (status, body) = probe(&router, "GetSendQuota", &[]).await;
    assert_eq!(
        status, 200,
        "case GetSendQuota: expected a routed operation answering 200, got {status} with {body}"
    );
    assert!(
        body.contains("<GetSendQuotaResponse") && body.contains("<GetSendQuotaResult>"),
        "case GetSendQuota: expected the awsQuery envelope for this operation, got {body}"
    );
    for member in ["Max24HourSend", "MaxSendRate", "SentLast24Hours"] {
        assert!(
            body.contains(&format!("<{member}>")),
            "case GetSendQuota: expected the required member {member}, got {body}"
        );
    }
}

#[tokio::test]
async fn operations_this_slice_left_unimplemented_still_answer_501() {
    let router = emulator();
    // Operations whose input carries no required member (or whose required
    // members are supplied here), so the request reaches the handler and the
    // answer is the handler's, not the boundary validator's.
    let rows: &[(&str, &[(&str, &str)])] = &[
        ("ListConfigurationSets", &[]),
        ("ListTemplates", &[]),
        ("DescribeActiveReceiptRuleSet", &[]),
        ("CreateReceiptRuleSet", &[("RuleSetName", "inbound")]),
        (
            "GetIdentityDkimAttributes",
            &[("Identities.member.1", "example.com")],
        ),
        (
            "SendCustomVerificationEmail",
            &[("EmailAddress", "a@example.com"), ("TemplateName", "t")],
        ),
    ];
    for (action, extra) in rows {
        let (status, body) = probe(&router, action, extra).await;
        assert_eq!(
            status, 501,
            "case {action}: an unimplemented operation must answer 501, got {status} with {body}"
        );
        assert!(
            body.contains("<Code>NotImplemented</Code>"),
            "case {action}: expected the NotImplemented code, got {body}"
        );
    }
}

#[tokio::test]
async fn operations_the_generator_still_declines_are_not_served() {
    let router = emulator();
    // Still listed in `src/ops/SKIPPED_STUBS.md`: modeled but unwired, so they
    // are answered by the framework's unroutable-operation path rather than
    // pretending to be implemented.
    for action in [
        "GetAccountSendingEnabled",
        "GetSendStatistics",
        "ListVerifiedEmailAddresses",
    ] {
        let (status, body) = probe(&router, action, &[]).await;
        assert_eq!(
            status, 400,
            "case {action}: an unwired operation takes the query protocol's \
             unroutable-operation path (400 InvalidAction), got {status} with {body}"
        );
        assert!(
            body.contains("<Code>InvalidAction</Code>"),
            "case {action}: expected InvalidAction, got {body}"
        );
    }
}
