// SPDX-License-Identifier: CC0-1.0

//! The spelling a continuation token of this service is honoured in: the one a page handed
//! it out in, and no other.
//!
//! The rule under test: a `NextToken` is opaque, so a paginated reader resumes only for the
//! exact value one of its own pages issued — a value that merely DECODES to the same bytes
//! as an issued token is a value the service never issued, and names no resume point, so it
//! is refused with the modelled `ValidationException` (HTTP 400) naming it. The hex layer
//! the shared codec prints a token with is where that is lost if the decoder is permissive:
//! a hex decoder admits `A`-`F` as well as the `a`-`f` the encoder writes, so every
//! case-altered re-spelling of an issued token carries the same bytes while being a value
//! no page ever produced (a client upper-casing a token, an intermediary normalising its
//! case).
//!
//! The table spans the two things the rule generalises over:
//!
//! * WHICH reader is asked — all four paginated readers of this service, whose tokens differ
//!   in marker, in arity and in where the request binds `NextToken` (the query string or the
//!   request body); and
//! * HOW the issued token is re-spelled — every hex digit upper-cased, only the first
//!   case-bearing digit, only the last one, and alternating ones; each row asserts the
//!   re-spelling really differs from the issued token and really decodes to the same bytes,
//!   so no row can pass by not being a re-spelling at all.
//!
//! The token each reader judges is one IT issued, taken from a first page of its own seeded
//! store, and the issued spelling itself is driven through as the control row: the rule is
//! about the token's provenance, not about refusing tokens.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    call, create, created_metric_definitions, emulator, experiment_arn, message,
    metrics_destination_query, percent_encoded, put_metrics_destination, rum_events_body, Reply,
};

/// The `MetricDestination` value that carries no `DestinationArn`.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` — an enum of
/// `CloudWatch` and `Evidently`.
const CLOUD_WATCH: &str = "CloudWatch";

/// The other `MetricDestination` value, which names the Evidently experiment it writes to.
const EVIDENTLY: &str = "Evidently";

/// How many entries each listing is seeded with, asked for one at a time: more than one, so
/// a first page always leaves something to resume and therefore issues a token.
const STORE_SIZE: usize = 3;

/// One paginated reader of this service, as the request that asks it for a page of one.
struct Listing {
    /// The operation, named in every assertion.
    operation: &'static str,
    /// Its HTTP method.
    method: &'static str,
    /// Its URI for an app monitor and, where it binds `NextToken` to the query string, a
    /// token.
    uri: fn(&str, Option<&str>) -> String,
    /// Its body, carrying the token where it binds `NextToken` to the body.
    body: fn(Option<&str>) -> Option<Value>,
}

impl Listing {
    /// Ask this listing for a page of one entry, resuming after `token` when the case
    /// carries one.
    async fn page(
        &self,
        router: &MultiServiceRouter,
        monitor: &str,
        token: Option<&str>,
    ) -> Reply {
        let encoded = token.map(percent_encoded);
        call(
            router,
            self.method,
            &(self.uri)(monitor, encoded.as_deref()),
            (self.body)(token),
        )
        .await
    }

    /// The `NextToken` this listing's FIRST page issues over its seeded store.
    async fn issued_token(&self, router: &MultiServiceRouter, monitor: &str) -> String {
        let case = format!("{}: the first page of its own store", self.operation);
        let reply = self.page(router, monitor, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the first page to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(&case);
        page["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a NextToken while {} entries remain, got {page}",
                    STORE_SIZE - 1
                )
            })
            .to_string()
    }
}

/// The body a reader that binds `NextToken` to the query string sends: none.
fn no_body(_token: Option<&str>) -> Option<Value> {
    None
}

/// `&nextToken=<token>` for a reader that binds `NextToken` to the query string.
fn token_query(token: Option<&str>) -> String {
    match token {
        Some(token) => format!("&nextToken={token}"),
        None => String::new(),
    }
}

/// One seeded emulator and the listing over its store.
struct Seeded {
    router: MultiServiceRouter,
    /// The app monitor a monitor-scoped listing addresses; unused by the listing over the
    /// account's collection.
    monitor: String,
    listing: Listing,
}

