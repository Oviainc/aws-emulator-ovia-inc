// SPDX-License-Identifier: CC0-1.0

//! `GetResourceMetadata`'s answer against the MODELLED bounds of its shapes.
//!
//! RULE under test: whatever status the projection decides, every entry of the
//! answer's `Features` map is a value the model admits — the key is a
//! `FeatureMetadataMap` key (a `String` of length 0..256 matching `\S`, so no
//! blank or oversize name), and the value's `Status` is a member of the
//! `FeatureStatus` enum — and the `Identifier` the answer echoes is the one the
//! request addressed, inside the modelled `len 0..256, pattern \S` bound.
//!
//! This is the shape half of the contract, which the value half
//! (`tests/all/resource_features.rs`, and across services
//! `core/router/tests/all/pi_resource_features.rs`) does not pin: those suites
//! assert WHICH status each feature gets for a configuration, this one asserts
//! that no answer can carry a key or a status the model does not define,
//! whatever the directory holds. The catalogue it checks against is the
//! GENERATED one (`ops::types::shared_types::FEATURE_STATUS_VALUES`), not a list
//! retyped here, so a change to the modelled enum cannot leave this assertion
//! asserting a stale set.
//!
//! The rows span what the rule generalises over: both `ServiceType` values, a
//! data source with monitoring on and one with it off, one whose owner recorded
//! no configuration at all, and one the directory holds no record of — the four
//! ways a status is decided.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator_with_engine, register_data_source};

use service_com_amazonaws_pi_performanceinsightsv20180227::ops::types::shared_types::FEATURE_STATUS_VALUES;
use service_com_amazonaws_pi_performanceinsightsv20180227::state::FEATURE_UNKNOWN;

/// `FeatureMetadataMap`'s key is a `String` with `length 0..256` and pattern
/// `\S`; `GetResourceMetadataOutput$Identifier` carries the same bound. Read
/// with `smithy_query(query="shape", shape="FeatureMetadataMap")`.
const MAX_MODELLED_STRING: usize = 256;

/// One data source this suite asks about: how the directory holds it, if at all.
struct Row {
    case: &'static str,
    /// The monitoring `ServiceType` the request addresses it under.
    service_type: &'static str,
    resource_id: &'static str,
    /// `None` leaves the id unregistered, so the directory holds no record.
    registration: Option<Registration>,
}

/// What a database control plane recorded for a data source.
struct Registration {
    engine: &'static str,
    /// `None` when the owner registered the id but recorded no monitoring
    /// configuration for it.
    monitoring: Option<(bool, Option<&'static str>)>,
}

const ROWS: &[Row] = &[
    Row {
        case: "an RDS instance monitored in advanced mode",
        service_type: "RDS",
        resource_id: "db-SHAPEADVANCED0000001",
        registration: Some(Registration {
            engine: "mysql",
            monitoring: Some((true, Some("advanced"))),
        }),
    },
    Row {
        case: "an RDS cluster with monitoring off",
        service_type: "RDS",
        resource_id: "cluster-SHAPEPIOFF02",
        registration: Some(Registration {
            engine: "aurora-postgresql",
            monitoring: Some((false, Some("standard"))),
        }),
    },
    Row {
        case: "a DocumentDB cluster monitored",
        service_type: "DOCDB",
        resource_id: "cluster-SHAPEDOCDB03",
        registration: Some(Registration {
            engine: "docdb",
            monitoring: Some((true, None)),
        }),
    },
    Row {
        case: "a registered id whose owner recorded no configuration",
        service_type: "RDS",
        resource_id: "db-SHAPENOCONFIG0000004",
        registration: Some(Registration {
            engine: "mysql",
            monitoring: None,
        }),
    },
    Row {
        case: "an id the directory holds no record of",
        service_type: "RDS",
        resource_id: "db-SHAPEUNREGISTERED05",
        registration: None,
    },
    Row {
        case: "an id the directory holds no record of, under DocumentDB",
        service_type: "DOCDB",
        resource_id: "cluster-SHAPENONE06",
        registration: None,
    },
];

/// Seed the shared data-source directory the way a database control plane does,
/// through this crate's ONE test-side seeding helper.
fn register(engine: &engine::InMemoryEngine, row: &Row) {
    let Some(registration) = row.registration.as_ref() else {
        return;
    };
    register_data_source(
        engine,
        row.resource_id,
        row.service_type,
        registration.engine,
        registration.monitoring.map(|(performance_insights_enabled, database_insights_mode)| {
            ::service_common::db_data_sources::MonitoringConfiguration {
                performance_insights_enabled,
                database_insights_mode: database_insights_mode.map(str::to_string),
            }
        }),
    );
}

/// RULE: every key and every `Status` of the answer's `Features` map is a value
/// the model admits, and the echoed `Identifier` is the addressed one.
#[tokio::test]
async fn every_reported_feature_is_within_the_modelled_shapes() {
    let (emulator, engine) = emulator_with_engine();
    for row in ROWS {
        register(&engine, row);
    }

    for row in ROWS {
        let case = row.case;
        let answer = call(
            &emulator,
            "GetResourceMetadata",
            json!({"ServiceType": row.service_type, "Identifier": row.resource_id}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let identifier = answer.member("Identifier");
        assert_eq!(
            identifier.as_str(),
            Some(row.resource_id),
            "case {case}: expected the addressed Identifier echoed, got {:?}",
            answer.body
        );
        let echoed = identifier.as_str().unwrap_or_default();
        assert!(
            !echoed.trim().is_empty() && echoed.len() <= MAX_MODELLED_STRING,
            "case {case}: expected an Identifier inside the modelled bound, got {echoed:?}"
        );

        let features = answer
            .member("Features")
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !features.is_empty(),
            "case {case}: expected at least one Features entry, got {:?}",
            answer.body
        );
        // A row whose owner recorded a configuration is DECIDABLE, so the answer
        // cannot be the undecided one all the way across: that keeps this suite
        // from passing a projection that reported `UNKNOWN` for everything while
        // still carrying modelled keys and statuses.
        if row.registration.as_ref().is_some_and(|registration| registration.monitoring.is_some()) {
            let statuses: Vec<&str> = features
                .values()
                .map(|metadata| metadata.get("Status").and_then(Value::as_str).unwrap_or_default())
                .collect();
            assert!(
                statuses.iter().any(|status| *status != FEATURE_UNKNOWN),
                "case {case}: a recorded configuration is decidable, so expected at least one \
                 status other than {FEATURE_UNKNOWN}, got {statuses:?}"
            );
        }

        for (name, metadata) in &features {
            // The modelled map key: a String of length 0..256 matching `\S`, so
            // a blank or oversize feature name is not a value AWS could return.
            assert!(
                !name.trim().is_empty()
                    && name.len() <= MAX_MODELLED_STRING
                    && !name.chars().any(char::is_whitespace),
                "case {case}: expected a modelled FeatureMetadataMap key, got {name:?}"
            );
            let status = metadata.get("Status").and_then(Value::as_str);
            assert!(
                status.is_some_and(|status| FEATURE_STATUS_VALUES.contains(&status)),
                "case {case} / {name}: expected a member of the modelled FeatureStatus \
                 catalogue {FEATURE_STATUS_VALUES:?}, got {status:?}"
            );
        }
    }
}
