// SPDX-License-Identifier: CC0-1.0

//! One resume point has exactly ONE cursor, driven end to end through both listings.
//!
//! A `nextToken` of this service is the cursor prefix followed by the resume ARN's bytes
//! in lower-case hex, and that single spelling is the whole cursor format: a token is
//! read as a resume point only when it is byte-for-byte the string the service would mint
//! for the ARN it carries. The hex codec underneath reads upper- and mixed-case digits as
//! readily as the lower case it writes, so a re-cased body decodes to the very same ARN
//! while being a token this service could not have issued — and a token this service did
//! not mint is refused with `ValidationException` (400), not served as a page.
//!
//! The table below is that rule as an invariant: for several resume ARNs (one cursor the
//! service itself minted, one naming a stored record, one past every stored record) and
//! several respellings of the hex body (all digits, the first, every second, the last
//! byte's), the call is accepted exactly when the respelling leaves the minted string
//! unchanged — and every row first asserts that the respelling decodes to the same bytes,
//! so what is under test is the spelling and nothing else. It runs through
//! `ListDashboards` AND `ListScheduledReports`, the two operations that share the codec.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::{
    encode_page_token, DASHBOARD_RESOURCE_TYPE, SCHEDULED_REPORT_RESOURCE_TYPE,
};

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The prefix every cursor of this service carries, spelled here so a test can take a
/// token apart into the prefix and the hex body whose spelling is under test.
const CURSOR_PREFIX: &str = "v1:";

/// The account field of an ARN written by hand here, which is also the account this
/// emulator mints every ARN under (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// One listing under test: its operation, the output member its page lands in, and the
/// ARN resource type its cursors name.
#[derive(Debug)]
struct Listing {
    operation: &'static str,
    items_key: &'static str,
    arn_resource_type: &'static str,
}

/// Both listings sharing the cursor codec.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListDashboards",
            items_key: "dashboards",
            arn_resource_type: DASHBOARD_RESOURCE_TYPE,
        },
        Listing {
            operation: "ListScheduledReports",
            items_key: "scheduledReports",
            arn_resource_type: SCHEDULED_REPORT_RESOURCE_TYPE,
        },
    ]
}

/// A respelling of a cursor's hex body: a transform that leaves the bytes it encodes
/// alone and changes only how they are written.
struct Spelling {
    label: &'static str,
    respell: fn(&str) -> String,
}

/// Upper-case the hex digit at `at`, leaving the rest of the body as written.
fn upper_at(body: &str, at: usize) -> String {
    body.char_indices()
        .map(|(index, digit)| {
            if index == at {
                digit.to_ascii_uppercase()
            } else {
                digit
            }
        })
        .collect()
}

/// Every spelling of a hex body this suite drives. Hex has one degree of spelling
/// freedom — the case of its `a`-`f` digits — so the rows differ in HOW MUCH of the body
/// is re-cased: nothing (the minted spelling), all of it, its first digit, every second
/// digit, and the two digits of its last byte.
fn spellings() -> Vec<Spelling> {
    vec![
        Spelling {
            label: "the minted spelling, every hex digit lower case",
            respell: str::to_string,
        },
        Spelling {
            label: "every hex digit upper cased",
            respell: str::to_ascii_uppercase,
        },
        Spelling {
            label: "the first hex digit upper cased",
            respell: |body| upper_at(body, 0),
        },
        Spelling {
            label: "every second hex digit upper cased",
            respell: |body| {
                body.char_indices()
                    .map(|(index, digit)| {
                        if index % 2 == 0 {
                            digit.to_ascii_uppercase()
                        } else {
                            digit
                        }
                    })
                    .collect()
            },
        },
        Spelling {
            label: "the two digits of the last byte upper cased",
            respell: |body| {
                let tail = body.len().saturating_sub(2);
                body.char_indices()
                    .map(|(index, digit)| {
                        if index >= tail {
                            digit.to_ascii_uppercase()
                        } else {
                            digit
                        }
                    })
                    .collect()
            },
        },
    ]
}

/// The ARNs a page carries, in the order it carries them.
fn page_arns(page: &Value, listing: &Listing, case: &str) -> Vec<String> {
    page[listing.items_key]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no {} in {page}", listing.items_key))
        .iter()
        .map(|item| {
            item["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: no arn in {item}"))
                .to_string()
        })
        .collect()
}

/// Store a dashboard and, on it, three more dashboards and three scheduled reports, so
/// every listing has several records to resume among. Answers the source dashboard's ARN.
async fn stored_fixtures(router: &MultiServiceRouter) -> String {
    let (dashboard_arn, _) = source_dashboard(router, "Cursor Spelling Source").await;
    for index in 1..=3 {
        ok_call(
            router,
            "CreateDashboard",
            &json!({"name": format!("Cursor Spelling Board {index:02}"), "widgets": []}),
        )
        .await;
        ok_call(
            router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": format!("Cursor Spelling Report {index:02}"),
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                }
            }),
        )
        .await;
    }
    dashboard_arn
}

