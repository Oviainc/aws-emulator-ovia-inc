// SPDX-License-Identifier: CC0-1.0

//! DB parameter groups, end to end over the wire.
//!
//! The suite drives real `awsJson1_0` requests through the same [`MultiServiceRouter`]
//! the production router dispatches with, reusing the harness in
//! [`crate::db_instance_lifecycle`] (`router`, `call`, `call_in`, `ok`) so there is one
//! wire harness for this crate.
//!
//! The assertions are RULES over a table of inputs, not a replay of one recorded call:
//! `CreateDbParameterGroup` persists a new group and reports the stored record — a
//! service-generated `id`, the ARN derived from that id and the request's region, and
//! every member the input and output shapes share echoed unchanged — a group name is
//! unique per region, and `GetDbParameterGroup` reports that same record by id. The rows
//! vary the name's length and form, the presence and size of the description, which
//! `Parameters` union member is supplied (and which union member inside it), the number
//! of tags and the region.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::db_instance_lifecycle::{call, call_in, ok, router, ACCOUNT, REGION};

/// Members that `CreateDbParameterGroupInput` and `CreateDbParameterGroupOutput` BOTH
/// model, from `smithy_query(query="operation", operation="CreateDbParameterGroup")`. A
/// value the request supplies for one of these must come back from the stored record
/// unchanged; `tags` is modelled on the input only, so nothing echoes it.
const ECHOED_CREATE_MEMBERS: &[&str] = &["description", "name", "parameters"];

/// Members of `GetDbParameterGroupOutput`, from
/// `smithy_query(query="operation", operation="GetDbParameterGroup")`: the same record
/// `CreateDbParameterGroup` answers with, so a read-back must agree member for member.
const RECORD_MEMBERS: &[&str] = &["arn", "description", "id", "name", "parameters"];

/// A `DbParameterGroupId` as the model constrains it: length 3..64, pattern
/// `^[a-zA-Z0-9]+$` (`smithy_query(query="shape", shape="DbParameterGroupId")`).
fn assert_well_formed_id(case: &str, id: &str) {
    assert!(
        (3..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a DbParameterGroupId (3..64 alphanumerics), got {id:?}"
    );
}

/// The `id` in a response body.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string id, got {body}"))
        .to_string()
}

/// One `InfluxDBv2` parameter set: an enum member, booleans, an integer, a long and a
/// `Duration` (`durationType` from `DURATION_TYPE_VALUES`, `logLevel` from `LogLevel`).
fn influxdb_v2_parameters() -> Value {
    json!({
        "InfluxDBv2": {
            "fluxLogEnabled": true,
            "logLevel": "debug",
            "noTasks": false,
            "queryConcurrency": 12,
            "influxqlMaxSelectBuckets": 4096,
            "tracingType": "jaeger",
            "httpIdleTimeout": { "durationType": "minutes", "value": 3 },
            "storageRetentionCheckInterval": { "durationType": "seconds", "value": 90 }
        }
    })
}

