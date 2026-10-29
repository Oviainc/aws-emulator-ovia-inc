// SPDX-License-Identifier: CC0-1.0

//! The order in which a handler of this service judges a request: the request's OWN
//! members FIRST, then the resource it addresses, then the caller's permission.
//!
//! The rule under test: a refusal that is about the request's own members — a check that
//! stands in for a constraint the framework's boundary walk cannot apply, and that reads
//! neither the store nor the caller's identity — is answered BEFORE the not-found for the
//! resource the request addresses and before the authorization decision, because that
//! walk runs in front of everything a handler judges. The modelled member errors are
//! therefore reachable for a resource that does not exist and for a caller holding no
//! grant.
//!
//! The table spans what the rule generalises over: the operation, the kind of member
//! check (a document's size, a document's syntax, a member pairing the model states in
//! prose, a value outside the catalog an operation searches), the modelled error that
//! check carries (`MalformedPolicyDocumentException`, `PolicySizeLimitExceededException`,
//! `ValidationException`), the HTTP method and the member's position in the request. Each
//! row also carries the SAME request with members the service admits, which pins the two
//! refusals the member check must not swallow: the not-found (404) for an app monitor that
//! does not exist, and the access denial (403) for a caller holding no grant.
//!
//! Everything is driven through the real wire path and through each service's own API: the
//! app monitor is created with `CreateAppMonitor`, the unauthorized principal is an IAM
//! role created with IAM's `CreateRole` and assumed through STS's `AssumeRole`, and every
//! RUM call carries that credential's access key id in its SigV4 credential scope.

use serde_json::{json, Value};

use super::authorization::{Emulator, Reply, ACCOUNT_KEY};
use super::support::experiment_arn;

/// An app monitor name no case creates, so every request naming it addresses a resource
/// that exists in no region.
const ABSENT_MONITOR: &str = "no-such-app-monitor";

/// The app monitor the authorization rows address, which exists for every one of them.
const EXISTING_MONITOR: &str = "member-check-precedence";

/// The documented ceiling on a resource policy document.
/// Source: `smithy_query(query="operation", operation="PutResourcePolicy")` — the
/// operation's doc says "The maximum size of the policy is 4 KB", and
/// `PolicySizeLimitExceededException` says "The limit is 4 KB".
const MAX_POLICY_DOCUMENT_BYTES: usize = 4 * 1024;

/// The destination kinds `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` — an enum of
/// `CloudWatch` and `Evidently`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// One request whose members the service refuses, paired with the same request carrying
/// members it admits.
struct Probe {
    /// What this row is, named in every assertion.
    case: &'static str,
    /// The operation's HTTP method.
    method: &'static str,
    /// The operation's URI with the app monitor's name left as `{monitor}`.
    path: &'static str,
    /// A body whose OWN members the handler refuses, needing neither the store nor the
    /// caller's identity to decide it.
    offending: Value,
    /// The status that member refusal carries.
    status: u16,
    /// The modelled error code that member refusal carries.
    code: &'static str,
    /// The same request with members the service admits, so what answers it is the
    /// not-found or the access denial the row is checking the member refusal precedes.
    admitted: Value,
}

impl Probe {
    /// This row's URI for the app monitor `monitor`.
    fn uri(&self, monitor: &str) -> String {
        self.path.replace("{monitor}", monitor)
    }
}

/// A resource policy document of exactly `bytes` bytes, well formed at any size: the
/// padding goes inside a statement's `Sid`, which the IAM policy language takes as a
/// string, so nothing but the document's SIZE can be what a refusal is about.
fn sized_policy_document(bytes: usize) -> String {
    let prefix = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"";
    let suffix = "\",\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\
                  \"arn:aws:iam::111122223333:root\"},\"Action\":\"rum:PutRumEvents\",\
                  \"Resource\":\"*\"}]}";
    let fixed = prefix.len() + suffix.len();
    assert!(
        bytes >= fixed,
        "a policy document of {bytes} bytes is shorter than the {fixed} bytes a statement takes"
    );
    format!("{prefix}{}{suffix}", "p".repeat(bytes - fixed))
}

/// A resource policy document the service stores, well under the documented ceiling.
fn well_formed_policy_document() -> String {
    sized_policy_document(256)
}

/// A metric definition the service stores: a custom metric under a namespace of its own.
fn metric_definition() -> Value {
    json!({"Name": "CartSize", "Namespace": "MyApp/Cart"})
}

