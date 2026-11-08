// SPDX-License-Identifier: CC0-1.0

//! WHICH OPERATING SYSTEM A DIRECTORY RUNS: `DirectoryDescription$OsVersion`.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body
//! (`crate::directory_lifecycle`'s harness), so nothing here reaches into the state struct
//! and an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: `OsVersion` is "the operating system (OS) version of the directory",
//! a value of the modelled `OSVersion` enum, and it names the platform the directory TYPE
//! runs on — so a type this service runs a Windows Server for reports one of that enum's
//! values whatever the request asked for, and a type whose operating system Directory
//! Service does not name reports none. It is a fact of the type rather than of the request,
//! so it does not vary with the edition, the size, the network or the subnets a create
//! names, and it does not change once the directory is in service.
//!
//! The rows span the directory types this service creates and, within the type that has an
//! OS, every create input that might be mistaken for deciding it: both published editions and
//! the request that names none, both `NetworkType` values Managed Microsoft AD admits, and a
//! short name given or derived.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::ops::types::O_S_VERSION_VALUES;
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::MICROSOFT_AD_OS_VERSION;

use crate::ad_connector::minimal_connect;
use crate::directory_lifecycle::{
    call, describe_one, ok, service, string, PASSWORD, PAST_ONE_SPAN, STAGE_ACTIVE,
};

/// The whole modelled `OSVersion` catalog, as the model itself declares it
/// (`smithy_query(query="shape",shape="OSVersion")` answers `["SERVER_2012",
/// "SERVER_2019"]`, which the generated `O_S_VERSION_VALUES` carries): the values a
/// directory that runs a Windows Server may report. Taken from the generated catalog rather
/// than written out here, so a row cannot pass against a value the model does not declare.
const OS_VERSION_CATALOG: &[&str] = O_S_VERSION_VALUES;

/// Subnets for a create's settings block. Which subnets a directory is placed in has nothing
/// to do with the OS it runs, which is the point of using the same pair in every row.
const SUBNETS: [&str; 2] = ["subnet-0a1b2c3d", "subnet-1a2b3c4d"];

/// One row: a create, and whether the type it makes runs a Windows Server this service names.
struct OsCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// Whether the directory must report a value of [`OS_VERSION_CATALOG`].
    runs_windows_server: bool,
}

/// The rows: every directory type this service creates, and — for the one that runs a Windows
/// Server — every create input a reader might expect the OS to follow from.
fn rows() -> Vec<OsCase> {
    let microsoft_ad = |name: &str, extra: Value| -> Value {
        let mut body = json!({
            "Name": name,
            "Password": PASSWORD,
            "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": SUBNETS }
        });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        body
    };
    vec![
        OsCase {
            case: "a Managed Microsoft AD, no edition named",
            operation: "CreateMicrosoftAD",
            body: microsoft_ad("os0.example.com", json!({})),
            runs_windows_server: true,
        },
        OsCase {
            case: "a Managed Microsoft AD, Standard edition",
            operation: "CreateMicrosoftAD",
            body: microsoft_ad("os1.example.com", json!({ "Edition": "Standard" })),
            runs_windows_server: true,
        },
        OsCase {
            case: "a Managed Microsoft AD, Enterprise edition and a short name",
            operation: "CreateMicrosoftAD",
            body: microsoft_ad(
                "os2.example.com",
                json!({ "Edition": "Enterprise", "ShortName": "OSTWO" }),
            ),
            runs_windows_server: true,
        },
        OsCase {
            case: "a Managed Microsoft AD on a dual-stack network",
            operation: "CreateMicrosoftAD",
            body: microsoft_ad("os3.example.com", json!({ "NetworkType": "Dual-stack" })),
            runs_windows_server: true,
        },
        OsCase {
            case: "a Managed Microsoft AD on an IPv4 network",
            operation: "CreateMicrosoftAD",
            body: microsoft_ad("os4.example.com", json!({ "NetworkType": "IPv4" })),
            runs_windows_server: true,
        },
        OsCase {
            case: "a Simple AD directory, Small",
            operation: "CreateDirectory",
            body: json!({
                "Name": "os5.example.com",
                "Password": PASSWORD,
                "Size": "Small",
                "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": SUBNETS }
            }),
            runs_windows_server: false,
        },
        OsCase {
            case: "a Simple AD directory, Large",
            operation: "CreateDirectory",
            body: json!({
                "Name": "os6.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": SUBNETS }
            }),
            runs_windows_server: false,
        },
        OsCase {
            case: "an AD Connector",
            operation: "ConnectDirectory",
            body: minimal_connect("os7.example.com"),
            runs_windows_server: false,
        },
    ]
}

