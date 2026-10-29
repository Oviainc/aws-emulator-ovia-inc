// SPDX-License-Identifier: CC0-1.0

//! The ARN a listing cursor carries is read through the SHARED ARN reader, driven
//! through both operations that reach it.
//!
//! Splitting a value into the six fields of an Amazon Resource Name, and splitting its
//! resource field into a type and an id, is one parse that `service_common::arn` owns for
//! the whole tree; what each field has to HOLD is this service's own modelled pattern
//! (`^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:<resource-type>/(\*|[-a-z0-9]+)$`,
//! length 20..2048, source
//! `smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`). Both parts
//! meet in one place — the cursor decoder every listing of this service pages through —
//! so this suite drives one table through `ListDashboards` AND `ListScheduledReports`,
//! the two operations that reach it, and pins the answer each cursor form gets.
//!
//! The rows differ along what the SPLIT decides rather than along the characters of one
//! field: how many fields the value carries, whether the partition and the vendor fields
//! are present at all, whether the resource field is separated from its id by `/` (which
//! these patterns spell) or by `:` (which an ARN may carry elsewhere and these patterns
//! do not), and the length bound the two ARN shapes publish, at the bound and one past
//! it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::{
    encode_page_token, DASHBOARD_RESOURCE_TYPE, SCHEDULED_REPORT_RESOURCE_TYPE,
};

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The longest ARN either of this service's resource shapes admits: `DashboardArn` and
/// `ScheduledReportArn` both declare `@length` 20..2048
/// (`smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`).
const RESOURCE_ARN_MAX_LENGTH: usize = 2048;

/// The twelve-digit account field of an ARN built by hand here, which is also the
/// account this emulator mints every ARN under (`engine::mock_account_id`), so a cursor
/// written here sorts among the stored records rather than beside them.
const ACCOUNT: &str = "000000000000";

/// One listing under test: its operation, the output member its page lands in, and the
/// ARN resource type its cursors name.
#[derive(Debug)]
struct Listing {
    operation: &'static str,
    items_key: &'static str,
    arn_resource_type: &'static str,
}

/// Both operations whose page cursor is decoded through the shared reader.
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

