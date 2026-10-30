// SPDX-License-Identifier: CC0-1.0

//! The members a read of an Amazon Keyspaces resource ALWAYS serves, driven end to end
//! over the service's own `awsJson1_0` wire protocol through the harness in
//! [`crate::keyspace_table_lifecycle`] and read off the raw JSON body, so an absent member
//! is distinguishable from an empty or zero one.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A read of a resource serves every modelled member whose value the service always
//!    has, with its empty, zero or derived value rather than omitting it. For a table
//!    those are the comment (the empty message when the create gave none), the default TTL
//!    (`0` when nothing set one), the schema's `clusteringKeys` and `staticColumns` (empty
//!    lists when the schema declares neither), the Region-specific settings (the empty list
//!    for a table replicated in one Region, on `GetTable` and on
//!    `GetTableAutoScalingSettings` alike) and the warm throughput summary.
//! 2. The warm throughput summary is the configuration a call pre-warmed the table with,
//!    and otherwise the baseline the table's capacity mode already gives it: the capacity a
//!    provisioned table provisions, and the published on-demand baseline for a table in
//!    `PAY_PER_REQUEST` mode.
//! 3. `capacitySpecification.lastUpdateToPayPerRequestTimestamp` is served exactly for a
//!    table that has BEEN in `PAY_PER_REQUEST` mode, and records when it last entered it —
//!    the create, for a table made in that mode (the modelled default).
//! 4. `GetType`'s `maxNestingDepth` counts the type ITSELF, so a type no field of which
//!    reaches another user-defined type is one level deep and each level of nesting adds
//!    one — whichever form the field type reaches the nested type through.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_ok, reported_schema, settle_wait, Emulator, REGION,
};

/// A Region other than the request's own ([`REGION`]) a keyspace can be replicated in.
const OTHER_REGION: &str = "eu-west-1";

/// What a table in on-demand (`PAY_PER_REQUEST`) capacity mode can serve without being
/// pre-warmed, which is the warm throughput it reports when nothing configured one:
/// Amazon Keyspaces documents that such a table can serve up to 12,000 read request units
/// and 4,000 write request units per second.
const ON_DEMAND_WARM_READ_UNITS: i64 = 12_000;
/// The write half of that documented baseline. See [`ON_DEMAND_WARM_READ_UNITS`].
const ON_DEMAND_WARM_WRITE_UNITS: i64 = 4_000;

/// `GetTable` for one table, as a 200 body.
async fn get_table(emulator: &Emulator, case: &str, keyspace: &str, table: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    )
}

/// `GetTableAutoScalingSettings` for one table, as a 200 body.
async fn get_auto_scaling(emulator: &Emulator, case: &str, keyspace: &str, table: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    )
}

/// One row of RULES 1-3: a table to create, and what the two reads of it must then serve.
struct Case {
    /// The case name, which is also the keyspace name and the table name.
    name: &'static str,
    /// The Regions the owning keyspace is replicated in, or `None` for the default
    /// single-Region strategy.
    regions: Option<&'static [&'static str]>,
    /// The optional members the `CreateTable` request carries, as a client sends them: a
    /// member this does not name is one the request leaves out entirely.
    create: fn() -> Value,
    /// The `comment` the read must serve.
    comment: fn() -> Value,
    /// The `defaultTimeToLive` the read must serve.
    default_time_to_live: i64,
    /// The `(readUnitsPerSecond, writeUnitsPerSecond)` of the warm throughput summary.
    warm_units: (i64, i64),
    /// Whether the capacity summary must carry `lastUpdateToPayPerRequestTimestamp`.
    pay_per_request_stamp: bool,
}

