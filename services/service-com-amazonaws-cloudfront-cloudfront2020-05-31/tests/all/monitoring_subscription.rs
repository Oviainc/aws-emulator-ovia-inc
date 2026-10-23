// SPDX-License-Identifier: CC0-1.0

//! The CloudFront monitoring subscription sub-resource, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's monitoring subscription
//! surface states:
//!
//! * `CreateMonitoringSubscription` "enables or disables additional Amazon CloudWatch
//!   metrics for the specified CloudFront distribution": it PERSISTS the submitted
//!   subscription on that distribution, so `GetMonitoringSubscription` answers from that
//!   record rather than from the request being served — down to the optional member a
//!   request left out — and one distribution's subscription is no other distribution's;
//! * a monitoring subscription is a SUB-RESOURCE of the distribution, not a member of its
//!   configuration, so associating or releasing one leaves the distribution's
//!   configuration, version token and propagation status exactly as they were;
//! * a distribution carries at most ONE subscription, so a second create is
//!   `MonitoringSubscriptionAlreadyExists` (HTTP 409) and the subscription already stored
//!   stays as it is;
//! * `DeleteMonitoringSubscription` releases it, after which a read is
//!   `NoSuchMonitoringSubscription` (HTTP 404), a second delete is the same, and a create
//!   may associate a subscription again;
//! * an id no distribution answers to is `NoSuchDistribution` (HTTP 404) on all three
//!   calls, whatever shape the id takes.
//!
//! The table rows differ along everything these rules generalise over: every value the
//! model's `RealtimeMetricsSubscriptionStatus` enum admits (taken from the generated
//! `REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES`, so a value no recorded request named is
//! covered too), a subscription that leaves the optional
//! `RealtimeMetricsSubscriptionConfig` wrapper out entirely, and the distribution each row
//! runs against — drawn from this crate's `config_table`, so the distributions differ in
//! origin count, alias count, enabled flag and price class.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config, config_table, create_distribution, emulator, get_distribution,
    is_distribution_id, Answer, API_VERSION,
};

// ---------------------------------------------------------------------------
// Calling the monitoring subscription surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distributions/{DistributionId}/monitoring-subscription` with the
/// `MonitoringSubscription` document `body` — the member the operation binds to the HTTP
/// payload.
async fn create_subscription(
    router: &MultiServiceRouter,
    distribution_id: &str,
    body: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distributions/{distribution_id}/monitoring-subscription"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/distributions/{DistributionId}/monitoring-subscription`.
async fn get_subscription(router: &MultiServiceRouter, distribution_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributions/{distribution_id}/monitoring-subscription"),
        None,
        None,
    )
    .await
}

/// `DELETE /2020-05-31/distributions/{DistributionId}/monitoring-subscription`.
async fn delete_subscription(router: &MultiServiceRouter, distribution_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/distributions/{distribution_id}/monitoring-subscription"),
        None,
        None,
    )
    .await
}

/// Create one distribution from `row` and hand back `(id, etag)` — the identity every
/// monitoring subscription case below runs against.
async fn distribution(
    router: &MultiServiceRouter,
    row: &crate::distribution_lifecycle::Config,
) -> (String, String) {
    let created = create_distribution(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_distribution_id(&id),
        "case {}: expected an `E`+13 distribution id, got {id:?}",
        row.case
    );
    (id, created.header("etag").to_string())
}

// ---------------------------------------------------------------------------
// The subscriptions under test
// ---------------------------------------------------------------------------

/// One `MonitoringSubscription` a client submits.
#[derive(Debug, Clone)]
struct Subscription {
    case: String,
    /// `RealtimeMetricsSubscriptionConfig$RealtimeMetricsSubscriptionStatus`, one value of
    /// the model's `RealtimeMetricsSubscriptionStatus` enum, or `None` to leave the
    /// optional `RealtimeMetricsSubscriptionConfig` wrapper out of the document entirely.
    status: Option<&'static str>,
}

