// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TERMINAL states of an ApiGatewayV2 portal and for the
//! `Preview` every portal reader reports.
//!
//! The rules these cases assert:
//!
//! * **A portal reader always reports the `Preview` member, and reports it EMPTY until a
//!   preview has been built**: `preview` is `{}` for a portal nothing has previewed, so a
//!   client that reads it to decide whether a preview exists is handed a structure rather
//!   than nothing at all, and it carries the preview's status and URL once `PreviewPortal`
//!   has built one.
//! * **A call that takes a resource's publication down is accepted only from the statuses
//!   the service names for that transition**, and refuses every other with 409
//!   `ConflictException` whose message names those statuses — so a caller is told the
//!   take-down was illegal rather than left with a resource in a state the service would
//!   never reach. For a portal those statuses are
//!   `state::PORTAL_DISABLE_FROM_STATUSES`, and a refused `DisablePortal` leaves the
//!   portal exactly as it was.
//! * **A delete states the state the caller wants, so it is IDEMPOTENT**: `DeletePortal`
//!   answers its modelled 204 with no body however many times it is called and whether or
//!   not the account still holds the portal, and the portal stays READABLE afterwards in
//!   its terminal `DELETED` status.
//!
//! The rows vary what the rules generalise over: the build state the portal is in when it
//! is read (nothing built, published, published and taken down, a preview in flight, a
//! preview ready), WHICH reader reports it (`GetPortal`, `UpdatePortal`, the `ListPortals`
//! item), the `PublishStatus` a disable is attempted from (every one this service reaches,
//! plus a portal that is gone), and how many deletes one portal is sent (one, two, three).

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::PUBLISH_STATUS_VALUES;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    PORTAL_BUILD, PORTAL_DISABLE_FROM_STATUSES, PORTAL_DISABLE_PRECONDITION_MESSAGE,
    PORTAL_NOT_FOUND_MESSAGE, PREVIEW_STATUS_PREVIEW_IN_PROGRESS, PREVIEW_STATUS_PREVIEW_READY,
    PUBLISH_STATUS_DELETED, PUBLISH_STATUS_DISABLED, PUBLISH_STATUS_DISABLE_IN_PROGRESS,
    PUBLISH_STATUS_PUBLISHED, PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
};

use crate::api_lifecycle::{as_str, call, call_reading_headers, emulator, error_code};
use crate::portal_lifecycle::portal_content;

/// The portals collection URI (`CreatePortal`'s modelled `POST /v2/portals`).
const PORTALS: &str = "/v2/portals";

/// The sentence `DisablePortal` refuses a portal that is in no status its publication can
/// be taken down from with, spelled out here rather than taken from the crate's constant: a
/// test that asserts a handler's own constant back at it cannot catch the wording drifting
/// away from AWS's.
const DISABLE_PRECONDITION_SENTENCE: &str =
    "Portal must be PUBLISHED, PUBLISH_FAILED, or DISABLE_FAILED to disable";

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack. A BLOCKING wait on purpose — this workspace's `tokio` carries no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// Create one portal and answer its `PortalId`.
async fn create_portal(emulator: &MultiServiceRouter, case: &str, display_name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        PORTALS,
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content(display_name)
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
    );
    as_str(&created, "portalId")
}

/// Send one call expected to be accepted with `status`, and answer its body.
async fn accepted(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    status: u16,
) -> Value {
    let (answered, answer) = call(emulator, method, uri, body).await;
    assert_eq!(
        answered, status,
        "case {case:?}: expected {status} from {method} {uri}, got {answered} {answer}"
    );
    answer
}

/// What a portal had built on it before the read under test.
#[derive(Debug, Clone, Copy)]
enum Built {
    /// Created and left alone.
    Nothing,
    /// `PublishPortal`, settled.
    Published,
    /// Published, then `DisablePortal`, both settled.
    Disabled,
    /// `PreviewPortal` called, its build still in flight.
    PreviewInFlight,
    /// `PreviewPortal` called and its build settled.
    PreviewReady,
}

