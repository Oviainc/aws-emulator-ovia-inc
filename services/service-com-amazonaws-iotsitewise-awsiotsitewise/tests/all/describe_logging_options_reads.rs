// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the READ side of the IoT SiteWise logging-options
//! setting, `DescribeLoggingOptions`, driven over HTTP through the same router the
//! emulator boots with.
//!
//! The rule these tests pin: a describe REPORTS the stored setting of exactly the
//! one scope the request addresses — the `workspaceName` it carries, or the
//! account-and-region-wide scope when it carries none — answering the disabled
//! level for a scope nothing has configured, and it reports it from the one shared
//! store rather than from a value of its own, so the answer follows whatever last
//! wrote that scope and reading never changes what a later caller sees.
//!
//! Every workspace scope these cases address is opened first, so what each one
//! reports is about the SETTING; the refusal a workspace nobody opened earns is
//! the sibling `logging_options_workspace_scope` suite's subject.
//!
//! Where the sibling `logging_options` suite drives the setting through the two
//! modelled operations, these cases seed the store DIRECTLY and read it back over
//! the wire: a handler that answered from anything but the shared store — a
//! constant, a private store of its own — would report the wrong level for the
//! seeded scopes while the put/describe pair still agreed with itself. The wire
//! form of the read itself is the sibling suite's `describe` helper, reached
//! rather than copied, so one spelling serves both suites.

use std::sync::Arc;
use std::time::SystemTime;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    LoggingOptions, LOGGING_LEVEL_VALUES,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, DEFAULT_LOGGING_LEVEL};
use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

use crate::asset_lifecycle::router_with_engine;
use crate::logging_options::{describe, open_scopes, put_level, reported_level, scope_label};

/// Every scope form, each seeded in the store with a level of its own, reads back
/// through the describe: the operation reports the stored setting of the scope the
/// request addresses, and never the 501 of an unimplemented handler.
#[tokio::test]
async fn describe_reports_the_level_the_store_holds_for_the_scope_it_addresses() {
    let (router, engine) = router_with_engine();

    // The rows span the two things the rule generalises over: the scope addressed
    // (account-wide, and workspace names spanning the modelled `WorkspaceName`
    // form — one character, the mixed form its pattern allows, the 64-character
    // maximum) and the level stored on it, so that no two neighbouring scopes
    // share a level and a leak across scopes shows up as the wrong level. Every
    // row stores a level OTHER than the unconfigured default, so no row could
    // pass against a handler that ignored the store for that scope.
    let longest_name = "w".repeat(64);
    let seeded: [(Option<&str>, &str); 4] = [
        (None, "ERROR"),
        (Some("a"), "INFO"),
        (Some("Plant_1-east"), "ERROR"),
        (Some(&longest_name), "INFO"),
    ];
    // A workspace scope is reported only for a workspace this service holds, so
    // the rows addressing one open it first; `not-seeded` below is open too, so
    // that what it reports is about the SETTING and not about the workspace.
    let addressed: Vec<Option<&str>> = seeded
        .iter()
        .map(|(scope, _)| *scope)
        .chain([Some("not-seeded")])
        .collect();
    open_scopes(&router, &addressed).await;
    for (scope, level) in seeded {
        assert_ne!(
            level, DEFAULT_LOGGING_LEVEL,
            "the row seeding {} stores the unconfigured default {DEFAULT_LOGGING_LEVEL}, so it \
             cannot tell a stored level from a fabricated one",
            scope_label(scope)
        );
    }

    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for (scope, level) in seeded {
            store.set_logging_options(
                scope,
                LoggingOptions {
                    level: level.to_string(),
                },
                SystemTime::now(),
            );
        }
    }

    for (scope, level) in seeded {
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
            "describe {} with {level} in the store: got {body}",
            scope_label(scope)
        );
    }

    // A scope the seeding left alone still answers the unconfigured default, so
    // the describe is reading the addressed scope rather than any configured one.
    let (status, body) = describe(&router, Some("not-seeded")).await;
    assert_eq!(status, 200, "describe workspace not-seeded: {status} {body}");
    assert_eq!(
        reported_level(&body),
        DEFAULT_LOGGING_LEVEL,
        "describe workspace not-seeded while other scopes are configured: got {body}"
    );
}

