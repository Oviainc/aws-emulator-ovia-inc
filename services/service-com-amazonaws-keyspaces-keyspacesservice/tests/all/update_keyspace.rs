// SPDX-License-Identifier: CC0-1.0

//! `UpdateKeyspace`, driven end to end over the service's own `awsJson1_0` wire protocol
//! and believed only once `GetKeyspace` and `GetTable` report the change back.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. An accepted `UpdateKeyspace` makes the keyspace `MULTI_REGION` over the set of
//!    Regions its `replicationSpecification.regionList` names — in the order the request
//!    names them, each Region once — and answers with that keyspace's ARN. The rows span
//!    the strategy the keyspace already had, how many Regions the call adds, the ordering
//!    of the list and a list that repeats a Region.
//! 2. The addition is asynchronous: the keyspace and every table in it are `UPDATING`
//!    from the moment the call is accepted until the clock owes them `ACTIVE` again, and
//!    `GetKeyspace` reports the tables' replication progress over that window. A keyspace
//!    or a table that is not `ACTIVE` cannot be updated.
//! 3. "To successfully replicate all tables to the new Region, they must use client-side
//!    timestamps": a keyspace holding a table that does not is refused unless the call
//!    carries `clientSideTimestamps.status = ENABLED`, which turns them on for every
//!    table in the keyspace.
//! 4. A request that breaks a rule the model cannot state is refused with the error the
//!    model declares for it, never with a 501, and the refusal leaves the keyspace and
//!    its tables exactly as they were.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    KEYSPACE_STATUS_VALUES, TABLE_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, settle_wait, Emulator, REGION,
};

/// Regions other than the request's own ([`REGION`]), so a `regionList` can be built from
/// more than one additional Region. Each is within `RegionList`'s member length (2..25).
const OTHER_REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-northeast-1"];

/// The `clientSideTimestamps` member as a request carries it:
/// `ClientSideTimestampsStatus` models exactly one value, `ENABLED`
/// (`crate::ops::types::CLIENT_SIDE_TIMESTAMPS_STATUS_VALUES`).
fn client_side_timestamps() -> Value {
    json!({"status": "ENABLED"})
}

/// A `MULTI_REGION` replication specification over `regions`.
fn multi_region(regions: &[&str]) -> Value {
    json!({"replicationStrategy": "MULTI_REGION", "regionList": regions})
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

/// Create a keyspace (with `specification`, if any) and one table in it (with client-side
/// timestamps when `table_uses_client_side_timestamps`). The caller waits out
/// [`settle_wait`] before updating: a keyspace is updatable only once it is `ACTIVE`.
async fn seed(
    emulator: &Emulator,
    keyspace: &str,
    specification: Option<Value>,
    table: Option<(&str, bool)>,
) {
    let mut request = json!({"keyspaceName": keyspace});
    if let Some(specification) = specification {
        request["replicationSpecification"] = specification;
    }
    assert_ok(keyspace, emulator.call("CreateKeyspace", request).await);
    if let Some((table_name, uses_client_side_timestamps)) = table {
        let mut request = json!({
            "keyspaceName": keyspace,
            "tableName": table_name,
            "schemaDefinition": minimal_schema("pk"),
        });
        if uses_client_side_timestamps {
            request["clientSideTimestamps"] = client_side_timestamps();
        }
        assert_ok(table_name, emulator.call("CreateTable", request).await);
    }
}

/// The `replicationGroupStatuses` entries of a `GetKeyspace` body, or a panic naming the
/// case when the keyspace reports none.
fn replication_group_statuses(case: &str, body: &Value) -> Vec<Value> {
    body["replicationGroupStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a MULTI_REGION keyspace reports a status per replication \
                 Region, got {body}"
            )
        })
}