impl Built {
    /// Drive the portal into this state.
    async fn reach(self, emulator: &MultiServiceRouter, case: &str, portal_id: &str) {
        let publish = format!("{PORTALS}/{portal_id}/publish");
        let preview = format!("{PORTALS}/{portal_id}/preview");
        match self {
            Built::Nothing => {}
            Built::Published => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
            }
            Built::Disabled => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
                accepted(emulator, case, "DELETE", &publish, Option::None, 204).await;
                sleep(past_the_build());
            }
            Built::PreviewInFlight => {
                accepted(emulator, case, "POST", &preview, Option::None, 202).await;
            }
            Built::PreviewReady => {
                accepted(emulator, case, "POST", &preview, Option::None, 202).await;
                sleep(past_the_build());
            }
        }
    }

    /// The `PreviewStatus` a portal in this state reports, or `None` when no preview has
    /// been built on it — in which case the reported `Preview` carries nothing at all.
    fn preview_status(self) -> Option<&'static str> {
        match self {
            Built::Nothing | Built::Published | Built::Disabled => Option::None,
            Built::PreviewInFlight => Some(PREVIEW_STATUS_PREVIEW_IN_PROGRESS),
            Built::PreviewReady => Some(PREVIEW_STATUS_PREVIEW_READY),
        }
    }
}

/// Every build state a reader is asserted over.
const EVERY_BUILT: [Built; 5] = [
    Built::Nothing,
    Built::Published,
    Built::Disabled,
    Built::PreviewInFlight,
    Built::PreviewReady,
];

/// The `Preview` the portal `portal_id` reports, as each reader of a portal reports it:
/// `GetPortal`, the `UpdatePortal` that applies nothing, and the `ListPortals` item.
async fn previews_reported(
    emulator: &MultiServiceRouter,
    case: &str,
    portal_id: &str,
) -> Vec<(&'static str, Value)> {
    let uri = format!("{PORTALS}/{portal_id}");
    let read = accepted(emulator, case, "GET", &uri, Option::None, 200).await;
    // A PATCH carrying no member applies nothing, so what it answers is the portal as it
    // stands — the reader, with no write mixed into it.
    let patched = accepted(emulator, case, "PATCH", &uri, Some(json!({})), 200).await;
    let listed = accepted(emulator, case, "GET", PORTALS, Option::None, 200).await;
    let item = listed
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| as_str(item, "portalId") == portal_id)
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListPortals must report the portal: {listed}"));
    vec![
        ("GetPortal", read.get("preview").cloned().unwrap_or(Value::Null)),
        (
            "UpdatePortal",
            patched.get("preview").cloned().unwrap_or(Value::Null),
        ),
        (
            "ListPortals",
            item.get("preview").cloned().unwrap_or(Value::Null),
        ),
    ]
}

#[tokio::test]
async fn every_portal_reader_reports_the_preview_member_and_reports_it_empty_until_one_is_built() {
    for built in EVERY_BUILT {
        let case = format!("{built:?}");
        let case = case.as_str();
        // A fresh emulator per row, so `ListPortals` reports exactly this row's portal.
        let emulator = emulator();
        let portal_id = create_portal(&emulator, case, "Previewed Portal").await;
        built.reach(&emulator, case, &portal_id).await;

        for (reader, preview) in previews_reported(&emulator, case, &portal_id).await {
            match built.preview_status() {
                // Nothing built a preview, so the member is there and carries nothing.
                Option::None => assert_eq!(
                    preview,
                    json!({}),
                    "case {case:?} ({reader}): expected the empty preview on a portal nothing \
                     previewed, got {preview}"
                ),
                Some(status) => {
                    assert_eq!(
                        preview.get("previewStatus").and_then(Value::as_str),
                        Some(status),
                        "case {case:?} ({reader}): expected previewStatus {status}, got \
                         {preview}"
                    );
                    assert!(
                        preview
                            .get("previewUrl")
                            .and_then(Value::as_str)
                            .is_some_and(|url| url.ends_with("/preview")),
                        "case {case:?} ({reader}): expected the URL the preview is served \
                         on, got {preview}"
                    );
                }
            }
        }
    }
}

