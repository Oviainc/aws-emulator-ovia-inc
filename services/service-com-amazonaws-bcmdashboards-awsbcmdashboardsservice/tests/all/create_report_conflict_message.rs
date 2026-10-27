// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `ConflictException` a `CreateScheduledReport` `clientToken`
//! conflict is refused with, end to end over the wire.
//!
//! The rule under test: the refusal a duplicate idempotency token is answered with is
//! worded from the state of the create the token already started, and that wording is
//! fixed text a client matches on — the creation-in-flight branch carries NO variable
//! part at all, and the already-created branch's only variable part is the stored
//! resource's ARN. The caller's token is a value the client chose as its idempotency key
//! and never appears in either message, so neither branch's text varies with the
//! request.
//!
//! The table spans what the rule generalises over: the token itself. Its rows walk
//! `ClientToken`'s constraints (length 1..128, pattern `^[\u0021-\u007E]+$`, source:
//! `smithy_query(query="shape",shape="ClientToken")`) at both published length bounds,
//! over the punctuation the pattern admits, over a token shaped like an ARN (so a
//! message echoing the token instead of the stored ARN is caught) and over a token
//! spelled like the fixed wording itself (so a message interpolating the token is caught
//! even when it reads plausibly). Every row is asserted against the two texts
//! character for character.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::create_report_client_token::wait_out_the_creation_window;
use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The whole message a duplicate token racing the create it started is refused with.
/// Fixed text: there is no variable part, and the "retry after some time" instruction is
/// what tells a client this conflict is the transient one.
const IN_PROGRESS_MESSAGE: &str = "A scheduled report creation is already in progress \
     for the provided clientToken. Please retry after some time.";

/// The message a token whose create has settled is refused with: fixed text around the
/// one value a caller can act on, the ARN of the report the token already created.
fn already_exists_message(arn: &str) -> String {
    format!(
        "A scheduled report with the provided clientToken already exists (ARN: {arn}). \
         Use a different clientToken to create a new scheduled report, or use the \
         existing report's ARN to update it."
    )
}

/// One row: a `clientToken` a client may legitimately choose.
struct TokenCase {
    label: &'static str,
    token: String,
}

fn cases() -> Vec<TokenCase> {
    vec![
        TokenCase {
            label: "the token an SDK generates",
            token: "sdk-generated-token-7b31".to_string(),
        },
        TokenCase {
            // `ClientToken`'s shortest admitted value, and `!` is the first code point
            // its pattern admits (\u0021).
            label: "one character, the shortest admitted token",
            token: "!".to_string(),
        },
        TokenCase {
            // The published upper length bound, 128.
            label: "128 characters, the longest admitted token",
            token: "t".repeat(128),
        },
        TokenCase {
            // The printable range the pattern admits, excluding nothing but the space
            // it stops short of.
            label: "punctuation across the admitted printable range",
            token: "~!#$%&'()*+,-./:;<=>?@[\\]^_`{|}\"".to_string(),
        },
        TokenCase {
            label: "a token shaped like a scheduled-report ARN",
            token: "arn:aws:bcm-dashboards::123456789012:scheduled-report/not-a-report".to_string(),
        },
        TokenCase {
            label: "a token spelled like the refusal's own wording",
            token: "the-provided-clientToken".to_string(),
        },
    ]
}

/// The create request every row presents, under its own token.
fn create_request(token: &str, dashboard_arn: &str, widget_id: &str, name: &str) -> Value {
    json!({
        "clientToken": token,
        "scheduledReport": {
            "name": name,
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 9 1 * ? *)"},
            "widgetIds": [widget_id]
        }
    })
}

/// A row's own store, the create it served and the request that created it.
struct Served {
    router: MultiServiceRouter,
    request: Value,
    arn: String,
    dashboard_arn: String,
    widget_id: String,
}

/// The conflict body of a refused create: asserts the code and status, and hands back the
/// message.
fn conflict_message(case: &str, branch: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 409,
        "case {case:?} ({branch}): a duplicate clientToken is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(body).ends_with("ConflictException"),
        "case {case:?} ({branch}): expected ConflictException, got {body}"
    );
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} ({branch}): the refusal carries a message: {body}"))
        .to_string()
}

/// Both conflict branches, over the table: the in-flight refusal is the fixed text with
/// nothing of the request in it (identical for every row), and the settled refusal is the
/// fixed text carrying only the ARN the token's create minted.
#[tokio::test]
async fn a_duplicate_client_token_is_refused_in_aws_s_own_words() {
    let cases = cases();

    // The racing branch first, row by row: the duplicate has to reach the handler while
    // its create is still in flight, so each row creates and immediately duplicates.
    let mut served: Vec<Served> = Vec::new();
    let mut racing_messages: Vec<String> = Vec::new();
    for case in &cases {
        let router = router();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "ConflictMessageSource").await;
        let request = create_request(&case.token, &dashboard_arn, &widget_id, "Conflict Rollup");
        let created = ok_call(&router, "CreateScheduledReport", &request).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create returns an arn: {created}", case.label))
            .to_string();

        let (status, body) = call(&router, "CreateScheduledReport", &request).await;
        let message = conflict_message(case.label, "racing the create", status, &body);
        assert_eq!(
            message, IN_PROGRESS_MESSAGE,
            "case {:?} (racing the create): expected the fixed in-progress text {IN_PROGRESS_MESSAGE:?}, got {message:?}",
            case.label
        );
        assert!(
            !message.contains(&case.token),
            "case {:?} (racing the create): the presented clientToken must not appear in the refusal, got {message:?}",
            case.label
        );
        racing_messages.push(message);
        served.push(Served {
            router,
            request,
            arn,
            dashboard_arn,
            widget_id,
        });
    }

    // Fixed text means fixed: every row's in-flight refusal is the same bytes.
    for (case, message) in cases.iter().zip(&racing_messages) {
        assert_eq!(
            message, &racing_messages[0],
            "case {:?} (racing the create): the in-progress message has no variable part, so every row's must match {:?}, got {message:?}",
            case.label, racing_messages[0]
        );
    }

    // Past the creation window the same token is no longer racing anything: a request it
    // would have created a different report for is refused as already created.
    wait_out_the_creation_window().await;
    for (case, served) in cases.iter().zip(&served) {
        let different = create_request(
            &case.token,
            &served.dashboard_arn,
            &served.widget_id,
            "Another Rollup",
        );
        assert_ne!(
            different, served.request,
            "case {:?} (settled): a different-request row must really differ",
            case.label
        );
        let (status, body) = call(&served.router, "CreateScheduledReport", &different).await;
        let message = conflict_message(case.label, "settled", status, &body);
        let expected = already_exists_message(&served.arn);
        assert_eq!(
            message, expected,
            "case {:?} (settled): expected {expected:?}, got {message:?}",
            case.label
        );
        assert!(
            !message.contains(&case.token),
            "case {:?} (settled): the presented clientToken must not appear in the refusal, got {message:?}",
            case.label
        );
    }
}