impl Subscription {
    /// The `MonitoringSubscription` XML a client sends for this row.
    fn xml(&self) -> String {
        let mut out = format!(
            "<MonitoringSubscription xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        if let Some(status) = self.status {
            out.push_str(&format!(
                "<RealtimeMetricsSubscriptionConfig>\
                 <RealtimeMetricsSubscriptionStatus>{status}</RealtimeMetricsSubscriptionStatus>\
                 </RealtimeMetricsSubscriptionConfig>"
            ));
        }
        out.push_str("</MonitoringSubscription>");
        out
    }

    /// Assert that `answer` reports exactly this subscription: the status this row
    /// submitted, and no `RealtimeMetricsSubscriptionConfig` at all when the row left the
    /// wrapper out.
    fn assert_matches(&self, answer: &Answer, context: &str) {
        let case = &self.case;
        assert_eq!(
            answer.status, 200,
            "case {case} ({context}): expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let reported = answer.root().find_text("RealtimeMetricsSubscriptionStatus");
        assert_eq!(
            reported,
            self.status,
            "case {case} ({context}): expected RealtimeMetricsSubscriptionStatus {:?}, got \
             {reported:?} in {}",
            self.status,
            answer.raw
        );
        if self.status.is_none() {
            assert!(
                !answer.raw.contains("RealtimeMetricsSubscriptionConfig"),
                "case {case} ({context}): expected no RealtimeMetricsSubscriptionConfig for a \
                 subscription submitted without one, got {}",
                answer.raw
            );
        }
    }
}

/// The table every assertion below runs over: one row per value the model's
/// `RealtimeMetricsSubscriptionStatus` enum admits (from the generated
/// `REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES`, which is the model's own list), plus one
/// row that leaves the optional `RealtimeMetricsSubscriptionConfig` out entirely.
fn subscription_table() -> Vec<Subscription> {
    let mut rows: Vec<Subscription> = REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES
        .iter()
        .map(|status| Subscription {
            case: format!("status {status}"),
            status: Some(status),
        })
        .collect();
    rows.push(Subscription {
        case: "no RealtimeMetricsSubscriptionConfig at all".to_string(),
        status: None,
    });
    rows
}

/// The distribution row a subscription row runs against, cycling through this crate's
/// configuration table so the rows differ in the distribution's own shape as well.
fn distribution_row(index: usize) -> crate::distribution_lifecycle::Config {
    let table = config_table();
    let mut row = table[index % table.len()].clone();
    // One caller reference per distribution: a reference identifies ONE distribution.
    row.caller_reference = format!("{}-monitoring-{index}", row.caller_reference);
    row
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the submitted subscription on the distribution it names, and the read
/// answers from that one record — down to the optional member the request left out. The
/// distribution itself is untouched: its configuration, version token and propagation
/// status are the ones its own create handed back.
#[tokio::test]
async fn create_persists_the_subscription_the_get_then_reports() {
    let router = emulator();

    for (index, row) in subscription_table().into_iter().enumerate() {
        let distribution_row = distribution_row(index);
        let (id, etag) = distribution(&router, &distribution_row).await;
        let case = row.case.clone();

        // Before any create there is nothing to report.
        let missing = get_subscription(&router, &id).await;
        assert_eq!(
            missing.status, 404,
            "case {case}: expected HTTP 404 before any create, got {} with {}",
            missing.status, missing.raw
        );
        assert_eq!(
            missing.error_code(),
            "NoSuchMonitoringSubscription",
            "case {case}: expected NoSuchMonitoringSubscription before any create, got {}",
            missing.raw
        );

        let created = create_subscription(&router, &id, &row.xml()).await;
        row.assert_matches(&created, "the create");

        // The read answers from the stored record, not from any request.
        let fetched = get_subscription(&router, &id).await;
        row.assert_matches(&fetched, "the get");
        // Twice in a row: a read changes nothing about what a later reader sees.
        let again = get_subscription(&router, &id).await;
        row.assert_matches(&again, "the second get");

        // The subscription is a sub-resource of the distribution, so the distribution's
        // own record is exactly as its create left it.
        let distribution_read = get_distribution(&router, &id).await;
        assert_eq!(
            distribution_read.status, 200,
            "case {case}: expected HTTP 200 from GetDistribution, got {} with {}",
            distribution_read.status, distribution_read.raw
        );
        assert_eq!(
            distribution_read.header("etag"),
            etag,
            "case {case}: expected the distribution to keep the version token {etag} its own \
             create handed back, got {:?}",
            distribution_read.header("etag")
        );
        let read_distribution = distribution_read.root();
        distribution_row.assert_matches(
            read_distribution
                .child("DistributionConfig")
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: GetDistribution carried no DistributionConfig: {}",
                        distribution_read.raw
                    )
                }),
            "after CreateMonitoringSubscription",
        );
        assert!(
            !distribution_read.raw.contains("MonitoringSubscription"),
            "case {case}: expected the distribution payload to carry no monitoring \
             subscription, got {}",
            distribution_read.raw
        );
    }
}

