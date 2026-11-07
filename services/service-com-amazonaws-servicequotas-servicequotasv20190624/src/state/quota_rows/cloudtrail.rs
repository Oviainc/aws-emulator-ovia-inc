// SPDX-License-Identifier: CC0-1.0

//! AWS CloudTrail's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name CloudTrail publishes its call counts under.
const CLOUDTRAIL_USAGE_SERVICE: &str = "CloudTrail";

/// The transactions per second CloudTrail accepts for one of its management APIs.
///
/// CloudTrail publishes a throttle per API operation, and every member but the operation's
/// own name and the value is the same across the class: the window is one second, the usage
/// metric is that operation's `AWS/Usage` `CallCount` under the service's own dimension,
/// and the `Description` is the sentence the class is published with and the operation named
/// in it. One constructor fills them, so two rows of the class cannot drift apart.
fn api_throttle(code: &str, quota_name: String, api_name: &str, per_second: f64) -> QuotaRow {
    quota(quota_name, per_second)
        .code(code)
        .period(1, "SECOND")
        .metric(call_count_metric(CLOUDTRAIL_USAGE_SERVICE, api_name))
        .describe(&format!(
            "Maximum number of transactions per second (TPS) for each account for the \
             {api_name} API."
        ))
}

/// Every API CloudTrail publishes a per-second throttle for, with the code AWS publishes
/// that throttle under and the transactions per second it allows.
///
/// SOURCE: AWS's published Service Quotas data for the `cloudtrail` service code, whose
/// rows are dominated by this class — one per management API — and the recorded
/// default-quota listing of that code for which `QuotaCode` names which API's throttle and
/// for the rate each allows. `LookupEvents` is the one API of the class AWS publishes a
/// lower rate for.
const API_THROTTLES: &[(&str, &str, f64)] = &[
    ("L-08F66A51", "AddTags", 10.0),
    ("L-0DE8FCE1", "CancelQuery", 10.0),
    ("L-102075A9", "GetImport", 10.0),
    ("L-1433C85D", "CreateChannel", 10.0),
    ("L-203ED99D", "CreateDashboard", 10.0),
    ("L-235E2A41", "CreateEventDataStore", 10.0),
    ("L-2A4525B6", "CreateTrail", 10.0),
    ("L-2AAD1FD3", "DeleteChannel", 10.0),
    ("L-3044E773", "DeleteDashboard", 10.0),
    ("L-33958DE7", "DeleteEventDataStore", 10.0),
    ("L-3AD9B964", "DeleteResourcePolicy", 10.0),
    ("L-400AA82D", "DeleteTrail", 10.0),
    ("L-422D51DD", "DeregisterOrganizationDelegatedAdmin", 10.0),
    ("L-42572559", "DescribeQuery", 10.0),
    ("L-4964C79E", "DescribeTrails", 10.0),
    ("L-531B4597", "DisableFederation", 10.0),
    ("L-54F77C56", "EnableFederation", 10.0),
    ("L-58D9CABD", "GenerateQuery", 10.0),
    ("L-5E4FC5D2", "GetChannel", 10.0),
    ("L-631CF1F1", "GetDashboard", 10.0),
    ("L-66AE0662", "GetEventConfiguration", 10.0),
    ("L-6826A2E9", "GetEventDataStore", 10.0),
    ("L-694E6530", "GetEventSelectors", 10.0),
    ("L-71DEA5C6", "GetInsightSelectors", 10.0),
    ("L-72996F38", "GetQueryResults", 10.0),
    ("L-773FB33E", "GetResourcePolicy", 10.0),
    ("L-7A3F9D4B", "GetTrail", 10.0),
    ("L-8191F390", "GetTrailStatus", 10.0),
    ("L-81CCF560", "ListChannels", 10.0),
    ("L-82338BB4", "ListDashboards", 10.0),
    ("L-84EB1525", "ListEventDataStores", 10.0),
    ("L-865ADF4E", "ListImports", 10.0),
    ("L-866533BF", "ListInsightsMetricData", 10.0),
    ("L-8719FDD4", "ListPublicKeys", 10.0),
    ("L-88C956C4", "ListQueries", 10.0),
    ("L-8F9F4270", "ListTags", 10.0),
    ("L-9260AF5B", "ListTrails", 10.0),
    ("L-9387CED7", "LookupEvents", 2.0),
    ("L-961309DE", "PutAuditEvents", 10.0),
    ("L-98AA9EBA", "PutEventConfiguration", 10.0),
    ("L-9B1A1B4A", "PutEventSelectors", 10.0),
    ("L-9F37432B", "PutInsightSelectors", 10.0),
    ("L-A60E6D1D", "PutResourcePolicy", 10.0),
    ("L-AD66B35A", "RegisterOrganizationDelegatedAdmin", 10.0),
    ("L-AFFDFD7B", "RemoveTags", 10.0),
    ("L-B4D176B9", "RestoreEventDataStore", 10.0),
    ("L-B643CFE7", "SearchSampleQueries", 10.0),
    ("L-BEA14027", "StartDashboardRefresh", 10.0),
    ("L-BEC68768", "StartEventDataStoreIngestion", 10.0),
    ("L-C46BC83A", "StartImport", 10.0),
    ("L-C9DC023A", "StartLogging", 10.0),
    ("L-D0872747", "StartQuery", 10.0),
    ("L-D08C1460", "StopEventDataStoreIngestion", 10.0),
    ("L-E2417ECC", "StopImport", 10.0),
    ("L-EA524270", "StopLogging", 10.0),
    ("L-EBC99F5D", "UpdateChannel", 10.0),
    ("L-EF071748", "UpdateDashboard", 10.0),
    ("L-F5FAD268", "UpdateEventDataStore", 10.0),
    ("L-FAC66D2D", "UpdateTrail", 10.0),
];

