// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise logging-options setting
//! (`PutLoggingOptions` / `DescribeLoggingOptions`), driven over HTTP through the
//! same router the emulator boots with.
//!
//! The rule these tests pin: logging options are a per-scope SETTING, so a put
//! replaces the options of the one scope it names — the `workspaceName` it
//! carries, or the account-and-region-wide scope when it carries none — a
//! describe of that scope reports exactly what the last put stored, a scope no
//! put has named reports the disabled level, and a level outside the modelled
//! `LoggingLevel` set is refused rather than stored.

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::LOGGING_LEVEL_VALUES;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, DEFAULT_LOGGING_LEVEL, MAX_LOGGING_SCOPES,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};

/// Every value the modelled `LoggingLevel` enum accepts, written out here as an
/// oracle independent of the crate under test.
///
/// Source: `smithy_query(query="shape", shape="LoggingLevel")` on this service's
/// model, which reports `values: ["ERROR", "INFO", "OFF"]`.
const MODELLED_LEVELS: [&str; 3] = ["ERROR", "INFO", "OFF"];

/// Put `level` on `workspace` (`None` for the account-wide scope) and return
/// `(status, body)`.
async fn put_level(
    router: &smithy_server_runtime::MultiServiceRouter,
    workspace: Option<&str>,
    level: &str,
) -> (u16, Value) {
    let mut body = json!({ "loggingOptions": { "level": level } });
    if let Some(name) = workspace {
        body["workspaceName"] = Value::String(name.to_string());
    }
    call(router, "PUT", "/logging", Some(body)).await
}

/// Describe `workspace`'s logging options (`None` for the account-wide scope) and
/// return `(status, body)`.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    workspace: Option<&str>,
) -> (u16, Value) {
    let uri = match workspace {
        Some(name) => format!("/logging?workspaceName={name}"),
        None => "/logging".to_string(),
    };
    call(router, "GET", &uri, None).await
}

/// The `level` a describe reported, or `""`.
fn reported_level(body: &Value) -> &str {
    body["loggingOptions"]["level"].as_str().unwrap_or("")
}

/// The scope a case addresses, spelled as it reads in a case label.
fn scope_label(workspace: Option<&str>) -> String {
    match workspace {
        Some(name) => format!("workspace {name}"),
        None => "the account-wide scope".to_string(),
    }
}

/// The generated value table this service answers from carries exactly the
/// modelled enum, so the catalog the cases below span is the model's own.
#[test]
fn the_generated_level_table_is_the_modelled_enum() {
    assert_eq!(
        LOGGING_LEVEL_VALUES, &MODELLED_LEVELS,
        "the generated LoggingLevel table drifted from the model: expected {MODELLED_LEVELS:?}, \
         got {LOGGING_LEVEL_VALUES:?}"
    );
    assert!(
        MODELLED_LEVELS.contains(&DEFAULT_LOGGING_LEVEL),
        "the never-configured default {DEFAULT_LOGGING_LEVEL} is not a modelled LoggingLevel \
         ({MODELLED_LEVELS:?})"
    );
}

/// Every modelled level, on every scope form, reads back through the describe:
/// the setting the put stored is the one the describe reports.
#[tokio::test]
async fn every_modelled_level_reads_back_on_every_scope() {
    let router = router();

    // The rows differ along the two things the rule generalises over: the level
    // put, and the scope it is put on (account-wide, and workspace names spanning
    // the modelled `WorkspaceName` form — a single character, the mixed form with
    // the punctuation its pattern allows, and the 64-character maximum).
    let long_name = "w".repeat(64);
    let scopes: [Option<&str>; 4] = [None, Some("a"), Some("Plant_1-east"), Some(&long_name)];

    for scope in scopes {
        for level in MODELLED_LEVELS {
            let (status, body) = put_level(&router, scope, level).await;
            assert_eq!(
                status,
                200,
                "put {level} on {}: got {status} {body}",
                scope_label(scope)
            );

            let (status, body) = describe(&router, scope).await;
            assert_eq!(
                status,
                200,
                "describe {}: got {status} {body}",
                scope_label(scope)
            );
            assert_eq!(
                reported_level(&body),
                level,
                "describe {} after putting {level}: got {body}",
                scope_label(scope)
            );
        }
    }
}

