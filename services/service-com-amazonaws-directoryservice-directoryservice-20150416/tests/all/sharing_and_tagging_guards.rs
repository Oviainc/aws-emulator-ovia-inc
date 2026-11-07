// SPDX-License-Identifier: CC0-1.0

//! The guards the directory's SHARING, TAGGING, Region-replication and alias surfaces
//! owe, and the values they report.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each over a TABLE rather than one recorded call:
//!
//! * a directory is shared FROM its owner account WITH another one, so the owner's own
//!   account is not a target it can be shared with: such a `ShareDirectory` is refused
//!   with `InvalidTargetException` and records no share, while the identical call naming
//!   another account is served;
//! * `AcceptSharedDirectory` answers for the share the sharing request created, moving it
//!   out of `PendingAcceptance` into the `Sharing` the clock settles on `Shared`; an id no
//!   sharing request created is refused, and a share that is already in service is
//!   refused as one already shared;
//! * a tag key in the AWS-reserved `aws:` namespace is a system tag: neither
//!   `AddTagsToResource` nor `RemoveTagsFromResource` accepts one, and a refused call
//!   leaves the stored tags exactly as they were;
//! * `AddRegion` naming the directory's OWN home Region is refused as an operation that
//!   is not supported, with that refusal's own sentence;
//! * `AccessUrl` is the bare HOST the directory's alias names, with no scheme and no
//!   path, so a client can use it as a host name.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The account this emulator presents as the directory owner — `engine::mock_account_id`,
/// which is what every ARN and account-valued member of this emulator reports.
const OWNER_ACCOUNT_ID: &str = "000000000000";

/// Two consumer accounts that are NOT the owner's, spelled the way the modelled
/// `CustomerId` pattern `^(\d{12})$` admits.
const CONSUMER_ACCOUNT_IDS: [&str; 2] = ["111122223333", "444455556666"];

/// The `ShareMethod` values of the modelled enum.
const SHARE_METHOD_HANDSHAKE: &str = "HANDSHAKE";
const SHARE_METHOD_ORGANIZATIONS: &str = "ORGANIZATIONS";

/// The `ShareStatus` values of the modelled enum this suite names.
const SHARE_STATUS_PENDING_ACCEPTANCE: &str = "PendingAcceptance";
const SHARE_STATUS_SHARING: &str = "Sharing";
const SHARE_STATUS_SHARED: &str = "Shared";

/// The `TargetType` the modelled `ShareTarget$Type` declares: an account.
const TARGET_TYPE_ACCOUNT: &str = "ACCOUNT";

/// The suffix `crate::state::refusal` appends to every message of this service.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// The DNS suffix a directory's access URL is built on, from the `AccessUrl` member's own
/// documentation.
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// A Managed Microsoft AD directory in service: the only type a directory share is
/// supported for, and the type Region replication needs.
async fn directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// The `ShareDirectory` body for one target account.
fn share_body(directory_id: &str, account_id: &str, share_method: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "ShareMethod": share_method,
        "ShareTarget": { "Id": account_id, "Type": TARGET_TYPE_ACCOUNT },
    })
}

