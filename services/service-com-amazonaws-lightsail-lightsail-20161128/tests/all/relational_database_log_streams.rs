// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseLogStreams` — the log streams a managed relational
//! database publishes — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a log stream is a log FILE the database's own engine
//! writes, so the streams `GetRelationalDatabaseLogStreams` answers for a stored
//! database are that database's ENGINE's log-file set — the same answer on every
//! read, a different answer for a MySQL database than for a PostgreSQL one, and
//! no answer at all (an empty list) while the database is still being created and
//! no engine has started; a name that names no stored database is refused with
//! the service's `DoesNotExist` envelope rather than answered with an empty list.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::instance_lifecycle::{error_code, Emulator};

/// A master user password every engine's limits admit (8..41 characters for
/// MySQL, 8..128 for PostgreSQL), drawn only from characters the modelled
/// `masterUserPassword` documentation allows.
const PASSWORD: &str = "Pw9#x!ZqPw9#x!Zq";

/// The log streams a database of `engine` must publish, keyed on the catalog's
/// own `RelationalDatabaseBlueprint$engine` value — never on the production
/// code's engine-matching rule, and never on how a blueprint id is spelled, so
/// an engine the catalog adds fails this suite loudly instead of silently
/// inheriting another engine's log files.
///
/// A Lightsail managed database is an Amazon RDS DB instance and a log stream is
/// one of the log files its engine writes, so this is the engine's own log-file
/// set from the Amazon RDS User Guide's database-log-file pages: MySQL writes an
/// error log (`error/mysql-error.log`, with its hourly-rotated running copy
/// `error/mysql-error-running.log`), a general query log
/// (`general/mysql-general.log`) and a slow query log
/// (`slowquery/mysql-slowquery.log`); PostgreSQL writes one log,
/// `error/postgresql.log`.
fn want_streams(engine: &str) -> Vec<&'static str> {
    match engine {
        "postgres" => vec!["error/postgresql.log"],
        "mysql" => vec![
            "error/mysql-error-running.log",
            "error/mysql-error.log",
            "general/mysql-general.log",
            "slowquery/mysql-slowquery.log",
        ],
        other => panic!(
            "case {other}: this suite's table does not know the log files of the engine {other:?}"
        ),
    }
}

/// The `logStreams` `GetRelationalDatabaseLogStreams` answers for `name`.
///
/// The member is a list, so it is always present: a database that publishes no
/// stream answers an EMPTY list rather than leaving it out.
async fn read_streams(emulator: &Emulator, name: &str) -> Vec<String> {
    let body = emulator
        .ok(
            "GetRelationalDatabaseLogStreams",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    body["logStreams"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the streams of {name}: expected a logStreams array, got {body:?}"))
        .iter()
        .map(|stream| {
            stream
                .as_str()
                .unwrap_or_else(|| panic!("case reading the streams of {name}: expected string streams, got {body:?}"))
                .to_string()
        })
        .collect()
}

/// The state `GetRelationalDatabase` reports for `name`.
async fn read_state(emulator: &Emulator, name: &str) -> String {
    let body = emulator
        .ok("GetRelationalDatabase", json!({"relationalDatabaseName": name}))
        .await;
    body["relationalDatabase"]["state"]
        .as_str()
        .unwrap_or_else(|| panic!("case reading {name}: expected a state, got {body:?}"))
        .to_string()
}

/// Poll `GetRelationalDatabase` until it reports `available`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_until_available(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = read_state(emulator, name).await;
        if state == "available" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for 'available' on {name}: expected it within 5s, got {state}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the database `name` on `blueprint_id` and `bundle_id`.
async fn create(emulator: &Emulator, name: &str, blueprint_id: &str, bundle_id: &str) {
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": blueprint_id,
                "relationalDatabaseBundleId": bundle_id,
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "masterUserPassword": PASSWORD,
            }),
        )
        .await;
}

