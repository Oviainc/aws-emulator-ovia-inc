// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEARDOWN of a user's product subscription, driven over the
//! real wire path: every call goes through the router's `dispatch`, so the request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! a call that answers with a resource in a non-terminal state leaves that resource in
//! state, reported in that state, until the work it announces falls due — so
//! `StopProductSubscription`, which answers the subscription it ended (`UNSUBSCRIBED` with
//! the `SubscriptionEndDate` of the call; `ProductUserSummary$Status`, "The status of a
//! product for this user", is a free-form string in the model), keeps the product user
//! reported by `ListProductSubscriptions` carrying that status and that end date, instead
//! of the subscription vanishing between two reads.
//!
//! What this suite pins:
//!
//! * the stop answer and every later list report the same stored subscription as ended,
//!   with the instant it ended unchanged across reads, and the record stops being reported
//!   only once the teardown the clock owes has completed;
//! * the `Status` filter `ListProductSubscriptions` documents therefore selects on
//!   something other than a live subscription: it partitions the ended subscriptions from
//!   the ones still subscribed;
//! * a subscription that has ended is no longer LIVE — a second stop is refused exactly as
//!   one for a subscription that is gone, it is no longer taggable, and the
//!   `(IdentityProvider, Product, Username)` triple it occupies is subscribable again only
//!   once the teardown has completed, when a start mints a NEW product user.
//!
//! The read-back of one subscription is `ListProductSubscriptions`, the only operation of
//! this service that reports a product user besides the two that write one.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState;

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, harness, list_subscriptions, provider_union, register, start_subscription,
    stop_subscription, Harness, Reply, PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use super::resource_resolution_refusals::{
    assert_invalid_resource_arn, list_tags, registration, tag,
};

/// The tags a subscription's `StartProductSubscription` supplies when the row carries any.
fn row_tags() -> Value {
    json!({ "team": "licensing" })
}

/// The user a row leaves SUBSCRIBED beside the one it stops, so a filter has both halves
/// of the partition to select from.
const STILL_SUBSCRIBED: &str = "still.subscribed";

/// One subscription to end. The rows differ along what the rule generalises over: the
/// directory-id form, the product, the user-name form, whether a `Domain` narrows the
/// subscription, whether it carries tags, and which of the two addressings
/// `StopProductSubscription` admits the stop is asked for by.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; both spellings appear below.
    directory_id: &'static str,
    product: &'static str,
    username: &'static str,
    domain: Option<&'static str>,
    tagged: bool,
    /// The stop is addressed by `ProductUserArn` when true, and by the
    /// `(Username, IdentityProvider, Product)` triple when false — the model admits both,
    /// so the lifecycle must not depend on which one a caller uses.
    by_arn: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "aws-managed directory, plain user name, no domain, untagged, stopped by ARN",
        directory_id: "d-0123456789",
        product: PRODUCTS[0],
        username: "alice",
        domain: None,
        tagged: false,
        by_arn: true,
    },
    Row {
        case: "the same directory, a second product, with a domain, tagged, by triple",
        directory_id: "d-0123456789",
        product: PRODUCTS[2],
        username: "bob",
        domain: Some("example.com"),
        tagged: true,
        by_arn: false,
    },
    Row {
        case: "a shared directory, upn-shaped user name, with a domain, by ARN",
        directory_id: "sd-abcdef0123",
        product: PRODUCTS[4],
        username: "carol@example.com",
        domain: Some("corp.example"),
        tagged: false,
        by_arn: true,
    },
    Row {
        case: "another directory, punctuated user name, no domain, tagged, by triple",
        directory_id: "d-00000000ff",
        product: PRODUCTS[3],
        username: "dave.o-brien",
        domain: None,
        tagged: true,
        by_arn: false,
    },
];

impl Row {
    /// The `StartProductSubscription` body for `username` through this row's registration.
    fn start_request(&self, username: &str) -> Value {
        let mut body = json!({
            "Username": username,
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        });
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        if self.tagged && username == self.username {
            body["Tags"] = row_tags();
        }
        body
    }

    /// The `StopProductSubscription` body for this row, in the addressing the row names.
    fn stop_request(&self, arn: &str) -> Value {
        let mut body = if self.by_arn {
            json!({ "ProductUserArn": arn })
        } else {
            json!({
                "Username": self.username,
                "IdentityProvider": provider_union(self.directory_id),
                "Product": self.product,
            })
        };
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// The `StopProductSubscription` body in the OTHER addressing, so the refusal of a
    /// subscription that is no longer live is pinned for both forms.
    fn other_stop_request(&self, arn: &str) -> Value {
        let mut flipped = *self;
        flipped.by_arn = !self.by_arn;
        flipped.stop_request(arn)
    }

    /// The `ListProductSubscriptions` body that reports this row's registration.
    fn list_request(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        })
    }
}

