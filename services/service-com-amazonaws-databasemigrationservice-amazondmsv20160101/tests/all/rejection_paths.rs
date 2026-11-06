// SPDX-License-Identifier: CC0-1.0

//! The refusal paths of the DMS slice this crate implements: which requests DMS
//! refuses, with which error code, and with which message.
//!
//! Three rules are asserted here, each over a SET of inputs rather than one request:
//!
//! 1. A member whose value has to name an entry in a catalog DMS publishes, or
//!    another resource the account holds, is resolved before anything is persisted;
//!    an unresolvable value is refused with `InvalidParameterValueException` and the
//!    service's own wording, and the resolution of a reference runs before the
//!    created resource's own identifier-uniqueness check.
//! 2. A refusal that reports a resource the caller named reports it as the caller
//!    sent it, in DMS's fixed sentence for that refusal, and does not substitute a
//!    resource ARN this emulator generated.
//! 3. A filtered describe whose `Filters` narrow it to nothing raises the error it
//!    models for an absent resource instead of answering an empty list; an
//!    unfiltered one still reports what the account holds, even when that is
//!    nothing.
//!
//! Every case drives the real wire path through the router, so what is asserted is
//! what a client sees, body and HTTP status included.

use serde_json::{json, Value};
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::state::{
    dms_arn, service_state, ReplicationSubnetGroupRecord, SubnetRecord,
};

use crate::migration_task_slice::{error_code, text, Emulator, REGION};

/// A replication instance class every case that is not about the class catalog uses.
const VALID_CLASS: &str = "dms.t3.medium";

/// The fixed text DMS answers a `ReplicationInstanceClass` that names no class with.
const INVALID_CLASS_MESSAGE: &str = "Invalid ReplicationInstance class";

/// The error code DMS answers a parameter whose value names nothing with. It is not
/// among the operation's modelled errors, so the recorded responses are its only
/// authority.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// Persist a replication subnet group under `identifier`, as
/// `CreateReplicationSubnetGroup` will once it is implemented (it is still a 501
/// stub), so the resolution of a `ReplicationSubnetGroupIdentifier` can be asserted
/// on a group that EXISTS as well as on one that does not.
fn persist_subnet_group(emulator: &Emulator, identifier: &str) {
    let state = service_state(emulator.engine()).expect("the dms store");
    state.lock().replication_subnet_groups.insert(
        identifier.to_string(),
        ReplicationSubnetGroupRecord {
            arn: dms_arn(REGION, "subgrp", identifier),
            identifier: identifier.to_string(),
            description: format!("subnets for {identifier}"),
            vpc_id: "vpc-0dms0000000000001".to_string(),
            subnets: vec![
                SubnetRecord {
                    identifier: "subnet-0dms0000000000001".to_string(),
                    availability_zone: format!("{REGION}a"),
                    ipv4: true,
                    ipv6: false,
                },
                SubnetRecord {
                    identifier: "subnet-0dms0000000000002".to_string(),
                    availability_zone: format!("{REGION}b"),
                    ipv4: true,
                    ipv6: false,
                },
            ],
            // This suite asserts nothing about a group's tags.
            tags: Vec::new(),
        },
    );
}

