// SPDX-License-Identifier: CC0-1.0

//! The three facts every aps response states, driven end to end over HTTP:
//!
//! 1. the MEDIA TYPE it announces — one value for every JSON body this service produces,
//!    whatever the status and whichever operation served it;
//! 2. the shape of a REFUSED request — the modelled `ValidationException`, with `reason`,
//!    a `fieldList` entry per refused member and a `message` naming the member and the
//!    requirement it failed, never the value the caller sent;
//! 3. the message a MISSING resource reports — one constant sentence per kind of
//!    resource, with the requested identifier in `resourceId` and nowhere else.
//!
//! Each suite is a TABLE: the rule is a property of the service, so the rows span
//! statuses, operations, constraint classes and member types rather than restating one
//! recorded call.

use crate::workspace_lifecycle::{text, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    JSON_CONTENT_TYPE, LOGGING_NOT_FOUND_MESSAGE, POLICY_NOT_FOUND_MESSAGE,
    QUERY_LOGGING_NOT_FOUND_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE, RESPONSE_CONTENT_TYPE,
    RULE_GROUPS_NAMESPACE_NOT_FOUND_MESSAGE, VALIDATION_REASON, WORKSPACE_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_amp_amazonprometheusservice::EmulatedService;

/// Two `WorkspaceId`s no store mints (`[0-9A-Za-z][-.0-9A-Z_a-z]*`, 1..64), so every
/// per-workspace operation answers its not-found refusal for them.
const ABSENT_WORKSPACES: [&str; 2] = [
    "ws-00000000-0000-0000-0000-000000000000",
    "ws-ffffffff-ffff-ffff-ffff-ffffffffffff",
];

/// A CloudWatch Logs log group ARN satisfying `CreateLoggingConfiguration$logGroupArn`'s
/// pattern `^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$`.
fn log_group_arn(region: &str) -> String {
    format!(
        "arn:aws:logs:{region}:{}:log-group:/aws/vendedlogs/amp:*",
        engine::emulator_account_id()
    )
}

/// A KMS key ARN satisfying `KmsKeyArn`'s pattern
/// `^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+$`.
fn kms_key_arn(region: &str) -> String {
    format!(
        "arn:aws:kms:{region}:{}:key/1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
        engine::emulator_account_id()
    )
}

/// The `destinations` member of a query logging configuration: `LoggingDestinations` is
/// modelled `len 1..1`, so exactly one.
fn destinations_json(region: &str) -> serde_json::Value {
    serde_json::json!([{
        "cloudWatchLogs": { "logGroupArn": log_group_arn(region) },
        "filters": { "qspThreshold": 10 },
    }])
}

/// Percent-encode one URI path segment or query value, the way an SDK does: an ARN
/// carries `:` and `/`, and `/tags/{resourceArn}` is a single (non-greedy) label.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

/// The media type is a property of the SERVICE, not of one operation: every JSON body it
/// produces announces [`RESPONSE_CONTENT_TYPE`], across a 200, a 202, a 204 with no body
/// at all, a 400, a 404, a 409 and the 501 a scaffolded operation answers.
#[tokio::test]
async fn every_response_announces_one_media_type() {
    let region = "us-west-2";
    let amp = Amp::new();
    let active = amp.active_workspace(region, "media-type").await;
    // A workspace still CREATING, so the status-gated 409 is reachable.
    let creating = text(
        &amp.call(
            "POST",
            "/workspaces",
            region,
            &serde_json::json!({ "alias": "media-type-creating" }).to_string(),
        )
        .await
        .body["workspaceId"],
    );

    let absent = ABSENT_WORKSPACES[0];
    let cases: Vec<(&str, &str, String, String, u16)> = vec![
        (
            "ListWorkspaces, an empty-filter listing",
            "GET",
            "/workspaces".to_string(),
            String::new(),
            200,
        ),
        (
            "ListScrapers, an empty listing",
            "GET",
            "/scrapers".to_string(),
            String::new(),
            200,
        ),
        (
            "DescribeWorkspace, a stored record",
            "GET",
            format!("/workspaces/{active}"),
            String::new(),
            200,
        ),
        (
            "CreateWorkspace, a minted resource",
            "POST",
            "/workspaces".to_string(),
            serde_json::json!({ "alias": "media-type-created" }).to_string(),
            202,
        ),
        (
            "UpdateWorkspaceAlias, a 204 with no body",
            "POST",
            format!("/workspaces/{active}/alias"),
            serde_json::json!({ "alias": "media-type-renamed" }).to_string(),
            204,
        ),
        (
            "CreateWorkspace, a member outside its published pattern",
            "POST",
            "/workspaces".to_string(),
            serde_json::json!({ "kmsKeyArn": "not-a-valid-kms-arn-at-all" }).to_string(),
            400,
        ),
        (
            "DescribeWorkspace, a workspace no store holds",
            "GET",
            format!("/workspaces/{absent}"),
            String::new(),
            404,
        ),
        (
            "DeleteWorkspace, a workspace that is not yet ACTIVE",
            "DELETE",
            format!("/workspaces/{creating}"),
            String::new(),
            409,
        ),
        (
            "GetDefaultScraperConfiguration, a scaffolded operation",
            "GET",
            "/scraperconfiguration".to_string(),
            String::new(),
            501,
        ),
    ];

    for (case, method, uri, body, expected_status) in cases {
        let reply = amp.call(method, &uri, region, &body).await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected status {expected_status}, got {reply:?}"
        );
        let announced = reply.header("content-type");
        assert_eq!(
            announced,
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the response to announce {RESPONSE_CONTENT_TYPE:?}, \
             got {announced:?}"
        );
        assert_ne!(
            announced,
            Some(JSON_CONTENT_TYPE),
            "case {case:?}: the restJson1 default media type must not reach the wire, \
             got {announced:?}"
        );
    }
}

