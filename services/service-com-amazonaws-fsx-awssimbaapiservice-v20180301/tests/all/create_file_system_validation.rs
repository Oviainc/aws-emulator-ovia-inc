// SPDX-License-Identifier: CC0-1.0

//! `CreateFileSystem`'s pre-creation guards, driven over the wire.
//!
//! Every case goes through the same front door a client uses (`POST /` with
//! `X-Amz-Target: AWSSimbaAPIService_v20180301.CreateFileSystem`), dispatched by the real
//! `MultiServiceRouter`, and the harness is the lifecycle suite's, so a refusal here is
//! the one a client sees.
//!
//! Two rules, each asserted over a table of inputs:
//!
//! 1. A file system is placed in the subnets the request names, so every subnet
//!    identifier it names must name a subnet the account HAS in the Region the request is
//!    signed for; one that names none is refused with the modelled
//!    `InvalidNetworkSettings` carrying that identifier, and nothing is stored. The rows
//!    here name the Region's DEFAULT subnets; the subnets a client minted through Amazon
//!    EC2 are the `create_file_system_resolves_ec2_subnets` suite's rows.
//! 2. The operation requires a client request token ("This operation requires a client
//!    request token in the request that Amazon FSx uses to ensure idempotent creation"),
//!    so a request that omits it is refused with the modelled `BadRequest` before
//!    anything is stored.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::create_file_system_resolves_ec2_subnets::{create_subnet, create_vpc, delete_subnet};
use crate::file_system_lifecycle::{call, emulator, ok, with_client_request_token};

/// An Availability Zone of the Region these requests are signed for, for the one subnet this
/// suite has EC2 mint and then delete.
const ZONE: &str = "us-west-2a";

/// The modelled `ErrorMessage` length (`length 1..256`), which every `Message` member in
/// this service's errors targets.
const ERROR_MESSAGE_LENGTH: std::ops::RangeInclusive<usize> = 1..=256;

/// How many file systems the store holds, read back through the implemented reader.
async fn stored_count(router: &MultiServiceRouter) -> usize {
    ok(router, "DescribeFileSystems", &json!({})).await["FileSystems"]
        .as_array()
        .map_or(0, Vec::len)
}

/// The `__type` of an error body, with the namespace an `awsJson` client strips.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// RULE (the accepting half): a create that names subnets the account has in the signed
/// Region is stored, and the stored record reports the VPC those subnets sit in.
///
/// The table spans every subnet of the account's network in the Region — each alone, and
/// in groups — which is what the rule generalises over.
#[tokio::test]
async fn subnets_the_account_has_are_accepted_and_report_their_vpc() {
    let router = emulator().await;
    let all: Vec<&str> = router
        .account_subnets()
        .iter()
        .map(String::as_str)
        .collect();
    let mut cases: Vec<Vec<&str>> = all.iter().map(|id| vec![*id]).collect();
    cases.push(vec![all[0], all[1]]);
    cases.push(all.clone());
    // The same subnets in another order are the same placement.
    cases.push(all.iter().rev().copied().collect());

    // The VPC those subnets sit in, as the service that OWNS them reports it — not a value
    // this suite builds.
    let expected_vpc = router.subnet_vpc(all[0]).await;
    for case in &cases {
        let created = ok(
            &router,
            "CreateFileSystem",
            &with_client_request_token(&json!({
                "FileSystemType": "WINDOWS",
                "StorageCapacity": 32,
                "SubnetIds": case,
            })),
        )
        .await;
        let file_system = created["FileSystem"].clone();
        let id = file_system["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            file_system["SubnetIds"], json!(case),
            "case {case:?}: expected the requested SubnetIds on the created file system, got \
             {:?}",
            file_system["SubnetIds"]
        );
        assert_eq!(
            file_system["VpcId"], json!(expected_vpc),
            "case {case:?}: expected VpcId {expected_vpc} (the VPC the account's subnets sit \
             in), got {:?}",
            file_system["VpcId"]
        );

        // Read back through the implemented reader: the response was the stored record.
        let described = ok(
            &router,
            "DescribeFileSystems",
            &json!({ "FileSystemIds": [id] }),
        )
        .await["FileSystems"][0]
            .clone();
        for member in ["SubnetIds", "VpcId"] {
            assert_eq!(
                described[member], file_system[member],
                "case {case:?}: DescribeFileSystems must report the stored {member}; create said \
                 {:?} and describe said {:?}",
                file_system[member], described[member]
            );
        }
    }

    assert_eq!(
        stored_count(&router).await,
        cases.len(),
        "case accepted creates: expected one file system per accepted request"
    );
}

