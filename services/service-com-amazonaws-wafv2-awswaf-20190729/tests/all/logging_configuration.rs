// SPDX-License-Identifier: CC0-1.0

//! A web ACL's logging configuration, end to end over the wire.
//!
//! The rule these suites assert is: a logging configuration is an association
//! between a web ACL and its logging destinations — "defines an association between
//! logging destinations and a web ACL resource", with "one logging destination per
//! web ACL" and a `LogScope` saying who owns it — so a put REPLACES what the web ACL
//! its `ResourceArn` names carried under that log scope ("this operation completely
//! replaces any mutable specifications that you already have"),
//! `GetLoggingConfiguration` answers exactly that stored configuration,
//! `DeleteLoggingConfiguration` takes exactly that one away, and an address that
//! holds no such configuration is refused with 400 `WAFNonexistentItemException`
//! while an address that is not a wafv2 ARN at all is the parameter failure.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in the web ACL's scope, in how many destinations the
//! configuration carries, in whether it carries a logging filter, in its `LogScope`,
//! and — for the refusals — in HOW the address fails to name a stored configuration.
//! The harness is the one in [`crate::web_acl_lifecycle`]: every call goes through
//! the router's front door, so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{create_web_acl, Waf};

/// A string long enough to satisfy `ResourceArn`'s `@length(20..2048)` that is not
/// an ARN at all, so the modelled constraints admit it and the handler is what
/// refuses it.
const MALFORMED_ARN: &str = "this-is-not-an-arn-but-it-is-long-enough";

/// A well-formed wafv2 web ACL ARN that no create minted.
const MISSING_WEB_ACL_ARN: &str = "arn:aws:wafv2:us-west-2:000000000000:regional/\
     webacl/probe-absent-acl/00000000-1111-2222-3333-444444444444";

/// The `LogType` a configuration that names none carries ("Default: `WAF_LOGS`", the
/// member's own doc).
const DEFAULT_LOG_TYPE: &str = "WAF_LOGS";

/// The `LogScope` a configuration that names none carries ("Default: `CUSTOMER`").
const DEFAULT_LOG_SCOPE: &str = "CUSTOMER";

/// One configuration to put and read back.
struct ConfigurationCase {
    case: &'static str,
    scope: &'static str,
    /// The logging destinations, as `LogDestinationConfigs`.
    destinations: Vec<&'static str>,
    /// Whether the configuration carries a `LoggingFilter`.
    filtered: bool,
    /// The `LogScope` the configuration declares, or `None` to leave the member out.
    log_scope: Option<&'static str>,
}

/// The rows: both scopes, one and several destinations, with and without a filter,
/// and the `LogScope` left out, named explicitly, and set to another owner.
fn configuration_cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            case: "one destination, no filter, no log scope named",
            scope: "REGIONAL",
            destinations: vec![
                "arn:aws:firehose:us-west-2:000000000000:deliverystream/aws-waf-logs-one",
            ],
            filtered: false,
            log_scope: None,
        },
        ConfigurationCase {
            case: "several destinations and a filter, CLOUDFRONT scope",
            scope: "CLOUDFRONT",
            destinations: vec![
                "arn:aws:logs:us-east-1:000000000000:log-group:aws-waf-logs-two",
                "arn:aws:s3:::aws-waf-logs-bucket",
            ],
            filtered: true,
            log_scope: Some("CUSTOMER"),
        },
        ConfigurationCase {
            case: "a configuration owned by Security Lake",
            scope: "REGIONAL",
            destinations: vec!["arn:aws:s3:::aws-waf-logs-security-lake"],
            filtered: false,
            log_scope: Some("SECURITY_LAKE"),
        },
    ]
}

/// A `LoggingFilter` that keeps the logs of blocked requests and drops the rest.
///
/// `LoggingFilter$Filters` and `Filter$Conditions` are both `@length(min 1)`, so a
/// filter carries at least one of each.
fn logging_filter() -> Value {
    json!({
        "Filters": [{
            "Behavior": "KEEP",
            "Requirement": "MEETS_ALL",
            "Conditions": [{ "ActionCondition": { "Action": "BLOCK" } }],
        }],
        "DefaultBehavior": "DROP",
    })
}

/// The `LoggingConfiguration` a row puts in front of `resource_arn`.
fn configuration(row: &ConfigurationCase, resource_arn: &str) -> Value {
    let mut built = json!({
        "ResourceArn": resource_arn,
        "LogDestinationConfigs": row.destinations,
    });
    if row.filtered {
        built["LoggingFilter"] = logging_filter();
    }
    if let Some(log_scope) = row.log_scope {
        built["LogScope"] = json!(log_scope);
    }
    built
}