/// RULES 1, 2 and 3 over a set of creates.
///
/// The rows differ along what the rules generalise over: whether the create names a comment
/// (absent, a description, and the empty description a client may send explicitly), whether
/// it sets a default TTL (absent, the bottom and the top of its modelled `0..630720000`
/// range), which of the schema's optional column lists it declares (neither, clustering
/// keys only, both), the capacity mode (absent — the modelled default — explicit
/// `PAY_PER_REQUEST`, and `PROVISIONED` at differing units), whether the create pre-warms
/// the table, and the owning keyspace's replication strategy (single- and multi-Region).
#[tokio::test]
async fn a_read_of_a_table_serves_every_member_the_service_always_has() {
    let multi_region: &[&str] = &[REGION, OTHER_REGION];

    let cases: Vec<Case> = vec![
        Case {
            // Nothing optional at all: every always-served member is its empty, zero or
            // derived value, and the modelled default mode is PAY_PER_REQUEST.
            name: "defaults_only",
            regions: None,
            create: || json!({}),
            comment: || json!({"message": ""}),
            default_time_to_live: 0,
            warm_units: (ON_DEMAND_WARM_READ_UNITS, ON_DEMAND_WARM_WRITE_UNITS),
            pay_per_request_stamp: true,
        },
        Case {
            // The same table in the mode named explicitly, with a comment and the top of
            // the defaultTimeToLive range.
            name: "explicit_on_demand_with_comment",
            regions: None,
            create: || {
                json!({
                    "capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"},
                    "comment": {"message": "orders by customer"},
                    "defaultTimeToLive": 630_720_000,
                })
            },
            comment: || json!({"message": "orders by customer"}),
            default_time_to_live: 630_720_000,
            warm_units: (ON_DEMAND_WARM_READ_UNITS, ON_DEMAND_WARM_WRITE_UNITS),
            pay_per_request_stamp: true,
        },
        Case {
            // A comment whose message is empty, and a defaultTimeToLive of 0, sent
            // EXPLICITLY: the serialized body is the same as for the table that named
            // neither, so a client cannot tell "unset" from "set to the empty value".
            name: "empty_values_sent_explicitly",
            regions: None,
            create: || json!({"comment": {"message": ""}, "defaultTimeToLive": 0}),
            comment: || json!({"message": ""}),
            default_time_to_live: 0,
            warm_units: (ON_DEMAND_WARM_READ_UNITS, ON_DEMAND_WARM_WRITE_UNITS),
            pay_per_request_stamp: true,
        },
        Case {
            // PROVISIONED at the bottom of the capacity units' modelled `1..` range: the
            // warm throughput baseline is what the table provisions, and the table has
            // never been in PAY_PER_REQUEST mode.
            name: "provisioned_at_the_unit_minimum",
            regions: None,
            create: || {
                json!({"capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 1,
                    "writeCapacityUnits": 1,
                }})
            },
            comment: || json!({"message": ""}),
            default_time_to_live: 0,
            warm_units: (1, 1),
            pay_per_request_stamp: false,
        },
        Case {
            // PROVISIONED with differing read and write units, so the baseline is not one
            // number used twice.
            name: "provisioned_asymmetric_units",
            regions: None,
            create: || {
                json!({
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 4_000,
                        "writeCapacityUnits": 90,
                    },
                    "defaultTimeToLive": 86_400,
                })
            },
            comment: || json!({"message": ""}),
            default_time_to_live: 86_400,
            warm_units: (4_000, 90),
            pay_per_request_stamp: false,
        },
        Case {
            // A pre-warmed table reports ITS configuration rather than any baseline.
            name: "pre_warmed_table",
            regions: None,
            create: || {
                json!({
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 5,
                        "writeCapacityUnits": 5,
                    },
                    "warmThroughputSpecification": {
                        "readUnitsPerSecond": 24_000,
                        "writeUnitsPerSecond": 9_000,
                    },
                })
            },
            comment: || json!({"message": ""}),
            default_time_to_live: 0,
            warm_units: (24_000, 9_000),
            pay_per_request_stamp: false,
        },
        Case {
            // A multi-Region keyspace: the Region-specific settings are a non-empty list,
            // so the empty list the single-Region rows expect is the Region count and not
            // a blanket answer.
            name: "multi_region_table",
            regions: Some(multi_region),
            create: || {
                json!({
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 7,
                        "writeCapacityUnits": 3,
                    },
                    // Every table of a replicated keyspace "must use client-side timestamps
                    // for conflict resolution".
                    "clientSideTimestamps": {"status": "ENABLED"},
                })
            },
            comment: || json!({"message": ""}),
            default_time_to_live: 0,
            warm_units: (7, 3),
            pay_per_request_stamp: false,
        },
    ];

    // The schemas the rows are created with, one per shape of the two optional column
    // lists: neither declared, clustering keys only, and both.
    let schemas: [Value; 3] = [
        json!({
            "allColumns": [{"name": "pk", "type": "text"}, {"name": "payload", "type": "int"}],
            "partitionKeys": [{"name": "pk"}],
        }),
        json!({
            "allColumns": [
                {"name": "pk", "type": "text"},
                {"name": "ck", "type": "timestamp"},
                {"name": "payload", "type": "int"},
            ],
            "partitionKeys": [{"name": "pk"}],
            "clusteringKeys": [{"name": "ck", "orderBy": "DESC"}],
        }),
        json!({
            "allColumns": [
                {"name": "pk", "type": "text"},
                {"name": "ck", "type": "timestamp"},
                {"name": "shared", "type": "int"},
            ],
            "partitionKeys": [{"name": "pk"}],
            "clusteringKeys": [{"name": "ck", "orderBy": "ASC"}],
            "staticColumns": [{"name": "shared"}],
        }),
    ];

    let emulator = Emulator::start();

    for (index, case) in cases.iter().enumerate() {
        let mut keyspace_request = json!({"keyspaceName": case.name});
        if let Some(regions) = case.regions {
            keyspace_request["replicationSpecification"] =
                json!({"replicationStrategy": "MULTI_REGION", "regionList": regions});
        }
        assert_ok(
            case.name,
            emulator.call("CreateKeyspace", keyspace_request).await,
        );

        let mut request = (case.create)();
        request["keyspaceName"] = json!(case.name);
        request["tableName"] = json!(case.name);
        // Each row takes the next schema shape in turn, so the two optional column lists
        // are exercised across the rows rather than only on one of them.
        request["schemaDefinition"] = schemas[index % schemas.len()].clone();
        assert_ok(case.name, emulator.call("CreateTable", request.clone()).await);
    }

    // A create is believed only once the reader reports it back, and only once the clock
    // has settled the table into service.
    tokio::time::sleep(settle_wait()).await;

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let report = get_table(&emulator, name, name, name).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {name:?}: the creation span has elapsed, so the table is ACTIVE, got {report}"
        );

        // RULE 1: the comment and the default TTL are served for every table.
        assert_eq!(
            report["comment"],
            (case.comment)(),
            "case {name:?}: expected the comment {}, got {}",
            (case.comment)(),
            report["comment"]
        );
        assert_eq!(
            report["defaultTimeToLive"],
            json!(case.default_time_to_live),
            "case {name:?}: expected defaultTimeToLive {}, got {}",
            case.default_time_to_live,
            report["defaultTimeToLive"]
        );

        // RULE 1: the schema's two optional column lists are served, empty where the
        // create declared neither.
        let expected_schema = reported_schema(&schemas[index % schemas.len()]);
        assert_eq!(
            report["schemaDefinition"], expected_schema,
            "case {name:?}: expected the schema {expected_schema}, got {}",
            report["schemaDefinition"]
        );
        for member in ["clusteringKeys", "staticColumns"] {
            assert!(
                report["schemaDefinition"][member].is_array(),
                "case {name:?}: schemaDefinition.{member} is served as a list for every \
                 table, got {report}"
            );
        }

        // RULE 2: the warm throughput summary, for the table and for each Region it is
        // replicated in.
        let (read_units, write_units) = case.warm_units;
        let expected_warm = json!({
            "readUnitsPerSecond": read_units,
            "writeUnitsPerSecond": write_units,
            "status": keyspaces::WARM_THROUGHPUT_STATUS_AVAILABLE,
        });
        assert_eq!(
            report["warmThroughputSpecification"], expected_warm,
            "case {name:?}: expected the warm throughput {expected_warm}, got {}",
            report["warmThroughputSpecification"]
        );

        // RULE 1: the Region-specific settings, on both reads of the table — one entry per
        // Region the owning keyspace replicates the table to, and the empty list for a
        // table replicated in one Region.
        let scaling = get_auto_scaling(&emulator, name, name, name).await;
        let expected_regions: Vec<String> = case
            .regions
            .unwrap_or_default()
            .iter()
            .map(|region| (*region).to_string())
            .collect();
        for (source, body) in [("GetTable", &report), ("GetTableAutoScalingSettings", &scaling)] {
            let entries = body["replicaSpecifications"].as_array().unwrap_or_else(|| {
                panic!(
                    "case {name:?}: {source} serves replicaSpecifications as a list for \
                     every table, got {body}"
                )
            });
            let got: Vec<String> = entries
                .iter()
                .map(|entry| entry["region"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                got, expected_regions,
                "case {name:?}: expected {source} to report {expected_regions:?}, got {got:?}"
            );
        }
        for entry in report["replicaSpecifications"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert_eq!(
                entry["warmThroughputSpecification"], expected_warm,
                "case {name:?}: warm throughput is a table setting, so each Region reports \
                 the table's, got {report}"
            );
        }

        // RULE 3: the stamp belongs to a table that has been in PAY_PER_REQUEST mode, and
        // for a table CREATED in that mode it is the creation instant.
        let stamp = report["capacitySpecification"]["lastUpdateToPayPerRequestTimestamp"].clone();
        assert_eq!(
            !stamp.is_null(),
            case.pay_per_request_stamp,
            "case {name:?}: lastUpdateToPayPerRequestTimestamp belongs to a table that has \
             been in PAY_PER_REQUEST mode (expected: {}), got {report}",
            case.pay_per_request_stamp
        );
        if case.pay_per_request_stamp {
            assert_eq!(
                stamp, report["creationTimestamp"],
                "case {name:?}: a table created in PAY_PER_REQUEST entered that mode when it \
                 was created, got {report}"
            );
        }

        // Serving these members is still a READ: asked again, the table answers
        // identically.
        let again = get_table(&emulator, name, name, name).await;
        assert_eq!(
            again, report,
            "case {name:?}: a second read answers the same body, got {again}"
        );
    }
}