/// The media type is a property of the SERVICE, so the envelope that announces it must
/// front EVERY operation the model binds: one the gate missed would answer in the
/// `restJson1` default instead, and nothing in the request path would say so.
///
/// `OperationRoutingTable` exposes no iterator over its keys, so the model's own list is
/// read from `OPERATIONS.md` — the contract the generator renders for this crate from
/// the Smithy model, re-rendered whenever the service is scaffolded, so an operation
/// added later shows up here the moment it is bound.
#[test]
fn the_response_envelope_fronts_every_modelled_operation() {
    let service = EmulatedService::new(std::sync::Arc::new(engine::InMemoryEngine::new()));
    let announced = service.announced_operations();
    // `## <Operation>` is one operation's section; the file's other headings
    // (`## Wire protocol`) are prose and carry a space.
    let mut modelled: Vec<&str> = include_str!("../../OPERATIONS.md")
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .filter(|heading| !heading.contains(' '))
        .collect();
    modelled.sort_unstable();
    assert!(
        !modelled.is_empty(),
        "OPERATIONS.md must list this service's operations; found none"
    );
    assert_eq!(
        announced, modelled,
        "every operation the model binds must be fronted by the response envelope: \
         announced {announced:?}, modelled {modelled:?}"
    );
}

/// One refused-request case: the request, the member the model refuses it at, and the
/// requirement that member failed.
struct Refusal<'a> {
    case: &'a str,
    method: &'a str,
    uri: String,
    body: String,
    /// The member the refusal names, as the wire names it.
    member: &'a str,
    /// The requirement the member failed — the member's own modelled constraint, with a
    /// `@pattern` stated WITHOUT its anchors.
    requirement: String,
    /// A value the request carried that the refusal must not echo back.
    must_not_echo: Option<&'a str>,
}