/// A cursor is read as a resume point only in the one spelling the service mints; the
/// same bytes written with any upper-case hex digit are refused with
/// `ValidationException` (400), through both listings.
#[tokio::test]
async fn only_the_minted_spelling_of_a_cursor_resumes_a_listing() {
    let router = router();
    stored_fixtures(&router).await;

    for listing in listings() {
        let case = listing.operation;

        let every = ok_call(&router, case, &json!({"maxResults": 100})).await;
        let stored = page_arns(&every, &listing, case);
        assert!(
            stored.len() >= 2,
            "case {case}: expected at least two stored records to resume among, got {every}"
        );

        // The cursor the service itself minted for a page of one: its body is the
        // spelling every row below is measured against.
        let first_page = ok_call(&router, case, &json!({"maxResults": 1})).await;
        let minted = first_page
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case}: records remain after a page of one, so a cursor is owed: {first_page}")
            })
            .to_string();
        assert_eq!(
            minted,
            encode_page_token(&stored[1]),
            "case {case}: the minted cursor names the record after the page"
        );

        // An id of only the characters the resource segment admits that sorts after every
        // id this service mints, so a cursor naming it is a resume point past the last
        // stored record. Source for the segment's grammar: the `DashboardArn` and
        // `ScheduledReportArn` patterns,
        // `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:<resource-type>/(\*|[-a-z0-9]+)$`
        // (`smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`).
        let past_every_record = format!(
            "arn:aws:bcm-dashboards::{ACCOUNT}:{}/zzzzzzzz",
            listing.arn_resource_type
        );

        let resume_points: Vec<(&str, String)> = vec![
            ("the cursor this listing minted", minted),
            (
                "a cursor naming the last stored record",
                encode_page_token(stored.last().expect("a stored record")),
            ),
            (
                "a cursor naming an ARN past every stored record",
                encode_page_token(&past_every_record),
            ),
        ];

        for (point, canonical) in &resume_points {
            let body = canonical
                .strip_prefix(CURSOR_PREFIX)
                .unwrap_or_else(|| panic!("case {case} {point}: {canonical:?} carries the prefix"));

            for spelling in spellings() {
                let label = spelling.label;
                let respelled = (spelling.respell)(body);
                // What differs is the spelling only: the respelled body encodes exactly
                // the bytes the minted one does.
                assert_eq!(
                    hex::decode(&respelled).ok(),
                    hex::decode(body).ok(),
                    "case {case} {point} / {label}: the respelling must encode the same bytes, \
                     got {respelled:?} against {body:?}"
                );
                let token = format!("{CURSOR_PREFIX}{respelled}");
                let minted_spelling = token == *canonical;
                let (status, answer) =
                    call(&router, case, &json!({"maxResults": 1, "nextToken": token})).await;
                if minted_spelling {
                    assert_eq!(
                        status, 200,
                        "case {case} {point} / {label}: {token:?} is the spelling this service \
                         mints, so it resumes the walk, got {status}: {answer}"
                    );
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case} {point} / {label}: {token:?} is no cursor this service \
                         could have minted, so it must be refused, got {status}: {answer}"
                    );
                    assert!(
                        error_code(&answer).contains("ValidationException"),
                        "case {case} {point} / {label}: expected ValidationException, got {}",
                        error_code(&answer)
                    );
                    let message = answer["message"].as_str().unwrap_or("<no message>");
                    let expected = format!(
                        "Member must be a nextToken returned by a previous {case} call: nextToken"
                    );
                    assert_eq!(
                        message, expected,
                        "case {case} {point} / {label}: expected {expected:?}, got {message:?}"
                    );
                }
            }
        }

        // The prefix is as fixed as the body: a cursor whose prefix is written in another
        // case, without its separator or naming another version is no token this service
        // minted either.
        let canonical = encode_page_token(&stored[1]);
        let body = canonical
            .strip_prefix(CURSOR_PREFIX)
            .expect("a minted cursor carries the prefix");
        for prefix in ["V1:", "v1", "v2:"] {
            let token = format!("{prefix}{body}");
            let (status, answer) =
                call(&router, case, &json!({"maxResults": 1, "nextToken": token})).await;
            assert_eq!(
                status, 400,
                "case {case} prefix {prefix:?}: {token:?} is no cursor this service could have \
                 minted, so it must be refused, got {status}: {answer}"
            );
            assert!(
                error_code(&answer).contains("ValidationException"),
                "case {case} prefix {prefix:?}: expected ValidationException, got {}",
                error_code(&answer)
            );
        }

        // And the minted spelling is still a RESUME POINT rather than a page of its own,
        // so the refusals above rejected the spelling and not the cursor format: the
        // cursor naming the second stored record answers exactly that record.
        let resumed = ok_call(
            &router,
            case,
            &json!({"maxResults": 1, "nextToken": canonical}),
        )
        .await;
        assert_eq!(
            page_arns(&resumed, &listing, case),
            vec![stored[1].clone()],
            "case {case}: the minted cursor resumes at {:?}: {resumed}",
            stored[1]
        );
    }
}