/// A `GetLoggingConfiguration` / `DeleteLoggingConfiguration` request for
/// `resource_arn`, addressing `log_scope`.
fn request(resource_arn: &str, log_scope: Option<&str>) -> Value {
    let mut body = json!({ "ResourceArn": resource_arn });
    if let Some(log_scope) = log_scope {
        body["LogScope"] = json!(log_scope);
    }
    body
}

/// The whole lifecycle: absent, put, read back, delete, absent again.
#[tokio::test]
async fn a_logging_configuration_is_put_read_back_and_deleted() {
    for row in configuration_cases() {
        let waf = Waf::new();
        let (_acl_id, acl_arn, _acl_lock) =
            create_web_acl(&waf, row.scope, "logged-acl", json!([])).await;

        // Before anything is put in front of the web ACL it has no logging
        // configuration, which is the absent case a client branches on.
        waf.refused(
            "GetLoggingConfiguration",
            request(&acl_arn, row.log_scope),
            "WAFNonexistentItemException",
            row.case,
        )
        .await;
        waf.refused(
            "DeleteLoggingConfiguration",
            request(&acl_arn, row.log_scope),
            "WAFNonexistentItemException",
            row.case,
        )
        .await;

        let put = waf
            .ok(
                "PutLoggingConfiguration",
                json!({ "LoggingConfiguration": configuration(&row, &acl_arn) }),
            )
            .await;
        let answered = put["LoggingConfiguration"].clone();
        assert_eq!(
            answered["ResourceArn"],
            json!(acl_arn),
            "case {:?}: the configuration must name the web ACL it belongs to, got {put}",
            row.case
        );
        assert_eq!(
            answered["LogDestinationConfigs"],
            json!(row.destinations),
            "case {:?}: the destinations must be stored as given, got {put}",
            row.case
        );
        assert_eq!(
            answered["LogType"],
            json!(DEFAULT_LOG_TYPE),
            "case {:?}: the configuration must carry its log type, got {put}",
            row.case
        );
        assert_eq!(
            answered["LogScope"],
            json!(row.log_scope.unwrap_or(DEFAULT_LOG_SCOPE)),
            "case {:?}: the configuration must carry its log scope, got {put}",
            row.case
        );
        assert_eq!(
            answered["LoggingFilter"].is_null(),
            !row.filtered,
            "case {:?}: the filter must be stored as given, got {put}",
            row.case
        );

        // The read answers exactly what the put stored.
        let got = waf
            .ok("GetLoggingConfiguration", request(&acl_arn, row.log_scope))
            .await;
        assert_eq!(
            got["LoggingConfiguration"], answered,
            "case {:?}: the read must answer the stored configuration, got {got}",
            row.case
        );
        // The read changed nothing a later caller sees.
        let again = waf
            .ok("GetLoggingConfiguration", request(&acl_arn, row.log_scope))
            .await;
        assert_eq!(
            again, got,
            "case {:?}: reading a logging configuration must not change it, got {again}",
            row.case
        );

        // A put REPLACES: the same web ACL, one destination, and the previous
        // specification is gone rather than merged with.
        let replacement = "arn:aws:s3:::aws-waf-logs-replacement";
        let mut second = configuration(&row, &acl_arn);
        second["LogDestinationConfigs"] = json!([replacement]);
        second
            .as_object_mut()
            .expect("a configuration object")
            .remove("LoggingFilter");
        waf.ok(
            "PutLoggingConfiguration",
            json!({ "LoggingConfiguration": second }),
        )
        .await;
        let replaced = waf
            .ok("GetLoggingConfiguration", request(&acl_arn, row.log_scope))
            .await;
        assert_eq!(
            replaced["LoggingConfiguration"]["LogDestinationConfigs"],
            json!([replacement]),
            "case {:?}: a put must replace the destinations, got {replaced}",
            row.case
        );
        assert!(
            replaced["LoggingConfiguration"]["LoggingFilter"].is_null(),
            "case {:?}: a put must replace the whole specification, got {replaced}",
            row.case
        );

        // A configuration belongs to ONE log scope, so another scope's address names
        // nothing — whichever scope the row stored.
        let other_scope = if row.log_scope == Some("SECURITY_LAKE") {
            DEFAULT_LOG_SCOPE
        } else {
            "SECURITY_LAKE"
        };
        waf.refused(
            "GetLoggingConfiguration",
            request(&acl_arn, Some(other_scope)),
            "WAFNonexistentItemException",
            row.case,
        )
        .await;

        waf.ok(
            "DeleteLoggingConfiguration",
            request(&acl_arn, row.log_scope),
        )
        .await;
        // Deleted: the web ACL has no configuration again, and the delete is not
        // idempotent — a second one is refused the way the first read was.
        waf.refused(
            "GetLoggingConfiguration",
            request(&acl_arn, row.log_scope),
            "WAFNonexistentItemException",
            row.case,
        )
        .await;
        waf.refused(
            "DeleteLoggingConfiguration",
            request(&acl_arn, row.log_scope),
            "WAFNonexistentItemException",
            row.case,
        )
        .await;
    }
}