/// A value outside a constraint the model declares is refused with the MODELLED
/// `ValidationException`: `reason` set, one `fieldList` entry per refused member, and a
/// `message` of `Invalid <member>: <requirement>`.
///
/// One row per constraint CLASS the model can state (`@pattern`, `@length` min and max on
/// a string, `@length` max on a map, `@range` min and max, `@required` absent) over four
/// operations and five members, since the rule is a property of the request boundary
/// rather than of any one operation. Every constraint cited is the one `smithy_query`
/// reports for that member.
#[tokio::test]
async fn a_refused_request_carries_the_modelled_validation_exception() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "validation").await;
    let long_name = "a".repeat(129);

    let cases = vec![
        // `CreateWorkspace$kmsKeyArn`: pattern
        // `^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+$`, length 20..2048 —
        // this value is 26 characters, so only the pattern is at fault.
        Refusal {
            case: "a kmsKeyArn outside its published pattern",
            method: "POST",
            uri: "/workspaces".to_string(),
            body: serde_json::json!({ "kmsKeyArn": "not-a-valid-kms-arn-at-all" }).to_string(),
            member: "kmsKeyArn",
            requirement: "Member must satisfy regular expression pattern: \
                          arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+"
                .to_string(),
            must_not_echo: Some("not-a-valid-kms-arn-at-all"),
        },
        // `CreateWorkspace$alias`: `WorkspaceAlias` is length 1..100, and carries no
        // pattern — one below the published minimum.
        Refusal {
            case: "an alias one below its published length bound",
            method: "POST",
            uri: "/workspaces".to_string(),
            body: serde_json::json!({ "alias": "" }).to_string(),
            member: "alias",
            requirement: "Member must have length greater than or equal to 1".to_string(),
            must_not_echo: None,
        },
        // `CreateRuleGroupsNamespace$name`: length 1..128 — one past the maximum.
        Refusal {
            case: "a rule groups namespace name one past its published length bound",
            method: "POST",
            uri: format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            body: serde_json::json!({ "name": long_name, "data": "Z3JvdXBzOiBbXQ==" })
                .to_string(),
            member: "name",
            requirement: "Member must have length less than or equal to 128".to_string(),
            must_not_echo: Some(&long_name),
        },
        // `CreateLoggingConfiguration$logGroupArn`: pattern only, and its anchors are
        // dropped INCLUDING the escaped `\:\*` the pattern ends with.
        Refusal {
            case: "a logGroupArn outside its published pattern",
            method: "POST",
            uri: format!("/workspaces/{workspace_id}/logging"),
            body: serde_json::json!({ "logGroupArn": "not-a-valid-arn" }).to_string(),
            member: "logGroupArn",
            requirement: "Member must satisfy regular expression pattern: \
                          arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:\
                          [A-Za-z0-9\\.\\-\\_\\#/]{1,512}\\:\\*"
                .to_string(),
            must_not_echo: Some("not-a-valid-arn"),
        },
        // `CreateLoggingConfiguration$logGroupArn` is @required: absence is refused as a
        // member too, and names the same member.
        Refusal {
            case: "an absent required logGroupArn",
            method: "POST",
            uri: format!("/workspaces/{workspace_id}/logging"),
            body: "{}".to_string(),
            member: "logGroupArn",
            requirement: "Member must not be null".to_string(),
            must_not_echo: None,
        },
        // `ListWorkspaces$maxResults`: `@range(min: 1, max: 1000)` — one below and one
        // past the published bounds.
        Refusal {
            case: "a maxResults one below its published range",
            method: "GET",
            uri: "/workspaces?maxResults=0".to_string(),
            body: String::new(),
            member: "maxResults",
            requirement: "Member must have value greater than or equal to 1".to_string(),
            must_not_echo: None,
        },
        Refusal {
            case: "a maxResults one past its published range",
            method: "GET",
            uri: "/workspaces?maxResults=1001".to_string(),
            body: String::new(),
            member: "maxResults",
            requirement: "Member must have value less than or equal to 1000".to_string(),
            must_not_echo: Some("1001"),
        },
        // `ListScrapers$filters`: `ScraperFilters` is a map of length 1..4, and `filters`
        // is bound `@httpQueryParams`, so five query keys are five entries — one past
        // the published maximum. A map's length is the entry count, so the refusal names
        // the map member, not an entry.
        Refusal {
            case: "one filter key past the published map length bound",
            method: "GET",
            uri: "/scrapers?status=ACTIVE&alias=a&sourceArn=b&destinationArn=c&fifth=d"
                .to_string(),
            body: String::new(),
            member: "filters",
            requirement: "Member must have length less than or equal to 4".to_string(),
            must_not_echo: None,
        },
    ];

    for case in cases {
        let reply = amp
            .call(case.method, &case.uri, region, &case.body)
            .await;
        assert_refused(&reply, &case);
    }
}