/// One row of RULES 1 and 2: the keyspace to seed, the `regionList` to update it with and
/// the Region list that must then read back.
struct AddCase {
    /// The case name, which is also the keyspace name.
    name: &'static str,
    /// The `replicationSpecification` the keyspace is created with, if any.
    create: Option<Value>,
    /// The `regionList` the `UpdateKeyspace` request carries.
    request: &'static [&'static str],
    /// The Region list `GetKeyspace` must report afterwards.
    expect: &'static [&'static str],
    /// Whether the call carries `clientSideTimestamps` (and so the seeded table does
    /// NOT), or omits it (and so the seeded table already uses them). Both forms are
    /// accepted, and after either every table uses client-side timestamps.
    enables_client_side_timestamps: bool,
}

/// The rows: a keyspace that was single-Region and one that was already multi-Region, one
/// Region added and several, the current Region first in the list and in the middle, and a
/// list that names a Region twice.
fn add_cases() -> Vec<AddCase> {
    vec![
        // A keyspace created with no replicationSpecification at all is single-Region in
        // the Region it was created in; one call makes it multi-Region over two.
        AddCase {
            name: "implicit_single_to_pair",
            create: None,
            request: &[REGION, OTHER_REGIONS[0]],
            expect: &[REGION, OTHER_REGIONS[0]],
            enables_client_side_timestamps: true,
        },
        // An explicitly SINGLE_REGION keyspace, widened to three Regions in one call.
        AddCase {
            name: "explicit_single_to_three",
            create: Some(json!({"replicationStrategy": "SINGLE_REGION"})),
            request: &[REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]],
            expect: &[REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]],
            enables_client_side_timestamps: false,
        },
        // Already MULTI_REGION at the modelled minimum region count: one Region added.
        AddCase {
            name: "multi_region_widened_by_one",
            create: Some(multi_region(&[REGION, OTHER_REGIONS[0]])),
            request: &[REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]],
            expect: &[REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]],
            enables_client_side_timestamps: true,
        },
        // ... and two Regions added at once, to a keyspace that already had two.
        AddCase {
            name: "multi_region_widened_by_two",
            create: Some(multi_region(&[REGION, OTHER_REGIONS[1]])),
            request: &[REGION, OTHER_REGIONS[1], OTHER_REGIONS[0], OTHER_REGIONS[2]],
            expect: &[REGION, OTHER_REGIONS[1], OTHER_REGIONS[0], OTHER_REGIONS[2]],
            enables_client_side_timestamps: false,
        },
        // The list is reported in the order the request names it, which need not start
        // with the request's own Region.
        AddCase {
            name: "current_region_in_the_middle",
            create: None,
            request: &[OTHER_REGIONS[0], REGION, OTHER_REGIONS[2]],
            expect: &[OTHER_REGIONS[0], REGION, OTHER_REGIONS[2]],
            enables_client_side_timestamps: true,
        },
        // A Region named twice is one Region: it is kept once, where it was first named.
        AddCase {
            name: "repeated_region_kept_once",
            create: None,
            request: &[REGION, OTHER_REGIONS[0], REGION, OTHER_REGIONS[0]],
            expect: &[REGION, OTHER_REGIONS[0]],
            enables_client_side_timestamps: false,
        },
    ]
}