/// `ListLoggingConfigurations` enumerates the configurations of the web ACLs in one
/// scope, under one log scope, a page at a time.
#[tokio::test]
async fn list_logging_configurations_enumerates_one_scope_a_page_at_a_time() {
    let waf = Waf::new();
    let row = &configuration_cases()[0];

    // An account whose web ACLs log nowhere answers an empty list and no marker.
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let empty = waf
            .ok("ListLoggingConfigurations", json!({ "Scope": scope }))
            .await;
        assert_eq!(
            empty["LoggingConfigurations"],
            json!([]),
            "case {scope}: a scope with no logging configuration must answer an empty \
             list, got {empty}"
        );
        assert!(
            empty["NextMarker"].is_null(),
            "case {scope}: an empty page must carry no marker, got {empty}"
        );
    }

    // Three logged REGIONAL web ACLs, one logged CLOUDFRONT one, and one REGIONAL
    // web ACL that logs nowhere.
    let mut regional: Vec<String> = Vec::new();
    for name in ["acl-a", "acl-b", "acl-c"] {
        let (_id, arn, _lock) = create_web_acl(&waf, "REGIONAL", name, json!([])).await;
        waf.ok(
            "PutLoggingConfiguration",
            json!({ "LoggingConfiguration": configuration(row, &arn) }),
        )
        .await;
        regional.push(arn);
    }
    regional.sort();
    let (_global_id, global_arn, _global_lock) =
        create_web_acl(&waf, "CLOUDFRONT", "acl-global", json!([])).await;
    waf.ok(
        "PutLoggingConfiguration",
        json!({ "LoggingConfiguration": configuration(row, &global_arn) }),
    )
    .await;
    create_web_acl(&waf, "REGIONAL", "acl-unlogged", json!([])).await;

    let listed = waf
        .ok("ListLoggingConfigurations", json!({ "Scope": "REGIONAL" }))
        .await;
    let addressed: Vec<&str> = listed["LoggingConfigurations"]
        .as_array()
        .expect("LoggingConfigurations")
        .iter()
        .map(|configuration| configuration["ResourceArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        addressed, regional,
        "the listing must enumerate only this scope's configurations, in a stable \
         order, got {listed}"
    );

    // Another log scope owns none of them.
    let security_lake = waf
        .ok(
            "ListLoggingConfigurations",
            json!({ "Scope": "REGIONAL", "LogScope": "SECURITY_LAKE" }),
        )
        .await;
    assert_eq!(
        security_lake["LoggingConfigurations"],
        json!([]),
        "a listing of another log scope must enumerate none of them, got {security_lake}"
    );

    // Pages hold at most `Limit` entries and resume from the marker.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "Scope": "REGIONAL", "Limit": 2 });
        if let Some(marker) = marker.as_deref() {
            request["NextMarker"] = json!(marker);
        }
        let page = waf.ok("ListLoggingConfigurations", request).await;
        let entries = page["LoggingConfigurations"]
            .as_array()
            .expect("LoggingConfigurations")
            .clone();
        assert!(
            entries.len() <= 2,
            "a page must hold at most Limit configurations, got {page}"
        );
        if entries.is_empty() {
            assert!(
                page["NextMarker"].is_null(),
                "the terminal page must carry no marker, got {page}"
            );
            break;
        }
        seen.extend(
            entries
                .iter()
                .map(|entry| entry["ResourceArn"].as_str().unwrap_or_default().to_string()),
        );
        marker = page["NextMarker"].as_str().map(str::to_string);
        assert!(
            marker.is_some(),
            "a non-empty page must carry a marker, got {page}"
        );
    }
    assert_eq!(
        seen, regional,
        "paging must enumerate every configuration exactly once, got {seen:?}"
    );

    // A deleted configuration leaves the listing.
    waf.ok("DeleteLoggingConfiguration", request(&regional[0], None))
        .await;
    let after = waf
        .ok("ListLoggingConfigurations", json!({ "Scope": "REGIONAL" }))
        .await;
    let addressed: Vec<&str> = after["LoggingConfigurations"]
        .as_array()
        .expect("LoggingConfigurations")
        .iter()
        .map(|configuration| configuration["ResourceArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        addressed,
        regional[1..],
        "a deleted configuration must leave the listing, got {after}"
    );
}

