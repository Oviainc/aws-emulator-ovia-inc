// SPDX-License-Identifier: CC0-1.0

//! AD Connector directories — `ConnectDirectory` — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the directory-lifecycle suite's,
//! reached as `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from
//! the state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a directory's servers sit in two subnets in two different Availability Zones, so a
//!   create whose settings block names fewer than two DISTINCT subnets is refused with
//!   `InvalidParameterException` and stores nothing — asserted over a table of subnet
//!   lists (one subnet, the same subnet twice, two subnets, three subnets, of both widths
//!   the modelled `SubnetId` pattern admits) and over every create of this service that
//!   takes such a block (`ConnectDirectory`, `CreateDirectory`, `CreateMicrosoftAD`);
//! * an accepted connector is reported from the store: its `Type` is `ADConnector`, the
//!   `Size` the request named is echoed, its placement is reported as `ConnectSettings`
//!   (and not as `VpcSettings`), and it leaves the transient `Requested` stage one
//!   transition span later — over a table differing in size, short name, description,
//!   network type and tags;
//! * a connector holds no accounts and no configurable settings of its own, because both
//!   belong to the self-managed directory it connects to: `ResetUserPassword` and
//!   `DescribeSettings` refuse one with `UnsupportedOperationException` while the same
//!   calls reach the business logic for the directory types that do hold them.

use serde_json::{json, Value};

use crate::directory_lifecycle::{
    assert_directory_id, call, call_in_region, descriptions, minimal_create, ok, refused, service,
    string, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::minimal_microsoft_ad;

/// The password of the service account in the self-managed directory. `ConnectPassword`
/// carries a length bound and no pattern, so any non-empty value is one the boundary
/// validator admits.
const CONNECT_PASSWORD: &str = "Sim2Real-Connect-Pw1!";

/// The service account `ConnectSettings$CustomerUserName` names, spelled the way the
/// modelled pattern `^[a-zA-Z0-9._-]+$` admits.
const CUSTOMER_USER_NAME: &str = "ad-connector-svc";

/// The DNS servers of the self-managed directory, as `CustomerDnsIps` names them.
const CUSTOMER_DNS_IPS: [&str; 2] = ["10.7.0.10", "10.7.1.10"];

/// The whole modelled `DirectorySize` catalog
/// (`smithy_query(query="shape",shape="DirectorySize")` answers `["Small", "Large"]`) —
/// the values `ConnectDirectoryRequest$Size` ("The size of the directory", `@required`)
/// admits, each of which a connector must report back.
const SIZE_CATALOG: [&str; 2] = ["Small", "Large"];

/// `ConnectSettings` for `subnet_ids` in one VPC.
fn connect_settings(subnet_ids: &[&str]) -> Value {
    json!({
        "VpcId": "vpc-0a1b2c3d",
        "SubnetIds": subnet_ids,
        "CustomerDnsIps": CUSTOMER_DNS_IPS,
        "CustomerUserName": CUSTOMER_USER_NAME,
    })
}

/// A well-formed `ConnectDirectory` body for `name` with `size` and `subnet_ids`.
fn connect_body(name: &str, size: &str, subnet_ids: &[&str]) -> Value {
    json!({
        "Name": name,
        "Password": CONNECT_PASSWORD,
        "Size": size,
        "ConnectSettings": connect_settings(subnet_ids),
    })
}

/// `ConnectDirectory` with `body`, asserting it was accepted and returning the id.
pub(crate) async fn connect_directory(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
    case: &str,
) -> String {
    let answer = ok(router, "ConnectDirectory", body, case).await;
    let id = string(&answer, "DirectoryId", case);
    assert_directory_id(&id, case);
    id
}

/// The one description of `directory_id`.
async fn describe_one(
    router: &smithy_server_runtime::MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Value {
    let answer = ok(
        router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [directory_id] }),
        case,
    )
    .await;
    let found = descriptions(&answer, case);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one description of {directory_id}, got {found:?}"
    );
    found[0].clone()
}