/// A harness whose account has a registration for every `(directory, product)` pair
/// [`ROWS`] names, so a user can be subscribed through each of them.
async fn setup() -> Harness {
    let h = harness();
    for row in ROWS {
        register(
            &h,
            &registration(
                row.directory_id,
                row.product,
                &["subnet-0123456789abcdef0"],
                None,
            ),
        )
        .await
        .ok(row.case);
    }
    h
}

/// Subscribe every row's user and answer the `ProductUserArn` each one minted.
async fn subscribe_rows(h: &Harness) -> Vec<(Row, String)> {
    let mut minted = Vec::new();
    for row in ROWS {
        let reply = start_subscription(h, &row.start_request(row.username)).await;
        let summary = reply.ok(row.case)["ProductUserSummary"].clone();
        minted.push((*row, as_str(&summary, "ProductUserArn", row.case).to_string()));
    }
    minted
}

/// The `ProductUserSummary` of a reply the call answered 200 for.
fn summary(reply: &Reply, case: &str) -> Value {
    reply.ok(case)["ProductUserSummary"].clone()
}

/// Every summary `ListProductSubscriptions` reports for one row's registration, in list
/// order.
async fn listed(h: &Harness, row: &Row, case: &str) -> Vec<Value> {
    let reply = list_subscriptions(h, &row.list_request()).await;
    reply.ok(case)["ProductUserSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ProductUserSummaries in {:?}", reply.body))
        .clone()
}

/// The summary the list reports for `arn`, or `None` when the list does not report it.
async fn listed_summary(h: &Harness, row: &Row, arn: &str, case: &str) -> Option<Value> {
    listed(h, row, case)
        .await
        .into_iter()
        .find(|summary| summary.get("ProductUserArn").and_then(Value::as_str) == Some(arn))
}

/// Assert `instant` is an ISO-8601 UTC instant, the form every date this service reports
/// takes (`2026-10-24T12:48:28Z`).
fn assert_iso8601(instant: &str, case: &str) {
    assert!(
        instant.len() == 20 && instant.ends_with('Z') && instant.contains('T'),
        "case {case:?}: expected an ISO-8601 UTC instant, got {instant:?}"
    );
}