/// RULES 1 and 2: an accepted `UpdateKeyspace` replicates the keyspace over the Regions
/// its list names, reports the keyspace and its tables `UPDATING` until the clock owes
/// them `ACTIVE`, and answers with the keyspace's ARN.
#[tokio::test]
async fn update_keyspace_adds_regions_and_settles_the_keyspace_and_its_tables() {
    let emulator = Emulator::start();
    let cases = add_cases();
    const TABLE: &str = "replicated";

    for case in &cases {
        seed(
            &emulator,
            case.name,
            case.create.clone(),
            Some((TABLE, !case.enables_client_side_timestamps)),
        )
        .await;
    }
    // Every seeded keyspace and table is ACTIVE once the creations the clock owed settle.
    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        let before = get_keyspace(&emulator, case.name, case.name).await;
        let mut request = json!({
            "keyspaceName": case.name,
            "replicationSpecification": multi_region(case.request),
        });
        if case.enables_client_side_timestamps {
            request["clientSideTimestamps"] = client_side_timestamps();
        }
        let updated = assert_ok(case.name, emulator.call("UpdateKeyspace", request).await);

        // RULE 1: the answer is the ARN of the keyspace that was updated — the very ARN
        // the create minted, not a fresh resource.
        assert_eq!(
            updated["resourceArn"],
            json!(keyspace_arn(case.name)),
            "case {:?}: expected the published keyspace ARN, got {updated}",
            case.name
        );
        assert_eq!(
            updated["resourceArn"], before["resourceArn"],
            "case {:?}: the updated keyspace keeps the ARN it was created with, got {updated}",
            case.name
        );

        let got = get_keyspace(&emulator, case.name, case.name).await;
        assert_eq!(
            got["replicationStrategy"],
            json!(keyspaces::MULTI_REGION_STRATEGY),
            "case {:?}: a keyspace replicated in several Regions is MULTI_REGION, got {got}",
            case.name
        );
        assert_eq!(
            got["replicationRegions"],
            json!(case.expect),
            "case {:?}: expected the Region list {:?}, got {got}",
            case.name,
            case.expect
        );
        assert_eq!(
            got["keyspaceName"],
            json!(case.name),
            "case {:?}: the update changed no name, got {got}",
            case.name
        );

        // RULE 2: the keyspace is UPDATING in every Region while Amazon Keyspaces
        // replicates it, and no table is ACTIVE in the new Region yet.
        let statuses = replication_group_statuses(case.name, &got);
        assert_eq!(
            statuses.len(),
            case.expect.len(),
            "case {:?}: one status entry per replication Region, got {got}",
            case.name
        );
        for (status, region) in statuses.iter().zip(case.expect) {
            assert_eq!(
                status["region"],
                json!(region),
                "case {:?}: the status entries follow the Region list, got {got}",
                case.name
            );
            let keyspace_status = status["keyspaceStatus"].as_str().unwrap_or_default();
            assert!(
                KEYSPACE_STATUS_VALUES.contains(&keyspace_status),
                "case {:?}: keyspaceStatus {keyspace_status:?} is not a modelled value, got {got}",
                case.name
            );
            assert_eq!(
                keyspace_status,
                keyspaces::KEYSPACE_STATUS_UPDATING,
                "case {:?}: a keyspace whose Region addition was just accepted is UPDATING, \
                 got {got}",
                case.name
            );
            assert_eq!(
                status["tablesReplicationProgress"],
                json!("0.00%"),
                "case {:?}: no table has finished replicating yet, got {got}",
                case.name
            );
        }

        // ... and so is every table of the keyspace, which also now uses client-side
        // timestamps whether the call enabled them or the create did (RULE 3).
        let table = get_table(&emulator, case.name, case.name, TABLE).await;
        let table_status = table["status"].as_str().unwrap_or_default();
        assert!(
            TABLE_STATUS_VALUES.contains(&table_status),
            "case {:?}: status {table_status:?} is not a modelled TableStatus, got {table}",
            case.name
        );
        assert_eq!(
            table_status,
            keyspaces::TABLE_STATUS_UPDATING,
            "case {:?}: a table being replicated to another Region is UPDATING, got {table}",
            case.name
        );
        assert_eq!(
            table["clientSideTimestamps"],
            client_side_timestamps(),
            "case {:?}: every table of a replicated keyspace uses client-side timestamps, \
             got {table}",
            case.name
        );
    }

    // RULE 2, the other side of the span: the transitions the clock owed have become due,
    // so the keyspace and its table are ACTIVE again and the replication is complete —
    // and the Region list the update stored did not move.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let got = get_keyspace(&emulator, case.name, case.name).await;
        assert_eq!(
            got["replicationRegions"],
            json!(case.expect),
            "case {:?}: settling changed no Region, got {got}",
            case.name
        );
        for status in replication_group_statuses(case.name, &got) {
            assert_eq!(
                status["keyspaceStatus"],
                json!(keyspaces::KEYSPACE_STATUS_ACTIVE),
                "case {:?}: the keyspace is ACTIVE once the addition completes, got {got}",
                case.name
            );
            assert_eq!(
                status["tablesReplicationProgress"],
                json!("100.00%"),
                "case {:?}: every table has replicated once the addition completes, got {got}",
                case.name
            );
        }
        let table = get_table(&emulator, case.name, case.name, TABLE).await;
        assert_eq!(
            table["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the table is ACTIVE once the replication completes, got {table}",
            case.name
        );
    }
}