/// Every assertion one refused row makes.
fn assert_refused(reply: &Reply, case: &Refusal<'_>) {
    let label = case.case;
    assert_eq!(
        reply.status, 400,
        "case {label:?}: expected status 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ValidationException",
        "case {label:?}: expected the modelled ValidationException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["reason"]),
        VALIDATION_REASON,
        "case {label:?}: `reason` is @required and must name the reason aps reports, \
         got {reply:?}"
    );
    assert_eq!(
        reply.body["fieldList"],
        serde_json::json!([{ "name": case.member, "message": case.requirement }]),
        "case {label:?}: expected one fieldList entry naming {:?} and its requirement, \
         got {reply:?}",
        case.member
    );
    assert_eq!(
        text(&reply.body["message"]),
        format!("Invalid {}: {}", case.member, case.requirement),
        "case {label:?}: expected the message to name the member and its requirement, \
         got {reply:?}"
    );
    let rendered = reply.body.to_string();
    if let Some(sent) = case.must_not_echo {
        assert!(
            !rendered.contains(sent),
            "case {label:?}: the refusal must not echo the submitted value {sent:?}, \
             got {rendered}"
        );
    }
    assert!(
        !rendered.contains("1 validation error detected"),
        "case {label:?}: the framework's generic refusal envelope must not reach the \
         wire, got {rendered}"
    );
}

/// The published bounds themselves are ACCEPTED: the refusals above are about values
/// OUTSIDE a constraint, so each bound a row steps past is exercised at the bound too.
#[tokio::test]
async fn a_value_at_a_published_bound_is_accepted() {
    let region = "eu-west-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "bounds").await;

    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "maxResults at its published minimum",
            "GET",
            "/workspaces?maxResults=1".to_string(),
            String::new(),
        ),
        (
            "maxResults at its published maximum",
            "GET",
            "/workspaces?maxResults=1000".to_string(),
            String::new(),
        ),
        (
            "four filter keys, the published map length bound",
            "GET",
            "/scrapers?status=ACTIVE&alias=a&sourceArn=b&destinationArn=c".to_string(),
            String::new(),
        ),
        (
            "an alias at its published minimum length",
            "POST",
            "/workspaces".to_string(),
            serde_json::json!({ "alias": "a" }).to_string(),
        ),
        (
            "an alias at its published maximum length",
            "POST",
            "/workspaces".to_string(),
            serde_json::json!({ "alias": "a".repeat(100) }).to_string(),
        ),
        (
            "a kmsKeyArn inside its published pattern",
            "POST",
            "/workspaces".to_string(),
            serde_json::json!({ "alias": "kms", "kmsKeyArn": kms_key_arn(region) }).to_string(),
        ),
        (
            "a rule groups namespace name at its published maximum length",
            "POST",
            format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            serde_json::json!({ "name": "a".repeat(128), "data": "Z3JvdXBzOiBbXQ==" })
                .to_string(),
        ),
        (
            "a logGroupArn inside its published pattern",
            "POST",
            format!("/workspaces/{workspace_id}/logging"),
            serde_json::json!({ "logGroupArn": log_group_arn(region) }).to_string(),
        ),
    ];

    for (case, method, uri, body) in cases {
        let reply = amp.call(method, &uri, region, &body).await;
        assert!(
            reply.status < 400,
            "case {case:?}: a value at a published bound must be accepted, got {reply:?}"
        );
    }
}

