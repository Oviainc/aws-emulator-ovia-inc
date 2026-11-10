// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN this API evaluates an input constraint, and how it words the
//! refusal of one the boundary walk cannot reach.
//!
//! The rule: an input constraint is evaluated BEFORE the request's resources are resolved,
//! so a value outside a member's declared syntax is refused with `400 ValidationException`
//! naming that member — never reported as a resource that does not exist. Two classes of
//! constraint reach a handler rather than the boundary in front of it, and both are
//! asserted here:
//!
//! * a member whose string shape in THIS model declares no syntax while the resource it
//!   addresses declares one. `PackageID` here is a bare string
//!   (`smithy_query(query="shape",shape="PackageID")` on this model), while the shared
//!   package resource states the id's syntax
//!   (`smithy_query(query="shape",shape="PackageID")` on the modern OpenSearch model:
//!   `@pattern("^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$")`), and the `/2015-01-01/packages`
//!   routes address that one resource.
//! * a MAP whose KEY shape is an enumerated type. The validating walk treats a map key as
//!   a path segment rather than as a constrained leaf, so nothing ahead of the handler
//!   checks it: `LogPublishingOptions` is keyed by `LogType`, whose value set is
//!   `smithy_query(query="shape",shape="LogType,LogPublishingOptions")`.
//!
//! Each table spans what the rule generalises over: the form the offending value takes,
//! where it is bound (a URI label, a body member, a map key), the operation that carries
//! it, and whether the resource the call names exists. The control rows are the other half
//! of the rule: a value the syntax ADMITS is not refused here, so the resource lookup
//! still runs and still answers its own refusal.

use serde_json::{json, Value};

use crate::harness::{call_in, router, Answer};

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// The error class a refused input carries, and the one an absent resource carries.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The status a refused input is answered with, and the one an absent resource is.
const BAD_REQUEST: u16 = 400;
const NOT_FOUND_STATUS: u16 = 404;

/// The expression a package id has to satisfy, as the shared package resource declares it
/// — quoted as the model declares it, because its own branches are anchored
/// (`tests/all/pattern_refusal_quoting.rs` is the rule for that rendering).
const PACKAGE_ID_EXPRESSION: &str = r"^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$";

/// The log types this surface admits, in the order its refusal states them — the order the
/// front door's own hash table over those values is walked, which is neither the model's
/// declaration order nor alphabetical.
///
/// `TASK_DETAILS_LOGS` is NOT in it: the 2021-01-01 sibling surface's own recorded refusal
/// lists that value beside these four while this surface's recorded refusal lists only
/// these, so the two surfaces publish different sets and the row below sends it expecting a
/// refusal here.
const LOG_TYPE_SET: &str = "[INDEX_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS, SEARCH_SLOW_LOGS]";

/// A log group a request names when it enables a log type. Any ARN the member admits does,
/// so the rule is asserted over the one the request itself carried.
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/es/probe";

/// One operation that names a package by id, and where the id is bound.
struct PackageCall {
    operation: &'static str,
    method: &'static str,
    /// The path for `package_id`, which is `None` for the one route that binds the id in
    /// the body.
    path: fn(&str) -> String,
    /// The body for `package_id`, for the route that binds it there.
    body: fn(&str) -> Option<Value>,
}

/// Every implemented operation that resolves a package by id: three bind it to a URI label
/// and one to a body member, so the rule is asserted over both bindings.
fn package_calls() -> Vec<PackageCall> {
    vec![
        PackageCall {
            operation: "GetPackageVersionHistory",
            method: "GET",
            path: |id| format!("/2015-01-01/packages/{id}/history"),
            body: |_| None,
        },
        PackageCall {
            operation: "ListDomainsForPackage",
            method: "GET",
            path: |id| format!("/2015-01-01/packages/{id}/domains"),
            body: |_| None,
        },
        PackageCall {
            operation: "AssociatePackage",
            method: "POST",
            path: |id| format!("/2015-01-01/packages/associate/{id}/probe-domain"),
            body: |_| None,
        },
        PackageCall {
            operation: "UpdatePackage",
            method: "POST",
            path: |_| "/2015-01-01/packages/update".to_string(),
            body: |id| {
                Some(json!({
                    "PackageID": id,
                    "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v2.txt"}
                }))
            },
        },
    ]
}

