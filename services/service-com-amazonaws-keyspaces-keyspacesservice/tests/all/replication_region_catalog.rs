// SPDX-License-Identifier: CC0-1.0

//! A Region member of this service names an Amazon Web Services Region, driven end to end
//! over the service's own `awsJson1_0` wire protocol.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a keyspace
//! is replicated in "the Amazon Web Services Regions where the keyspace is replicated in"
//! (`ReplicationSpecification$regionList`), so every value a request supplies for a Region
//! member must name a Region that exists — the whole Region catalog is accepted, and a
//! value naming no Region is refused with 400 `ValidationException` and stores nothing.
//! `region` is modelled as a plain string (`len 2..25`, no pattern and no value set), so
//! the catalog is the only thing that decides this.
//!
//! The rows span the value (every Region of the catalog; non-Regions of several forms),
//! its position in the list, the list's length, the replication strategy that carries it
//! and the operation it reaches the service through (`CreateKeyspace`, `UpdateKeyspace`,
//! `CreateTable`'s and `UpdateTable`'s per-Region settings).

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator, REGION,
};

/// Every Amazon Web Services Region this emulator knows: the shared catalog
/// `emulator_telemetry::policy::KNOWN_REGIONS`, which `keyspaces::is_aws_region` checks a
/// `regionList` entry against.
const CATALOG: &[&str] = ::emulator_telemetry::policy::KNOWN_REGIONS;

/// A `MULTI_REGION` replication specification over `regions`.
fn multi_region(regions: &[&str]) -> Value {
    json!({"replicationStrategy": "MULTI_REGION", "regionList": regions})
}

/// A keyspace name derived from `seed`, inside the modelled `keyspaceName` pattern
/// (`^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`).
fn keyspace_name(prefix: &str, seed: &str) -> String {
    let sanitized: String = seed
        .chars()
        .map(|character| match character.is_ascii_alphanumeric() {
            true => character,
            false => '_',
        })
        .collect();
    format!("{prefix}_{sanitized}")
}

/// `GetKeyspace` for one keyspace, as a 200 body.
async fn get_keyspace(emulator: &Emulator, case: &str, keyspace: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    )
}

/// Values that name no Amazon Web Services Region, each inside `region`'s modelled length
/// (2..25) so only the catalog can refuse it. The forms span a grammar-shaped name that is
/// not a Region, a real Region's name in the wrong case, an Availability Zone, a Region
/// with the wrong ordinal, a partition-wide endpoint label and a word that is no
/// identifier at all.
const NON_REGIONS: [&str; 8] = [
    "not-a-region",
    "zz-fake-9",
    "xx-xxxx-1",
    "US-WEST-2",
    "us-west-2a",
    "eu-west-99",
    "aws-global",
    "local",
];

/// RULE, accepting side: every Region of the catalog is a Region a keyspace may be
/// replicated in, and the keyspace reads back replicated in exactly the Regions the list
/// named.
#[tokio::test]
async fn region_list_accepts_every_region_of_the_catalog() {
    let emulator = Emulator::start();
    // The catalog is large enough to be the point of this test, and holds Regions the
    // issue's examples never cited.
    assert!(
        CATALOG.len() > 8 && CATALOG.contains(&REGION),
        "case {REGION:?}: the Region catalog must hold the request's own Region, got {} entries",
        CATALOG.len()
    );

    for region in CATALOG {
        if *region == REGION {
            continue;
        }
        let keyspace = keyspace_name("catalog", region);
        let created = assert_ok(
            region,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": multi_region(&[REGION, region]),
                    }),
                )
                .await,
        );
        assert!(
            created["resourceArn"].as_str().is_some_and(|arn| arn.ends_with(&format!(
                "/keyspace/{keyspace}/"
            ))),
            "case {region:?}: expected the keyspace's own ARN, got {created}"
        );

        let got = get_keyspace(&emulator, region, &keyspace).await;
        assert_eq!(
            (
                got["replicationStrategy"].clone(),
                got["replicationRegions"].clone()
            ),
            (
                json!(keyspaces::MULTI_REGION_STRATEGY),
                json!([REGION, region])
            ),
            "case {region:?}: expected a MULTI_REGION keyspace over [{REGION}, {region}], got {got}"
        );
        let statuses: Vec<String> = got["replicationGroupStatuses"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry["region"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            statuses,
            vec![REGION.to_string(), (*region).to_string()],
            "case {region:?}: expected one replication status per replication Region, got {got}"
        );
    }
}