/// Store two dashboards and two scheduled reports, so every listing has records a cursor
/// can resume at. Answers the ARN of the dashboard the reports render.
async fn stored_fixtures(router: &MultiServiceRouter) -> String {
    let (dashboard_arn, _) = source_dashboard(router, "Shared Arn Reader Source").await;
    ok_call(
        router,
        "CreateDashboard",
        &json!({"name": "Shared Arn Reader Second", "widgets": []}),
    )
    .await;
    for index in 1..=2 {
        ok_call(
            router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": format!("Shared Arn Report {index:02}"),
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

/// One row of the table: how the value under test is written, and whether the cursor
/// carrying it names an ARN of the listing's resource.
struct Row {
    label: &'static str,
    arn: String,
    admitted: bool,
}

/// The rows for the listing over `resource`, with `other` the other resource type this
/// service keeps.
fn rows(resource: &str, other: &str) -> Vec<Row> {
    let minted_form = format!("arn:aws:bcm-dashboards::{ACCOUNT}:{resource}/zzzzzzzz");
    // An id of the admitted characters, padded until the whole value is exactly as long
    // as the shapes' `@length` maximum, and one character longer.
    let prefix = format!("arn:aws:bcm-dashboards::{ACCOUNT}:{resource}/");
    let at_max = format!(
        "{prefix}{}",
        "z".repeat(RESOURCE_ARN_MAX_LENGTH - prefix.len())
    );
    let past_max = format!("{at_max}z");
    vec![
        Row {
            label: "six fields with the resource written <type>/<id>, the form minted",
            arn: minted_form,
            admitted: true,
        },
        Row {
            label: "a partition suffix, which the partition field's grammar admits",
            arn: format!("arn:aws-cn:bcm-dashboards::{ACCOUNT}:{resource}/zzzz"),
            admitted: true,
        },
        Row {
            label: "the wildcard resource id the pattern admits beside a concrete one",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}:{resource}/*"),
            admitted: true,
        },
        Row {
            label: "a value exactly as long as the shapes' @length maximum",
            arn: at_max,
            admitted: true,
        },
        Row {
            label: "one character past the shapes' @length maximum",
            arn: past_max,
            admitted: false,
        },
        Row {
            label: "the resource written <type>:<id>, a form these patterns do not spell",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}:{resource}:zz"),
            admitted: false,
        },
        Row {
            label: "a seventh field behind the resource id",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}:{resource}/zz:extra"),
            admitted: false,
        },
        Row {
            label: "five fields: no resource field at all",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}"),
            admitted: false,
        },
        Row {
            label: "six fields with an empty resource field",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}:"),
            admitted: false,
        },
        Row {
            label: "an empty partition field",
            arn: format!("arn::bcm-dashboards::{ACCOUNT}:{resource}/zz"),
            admitted: false,
        },
        Row {
            label: "an empty vendor field",
            arn: format!("arn:aws:::{ACCOUNT}:{resource}/zz"),
            admitted: false,
        },
        Row {
            label: "the vendor field in another case, which names another namespace",
            arn: format!("arn:aws:BCM-Dashboards::{ACCOUNT}:{resource}/zz"),
            admitted: false,
        },
        Row {
            label: "the resource type of the other resource this service keeps",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}:{other}/zz"),
            admitted: false,
        },
        Row {
            label: "a resource id outside the id grammar",
            arn: format!("arn:aws:bcm-dashboards::{ACCOUNT}:{resource}/ZZ"),
            admitted: false,
        },
    ]
}

/// A cursor is read as a resume point exactly when the value it carries is an ARN of the
/// resource the listing walks, split into its six fields and its `<type>/<id>` resource
/// by the shared reader and held to this service's pattern field by field. Anything else
/// is refused with `ValidationException` (400), in the words the refused listing states.
#[tokio::test]
async fn both_listings_read_a_cursors_arn_through_the_shared_reader() {
    let router = router();
    stored_fixtures(&router).await;

    for listing in listings() {
        let case = listing.operation;
        let resource = listing.arn_resource_type;
        let other = if resource == DASHBOARD_RESOURCE_TYPE {
            SCHEDULED_REPORT_RESOURCE_TYPE
        } else {
            DASHBOARD_RESOURCE_TYPE
        };

        for row in rows(resource, other) {
            let token = encode_page_token(&row.arn);
            let (status, body) = call(&router, case, &json!({"nextToken": token})).await;
            let label = row.label;
            if row.admitted {
                assert_eq!(
                    status, 200,
                    "case {case} {label}: {:?} is an ARN of this listing's resource, so the \
                     cursor resumes the walk, got {status}: {body}",
                    row.arn
                );
                // A read cursor is a RESUME POINT: the page is the required list, and
                // every record on it sorts at or after the ARN the cursor named.
                let arns = page_arns(&body, &listing, label);
                assert!(
                    arns.iter().all(|arn| arn.as_str() >= row.arn.as_str()),
                    "case {case} {label}: a cursor resumes the walk at {:?}, so no earlier \
                     record may be on the page, got {arns:?}",
                    row.arn
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case} {label}: {:?} is no ARN of this listing's resource, so the \
                     cursor must be refused, got {status}: {body}",
                    row.arn
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case} {label}: expected ValidationException, got {}",
                    error_code(&body)
                );
                let message = body["message"].as_str().unwrap_or("<no message>");
                let expected = format!(
                    "Member must be a nextToken returned by a previous {case} call: nextToken"
                );
                assert_eq!(
                    message, expected,
                    "case {case} {label}: expected {expected:?}, got {message:?}"
                );
            }
        }
    }
}

/// The cursor a listing MINTS round-trips through the same reader: it names the first
/// record after the page, so resuming at it answers exactly that record — for both
/// listings, over every page size from one record to one past the stored count.
#[tokio::test]
async fn a_minted_cursor_resumes_at_the_record_it_names() {
    let router = router();
    stored_fixtures(&router).await;

    for listing in listings() {
        let case = listing.operation;
        let every = ok_call(&router, case, &json!({"maxResults": 100})).await;
        let stored = page_arns(&every, &listing, case);
        assert!(
            stored.len() >= 2,
            "case {case}: expected at least two stored records to page over, got {every}"
        );
        assert!(
            every.get("nextToken").is_none(),
            "case {case}: a page holding every record owes no cursor: {every}"
        );

        for page_size in 1..=stored.len() + 1 {
            let page = ok_call(&router, case, &json!({"maxResults": page_size})).await;
            let first = page_arns(&page, &listing, case);
            let token = page.get("nextToken").and_then(Value::as_str);
            match stored.get(page_size) {
                None => assert!(
                    token.is_none(),
                    "case {case} page size {page_size}: no record remains, so no cursor is \
                     owed, got {page}"
                ),
                Some(next) => {
                    let token = token.unwrap_or_else(|| {
                        panic!(
                            "case {case} page size {page_size}: {next:?} remains, so a cursor \
                             is owed: {page}"
                        )
                    });
                    let resumed =
                        ok_call(&router, case, &json!({"maxResults": 1, "nextToken": token}))
                            .await;
                    assert_eq!(
                        page_arns(&resumed, &listing, case),
                        vec![next.clone()],
                        "case {case} page size {page_size}: the minted cursor must resume at \
                         {next:?}, the record after {first:?}: {resumed}"
                    );
                }
            }
        }
    }
}