/// Every row of the table: one request per member check this service makes, across the
/// operations that make one.
fn probes() -> Vec<Probe> {
    let valid_policy = well_formed_policy_document();
    vec![
        Probe {
            case: "PutResourcePolicy, a PolicyDocument that is not JSON at all",
            method: "PUT",
            path: "/appmonitor/{monitor}/policy",
            offending: json!({"PolicyDocument": "{ not json"}),
            status: 400,
            code: "MalformedPolicyDocumentException",
            admitted: json!({"PolicyDocument": valid_policy}),
        },
        Probe {
            case: "PutResourcePolicy, a PolicyDocument that is JSON but not an object",
            method: "PUT",
            path: "/appmonitor/{monitor}/policy",
            offending: json!({"PolicyDocument": "[\"rum:PutRumEvents\"]"}),
            status: 400,
            code: "MalformedPolicyDocumentException",
            admitted: json!({"PolicyDocument": well_formed_policy_document()}),
        },
        Probe {
            case: "PutResourcePolicy, a policy whose statement carries no Effect",
            method: "PUT",
            path: "/appmonitor/{monitor}/policy",
            offending: json!({
                "PolicyDocument":
                    "{\"Statement\":[{\"Principal\":\"*\",\"Action\":\"rum:PutRumEvents\"}]}",
            }),
            status: 400,
            code: "MalformedPolicyDocumentException",
            admitted: json!({"PolicyDocument": well_formed_policy_document()}),
        },
        Probe {
            // One byte past the documented ceiling, against a document that is exactly at
            // it — so the row pins the bound as well as the precedence.
            case: "PutResourcePolicy, a PolicyDocument one byte past the 4 KB ceiling",
            method: "PUT",
            path: "/appmonitor/{monitor}/policy",
            offending: json!({
                "PolicyDocument": sized_policy_document(MAX_POLICY_DOCUMENT_BYTES + 1),
            }),
            status: 400,
            code: "PolicySizeLimitExceededException",
            admitted: json!({
                "PolicyDocument": sized_policy_document(MAX_POLICY_DOCUMENT_BYTES),
            }),
        },
        Probe {
            case: "PutResourcePolicy, a PolicyDocument both oversized and malformed",
            method: "PUT",
            path: "/appmonitor/{monitor}/policy",
            offending: json!({
                "PolicyDocument": format!("{{ {}", "p".repeat(MAX_POLICY_DOCUMENT_BYTES)),
            }),
            status: 400,
            code: "PolicySizeLimitExceededException",
            admitted: json!({"PolicyDocument": well_formed_policy_document()}),
        },
        Probe {
            case: "PutRumMetricsDestination, CloudWatch carrying a DestinationArn",
            method: "POST",
            path: "/rummetrics/{monitor}/metricsdestination",
            offending: json!({
                "Destination": METRIC_DESTINATIONS[0],
                "DestinationArn": experiment_arn("precedence"),
            }),
            status: 400,
            code: "ValidationException",
            admitted: json!({"Destination": METRIC_DESTINATIONS[0]}),
        },
        Probe {
            case: "PutRumMetricsDestination, Evidently carrying no DestinationArn",
            method: "POST",
            path: "/rummetrics/{monitor}/metricsdestination",
            offending: json!({"Destination": METRIC_DESTINATIONS[1]}),
            status: 400,
            code: "ValidationException",
            admitted: json!({"Destination": METRIC_DESTINATIONS[0]}),
        },
        Probe {
            case: "UpdateRumMetricDefinition, Evidently carrying no DestinationArn",
            method: "PATCH",
            path: "/rummetrics/{monitor}/metrics",
            offending: json!({
                "Destination": METRIC_DESTINATIONS[1],
                "MetricDefinitionId": "0000",
                "MetricDefinition": metric_definition(),
            }),
            status: 400,
            code: "ValidationException",
            admitted: json!({
                "Destination": METRIC_DESTINATIONS[0],
                "MetricDefinitionId": "0000",
                "MetricDefinition": metric_definition(),
            }),
        },
        Probe {
            case: "UpdateRumMetricDefinition, a dimension name the model's pattern excludes",
            method: "PATCH",
            path: "/rummetrics/{monitor}/metrics",
            offending: json!({
                "Destination": METRIC_DESTINATIONS[0],
                "MetricDefinitionId": "0000",
                "MetricDefinition": {
                    "Name": "CartSize",
                    "Namespace": "MyApp/Cart",
                    "DimensionKeys": {"metadata.pageId": ":PageId"},
                },
            }),
            status: 400,
            code: "ValidationException",
            admitted: json!({
                "Destination": METRIC_DESTINATIONS[0],
                "MetricDefinitionId": "0000",
                "MetricDefinition": metric_definition(),
            }),
        },
        Probe {
            case: "BatchCreateRumMetricDefinitions, CloudWatch carrying a DestinationArn",
            method: "POST",
            path: "/rummetrics/{monitor}/metrics",
            offending: json!({
                "Destination": METRIC_DESTINATIONS[0],
                "DestinationArn": experiment_arn("precedence"),
                "MetricDefinitions": [metric_definition()],
            }),
            status: 400,
            code: "ValidationException",
            admitted: json!({
                "Destination": METRIC_DESTINATIONS[0],
                "MetricDefinitions": [metric_definition()],
            }),
        },
        Probe {
            case: "GetAppMonitorData, a Filters entry naming a field it does not search",
            method: "POST",
            path: "/appmonitor/{monitor}/data",
            offending: json!({
                "TimeRange": {"After": 0},
                "Filters": [{"Name": "not_an_event_field", "Values": ["any"]}],
            }),
            status: 400,
            code: "ValidationException",
            admitted: json!({"TimeRange": {"After": 0}}),
        },
    ]
}