/// RULE: ending a subscription is a teardown the service REPORTS while it runs — the
/// product user stays reported by `ListProductSubscriptions` carrying the status the stop
/// answered and the end date it stamped, unchanged across reads, and stops being reported
/// only once the teardown the clock owes has completed. Neither the addressing used nor
/// the shape of the subscription changes that lifecycle.
#[tokio::test]
async fn an_ended_subscription_stays_reported_until_the_teardown_completes() {
    let h = setup().await;
    let minted = subscribe_rows(&h).await;

    for (row, arn) in &minted {
        let case = row.case;
        let accepted = stop_subscription(&h, &row.stop_request(arn)).await;
        let answered = summary(&accepted, case);
        assert_eq!(
            as_str(&answered, "ProductUserArn", case),
            arn,
            "case {case:?}: the stop must answer the stored subscription, got {answered:?}"
        );
        assert_eq!(
            as_str(&answered, "Status", case),
            "UNSUBSCRIBED",
            "case {case:?}: expected the ended status, got {answered:?}"
        );
        assert_eq!(
            as_str(&answered, "Username", case),
            row.username,
            "case {case:?}: wrong user name, got {answered:?}"
        );
        assert_iso8601(as_str(&answered, "SubscriptionEndDate", case), case);
        assert!(
            answered.get("SubscriptionStartDate").is_some(),
            "case {case:?}: the instant the subscription started is still reported, got \
             {answered:?}"
        );

        // A client listing straight after the call still sees the subscription ending, and
        // a second read sees the same thing: a read does not move the lifecycle on, and
        // nothing is re-stamped.
        for read in ["the first read", "a second read"] {
            let case = format!("{case} ({read})");
            let reported = listed_summary(&h, row, arn, &case).await.unwrap_or_else(|| {
                panic!("case {case:?}: the list must still report {arn} while it is torn down")
            });
            assert_eq!(
                reported, answered,
                "case {case:?}: the list must report the ended subscription exactly as the \
                 stop answered it, got {reported:?}"
            );
        }
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // The teardown the status announced has completed. A READ is enough to see it, whether
    // or not anything was called in between.
    for (row, arn) in &minted {
        let case = format!("{} (past the owed teardown)", row.case);
        assert_eq!(
            listed_summary(&h, row, arn, &case).await,
            None,
            "case {case:?}: the subscription must stop being reported once the teardown the \
             clock owed completed"
        );
        assert!(
            listed(&h, row, &case).await.is_empty(),
            "case {case:?}: the registration has no subscription left"
        );
    }

    // Every view the store keeps of a subscription agrees: a completed teardown unindexed
    // it everywhere, so nothing is left pointing at a record that is gone, and the tags it
    // carried are gone with it.
    {
        let store = h
            .engine
            .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
        let guard = store.lock();
        assert!(
            guard.product_users.is_empty()
                && guard.product_user_order.is_empty()
                && guard.product_user_index.is_empty(),
            "a completed teardown must leave no index pointing at the subscription, got \
             product_users={:?}, order={:?}, index={:?}",
            guard.product_users.keys().collect::<Vec<_>>(),
            guard.product_user_order,
            guard.product_user_index
        );
        for (_, arn) in &minted {
            assert!(
                !guard.tags.contains_key(arn),
                "a subscription that is gone carries no tags, got {:?}",
                guard.tags.get(arn)
            );
        }
    }
}

/// RULE: because an ended subscription is still reported, the `Status` attribute
/// `ListProductSubscriptions` documents as filterable selects on something other than a
/// live subscription — it partitions the subscriptions that have ended from the ones still
/// subscribed, and selects nothing once the ended ones have been torn down.
#[tokio::test]
async fn the_documented_status_filter_selects_an_ended_subscription() {
    let h = setup().await;
    // One user per row stays SUBSCRIBED beside the one that is stopped, so each filter has
    // both halves of the partition to select from.
    let mut live = Vec::new();
    for row in ROWS {
        let reply = start_subscription(&h, &row.start_request(STILL_SUBSCRIBED)).await;
        let summary = summary(&reply, row.case);
        live.push(as_str(&summary, "ProductUserArn", row.case).to_string());
    }
    let minted = subscribe_rows(&h).await;

    /// What one `Status` filter must select.
    struct Selection {
        operation: &'static str,
        value: &'static str,
    }

    for ((row, ended_arn), live_arn) in minted.iter().zip(&live) {
        stop_subscription(&h, &row.stop_request(ended_arn))
            .await
            .ok(row.case);

        for (selection, expected) in [
            (
                Selection {
                    operation: "eq",
                    value: "UNSUBSCRIBED",
                },
                vec![ended_arn.clone()],
            ),
            (
                Selection {
                    operation: "eq",
                    value: "SUBSCRIBED",
                },
                vec![live_arn.clone()],
            ),
            (
                Selection {
                    operation: "ne",
                    value: "SUBSCRIBED",
                },
                vec![ended_arn.clone()],
            ),
            (
                Selection {
                    operation: "ne",
                    value: "UNSUBSCRIBED",
                },
                vec![live_arn.clone()],
            ),
        ] {
            let case = format!(
                "{} (Status {} {})",
                row.case, selection.operation, selection.value
            );
            let selected = selected_by_status(&h, row, selection.operation, selection.value, &case)
                .await;
            assert_eq!(
                selected, expected,
                "case {case:?}: the Status filter must select exactly the subscriptions in \
                 that status"
            );
        }
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    for ((row, _), live_arn) in minted.iter().zip(&live) {
        let case = format!("{} (past the owed teardown)", row.case);
        assert!(
            selected_by_status(&h, row, "eq", "UNSUBSCRIBED", &case)
                .await
                .is_empty(),
            "case {case:?}: nothing is in the ended status once every teardown completed"
        );
        assert_eq!(
            selected_by_status(&h, row, "eq", "SUBSCRIBED", &case).await,
            vec![live_arn.clone()],
            "case {case:?}: the subscription that was never stopped is untouched"
        );
    }
}

/// The `ProductUserArn`s one `Status` filter selects from a row's registration, in list
/// order.
async fn selected_by_status(
    h: &Harness,
    row: &Row,
    operation: &str,
    value: &str,
    case: &str,
) -> Vec<String> {
    let mut body = row.list_request();
    body["Filters"] = json!([{ "Attribute": "Status", "Operation": operation, "Value": value }]);
    let reply = list_subscriptions(h, &body).await;
    reply.ok(case)["ProductUserSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ProductUserSummaries in {:?}", reply.body))
        .iter()
        .map(|summary| as_str(summary, "ProductUserArn", case).to_string())
        .collect()
}

/// RULE: a subscription that has ended is REPORTED so a client can watch the teardown
/// finish, not so work can be built on it — it resolves for no operation that acts on a
/// subscription (a second stop is refused exactly as one for a subscription that is gone),
/// it is no longer taggable, and the `(IdentityProvider, Product, Username)` triple it
/// occupies is subscribable again only once the teardown has completed, when a start mints
/// a NEW product user.
#[tokio::test]
async fn an_ended_subscription_is_no_longer_a_live_subscription() {
    let h = setup().await;
    let minted = subscribe_rows(&h).await;

    for (row, arn) in &minted {
        let case = row.case;
        // The precondition: a live subscription is taggable, reporting whatever tags its
        // start stored (an untagged row reports an empty map).
        let before = list_tags(&h, arn).await;
        let tags = before.ok(case)["Tags"].clone();
        assert_eq!(
            tags.as_object().is_some_and(|tags| !tags.is_empty()),
            row.tagged,
            "case {case:?}: a live subscription reports the tags its start stored, got {:?}",
            before.body
        );

        stop_subscription(&h, &row.stop_request(arn)).await.ok(case);

        // A second stop, by either addressing, is refused: the subscription has already
        // ended, so there is none left to end. WHICH refusal follows the addressing — a
        // `ProductUserArn` that names no live subscription of this account fails closed,
        // and the members that identify a user get the modelled `ResourceNotFoundException`
        // — so both forms are driven here.
        for (form, request, by_arn) in [
            ("the same addressing", row.stop_request(arn), row.by_arn),
            (
                "the other addressing",
                row.other_stop_request(arn),
                !row.by_arn,
            ),
        ] {
            let case = format!("{case}, stopped again through {form}");
            let again = stop_subscription(&h, &request).await;
            let (status, code) = if by_arn {
                (401, "UnauthorizedException")
            } else {
                (404, "ResourceNotFoundException")
            };
            assert_eq!(
                (again.status, again.error_type.as_deref()),
                (status, Some(code)),
                "case {case:?}: expected HTTP {status} {code} for a second stop, got {again:?}"
            );
        }

        // The tagging surface resolves it exactly as it resolves a resource that is gone.
        for (surface, reply) in [
            ("ListTagsForResource", list_tags(&h, arn).await),
            ("TagResource", tag(&h, arn, &row_tags()).await),
        ] {
            assert_invalid_resource_arn(&format!("{case}/{surface} (while ending)"), &reply);
        }

        // The triple is still occupied, so re-subscribing that user to that product
        // conflicts rather than starting a second subscription beside the one ending.
        let conflict = start_subscription(&h, &row.start_request(row.username)).await;
        // `ConflictException` carries `@error("server")` and no `@httpError` in this
        // service's model, so the framework serializes it at HTTP 500.
        assert_eq!(
            conflict.status, 500,
            "case {case:?}: ConflictException is modelled at HTTP 500, got {conflict:?}"
        );
        assert_eq!(
            conflict.error_type.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected ConflictException, got {conflict:?}"
        );

        // None of those refusals is because the subscription is gone: it is still reported
        // in its ended status, which is the whole point of reporting a teardown.
        let reported = listed_summary(&h, row, arn, case).await.unwrap_or_else(|| {
            panic!("case {case:?}: the list must still report {arn} while it is torn down")
        });
        assert_eq!(
            as_str(&reported, "Status", case),
            "UNSUBSCRIBED",
            "case {case:?}: a refused call must leave the ending subscription alone, got \
             {reported:?}"
        );
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    for (row, arn) in &minted {
        let case = format!("{} (past the owed teardown)", row.case);
        let fresh = start_subscription(&h, &row.start_request(row.username)).await;
        let answered = summary(&fresh, &case);
        let fresh_arn = as_str(&answered, "ProductUserArn", &case);
        assert_ne!(
            fresh_arn, arn,
            "case {case:?}: a subscription after a completed teardown is a NEW one, got \
             {answered:?}"
        );
        assert_eq!(
            as_str(&answered, "Status", &case),
            "SUBSCRIBED",
            "case {case:?}: the new subscription starts its own lifecycle, got {answered:?}"
        );
        assert!(
            answered
                .get("SubscriptionEndDate")
                .is_none_or(Value::is_null),
            "case {case:?}: a live subscription has not ended, got {answered:?}"
        );
        let reported: Vec<String> = listed(&h, row, &case)
            .await
            .iter()
            .map(|summary| as_str(summary, "ProductUserArn", &case).to_string())
            .collect();
        assert_eq!(
            reported,
            vec![fresh_arn.to_string()],
            "case {case:?}: the torn-down subscription must not be resurrected beside the \
             new one"
        );
    }
}