/// RULE 3: every table of the keyspace must use client-side timestamps for the keyspace to
/// be replicated, so a keyspace holding a table that does not is refused unless the call
/// enables them — and when it does, every table of the keyspace reports them afterwards.
#[tokio::test]
async fn update_keyspace_requires_client_side_timestamps_on_every_table() {
    let emulator = Emulator::start();
    const KEYSPACE: &str = "cst_rules";
    const EMPTY_KEYSPACE: &str = "cst_rules_empty";
    // Three tables: two already use client-side timestamps, one does not — so the rule is
    // asserted on a keyspace where only part of the tables block the addition.
    let tables: [(&str, bool); 3] = [("with_a", true), ("with_b", true), ("without", false)];

    seed(&emulator, KEYSPACE, None, None).await;
    for (table, uses) in tables {
        let mut request = json!({
            "keyspaceName": KEYSPACE,
            "tableName": table,
            "schemaDefinition": minimal_schema("pk"),
        });
        if uses {
            request["clientSideTimestamps"] = client_side_timestamps();
        }
        assert_ok(table, emulator.call("CreateTable", request).await);
    }
    // A keyspace holding no table at all: nothing can block the addition there.
    seed(&emulator, EMPTY_KEYSPACE, None, None).await;
    tokio::time::sleep(settle_wait()).await;

    let request = json!({
        "keyspaceName": KEYSPACE,
        "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[0]]),
    });
    assert_failure(
        "no clientSideTimestamps, and a table without them",
        emulator.call("UpdateKeyspace", request.clone()).await,
        400,
        "ValidationException",
    );
    // The refusal stored nothing: the keyspace is still single-Region and every table is
    // still exactly as it was created.
    let got = get_keyspace(&emulator, "refused", KEYSPACE).await;
    assert!(
        got["replicationRegions"].is_null()
            && got["replicationStrategy"] == json!(keyspaces::SINGLE_REGION_STRATEGY),
        "case {:?}: a refused update leaves the keyspace single-Region, got {got}",
        "refused"
    );
    for (table, uses) in tables {
        let body = get_table(&emulator, table, KEYSPACE, table).await;
        let expected = match uses {
            true => client_side_timestamps(),
            false => Value::Null,
        };
        assert_eq!(
            body["clientSideTimestamps"], expected,
            "case {table:?}: a refused update changes no table setting, got {body}"
        );
        assert_eq!(
            body["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {table:?}: a refused update leaves the table ACTIVE, got {body}"
        );
    }

    // The same call carrying the enable is accepted, and turns client-side timestamps on
    // for every table of the keyspace — including the ones that already had them.
    let mut enabling = request;
    enabling["clientSideTimestamps"] = client_side_timestamps();
    assert_ok(
        "clientSideTimestamps ENABLED",
        emulator.call("UpdateKeyspace", enabling).await,
    );
    for (table, _) in tables {
        let body = get_table(&emulator, table, KEYSPACE, table).await;
        assert_eq!(
            body["clientSideTimestamps"],
            client_side_timestamps(),
            "case {table:?}: the accepted update enabled client-side timestamps, got {body}"
        );
    }

    // A keyspace with no table needs no enable: there is no table to replicate.
    let got = assert_ok(
        EMPTY_KEYSPACE,
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": EMPTY_KEYSPACE,
                    "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[1]]),
                }),
            )
            .await,
    );
    assert_eq!(
        got["resourceArn"],
        json!(keyspace_arn(EMPTY_KEYSPACE)),
        "case {EMPTY_KEYSPACE:?}: expected the published keyspace ARN, got {got}"
    );
    let got = get_keyspace(&emulator, EMPTY_KEYSPACE, EMPTY_KEYSPACE).await;
    assert_eq!(
        got["replicationRegions"],
        json!([REGION, OTHER_REGIONS[1]]),
        "case {EMPTY_KEYSPACE:?}: the empty keyspace is replicated in the listed Regions, \
         got {got}"
    );
    for status in replication_group_statuses(EMPTY_KEYSPACE, &got) {
        assert!(
            status["tablesReplicationProgress"].is_null(),
            "case {EMPTY_KEYSPACE:?}: a keyspace holding no table has no replication \
             progress, got {got}"
        );
    }
}