/// A logging configuration belongs to its web ACL, so deleting the web ACL takes
/// the configuration with it: no reader can still answer it, and a web ACL minted
/// afterwards under the same name starts with none.
#[tokio::test]
async fn deleting_a_web_acl_takes_its_logging_configuration_with_it() {
    let waf = Waf::new();
    let row = &configuration_cases()[0];
    let name = "short-lived-acl";
    let (acl_id, acl_arn, acl_lock) = create_web_acl(&waf, "REGIONAL", name, json!([])).await;
    waf.ok(
        "PutLoggingConfiguration",
        json!({ "LoggingConfiguration": configuration(row, &acl_arn) }),
    )
    .await;

    waf.ok(
        "DeleteWebACL",
        json!({
            "Name": name,
            "Scope": "REGIONAL",
            "Id": &acl_id,
            "LockToken": &acl_lock,
        }),
    )
    .await;

    // Both readers agree the configuration is gone.
    waf.refused(
        "GetLoggingConfiguration",
        request(&acl_arn, None),
        "WAFNonexistentItemException",
        "the configuration of a deleted web ACL",
    )
    .await;
    let listed = waf
        .ok("ListLoggingConfigurations", json!({ "Scope": "REGIONAL" }))
        .await;
    assert_eq!(
        listed["LoggingConfigurations"],
        json!([]),
        "a deleted web ACL must leave no configuration behind, got {listed}"
    );

    // A fresh web ACL of the same name logs nowhere until something puts a
    // configuration in front of it.
    let (_new_id, new_arn, _new_lock) = create_web_acl(&waf, "REGIONAL", name, json!([])).await;
    waf.refused(
        "GetLoggingConfiguration",
        request(&new_arn, None),
        "WAFNonexistentItemException",
        "a web ACL re-created under the same name",
    )
    .await;
}

/// An address that holds no logging configuration is refused, and WAF separates the
/// two ARN failures the model names: an ARN that is malformed is the parameter
/// failure, a well-formed one naming nothing is the nonexistent-item failure.
#[tokio::test]
async fn an_address_that_holds_no_configuration_is_refused() {
    let waf = Waf::new();
    let (_acl_id, acl_arn, _acl_lock) =
        create_web_acl(&waf, "REGIONAL", "logged-acl", json!([])).await;
    let (_other_id, other_arn, _other_lock) =
        create_web_acl(&waf, "REGIONAL", "unlogged-acl", json!([])).await;
    let row = &configuration_cases()[0];
    waf.ok(
        "PutLoggingConfiguration",
        json!({ "LoggingConfiguration": configuration(row, &acl_arn) }),
    )
    .await;

    // Each way an address can fail to name a stored configuration, with the error
    // class WAF answers for it. A put is refused for the same reasons a read is,
    // except that a live web ACL with no configuration yet is exactly what a put is
    // for — so it is not in the put's table.
    let cases: &[(&str, &str, &str)] = &[
        (
            "not an ARN at all",
            MALFORMED_ARN,
            "WAFInvalidParameterException",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::a-bucket-named-like-this",
            "WAFInvalidParameterException",
        ),
        (
            "a well-formed web ACL ARN naming nothing",
            MISSING_WEB_ACL_ARN,
            "WAFNonexistentItemException",
        ),
        (
            "a live web ACL that carries no configuration",
            other_arn.as_str(),
            "WAFNonexistentItemException",
        ),
    ];

    for (case, arn, code) in cases {
        for operation in ["GetLoggingConfiguration", "DeleteLoggingConfiguration"] {
            waf.refused(operation, request(arn, None), code, case).await;
        }
        if *arn == other_arn.as_str() {
            continue;
        }
        let mut configuration = configuration(row, arn);
        configuration["ResourceArn"] = json!(arn);
        waf.refused(
            "PutLoggingConfiguration",
            json!({ "LoggingConfiguration": configuration }),
            code,
            case,
        )
        .await;
    }

    // None of those refusals touched the configuration that IS stored, and none of
    // them put one in front of the other web ACL.
    let got = waf
        .ok("GetLoggingConfiguration", request(&acl_arn, None))
        .await;
    assert_eq!(
        got["LoggingConfiguration"]["ResourceArn"],
        json!(acl_arn),
        "a refused call must leave the stored configuration in place, got {got}"
    );
    waf.refused(
        "GetLoggingConfiguration",
        request(&other_arn, None),
        "WAFNonexistentItemException",
        "the other web ACL after every refusal",
    )
    .await;
}