/// RULE: a create that places directory servers in a VPC needs two DISTINCT subnets, so
/// that its servers can sit in two Availability Zones; a settings block naming fewer is
/// refused with `InvalidParameterException` and persists nothing.
#[tokio::test]
async fn a_settings_block_naming_fewer_than_two_distinct_subnets_is_refused() {
    let router = service();

    // (case, the subnet list, whether it can carry a directory's servers). The widths are
    // the two the modelled `SubnetId` pattern
    // `^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$` admits.
    let rows: [(&str, &[&str], bool); 6] = [
        ("one eight-hex subnet", &["subnet-0a1b2c3d"], false),
        (
            "one seventeen-hex subnet",
            &["subnet-0a1b2c3d4e5f67890"],
            false,
        ),
        (
            "the same subnet twice",
            &["subnet-0a1b2c3d", "subnet-0a1b2c3d"],
            false,
        ),
        (
            "two distinct subnets",
            &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
            true,
        ),
        (
            "two distinct seventeen-hex subnets",
            &["subnet-0a1b2c3d4e5f67890", "subnet-09f8e7d6c5b4a3210"],
            true,
        ),
        (
            "three distinct subnets",
            &["subnet-aaaaaaaa", "subnet-bbbbbbbb", "subnet-cccccccc"],
            true,
        ),
    ];

    for (index, (case, subnets, servable)) in rows.iter().enumerate() {
        // Every create of this service that takes a settings block of subnets, so the
        // rule is one rule rather than one operation's check.
        let connector = connect_body(
            &format!("connector-{index}.example.com"),
            SIZE_CATALOG[0],
            subnets,
        );
        let mut simple = minimal_create(&format!("simple-{index}.example.com"));
        simple["VpcSettings"] = json!({ "VpcId": "vpc-0a1b2c3d", "SubnetIds": subnets });
        let mut managed = minimal_microsoft_ad(&format!("managed-{index}.example.com"));
        managed["VpcSettings"] = json!({ "VpcId": "vpc-0a1b2c3d", "SubnetIds": subnets });

        for (operation, body) in [
            ("ConnectDirectory", connector),
            ("CreateDirectory", simple),
            ("CreateMicrosoftAD", managed),
        ] {
            let row = format!("{operation} with {case}");
            if *servable {
                let answer = ok(&router, operation, body, &row).await;
                let id = string(&answer, "DirectoryId", &row);
                assert_directory_id(&id, &row);
            } else {
                refused(&router, operation, body, "InvalidParameterException", &row).await;
            }
        }
    }

    // Only the servable rows reached the store: three operations times three servable
    // subnet lists.
    let stored = ok(&router, "DescribeDirectories", json!({}), "after the table").await;
    let held = descriptions(&stored, "after the table");
    assert_eq!(
        held.len(),
        9,
        "case \"after the table\": expected only the servable rows to be stored, got {held:?}"
    );
}