/// Every replication instance identifier the account holds, read back through the
/// unfiltered describe.
async fn instance_identifiers(emulator: &Emulator) -> Vec<String> {
    let described = emulator
        .ok("read the account's instances", "DescribeReplicationInstances", &json!({}))
        .await;
    described
        .pointer("/ReplicationInstances")
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get("ReplicationInstanceIdentifier")
                        .and_then(Value::as_str)
                        .unwrap_or("<missing>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `ReplicationInstanceClass` is a plain string in the model, so the classes DMS
/// offers come from the page its doc links — "Selecting the right DMS replication
/// instance for your migration" in the DMS user guide — of which
/// `CreateReplicationInstance$ReplicationInstanceClass` names `dms.c4.large` as an
/// example. Each row is `(case, class, accepted)`.
const CLASS_CASES: &[(&str, &str, bool)] = &[
    // Accepted: one row per generation the catalog carries, including sizes no step
    // of the recorded trace used.
    ("burstable, previous generation", "dms.t2.micro", true),
    ("burstable, current generation", "dms.t3.medium", true),
    ("the class the member doc names", "dms.c4.large", true),
    ("compute optimised", "dms.c5.9xlarge", true),
    ("the largest compute optimised", "dms.c6i.32xlarge", true),
    ("memory optimised", "dms.r5.24xlarge", true),
    ("memory optimised, intel", "dms.r6i.8xlarge", true),
    // Refused: a size the family is not offered in, a family DMS does not run, and
    // every way the `dms.<family>.<size>` form can be broken.
    ("a size that family is not offered in", "dms.r4.12xlarge", false),
    ("a family DMS does not run", "dms.x9.large", false),
    ("a size no family is offered in", "dms.t3.enormous", false),
    ("a retired-looking class", "dms.c3.large", false),
    ("no service prefix", "t3.medium", false),
    ("another service's prefix", "db.t3.medium", false),
    ("no size", "dms.t3", false),
    ("a trailing segment", "dms.t3.medium.large", false),
    ("uppercased", "DMS.T3.MEDIUM", false),
    ("empty", "", false),
];

#[tokio::test]
async fn create_replication_instance_resolves_its_instance_class_against_the_catalog() {
    let emulator = Emulator::new();

    for (index, (case, class, accepted)) in CLASS_CASES.iter().enumerate() {
        let identifier = format!("class-rep-{index}");
        let (status, response) = emulator
            .call(
                "CreateReplicationInstance",
                &json!({
                    "ReplicationInstanceIdentifier": identifier,
                    "ReplicationInstanceClass": class,
                }),
            )
            .await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({class:?}): expected the create to be accepted, got {status} \
                 {response}"
            );
            assert_eq!(
                text(&response, "/ReplicationInstance/ReplicationInstanceClass"),
                *class,
                "case {case:?}: expected the created instance to report the class the request \
                 carried, got {response}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&response),
                    text(&response, "/message").as_str()
                ),
                (
                    400,
                    INVALID_PARAMETER_VALUE.to_string(),
                    INVALID_CLASS_MESSAGE
                ),
                "case {case:?} ({class:?}): expected 400 {INVALID_PARAMETER_VALUE} \
                 {INVALID_CLASS_MESSAGE:?}, got {status} {response}"
            );
        }
    }

    // A refused create persists NOTHING: exactly the accepted rows are in the
    // account, and a refused identifier is still free afterwards.
    let held = instance_identifiers(&emulator).await;
    for (index, (case, class, accepted)) in CLASS_CASES.iter().enumerate() {
        let identifier = format!("class-rep-{index}");
        assert_eq!(
            held.contains(&identifier),
            *accepted,
            "case {case:?} ({class:?}): expected persisted={accepted}, and the account holds \
             {held:?}"
        );
    }

    // The identifier of a refused create is still free: the last row above was
    // refused, so creating it now with a class the catalog carries is accepted.
    let refused_index = CLASS_CASES.len() - 1;
    let reused = emulator
        .call(
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": format!("class-rep-{refused_index}"),
                "ReplicationInstanceClass": VALID_CLASS,
            }),
        )
        .await;
    assert_eq!(
        reused.0, 200,
        "case \"identifier of a refused create\": expected it to still be free, got {} {}",
        reused.0, reused.1
    );
}

