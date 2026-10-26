// SPDX-License-Identifier: CC0-1.0

//! The three SES operations whose modeled input is `smithy.api#Unit`, and the
//! writers that make them observable, end to end over the wire.
//!
//! The scaffold generator declines a Unit-input operation, so
//! `GetAccountSendingEnabled`, `GetSendStatistics` and
//! `ListVerifiedEmailAddresses` were modeled but UNWIRED: absent from the
//! service's operation table, a caller addressing one got the framework's
//! unroutable-operation refusal (`400 InvalidAction`) instead of the modeled
//! output. They are hand-authored now, and every assertion here goes through
//! `MultiServiceRouter::dispatch`, so each call is a real awsQuery request:
//! form-urlencoded body in, XML out, one shared engine behind all of them.
//!
//! What is asserted is the RULE each one follows over a SET of inputs:
//!
//! * the account's sending flag is whatever the last `UpdateAccountSendingEnabled`
//!   wrote, for every sequence of writes, and a fresh account reports the default;
//! * the deprecated address APIs read and write the SAME identity list as the
//!   `*Identity` operations, and the listing reports the EMAIL ADDRESS identities
//!   of that list — never a domain;
//! * the sending statistics are the send log bucketed into the 15-minute periods
//!   the operation documents, over its two-week window.

use crate::harness::{call, element, elements, emulator, emulator_over, field, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    self, SentMessage, SEND_STATISTICS_PERIOD, SEND_STATISTICS_WINDOW,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One call that must succeed, as its XML body.
async fn ok(router: &MultiServiceRouter, action: &str, params: &[Field]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {action}: expected 200, got {status} with {body}"
    );
    body
}

/// `GetAccountSendingEnabled`, as the flag it reports.
async fn account_sending_enabled(router: &MultiServiceRouter) -> bool {
    let body = ok(router, "GetAccountSendingEnabled", &[]).await;
    let enabled = element(&body, "Enabled")
        .unwrap_or_else(|| panic!("GetAccountSendingEnabled must report Enabled, got {body}"));
    enabled.parse::<bool>().unwrap_or_else(|_| {
        panic!("GetAccountSendingEnabled's Enabled must be a boolean, got {enabled:?}")
    })
}