/// RULE 1 at the emulator's own bound: a keyspace is replicated in at most
/// [`keyspaces::MAX_REPLICATION_REGIONS`] Regions, so a list at the cap is accepted and
/// one past it is refused with a modelled error before anything is stored.
#[tokio::test]
async fn update_keyspace_caps_the_replication_region_count() {
    let emulator = Emulator::start();
    const AT_CAP: &str = "regions_at_cap";
    const PAST_CAP: &str = "regions_past_cap";
    seed(&emulator, AT_CAP, None, None).await;
    seed(&emulator, PAST_CAP, None, None).await;
    tokio::time::sleep(settle_wait()).await;

    // `RegionList`'s member is `length 2..25` with no pattern, so these synthetic Region
    // names are inside the modelled member shape; the current Region leads the list.
    let region_list = |count: usize| -> Vec<String> {
        let mut regions = vec![REGION.to_string()];
        while regions.len() < count {
            regions.push(format!("region-{:04}", regions.len()));
        }
        regions
    };
    let at_cap = region_list(keyspaces::MAX_REPLICATION_REGIONS);
    let past_cap = region_list(keyspaces::MAX_REPLICATION_REGIONS + 1);

    assert_ok(
        "regionList at the cap",
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": AT_CAP,
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": at_cap,
                    },
                }),
            )
            .await,
    );
    let got = get_keyspace(&emulator, AT_CAP, AT_CAP).await;
    assert_eq!(
        got["replicationRegions"],
        json!(at_cap),
        "case {AT_CAP:?}: a list at the cap is stored whole, got {got}"
    );

    assert_failure(
        "regionList one past the cap",
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": PAST_CAP,
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": past_cap,
                    },
                }),
            )
            .await,
        400,
        "ValidationException",
    );
    let got = get_keyspace(&emulator, PAST_CAP, PAST_CAP).await;
    assert_eq!(
        (
            got["replicationStrategy"].clone(),
            got["replicationRegions"].clone()
        ),
        (json!(keyspaces::SINGLE_REGION_STRATEGY), Value::Null),
        "case {PAST_CAP:?}: a refused update stores no Region, got {got}"
    );

    // The same cap holds on a create, which states it in the one place both share.
    assert_failure(
        "CreateKeyspace past the cap",
        emulator
            .call(
                "CreateKeyspace",
                json!({
                    "keyspaceName": "created_past_cap",
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": region_list(keyspaces::MAX_REPLICATION_REGIONS + 1),
                    },
                }),
            )
            .await,
        400,
        "ValidationException",
    );
}

/// One row of RULE 4: an `UpdateKeyspace` request and the modelled failure it must draw.
struct RefusalCase {
    /// The case name.
    name: &'static str,
    /// The request body, verbatim.
    request: fn() -> Value,
    /// The HTTP status and wire error code the model declares for it.
    status: u16,
    code: &'static str,
}

