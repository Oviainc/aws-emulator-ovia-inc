// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseParameters` — the runtime parameters of one managed
//! database, driven over the wire.
//!
//! Every wire case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRelationalDatabaseParameters` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — every engine this
//! service publishes a blueprint for, on bundles of four different sizes —
//! rather than over one recorded call:
//!
//! * the operation reports the runtime parameter set of the ENGINE the
//!   database's blueprint names, each entry carrying the allowed range, data
//!   type, apply type, apply method, description and modifiability that engine
//!   publishes for it;
//! * a parameter's `parameterValue` is the value that database RUNS it at: no
//!   value at all where the service's parameter group leaves the parameter at
//!   the engine's own default, the pinned value where the group fixes one, and
//!   the published memory formula resolved against the database's own bundle
//!   where the group sizes the parameter by memory — so two databases of
//!   different sizes report different numbers, and every reported value lies
//!   inside the range reported beside it;
//! * `applyMethod` says when a change lands and so follows `applyType`:
//!   `immediate` for a dynamic parameter, `pending-reboot` for a static one;
//! * one page is the set from the entry `pageToken` names onward, bounded by the
//!   published page size, with `nextPageToken` naming the first entry of the
//!   next page and present only while entries remain, and a `pageToken` that
//!   names no parameter OF THIS DATABASE's engine is a modelled 400
//!   `InvalidInputException`;
//! * a database name the account does not hold is the service's `DoesNotExist`
//!   envelope, never a hollow 200; and
//! * reporting parameters is a READ: identical calls answer identically and
//!   nothing else a caller can see changes.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    relational_database_parameter_page, relational_database_parameter_specs,
    RelationalDatabaseParameterSpec, RelationalDatabaseParameterValue,
    PARAMETER_APPLY_METHOD_IMMEDIATE, PARAMETER_APPLY_METHOD_PENDING_REBOOT,
    PARAMETER_APPLY_TYPE_DYNAMIC, PARAMETER_APPLY_TYPE_STATIC,
    RELATIONAL_DATABASE_PARAMETER_PAGE_SIZE,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// The operation under test.
const OPERATION: &str = "GetRelationalDatabaseParameters";

/// Every member the modelled `RelationalDatabaseParameter` structure declares
/// (`smithy_query(query="shape",shape="RelationalDatabaseParameter")`), in wire
/// spelling. A reported parameter carries no member outside this set.
const PARAMETER_MEMBERS: &[&str] = &[
    "allowedValues",
    "applyMethod",
    "applyType",
    "dataType",
    "description",
    "isModifiable",
    "parameterName",
    "parameterValue",
];

/// The members a reported parameter always carries: everything the engine
/// publishes about the parameter itself, whatever value it happens to run at.
const ALWAYS_REPORTED_MEMBERS: &[&str] = &[
    "applyMethod",
    "applyType",
    "dataType",
    "description",
    "isModifiable",
    "parameterName",
];

/// Bytes in a gibibyte: the unit a bundle's RAM is published in.
const BYTES_PER_GIB: i64 = 1024 * 1024 * 1024;

/// How long to wait for the clock-owed `creating` -> `available` transition
/// before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// What the engines publish
// ---------------------------------------------------------------------------