/// Every blueprint this service publishes, as `(blueprintId, engine)` — the
/// catalog is the source of the engine a blueprint installs, so the table below
/// spans every engine and major version the service offers rather than the one
/// the issue named.
async fn published_blueprints(emulator: &Emulator) -> Vec<(String, String)> {
    let body = emulator
        .ok("GetRelationalDatabaseBlueprints", json!({}))
        .await;
    let blueprints = body["blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the blueprint catalog: expected an array, got {body:?}"))
        .iter()
        .map(|blueprint| {
            (
                blueprint["blueprintId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case reading the blueprint catalog: expected a blueprintId, got {blueprint:?}"))
                    .to_string(),
                blueprint["engine"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case reading the blueprint catalog: expected an engine, got {blueprint:?}"))
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        blueprints.len() > 1,
        "case reading the blueprint catalog: expected more than one blueprint to span, got \
         {blueprints:?}"
    );
    blueprints
}

/// Every bundle this service publishes, by id — the hardware a database is
/// created on, varied across the table so the streams are shown to follow the
/// engine and not the bundle.
async fn published_bundles(emulator: &Emulator) -> Vec<String> {
    let body = emulator.ok("GetRelationalDatabaseBundles", json!({})).await;
    let bundles = body["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the bundle catalog: expected an array, got {body:?}"))
        .iter()
        .map(|bundle| {
            bundle["bundleId"]
                .as_str()
                .unwrap_or_else(|| panic!("case reading the bundle catalog: expected a bundleId, got {bundle:?}"))
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(
        !bundles.is_empty(),
        "case reading the bundle catalog: expected at least one bundle, got {body:?}"
    );
    bundles
}

/// The engine the published blueprint `blueprint_id` installs, as the catalog
/// reports it.
///
/// The expectation is always keyed on the catalog's own `engine` member rather
/// than on how a blueprint id is spelled, so a case stays independent of the
/// service's engine-matching rule.
async fn engine_of(emulator: &Emulator, blueprint_id: &str) -> String {
    published_blueprints(emulator)
        .await
        .into_iter()
        .find(|(id, _)| id == blueprint_id)
        .unwrap_or_else(|| panic!("case reading the blueprint catalog: expected an entry for {blueprint_id}"))
        .1
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: the log streams a database publishes are its ENGINE's log-file set, so
/// every published blueprint answers the streams of the engine it installs — each
/// database answering its own, all of them in one store, on whatever hardware the
/// bundle gave it.
#[tokio::test]
async fn log_streams_follow_the_databases_engine() {
    let emulator = Emulator::new();
    let blueprints = published_blueprints(&emulator).await;
    let bundles = published_bundles(&emulator).await;

    // One database per published blueprint, in ONE store, each on a different
    // bundle than its neighbour.
    let mut databases = Vec::new();
    for (index, (blueprint_id, engine)) in blueprints.iter().enumerate() {
        let name = format!("logstreams-{blueprint_id}");
        let bundle_id = bundles[index % bundles.len()].clone();
        create(&emulator, &name, blueprint_id, &bundle_id).await;
        databases.push((name, blueprint_id.clone(), engine.clone(), bundle_id));
    }

    for (name, blueprint_id, engine, bundle_id) in &databases {
        let case = format!("{blueprint_id} on {bundle_id}");
        wait_until_available(&emulator, name).await;

        let streams = read_streams(&emulator, name).await;
        let want = want_streams(engine);
        assert_eq!(
            streams, want,
            "case {case}: expected the {engine} log streams {want:?}, got {streams:?}"
        );

        // A read changes nothing a later reader sees: the streams, and the
        // database itself, are the same afterwards.
        let again = read_streams(&emulator, name).await;
        assert_eq!(
            again, streams,
            "case {case}: two reads of one database's log streams must answer the same list, got \
             {again:?}"
        );
        let state = read_state(&emulator, name).await;
        assert_eq!(
            state, "available",
            "case {case}: reading the log streams must not move the database's state, got {state}"
        );
    }

    // The engines really do differ, so the table above is not asserting one
    // constant answer for every row.
    let answers: std::collections::BTreeSet<Vec<String>> = {
        let mut set = std::collections::BTreeSet::new();
        for (name, _, _, _) in &databases {
            set.insert(read_streams(&emulator, name).await);
        }
        set
    };
    assert!(
        answers.len() > 1,
        "case spanning the catalog: expected the engines to publish different stream sets, got \
         {answers:?}"
    );
}

/// RULE: a log stream exists only once the engine writing it has run, so a
/// database still being created publishes NO stream — and the clock, not a
/// reader, is what makes the streams appear.
#[tokio::test]
async fn a_database_still_being_created_publishes_no_stream() {
    for (blueprint_id, bundle_id) in [("mysql_8_4", "micro_1_0"), ("postgres_17", "small_1_0")] {
        let emulator = Emulator::new();
        let name = format!("creating-{blueprint_id}");
        create(&emulator, &name, blueprint_id, bundle_id).await;

        // Straight after the create the database is still `creating`: it serves
        // no connection and has written no log.
        let state = read_state(&emulator, &name).await;
        assert_eq!(
            state, "creating",
            "case {blueprint_id}: expected the database to be created `creating`, got {state}"
        );
        let streams = read_streams(&emulator, &name).await;
        assert!(
            streams.is_empty(),
            "case {blueprint_id}: a database that is still being created publishes no log stream, \
             got {streams:?}"
        );

        wait_until_available(&emulator, &name).await;
        let streams = read_streams(&emulator, &name).await;
        let want = want_streams(&engine_of(&emulator, blueprint_id).await);
        assert_eq!(
            streams, want,
            "case {blueprint_id}: expected the engine's streams {want:?} once the database is \
             available, got {streams:?}"
        );
    }
}

/// RULE: only a database that has never run its engine publishes no stream — a
/// log file survives a RESTART, since the engine already wrote it and the
/// restart is itself logged — so a rebooting database reports the same streams
/// it did before the reboot and goes on reporting them once the clock settles it
/// back to `available`.
#[tokio::test]
async fn a_rebooting_database_keeps_its_log_streams() {
    for (blueprint_id, bundle_id) in [("mysql_8_4", "micro_1_0"), ("postgres_17", "small_1_0")] {
        let emulator = Emulator::new();
        let name = format!("rebooting-{blueprint_id}");
        create(&emulator, &name, blueprint_id, bundle_id).await;
        wait_until_available(&emulator, &name).await;
        let want = want_streams(&engine_of(&emulator, blueprint_id).await);
        let before = read_streams(&emulator, &name).await;
        assert_eq!(
            before, want,
            "case {blueprint_id}: expected the engine's streams {want:?} before the reboot, got \
             {before:?}"
        );

        emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let state = read_state(&emulator, &name).await;
        assert_eq!(
            state, "rebooting",
            "case {blueprint_id}: expected the reboot to leave the database `rebooting`, got \
             {state}"
        );
        let during = read_streams(&emulator, &name).await;
        assert_eq!(
            during, want,
            "case {blueprint_id}: a restart does not erase the log files, so the streams stay \
             {want:?}, got {during:?}"
        );

        wait_until_available(&emulator, &name).await;
        let after = read_streams(&emulator, &name).await;
        assert_eq!(
            after, want,
            "case {blueprint_id}: expected the same streams once the database is available again, \
             got {after:?}"
        );
    }
}

/// RULE: a well-formed name that names no stored database is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name —
/// never a 501 and never an empty list of streams.
#[tokio::test]
async fn an_unknown_database_has_no_log_streams_to_report() {
    let emulator = Emulator::new();
    create(&emulator, "present-db", "mysql_8_4", "micro_1_0").await;

    // Names that name nothing: one never created, one that differs from a stored
    // name only in case, one that differs only in its separator, and one an
    // instance — a different resource type — could hold.
    for name in ["absent-db", "Present-DB", "present_db", "present-db-2"] {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseLogStreams",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case reading the streams of {name}: expected the DoesNotExist refusal, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading the streams of {name}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading the streams of {name}: expected the DoesNotExist code, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The RelationalDatabase does not exist: {name}").as_str()),
            "case reading the streams of {name}: expected the resource-type/name message, got \
             {body:?}"
        );
    }

    // The refused reads left the one stored database exactly as it was.
    wait_until_available(&emulator, "present-db").await;
    let streams = read_streams(&emulator, "present-db").await;
    let want = want_streams(&engine_of(&emulator, "mysql_8_4").await);
    assert_eq!(
        streams, want,
        "case after the refusals: expected the stored database's streams {want:?}, got {streams:?}"
    );
}

