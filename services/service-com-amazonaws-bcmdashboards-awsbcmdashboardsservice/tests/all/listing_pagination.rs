// SPDX-License-Identifier: CC0-1.0

//! The ONE pagination rule both of this service's listings serve, driven end to end
//! through each of them.
//!
//! `ListDashboards` and `ListScheduledReports` page over their resource map the same
//! way: the walk runs in ARN order, a page holds at most `maxResults` records,
//! `nextToken` is present exactly when a record remains after the page and resumes the
//! walk at that record, and a `nextToken` this service did not mint for that listing's
//! resource type is refused with `ValidationException` (400). The suite below asserts
//! that rule over BOTH listings from one table, so the shared implementation is
//! exercised through an operation of each kind rather than one of them only.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::{
    DASHBOARD_RESOURCE_TYPE, SCHEDULED_REPORT_RESOURCE_TYPE,
};

use super::dashboard_lifecycle::{call, error_code, managed_dashboard_count, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// Which resource map a listing pages over — the only thing that differs between the
/// two listings as far as the pagination rule is concerned.
#[derive(Clone, Copy, Debug)]
enum Resource {
    Dashboard,
    ScheduledReport,
}

/// One listing under test: its operation, the output member its page lands in, the
/// resource it pages over and the ARN resource type that resource's cursors name.
#[derive(Debug)]
struct Listing {
    operation: &'static str,
    items_key: &'static str,
    resource: Resource,
    arn_resource_type: &'static str,
}

/// Both listings this service serves.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListDashboards",
            items_key: "dashboards",
            resource: Resource::Dashboard,
            arn_resource_type: DASHBOARD_RESOURCE_TYPE,
        },
        Listing {
            operation: "ListScheduledReports",
            items_key: "scheduledReports",
            resource: Resource::ScheduledReport,
            arn_resource_type: SCHEDULED_REPORT_RESOURCE_TYPE,
        },
    ]
}

/// Create the `index`-th resource of `resource` and answer its ARN. `dashboard_arn` is
/// the dashboard a scheduled report renders.
async fn create_resource(
    router: &MultiServiceRouter,
    resource: Resource,
    index: usize,
    dashboard_arn: &str,
) -> String {
    let created = match resource {
        Resource::Dashboard => {
            ok_call(
                router,
                "CreateDashboard",
                &json!({"name": format!("Paged Board {index:02}"), "widgets": []}),
            )
            .await
        }
        Resource::ScheduledReport => {
            ok_call(
                router,
                "CreateScheduledReport",
                &json!({
                    "scheduledReport": {
                        "name": format!("Paged Report {index:02}"),
                        "dashboardArn": dashboard_arn,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                    }
                }),
            )
            .await
        }
    };
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {resource:?} {index}: no arn in {created}"))
        .to_string()
}

/// One page of `listing`.
async fn fetch_page(
    router: &MultiServiceRouter,
    listing: &Listing,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = json!({});
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    ok_call(router, listing.operation, &request).await
}

/// The ARNs a page carries, in the order the page carries them.
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