/// Assert `reply` is the modelled refusal `code` with `status`.
fn assert_refused(reply: &Reply, case: &str, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let actual = reply.error_code(case);
    assert_eq!(
        actual, code,
        "case {case:?}: expected error code {code}, got {actual:?}"
    );
}

/// A request whose own members the service refuses is answered with that member refusal
/// even when the app monitor it addresses does not exist, while the same request with
/// members the service admits is answered with the not-found.
#[tokio::test]
async fn a_member_refusal_precedes_the_not_found_for_the_addressed_app_monitor() {
    let emulator = Emulator::new();
    // Nothing is created: every row below addresses an app monitor that exists in no
    // region, and the caller is the account's own credential, so nothing but the member
    // check and the resolution can decide these answers.
    for probe in probes() {
        let refused = emulator
            .rum(
                ACCOUNT_KEY,
                probe.method,
                &probe.uri(ABSENT_MONITOR),
                Some(probe.offending.clone()),
            )
            .await;
        assert_refused(&refused, probe.case, probe.status, probe.code);

        let control = format!("{} (members the service admits)", probe.case);
        let not_found = emulator
            .rum(
                ACCOUNT_KEY,
                probe.method,
                &probe.uri(ABSENT_MONITOR),
                Some(probe.admitted.clone()),
            )
            .await;
        assert_refused(&not_found, &control, 404, "ResourceNotFoundException");
    }
}

/// A request whose own members the service refuses is answered with that member refusal
/// even when the caller holds no grant for the operation, while the same request with
/// members the service admits is answered with the access denial.
#[tokio::test]
async fn a_member_refusal_precedes_the_authorization_decision() {
    let emulator = Emulator::new();
    emulator.create_monitor(EXISTING_MONITOR).await;
    // A principal with no policy at all: every action of this service on that monitor is
    // refused for it, so an answer that is not the member refusal is the denial.
    let unauthorized = emulator.assumed_role_key("holds-no-grant", None).await;

    for probe in probes() {
        let refused = emulator
            .rum(
                &unauthorized,
                probe.method,
                &probe.uri(EXISTING_MONITOR),
                Some(probe.offending.clone()),
            )
            .await;
        assert_refused(&refused, probe.case, probe.status, probe.code);

        let control = format!("{} (members the service admits)", probe.case);
        let denied = emulator
            .rum(
                &unauthorized,
                probe.method,
                &probe.uri(EXISTING_MONITOR),
                Some(probe.admitted.clone()),
            )
            .await;
        assert_refused(&denied, &control, 403, "AccessDeniedException");
    }
}

/// A refused request writes nothing: after every row above, the app monitor carries no
/// resource policy and no metrics destination, read back through the operations that
/// report them with a caller that may read them.
#[tokio::test]
async fn a_member_refusal_stores_nothing() {
    let emulator = Emulator::new();
    emulator.create_monitor(EXISTING_MONITOR).await;
    let unauthorized = emulator.assumed_role_key("holds-no-grant", None).await;

    for probe in probes() {
        let reply = emulator
            .rum(
                &unauthorized,
                probe.method,
                &probe.uri(EXISTING_MONITOR),
                Some(probe.offending.clone()),
            )
            .await;
        assert_refused(&reply, probe.case, probe.status, probe.code);
    }

    let policy = emulator
        .rum(
            ACCOUNT_KEY,
            "GET",
            &format!("/appmonitor/{EXISTING_MONITOR}/policy"),
            None,
        )
        .await;
    assert_refused(
        &policy,
        "the stored resource policy after the refusals",
        404,
        "PolicyNotFoundException",
    );

    let destinations = emulator
        .rum(
            ACCOUNT_KEY,
            "GET",
            &format!("/rummetrics/{EXISTING_MONITOR}/metricsdestination"),
            None,
        )
        .await;
    let case = "the stored metrics destinations after the refusals";
    assert_eq!(
        destinations.status, 200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        destinations.status,
        String::from_utf8_lossy(&destinations.body)
    );
    let reported = destinations.json(case)["Destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no metrics destination to have been stored, got {reported:?}"
    );
}