/// A subscription belongs to the ONE distribution it was created against: a second
/// distribution created beside it still has none, whatever the first one's subscription
/// says.
#[tokio::test]
async fn a_subscription_belongs_to_one_distribution() {
    let router = emulator();

    for (index, row) in subscription_table().into_iter().enumerate() {
        let case = row.case.clone();
        let (subscribed, _) = distribution(&router, &distribution_row(index)).await;
        let (bare, _) = distribution(
            &router,
            &config("the distribution beside it", &format!("bare-{index}")),
        )
        .await;
        assert_ne!(
            subscribed, bare,
            "case {case}: the two distributions must be distinct"
        );

        row.assert_matches(
            &create_subscription(&router, &subscribed, &row.xml()).await,
            "the create",
        );

        let other = get_subscription(&router, &bare).await;
        assert_eq!(
            other.status, 404,
            "case {case}: expected HTTP 404 for the distribution that was never subscribed, got \
             {} with {}",
            other.status, other.raw
        );
        assert_eq!(
            other.error_code(),
            "NoSuchMonitoringSubscription",
            "case {case}: expected NoSuchMonitoringSubscription for the other distribution, got {}",
            other.raw
        );
        // And the one that IS subscribed still reports its own subscription.
        row.assert_matches(&get_subscription(&router, &subscribed).await, "the get");
    }
}

/// A distribution carries at most ONE monitoring subscription: a second create is
/// `MonitoringSubscriptionAlreadyExists` (HTTP 409), and the subscription already stored
/// keeps the status its own create submitted — the refused request stores nothing.
#[tokio::test]
async fn a_distribution_carries_at_most_one_monitoring_subscription() {
    let router = emulator();
    let table = subscription_table();

    // Every ordered pair of distinct rows, so each row is both the subscription that
    // holds the distribution and the one that is refused. Each pair runs against its own
    // freshly created distribution.
    let mut pair = 0usize;
    for first in table.iter() {
        for second in table.iter().filter(|row| row.case != first.case) {
            let (id, _) = distribution(&router, &distribution_row(pair)).await;
            pair += 1;
            let case = format!("{} then {}", first.case, second.case);
            first.assert_matches(
                &create_subscription(&router, &id, &first.xml()).await,
                "the first create",
            );

            let refused = create_subscription(&router, &id, &second.xml()).await;
            assert_eq!(
                refused.status, 409,
                "case {case}: expected HTTP 409 from the second create, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "MonitoringSubscriptionAlreadyExists",
                "case {case}: expected MonitoringSubscriptionAlreadyExists, got {}",
                refused.raw
            );
            // The envelope carries this code's own sentence; its text is fixed service
            // text rather than anything derived from the request, so what the test pins is
            // that the refusal is worded at all.
            assert!(
                refused
                    .root()
                    .find_text("Message")
                    .is_some_and(|message| !message.trim().is_empty()),
                "case {case}: expected a Message for this code, got {}",
                refused.raw
            );

            // The refusal stored nothing: the first subscription is still the one served.
            first.assert_matches(
                &get_subscription(&router, &id).await,
                "the get after the refused create",
            );
        }
    }
}