/// Both listings page by the same rule: every page holds at most `maxResults` records,
/// `nextToken` is present exactly when records remain, and resuming from it walks every
/// stored record exactly once in ARN order — at every page size from one to past the
/// stored count.
#[tokio::test]
async fn both_listings_page_by_the_same_rule() {
    let router = router();
    // The source dashboard is itself a stored dashboard, so each listing pages over the
    // same number of records and the two tables are directly comparable.
    let (dashboard_arn, _) = source_dashboard(&router, "Paged Board 00").await;

    for listing in listings() {
        let case = listing.operation;

        // Before anything of this kind is stored the required list is empty and no
        // cursor is owed — except for dashboards, where the source dashboard is
        // already stored beside the AWS-managed catalog a fresh store holds.
        let own_already = match listing.resource {
            Resource::Dashboard => 1,
            Resource::ScheduledReport => 0,
        };
        let stored_already = own_already
            + match listing.resource {
                Resource::Dashboard => managed_dashboard_count(),
                Resource::ScheduledReport => 0,
            };
        let empty = fetch_page(&router, &listing, None, None).await;
        assert_eq!(
            page_arns(&empty, &listing, case).len(),
            stored_already,
            "case {case} before storing: expected {stored_already} records, got {empty}"
        );
        assert!(
            empty.get("nextToken").is_none(),
            "case {case} before storing: an exhausted listing carries no nextToken: {empty}"
        );

        let mut expected: Vec<String> = page_arns(&empty, &listing, case);
        for index in own_already..5 {
            expected.push(create_resource(&router, listing.resource, index, &dashboard_arn).await);
        }
        // Both maps are keyed by ARN, so both listings walk in ARN order.
        expected.sort();
        let stored = expected.len();

        for page_size in [1_usize, 2, 3, 4, 5, 100] {
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let page = fetch_page(&router, &listing, Some(page_size), token.as_deref()).await;
                pages += 1;
                let arns = page_arns(&page, &listing, &format!("{case} page size {page_size}"));
                let want_on_page = (stored - collected.len()).min(page_size);
                assert_eq!(
                    arns.len(),
                    want_on_page,
                    "case {case} page size {page_size} page {pages}: expected {want_on_page} records, got {page}"
                );
                collected.extend(arns);
                let remain = stored - collected.len();
                token = page
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                assert_eq!(
                    token.is_some(),
                    remain > 0,
                    "case {case} page size {page_size} page {pages}: {remain} records remain, so nextToken present={} is wrong: {page}",
                    token.is_some()
                );
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= stored,
                    "case {case} page size {page_size}: pagination did not terminate after {pages} pages"
                );
            }
            assert_eq!(
                collected, expected,
                "case {case} page size {page_size}: paging must walk every record exactly once, in ARN order"
            );
            let want_pages = stored.div_ceil(page_size);
            assert_eq!(
                pages, want_pages,
                "case {case} page size {page_size}: expected {want_pages} pages, walked {pages}"
            );
        }
    }
}

/// Both listings refuse the same bad page requests, each naming its OWN operation in
/// the refusal: a `maxResults` outside the modelled 1..100, and a `nextToken` the
/// listing did not mint for its resource type — including the cursor the OTHER listing
/// minted.
#[tokio::test]
async fn both_listings_refuse_the_same_bad_page_requests() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "Refusal Source").await;
    // Two of each kind, so each listing's first page of one owes a cursor.
    for index in 1..=2 {
        create_resource(&router, Resource::Dashboard, index, &dashboard_arn).await;
        create_resource(&router, Resource::ScheduledReport, index, &dashboard_arn).await;
    }

    // The cursor each listing mints, to offer to the other one.
    let mut cursors: Vec<(&'static str, String)> = Vec::new();
    for listing in listings() {
        let page = fetch_page(&router, &listing, Some(1), None).await;
        let cursor = page
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: records remain after a page of one, so a cursor is owed: {page}",
                    listing.operation
                )
            })
            .to_string();
        cursors.push((listing.operation, cursor));
    }

    for listing in listings() {
        let case = listing.operation;

        // Source: `maxResults`, range 1..100 on both listings
        // (`smithy_query(query="operation",operation="ListDashboards,ListScheduledReports")`)
        // — each published bound, and one past each.
        let max_results_cases: &[(&str, i64, bool)] = &[
            ("at min", 1, true),
            ("inside", 37, true),
            ("at max", 100, true),
            ("below min", 0, false),
            ("above max", 101, false),
            ("negative", -7, false),
        ];
        for (label, max_results, accepted) in max_results_cases {
            let (status, body) =
                call(&router, listing.operation, &json!({"maxResults": max_results})).await;
            if *accepted {
                assert_eq!(
                    status, 200,
                    "case {case} maxResults {label}: {max_results} is inside 1..100, got {status}: {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case} maxResults {label}: {max_results} is outside 1..100, got {status}: {body}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case} maxResults {label}: expected ValidationException, got {}",
                    error_code(&body)
                );
            }
        }

        // Tokens no listing minted, plus the cursor the OTHER listing minted: all
        // refused, and the refusal names the listing that was called.
        let other_cursor = cursors
            .iter()
            .find(|(operation, _)| *operation != listing.operation)
            .map(|(_, cursor)| cursor.clone())
            .expect("the other listing minted a cursor");
        for token in [
            "not-a-token",
            "v1:",
            "v1:zzzz",
            "v1:6e6f",
            "deadbeef",
            "{\"page\": 2}",
            other_cursor.as_str(),
        ] {
            let (status, body) = call(&router, listing.operation, &json!({"nextToken": token})).await;
            assert_eq!(
                status, 400,
                "case {case} token {token:?}: an uninterpretable nextToken must be refused, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case} token {token:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            let message = body["message"].as_str().unwrap_or("<no message>");
            assert!(
                message.contains(listing.operation) && message.contains("nextToken"),
                "case {case} token {token:?}: the refusal must name this listing and the member, got {message:?}"
            );
        }

        // And its OWN cursor still resumes the walk, so the refusals above rejected the
        // token rather than the listing's cursor format.
        let own_cursor = cursors
            .iter()
            .find(|(operation, _)| *operation == listing.operation)
            .map(|(_, cursor)| cursor.clone())
            .expect("this listing minted a cursor");
        let resumed = fetch_page(&router, &listing, Some(1), Some(&own_cursor)).await;
        assert_eq!(
            page_arns(&resumed, &listing, case).len(),
            1,
            "case {case}: its own cursor must resume the walk: {resumed}"
        );
    }
}