/// Package ids the shared resource's syntax does NOT admit, spanning the ways a value can
/// fall outside it: the wrong alphabet, the wrong case, a prefix with nothing after it, a
/// published-id letter with no digits, digits with no prefix at all.
fn malformed_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a name that is no id at all", "not-a-valid-id"),
        ("an uppercase prefix", "PKG-0000000000000000"),
        ("the prefix with no hex run", "pkg-"),
        ("a hex run that is not hex", "pkg-00zz"),
        ("an uppercase hex run", "pkg-00AB"),
        ("a published-id letter with no digits", "F"),
        ("a published-id letter with a separator", "G-12"),
        ("digits with no prefix", "12345"),
    ]
}

/// Package ids the syntax ADMITS, spanning both branches of the expression: the
/// published-package form (`F`/`G` with digits) and an account's own `pkg-` form.
fn well_formed_absent_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a published-package id on F", "F123"),
        ("a published-package id on G", "G7"),
        ("a short account id", "pkg-abc123"),
        ("an id nothing minted", "pkg-0000000000000000"),
    ]
}

/// One call of `call` for `package_id`.
async fn call_package(
    router: &smithy_server_runtime::MultiServiceRouter,
    call: &PackageCall,
    package_id: &str,
) -> Answer {
    let body = (call.body)(package_id);
    call_in(
        router,
        REGION,
        call.method,
        &(call.path)(package_id),
        body.as_ref(),
    )
    .await
}

