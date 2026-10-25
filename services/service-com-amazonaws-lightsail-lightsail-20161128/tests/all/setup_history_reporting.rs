// SPDX-License-Identifier: CC0-1.0

//! `GetSetupHistory` served from persisted state, driven over the wire through
//! the same front door a client uses.
//!
//! The rule under test: `GetSetupHistory` is a REPORTING operation computed from
//! the store — for a name the account holds an instance under it answers HTTP
//! 200 carrying that instance's recorded `SetupInstanceHttps` requests, newest
//! first, every reported member taken from what the store holds for that
//! instance and the requests it persisted; for a name the account holds no
//! instance under — including a name another resource type holds — it answers
//! the service's `DoesNotExist` refusal. It never answers with a status that
//! says the operation is not served, and reading it moves nothing.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! the FORM of the resource name (its length, its digits, its hyphens and
//! underscores — every form the modelled `ResourceName` pattern admits), how
//! many setups the store holds for it (none, one, several), how many domain
//! names each of those carried, and where the instance lives.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has, and the
//! paging, ordering and redaction rules are pinned in `instance_https_setup.rs`.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};

/// The only value of the modelled `CertificateProvider` enum, which every
/// accepted request here carries.
const PROVIDER: &str = "LetsEncrypt";

/// The renewal contact every setup here carries, satisfying the modelled
/// `EmailAddress` length (6..254) and pattern.
const EMAIL: &str = "certs@example.com";

/// The `resourceType` an instance reports, from the modelled `ResourceType`
/// enum.
const RESOURCE_TYPE: &str = "Instance";

/// One instance to stand up, and the setups the store is to hold for it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// A name form the modelled `ResourceName` pattern (`^\w[\w\-]*\w$`)
    /// admits.
    instance_name: &'static str,
    availability_zone: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    /// The domain names of each `SetupInstanceHttps` call to make, in the order
    /// they are made. Empty for an instance nothing is set up on.
    setups: &'static [&'static [&'static str]],
}

/// The instances every case here reports the history of. The rows differ in the
/// form of the name (the two-character minimum the pattern admits, digits,
/// hyphens, underscores, and a long name), in how many setups the store holds
/// for the instance (none, one, three), in how many domain names each setup
/// carried, and in where the instance lives.
const ROWS: &[Row] = &[
    Row {
        case: "the shortest name the pattern admits, nothing set up on it",
        instance_name: "w1",
        availability_zone: "us-east-1a",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        setups: &[],
    },
    Row {
        case: "a hyphenated name with digits, one setup of one domain",
        instance_name: "web-server-01",
        availability_zone: "us-west-2b",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        setups: &[&["example.com"]],
    },
    Row {
        case: "an underscored name, three setups of one, two and three domains",
        instance_name: "under_scored_host",
        availability_zone: "ap-northeast-1c",
        blueprint_id: "wordpress",
        bundle_id: "large_3_0",
        setups: &[
            &["one.example.com"],
            &["two.example.com", "www.two.example.com"],
            &[
                "three.example.com",
                "www.three.example.com",
                "api.three.example.com",
            ],
        ],
    },
    Row {
        case: "a long mixed-form name, two setups",
        instance_name: "a-very-long-instance-name_with_digits-0123456789-and-more",
        availability_zone: "eu-west-3a",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "small_3_0",
        setups: &[&["long.example.com"], &["www.long.example.com"]],
    },
];