/// RULE: the directory type that runs a Windows Server reports a value of the modelled
/// `OSVersion` enum, whatever else the create asked for; a type whose operating system
/// Directory Service does not name reports none.
#[tokio::test]
async fn the_os_version_names_the_platform_the_directory_type_runs_on() {
    // The value this service reports has to be one the MODEL declares, or no client that
    // branches on the enum can read it: the constant and the generated catalog are pinned
    // together here so the model changing shows up as a failure rather than as a value a
    // client cannot match.
    assert!(
        OS_VERSION_CATALOG.contains(&MICROSOFT_AD_OS_VERSION),
        "expected the OS version this service reports to be one the model declares \
         {OS_VERSION_CATALOG:?}, got {MICROSOFT_AD_OS_VERSION:?}"
    );
    let router = service();

    let mut reported_values: Vec<String> = Vec::new();
    for row in rows() {
        let created = ok(&router, row.operation, row.body, row.case).await;
        let directory_id = string(&created, "DirectoryId", row.case);

        // Before the transition the create owes falls due, and after it: the OS a directory
        // runs is not something its lifecycle changes.
        for stage in ["while it is being created", "once it is in service"] {
            let case = format!("{} — {stage}", row.case);
            let described = describe_one(&router, &directory_id, &case).await;
            let reported = described["OsVersion"].as_str();
            if row.runs_windows_server {
                let reported = reported.unwrap_or_else(|| {
                    panic!("case {case:?}: expected an OsVersion, got {described}")
                });
                assert!(
                    OS_VERSION_CATALOG.contains(&reported),
                    "case {case:?}: expected a value of the modelled OSVersion enum \
                     {OS_VERSION_CATALOG:?}, got {reported:?}"
                );
                reported_values.push(reported.to_string());
            } else {
                assert!(
                    reported.is_none(),
                    "case {case:?}: expected no OsVersion for a type whose operating system \
                     Directory Service does not name, got {described}"
                );
            }
            if stage.starts_with("while") {
                tokio::time::sleep(PAST_ONE_SPAN).await;
                continue;
            }
            assert_eq!(
                described["Stage"],
                json!(STAGE_ACTIVE),
                "case {case:?}: expected the directory in service by now, got {described}"
            );
        }
    }

    // The value is the platform's, not the directory's: every directory of the type that has
    // an OS reports the SAME one, whatever its create asked for and wherever its rows sit in
    // the table.
    let distinct: std::collections::BTreeSet<String> = reported_values.iter().cloned().collect();
    assert_eq!(
        distinct.len(),
        1,
        "expected one OS version across every Managed Microsoft AD, got {reported_values:?}"
    );
    // And WHICH value: AWS Managed Microsoft AD runs its domain controllers on Windows
    // Server 2019, so the platform fact is pinned here rather than left as "some member of
    // the enum".
    assert_eq!(
        distinct.iter().next().map(String::as_str),
        Some("SERVER_2019"),
        "expected the Windows Server version AWS Managed Microsoft AD runs, got \
         {reported_values:?}"
    );

    // A listing reports what the per-id read reports: the member is derived in one place.
    let (status, listing) = call(&router, "DescribeDirectories", json!({})).await;
    assert_eq!(
        status, 200,
        "expected the listing to answer 200, got {status} with {listing}"
    );
    let listed: Vec<Value> = listing["DirectoryDescriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected DirectoryDescriptions, got {listing}"))
        .clone();
    for description in listed {
        let expected = match description["Type"].as_str() {
            Some("MicrosoftAD") => json!(reported_values[0]),
            _ => json!(null),
        };
        assert_eq!(
            description["OsVersion"], expected,
            "expected the listing to report the same OsVersion as the read, got {description}"
        );
    }
}