/// The `PublishStatus` a disable is attempted from, and how the portal reaches it.
#[derive(Debug, Clone, Copy)]
enum DisableFrom {
    /// A created portal: nothing has published it, so it publishes nothing.
    NeverPublished,
    /// `PublishPortal` called, its build still in flight.
    PublishInFlight,
    /// Published, settled: the one status this service reaches that a publication can be
    /// taken down from.
    Published,
    /// A take-down still in flight.
    DisableInFlight,
    /// A take-down that has settled.
    DisableSettled,
    /// A portal `DeletePortal` has taken away.
    Deleted,
}

impl DisableFrom {
    /// Drive the portal into this state, answering the `PublishStatus` it then reports.
    async fn reach(
        self,
        emulator: &MultiServiceRouter,
        case: &str,
        portal_id: &str,
    ) -> &'static str {
        let publish = format!("{PORTALS}/{portal_id}/publish");
        match self {
            DisableFrom::NeverPublished => PUBLISH_STATUS_DISABLED,
            DisableFrom::PublishInFlight => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                PUBLISH_STATUS_PUBLISH_IN_PROGRESS
            }
            DisableFrom::Published => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
                PUBLISH_STATUS_PUBLISHED
            }
            DisableFrom::DisableInFlight => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
                accepted(emulator, case, "DELETE", &publish, Option::None, 204).await;
                PUBLISH_STATUS_DISABLE_IN_PROGRESS
            }
            DisableFrom::DisableSettled => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
                accepted(emulator, case, "DELETE", &publish, Option::None, 204).await;
                sleep(past_the_build());
                PUBLISH_STATUS_DISABLED
            }
            DisableFrom::Deleted => {
                accepted(
                    emulator,
                    case,
                    "DELETE",
                    &format!("{PORTALS}/{portal_id}"),
                    Option::None,
                    204,
                )
                .await;
                PUBLISH_STATUS_DELETED
            }
        }
    }
}

#[tokio::test]
async fn disable_portal_is_accepted_only_from_a_status_its_publication_can_be_taken_down_from() {
    // The sentence the handler answers with is built from the service's own constant, so
    // it is asserted to BE the literal this suite expects and cannot drift from it; and
    // the statuses it names are the catalog the handler accepts, every one of them a
    // `PublishStatus` the model declares.
    assert_eq!(
        PORTAL_DISABLE_PRECONDITION_MESSAGE, DISABLE_PRECONDITION_SENTENCE,
        "expected the refusal sentence to be {DISABLE_PRECONDITION_SENTENCE:?}, got \
         {PORTAL_DISABLE_PRECONDITION_MESSAGE:?}"
    );
    for status in PORTAL_DISABLE_FROM_STATUSES {
        assert!(
            PORTAL_DISABLE_PRECONDITION_MESSAGE.contains(status),
            "case {status:?}: expected the refusal to name every status a publication can \
             be taken down from, got {PORTAL_DISABLE_PRECONDITION_MESSAGE:?}"
        );
        assert!(
            PUBLISH_STATUS_VALUES.contains(status),
            "case {status:?}: expected a PublishStatus the model declares, out of \
             {PUBLISH_STATUS_VALUES:?}"
        );
    }

    for from in [
        DisableFrom::NeverPublished,
        DisableFrom::PublishInFlight,
        DisableFrom::Published,
        DisableFrom::DisableInFlight,
        DisableFrom::DisableSettled,
        DisableFrom::Deleted,
    ] {
        let case = format!("a disable from {from:?}");
        let case = case.as_str();
        let emulator = emulator();
        let portal_id = create_portal(&emulator, case, "Disabled Portal").await;
        let reported = from.reach(&emulator, case, &portal_id).await;
        let uri = format!("{PORTALS}/{portal_id}");

        // The status the portal reports is the one the row says it is: the refusal below
        // is decided by it, so a row that did not reach it would assert nothing.
        let before = accepted(&emulator, case, "GET", &uri, Option::None, 200).await;
        assert_eq!(
            as_str(&before, "publishStatus"),
            reported,
            "case {case:?}: expected the portal to report {reported}, got {before}"
        );

        let (status, headers, answer) =
            call_reading_headers(&emulator, "DELETE", &format!("{uri}/publish"), None).await;
        if PORTAL_DISABLE_FROM_STATUSES.contains(&reported) {
            assert_eq!(
                status, 204,
                "case {case:?}: expected the modelled 204 from a legal disable, got {status} \
                 {answer}"
            );
            let during = accepted(&emulator, case, "GET", &uri, Option::None, 200).await;
            assert_eq!(
                as_str(&during, "publishStatus"),
                PUBLISH_STATUS_DISABLE_IN_PROGRESS,
                "case {case:?}: expected the accepted disable to start the take-down, got \
                 {during}"
            );
            continue;
        }

        // A portal that is GONE is not refused as a conflict: there is no resource to
        // conflict with, which is the 404 every writer answers for an id the account holds
        // no portal under.
        if matches!(from, DisableFrom::Deleted) {
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 on a portal the account no longer holds, got \
                 {status} {answer}"
            );
            assert_eq!(
                as_str(&answer, "message"),
                PORTAL_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the sentence this API answers a missing portal \
                 with, got {answer}"
            );
            continue;
        }

        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 from a disable of a portal in {reported}, got \
             {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {answer}"
        );
        assert_eq!(
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok()),
            Some("ConflictException"),
            "case {case:?}: expected the error type on the response headers, got {headers:?}"
        );
        assert_eq!(
            as_str(&answer, "message"),
            DISABLE_PRECONDITION_SENTENCE,
            "case {case:?}: expected the sentence AWS refuses an illegal disable with, got \
             {answer}"
        );

        // The refusal wrote nothing: the portal is in the state it was in, and the next
        // read reports the same status.
        let after = accepted(&emulator, case, "GET", &uri, Option::None, 200).await;
        assert_eq!(
            as_str(&after, "publishStatus"),
            reported,
            "case {case:?}: expected the refused disable to leave the portal in {reported}, \
             got {after}"
        );
    }
}

