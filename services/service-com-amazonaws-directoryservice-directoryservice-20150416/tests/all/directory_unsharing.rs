// SPDX-License-Identifier: CC0-1.0

//! Stopping the sharing of a Managed Microsoft AD directory with one consumer account,
//! driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`). Nothing here reaches into
//! the state struct, so a response that is not computed from persisted state cannot pass:
//! an `UnshareDirectory` request carries the owner directory and the consumer account but
//! NOT the consumer's `SharedDirectoryId`, so reporting that id back can only come from
//! the stored share, and every teardown is read back through
//! `DescribeSharedDirectories`.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the teardown
//! over a table of shares spanning the whole modelled `ShareMethod` catalog and every
//! status a stored share can be in when the call arrives; the refusals over tables of
//! directory ids that name nothing, of targets that are not account ids and of pairings
//! the named directory does not hold (including a directory of a type that is never
//! shared); and the re-share the completed teardown allows.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{create_directory, minimal_create, PAST_ONE_SPAN};
use crate::harness::{call, ok, refused, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};
use crate::shared_directories::{
    consumer_account, describe_one_share, describe_shares, share, share_body, share_list,
    shared_ids, SHARE_METHOD_CATALOG, TARGET_TYPE_CATALOG,
};

/// The `ShareStatus` a handshake share rests at until the consumer account answers: the
/// `PendingAcceptance` value of the modelled `ShareStatus` enum
/// (`smithy_query(query="shape",shape="ShareStatus")`).
const SHARE_STATUS_PENDING_ACCEPTANCE: &str = "PendingAcceptance";

/// The `ShareStatus` an organization share carries between `ShareDirectory` and the
/// transition the clock owes: the enum's `Sharing` value.
const SHARE_STATUS_SHARING: &str = "Sharing";

/// The `ShareStatus` an organization share settles on: the enum's `Shared` value.
const SHARE_STATUS_SHARED: &str = "Shared";

/// The `ShareStatus` a share carries while `UnshareDirectory`'s teardown is outstanding:
/// the enum's `Deleting` value.
const SHARE_STATUS_DELETING: &str = "Deleting";

/// An `UnshareDirectory` body: the owner directory and the consumer account as an
/// `ACCOUNT` target.
fn unshare_body(directory_id: &str, account_id: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "UnshareTarget": { "Id": account_id, "Type": TARGET_TYPE_CATALOG[0] }
    })
}

/// `UnshareDirectory` with the pairing, asserting it was accepted and returning the
/// `SharedDirectoryId` it reported.
async fn unshare(
    router: &MultiServiceRouter,
    directory_id: &str,
    account_id: &str,
    case: &str,
) -> String {
    let answer = ok(
        router,
        "UnshareDirectory",
        unshare_body(directory_id, account_id),
        case,
    )
    .await;
    string(&answer, "SharedDirectoryId", case)
}