/// A scope no put has named reports the disabled level, and reading it leaves it
/// that way — a describe is a read, so a later caller sees what the first one saw.
#[tokio::test]
async fn an_unconfigured_scope_reports_the_disabled_level_and_stays_that_way() {
    let router = router();

    let scopes: [Option<&str>; 3] = [None, Some("never-set"), Some("other")];
    for scope in scopes {
        for attempt in 1..=2 {
            let (status, body) = describe(&router, scope).await;
            assert_eq!(
                status,
                200,
                "describe {} (attempt {attempt}): got {status} {body}",
                scope_label(scope)
            );
            assert_eq!(
                reported_level(&body),
                DEFAULT_LOGGING_LEVEL,
                "describe {} (attempt {attempt}) with nothing set: got {body}",
                scope_label(scope)
            );
        }
    }

    // Configuring one scope does not configure another: the scopes left alone
    // still report the default.
    let (status, body) = put_level(&router, Some("never-set"), "INFO").await;
    assert_eq!(status, 200, "put INFO on workspace never-set: {status} {body}");
    for scope in [None, Some("other")] {
        let (_, body) = describe(&router, scope).await;
        assert_eq!(
            reported_level(&body),
            DEFAULT_LOGGING_LEVEL,
            "describe {} after another scope was configured: got {body}",
            scope_label(scope)
        );
    }
}

/// One scope's options are independent of every other's, and a put replaces only
/// the scope it names: the levels below are chosen so no two scopes share one, so
/// a put that leaked across scopes shows up as the wrong level.
#[tokio::test]
async fn each_scope_keeps_its_own_options() {
    let router = router();

    // scope -> the level it is set to. Every modelled level appears, and the
    // account-wide scope is set between two workspace scopes.
    let assignments: [(Option<&str>, &str); 4] = [
        (Some("plant-a"), "ERROR"),
        (None, "INFO"),
        (Some("plant-b"), "OFF"),
        (Some("plant-c"), "INFO"),
    ];

    for (scope, level) in assignments {
        let (status, body) = put_level(&router, scope, level).await;
        assert_eq!(
            status,
            200,
            "put {level} on {}: got {status} {body}",
            scope_label(scope)
        );
    }

    for (scope, level) in assignments {
        let (_, body) = describe(&router, scope).await;
        assert_eq!(
            reported_level(&body),
            level,
            "describe {} with every scope configured: expected {level}, got {body}",
            scope_label(scope)
        );
    }

    // Overwriting one scope moves that scope only.
    let (status, body) = put_level(&router, None, "OFF").await;
    assert_eq!(status, 200, "overwrite the account-wide scope: {status} {body}");
    let (_, body) = describe(&router, None).await;
    assert_eq!(
        reported_level(&body),
        "OFF",
        "the account-wide scope after being overwritten with OFF: got {body}"
    );
    for (scope, level) in assignments.into_iter().filter(|(scope, _)| scope.is_some()) {
        let (_, body) = describe(&router, scope).await;
        assert_eq!(
            reported_level(&body),
            level,
            "describe {} after the account-wide scope was overwritten: expected {level}, got \
             {body}",
            scope_label(scope)
        );
    }
}

/// Putting the same options twice is idempotent, and a put reports no body of its
/// own: the modelled response carries no members.
#[tokio::test]
async fn repeating_a_put_changes_nothing() {
    let router = router();

    for level in MODELLED_LEVELS {
        for attempt in 1..=3 {
            let (status, body) = put_level(&router, Some("repeated"), level).await;
            assert_eq!(
                status, 200,
                "put {level} on workspace repeated (attempt {attempt}): got {status} {body}"
            );
            let members = body.as_object().map(|object| object.len()).unwrap_or(0);
            assert_eq!(
                members, 0,
                "put {level} (attempt {attempt}) answered with members: {body}"
            );

            let (_, body) = describe(&router, Some("repeated")).await;
            assert_eq!(
                reported_level(&body),
                level,
                "describe workspace repeated after put {attempt} of {level}: got {body}"
            );
        }
    }
}

/// A level outside the modelled `LoggingLevel` set is refused, and the refusal
/// leaves the scope's stored options alone.
#[tokio::test]
async fn a_level_outside_the_modelled_set_is_refused_and_stores_nothing() {
    let router = router();

    let (status, body) = put_level(&router, None, "INFO").await;
    assert_eq!(status, 200, "seed the account-wide scope: {status} {body}");

    // Each row is a level the enum does not carry: a lowercase spelling of one it
    // does, a verbosity level other AWS services use, the empty string and a
    // value with trailing whitespace.
    for level in ["info", "DEBUG", "", "INFO "] {
        let (status, body) = put_level(&router, None, level).await;
        assert_eq!(
            status, 400,
            "put the unmodelled level {level:?}: got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "put the unmodelled level {level:?}: got {body}"
        );

        let (_, body) = describe(&router, None).await;
        assert_eq!(
            reported_level(&body),
            "INFO",
            "the account-wide scope after the refused level {level:?}: got {body}"
        );
    }
}