/// The `CreateInstances` body standing `row`'s instance up.
fn create_body(row: &Row) -> Value {
    json!({
        "instanceNames": [row.instance_name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    })
}

/// The `SetupInstanceHttps` body securing `instance` with `domain_names`.
fn setup_body(instance: &str, domain_names: &[&str]) -> Value {
    json!({
        "instanceName": instance,
        "emailAddress": EMAIL,
        "domainNames": domain_names,
        "certificateProvider": PROVIDER,
    })
}

/// The `operationId`s of a `GetSetupHistory` response, in the order reported.
fn reported_ids(history: &Value) -> Vec<String> {
    history["setupHistory"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record["operationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: `GetSetupHistory` answers 200 from the store for any name the account
/// holds an instance under, reporting the setups that instance has — newest
/// first, each one carrying the request it was made with and the instance as
/// the store holds it.
#[tokio::test]
async fn the_setups_a_store_holds_are_reported_for_the_instance_they_ran_on() {
    for row in ROWS {
        let emulator = Emulator::new();
        emulator.ok("CreateInstances", create_body(row)).await;
        let instance = emulator
            .ok("GetInstance", json!({"instanceName": row.instance_name}))
            .await["instance"]
            .clone();

        // Every setup the store is to hold for this instance, in the order made.
        let mut submitted: Vec<String> = Vec::new();
        for domain_names in row.setups {
            let answered = emulator
                .ok("SetupInstanceHttps", setup_body(row.instance_name, domain_names))
                .await;
            let id = answered["operations"][0]["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: a setup logs one operation carrying its id, got {answered:?}",
                        row.case
                    )
                })
                .to_string();
            submitted.push(id);
        }

        let (status, history) = emulator
            .call(
                "GetSetupHistory",
                json!({"resourceName": row.instance_name}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: the history of a stored instance is served, got {status} {history:?}",
            row.case
        );

        // Newest first is the reverse of the order the setups were made in.
        let expected: Vec<String> = submitted.iter().rev().cloned().collect();
        assert_eq!(
            reported_ids(&history),
            expected,
            "case {}: expected every stored setup once, newest first, got {history:?}",
            row.case
        );
        // A page carries at most five records and these rows make at most three,
        // so nothing remains to page through.
        assert!(
            history["nextPageToken"].is_null(),
            "case {}: a single page of history reports no next page token, got {history:?}",
            row.case
        );

        let records = history["setupHistory"].as_array().unwrap_or_else(|| {
            panic!(
                "case {}: the history is reported as a list, got {history:?}",
                row.case
            )
        });
        for (index, record) in records.iter().enumerate() {
            // The request, as it was submitted: these rows are reported newest
            // first, so record `index` is the setup `index` from the end.
            let domain_names = row.setups[row.setups.len() - 1 - index];
            assert_eq!(
                record["request"],
                json!({
                    "instanceName": row.instance_name,
                    "domainNames": domain_names,
                    "certificateProvider": PROVIDER,
                }),
                "case {}: record {index} must carry the request it was made with, got {record:?}",
                row.case
            );
            // The resource, as the store holds it.
            for member in ["name", "arn", "createdAt", "location"] {
                assert_eq!(
                    record["resource"][member], instance[member],
                    "case {}: record {index} resource.{member} must be the stored instance's, \
                     expected {:?}, got {:?}",
                    row.case, instance[member], record["resource"][member]
                );
            }
            assert_eq!(
                record["resource"]["resourceType"].as_str(),
                Some(RESOURCE_TYPE),
                "case {}: record {index} must name the resource type, got {record:?}",
                row.case
            );
            // A value of the modelled `SetupStatus` enum: the certificate is
            // being put in place when the call returns, and the clock settles it.
            let status = record["status"].as_str().unwrap_or_default();
            assert!(
                ["inProgress", "succeeded"].contains(&status),
                "case {}: record {index} must carry a modelled SetupStatus, got {status:?}",
                row.case
            );
        }

        // Reading is reporting: a second read reports the same records.
        let again = emulator
            .ok(
                "GetSetupHistory",
                json!({"resourceName": row.instance_name}),
            )
            .await;
        assert_eq!(
            reported_ids(&again),
            expected,
            "case {}: a read must leave the history where it was, got {again:?}",
            row.case
        );
    }
}

/// RULE: the history belongs to the instance the setups ran on, so a name the
/// account holds no INSTANCE under — never created, or held by another resource
/// type — is refused with the service's `DoesNotExist` envelope rather than
/// answered.
#[tokio::test]
async fn a_name_no_instance_is_held_under_is_refused() {
    let emulator = Emulator::new();

    // A disk and a static IP hold names of their own, and neither is an
    // instance a certificate can be set up on.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "data-volume",
                "availabilityZone": "us-east-1a",
                "sizeInGb": 32,
            }),
        )
        .await;
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "front-door"}))
        .await;
    // One instance really does exist, so the refusals below are about the name
    // asked for rather than an empty store.
    emulator.ok("CreateInstances", create_body(&ROWS[1])).await;

    for (case, name) in [
        ("a name nothing was ever created under", "never-created"),
        ("the shortest name nothing holds", "zz"),
        ("a name a block storage disk holds", "data-volume"),
        ("a name a static IP holds", "front-door"),
    ] {
        let (status, body) = emulator
            .call("GetSetupHistory", json!({"resourceName": name}))
            .await;
        assert_eq!(
            (status, error_code(&body), body["code"].as_str()),
            (400, "NotFoundException".to_string(), Some("DoesNotExist")),
            "case {case}: expected the DoesNotExist envelope, got {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(RESOURCE_TYPE) && message.contains(name),
            "case {case}: the refusal must name the resource type and the requested name, \
             got {body:?}"
        );
    }

    // The instance that does exist is still served, with its own empty history.
    let history = emulator
        .ok(
            "GetSetupHistory",
            json!({"resourceName": ROWS[1].instance_name}),
        )
        .await;
    assert_eq!(
        history["setupHistory"],
        json!([]),
        "case an instance nothing was set up on: expected an empty history, got {history:?}"
    );
}