/// The timestamp a share reports in `member`, as the wire carries it.
fn timestamp(share: &Value, member: &str, case: &str) -> f64 {
    share[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {share}"))
}

/// One row of the teardown table: the share the call arrives at, and the status it is in
/// by then.
struct UnshareCase {
    name: &'static str,
    method: &'static str,
    /// Whether the row waits for the transition a share owes before unsharing, so the
    /// call arrives at a settled share rather than a transient one.
    settle: bool,
    status_before: &'static str,
}

#[tokio::test]
async fn unsharing_tears_down_the_one_pairing_whatever_status_it_is_in() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("unshare.example.com"),
        "unshare table",
    )
    .await;

    // A share of the same directory with a consumer account no row unshares: what proves
    // the call stops ONE pairing and leaves the directory's other shares alone.
    let bystander_account = consumer_account(90);
    let bystander = share(
        &router,
        share_body(
            &owner,
            &bystander_account,
            SHARE_METHOD_CATALOG[1],
            Some("kept"),
        ),
        "bystander",
    )
    .await;

    // The rows span the whole modelled `ShareMethod` catalog and, with it, every status a
    // stored share can be in when the unshare arrives: the transient `Sharing` an
    // organization share starts at, the `Shared` it settles on, and the
    // `PendingAcceptance` a handshake share rests at whether or not a span has passed.
    let cases = [
        UnshareCase {
            name: "organizations, still sharing",
            method: SHARE_METHOD_CATALOG[0],
            settle: false,
            status_before: SHARE_STATUS_SHARING,
        },
        UnshareCase {
            name: "organizations, settled on shared",
            method: SHARE_METHOD_CATALOG[0],
            settle: true,
            status_before: SHARE_STATUS_SHARED,
        },
        UnshareCase {
            name: "handshake, pending acceptance",
            method: SHARE_METHOD_CATALOG[1],
            settle: false,
            status_before: SHARE_STATUS_PENDING_ACCEPTANCE,
        },
        UnshareCase {
            name: "handshake, still pending past one span",
            method: SHARE_METHOD_CATALOG[1],
            settle: true,
            status_before: SHARE_STATUS_PENDING_ACCEPTANCE,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let account = consumer_account(index);
        let shared_id = share(
            &router,
            share_body(&owner, &account, case.method, None),
            case.name,
        )
        .await;
        if case.settle {
            tokio::time::sleep(PAST_ONE_SPAN).await;
        }

        let before = describe_one_share(&router, &owner, &shared_id, case.name).await;
        assert_eq!(
            before["ShareStatus"],
            json!(case.status_before),
            "case {name:?}: expected the share to be {status:?} before the unshare, got \
             {before}",
            name = case.name,
            status = case.status_before
        );

        // The request names the directory and the consumer account; the id it answers
        // with is the one the stored share holds for that pairing.
        let reported = unshare(&router, &owner, &account, case.name).await;
        assert_eq!(
            reported, shared_id,
            "case {name:?}: expected the stored shared directory id {shared_id}, got \
             {reported}",
            name = case.name
        );

        // The span is never zero, so a caller reading straight afterwards still sees the
        // teardown — and sees it on the share it was persisted against, with the creation
        // time untouched.
        let tearing_down = describe_one_share(&router, &owner, &shared_id, case.name).await;
        assert_eq!(
            tearing_down["ShareStatus"],
            json!(SHARE_STATUS_DELETING),
            "case {name:?}: expected the share to be tearing down, got {tearing_down}",
            name = case.name
        );
        assert_eq!(
            tearing_down["CreatedDateTime"], before["CreatedDateTime"],
            "case {name:?}: expected the creation time to be unchanged by the unshare, \
             got {tearing_down}",
            name = case.name
        );
        let (was, now) = (
            timestamp(&before, "LastUpdatedDateTime", case.name),
            timestamp(&tearing_down, "LastUpdatedDateTime", case.name),
        );
        assert!(
            now >= was,
            "case {name:?}: expected the unshare to carry the update forward, got {now} \
             after {was}",
            name = case.name
        );

        tokio::time::sleep(PAST_ONE_SPAN).await;

        // Unsharing takes the consumer account's shared directory away, so once the
        // teardown falls due there is nothing left to report for it.
        let page = describe_shares(
            &router,
            &owner,
            vec![("SharedDirectoryIds", json!([&shared_id]))],
            case.name,
        )
        .await;
        assert!(
            share_list(&page, case.name).is_empty(),
            "case {name:?}: expected the unshared directory to be gone, got {page}",
            name = case.name
        );
    }

    // Every other share of the directory is exactly as it was.
    let page = describe_shares(&router, &owner, vec![], "bystander").await;
    assert_eq!(
        shared_ids(&page, "bystander"),
        vec![bystander.clone()],
        "expected only the share no row unshared to be left, got {page}"
    );
    let kept = describe_one_share(&router, &owner, &bystander, "bystander").await;
    assert_eq!(
        kept["ShareStatus"],
        json!(SHARE_STATUS_PENDING_ACCEPTANCE),
        "expected the bystander share to be untouched, got {kept}"
    );
    assert_eq!(
        kept["SharedAccountId"],
        json!(&bystander_account),
        "expected the bystander share to still name its consumer account, got {kept}"
    );
}