/// RULE (the refusing half): a subnet identifier that names no subnet of the account in
/// the signed Region is refused with `InvalidNetworkSettings` (HTTP 400) naming that
/// identifier in `InvalidSubnetId`, and NOTHING is stored.
///
/// The table spans the forms the modelled `SubnetId` (`length 15..24`, pattern
/// `^(subnet-[0-9a-f]{8,})$`) allows — the shortest and the longest — a subnet the account
/// HELD and holds no longer, and the position of the offending identifier in the request:
/// first, last, or in a per-type configuration's `PreferredSubnetId`.
#[tokio::test]
async fn a_subnet_the_account_does_not_have_is_refused() {
    let router = emulator().await;
    // A subnet EC2 minted for this account and then removed again through its own API: an
    // identifier the account HELD and holds no longer, which is a different thing from one
    // it never had. (The suite that resolves client-created subnets end to end is
    // `create_file_system_resolves_ec2_subnets`; its EC2 helpers are reused here.)
    let vpc = create_vpc(&router, "a deleted subnet", "10.3.0.0/16").await;
    let deleted = create_subnet(&router, "a deleted subnet", &vpc, "10.3.1.0/24", ZONE).await;
    delete_subnet(&router, "a deleted subnet", &deleted).await;
    // (case, request members, the identifier the refusal must name)
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "the shortest form the pattern allows",
            json!({ "SubnetIds": ["subnet-00000000"] }),
            "subnet-00000000".to_string(),
        ),
        (
            "the longest form the pattern allows",
            json!({ "SubnetIds": ["subnet-fffffffffffffffff"] }),
            "subnet-fffffffffffffffff".to_string(),
        ),
        (
            "a 17-hex identifier that names nothing",
            json!({ "SubnetIds": ["subnet-0123456789abcdef0"] }),
            "subnet-0123456789abcdef0".to_string(),
        ),
        (
            "a subnet EC2 minted for this account and then deleted",
            json!({ "SubnetIds": [deleted.clone()] }),
            deleted.clone(),
        ),
        (
            "an absent identifier after one that resolves",
            json!({ "SubnetIds": [router.subnet_a(), "subnet-00000000deadbeef"] }),
            "subnet-00000000deadbeef".to_string(),
        ),
        (
            "an absent identifier before one that resolves",
            json!({ "SubnetIds": ["subnet-00000000deadbeef", router.subnet_a()] }),
            "subnet-00000000deadbeef".to_string(),
        ),
        (
            "two absent identifiers: the first is the one reported",
            json!({ "SubnetIds": ["subnet-aaaaaaaa", "subnet-bbbbbbbb"] }),
            "subnet-aaaaaaaa".to_string(),
        ),
        (
            "a PreferredSubnetId that resolves to nothing",
            json!({
                "SubnetIds": [router.subnet_a()],
                "WindowsConfiguration": {
                    "ThroughputCapacity": 32,
                    "PreferredSubnetId": "subnet-00000000cafebabe",
                },
            }),
            "subnet-00000000cafebabe".to_string(),
        ),
    ];

    for (case, members, offending) in &cases {
        let mut request = json!({ "FileSystemType": "WINDOWS", "StorageCapacity": 32 });
        for (member, value) in members.as_object().expect("the case members are an object") {
            request[member] = value.clone();
        }
        let (status, body) = call(
            &router,
            "CreateFileSystem",
            &with_client_request_token(&request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a subnet the account does not have, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNetworkSettings",
            "case {case}: expected the modelled InvalidNetworkSettings, got {body}"
        );
        assert_eq!(
            body["InvalidSubnetId"], json!(offending),
            "case {case}: expected InvalidSubnetId {offending}, got {:?}",
            body["InvalidSubnetId"]
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            ERROR_MESSAGE_LENGTH.contains(&message.len()) && message.contains(offending.as_str()),
            "case {case}: expected a Message of 1..256 characters naming {offending}, got \
             {message:?}"
        );
    }

    assert_eq!(
        stored_count(&router).await,
        0,
        "case refused creates: a refused create must store nothing"
    );
}