/// `ListVerifiedEmailAddresses`, as the addresses it reports.
async fn verified_addresses(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListVerifiedEmailAddresses", &[]).await;
    element(&body, "VerifiedEmailAddresses")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// `ListIdentities`, as the identities it reports.
async fn identities(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListIdentities", &[]).await;
    element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// One `SendDataPoint`, as the members an assertion is about.
#[derive(Debug, PartialEq, Eq)]
struct DataPoint {
    timestamp: String,
    delivery_attempts: i64,
    bounces: i64,
    complaints: i64,
    rejects: i64,
}

/// `GetSendStatistics`, as its data points in document order.
async fn send_statistics(router: &MultiServiceRouter) -> Vec<DataPoint> {
    let body = ok(router, "GetSendStatistics", &[]).await;
    let list = element(&body, "SendDataPoints").unwrap_or_default();
    elements(&list, "member")
        .into_iter()
        .map(|point| {
            let counter = |tag: &str| -> i64 {
                let text = element(&point, tag)
                    .unwrap_or_else(|| panic!("a data point must carry {tag}, got {point}"));
                text.parse::<i64>()
                    .unwrap_or_else(|_| panic!("{tag} must be a whole number, got {text:?}"))
            };
            DataPoint {
                timestamp: element(&point, "Timestamp")
                    .unwrap_or_else(|| panic!("a data point must carry Timestamp, got {point}")),
                delivery_attempts: counter("DeliveryAttempts"),
                bounces: counter("Bounces"),
                complaints: counter("Complaints"),
                rejects: counter("Rejects"),
            }
        })
        .collect()
}

/// The start of the 15-minute period `at` falls in, the way the operation's own
/// documentation defines a data point ("statistics for a 15-minute period"):
/// periods aligned to the epoch, so one message always falls in the same one.
/// Computed here independently of the service, as the test's oracle.
fn period_start(at: SystemTime) -> SystemTime {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .expect("the test clock stays after the epoch")
        .as_secs();
    let period = SEND_STATISTICS_PERIOD.as_secs();
    UNIX_EPOCH + Duration::from_secs(seconds - seconds % period)
}

/// A timestamp as the query protocol renders it in a response body.
fn wire_timestamp(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the test clock renders")
}

/// A `SendEmail` request from the verified sender, to one To: and one CC:
/// recipient — two recipients, so a send is visible as two delivery attempts.
fn send_params() -> Vec<Field> {
    vec![
        field("Source", "sender@example.com"),
        field("Destination.ToAddresses.member.1", "to@example.net"),
        field("Destination.CcAddresses.member.1", "cc@example.net"),
        field("Message.Subject.Data", "Statistics"),
        field("Message.Body.Text.Data", "Body"),
    ]
}

/// One accepted message in the send log, with `recipients` recipients.
fn logged(index: usize, recipients: usize, sent_at: SystemTime) -> SentMessage {
    SentMessage {
        message_id: format!("seeded-{index}"),
        source: "sender@example.com".to_string(),
        destinations: (0..recipients)
            .map(|slot| format!("r{index}-{slot}@example.net"))
            .collect(),
        subject: None,
        body: None,
        configuration_set: None,
        tags: Vec::new(),
        sent_at,
    }
}

#[tokio::test]
async fn the_account_sending_flag_is_whatever_the_last_write_left() {
    let router = emulator();

    // A fresh account has sending enabled — the state, not a canned answer: the
    // toggles below change what this same reader reports.
    assert!(
        account_sending_enabled(&router).await,
        "case fresh emulator: expected sending enabled"
    );

    // Each row is a sequence of writes; the flag after it must be the LAST value
    // written, so the rows span a single write, a change, a repeat of the value
    // already stored (idempotent), and a longer alternation.
    let rows: &[(&str, &[bool])] = &[
        ("disable", &[false]),
        ("disable then enable", &[false, true]),
        ("enable twice", &[true, true]),
        ("disable twice", &[false, false]),
        ("alternating", &[true, false, true, false, true]),
        ("ending disabled", &[true, false, true, false]),
    ];
    for (case, writes) in rows {
        for enabled in *writes {
            ok(
                &router,
                "UpdateAccountSendingEnabled",
                &[field("Enabled", enabled.to_string())],
            )
            .await;
        }
        let expected = *writes.last().expect("every row writes at least once");
        let reported = account_sending_enabled(&router).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected the flag to be {expected} after {writes:?}, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_disabled_account_refuses_a_send_and_serving_it_again_restores_sending() {
    let router = emulator();
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "sender@example.com")],
    )
    .await;

    // The flag is not just reported back: it is the account state a send is
    // checked against, which is what the operation it belongs to documents
    // ("temporarily pause email sending across your Amazon SES account").
    for enabled in [false, true] {
        ok(
            &router,
            "UpdateAccountSendingEnabled",
            &[field("Enabled", enabled.to_string())],
        )
        .await;
        let (status, body) = call(&router, "SendEmail", &send_params()).await;
        let expected = if enabled { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case sending {enabled}: expected {expected}, got {status} with {body}"
        );
        if !enabled {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccountSendingPausedException"),
                "case sending disabled: expected the modeled refusal, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_deprecated_address_apis_share_the_one_identity_list() {
    let router = emulator();

    assert!(
        verified_addresses(&router).await.is_empty(),
        "case fresh emulator: expected no verified addresses"
    );

    // Addresses spanning the forms a mailbox identity takes: a plain one, a
    // dotted local part with a `+` tag, a mailbox under a subdomain, and the
    // shortest labels that are still an address. The rule is about the identity
    // STORE, so a row's only job is to be a different name.
    let addresses = [
        "ops@example.com",
        "first.last+ses@example.org",
        "alerts@mail.corp.example.net",
        "a@b.co",
    ];
    // Verified through the DEPRECATED operation; every reader of the one list
    // must report them, whichever operation wrote them. Twice each, because
    // verification is idempotent: the list never doubles up.
    for address in addresses {
        for attempt in 1..=2 {
            ok(
                &router,
                "VerifyEmailAddress",
                &[field("EmailAddress", address)],
            )
            .await;
            let listed = verified_addresses(&router).await;
            assert_eq!(
                listed.iter().filter(|listed| *listed == address).count(),
                1,
                "case {address:?} attempt {attempt}: expected exactly one entry, got {listed:?}"
            );
        }
    }

    // One verified through the CURRENT operation, and a domain identity, which
    // is not an address: the listing reports the email-address identities of the
    // list, so `ListIdentities` carries both classes and this one does not.
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "modern@example.com")],
    )
    .await;
    ok(
        &router,
        "VerifyDomainIdentity",
        &[field("Domain", "example.com")],
    )
    .await;

    let mut expected: Vec<String> = addresses
        .iter()
        .map(|address| (*address).to_string())
        .chain(["modern@example.com".to_string()])
        .collect();
    expected.sort();
    let listed = verified_addresses(&router).await;
    assert_eq!(
        listed, expected,
        "case mixed identity classes: expected every verified address in one stable order, got \
         {listed:?}"
    );
    let all = identities(&router).await;
    assert!(
        all.contains(&"example.com".to_string()) && !listed.contains(&"example.com".to_string()),
        "case domain identity: ListIdentities reports it and the address listing must not, got \
         {all:?} / {listed:?}"
    );

    // Removed through the DEPRECATED operation, including one that was verified
    // through the current one, and one that is not in the list at all: a removal
    // that removed nothing still succeeds, and what the caller observes is the
    // absence afterwards.
    for address in addresses
        .iter()
        .chain(["modern@example.com", "ghost@example.net"].iter())
    {
        ok(
            &router,
            "DeleteVerifiedEmailAddress",
            &[field("EmailAddress", *address)],
        )
        .await;
        let listed = verified_addresses(&router).await;
        assert!(
            !listed.contains(&(*address).to_string()),
            "case delete {address:?}: expected it gone, got {listed:?}"
        );
    }
    let listed = verified_addresses(&router).await;
    assert!(
        listed.is_empty(),
        "case every address deleted: expected an empty listing, got {listed:?}"
    );
    let all = identities(&router).await;
    assert_eq!(
        all,
        vec!["example.com".to_string()],
        "case every address deleted: the domain identity is untouched, got {all:?}"
    );
}