#[tokio::test]
async fn create_replication_instance_resolves_its_subnet_group_before_its_own_identifier() {
    let emulator = Emulator::new();
    persist_subnet_group(&emulator, "probe-sng-0001");

    // The group's identifier is stored lowercased, so a request naming it in any
    // case resolves; an identifier no group carries does not, whatever its form.
    let cases: &[(&str, &str, bool)] = &[
        ("the group the account holds", "probe-sng-0001", true),
        ("the same group, uppercased", "PROBE-SNG-0001", true),
        ("a group the account does not hold", "probe-sng-0002", false),
        ("an unheld group, uppercased", "Probe-SNG-9999", false),
        ("a group named like a default", "default", false),
        ("a long unheld identifier", &"sng-0000000001".repeat(3), false),
    ];

    for (index, (case, group, resolves)) in cases.iter().enumerate() {
        let identifier = format!("sng-rep-{index}");
        let (status, response) = emulator
            .call(
                "CreateReplicationInstance",
                &json!({
                    "ReplicationInstanceIdentifier": identifier,
                    "ReplicationInstanceClass": VALID_CLASS,
                    "ReplicationSubnetGroupIdentifier": group,
                }),
            )
            .await;

        if *resolves {
            assert_eq!(
                status, 200,
                "case {case:?} ({group:?}): expected the create to be accepted, got {status} \
                 {response}"
            );
            // The create resolved the group, so the instance reports it — read back
            // out of the store rather than echoed from the request.
            assert_eq!(
                (
                    text(
                        &response,
                        "/ReplicationInstance/ReplicationSubnetGroup\
                         /ReplicationSubnetGroupIdentifier"
                    ),
                    text(
                        &response,
                        "/ReplicationInstance/ReplicationSubnetGroup\
                         /ReplicationSubnetGroupDescription"
                    ),
                ),
                (
                    "probe-sng-0001".to_string(),
                    "subnets for probe-sng-0001".to_string()
                ),
                "case {case:?}: expected the instance to report the persisted group, got \
                 {response}"
            );
        } else {
            // The message quotes the identifier the REQUEST carried, not a folded or
            // substituted form of it.
            let expected = format!("Replication Subnet Group '{group}' does not exist.");
            assert_eq!(
                (status, error_code(&response), text(&response, "/message")),
                (400, INVALID_PARAMETER_VALUE.to_string(), expected.clone()),
                "case {case:?}: expected 400 {INVALID_PARAMETER_VALUE} {expected:?}, got \
                 {status} {response}"
            );
        }
    }

    // A request that names no subnet group resolves nothing, and reports none.
    let without = emulator
        .ok(
            "no subnet group named",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "sng-rep-none",
                "ReplicationInstanceClass": VALID_CLASS,
            }),
        )
        .await;
    assert!(
        without
            .pointer("/ReplicationInstance/ReplicationSubnetGroup")
            .is_none(),
        "case \"no subnet group named\": expected no ReplicationSubnetGroup member, got {without}"
    );

    // Precedence: an unresolvable subnet group is reported even when the instance
    // identifier is ALSO contentious, and the uniqueness check still fires once the
    // group resolves.
    let (status, response) = emulator
        .call(
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "sng-rep-none",
                "ReplicationInstanceClass": VALID_CLASS,
                "ReplicationSubnetGroupIdentifier": "probe-sng-0002",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response), text(&response, "/message")),
        (
            400,
            INVALID_PARAMETER_VALUE.to_string(),
            "Replication Subnet Group 'probe-sng-0002' does not exist.".to_string()
        ),
        "case \"duplicate identifier and unheld subnet group\": expected the subnet group to be \
         reported first, got {status} {response}"
    );

    let (status, response) = emulator
        .call(
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "sng-rep-none",
                "ReplicationInstanceClass": VALID_CLASS,
                "ReplicationSubnetGroupIdentifier": "probe-sng-0001",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response)),
        (400, "ResourceAlreadyExistsFault".to_string()),
        "case \"duplicate identifier and a held subnet group\": expected the uniqueness check to \
         fire, got {status} {response}"
    );
}