/// One row of the connector table: how the connector is created, and the members its
/// description must carry.
struct ConnectCase {
    case: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

/// RULE: an accepted AD Connector is reported from what was persisted — its own type, the
/// size the request named, its placement as `ConnectSettings`, and the transient stage its
/// create owes.
#[tokio::test]
async fn a_connected_directory_is_reported_from_persisted_state() {
    let router = service();

    // (case, the body, the members the answer must carry). The rows differ along
    // everything `ConnectDirectory` persists: the size, whether a short name, a
    // description, a network type or tags came with the request.
    let rows: Vec<ConnectCase> = vec![
        ConnectCase {
            case: "minimal, Small",
            body: connect_body(
                "corp.example.com",
                SIZE_CATALOG[0],
                &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
            ),
            expect: vec![
                ("Name", json!("corp.example.com")),
                ("Size", json!(SIZE_CATALOG[0])),
                ("ShortName", json!(null)),
                ("Description", json!(null)),
                ("NetworkType", json!(null)),
                // An AD Connector has no edition and runs no domain controller of its
                // own: it forwards to the self-managed directory's.
                ("Edition", json!(null)),
                ("DesiredNumberOfDomainControllers", json!(null)),
            ],
        },
        ConnectCase {
            case: "Large with short name, description, network type and tags",
            body: json!({
                "Name": "sales.example.org",
                "Password": CONNECT_PASSWORD,
                "Size": SIZE_CATALOG[1],
                "ShortName": "SALESNB",
                "Description": "Sales connector",
                "NetworkType": "Dual-stack",
                "ConnectSettings": connect_settings(&["subnet-11223344", "subnet-55667788"]),
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
            expect: vec![
                ("Name", json!("sales.example.org")),
                ("Size", json!(SIZE_CATALOG[1])),
                ("ShortName", json!("SALESNB")),
                ("Description", json!("Sales connector")),
                ("NetworkType", json!("Dual-stack")),
            ],
        },
    ];

    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        ids.push(connect_directory(&router, row.body.clone(), row.case).await);
    }
    assert_ne!(
        ids[0], ids[1],
        "case \"two connectors\": expected two distinct ids, got {ids:?}"
    );

    for (row, id) in rows.iter().zip(&ids) {
        let case = row.case;
        let described = describe_one(&router, id, case).await;
        for (member, value) in &row.expect {
            assert_eq!(
                &described[*member], value,
                "case {case:?}: expected {member} to be {value}, got {described}"
            );
        }
        assert_eq!(
            described["Type"],
            json!("ADConnector"),
            "case {case:?}: expected Type ADConnector, got {described}"
        );
        // The placement is reported through the connector member, built from the request's
        // own settings block, and the VPC member a directory this service RUNS reports
        // stays absent.
        let requested = &row.body["ConnectSettings"];
        assert_eq!(
            described["ConnectSettings"]["VpcId"], requested["VpcId"],
            "case {case:?}: expected the requested VpcId in ConnectSettings, got {described}"
        );
        assert_eq!(
            described["ConnectSettings"]["SubnetIds"], requested["SubnetIds"],
            "case {case:?}: expected the requested SubnetIds in ConnectSettings, got {described}"
        );
        assert_eq!(
            described["ConnectSettings"]["CustomerUserName"],
            json!(CUSTOMER_USER_NAME),
            "case {case:?}: expected the requested CustomerUserName, got {described}"
        );
        assert_eq!(
            described["VpcSettings"],
            json!(null),
            "case {case:?}: expected no VpcSettings on an AD Connector, got {described}"
        );
        // The create answers while the connector is still being built, so a caller that
        // describes it immediately sees the transient stage the modelled `DirectoryStage`
        // enum opens with.
        assert_eq!(
            described["Stage"],
            json!("Requested"),
            "case {case:?}: expected the transient Requested stage, got {described}"
        );
    }

    // The stage transition the create owes falls due on its own, with nobody writing.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, id) in rows.iter().zip(&ids) {
        let case = row.case;
        let described = describe_one(&router, id, case).await;
        assert_eq!(
            described["Stage"],
            json!("Active"),
            "case {case:?}: expected the stage transition to have fallen due, got {described}"
        );
    }
}

/// RULE: the accounts and the configurable settings of an AD Connector belong to the
/// self-managed directory it connects to, so the operations that read or write either
/// refuse it with `UnsupportedOperationException` — while the directory types that DO hold
/// them reach the business logic.
#[tokio::test]
async fn a_connector_holds_neither_accounts_nor_settings_of_its_own() {
    let router = service();

    let connector = connect_directory(
        &router,
        connect_body(
            "connected.example.com",
            SIZE_CATALOG[0],
            &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        ),
        "the connector",
    )
    .await;
    // The two types that hold accounts of their own, as the controls.
    let simple = crate::directory_lifecycle::create_directory(
        &router,
        minimal_create("simple.example.com"),
        "the Simple AD control",
    )
    .await;
    let managed = crate::microsoft_ad::create_microsoft_ad(
        &router,
        minimal_microsoft_ad("managed.example.com"),
        "the Managed Microsoft AD control",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (operation, body) in [
        (
            "ResetUserPassword",
            json!({
                "DirectoryId": &connector,
                "UserName": "someone",
                "NewPassword": PASSWORD,
            }),
        ),
        (
            "DescribeSettings",
            json!({ "DirectoryId": &connector }),
        ),
    ] {
        let case = format!("{operation} on an AD Connector");
        refused(
            &router,
            operation,
            body,
            "UnsupportedOperationException",
            &case,
        )
        .await;
    }

    // The same two calls on the types that hold accounts and settings are decided on
    // their merits instead: the user that does not exist, and the settings that do.
    for directory in [&simple, &managed] {
        let case = format!("ResetUserPassword on {directory}");
        refused(
            &router,
            "ResetUserPassword",
            json!({
                "DirectoryId": directory,
                "UserName": "someone-who-does-not-exist",
                "NewPassword": PASSWORD,
            }),
            "UserDoesNotExistException",
            &case,
        )
        .await;
    }
    let settings = ok(
        &router,
        "DescribeSettings",
        json!({ "DirectoryId": &managed }),
        "DescribeSettings on a Managed Microsoft AD",
    )
    .await;
    assert!(
        !settings["SettingEntries"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true),
        "case \"DescribeSettings on a Managed Microsoft AD\": expected the settings a \
         Managed Microsoft AD carries, got {settings}"
    );

    // Nothing the three refusals asked for changed the store: the connector is still
    // there, reported the way it was persisted.
    let described = describe_one(&router, &connector, "after the refusals").await;
    assert_eq!(
        described["Type"],
        json!("ADConnector"),
        "case \"after the refusals\": expected the connector untouched, got {described}"
    );

    // The Region a connector is created in is the one the create was signed for, so a
    // read in that Region resolves it — the same reading `RemoveRegion` takes.
    let (status, parsed) = call_in_region(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [&connector] }),
        "us-east-1",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a read in the connector's own Region\": expected 200, got {status} with {parsed}"
    );

    // And the scaffold's 501 is gone from both operations: whatever they answer, it is a
    // modelled one.
    for (operation, body) in [
        (
            "ConnectDirectory",
            connect_body(
                "another.example.com",
                SIZE_CATALOG[1],
                &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
            ),
        ),
        (
            "DescribeSettings",
            json!({ "DirectoryId": &managed }),
        ),
        (
            "ResetUserPassword",
            json!({
                "DirectoryId": &managed,
                "UserName": "Admin",
                "NewPassword": PASSWORD,
            }),
        ),
    ] {
        let (status, parsed) = call(&router, operation, body).await;
        assert_ne!(
            status, 501,
            "case {operation:?}: expected an implemented operation, got {status} with {parsed}"
        );
    }
}