/// RULE: the operation requires a client request token, so a request that omits it is
/// refused with `BadRequest` (HTTP 400) and stores nothing; the same request carrying one
/// is created.
///
/// The table spans the modelled `FileSystemType` values, storage capacities and subnet
/// counts, so the refusal is the token's absence and never one request's shape. The last
/// row also fixes the PRECEDENCE: a request that both omits the token and names a subnet
/// the account does not have is refused for the token.
#[tokio::test]
async fn a_create_without_a_client_request_token_is_refused() {
    let router = emulator().await;
    // (case, the body without a ClientRequestToken, whether the body is otherwise valid)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "WINDOWS, one subnet",
            json!({
                "FileSystemType": "WINDOWS",
                "StorageCapacity": 32,
                "SubnetIds": [router.subnet_a()],
            }),
            true,
        ),
        (
            "LUSTRE, no StorageCapacity",
            json!({ "FileSystemType": "LUSTRE", "SubnetIds": [router.subnet_a()] }),
            true,
        ),
        (
            "ONTAP, two subnets and tags",
            json!({
                "FileSystemType": "ONTAP",
                "StorageCapacity": 1024,
                "SubnetIds": router.account_subnets()
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<String>>(),
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
            true,
        ),
        (
            "OPENZFS, the largest modelled StorageCapacity",
            json!({
                "FileSystemType": "OPENZFS",
                "StorageCapacity": 2_147_483_647_i64,
                "SubnetIds": [router.subnet_a()],
            }),
            true,
        ),
        (
            "no token AND a subnet the account does not have: the token is reported",
            json!({
                "FileSystemType": "WINDOWS",
                "StorageCapacity": 32,
                "SubnetIds": ["subnet-00000000"],
            }),
            false,
        ),
    ];

    let mut created = 0;
    for (case, body, otherwise_valid) in &cases {
        let (status, refused) = call(&router, "CreateFileSystem", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 without a ClientRequestToken, got {status} with \
             body {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequest",
            "case {case}: expected the modelled BadRequest, got {refused}"
        );
        let message = refused["Message"].as_str().unwrap_or_default();
        assert!(
            ERROR_MESSAGE_LENGTH.contains(&message.len())
                && message.to_lowercase().contains("client request token"),
            "case {case}: expected a Message of 1..256 characters naming the missing client \
             request token, got {message:?}"
        );

        // The same request, with a token, is created — so the token's absence was the only
        // reason it was refused.
        if *otherwise_valid {
            let accepted =
                ok(&router, "CreateFileSystem", &with_client_request_token(body)).await;
            created += 1;
            let id = accepted["FileSystem"]["FileSystemId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                ok(
                    &router,
                    "DescribeFileSystems",
                    &json!({ "FileSystemIds": [id.clone()] })
                )
                .await["FileSystems"][0]["FileSystemId"],
                json!(id),
                "case {case}: the created file system must be readable back through \
                 DescribeFileSystems"
            );
        }
    }

    assert_eq!(
        stored_count(&router).await,
        created,
        "case tokenless creates: only the requests carrying a token may have been stored"
    );
}