/// `DeleteMonitoringSubscription` releases the subscription: the read is then
/// `NoSuchMonitoringSubscription` (HTTP 404), a second delete is the same refusal, and a
/// create may associate a subscription again — a different one, which the read then
/// reports.
#[tokio::test]
async fn delete_releases_the_subscription_a_later_create_may_replace() {
    let router = emulator();
    let table = subscription_table();

    for (index, first) in table.iter().enumerate() {
        let replacement = &table[(index + 1) % table.len()];
        let (id, etag) = distribution(&router, &distribution_row(index)).await;
        let case = format!("{} replaced by {}", first.case, replacement.case);

        first.assert_matches(
            &create_subscription(&router, &id, &first.xml()).await,
            "the first create",
        );

        let deleted = delete_subscription(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected HTTP 200 from the delete, got {} with {}",
            deleted.status, deleted.raw
        );

        for (context, answer) in [
            ("the get after the delete", get_subscription(&router, &id).await),
            ("the second delete", delete_subscription(&router, &id).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case} ({context}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchMonitoringSubscription",
                "case {case} ({context}): expected NoSuchMonitoringSubscription, got {}",
                answer.raw
            );
            assert!(
                answer
                    .root()
                    .find_text("Message")
                    .is_some_and(|message| !message.trim().is_empty()),
                "case {case} ({context}): expected a Message for this code, got {}",
                answer.raw
            );
        }

        // The distribution outlives its subscription, at the version its own create
        // handed back: releasing a sub-resource is no change to the distribution.
        let distribution_read = get_distribution(&router, &id).await;
        assert_eq!(
            distribution_read.header("etag"),
            etag,
            "case {case}: expected the distribution to keep the version token {etag}, got {:?}",
            distribution_read.header("etag")
        );

        replacement.assert_matches(
            &create_subscription(&router, &id, &replacement.xml()).await,
            "the create after the delete",
        );
        replacement.assert_matches(
            &get_subscription(&router, &id).await,
            "the get after the second create",
        );
    }
}

/// An id no distribution answers to is `NoSuchDistribution` (HTTP 404) on all three calls,
/// whatever shape the id takes — and nothing is stored under it, so a later create for a
/// real distribution is unaffected.
#[tokio::test]
async fn an_id_no_distribution_holds_is_refused() {
    let router = emulator();
    // One real distribution, so the refusals below are about the id and not about an empty
    // store.
    let (present, _) = distribution(&router, &distribution_row(0)).await;
    let row = Subscription {
        case: "the one distribution that exists".to_string(),
        status: Some("Enabled"),
    };
    row.assert_matches(
        &create_subscription(&router, &present, &row.xml()).await,
        "the create against the real distribution",
    );

    let absent = [
        // Well-formed CloudFront ids the store never allocated.
        "EDFDVBD6EXAMPLE",
        "E1AAAAAAAAAAAA",
        // Shapes a distribution id never takes at all.
        "not-a-distribution-id",
        "e1lowercase1234",
        "0",
        // A long one, to show the refusal is about the lookup rather than the length.
        "E111111111111111111111111111111111111111111111111",
    ];
    for id in absent {
        assert_ne!(id, present, "the absent ids must not name the real one");
        for (context, answer) in [
            ("create", create_subscription(&router, id, &row.xml()).await),
            ("get", get_subscription(&router, id).await),
            ("delete", delete_subscription(&router, id).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {id} ({context}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchDistribution",
                "case {id} ({context}): expected NoSuchDistribution, got {}",
                answer.raw
            );
        }
    }

    // The refusals touched nothing: the real distribution still reports its subscription.
    row.assert_matches(
        &get_subscription(&router, &present).await,
        "the get after the refusals",
    );
}