/// Every share `DescribeSharedDirectories` reports for `directory_id`, by
/// `SharedDirectoryId`.
async fn shares(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> BTreeMap<String, Value> {
    let answer = ok(
        router,
        "DescribeSharedDirectories",
        json!({ "OwnerDirectoryId": directory_id }),
        case,
    )
    .await;
    answer["SharedDirectories"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected SharedDirectories, got {answer}"))
        .iter()
        .map(|share| {
            (
                share["SharedDirectoryId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                share.clone(),
            )
        })
        .collect()
}

/// The text of a refusal's `Message`, with the request-id suffix this service appends to
/// every one of them taken off.
fn refusal_text(parsed: &Value) -> String {
    let message = parsed["Message"].as_str().unwrap_or_default();
    message
        .split_once(REQUEST_ID_SEPARATOR)
        .map(|(text, _)| text)
        .unwrap_or(message)
        .to_string()
}

/// RULE: the directory owner's own account is not a target its directory can be shared
/// with; such a call is refused and records no share, while another account is served.
#[tokio::test]
async fn a_directory_is_not_shared_with_its_own_owner_account() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "the share target").await;

    // The rows differ along the target the request names and the method it shares by, so
    // the rule holds for every way a directory can be shared: the owner's own account is
    // refused whichever method asked for it, and another account is served.
    let rows: [(&str, &str, &str, bool); 4] = [
        (
            "the owner's own account, by handshake",
            OWNER_ACCOUNT_ID,
            SHARE_METHOD_HANDSHAKE,
            false,
        ),
        (
            "the owner's own account, inside the organization",
            OWNER_ACCOUNT_ID,
            SHARE_METHOD_ORGANIZATIONS,
            false,
        ),
        (
            "another account, by handshake",
            CONSUMER_ACCOUNT_IDS[0],
            SHARE_METHOD_HANDSHAKE,
            true,
        ),
        (
            "another account, inside the organization",
            CONSUMER_ACCOUNT_IDS[1],
            SHARE_METHOD_ORGANIZATIONS,
            true,
        ),
    ];

    let mut served = 0;
    for (case, account_id, share_method, accepted) in rows {
        let (status, parsed) = call(
            &router,
            "ShareDirectory",
            share_body(&directory_id, account_id, share_method),
        )
        .await;
        let inventory = shares(&router, &directory_id, case).await;
        if accepted {
            served += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the share to be served, got {status} with {parsed}"
            );
            assert_eq!(
                inventory.len(),
                served,
                "case {case:?}: expected the served share to be recorded, got {inventory:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the share to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("InvalidTargetException"),
            "case {case:?}: expected InvalidTargetException, got {parsed}"
        );
        assert_eq!(
            refusal_text(&parsed),
            format!(
                "The Target AWS account ID {account_id} you specified cannot be the same as \
                 the directory owner AWS account ID."
            ),
            "case {case:?}: expected the refusal to name the target account the request \
             sent, got {parsed}"
        );
        assert_eq!(
            inventory.len(),
            served,
            "case {case:?}: expected the refused share to record nothing, got {inventory:?}"
        );
    }
}

/// RULE: `AcceptSharedDirectory` answers for the share the sharing request created and
/// moves it out of `PendingAcceptance`; an id no request created, and a share already in
/// service, are refused.
#[tokio::test]
async fn accepting_a_sharing_request_puts_that_share_in_service() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "acceptance").await;

    // An id no sharing request minted names no shared directory, whatever store the
    // request is made against: the rows are a well-formed id nothing created and the
    // OWNER directory's own id, which is a directory but not a share.
    for (case, shared_directory_id) in [
        ("an id no sharing request created", "d-9026a1b2c3"),
        ("the owner directory's own id", directory_id.as_str()),
    ] {
        let (status, parsed) = call(
            &router,
            "AcceptSharedDirectory",
            json!({ "SharedDirectoryId": shared_directory_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the acceptance to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("EntityDoesNotExistException"),
            "case {case:?}: expected EntityDoesNotExistException, got {parsed}"
        );
        assert_eq!(
            refusal_text(&parsed),
            format!("Directory: '{shared_directory_id}' does not exist."),
            "case {case:?}: expected the not-found sentence for the id the request named, \
             got {parsed}"
        );
    }

    // A handshake share waits for the consumer account's answer, which is what this
    // operation gives it.
    let shared = ok(
        &router,
        "ShareDirectory",
        share_body(
            &directory_id,
            CONSUMER_ACCOUNT_IDS[0],
            SHARE_METHOD_HANDSHAKE,
        ),
        "a pending share",
    )
    .await;
    let shared_directory_id = string(&shared, "SharedDirectoryId", "a pending share");
    let pending = shares(&router, &directory_id, "a pending share").await;
    assert_eq!(
        pending[&shared_directory_id]["ShareStatus"],
        json!(SHARE_STATUS_PENDING_ACCEPTANCE),
        "case \"a pending share\": expected the share to wait for the consumer account, got \
         {pending:?}"
    );

    let accepted = ok(
        &router,
        "AcceptSharedDirectory",
        json!({ "SharedDirectoryId": &shared_directory_id }),
        "the acceptance",
    )
    .await;
    // The acceptance answers while the consumer's shared directory is still being put in
    // service, and it answers with the stored record — so the two agree.
    assert_eq!(
        accepted["SharedDirectory"]["ShareStatus"],
        json!(SHARE_STATUS_SHARING),
        "case \"the acceptance\": expected the transient status right after the acceptance, \
         got {accepted}"
    );
    assert_eq!(
        accepted["SharedDirectory"]["SharedDirectoryId"],
        json!(&shared_directory_id),
        "case \"the acceptance\": expected the answer to name the share it accepted, got \
         {accepted}"
    );
    let reported = shares(&router, &directory_id, "the acceptance").await;
    assert_eq!(
        reported[&shared_directory_id], accepted["SharedDirectory"],
        "case \"the acceptance\": expected the inventory to agree with the answer, got \
         {reported:?}"
    );

    // A share already in service is one this account has already accepted.
    let (status, parsed) = call(
        &router,
        "AcceptSharedDirectory",
        json!({ "SharedDirectoryId": &shared_directory_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a second acceptance\": expected it to be refused, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("DirectoryAlreadySharedException"),
        "case \"a second acceptance\": expected DirectoryAlreadySharedException, got {parsed}"
    );

    // The transition the acceptance owed has fallen due: the share is in service, and the
    // clock — not a later call — is what moved it.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = shares(&router, &directory_id, "past one span").await;
    assert_eq!(
        settled[&shared_directory_id]["ShareStatus"],
        json!(SHARE_STATUS_SHARED),
        "case \"past one span\": expected the share to reach the settled status, got \
         {settled:?}"
    );
}

/// RULE: a tag key in the AWS-reserved `aws:` namespace is a system tag, which neither
/// tagging operation accepts, and a refused call leaves the stored tags as they were.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_by_both_tagging_operations() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "tag keys").await;

    // The ordinary key the directory holds through every case below, so a refusal can
    // never be mistaken for an empty resource.
    ok(
        &router,
        "AddTagsToResource",
        json!({
            "ResourceId": &directory_id,
            "Tags": [{ "Key": "owner", "Value": "platform" }]
        }),
        "an ordinary key",
    )
    .await;

    // The rows differ along the key the request names: the reserved prefix itself, keys
    // under it in both cases AWS treats as that namespace, keys that merely START with
    // the same letters, which are ordinary keys, and a key whose characters are
    // multi-byte — which the modelled `TagKey` admits, since it allows any Unicode
    // letter, and which no prefix test may stumble over.
    let rows: [(&str, &str, bool); 8] = [
        ("the reserved prefix alone", "aws:", true),
        ("a key in the reserved namespace", "aws:reserved-tag", true),
        ("the reserved prefix in upper case", "AWS:cloudformation", true),
        ("the reserved prefix in mixed case", "Aws:Created", true),
        ("a key that only starts with those letters", "awsome", false),
        ("an ordinary key", "team", false),
        ("a key of multi-byte characters", "日本", false),
        ("a multi-byte key under the reserved prefix", "aws:日本", true),
    ];

    for (case, key, reserved) in rows {
        let before = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceId": &directory_id }),
            case,
        )
        .await;

        for operation in ["AddTagsToResource", "RemoveTagsFromResource"] {
            let body = if operation == "AddTagsToResource" {
                json!({
                    "ResourceId": &directory_id,
                    "Tags": [{ "Key": key, "Value": "x" }]
                })
            } else {
                json!({ "ResourceId": &directory_id, "TagKeys": [key] })
            };
            let (status, parsed) = call(&router, operation, body).await;
            if !reserved {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {operation} to serve an ordinary key, got \
                     {status} with {parsed}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to refuse a system tag, got {status} \
                 with {parsed}"
            );
            let wire_type = parsed["__type"].as_str().unwrap_or_default();
            assert!(
                wire_type.contains("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException from {operation}, got \
                 {parsed}"
            );
            assert_eq!(
                refusal_text(&parsed),
                "Adding or removing system tags (with prefix aws:) is not supported.",
                "case {case:?}: expected that refusal's own sentence from {operation}, got \
                 {parsed}"
            );

            // A refused call stored nothing and removed nothing.
            let after = ok(
                &router,
                "ListTagsForResource",
                json!({ "ResourceId": &directory_id }),
                case,
            )
            .await;
            assert_eq!(
                after["Tags"], before["Tags"],
                "case {case:?}: expected the refused {operation} to leave the stored tags \
                 alone, got {after}"
            );
        }
    }
}

/// RULE: `AddRegion` naming the directory's own home Region is refused as an operation
/// that is not supported, with that refusal's own sentence, and replicates nothing.
#[tokio::test]
async fn adding_the_directorys_own_home_region_is_refused() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "the home Region").await;

    let home_region = {
        let answer = ok(
            &router,
            "DescribeRegions",
            json!({ "DirectoryId": &directory_id }),
            "the home Region",
        )
        .await;
        answer["RegionsDescription"][0]["RegionName"]
            .as_str()
            .unwrap_or_else(|| panic!("expected the primary Region, got {answer}"))
            .to_string()
    };

    let (status, parsed) = call(
        &router,
        "AddRegion",
        json!({
            "DirectoryId": &directory_id,
            "RegionName": &home_region,
            "VPCSettings": {
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
            }
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"the home Region\": expected the call to be refused, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("UnsupportedOperationException"),
        "case \"the home Region\": expected UnsupportedOperationException, got {parsed}"
    );
    assert_eq!(
        refusal_text(&parsed),
        "The region you specified is the same region where the AWS Managed Microsoft AD \
         directory was created. Specify a different region and try again.",
        "case \"the home Region\": expected that refusal's own sentence, got {parsed}"
    );
    let regions = ok(
        &router,
        "DescribeRegions",
        json!({ "DirectoryId": &directory_id }),
        "the home Region",
    )
    .await;
    assert_eq!(
        regions["RegionsDescription"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case \"the home Region\": expected the refused call to replicate nothing, got \
         {regions}"
    );
}

/// RULE: `AccessUrl` is the bare HOST the directory's alias names — no scheme, no path —
/// so a client can use the value as a host name.
#[tokio::test]
async fn the_access_url_is_the_bare_host_the_alias_names() {
    let router = service();

    // The rows differ along the alias the host is built from: the directory id itself,
    // which stands in when no alias was assigned ("If no alias exists, the alias is the
    // directory identifier"), and the alias forms the modelled `AliasName` admits at both
    // published length bounds.
    let rows: [(&str, Option<String>); 4] = [
        ("no alias assigned", None),
        ("an alias of one character", Some("a".to_string())),
        ("a hyphenated alias", Some("corp-dev".to_string())),
        (
            "the longest alias the model admits",
            Some(format!("a{}", "b".repeat(61))),
        ),
    ];

    for (index, (case, alias)) in rows.iter().enumerate() {
        let directory_id = directory(&router, &format!("url{index}.example.com"), case).await;
        if let Some(alias) = alias {
            ok(
                &router,
                "CreateAlias",
                json!({ "DirectoryId": &directory_id, "Alias": alias }),
                case,
            )
            .await;
        }
        let described = ok(
            &router,
            "DescribeDirectories",
            json!({ "DirectoryIds": [&directory_id] }),
            case,
        )
        .await;
        let description = &described["DirectoryDescriptions"][0];
        let expected_alias = alias.clone().unwrap_or_else(|| directory_id.clone());
        let access_url = description["AccessUrl"].as_str().unwrap_or_default();
        assert_eq!(
            access_url,
            format!("{expected_alias}{ACCESS_URL_SUFFIX}"),
            "case {case:?}: expected the host the alias names, got {description}"
        );
        // A host and nothing else: no scheme, no path, no user info, no query.
        assert!(
            access_url
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-'),
            "case {case:?}: expected a bare host, got {access_url:?}"
        );
        assert!(
            access_url.starts_with(&expected_alias),
            "case {case:?}: expected the host to lead with the alias {expected_alias:?}, got \
             {access_url:?}"
        );
    }
}