/// RULE 4: every request a rule refuses draws the modelled error, and leaves the keyspaces
/// it named exactly as they were.
#[tokio::test]
async fn update_keyspace_refuses_a_request_no_rule_allows() {
    let emulator = Emulator::start();
    // One single-Region keyspace and one already replicated in two Regions, each holding a
    // table that already uses client-side timestamps, so no refusal below is the
    // client-side-timestamps one.
    const SINGLE: &str = "refusals_single";
    const MULTI: &str = "refusals_multi";
    const TABLE: &str = "t";
    seed(&emulator, SINGLE, None, Some((TABLE, true))).await;
    seed(
        &emulator,
        MULTI,
        Some(multi_region(&[REGION, OTHER_REGIONS[0]])),
        Some((TABLE, true)),
    )
    .await;
    tokio::time::sleep(settle_wait()).await;

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            // A keyspace nobody created: "The operation tried to access a keyspace, table,
            // or type that doesn't exist."
            name: "unknown keyspace",
            request: || json!({
                "keyspaceName": "never_created",
                "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[0]]),
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            // A service-managed keyspace is not user-modifiable.
            name: "service-managed keyspace",
            request: || json!({
                "keyspaceName": "system",
                "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[0]]),
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // MULTI_REGION without a regionList names no Region to replicate in.
            name: "MULTI_REGION without regionList",
            request: || json!({
                "keyspaceName": SINGLE,
                "replicationSpecification": {"replicationStrategy": "MULTI_REGION"},
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // "the regionList requires the current Region and at least one additional
            // Amazon Web Services Region".
            name: "regionList without the current Region",
            request: || json!({
                "keyspaceName": SINGLE,
                "replicationSpecification": multi_region(&[OTHER_REGIONS[0], OTHER_REGIONS[1]]),
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // A list that only repeats one Region denotes a single Region, which is one
            // short of the documented "at least one additional" Region.
            name: "regionList repeating one Region",
            request: || json!({
                "keyspaceName": SINGLE,
                "replicationSpecification": multi_region(&[REGION, REGION]),
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // UpdateKeyspace ADDS a Region; a SINGLE_REGION specification would narrow the
            // keyspace instead.
            name: "SINGLE_REGION specification",
            request: || json!({
                "keyspaceName": MULTI,
                "replicationSpecification": {
                    "replicationStrategy": "SINGLE_REGION",
                    "regionList": [REGION, OTHER_REGIONS[0]],
                },
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // A list that drops a Region the keyspace is already replicated in would
            // remove a Region, which this operation cannot do.
            name: "regionList dropping an existing Region",
            request: || json!({
                "keyspaceName": MULTI,
                "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[1]]),
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // A list identical to the keyspace's own Regions adds none, so it is not an
            // update at all...
            name: "regionList adding nothing",
            request: || json!({
                "keyspaceName": MULTI,
                "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[0]]),
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // ... and neither does the same set in another order.
            name: "regionList reordering the existing Regions",
            request: || json!({
                "keyspaceName": MULTI,
                "replicationSpecification": multi_region(&[OTHER_REGIONS[0], REGION]),
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // Modelled: `RegionList` is `length 2..`, so one Region is one past the bound.
            name: "regionList of one Region",
            request: || json!({
                "keyspaceName": SINGLE,
                "replicationSpecification": {
                    "replicationStrategy": "MULTI_REGION",
                    "regionList": [REGION],
                },
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // Modelled: `rs` accepts only SINGLE_REGION or MULTI_REGION.
            name: "replicationStrategy outside the value set",
            request: || json!({
                "keyspaceName": SINGLE,
                "replicationSpecification": {
                    "replicationStrategy": "GLOBAL",
                    "regionList": [REGION, OTHER_REGIONS[0]],
                },
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // Modelled: `replicationSpecification` is required.
            name: "no replicationSpecification",
            request: || json!({"keyspaceName": SINGLE}),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            // Modelled: `keyspaceName` is `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`.
            name: "keyspaceName outside the pattern",
            request: || json!({
                "keyspaceName": "_not a name",
                "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[0]]),
            }),
            status: 400,
            code: "ValidationException",
        },
    ];

    for case in &cases {
        assert_failure(
            case.name,
            emulator.call("UpdateKeyspace", (case.request)()).await,
            case.status,
            case.code,
        );
    }

    // Not one of those requests stored anything: both keyspaces still carry the
    // replication they were created with, and their tables are still ACTIVE.
    let got = get_keyspace(&emulator, SINGLE, SINGLE).await;
    assert_eq!(
        (
            got["replicationStrategy"].clone(),
            got["replicationRegions"].clone()
        ),
        (json!(keyspaces::SINGLE_REGION_STRATEGY), Value::Null),
        "case {SINGLE:?}: a refused update leaves the keyspace single-Region, got {got}"
    );
    let got = get_keyspace(&emulator, MULTI, MULTI).await;
    assert_eq!(
        got["replicationRegions"],
        json!([REGION, OTHER_REGIONS[0]]),
        "case {MULTI:?}: a refused update leaves the Region list as it was, got {got}"
    );
    for status in replication_group_statuses(MULTI, &got) {
        assert_eq!(
            status["keyspaceStatus"],
            json!(keyspaces::KEYSPACE_STATUS_ACTIVE),
            "case {MULTI:?}: a refused update moves no status, got {got}"
        );
    }
    for keyspace in [SINGLE, MULTI] {
        let body = get_table(&emulator, keyspace, keyspace, TABLE).await;
        assert_eq!(
            body["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {keyspace:?}: a refused update leaves the table ACTIVE, got {body}"
        );
    }
}

/// RULE 2, from the refusal side: a keyspace is replicable only from `ACTIVE`, and only
/// while every table in it is `ACTIVE` too, so a resource the clock still owes a
/// transition is the modelled conflict.
#[tokio::test]
async fn update_keyspace_conflicts_while_a_resource_is_still_settling() {
    let emulator = Emulator::start();
    const KEYSPACE: &str = "settling";
    const TABLE: &str = "t";
    let request = || {
        json!({
            "keyspaceName": KEYSPACE,
            "replicationSpecification": multi_region(&[REGION, OTHER_REGIONS[0]]),
            "clientSideTimestamps": client_side_timestamps(),
        })
    };

    // A keyspace whose creation has only just been accepted is CREATING.
    assert_ok(
        KEYSPACE,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": KEYSPACE}))
            .await,
    );
    assert_failure(
        "keyspace still CREATING",
        emulator.call("UpdateKeyspace", request()).await,
        409,
        "ConflictException",
    );
    tokio::time::sleep(settle_wait()).await;

    // A table whose creation has only just been accepted is CREATING, and the keyspace
    // cannot be replicated until every table has settled.
    assert_ok(
        TABLE,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tableName": TABLE,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    assert_failure(
        "table still CREATING",
        emulator.call("UpdateKeyspace", request()).await,
        409,
        "ConflictException",
    );
    tokio::time::sleep(settle_wait()).await;

    // Everything is ACTIVE, so the addition is accepted — and a second addition while the
    // first is still settling is the same conflict.
    assert_ok(
        "every resource ACTIVE",
        emulator.call("UpdateKeyspace", request()).await,
    );
    assert_failure(
        "keyspace still UPDATING",
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": KEYSPACE,
                    "replicationSpecification": multi_region(&[
                        REGION,
                        OTHER_REGIONS[0],
                        OTHER_REGIONS[1],
                    ]),
                }),
            )
            .await,
        409,
        "ConflictException",
    );

    // Once the clock owes it, the keyspace is ACTIVE again and a further Region can be
    // added — so the conflict above was the transient status, not a keyspace stuck in it.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "a second addition after settling",
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": KEYSPACE,
                    "replicationSpecification": multi_region(&[
                        REGION,
                        OTHER_REGIONS[0],
                        OTHER_REGIONS[1],
                    ]),
                }),
            )
            .await,
    );
    let got = get_keyspace(&emulator, KEYSPACE, KEYSPACE).await;
    assert_eq!(
        got["replicationRegions"],
        json!([REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]]),
        "case {KEYSPACE:?}: the second addition widened the Region list, got {got}"
    );
}