#[tokio::test]
async fn send_statistics_bucket_the_send_log_into_fifteen_minute_periods() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    assert!(
        send_statistics(&router).await.is_empty(),
        "case fresh emulator: expected no data points"
    );

    // The rows differ in the ONE thing the rule generalises over: WHEN a message
    // was accepted, and how many recipients it carried. Ages are whole multiples
    // of the period, so a row's bucket is decided by the multiple rather than by
    // where the clock happens to sit inside the current period.
    let now = SystemTime::now();
    let period = SEND_STATISTICS_PERIOD;
    let rows: &[(&str, Duration, usize)] = &[
        ("this period, one recipient", Duration::ZERO, 1),
        ("this period, two more recipients", Duration::ZERO, 2),
        ("the period before", period, 1),
        ("three periods back", period * 3, 4),
        ("a day back", period * 96, 7),
        // On and past the documented two-week window: outside it, so not a data
        // point at all.
        ("exactly the window", SEND_STATISTICS_WINDOW, 5),
        ("past the window", SEND_STATISTICS_WINDOW + period, 6),
    ];
    // Seeded into the log directly, because the ages under test are older than
    // any call this test could make; the wire send below is what proves an
    // accepted send reaches the same log.
    {
        let state = state::service_state(&*engine).expect("this service has a store");
        let mut state = state.lock();
        state.sent_messages = rows
            .iter()
            .enumerate()
            .map(|(index, (_, age, recipients))| {
                let sent_at = now
                    .checked_sub(*age)
                    .expect("the test clock stays after the epoch");
                logged(index, *recipients, sent_at)
            })
            .collect();
    }

    // Expected: one data point per period the log has activity in, oldest first,
    // with every recipient counted as one delivery attempt — and nothing for the
    // periods outside the window.
    let expected: Vec<DataPoint> = [
        (period * 96, 7),
        (period * 3, 4),
        (period, 1),
        (Duration::ZERO, 3),
    ]
    .iter()
    .map(|(age, deliveries)| DataPoint {
        timestamp: wire_timestamp(period_start(
            now.checked_sub(*age)
                .expect("the test clock stays after the epoch"),
        )),
        delivery_attempts: *deliveries,
        bounces: 0,
        complaints: 0,
        rejects: 0,
    })
    .collect();
    let reported = send_statistics(&router).await;
    assert_eq!(
        reported, expected,
        "case seeded log: expected one data point per 15-minute period with activity inside the \
         two-week window, got {reported:?}"
    );

    // An accepted send is counted in the period it was accepted in, by the same
    // rule: two recipients are two delivery attempts, and nothing else moves.
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "sender@example.com")],
    )
    .await;
    ok(&router, "SendEmail", &send_params()).await;
    let sent_period = wire_timestamp(period_start(SystemTime::now()));
    let after = send_statistics(&router).await;
    let total: i64 = after.iter().map(|point| point.delivery_attempts).sum();
    assert_eq!(
        total,
        expected
            .iter()
            .map(|point| point.delivery_attempts)
            .sum::<i64>()
            + 2,
        "case after a two-recipient send: expected two more delivery attempts in all, got {after:?}"
    );
    // The period the send was accepted in carries those two, plus whatever the
    // seeded log already had in that same period (the rows at age zero, unless
    // the clock crossed a period boundary between the seeding and the send).
    let seeded_in_period = expected
        .iter()
        .filter(|point| point.timestamp == sent_period)
        .map(|point| point.delivery_attempts)
        .sum::<i64>();
    let reported_in_period = after
        .iter()
        .filter(|point| point.timestamp == sent_period)
        .map(|point| point.delivery_attempts)
        .sum::<i64>();
    assert_eq!(
        reported_in_period,
        seeded_in_period + 2,
        "case after a two-recipient send: expected the send's own period to carry it, got \
         {after:?}"
    );
}