/// A workspace no store holds is refused with ONE constant sentence, whichever operation
/// looked it up and whichever id it looked up: the message carries nothing from the
/// request, and the requested id is reported in `resourceId`.
#[tokio::test]
async fn a_missing_workspace_reports_one_constant_message() {
    let region = "us-east-2";
    let amp = Amp::new();
    let mut reported: Vec<(String, String)> = Vec::new();

    for absent in ABSENT_WORKSPACES {
        let cases: Vec<(&str, &str, String, String)> = vec![
            (
                "DescribeWorkspace",
                "GET",
                format!("/workspaces/{absent}"),
                String::new(),
            ),
            (
                "UpdateWorkspaceAlias",
                "POST",
                format!("/workspaces/{absent}/alias"),
                serde_json::json!({ "alias": "renamed" }).to_string(),
            ),
            (
                "DeleteWorkspace",
                "DELETE",
                format!("/workspaces/{absent}"),
                String::new(),
            ),
            (
                "DescribeWorkspaceConfiguration",
                "GET",
                format!("/workspaces/{absent}/configuration"),
                String::new(),
            ),
            (
                "UpdateWorkspaceConfiguration",
                "PATCH",
                format!("/workspaces/{absent}/configuration"),
                serde_json::json!({ "retentionPeriodInDays": 30 }).to_string(),
            ),
            (
                "DescribeLoggingConfiguration",
                "GET",
                format!("/workspaces/{absent}/logging"),
                String::new(),
            ),
            (
                "CreateLoggingConfiguration",
                "POST",
                format!("/workspaces/{absent}/logging"),
                serde_json::json!({ "logGroupArn": log_group_arn(region) }).to_string(),
            ),
            (
                "DescribeQueryLoggingConfiguration",
                "GET",
                format!("/workspaces/{absent}/logging/query"),
                String::new(),
            ),
            (
                "CreateQueryLoggingConfiguration",
                "POST",
                format!("/workspaces/{absent}/logging/query"),
                serde_json::json!({ "destinations": destinations_json(region) }).to_string(),
            ),
            (
                "UpdateQueryLoggingConfiguration",
                "PUT",
                format!("/workspaces/{absent}/logging/query"),
                serde_json::json!({ "destinations": destinations_json(region) }).to_string(),
            ),
            (
                "DeleteQueryLoggingConfiguration",
                "DELETE",
                format!("/workspaces/{absent}/logging/query"),
                String::new(),
            ),
            (
                "CreateRuleGroupsNamespace",
                "POST",
                format!("/workspaces/{absent}/rulegroupsnamespaces"),
                serde_json::json!({ "name": "rules", "data": "Z3JvdXBzOiBbXQ==" }).to_string(),
            ),
            (
                "DescribeRuleGroupsNamespace",
                "GET",
                format!("/workspaces/{absent}/rulegroupsnamespaces/rules"),
                String::new(),
            ),
            (
                "DescribeAnomalyDetector",
                "GET",
                format!(
                    "/workspaces/{absent}/anomalydetectors/\
                     ad-00000000-0000-0000-0000-000000000000"
                ),
                String::new(),
            ),
            (
                "DeleteAnomalyDetector",
                "DELETE",
                format!(
                    "/workspaces/{absent}/anomalydetectors/\
                     ad-00000000-0000-0000-0000-000000000000"
                ),
                String::new(),
            ),
            (
                "DescribeResourcePolicy",
                "GET",
                format!("/workspaces/{absent}/policy"),
                String::new(),
            ),
            (
                "PutResourcePolicy",
                "PUT",
                format!("/workspaces/{absent}/policy"),
                serde_json::json!({ "policyDocument": "{}" }).to_string(),
            ),
            (
                "DeleteResourcePolicy",
                "DELETE",
                format!("/workspaces/{absent}/policy"),
                String::new(),
            ),
        ];

        for (operation, method, uri, body) in cases {
            let case = format!("{operation} on {absent}");
            let reply = amp.call(method, &uri, region, &body).await;
            assert_missing(&reply, &case, WORKSPACE_NOT_FOUND_MESSAGE, absent);
            reported.push((case, text(&reply.body["message"])));
        }
    }

    let first = reported[0].1.clone();
    for (case, message) in &reported {
        assert_eq!(
            message, &first,
            "case {case:?}: every operation must report the same not-found message, \
             got {message:?} against {first:?}"
        );
    }
}