/// `ListAppMonitors` over an account holding `STORE_SIZE` monitors.
///
/// Each listing gets an emulator of its OWN, because this one reports the account's whole
/// collection: a monitor another listing's store needs would otherwise be part of this one's.
async fn seeded_app_monitors() -> Seeded {
    let (router, _engine) = emulator();
    for index in 0..STORE_SIZE {
        create(
            &router,
            "seed monitors",
            json!({"Name": format!("token-spelling-{index}"), "Domain": "example.com"}),
        )
        .await;
    }
    Seeded {
        router,
        monitor: "token-spelling-0".to_string(),
        listing: Listing {
            operation: "ListAppMonitors",
            method: "POST",
            uri: |_monitor, token| format!("/appmonitors?maxResults=1{}", token_query(token)),
            body: no_body,
        },
    }
}

/// `ListRumMetricsDestinations` over one monitor holding `STORE_SIZE` destinations: the
/// CloudWatch one plus an Evidently one per distinct experiment ARN.
async fn seeded_metrics_destinations() -> Seeded {
    let (router, _engine) = emulator();
    let monitor = "token-spelling-destinations".to_string();
    let case = "seed destinations";
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(&router, case, &monitor, CLOUD_WATCH, None).await;
    for index in 1..STORE_SIZE {
        let arn = experiment_arn(&format!("token-spelling-{index}"));
        put_metrics_destination(&router, case, &monitor, EVIDENTLY, Some(&arn)).await;
    }
    Seeded {
        router,
        monitor,
        listing: Listing {
            operation: "ListRumMetricsDestinations",
            method: "GET",
            uri: |monitor, token| {
                format!(
                    "/rummetrics/{monitor}/metricsdestination?maxResults=1{}",
                    token_query(token)
                )
            },
            body: no_body,
        },
    }
}

/// `BatchGetRumMetricDefinitions` over one CloudWatch destination holding `STORE_SIZE`
/// custom metric definitions.
async fn seeded_metric_definitions() -> Seeded {
    let (router, _engine) = emulator();
    let monitor = "token-spelling-definitions".to_string();
    let case = "seed definitions";
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(&router, case, &monitor, CLOUD_WATCH, None).await;
    let definitions: Vec<Value> = (0..STORE_SIZE)
        .map(|index| json!({"Name": format!("TokenSpelling{index}"), "Namespace": "MyApp/Paging"}))
        .collect();
    let created =
        created_metric_definitions(&router, case, &monitor, CLOUD_WATCH, None, &definitions).await;
    assert_eq!(
        created.len(),
        STORE_SIZE,
        "case {case:?}: expected {STORE_SIZE} definitions to be created, got {created:?}"
    );
    Seeded {
        router,
        monitor,
        listing: Listing {
            operation: "BatchGetRumMetricDefinitions",
            method: "GET",
            uri: |monitor, token| {
                format!(
                    "/rummetrics/{monitor}/metrics?{}&maxResults=1{}",
                    metrics_destination_query(CLOUD_WATCH, None),
                    token_query(token)
                )
            },
            body: no_body,
        },
    }
}

/// `GetAppMonitorData` over one monitor holding more than one collected event: each
/// delivered batch carries two events, so one batch is enough for a page of one to leave
/// something to resume.
async fn seeded_app_monitor_data() -> Seeded {
    let (router, _engine) = emulator();
    let monitor = "token-spelling-data".to_string();
    let case = "seed events";
    let id = create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    let reply = call(
        &router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(&id)),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    Seeded {
        router,
        monitor,
        listing: Listing {
            operation: "GetAppMonitorData",
            method: "POST",
            uri: |monitor, _token| format!("/appmonitor/{monitor}/data"),
            body: |token| {
                let mut body = json!({"TimeRange": {"After": 0}, "MaxResults": 1});
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                Some(body)
            },
        },
    }
}

/// How many of a token's characters bear a case at all — its `a`-`f` hex digits, the only
/// ones a re-spelling can change.
fn case_bearing_digits(token: &str) -> usize {
    token.chars().filter(|c| c.is_ascii_alphabetic()).count()
}