/// The prefix a cursor of this service carries, with the ARN the page resumes at hex
/// encoded behind it — the form `encode_page_token` mints, spelled here so a token the
/// service never minted can be handed to a listing.
const CURSOR_PREFIX: &str = "v1:";

/// A cursor of this service's form whose body is `arn`, well formed or not.
fn cursor_for(arn: &str) -> String {
    format!("{CURSOR_PREFIX}{}", hex::encode(arn))
}

/// A `nextToken` is read as a resume point only when it decodes to an ARN of the resource
/// the listing walks, as that resource's modelled ARN pattern spells it — anchored, so
/// every field is judged at its own position. Anything else is refused with
/// `ValidationException` (400) rather than read as a resume point, which would answer a
/// silently truncated or empty page with HTTP 200.
///
/// Source for the expression: the `DashboardArn` and `ScheduledReportArn` patterns,
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:<resource-type>/(\*|[-a-z0-9]+)$`, as
/// `smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")` reports them.
/// The rows differ along its fields one at a time — partition, namespace, region,
/// account, resource type, resource id and the number of fields — and the table is
/// driven through BOTH listings, since one decoder serves them both.
#[tokio::test]
async fn a_cursor_resumes_only_when_it_names_an_arn_of_the_resource_the_listing_walks() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "Cursor Pattern Source").await;
    for index in 1..=3 {
        create_resource(&router, Resource::Dashboard, index, &dashboard_arn).await;
        create_resource(&router, Resource::ScheduledReport, index, &dashboard_arn).await;
    }

    for listing in listings() {
        let case = listing.operation;
        let resource = listing.arn_resource_type;
        let other = if resource == DASHBOARD_RESOURCE_TYPE {
            SCHEDULED_REPORT_RESOURCE_TYPE
        } else {
            DASHBOARD_RESOURCE_TYPE
        };
        // An id of this form holds only the characters the pattern's resource segment
        // admits and sorts after every id this service mints (hex digits, or the managed
        // catalog's `managed-` prefix), so a cursor naming it is a resume point past the
        // last stored record.
        let past_every_record = format!("arn:aws:bcm-dashboards::000000000000:{resource}/zzzzzzzz");

        // (row, the ARN the cursor's body decodes to, whether the pattern admits it)
        let rows: Vec<(&str, String, bool)> = vec![
            ("the minted form", past_every_record.clone(), true),
            (
                "a partition suffix, which `aws[-a-z0-9]*` admits",
                format!("arn:aws-cn:bcm-dashboards::000000000000:{resource}/zzzz"),
                true,
            ),
            (
                "the wildcard resource id the pattern admits",
                format!("arn:aws:bcm-dashboards::000000000000:{resource}/*"),
                true,
            ),
            (
                "an account of letters",
                format!("arn:aws:bcm-dashboards::nope:{resource}/zz"),
                false,
            ),
            (
                "an account of eleven digits",
                format!("arn:aws:bcm-dashboards::00000000000:{resource}/zz"),
                false,
            ),
            (
                "an account of thirteen digits",
                format!("arn:aws:bcm-dashboards::0000000000000:{resource}/zz"),
                false,
            ),
            (
                "a region, which these ARNs do not carry",
                format!("arn:aws:bcm-dashboards:us-east-1:000000000000:{resource}/zz"),
                false,
            ),
            (
                "another vendor's ARN namespace",
                format!("arn:aws:cloudwatch::000000000000:{resource}/zz"),
                false,
            ),
            (
                "a partition that does not open with aws",
                format!("arn:azure:bcm-dashboards::000000000000:{resource}/zz"),
                false,
            ),
            (
                "no resource id at all",
                format!("arn:aws:bcm-dashboards::000000000000:{resource}/"),
                false,
            ),
            (
                "an uppercase resource id",
                format!("arn:aws:bcm-dashboards::000000000000:{resource}/ZZ"),
                false,
            ),
            (
                "text after the resource id",
                format!("arn:aws:bcm-dashboards::000000000000:{resource}/zz and more"),
                false,
            ),
            (
                "the resource segment carried in a seventh field",
                format!("arn:aws:bcm-dashboards::000000000000:{other}/aa:{resource}/zz"),
                false,
            ),
            (
                "a field of its own before the ARN",
                format!("resume:arn:aws:bcm-dashboards::000000000000:{resource}/zz"),
                false,
            ),
            (
                "the other resource type of this service",
                format!("arn:aws:bcm-dashboards::000000000000:{other}/zz"),
                false,
            ),
        ];

        for (label, arn, admitted) in &rows {
            let token = cursor_for(arn);
            let (status, body) =
                call(&router, listing.operation, &json!({"nextToken": token})).await;
            if *admitted {
                assert_eq!(
                    status, 200,
                    "case {case} {label}: the pattern admits {arn:?}, so it resumes the walk, got {status}: {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case} {label}: {arn:?} is no ARN of this listing's resource, so it must be refused, got {status}: {body}"
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

        // An admitted cursor is a RESUME POINT, not a page of its own: one naming an ARN
        // past every stored record ends the walk, and one naming a stored ARN resumes at
        // exactly that record — so the refusals above rejected the ARN each token carried
        // and not the hand-built form.
        let ended = fetch_page(&router, &listing, Some(5), Some(&cursor_for(&past_every_record))).await;
        assert!(
            page_arns(&ended, &listing, case).is_empty() && ended.get("nextToken").is_none(),
            "case {case}: a cursor past every stored record is an empty last page: {ended}"
        );

        let every = fetch_page(&router, &listing, Some(100), None).await;
        let stored = page_arns(&every, &listing, case);
        let second = stored
            .get(1)
            .unwrap_or_else(|| panic!("case {case}: expected at least two stored records: {every}"))
            .clone();
        let resumed = fetch_page(&router, &listing, Some(1), Some(&cursor_for(&second))).await;
        assert_eq!(
            page_arns(&resumed, &listing, case),
            vec![second.clone()],
            "case {case}: a cursor naming the stored ARN {second:?} resumes at that record: {resumed}"
        );
    }
}