/// The same rule for the resources a workspace CONTAINS: one constant sentence per kind
/// of missing resource, with the identifier in `resourceId`.
#[tokio::test]
async fn a_missing_subresource_reports_one_constant_message() {
    let region = "ap-south-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "subresources").await;
    let unknown_arn = format!(
        "arn:aws:aps:{region}:{}:workspace/ws-11111111-1111-1111-1111-111111111111",
        engine::emulator_account_id()
    );

    let cases: Vec<(&str, &str, String, String, &str, String)> = vec![
        (
            "DescribeLoggingConfiguration on a workspace with no logging configuration",
            "GET",
            format!("/workspaces/{workspace_id}/logging"),
            String::new(),
            LOGGING_NOT_FOUND_MESSAGE,
            workspace_id.clone(),
        ),
        (
            "DescribeQueryLoggingConfiguration on a workspace with none",
            "GET",
            format!("/workspaces/{workspace_id}/logging/query"),
            String::new(),
            QUERY_LOGGING_NOT_FOUND_MESSAGE,
            workspace_id.clone(),
        ),
        (
            "DescribeRuleGroupsNamespace for a name the workspace does not hold",
            "GET",
            format!("/workspaces/{workspace_id}/rulegroupsnamespaces/absent"),
            String::new(),
            RULE_GROUPS_NAMESPACE_NOT_FOUND_MESSAGE,
            "absent".to_string(),
        ),
        (
            "DescribeResourcePolicy on a workspace that holds no policy",
            "GET",
            format!("/workspaces/{workspace_id}/policy"),
            String::new(),
            POLICY_NOT_FOUND_MESSAGE,
            workspace_id.clone(),
        ),
        (
            "DeleteResourcePolicy on a workspace that holds no policy",
            "DELETE",
            format!("/workspaces/{workspace_id}/policy"),
            String::new(),
            POLICY_NOT_FOUND_MESSAGE,
            workspace_id.clone(),
        ),
        (
            "ListTagsForResource for an ARN no workspace carries",
            "GET",
            format!("/tags/{}", encode_label(&unknown_arn)),
            String::new(),
            RESOURCE_NOT_FOUND_MESSAGE,
            unknown_arn.clone(),
        ),
        (
            "TagResource for an ARN no workspace carries",
            "POST",
            format!("/tags/{}", encode_label(&unknown_arn)),
            serde_json::json!({ "tags": { "team": "metrics" } }).to_string(),
            RESOURCE_NOT_FOUND_MESSAGE,
            unknown_arn.clone(),
        ),
    ];

    for (case, method, uri, body, expected, resource_id) in cases {
        let reply = amp.call(method, &uri, region, &body).await;
        assert_missing(&reply, case, expected, &resource_id);
    }
}

/// Every assertion a not-found row makes: the modelled 404, the constant message, the
/// requested identifier in `resourceId` — and nowhere in the message.
fn assert_missing(reply: &Reply, case: &str, expected: &str, resource_id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected status 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
    );
    let message = text(&reply.body["message"]);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the constant message {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(resource_id),
        "case {case:?}: the message must not carry the requested identifier \
         {resource_id:?}, got {message:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        resource_id,
        "case {case:?}: the identifier belongs in `resourceId`, got {reply:?}"
    );
    assert!(
        !text(&reply.body["resourceType"]).is_empty(),
        "case {case:?}: `resourceType` is @required, got {reply:?}"
    );
}