/// AWS CloudTrail's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cloudtrail` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code cloudtrail` reports, which
/// is the "Quotas in AWS CloudTrail" page of the AWS CloudTrail User Guide), and the
/// recorded default-quota listing of that code for which `QuotaCode` names which quota, for
/// the per-API wording of the throttle class and for each row's unit and adjustability.
///
/// The catalog is mostly the per-API throttle class [`API_THROTTLES`] holds; the rows below
/// are the planes and the measured limits CloudTrail publishes beside it. Rows are published
/// in `QuotaCode` order, which is the order the recorded listing of this service carries.
pub(in crate::state) fn cloudtrail_quotas() -> Vec<QuotaRow> {
    let mut rows = vec![
        quota("Event data stores per Region", 10.0)
            .code("L-08352D4A")
            .describe(
                "The maximum number of event data stores in this account in the current \
                 Region.",
            ),
        quota("Channels per Region", 5.0).code("L-1568E18E").describe(
            "The maximum number of channels in this account in the current Region.",
        ),
        quota("Dashboards per Region", 10.0)
            .code("L-262F8D6F")
            .describe(
                "The maximum number of dashboards in this account in the current Region.",
            ),
        quota("Concurrent queries", 10.0).code("L-33C67BE1").describe(
            "The maximum number of queries running at once in this account in the current \
             Region.",
        ),
        quota("Summarized query results size", 3.0)
            .code("L-3BB16389")
            .unit("Megabytes")
            .describe("The maximum size, in MB, of the results a query is summarized over."),
        quota("Event selectors per trail", 5.0)
            .code("L-46286FE1")
            .fixed()
            .describe("The maximum number of event selectors on one trail."),
        quota("Data resources per trail", 250.0)
            .code("L-62E172F7")
            .fixed()
            .describe("The maximum number of data resources on one trail's event selectors."),
        quota("Advanced event selectors per trail", 500.0)
            .code("L-7AC9FF3E")
            .fixed()
            .describe("The maximum number of advanced event selectors on one trail."),
        quota("Imports in progress", 5.0)
            .code("L-888731F1")
            .fixed()
            .describe(
                "The maximum number of imports in progress at once in this account in the \
                 current Region.",
            ),
        quota("Trails per Region", 5.0)
            .code("L-9552D44E")
            .fixed()
            .describe("The maximum number of trails in this account in the current Region."),
        quota("Maximum event size", 256.0)
            .code("L-A0136C1A")
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size, in KB, of one event CloudTrail records."),
        quota("Query results retention period", 7.0)
            .code("L-CA6CC904")
            .unit("Days")
            .fixed()
            .describe("The number of days a query's results stay available for."),
    ];
    for (code, api_name, per_second) in API_THROTTLES {
        rows.push(api_throttle(
            code,
            format!("Transactions per second (TPS) for the {api_name} API"),
            api_name,
            *per_second,
        ));
    }
    // The one API of the class AWS publishes the throttle of under the SHORT form of the
    // name, which is the name the recorded listing of this service carries for it.
    rows.push(api_throttle(
        "L-A4CD5068",
        "TPS for the ListImportFailures API".to_string(),
        "ListImportFailures",
        10.0,
    ));
    rows.sort_by(|left, right| left.code.cmp(&right.code));
    rows
}