/// RULE: a package id outside the syntax the shared package resource declares is refused
/// as invalid INPUT — the member, the submitted value and the expression — on every
/// operation that names a package, before the id is resolved against any state.
#[tokio::test]
async fn a_malformed_package_id_is_refused_as_invalid_input_before_any_lookup() {
    let router = router();

    for call in package_calls() {
        for (label, package_id) in malformed_ids() {
            let case = format!("{} with {label} ({package_id:?})", call.operation);
            let answer = call_package(&router, &call, package_id).await;
            assert_eq!(
                answer.status, BAD_REQUEST,
                "case {case:?}: a malformed id must be refused with HTTP {BAD_REQUEST}, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains(VALIDATION),
                "case {case:?}: the error must be {VALIDATION}, got {:?} / {}",
                answer.error_type,
                answer.body
            );
            let message = answer.text(&case, "/message");
            let expected = format!(
                "1 validation error detected: Value '{package_id}' at 'packageID' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {PACKAGE_ID_EXPRESSION}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// The other half of the rule: a well-formed id passes the constraint, so the LOOKUP runs
/// and answers its own refusal — the ordering is a precedence, not a replacement.
#[tokio::test]
async fn a_well_formed_but_absent_package_id_still_answers_not_found() {
    let router = router();

    for call in package_calls() {
        for (label, package_id) in well_formed_absent_ids() {
            let case = format!("{} with {label} ({package_id:?})", call.operation);
            let answer = call_package(&router, &call, package_id).await;
            assert_eq!(
                answer.status, NOT_FOUND_STATUS,
                "case {case:?}: an absent package must answer HTTP {NOT_FOUND_STATUS}, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains(NOT_FOUND),
                "case {case:?}: the error must be {NOT_FOUND}, got {:?} / {}",
                answer.error_type,
                answer.body
            );
            let message = answer.text(&case, "/message");
            assert!(
                message.contains(package_id),
                "case {case:?}: the refusal must name the id it could not resolve, got \
                 {message:?}"
            );
        }
    }
}

/// A package this account created, so the id the ADMITTED-syntax rows resolve with is one
/// the service itself minted.
async fn create_package(
    router: &smithy_server_runtime::MultiServiceRouter,
    package_name: &str,
) -> String {
    let created = call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/packages",
        Some(&json!({
            "PackageName": package_name,
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v1.txt"}
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "creating {package_name:?} must succeed, got {} {}",
        created.status, created.body
    );
    created.text(package_name, "/PackageDetails/PackageID")
}

/// The ids this service MINTS satisfy the syntax it enforces, so a create's own id is
/// accepted by the reads that name it: the check cannot refuse the service's own output.
#[tokio::test]
async fn a_minted_package_id_satisfies_the_syntax_the_reads_enforce() {
    let router = router();
    let package_id = create_package(&router, "minted-probe").await;

    let history = call_in(
        &router,
        REGION,
        "GET",
        &format!("/2015-01-01/packages/{package_id}/history"),
        None,
    )
    .await;
    assert_eq!(
        history.status, 200,
        "case {package_id:?}: a minted id must read back, got {} {}",
        history.status, history.body
    );
    assert_eq!(
        history.at(&package_id, "/PackageID"),
        &json!(package_id),
        "case {package_id:?}: the read must report the id it was called for, got {}",
        history.body
    );
}

/// One row of the map-key table: the log-publishing map a request carries, and how the
/// refusal quotes it back.
struct LogCase {
    case: &'static str,
    /// The `LogPublishingOptions` member as the request sends it.
    submitted: Value,
    /// The map as the refusal quotes it: `{<key>=<value>, …}` over the entries in key
    /// order, each value the printed form of the structure it carried.
    quoted: &'static str,
}

/// The rows span what the rule generalises over: which key is outside the set (a log type
/// of the sibling surface, a misspelling, the wrong case, an empty key), how many entries
/// the map carries, and which members of the entry's structure the request set.
fn log_cases() -> Vec<LogCase> {
    vec![
        LogCase {
            case: "a log type no surface declares",
            submitted: json!({
                "ACCESS_LOGS": {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": true}
            }),
            quoted: "{ACCESS_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=arn:aws:logs:\
                     us-east-1:000000000000:log-group:/aws/es/probe, enabled=true)}",
        },
        LogCase {
            case: "the log type only the 2021-01-01 surface admits",
            submitted: json!({
                "TASK_DETAILS_LOGS": {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": true}
            }),
            quoted: "{TASK_DETAILS_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=arn:aws:\
                     logs:us-east-1:000000000000:log-group:/aws/es/probe, enabled=true)}",
        },
        LogCase {
            case: "a modelled log type in the wrong case",
            submitted: json!({
                "index_slow_logs": {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": false}
            }),
            quoted: "{index_slow_logs=LogPublishingOption(cloudWatchLogsLogGroupArn=arn:aws:logs:\
                     us-east-1:000000000000:log-group:/aws/es/probe, enabled=false)}",
        },
        LogCase {
            case: "an entry whose log group the request left unset",
            submitted: json!({"AUDIT_LOG": {"Enabled": true}}),
            quoted: "{AUDIT_LOG=LogPublishingOption(cloudWatchLogsLogGroupArn=null, \
                     enabled=true)}",
        },
        LogCase {
            case: "an empty key",
            submitted: json!({"": {"Enabled": true}}),
            quoted: "{=LogPublishingOption(cloudWatchLogsLogGroupArn=null, enabled=true)}",
        },
        LogCase {
            case: "one key inside the set and one outside it",
            submitted: json!({
                "AUDIT_LOGS": {"Enabled": true},
                "ACCESS_LOGS": {"Enabled": false}
            }),
            quoted: "{ACCESS_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=null, \
                     enabled=false), AUDIT_LOGS=LogPublishingOption(\
                     cloudWatchLogsLogGroupArn=null, enabled=true)}",
        },
    ]
}

/// The whole message a map-key refusal states for `quoted`.
fn expected_map_key_message(quoted: &str) -> String {
    format!(
        "1 validation error detected: Value '{quoted}' at 'logPublishingOptions' failed to \
         satisfy constraint: Map keys must satisfy constraint: [Member must satisfy enum value \
         set: {LOG_TYPE_SET}]"
    )
}

/// RULE: a `LogPublishingOptions` key outside the modelled `LogType` set is refused as
/// invalid INPUT — the member, the submitted map and the accepted key set — before the
/// domain the call names is resolved, on both operations that carry the member.
#[tokio::test]
async fn a_log_type_outside_the_modelled_set_is_refused_before_the_domain_is_resolved() {
    let router = router();

    for LogCase {
        case,
        submitted,
        quoted,
    } in log_cases()
    {
        // The UPDATE route names a domain that does not exist, which is what makes the
        // ordering observable: the recorded answer is the input refusal, not a not-found.
        let calls = [
            (
                "UpdateElasticsearchDomainConfig on an absent domain",
                "POST",
                "/2015-01-01/es/domain/no-such-domain/config".to_string(),
                json!({"LogPublishingOptions": submitted}),
            ),
            (
                "CreateElasticsearchDomain",
                "POST",
                "/2015-01-01/es/domain".to_string(),
                json!({"DomainName": "log-probe", "LogPublishingOptions": submitted}),
            ),
        ];
        for (operation, method, path, body) in calls {
            let row = format!("{operation}: {case}");
            let answer = call_in(&router, REGION, method, &path, Some(&body)).await;
            assert_eq!(
                answer.status, BAD_REQUEST,
                "case {row:?}: an out-of-enum key must be refused with HTTP {BAD_REQUEST}, got \
                 {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains(VALIDATION),
                "case {row:?}: the error must be {VALIDATION}, got {:?} / {}",
                answer.error_type,
                answer.body
            );
            let message = answer.text(&row, "/message");
            let expected = expected_map_key_message(quoted);
            assert_eq!(
                message, expected,
                "case {row:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // Nothing was stored by any refused create: the refusal ran before the store was
    // reached.
    let read = call_in(&router, REGION, "GET", "/2015-01-01/es/domain/log-probe", None).await;
    assert_eq!(
        read.status, NOT_FOUND_STATUS,
        "a refused create must store no domain, got {} {}",
        read.status, read.body
    );
}

/// The other half of the rule, on the map-key class: every log type the model DOES declare
/// is accepted, stored and read back, and a valid map on an absent domain still answers
/// the domain's own refusal.
#[tokio::test]
async fn every_modelled_log_type_is_accepted_and_read_back() {
    let router = router();

    let created = call_in(
        &router,
        REGION,
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({"DomainName": "log-types-domain"})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "creating the domain must succeed, got {} {}",
        created.status, created.body
    );

    // Every value of the key enum, read with
    // `smithy_query(query="shape",shape="LogType")`.
    for log_type in [
        "INDEX_SLOW_LOGS",
        "SEARCH_SLOW_LOGS",
        "ES_APPLICATION_LOGS",
        "AUDIT_LOGS",
    ] {
        let updated = call_in(
            &router,
            REGION,
            "POST",
            "/2015-01-01/es/domain/log-types-domain/config",
            Some(&json!({
                "LogPublishingOptions": {
                    log_type: {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": true}
                }
            })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {log_type:?}: a modelled log type must be accepted, got {} {}",
            updated.status, updated.body
        );

        let read = call_in(
            &router,
            REGION,
            "GET",
            "/2015-01-01/es/domain/log-types-domain/config",
            None,
        )
        .await;
        let pointer = format!("/DomainConfig/LogPublishingOptions/Options/{log_type}/Enabled");
        assert_eq!(
            read.at(log_type, &pointer),
            &json!(true),
            "case {log_type:?}: the stored option must read back at {pointer}, got {}",
            read.body
        );
    }

    // A map every key of which is inside the set leaves the domain lookup to answer.
    let absent = call_in(
        &router,
        REGION,
        "POST",
        "/2015-01-01/es/domain/no-such-domain/config",
        Some(&json!({
            "LogPublishingOptions": {"AUDIT_LOGS": {"Enabled": true}}
        })),
    )
    .await;
    assert_eq!(
        absent.status, NOT_FOUND_STATUS,
        "a valid map on an absent domain must answer HTTP {NOT_FOUND_STATUS}, got {} {}",
        absent.status, absent.body
    );
    assert!(
        absent.error_type.contains(NOT_FOUND),
        "a valid map on an absent domain must answer {NOT_FOUND}, got {:?} / {}",
        absent.error_type,
        absent.body
    );
}