/// Where a parameter's current value comes from.
///
/// Written out here independently of the service's own catalog types, so these
/// rows pin the published figures rather than restating whatever the code
/// happens to hold.
enum GroupValue {
    /// The parameter group pins nothing, so the parameter runs at the engine's
    /// own default and the response carries no `parameterValue` for it.
    EngineDefault,
    /// The parameter group pins this value on every database.
    Fixed(&'static str),
    /// The parameter group sizes the parameter by the hardware's memory:
    /// `min(memory_bytes * numerator / denominator, cap)`.
    FromMemory {
        numerator: i64,
        denominator: i64,
        cap: Option<i64>,
    },
}

/// What the service must report for one runtime parameter of one engine.
struct Expected {
    parameter_name: &'static str,
    value: GroupValue,
    /// The published range, or `None` for a parameter the engine publishes no
    /// range for — and then `allowedValues` must be left out entirely.
    allowed_values: Option<&'static str>,
    apply_type: &'static str,
    data_type: &'static str,
    is_modifiable: bool,
}

/// MySQL parameters the service must report.
///
/// SOURCE: the MySQL manual's "Server System Variables" for each variable's
/// range and dynamic/static nature, and the managed service's own parameter
/// group for the two memory-sized values — RDS publishes
/// `innodb_buffer_pool_size` as `{DBInstanceClassMemory*3/4}` and
/// `max_connections` as `{DBInstanceClassMemory/12582880}` for MySQL — and the
/// UTC time zone a managed DB instance runs in unless the customer changes it.
/// `allow-suspicious-udfs` is the entry the AWS CLI reference's own
/// `get-relational-database-parameters` example shows: static, boolean,
/// `0,1`, not modifiable and carrying no value.
const MYSQL_EXPECTED: &[Expected] = &[
    Expected {
        parameter_name: "allow-suspicious-udfs",
        value: GroupValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: "boolean",
        is_modifiable: false,
    },
    Expected {
        parameter_name: "auto_increment_increment",
        value: GroupValue::EngineDefault,
        allowed_values: Some("1-65535"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "innodb_buffer_pool_size",
        value: GroupValue::FromMemory {
            numerator: 3,
            denominator: 4,
            cap: None,
        },
        allowed_values: Some("5242880-9223372036854775807"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "max_connections",
        value: GroupValue::FromMemory {
            numerator: 1,
            denominator: 12_582_880,
            cap: None,
        },
        allowed_values: Some("1-100000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "time_zone",
        value: GroupValue::Fixed("UTC"),
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "string",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "wait_timeout",
        value: GroupValue::EngineDefault,
        allowed_values: Some("1-31536000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        is_modifiable: true,
    },
];

/// PostgreSQL parameters the service must report.
///
/// SOURCE: the PostgreSQL documentation's "Server Configuration" for each
/// parameter's range, unit and reset-required nature — `max_connections` and
/// `shared_buffers` are read only at startup, so both are static — and the
/// managed service's own parameter group for the two memory-sized values: RDS
/// publishes `shared_buffers` as `{DBInstanceClassMemory/32768}` (its unit is an
/// 8 kB page) and `max_connections` as
/// `LEAST({DBInstanceClassMemory/9531392},5000)` for PostgreSQL, beside the same
/// UTC time zone.
const POSTGRES_EXPECTED: &[Expected] = &[
    Expected {
        parameter_name: "client_encoding",
        value: GroupValue::EngineDefault,
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "string",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "max_connections",
        value: GroupValue::FromMemory {
            numerator: 1,
            denominator: 9_531_392,
            cap: Some(5000),
        },
        allowed_values: Some("1-262143"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: "integer",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "shared_buffers",
        value: GroupValue::FromMemory {
            numerator: 1,
            denominator: 32_768,
            cap: None,
        },
        allowed_values: Some("16-1073741823"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: "integer",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "statement_timeout",
        value: GroupValue::EngineDefault,
        allowed_values: Some("0-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "timezone",
        value: GroupValue::Fixed("UTC"),
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "string",
        is_modifiable: true,
    },
    Expected {
        parameter_name: "work_mem",
        value: GroupValue::EngineDefault,
        allowed_values: Some("64-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        is_modifiable: true,
    },
];

/// One engine a database can be created on: the blueprint that names it, the
/// parameters the engine must report, and a parameter of the OTHER engine, which
/// this one must not report.
struct EngineCase {
    case: &'static str,
    blueprint_id: &'static str,
    expected: &'static [Expected],
    foreign_parameter: &'static str,
}

/// The engine rows: both engines the blueprint catalog publishes, and — for
/// PostgreSQL, whose catalog spans major versions 13 to 18 — the oldest, the
/// default and the newest of them, so the rule is exercised on an entry this
/// issue did not cite as well as on the engine's default version.
const ENGINE_CASES: &[EngineCase] = &[
    EngineCase {
        case: "mysql_8_4",
        blueprint_id: "mysql_8_4",
        expected: MYSQL_EXPECTED,
        foreign_parameter: "shared_buffers",
    },
    EngineCase {
        case: "postgres_13",
        blueprint_id: "postgres_13",
        expected: POSTGRES_EXPECTED,
        foreign_parameter: "innodb_buffer_pool_size",
    },
    EngineCase {
        case: "postgres_17",
        blueprint_id: "postgres_17",
        expected: POSTGRES_EXPECTED,
        foreign_parameter: "innodb_buffer_pool_size",
    },
    EngineCase {
        case: "postgres_18",
        blueprint_id: "postgres_18",
        expected: POSTGRES_EXPECTED,
        foreign_parameter: "wait_timeout",
    },
];

/// The bundles a database is created on, with the RAM the published
/// managed-database plan table gives each: 1 GB (Micro), 2 GB (Small), 4 GB
/// (Medium) and 8 GB (Large with High Availability, which sells the same 8 GB of
/// RAM with a standby). The rows differ in exactly what the memory-sized
/// parameter values are computed from.
const BUNDLES: &[(&str, i64)] = &[
    ("micro_1_0", 1),
    ("small_1_0", 2),
    ("medium_1_0", 4),
    ("large_ha_1_0", 8),
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The value `expected` must report on a database with `ram_gib` gibibytes of
/// RAM, or `None` where it must report no value at all.
fn want_value(expected: &Expected, ram_gib: i64) -> Option<String> {
    match expected.value {
        GroupValue::EngineDefault => None,
        GroupValue::Fixed(value) => Some(value.to_string()),
        GroupValue::FromMemory {
            numerator,
            denominator,
            cap,
        } => {
            let value = ram_gib * BYTES_PER_GIB * numerator / denominator;
            Some(cap.map_or(value, |cap| value.min(cap)).to_string())
        }
    }
}

/// The `applyMethod` a parameter of `apply_type` must report: a change to a
/// dynamic parameter lands at once, a change to a static one at the next reboot.
fn want_apply_method(apply_type: &str) -> &'static str {
    if apply_type == PARAMETER_APPLY_TYPE_DYNAMIC {
        PARAMETER_APPLY_METHOD_IMMEDIATE
    } else {
        PARAMETER_APPLY_METHOD_PENDING_REBOOT
    }
}

/// Create a database named `db_name` on `blueprint_id` and `bundle_id`.
async fn create_database(emulator: &Emulator, db_name: &str, blueprint_id: &str, bundle_id: &str) {
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": db_name,
                "relationalDatabaseBlueprintId": blueprint_id,
                "relationalDatabaseBundleId": bundle_id,
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "masterUserPassword": "Pw9#x!ZqPw9#x!Zq",
            }),
        )
        .await;
}

/// The parameters one `GetRelationalDatabaseParameters` call answers, in the
/// order it reported them, together with its `nextPageToken`.
async fn page(emulator: &Emulator, request: Value) -> (Vec<Value>, Option<String>) {
    let case = request.clone();
    let body = emulator.ok(OPERATION, request).await;
    let parameters = body["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a parameters array, got {body:?}"))
        .clone();
    let next = body["nextPageToken"].as_str().map(str::to_string);
    (parameters, next)
}

/// Every parameter the walk from `request` reports, in page order, with the size
/// of each page it took.
async fn walk(emulator: &Emulator, request: Value) -> (Vec<Value>, Vec<usize>) {
    let mut parameters = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let (this_page, next) = page(emulator, body).await;
        page_sizes.push(this_page.len());
        parameters.extend(this_page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case {request:?}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (parameters, page_sizes)
}

/// The parameter names, in reported order, of `parameters`.
fn names(case: &str, parameters: &[Value]) -> Vec<String> {
    parameters
        .iter()
        .map(|parameter| {
            parameter["parameterName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every parameter carries a parameterName, got {parameter:?}")
                })
                .to_string()
        })
        .collect()
}

/// `parameters` keyed by name, refusing a name reported twice.
fn keyed(case: &str, parameters: &[Value]) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for (name, parameter) in names(case, parameters).into_iter().zip(parameters) {
        assert!(
            map.insert(name.clone(), parameter.clone()).is_none(),
            "case {case}: parameter {name:?} was reported twice"
        );
    }
    map
}

/// Whether `value` is inside the published `allowed_values`, which is either a
/// comma-separated list of literals (`0,1`) or a `low-high` range — where the
/// low bound may itself be negative (`-1-2147483647`).
fn inside_allowed(value: &str, allowed: &str) -> bool {
    if allowed.contains(',') {
        return allowed.split(',').any(|candidate| candidate == value);
    }
    let Some((low, high)) = split_range(allowed) else {
        // Not a range the engine published in a form this suite understands;
        // membership is then not asserted.
        return true;
    };
    value
        .parse::<i64>()
        .is_ok_and(|value| value >= low && value <= high)
}

/// `low-high` as its two bounds, honouring a negative low bound.
fn split_range(allowed: &str) -> Option<(i64, i64)> {
    let (low, high) = match allowed.strip_prefix('-') {
        Some(rest) => {
            let (low, high) = rest.split_once('-')?;
            (format!("-{low}"), high.to_string())
        }
        None => {
            let (low, high) = allowed.split_once('-')?;
            (low.to_string(), high.to_string())
        }
    };
    Some((low.parse().ok()?, high.parse().ok()?))
}

/// The stored database `name`, once the clock has settled it into `available`.
async fn wait_for_available(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let database = emulator
            .ok("GetRelationalDatabase", json!({"relationalDatabaseName": name}))
            .await["relationalDatabase"]
            .clone();
        if database["state"].as_str() == Some("available") {
            return database;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the database never became available, last read {database:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: the operation reports the ENGINE's runtime parameter set for the
/// database named, each entry carrying what the engine publishes about it and
/// the value that database runs it at.
#[tokio::test]
async fn every_engine_parameter_is_reported_with_the_value_its_database_runs_it_at() {
    for engine_case in ENGINE_CASES {
        for (bundle_id, ram_gib) in BUNDLES {
            let case = format!("{} on {bundle_id}", engine_case.case);
            let emulator = Emulator::new();
            let db_name = "parameters-db";
            create_database(&emulator, db_name, engine_case.blueprint_id, bundle_id).await;

            let (parameters, page_sizes) = walk(
                &emulator,
                json!({"relationalDatabaseName": db_name}),
            )
            .await;
            let reported = keyed(&case, &parameters);

            for expected in engine_case.expected {
                let name = expected.parameter_name;
                let parameter = reported.get(name).unwrap_or_else(|| {
                    panic!(
                        "case {case}: expected parameter {name} to be reported, got {:?}",
                        reported.keys().collect::<Vec<_>>()
                    )
                });

                assert_eq!(
                    parameter["applyType"].as_str(),
                    Some(expected.apply_type),
                    "case {case}: expected {name} applyType {}, got {:?}",
                    expected.apply_type,
                    parameter["applyType"]
                );
                assert_eq!(
                    parameter["applyMethod"].as_str(),
                    Some(want_apply_method(expected.apply_type)),
                    "case {case}: expected {name} applyMethod {}, got {:?}",
                    want_apply_method(expected.apply_type),
                    parameter["applyMethod"]
                );
                assert_eq!(
                    parameter["dataType"].as_str(),
                    Some(expected.data_type),
                    "case {case}: expected {name} dataType {}, got {:?}",
                    expected.data_type,
                    parameter["dataType"]
                );
                assert_eq!(
                    parameter["isModifiable"].as_bool(),
                    Some(expected.is_modifiable),
                    "case {case}: expected {name} isModifiable {}, got {:?}",
                    expected.is_modifiable,
                    parameter["isModifiable"]
                );
                assert_eq!(
                    parameter["allowedValues"].as_str(),
                    expected.allowed_values,
                    "case {case}: expected {name} allowedValues {:?}, got {:?}",
                    expected.allowed_values,
                    parameter["allowedValues"]
                );
                assert_eq!(
                    parameter["parameterValue"].as_str().map(str::to_string),
                    want_value(expected, *ram_gib),
                    "case {case}: expected {name} parameterValue {:?}, got {:?}",
                    want_value(expected, *ram_gib),
                    parameter["parameterValue"]
                );
                assert!(
                    parameter["description"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "case {case}: expected {name} to describe itself, got {parameter:?}"
                );
            }

            // The set is the ENGINE's: a parameter of the other engine is not in
            // it.
            assert!(
                !reported.contains_key(engine_case.foreign_parameter),
                "case {case}: {} belongs to the other engine and must not be reported, got {:?}",
                engine_case.foreign_parameter,
                reported.keys().collect::<Vec<_>>()
            );

            // Every reported entry — not only the ones written out above —
            // carries exactly the modelled members, and nothing outside them.
            for (name, parameter) in &reported {
                let members: BTreeSet<&str> = parameter
                    .as_object()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a parameter is an object, got {parameter:?}")
                    })
                    .keys()
                    .map(String::as_str)
                    .collect();
                let modelled: BTreeSet<&str> = PARAMETER_MEMBERS.iter().copied().collect();
                assert!(
                    members.is_subset(&modelled),
                    "case {case}: {name} reported members outside the modelled \
                     RelationalDatabaseParameter, got {members:?}"
                );
                for always in ALWAYS_REPORTED_MEMBERS {
                    assert!(
                        members.contains(always),
                        "case {case}: {name} must report {always}, got {members:?}"
                    );
                }
                // applyMethod follows applyType for EVERY entry, not only the
                // cited ones.
                let apply_type = parameter["applyType"].as_str().unwrap_or_default();
                assert!(
                    apply_type == PARAMETER_APPLY_TYPE_DYNAMIC
                        || apply_type == PARAMETER_APPLY_TYPE_STATIC,
                    "case {case}: {name} must report a dynamic or static applyType, got {apply_type:?}"
                );
                assert_eq!(
                    parameter["applyMethod"].as_str(),
                    Some(want_apply_method(apply_type)),
                    "case {case}: {name} is {apply_type}, so expected applyMethod {}, got {:?}",
                    want_apply_method(apply_type),
                    parameter["applyMethod"]
                );
                // A reported value is inside the range reported beside it.
                if let (Some(value), Some(allowed)) = (
                    parameter["parameterValue"].as_str(),
                    parameter["allowedValues"].as_str(),
                ) {
                    assert!(
                        inside_allowed(value, allowed),
                        "case {case}: {name} reported value {value:?} outside its own \
                         allowedValues {allowed:?}"
                    );
                }
            }

            // The whole set is the service's published catalog for that engine,
            // reported once each in catalog order, and it fits in one page.
            let catalog: Vec<&str> = relational_database_parameter_specs(
                if engine_case.blueprint_id.starts_with("postgres") {
                    "postgres"
                } else {
                    "mysql"
                },
            )
            .iter()
            .map(|spec| spec.parameter_name)
            .collect();
            assert_eq!(
                names(&case, &parameters),
                catalog
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>(),
                "case {case}: expected the engine's whole parameter set in catalog order"
            );
            assert!(
                catalog.len() <= RELATIONAL_DATABASE_PARAMETER_PAGE_SIZE,
                "case {case}: the published page size must hold a whole parameter set, got {} \
                 entries for a page of {RELATIONAL_DATABASE_PARAMETER_PAGE_SIZE}",
                catalog.len()
            );
            assert_eq!(
                page_sizes.len(),
                1,
                "case {case}: a set that fits in one page is answered in one page, got pages {page_sizes:?}"
            );
        }
    }
}

/// RULE: a memory-sized parameter is sized by the database's OWN hardware, so a
/// bigger bundle reports a bigger value — never the same number for every
/// database.
#[tokio::test]
async fn a_memory_sized_parameter_grows_with_the_bundle_it_runs_on() {
    for engine_case in ENGINE_CASES {
        let mut seen: Vec<(i64, i64)> = Vec::new();
        for (bundle_id, ram_gib) in BUNDLES {
            let case = format!("{} on {bundle_id}", engine_case.case);
            let emulator = Emulator::new();
            let db_name = "sized-db";
            create_database(&emulator, db_name, engine_case.blueprint_id, bundle_id).await;

            let (parameters, _) =
                walk(&emulator, json!({"relationalDatabaseName": db_name})).await;
            let reported = keyed(&case, &parameters);
            let connections = reported["max_connections"]["parameterValue"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: max_connections must report a value, got {reported:?}")
                })
                .parse::<i64>()
                .unwrap_or_else(|error| {
                    panic!("case {case}: max_connections must be a whole number, got {error}")
                });
            seen.push((*ram_gib, connections));
        }

        for window in seen.windows(2) {
            let (small_ram, small) = window[0];
            let (large_ram, large) = window[1];
            assert!(
                large > small,
                "case {}: {large_ram} GB must admit more connections than {small_ram} GB, \
                 got {large} against {small}",
                engine_case.case
            );
        }
    }
}

/// RULE: one page is the set from the entry `pageToken` names onward, and the
/// token is the parameter name a previous page reported.
#[tokio::test]
async fn a_page_token_opens_the_set_at_the_parameter_it_names() {
    for engine_case in ENGINE_CASES {
        let emulator = Emulator::new();
        let db_name = "paged-db";
        create_database(&emulator, db_name, engine_case.blueprint_id, "micro_1_0").await;

        let (whole, _) = walk(&emulator, json!({"relationalDatabaseName": db_name})).await;
        let all = names(engine_case.case, &whole);

        // The rows differ along which entry the token names: the first, one in
        // the middle, the last — and the two forms of "no token at all".
        let mut rows: Vec<(String, Value)> = vec![
            (
                "absent".to_string(),
                json!({"relationalDatabaseName": db_name}),
            ),
            (
                "empty".to_string(),
                json!({"relationalDatabaseName": db_name, "pageToken": ""}),
            ),
        ];
        for index in [0, all.len() / 2, all.len() - 1] {
            rows.push((
                format!("from {}", all[index]),
                json!({"relationalDatabaseName": db_name, "pageToken": all[index]}),
            ));
        }

        for (case, request) in rows {
            let case = format!("{} {case}", engine_case.case);
            let (parameters, next) = page(&emulator, request).await;
            let reported = names(&case, &parameters);
            let want: Vec<String> = match reported.first() {
                Some(first) => all
                    .iter()
                    .skip_while(|name| *name != first)
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            assert_eq!(
                reported, want,
                "case {case}: a page is the set from its first entry onward, in order"
            );
            // The whole remaining set fits in one page, so no page of these
            // rows carries a next token.
            assert_eq!(
                next, None,
                "case {case}: no parameters remain after this page, so it reports no nextPageToken"
            );
        }

        // A token naming the first parameter answers the whole set, exactly as
        // no token does.
        let (from_first, _) = page(
            &emulator,
            json!({"relationalDatabaseName": db_name, "pageToken": all[0]}),
        )
        .await;
        assert_eq!(
            names(engine_case.case, &from_first),
            all,
            "case {}: a token naming the first parameter answers the whole set",
            engine_case.case
        );
    }
}

/// RULE: a `pageToken` that names no parameter of THIS database's engine is a
/// modelled 400 `InvalidInputException` — including a parameter name that
/// belongs to the other engine.
#[tokio::test]
async fn a_page_token_that_names_no_parameter_of_this_database_is_refused() {
    for engine_case in ENGINE_CASES {
        let emulator = Emulator::new();
        let db_name = "bad-token-db";
        create_database(&emulator, db_name, engine_case.blueprint_id, "small_1_0").await;

        // The rows differ along HOW the token misses: a name no engine has, a
        // name of the other engine, a case variant of a real one, and a prefix
        // of a real one.
        for token in [
            "not-a-parameter",
            engine_case.foreign_parameter,
            "MAX_CONNECTIONS",
            "max_connection",
        ] {
            let case = format!("{} token {token}", engine_case.case);
            let (status, body) = emulator
                .call(
                    OPERATION,
                    json!({"relationalDatabaseName": db_name, "pageToken": token}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("pageToken")),
                "case {case}: the refusal must name the member it refused, got {body:?}"
            );
        }
    }
}

/// RULE: a well-formed name that names no stored database is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name
/// — never a 501 and never a hollow 200.
#[tokio::test]
async fn parameters_of_an_unknown_database_do_not_exist() {
    let emulator = Emulator::new();
    create_database(&emulator, "present-db", "mysql_8_4", "micro_1_0").await;

    for name in ["absent-db", "present-db-2", "other_db"] {
        let (status, body) = emulator
            .call(OPERATION, json!({"relationalDatabaseName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {name}: expected the DoesNotExist envelope's 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {name}: expected the body code DoesNotExist, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("RelationalDatabase")
                    && message.contains(name)),
            "case {name}: the refusal must name the resource type and the requested name, got {body:?}"
        );
    }
}

/// RULE: reporting parameters is a READ — two identical calls answer
/// identically, the stored database is untouched by the call, and the set does
/// not depend on the database's lifecycle state.
#[tokio::test]
async fn reporting_parameters_changes_nothing_a_later_caller_sees() {
    for engine_case in ENGINE_CASES {
        let emulator = Emulator::new();
        let db_name = "read-db";
        create_database(&emulator, db_name, engine_case.blueprint_id, "medium_1_0").await;

        let request = json!({"relationalDatabaseName": db_name});
        // While the database is still being created...
        let first = emulator.ok(OPERATION, request.clone()).await;
        let again = emulator.ok(OPERATION, request.clone()).await;
        assert_eq!(
            first, again,
            "case {}: two reads of one database's parameters must answer the same set",
            engine_case.case
        );

        // ...and once the clock has settled it into `available`: the engine's
        // parameters do not depend on the lifecycle state.
        let settled = wait_for_available(&emulator, db_name).await;
        let after = emulator.ok(OPERATION, request.clone()).await;
        assert_eq!(
            first, after,
            "case {}: the engine's parameter set must not change when the database settles",
            engine_case.case
        );

        // The call left the database record itself alone.
        let read_again = emulator
            .ok(
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": db_name}),
            )
            .await["relationalDatabase"]
            .clone();
        assert_eq!(
            settled, read_again,
            "case {}: reporting parameters must not change the stored database",
            engine_case.case
        );
    }
}

// ---------------------------------------------------------------------------
// The paging and sizing rules themselves
// ---------------------------------------------------------------------------

/// A parameter spec that differs from its neighbours only in name and value —
/// all the paging rule can see.
fn spec(
    parameter_name: &'static str,
    value: RelationalDatabaseParameterValue,
) -> RelationalDatabaseParameterSpec {
    RelationalDatabaseParameterSpec {
        parameter_name,
        value,
        allowed_values: Some("1-65535"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        description: "a parameter",
        is_modifiable: true,
    }
}

/// RULE: a page is the slice from the token onward, bounded by the page size,
/// with `nextPageToken` naming the first entry after it — over page sizes and
/// sets the published catalog does not have.
///
/// Every published parameter set fits in one page, so the wire cases above
/// cannot exercise a second page; these rows put the rule's own dimensions under
/// test: how many entries the set holds, how big a page is, and which entry the
/// token names.
#[test]
fn a_page_is_the_set_from_the_token_onward_bounded_by_the_page_size() {
    let set = [
        spec("alpha", RelationalDatabaseParameterValue::EngineDefault),
        spec("beta", RelationalDatabaseParameterValue::Fixed("UTC")),
        spec("gamma", RelationalDatabaseParameterValue::EngineDefault),
        spec("delta", RelationalDatabaseParameterValue::EngineDefault),
        spec("epsilon", RelationalDatabaseParameterValue::EngineDefault),
    ];

    /// One row: a set, a page size and a token, against the names the page must
    /// carry and the token that must follow it — or `None` when the token names
    /// no entry of the set.
    struct Row {
        case: &'static str,
        specs: usize,
        page_size: usize,
        token: Option<&'static str>,
        page: Option<(&'static [&'static str], Option<&'static str>)>,
    }

    let rows: &[Row] = &[
        Row {
            case: "an empty set",
            specs: 0,
            page_size: 2,
            token: None,
            page: Some((&[], None)),
        },
        Row {
            case: "an empty set with a token",
            specs: 0,
            page_size: 2,
            token: Some("alpha"),
            page: None,
        },
        Row {
            case: "the first page of five in pages of two",
            specs: 5,
            page_size: 2,
            token: None,
            page: Some((&["alpha", "beta"], Some("gamma"))),
        },
        Row {
            case: "the middle page of five in pages of two",
            specs: 5,
            page_size: 2,
            token: Some("gamma"),
            page: Some((&["gamma", "delta"], Some("epsilon"))),
        },
        Row {
            case: "the last page of five in pages of two",
            specs: 5,
            page_size: 2,
            token: Some("epsilon"),
            page: Some((&["epsilon"], None)),
        },
        Row {
            case: "pages of one",
            specs: 3,
            page_size: 1,
            token: Some("beta"),
            page: Some((&["beta"], Some("gamma"))),
        },
        Row {
            case: "a page larger than the set",
            specs: 3,
            page_size: 50,
            token: None,
            page: Some((&["alpha", "beta", "gamma"], None)),
        },
        Row {
            case: "a page size of zero still advances",
            specs: 3,
            page_size: 0,
            token: None,
            page: Some((&["alpha"], Some("beta"))),
        },
        Row {
            case: "an empty token asks for the first page",
            specs: 3,
            page_size: 2,
            token: Some(""),
            page: Some((&["alpha", "beta"], Some("gamma"))),
        },
        Row {
            case: "a token naming no entry of the set",
            specs: 3,
            page_size: 2,
            token: Some("omega"),
            page: None,
        },
        Row {
            case: "a token naming an entry outside the set",
            specs: 2,
            page_size: 2,
            token: Some("gamma"),
            page: None,
        },
    ];

    for row in rows {
        let specs = &set[..row.specs];
        let answered = relational_database_parameter_page(specs, 1.0, row.token, row.page_size);
        match row.page {
            None => assert!(
                answered.is_none(),
                "case {}: a token naming no entry has no page, got {answered:?}",
                row.case
            ),
            Some((want_names, want_token)) => {
                let answered = answered.unwrap_or_else(|| {
                    panic!("case {}: expected a page, got none", row.case)
                });
                let got: Vec<String> = answered
                    .parameters
                    .iter()
                    .map(|parameter| parameter.parameter_name.clone().unwrap_or_default())
                    .collect();
                assert_eq!(
                    got,
                    want_names
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect::<Vec<_>>(),
                    "case {}: expected this page's entries, got {got:?}",
                    row.case
                );
                assert_eq!(
                    answered.next_page_token.as_deref(),
                    want_token,
                    "case {}: expected nextPageToken {want_token:?}, got {:?}",
                    row.case,
                    answered.next_page_token
                );
            }
        }
    }
}

/// RULE: a memory-sized value is the published formula resolved against the
/// hardware — including at the ceiling the formula carries, and on hardware
/// beyond what the plan catalog publishes today.
#[test]
fn a_memory_sized_value_resolves_the_published_formula_against_the_hardware() {
    /// One row: the formula, the RAM it is resolved against, and the value it
    /// must come to.
    struct Row {
        case: &'static str,
        value: RelationalDatabaseParameterValue,
        ram_size_in_gb: f32,
        want: Option<&'static str>,
    }

    // The two PostgreSQL rows bracket the published ceiling of
    // `LEAST({DBInstanceClassMemory/9531392},5000)`: one connection per
    // 9_531_392 bytes reaches 5000 just above 44 GB of RAM, so 44 GB is below
    // the ceiling and 45 GB is held at it.
    let rows: &[Row] = &[
        Row {
            case: "a parameter the group leaves at the engine default",
            value: RelationalDatabaseParameterValue::EngineDefault,
            ram_size_in_gb: 4.0,
            want: None,
        },
        Row {
            case: "a value the group pins",
            value: RelationalDatabaseParameterValue::Fixed("UTC"),
            ram_size_in_gb: 4.0,
            want: Some("UTC"),
        },
        Row {
            case: "three quarters of 1 GB, in bytes",
            value: RelationalDatabaseParameterValue::FromMemory {
                numerator: 3,
                denominator: 4,
                cap: None,
            },
            ram_size_in_gb: 1.0,
            want: Some("805306368"),
        },
        Row {
            case: "three quarters of 8 GB, in bytes",
            value: RelationalDatabaseParameterValue::FromMemory {
                numerator: 3,
                denominator: 4,
                cap: None,
            },
            ram_size_in_gb: 8.0,
            want: Some("6442450944"),
        },
        Row {
            case: "1 GB of 8 kB shared buffers",
            value: RelationalDatabaseParameterValue::FromMemory {
                numerator: 1,
                denominator: 32_768,
                cap: None,
            },
            ram_size_in_gb: 1.0,
            want: Some("32768"),
        },
        Row {
            case: "a half-gigabyte bundle, where the plan RAM is fractional",
            value: RelationalDatabaseParameterValue::FromMemory {
                numerator: 1,
                denominator: 32_768,
                cap: None,
            },
            ram_size_in_gb: 0.5,
            want: Some("16384"),
        },
        Row {
            case: "below the published connection ceiling",
            value: RelationalDatabaseParameterValue::FromMemory {
                numerator: 1,
                denominator: 9_531_392,
                cap: Some(5000),
            },
            ram_size_in_gb: 44.0,
            want: Some("4956"),
        },
        Row {
            case: "at the published connection ceiling",
            value: RelationalDatabaseParameterValue::FromMemory {
                numerator: 1,
                denominator: 9_531_392,
                cap: Some(5000),
            },
            ram_size_in_gb: 45.0,
            want: Some("5000"),
        },
    ];

    for row in rows {
        let got = row.value.resolve(row.ram_size_in_gb);
        assert_eq!(
            got.as_deref(),
            row.want,
            "case {}: expected {:?} on {} GB of RAM, got {got:?}",
            row.case,
            row.want,
            row.ram_size_in_gb
        );
    }
}