/// RULE: every stream a database publishes is a log file's path, so each is a
/// non-empty value a caller can hand straight back as
/// `GetRelationalDatabaseLogEvents$logStreamName`, and the list carries no
/// duplicate.
#[tokio::test]
async fn every_published_stream_is_a_usable_name() {
    let emulator = Emulator::new();
    let blueprints = published_blueprints(&emulator).await;
    for (blueprint_id, _) in &blueprints {
        let name = format!("usable-{blueprint_id}");
        create(&emulator, &name, blueprint_id, "micro_1_0").await;
        wait_until_available(&emulator, &name).await;
        let streams = read_streams(&emulator, &name).await;
        assert!(
            !streams.is_empty(),
            "case {blueprint_id}: an available database publishes at least its error log, got \
             {streams:?}"
        );
        let unique: std::collections::BTreeSet<&String> = streams.iter().collect();
        assert_eq!(
            unique.len(),
            streams.len(),
            "case {blueprint_id}: a stream name addresses one log, so the list carries no \
             duplicate, got {streams:?}"
        );
        for stream in &streams {
            assert!(
                !stream.trim().is_empty() && !stream.starts_with('/'),
                "case {blueprint_id}: expected a relative, non-empty log path, got {stream:?}"
            );
        }
        let sorted = {
            let mut sorted = streams.clone();
            sorted.sort();
            sorted
        };
        assert_eq!(
            streams, sorted,
            "case {blueprint_id}: expected the streams in a stable, sorted order, got {streams:?}"
        );
    }
}