/// `token` with the case-bearing digits `which` selects (by their index among them)
/// upper-cased, and every other character left as issued.
fn upper_cased(token: &str, which: impl Fn(usize) -> bool) -> String {
    let mut seen = 0_usize;
    token
        .chars()
        .map(|digit| {
            if digit.is_ascii_alphabetic() {
                let index = seen;
                seen += 1;
                if which(index) {
                    return digit.to_ascii_uppercase();
                }
            }
            digit
        })
        .collect()
}

/// Every digit of the token upper-cased.
fn all_digits(token: &str) -> String {
    token.to_ascii_uppercase()
}

/// Only the token's first case-bearing digit upper-cased.
fn first_digit(token: &str) -> String {
    upper_cased(token, |index| index == 0)
}

/// Only the token's last case-bearing digit upper-cased.
fn last_digit(token: &str) -> String {
    let last = case_bearing_digits(token).saturating_sub(1);
    upper_cased(token, |index| index == last)
}

/// Every other case-bearing digit upper-cased, so the token carries both spellings at once.
fn alternating_digits(token: &str) -> String {
    upper_cased(token, |index| index % 2 == 0)
}

/// One way to re-spell an issued token without changing the bytes it decodes to.
struct Respelling {
    label: &'static str,
    spell: fn(&str) -> String,
}

/// The re-spellings every listing's issued token is driven through.
fn respellings() -> [Respelling; 4] {
    [
        Respelling {
            label: "every digit upper-cased",
            spell: all_digits,
        },
        Respelling {
            label: "the first case-bearing digit upper-cased",
            spell: first_digit,
        },
        Respelling {
            label: "the last case-bearing digit upper-cased",
            spell: last_digit,
        },
        Respelling {
            label: "alternating case-bearing digits upper-cased",
            spell: alternating_digits,
        },
    ]
}

/// A value that decodes to the bytes of a token this listing issued, but is not the value
/// it issued, is refused as a token this service never issued — for every paginated reader
/// of this service.
#[tokio::test]
async fn a_respelled_token_is_not_a_token_this_service_issued() {
    for seeded in [
        seeded_app_monitors().await,
        seeded_metrics_destinations().await,
        seeded_metric_definitions().await,
        seeded_app_monitor_data().await,
    ] {
        let Seeded {
            router,
            monitor,
            listing,
        } = seeded;
        let issued = listing.issued_token(&router, &monitor).await;

        // The control row: the token in the spelling it was issued in resumes the listing,
        // so the refusals below are about the spelling and not about the token.
        let control = format!("{}: the issued spelling {issued}", listing.operation);
        let resumed = listing.page(&router, &monitor, Some(&issued)).await;
        assert_eq!(
            resumed.status,
            200,
            "case {control:?}: expected the issued token to resume the listing, got {} {:?}",
            resumed.status,
            String::from_utf8_lossy(&resumed.body)
        );

        for respelling in respellings() {
            let forged = (respelling.spell)(&issued);
            let case = format!(
                "{}: {} ({forged} for the issued {issued})",
                listing.operation, respelling.label
            );
            // The row is only a test of the rule if it really is a different spelling of
            // the same bytes.
            assert_ne!(
                forged, issued,
                "case {case:?}: the row must differ from the issued token, got {forged:?}"
            );
            assert_eq!(
                hex::decode(&forged).ok(),
                hex::decode(&issued).ok(),
                "case {case:?}: the row must decode to the bytes the issued token decodes to"
            );

            let refused = listing.page(&router, &monitor, Some(&forged)).await;
            assert_eq!(
                refused.status,
                400,
                "case {case:?}: expected HTTP 400 for a token this service never issued, got \
                 {} {:?}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            let code = refused.error_code(&case);
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected error code ValidationException, got {code:?}"
            );
            let body = refused.json(&case);
            let stated = message(&body).unwrap_or_else(|| {
                panic!("case {case:?}: expected the refusal to state a message, got {body}")
            });
            assert!(
                stated.contains(&forged),
                "case {case:?}: expected the refusal to name the token {forged:?}, got {stated:?}"
            );
        }
    }
}