#[tokio::test]
async fn delete_portal_answers_the_same_204_however_many_times_it_is_called() {
    // The rows vary the state the first delete meets and how many deletes are sent: the
    // answer is the same 204 for all of them.
    for (built, deletes) in [
        (Built::Nothing, 1_usize),
        (Built::Published, 2),
        (Built::Disabled, 3),
        (Built::PreviewReady, 2),
    ] {
        let case = format!("{deletes} delete(s) of a portal in {built:?}");
        let case = case.as_str();
        let emulator = emulator();
        let portal_id = create_portal(&emulator, case, "Deleted Portal").await;
        built.reach(&emulator, case, &portal_id).await;
        let uri = format!("{PORTALS}/{portal_id}");

        for attempt in 1..=deletes {
            let (status, answer) = call(&emulator, "DELETE", &uri, None).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected 204 from delete {attempt}, got {status} {answer}"
            );
            assert!(
                answer.is_null() || answer.as_object().is_some_and(serde_json::Map::is_empty),
                "case {case:?}: expected delete {attempt} to report no members, got {answer}"
            );

            // And after each one the portal reads back in its terminal status, with the
            // body it had.
            let read = accepted(&emulator, case, "GET", &uri, Option::None, 200).await;
            assert_eq!(
                as_str(&read, "publishStatus"),
                PUBLISH_STATUS_DELETED,
                "case {case:?}: expected {PUBLISH_STATUS_DELETED} after delete {attempt}, \
                 got {read}"
            );
            assert_eq!(
                as_str(&read, "portalId"),
                portal_id,
                "case {case:?}: expected the deleted portal's own record after delete \
                 {attempt}, got {read}"
            );
            assert!(
                read.get("portalArn").and_then(Value::as_str).is_some(),
                "case {case:?}: expected the deleted portal to keep the body it had, got \
                 {read}"
            );
        }

        // It is gone from the collection, whatever it reads back as.
        let listed = accepted(&emulator, case, "GET", PORTALS, Option::None, 200).await;
        let ids: Vec<String> = listed
            .get("items")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(|item| as_str(item, "portalId")).collect())
            .unwrap_or_default();
        assert!(
            !ids.contains(&portal_id),
            "case {case:?}: expected ListPortals not to list the deleted portal, got {ids:?}"
        );
    }
}