/// Every level the store can hold is a level the describe reports verbatim: the
/// rows below are the modelled `LoggingLevel` catalog, each one seeded on the same
/// scope in turn, so the describe follows the last write rather than reporting a
/// level of its own.
#[tokio::test]
async fn describe_follows_the_last_write_to_the_scope() {
    let (router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the iotsitewise store");

    // Source: `smithy_query(query="shape", shape="LoggingLevel")`, whose values are
    // ["ERROR", "INFO", "OFF"] — the generated table this iterates is pinned
    // against that list by the sibling `logging_options` suite.
    open_scopes(&router, &[Some("rewritten")]).await;
    for scope in [None, Some("rewritten")] {
        for level in LOGGING_LEVEL_VALUES {
            state.lock().set_logging_options(
                scope,
                LoggingOptions {
                    level: (*level).to_string(),
                },
                SystemTime::now(),
            );

            let (status, body) = describe(&router, scope).await;
            assert_eq!(
                status,
                200,
                "describe {} after {level} was stored: got {status} {body}",
                scope_label(scope)
            );
            assert_eq!(
                reported_level(&body),
                *level,
                "describe {} after {level} was stored: got {body}",
                scope_label(scope)
            );
        }
    }
}

/// A describe is a pure READ: it materialises nothing for the scope it reports, so
/// the store holds no more entries afterwards — on an empty store and on one that
/// already carries configured scopes — and the next caller sees exactly what the
/// first one saw.
#[tokio::test]
async fn describing_a_scope_stores_nothing_for_it() {
    let (router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the iotsitewise store");

    let scopes: [Option<&str>; 4] = [None, Some("one"), Some("two"), Some("three")];
    open_scopes(&router, &scopes).await;
    let empty = state.lock().logging_options.len();
    assert_eq!(
        empty, 0,
        "a fresh store already holds {empty} logging-option scopes"
    );

    // Round one: nothing is configured, so every scope answers the default and the
    // store stays empty.
    for attempt in 1..=3 {
        for scope in scopes {
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
                "describe {} (attempt {attempt}) with nothing stored: got {body}",
                scope_label(scope)
            );
        }

        let held = state.lock().logging_options.len();
        assert_eq!(
            held, empty,
            "after {attempt} rounds of describes on an empty store it holds {held} \
             logging-option scopes, up from {empty}"
        );
    }

    // Round two: two scopes ARE configured, so the store's entry count must stay at
    // those two however many scopes — configured or not — are described.
    for (scope, level) in [(None, "ERROR"), (Some("one"), "INFO")] {
        let (status, body) = put_level(&router, scope, level).await;
        assert_eq!(
            status,
            200,
            "put {level} on {}: got {status} {body}",
            scope_label(scope)
        );
    }
    let seeded = state.lock().logging_options.len();
    assert_eq!(
        seeded, 2,
        "two puts left {seeded} logging-option scopes in the store"
    );

    for attempt in 1..=3 {
        for scope in scopes {
            let (status, body) = describe(&router, scope).await;
            assert_eq!(
                status,
                200,
                "describe {} (configured round, attempt {attempt}): got {status} {body}",
                scope_label(scope)
            );
            let expected = match scope {
                None => "ERROR",
                Some("one") => "INFO",
                _ => DEFAULT_LOGGING_LEVEL,
            };
            assert_eq!(
                reported_level(&body),
                expected,
                "describe {} (configured round, attempt {attempt}): expected {expected}, got \
                 {body}",
                scope_label(scope)
            );
        }

        let held = state.lock().logging_options.len();
        assert_eq!(
            held, seeded,
            "after {attempt} rounds of describes over two configured scopes the store holds \
             {held} logging-option scopes, up from {seeded}"
        );
    }
}

/// The operation answers from the ONE shared engine store: a scope configured
/// through one mounted service reads back through another mounted on the same
/// engine, so no request is served from a store private to its own handler.
#[tokio::test]
async fn a_second_service_on_the_same_engine_reports_the_same_setting() {
    let (writer, engine) = router_with_engine();

    let mut reader = MultiServiceRouter::new();
    reader.set_request_authentication(false);
    reader.register_service(EmulatedService::new(Arc::clone(&engine)));

    let cases: [(Option<&str>, &str); 3] = [
        (None, "INFO"),
        (Some("shared"), "ERROR"),
        (Some("shared-too"), "OFF"),
    ];
    // Opened through the first service, on the engine both share.
    let addressed: Vec<Option<&str>> = cases.iter().map(|(scope, _)| *scope).collect();
    open_scopes(&writer, &addressed).await;

    for (scope, level) in cases {
        let (status, response) = put_level(&writer, scope, level).await;
        assert_eq!(
            status,
            200,
            "put {level} on {} through the first service: got {status} {response}",
            scope_label(scope)
        );
    }

    for (scope, level) in cases {
        let (status, body) = describe(&reader, scope).await;
        assert_eq!(
            status,
            200,
            "describe {} through the second service: got {status} {body}",
            scope_label(scope)
        );
        assert_eq!(
            reported_level(&body),
            level,
            "describe {} through the second service after putting {level}: got {body}",
            scope_label(scope)
        );
    }
}