/// RULE 4: `maxNestingDepth` counts the type itself, over a set of ways a field reaches a
/// nested type.
///
/// The rows differ along the form the field type reaches another user-defined type through
/// (none at all, bare, frozen, inside a list, as a map value, inside a tuple) and along how
/// deep the chain already is, so the depth is derived from the stored fields rather than
/// taken from one shape.
#[tokio::test]
async fn get_type_counts_the_type_itself_in_its_nesting_depth() {
    // (the type to create, its field types, the depth it must then report)
    let rows: [(&str, &[&str], i64); 7] = [
        // A type whose fields are all scalar: one level, the type itself.
        ("flat", &["text", "int"], 1),
        // One level of nesting, reached bare and frozen.
        ("wraps_bare", &["flat"], 2),
        ("wraps_frozen", &["frozen<flat>", "text"], 2),
        // Two levels, reached through a collection and through a map value.
        ("wraps_in_a_list", &["list<frozen<wraps_frozen>>"], 3),
        ("wraps_in_a_map", &["map<text, frozen<wraps_bare>>"], 3),
        // Three levels, reached through a tuple.
        ("wraps_in_a_tuple", &["tuple<int, frozen<wraps_in_a_list>>"], 4),
        // A type that reaches two chains of differing depth reports the DEEPEST.
        (
            "wraps_two_chains",
            &["frozen<flat>", "frozen<wraps_in_a_tuple>"],
            5,
        ),
    ];

    let emulator = Emulator::start();
    let keyspace = "nesting_depth_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    for (name, field_types, _) in &rows {
        let fields: Vec<Value> = field_types
            .iter()
            .enumerate()
            .map(|(index, field_type)| json!({"name": format!("f{index}"), "type": field_type}))
            .collect();
        assert_ok(
            name,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": keyspace,
                        "typeName": name,
                        "fieldDefinitions": fields,
                    }),
                )
                .await,
        );
        // A type is referable by the next row only once it is ACTIVE.
        tokio::time::sleep(settle_wait()).await;
    }

    for (name, field_types, depth) in &rows {
        let got = assert_ok(
            name,
            emulator
                .call("GetType", json!({"keyspaceName": keyspace, "typeName": name}))
                .await,
        );
        assert_eq!(
            got["maxNestingDepth"],
            json!(depth),
            "case {name:?}: the fields {field_types:?} nest {depth} level(s) deep, got {got}"
        );
    }
}