/// RULE, refusing side: a `regionList` entry that names no Region is refused with the
/// modelled `ValidationException`, whatever its position in the list, the list's length or
/// the strategy carrying it — and the refused `CreateKeyspace` stores nothing.
#[tokio::test]
async fn create_keyspace_refuses_a_region_list_entry_that_names_no_region() {
    let emulator = Emulator::start();
    // A row cannot silently become legal: each value is outside the catalog this rule is
    // stated against.
    for value in NON_REGIONS {
        assert!(
            !keyspaces::is_aws_region(value),
            "case {value:?}: this row is meant to name no Amazon Web Services Region"
        );
    }

    for (index, value) in NON_REGIONS.iter().enumerate() {
        // The position of the bad entry alternates between last and first, and the list
        // length between the modelled minimum of 2 and 3.
        let (case, list): (String, Vec<&str>) = match index % 3 {
            0 => (format!("{value}_last"), vec![REGION, value]),
            1 => (format!("{value}_first"), vec![value, REGION]),
            _ => (
                format!("{value}_middle_of_three"),
                vec![REGION, value, "eu-west-1"],
            ),
        };
        let keyspace = keyspace_name("bogus", &format!("{index}"));
        assert_failure(
            &case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace,
                        "replicationSpecification": multi_region(&list),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        // The refusal stored no keyspace at all.
        assert_failure(
            &case,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": keyspace}))
                .await,
            404,
            "ResourceNotFoundException",
        );

        // The same entry under the other strategy: a `SINGLE_REGION` specification that
        // carries a list is held to the same rule, because the entries are Regions either
        // way.
        assert_failure(
            &format!("{case}_single_region_strategy"),
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace_name("bogus_single", &format!("{index}")),
                        "replicationSpecification": {
                            "replicationStrategy": "SINGLE_REGION",
                            "regionList": list,
                        },
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    // One past each published bound of the member itself (`region`, len 2..25): refused by
    // the boundary validator the model drives, with the same modelled error.
    let over_long = "e".repeat(26);
    for value in ["a", over_long.as_str()] {
        assert_failure(
            value,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace_name("length", &format!("{}", value.len())),
                        "replicationSpecification": multi_region(&[REGION, value]),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    // The catalog's shortest and longest entries, which those bounds must not exclude.
    let shortest = CATALOG
        .iter()
        .filter(|region| **region != REGION)
        .min_by_key(|region| region.len())
        .expect("the Region catalog is not empty");
    let longest = CATALOG
        .iter()
        .filter(|region| **region != REGION)
        .max_by_key(|region| region.len())
        .expect("the Region catalog is not empty");
    for region in [shortest, longest] {
        assert_ok(
            region,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": keyspace_name("bound", region),
                        "replicationSpecification": multi_region(&[REGION, region]),
                    }),
                )
                .await,
        );
    }
}

/// RULE, through `UpdateKeyspace`: the operation resolves its own `regionList` by the same
/// rule, so a non-Region is refused and the keyspace keeps the replication it had.
#[tokio::test]
async fn update_keyspace_refuses_a_region_list_entry_that_names_no_region() {
    let emulator = Emulator::start();
    const KEYSPACE: &str = "update_region_rule";
    assert_ok(
        KEYSPACE,
        emulator
            .call(
                "CreateKeyspace",
                json!({
                    "keyspaceName": KEYSPACE,
                    "replicationSpecification": multi_region(&[REGION, "us-east-1"]),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    for (index, value) in NON_REGIONS.iter().enumerate() {
        let (case, list): (String, Vec<&str>) = match index % 2 {
            0 => (
                format!("{value}_added_last"),
                vec![REGION, "us-east-1", value],
            ),
            _ => (
                format!("{value}_added_first"),
                vec![value, REGION, "us-east-1"],
            ),
        };
        assert_failure(
            &case,
            emulator
                .call(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "replicationSpecification": multi_region(&list),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        let got = get_keyspace(&emulator, &case, KEYSPACE).await;
        assert_eq!(
            got["replicationRegions"],
            json!([REGION, "us-east-1"]),
            "case {case:?}: a refused update leaves the Region list as it was, got {got}"
        );
    }

    // A Region the catalog does hold still widens the keyspace, so the rule refuses only
    // what names no Region.
    let added = CATALOG
        .iter()
        .find(|region| **region != REGION && **region != "us-east-1")
        .expect("the Region catalog holds a third Region");
    assert_ok(
        added,
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": KEYSPACE,
                    "replicationSpecification": multi_region(&[REGION, "us-east-1", added]),
                }),
            )
            .await,
    );
    let got = get_keyspace(&emulator, added, KEYSPACE).await;
    assert_eq!(
        got["replicationRegions"],
        json!([REGION, "us-east-1", added]),
        "case {added:?}: a Region of the catalog widens the keyspace, got {got}"
    );
}

/// RULE, through the per-Region table settings: `replicaSpecifications` names the Regions
/// the table IS replicated in, which are the keyspace's Regions — all of them Regions of
/// the catalog — so a value naming no Region is refused there too.
#[tokio::test]
async fn replica_specifications_refuse_a_region_that_names_no_region() {
    let emulator = Emulator::start();
    const KEYSPACE: &str = "replica_region_rule";
    const TABLE: &str = "t";
    assert_ok(
        KEYSPACE,
        emulator
            .call(
                "CreateKeyspace",
                json!({
                    "keyspaceName": KEYSPACE,
                    "replicationSpecification": multi_region(&[REGION, "us-east-1"]),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    for value in NON_REGIONS {
        assert_failure(
            &format!("CreateTable_{value}"),
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": keyspace_name("t", value),
                        "schemaDefinition": minimal_schema("pk"),
                        "replicaSpecifications": [{"region": value}],
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    // A Region the keyspace IS replicated in is accepted for the same member, so the
    // refusals above are about the value, not about the member.
    assert_ok(
        TABLE,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tableName": TABLE,
                    "schemaDefinition": minimal_schema("pk"),
                    "replicaSpecifications": [{"region": "us-east-1"}],
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    for value in NON_REGIONS {
        assert_failure(
            &format!("UpdateTable_{value}"),
            emulator
                .call(
                    "UpdateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": TABLE,
                        "replicaSpecifications": [{"region": value}],
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }
}