/// The create requests this suite spans: one row per shape of the input the rule has to
/// hold over.
fn create_cases() -> Vec<(String, Value)> {
    // `DbParameterGroupName` is length 3..64, pattern
    // `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$`; `description` is length 0..500.
    let name_at_max = format!("A{}", "b".repeat(63));
    let description_at_max = "d".repeat(500);

    vec![
        (
            "name only, at the modelled minimum length 3".to_string(),
            json!({ "name": "Abc" }),
        ),
        (
            "name at the modelled maximum length 64".to_string(),
            json!({ "name": name_at_max }),
        ),
        (
            "hyphenated name and a description at the modelled maximum length 500"
                .to_string(),
            json!({ "name": "pg-with-hyphens-9", "description": description_at_max }),
        ),
        (
            "an empty description, the modelled minimum length 0".to_string(),
            json!({ "name": "EmptyDescription", "description": "" }),
        ),
        (
            "InfluxDBv2 parameters: enum, booleans, numbers and durations".to_string(),
            json!({
                "name": "v2-parameters",
                "description": "InfluxDB v2 tuning",
                "parameters": influxdb_v2_parameters(),
            }),
        ),
        (
            "InfluxDBv3Core parameters with the percent union member".to_string(),
            json!({
                "name": "v3core-percent",
                "parameters": {
                    "InfluxDBv3Core": {
                        "queryFileLimit": 432,
                        "logFormat": "full",
                        "forceSnapshotMemThreshold": { "percent": "70%" },
                        "parquetMemCachePrunePercentage": 0.5,
                        "gen1Duration": { "durationType": "minutes", "value": 5 }
                    }
                },
            }),
        ),
        (
            "InfluxDBv3Core parameters with the absolute union member".to_string(),
            json!({
                "name": "v3core-absolute",
                "parameters": {
                    "InfluxDBv3Core": {
                        "execMemPoolBytes": { "absolute": 8000000000i64 },
                        "walReplayFailOnError": true,
                        "dataFusionRuntimeThreadPriority": -20
                    }
                },
            }),
        ),
        (
            "InfluxDBv3Enterprise parameters with its required members".to_string(),
            json!({
                "name": "v3-enterprise",
                "parameters": {
                    "InfluxDBv3Enterprise": {
                        "ingestQueryInstances": 2,
                        "queryOnlyInstances": 1,
                        "dedicatedCompactor": true,
                        "compactionRowLimit": 1000,
                        "replicationInterval": { "durationType": "seconds", "value": 30 }
                    }
                },
            }),
        ),
        (
            "one tag, the modelled minimum map size 1".to_string(),
            json!({ "name": "OneTag", "tags": { "team": "data" } }),
        ),
        (
            "several tags, one of them with the empty value the model allows".to_string(),
            json!({
                "name": "ManyTags",
                "description": "tagged",
                "parameters": influxdb_v2_parameters(),
                "tags": { "team": "data", "env": "test", "owner": "" },
            }),
        ),
    ]
}

#[tokio::test]
async fn create_reports_the_stored_record_and_get_reads_it_back() {
    let router = router();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for (case, request) in create_cases() {
        let created = ok(
            &case,
            call(&router, "CreateDbParameterGroup", request.clone()).await,
        );

        let id = id_of(&case, &created);
        assert_well_formed_id(&case, &id);
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: the service-generated id {id:?} is not unique"
        );

        assert_eq!(
            created["arn"].as_str(),
            Some(
                format!("arn:aws:timestream-influxdb:{REGION}:{ACCOUNT}:db-parameter-group/{id}")
                    .as_str()
            ),
            "case {case:?}: unexpected arn {:?}",
            created["arn"]
        );

        // Every member both shapes model comes back exactly as the request gave it, and
        // a member the request omitted is reported by nobody.
        for member in ECHOED_CREATE_MEMBERS {
            assert_eq!(
                &created[*member], &request[*member],
                "case {case:?}: member {member:?} must echo the request, got {:?}",
                created[*member]
            );
        }

        // `tags` is an input-only member: no parameter-group output shape carries it.
        assert!(
            created["tags"].is_null(),
            "case {case:?}: the output models no tags member, got {:?}",
            created["tags"]
        );

        // Round trip: the reader answers the same record, member for member.
        let fetched = ok(
            &case,
            call(&router, "GetDbParameterGroup", json!({ "identifier": &id })).await,
        );
        for member in RECORD_MEMBERS {
            assert_eq!(
                &fetched[*member], &created[*member],
                "case {case:?}: GetDbParameterGroup disagrees on {member:?}: \
                 expected {:?}, got {:?}",
                created[*member], fetched[*member]
            );
        }

        // A read changes nothing a later caller sees.
        let again = ok(
            &case,
            call(&router, "GetDbParameterGroup", json!({ "identifier": &id })).await,
        );
        assert_eq!(
            again, fetched,
            "case {case:?}: a second read must report the same record, got {again}"
        );
    }
}