#[tokio::test]
async fn create_replication_task_reports_an_unresolved_endpoint_arn_as_dms_words_it() {
    let emulator = Emulator::new();
    let instance_arn = emulator.available_instance("task-rep").await;
    let source_arn = emulator.endpoint("task-src", "source", "mysql").await;
    let target_arn = emulator.endpoint("task-tgt", "target", "postgres").await;

    // The same sentence whichever member carried the ARN, and whatever the ARN's
    // form: it names no endpoint, so it does not exist.
    let unknown_arns = [
        (
            "an endpoint ARN nothing minted",
            format!("arn:aws:dms:{REGION}:000000000000:endpoint:AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ),
        (
            "an endpoint ARN in another region",
            "arn:aws:dms:eu-west-1:000000000000:endpoint:ZZZZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        ),
        (
            "an endpoint ARN in another account",
            format!("arn:aws:dms:{REGION}:111111111111:endpoint:BBBBBBBBBBBBBBBBBBBBBBBBBB"),
        ),
        ("a replication instance's ARN", instance_arn.clone()),
        ("not an ARN at all", "task-src".to_string()),
    ];

    for (case, arn) in &unknown_arns {
        for member in ["SourceEndpointArn", "TargetEndpointArn"] {
            let mut request = json!({
                "ReplicationTaskIdentifier": "arn-probe-task",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": instance_arn,
                "MigrationType": "full-load",
                "TableMappings": "{\"rules\":[]}",
            });
            request[member] = json!(arn);

            let (status, response) = emulator.call("CreateReplicationTask", &request).await;
            let expected = format!("Endpoint ARN:{arn} does not exist");
            assert_eq!(
                (status, error_code(&response), text(&response, "/message")),
                (
                    400,
                    "ResourceNotFoundFault".to_string(),
                    expected.clone()
                ),
                "case {case:?} on {member}: expected 400 ResourceNotFoundFault {expected:?}, got \
                 {status} {response}"
            );
            assert!(
                !text(&response, "/message").contains(member),
                "case {case:?} on {member}: the message must not name the input member, got \
                 {response}"
            );
        }
    }
}

#[tokio::test]
async fn create_endpoint_reports_an_identifier_already_in_use_as_dms_words_it() {
    let emulator = Emulator::new();

    // The identifier the CALLER sent is what the message quotes, whatever the rest of
    // the second request asks for.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("short identifier", "ep", "source", "mysql"),
        ("hyphenated identifier", "probe-ep-36ad8d1ec3", "target", "postgres"),
        ("mixed case identifier", "Probe-EP-Mixed", "source", "oracle"),
        ("digits and hyphens", "ep-0001-0002", "target", "s3"),
        (
            "a long identifier",
            "ep-this-is-a-considerably-longer-endpoint-identifier-0001",
            "source",
            "sqlserver",
        ),
    ];

    for (case, identifier, endpoint_type, engine_name) in cases {
        let created = emulator
            .ok(
                case,
                "CreateEndpoint",
                &json!({
                    "EndpointIdentifier": identifier,
                    "EndpointType": endpoint_type,
                    "EngineName": engine_name,
                }),
            )
            .await;
        let existing_arn = text(&created, "/Endpoint/EndpointArn");

        // The second create differs from the first in every member but the
        // identifier, so what is refused is the identifier alone.
        let (status, response) = emulator
            .call(
                "CreateEndpoint",
                &json!({
                    "EndpointIdentifier": identifier,
                    "EndpointType": if *endpoint_type == "source" { "target" } else { "source" },
                    "EngineName": "mariadb",
                    "ServerName": "db.example.internal",
                    "Port": 3306,
                }),
            )
            .await;
        let expected = format!(r#"ReplicationEndpoint "{identifier}" already in use."#);
        assert_eq!(
            (status, error_code(&response), text(&response, "/message")),
            (
                400,
                "ResourceAlreadyExistsFault".to_string(),
                expected.clone()
            ),
            "case {case:?}: expected 400 ResourceAlreadyExistsFault {expected:?}, got {status} \
             {response}"
        );
        assert!(
            !text(&response, "/message").contains(&existing_arn),
            "case {case:?}: the message must not disclose the existing endpoint's ARN \
             ({existing_arn}), got {response}"
        );
    }
}

#[tokio::test]
async fn a_filtered_describe_that_matches_nothing_raises_resource_not_found() {
    let emulator = Emulator::new();

    // An UNFILTERED describe reports what the account holds, which here is nothing:
    // the rule is about filters that match nothing, not about an empty account.
    for (operation, member) in [
        ("DescribeReplicationInstances", "ReplicationInstances"),
        ("DescribeEndpoints", "Endpoints"),
    ] {
        let described = emulator
            .ok("empty account, no filters", operation, &json!({}))
            .await;
        assert_eq!(
            described.pointer(&format!("/{member}")).and_then(Value::as_array),
            Some(&Vec::new()),
            "case \"empty account, no filters\" on {operation}: expected an empty list, got \
             {described}"
        );
    }

    let instance_arn = emulator.available_instance("filtered-rep").await;
    let endpoint_arn = emulator.endpoint("filtered-ep", "source", "mysql").await;

    // One row per documented filter name, each with a value no resource carries, plus
    // a pair of filters each of which matches on its own but which match nothing
    // together.
    let instance_cases: Vec<(&str, Value)> = vec![
        (
            "an ARN nothing minted",
            json!([{ "Name": "replication-instance-arn", "Values": [format!("{instance_arn}X")] }]),
        ),
        (
            "an identifier no instance carries",
            json!([{ "Name": "replication-instance-id", "Values": ["no-such-rep"] }]),
        ),
        (
            "a class no instance was created with",
            json!([{ "Name": "replication-instance-class", "Values": ["dms.c4.xlarge"] }]),
        ),
        (
            "an engine version no instance runs",
            json!([{ "Name": "engine-version", "Values": ["0.0.0"] }]),
        ),
        (
            "several values, none of which match",
            json!([{ "Name": "replication-instance-id", "Values": ["no-such-rep", "nor-this"] }]),
        ),
        (
            "two filters that cannot both match",
            json!([
                { "Name": "replication-instance-id", "Values": ["filtered-rep"] },
                { "Name": "replication-instance-class", "Values": ["dms.r5.large"] },
            ]),
        ),
    ];
    for (case, filters) in instance_cases {
        let (status, response) = emulator
            .call("DescribeReplicationInstances", &json!({ "Filters": filters }))
            .await;
        assert_eq!(
            (
                status,
                error_code(&response),
                text(&response, "/message").as_str()
            ),
            (
                400,
                "ResourceNotFoundFault".to_string(),
                "No instances found matching provided filters"
            ),
            "case {case:?} on DescribeReplicationInstances: expected 400 ResourceNotFoundFault, \
             got {status} {response}"
        );
    }

    let endpoint_cases: Vec<(&str, Value)> = vec![
        (
            "an ARN nothing minted",
            json!([{ "Name": "endpoint-arn", "Values": [format!("{endpoint_arn}X")] }]),
        ),
        (
            "an identifier no endpoint carries",
            json!([{ "Name": "endpoint-id", "Values": ["no-such-ep"] }]),
        ),
        (
            "the other endpoint type",
            json!([{ "Name": "endpoint-type", "Values": ["target"] }]),
        ),
        (
            "an engine no endpoint runs",
            json!([{ "Name": "engine-name", "Values": ["sqlserver"] }]),
        ),
        (
            "several values, none of which match",
            json!([{ "Name": "engine-name", "Values": ["sqlserver", "db2"] }]),
        ),
        (
            "two filters that cannot both match",
            json!([
                { "Name": "endpoint-id", "Values": ["filtered-ep"] },
                { "Name": "engine-name", "Values": ["postgres"] },
            ]),
        ),
    ];
    for (case, filters) in endpoint_cases {
        let (status, response) = emulator
            .call("DescribeEndpoints", &json!({ "Filters": filters }))
            .await;
        assert_eq!(
            (
                status,
                error_code(&response),
                text(&response, "/message").as_str()
            ),
            (
                400,
                "ResourceNotFoundFault".to_string(),
                "No Endpoints found matching provided filters"
            ),
            "case {case:?} on DescribeEndpoints: expected 400 ResourceNotFoundFault, got {status} \
             {response}"
        );
    }

    // A filter that DOES match still answers, so the refusal is driven by the result
    // set and not by the presence of filters.
    let matched = emulator
        .ok(
            "a filter that matches",
            "DescribeReplicationInstances",
            &json!({ "Filters": [{ "Name": "replication-instance-id", "Values": ["filtered-rep"] }] }),
        )
        .await;
    assert_eq!(
        text(
            &matched,
            "/ReplicationInstances/0/ReplicationInstanceArn"
        ),
        instance_arn,
        "case \"a filter that matches\": expected the matching instance, got {matched}"
    );
    let matched = emulator
        .ok(
            "a filter that matches",
            "DescribeEndpoints",
            &json!({ "Filters": [{ "Name": "endpoint-id", "Values": ["filtered-ep"] }] }),
        )
        .await;
    assert_eq!(
        text(&matched, "/Endpoints/0/EndpointArn"),
        endpoint_arn,
        "case \"a filter that matches\": expected the matching endpoint, got {matched}"
    );
}