#[tokio::test]
async fn unsharing_resolves_the_directory_the_target_and_the_pairing() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("resolve.example.com"),
        "resolve",
    )
    .await;
    let other_owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("other.example.com"),
        "resolve",
    )
    .await;
    // A directory of a type sharing is not supported for holds no share at all, so a
    // pairing naming it is one this service does not hold.
    let simple_ad = create_directory(&router, minimal_create("simple.example.com"), "resolve").await;

    let account = consumer_account(11);
    let shared_id = share(
        &router,
        share_body(&owner, &account, SHARE_METHOD_CATALOG[1], None),
        "resolve",
    )
    .await;
    let other_account = consumer_account(12);
    let elsewhere = share(
        &router,
        share_body(&other_owner, &other_account, SHARE_METHOD_CATALOG[0], None),
        "resolve",
    )
    .await;

    // Well-formed `DirectoryId`s that name nothing this service holds.
    for absent in ["d-0123456789", "d-ffffffffff", "d-abcdef0123"] {
        refused(
            &router,
            "UnshareDirectory",
            unshare_body(absent, &account),
            "EntityDoesNotExistException",
            absent,
        )
        .await;
    }

    // An `ACCOUNT` target is identified by the twelve digits the modelled `CustomerId`
    // pattern `^(\d{12})$` admits (`smithy_query(query="shape",shape="SharedDirectory")`,
    // member `SharedAccountId`), so the rows span the digit count on both sides of it,
    // non-digits, separators and blanks.
    let not_account_ids: [(&str, &str); 7] = [
        ("eleven digits", "11112222333"),
        ("thirteen digits", "1111222233334"),
        ("one digit", "1"),
        ("alphanumeric", "acct11112222"),
        ("dashed", "1111-2222-3333"),
        ("blank", " "),
        ("padded account id", " 111122223333 "),
    ];
    for (name, target) in not_account_ids {
        refused(
            &router,
            "UnshareDirectory",
            unshare_body(&owner, target),
            "InvalidTargetException",
            name,
        )
        .await;
    }

    // A target `Type` outside the modelled `TargetType` catalog never reaches the
    // handler: the member constraint is the framework's to refuse.
    let (status, answer) = call(
        &router,
        "UnshareDirectory",
        json!({
            "DirectoryId": &owner,
            "UnshareTarget": { "Id": &account, "Type": "ORGANIZATION" }
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an off-catalog UnshareTarget.Type to be refused, got {status} with \
         {answer}"
    );

    // A pairing the named directory does not hold: an account it was never shared with,
    // an account another directory is shared with, its own consumer account against
    // another directory, and a directory that is never shared at all.
    let not_shared: [(&str, &str, &str); 4] = [
        ("never shared with this account", &owner, "111122223333"),
        ("another directory's consumer account", &owner, &other_account),
        (
            "this directory's consumer account, another directory",
            &other_owner,
            &account,
        ),
        ("a directory type that is never shared", &simple_ad, &account),
    ];
    for (name, directory, target) in not_shared {
        refused(
            &router,
            "UnshareDirectory",
            unshare_body(directory, target),
            "DirectoryNotSharedException",
            name,
        )
        .await;
    }

    // Every refused call changed nothing: both shares are as they were, and neither
    // directory lost or gained one.
    let still_shared = describe_one_share(&router, &owner, &shared_id, "unchanged").await;
    assert_eq!(
        still_shared["ShareStatus"],
        json!(SHARE_STATUS_PENDING_ACCEPTANCE),
        "expected the refusals to have left the share alone, got {still_shared}"
    );
    assert_eq!(
        shared_ids(
            &describe_shares(&router, &owner, vec![], "unchanged").await,
            "unchanged"
        ),
        vec![shared_id],
        "expected the owner directory to still hold exactly its own share"
    );
    assert_eq!(
        shared_ids(
            &describe_shares(&router, &other_owner, vec![], "unchanged").await,
            "unchanged"
        ),
        vec![elsewhere],
        "expected the other directory to still hold exactly its own share"
    );
}

#[tokio::test]
async fn an_unshared_pairing_is_shared_again_once_the_teardown_falls_due() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("reshare.example.com"),
        "reshare",
    )
    .await;
    let account = consumer_account(21);

    let first = share(
        &router,
        share_body(&owner, &account, SHARE_METHOD_CATALOG[0], None),
        "reshare",
    )
    .await;
    let reported = unshare(&router, &owner, &account, "reshare").await;
    assert_eq!(
        reported, first,
        "expected the unshare to report the stored shared directory id {first}, got \
         {reported}"
    );

    // While the teardown is outstanding the share is still there, so a repeat of the
    // unshare names it rather than being refused, and the pairing cannot be shared again.
    let repeat = unshare(&router, &owner, &account, "repeat unshare").await;
    assert_eq!(
        repeat, first,
        "expected a repeated unshare to report the same shared directory id {first}, got \
         {repeat}"
    );
    refused(
        &router,
        "ShareDirectory",
        share_body(&owner, &account, SHARE_METHOD_CATALOG[0], None),
        "DirectoryAlreadySharedException",
        "re-share while the teardown is outstanding",
    )
    .await;

    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Once it has fallen due the share is gone: unsharing the pairing again is the
    // modelled not-shared refusal, and sharing it mints a shared directory id of its own.
    refused(
        &router,
        "UnshareDirectory",
        unshare_body(&owner, &account),
        "DirectoryNotSharedException",
        "unshare after the teardown fell due",
    )
    .await;
    let second = share(
        &router,
        share_body(&owner, &account, SHARE_METHOD_CATALOG[1], None),
        "reshare",
    )
    .await;
    assert_ne!(
        second, first,
        "expected the re-shared pairing to mint a fresh shared directory id, got {first} \
         again"
    );

    let page = describe_shares(&router, &owner, vec![], "reshare").await;
    assert_eq!(
        shared_ids(&page, "reshare"),
        vec![second.clone()],
        "expected only the re-shared pairing to be stored, got {page}"
    );
    let reported = describe_one_share(&router, &owner, &second, "reshare").await;
    assert_eq!(
        reported["ShareStatus"],
        json!(SHARE_STATUS_PENDING_ACCEPTANCE),
        "expected the re-shared pairing to report the method the new request asked for, \
         got {reported}"
    );
}