#[tokio::test]
async fn a_name_is_unique_per_region() {
    let router = router();
    // One short, one hyphenated, one at the modelled maximum length: the rule is about
    // the name being taken, not about its form.
    let names = ["Abc", "pg-shared-name", &format!("Z{}", "9".repeat(63))].map(String::from);
    // A second region to prove the index is scoped: the same name there is a new group.
    let other_region = "eu-central-1";

    for name in names {
        let case = format!("name {name:?}");
        let first = ok(
            &case,
            call(&router, "CreateDbParameterGroup", json!({ "name": &name })).await,
        );
        let first_id = id_of(&case, &first);

        let (status, body) = call(
            &router,
            "CreateDbParameterGroup",
            json!({ "name": &name, "description": "a second attempt" }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate name must conflict, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ConflictException"),
            "case {case:?}: expected a ConflictException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(first_id.as_str()),
            "case {case:?}: the conflict must name the existing group, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("DB_PARAMETER_GROUP"),
            "case {case:?}: unexpected resourceType in {body}"
        );

        // The refused create stored nothing: the first record is untouched.
        let unchanged = ok(
            &case,
            call(
                &router,
                "GetDbParameterGroup",
                json!({ "identifier": &first_id }),
            )
            .await,
        );
        assert_eq!(
            unchanged, first,
            "case {case:?}: the conflicting create must not touch the stored record, \
             got {unchanged}"
        );

        // The same name in another region is a different group, with that region's ARN.
        let elsewhere = ok(
            &case,
            call_in(
                &router,
                other_region,
                "CreateDbParameterGroup",
                json!({ "name": &name }),
            )
            .await,
        );
        let other_id = id_of(&case, &elsewhere);
        assert_ne!(
            other_id, first_id,
            "case {case:?}: a group created in another region must get its own id"
        );
        assert_eq!(
            elsewhere["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:timestream-influxdb:{other_region}:{ACCOUNT}\
                     :db-parameter-group/{other_id}"
                )
                .as_str()
            ),
            "case {case:?}: unexpected arn {:?}",
            elsewhere["arn"]
        );
    }
}

#[tokio::test]
async fn getting_an_identifier_nothing_created_is_not_found() {
    let router = router();
    let created = ok(
        "a live group",
        call(
            &router,
            "CreateDbParameterGroup",
            json!({ "name": "LiveGroup" }),
        )
        .await,
    );
    let live_id = id_of("a live group", &created);

    // `identifier` is the group's id, pattern `^[a-zA-Z0-9]+$`: a name is not an
    // identifier, and neither is the id of nothing.
    let unknown = [
        ("an id nothing minted", "abc123def4".to_string()),
        ("the id at the modelled minimum length", "zzz".to_string()),
        ("the id at the modelled maximum length", "f".repeat(64)),
        ("the group's NAME instead of its id", "LiveGroup".to_string()),
    ];

    for (case, identifier) in unknown {
        let (status, body) = call(
            &router,
            "GetDbParameterGroup",
            json!({ "identifier": &identifier }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(identifier.as_str()),
            "case {case:?}: the error must name the identifier asked for, got {body}"
        );
    }

    // The live group is untouched by all of that.
    let still_there = ok(
        "the live group survives",
        call(
            &router,
            "GetDbParameterGroup",
            json!({ "identifier": &live_id }),
        )
        .await,
    );
    assert_eq!(
        still_there, created,
        "case \"the live group survives\": expected the stored record, got {still_there}"
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let router = router();
    // `DbParameterGroupName`: length 3..64, pattern
    // `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$`; `description`: length 0..500.
    let name_past_max = format!("A{}", "b".repeat(64));
    let description_past_max = "d".repeat(501);

    let cases: Vec<(&str, Value)> = vec![
        (
            "a name one shorter than the modelled minimum 3",
            json!({ "name": "ab" }),
        ),
        (
            "a name one longer than the modelled maximum 64",
            json!({ "name": name_past_max }),
        ),
        (
            "a name that does not start with a letter",
            json!({ "name": "9starts-with-a-digit" }),
        ),
        (
            "a name with two consecutive hyphens",
            json!({ "name": "pg--double" }),
        ),
        (
            "a name ending in a hyphen",
            json!({ "name": "pg-trailing-" }),
        ),
        (
            "a description one longer than the modelled maximum 500",
            json!({ "name": "LongDescription", "description": description_past_max }),
        ),
        (
            "a request missing the required name",
            json!({ "description": "no name at all" }),
        ),
        (
            "an empty tag map, below the modelled minimum size 1",
            json!({ "name": "NoTags", "tags": {} }),
        ),
    ];

    for (case, body) in cases {
        let (status, answer) = call(&router, "CreateDbParameterGroup", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
    }

    // Nothing above was stored: a well-formed create under one of the refused names now
    // succeeds, which it could not do if the refusal had claimed the name.
    let created = ok(
        "the refusals stored nothing",
        call(
            &router,
            "CreateDbParameterGroup",
            json!({ "name": "LongDescription", "description": "short enough" }),
        )
        .await,
    );
    assert_eq!(
        created["description"].as_str(),
        Some("short enough"),
        "case \"the refusals stored nothing\": unexpected description in {created}"
    );
}