/// A `workspaceName` outside the modelled `WorkspaceName` form is refused on both
/// operations, and leaves the setting alone.
///
/// Bounds from `smithy_query(query="shape", shape="WorkspaceName")`: `len 1..64`,
/// pattern `^[a-zA-Z0-9_-]+$`.
#[tokio::test]
async fn a_workspace_name_outside_the_modelled_form_is_refused() {
    let router = router();

    let (status, body) = put_level(&router, None, "ERROR").await;
    assert_eq!(status, 200, "seed the account-wide scope: {status} {body}");

    let too_long = "w".repeat(65);
    // One row past each published bound of the length, plus a name carrying a
    // character the pattern excludes.
    for name in [too_long.as_str(), "plant.a", "plant@a"] {
        let (status, body) = put_level(&router, Some(name), "INFO").await;
        assert_eq!(
            status, 400,
            "put on the unmodelled workspace name {name:?}: got {status} {body}"
        );

        let (status, body) = describe(&router, Some(name)).await;
        assert_eq!(
            status, 400,
            "describe the unmodelled workspace name {name:?}: got {status} {body}"
        );
    }

    let (_, body) = describe(&router, None).await;
    assert_eq!(
        reported_level(&body),
        "ERROR",
        "the account-wide scope after the refused workspace names: got {body}"
    );
}

/// The store of scopes is bounded: puts naming ever-fresh workspaces never grow
/// it past `MAX_LOGGING_SCOPES`, an evicted scope reverts to the disabled level
/// (what it reported before anything set it), and the account-and-region-wide
/// scope — exempt from the eviction — keeps its options throughout.
#[tokio::test]
async fn the_scope_store_is_bounded_and_keeps_the_newest_scopes() {
    let (router, engine) = router_with_engine();

    let (status, body) = put_level(&router, None, "INFO").await;
    assert_eq!(status, 200, "seed the account-wide scope: {status} {body}");

    // One more workspace scope than the store keeps, so the eviction has to run.
    let overflow = MAX_LOGGING_SCOPES + 1;
    for index in 0..overflow {
        let (status, body) = put_level(&router, Some(&format!("w{index}")), "ERROR").await;
        assert_eq!(
            status, 200,
            "put ERROR on workspace w{index}: got {status} {body}"
        );
    }

    let held = service_state(&*engine)
        .expect("the iotsitewise store")
        .lock()
        .logging_options
        .len();
    assert!(
        held <= MAX_LOGGING_SCOPES,
        "after {overflow} workspace puts the store holds {held} scopes, past the \
         {MAX_LOGGING_SCOPES} bound"
    );

    // The scope set last is still there, and the account-wide one was never a
    // candidate for eviction.
    let newest = format!("w{}", overflow - 1);
    let (_, body) = describe(&router, Some(&newest)).await;
    assert_eq!(
        reported_level(&body),
        "ERROR",
        "the most recently set scope {newest} after the eviction: got {body}"
    );
    let (_, body) = describe(&router, None).await;
    assert_eq!(
        reported_level(&body),
        "INFO",
        "the account-wide scope after {overflow} workspace puts: got {body}"
    );

    // Every scope the store dropped answers as an unconfigured one again, and the
    // overflow guarantees at least one was dropped.
    let mut evicted = 0;
    for index in 0..overflow {
        let (_, body) = describe(&router, Some(&format!("w{index}"))).await;
        let level = reported_level(&body);
        assert!(
            level == "ERROR" || level == DEFAULT_LOGGING_LEVEL,
            "workspace w{index} reports {level}, which is neither the level it was set \
             to nor the unconfigured default"
        );
        if level == DEFAULT_LOGGING_LEVEL {
            evicted += 1;
        }
    }
    assert!(
        evicted >= overflow - MAX_LOGGING_SCOPES,
        "{overflow} workspace puts past a {MAX_LOGGING_SCOPES} bound dropped only \
         {evicted} scopes"
    );
}
