// SPDX-License-Identifier: CC0-1.0

//! The Directory Service log subscriptions, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, using
//! the shared `harness` plumbing, so nothing here reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: a directory
//! forwards its domain controller security logs to AT MOST ONE log group, created by
//! `CreateLogSubscription`, reported by `ListLogSubscriptions` and removed by
//! `DeleteLogSubscription`. The rows span what the rule generalises over — the form and
//! length of the `LogGroupName` the model admits (both published bounds and one past the
//! upper one), the directory type it is created for, how many directories hold a
//! subscription, the filter the listing carries and the page size it asks for.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{create_directory, minimal_create};
use crate::event_topics::epoch_seconds;
use crate::harness::{call, ok, refused, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The published bounds of the modelled `LogGroupName` shape: `length 1..512`
/// (`smithy_query(query="shape",shape="LogSubscription")`).
const LOG_GROUP_NAME_MIN_LEN: usize = 1;
const LOG_GROUP_NAME_MAX_LEN: usize = 512;

/// `CreateLogSubscription` for `log_group_name`, asserting it was accepted.
async fn subscribe(
    router: &MultiServiceRouter,
    directory_id: &str,
    log_group_name: &str,
    case: &str,
) {
    ok(
        router,
        "CreateLogSubscription",
        json!({ "DirectoryId": directory_id, "LogGroupName": log_group_name }),
        case,
    )
    .await;
}

/// `ListLogSubscriptions` with `body`, returning its `LogSubscriptions` list.
async fn list(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<Value> {
    let answer = ok(router, "ListLogSubscriptions", body, case).await;
    answer["LogSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected LogSubscriptions, got {answer}"))
        .clone()
}

/// The `(DirectoryId, LogGroupName)` pairs a listing reports, as a set: the subscription
/// identity, independent of the order the list came back in.
fn reported_pairs(subscriptions: &[Value]) -> BTreeSet<(String, String)> {
    subscriptions
        .iter()
        .map(|subscription| {
            (
                subscription["DirectoryId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                subscription["LogGroupName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// One row of the subscription table: a directory and the log group its logs go to.
struct SubscriptionCase {
    case: &'static str,
    directory: &'static str,
    log_group_name: String,
    /// Whether the directory is a Managed Microsoft AD one rather than a Simple AD one,
    /// so the rule is exercised over both directory types the emulator creates.
    microsoft_ad: bool,
}

/// One row of the filter table: a `ListLogSubscriptions` request and the subscriptions it
/// must narrow the account's to.
struct FilterCase {
    case: String,
    request: Value,
    expected: BTreeSet<(String, String)>,
}

#[tokio::test]
async fn a_created_log_subscription_is_reported_from_persisted_state() {
    let router = service();

    // The rows differ along everything a subscription varies in: the form and length of
    // the `LogGroupName` the modelled pattern `^[-._/#A-Za-z0-9]+$` admits — letters,
    // digits, hyphens, dots, underscores, slashes and hashes, at both published length
    // bounds — and the type of directory the logs are forwarded for.
    let cases = vec![
        SubscriptionCase {
            case: "a path-shaped log group name",
            directory: "corp.example.com",
            log_group_name: "/aws/directoryservice/security".to_string(),
            microsoft_ad: false,
        },
        SubscriptionCase {
            case: "every character class the pattern admits",
            directory: "mixed.example.com",
            log_group_name: "Group-1_2.3/four#five".to_string(),
            microsoft_ad: false,
        },
        SubscriptionCase {
            case: "digits only",
            directory: "digits.example.com",
            log_group_name: "20150416".to_string(),
            microsoft_ad: false,
        },
        SubscriptionCase {
            case: "shortest name the model admits",
            directory: "short.example.com",
            log_group_name: "a".repeat(LOG_GROUP_NAME_MIN_LEN),
            microsoft_ad: false,
        },
        SubscriptionCase {
            case: "longest name the model admits",
            directory: "long.example.com",
            log_group_name: "x".repeat(LOG_GROUP_NAME_MAX_LEN),
            microsoft_ad: false,
        },
        SubscriptionCase {
            case: "a Managed Microsoft AD directory",
            directory: "managed.example.com",
            log_group_name: "/aws/directoryservice/managed".to_string(),
            microsoft_ad: true,
        },
    ];

    for case in &cases {
        let label = case.case;
        let directory_id = if case.microsoft_ad {
            create_microsoft_ad(&router, minimal_microsoft_ad(case.directory), label).await
        } else {
            create_directory(&router, minimal_create(case.directory), label).await
        };

        // A directory that forwards its logs nowhere reports an empty list, not a 501 and
        // not a subscription nobody created.
        let before = list(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: before subscribing"),
        )
        .await;
        assert!(
            before.is_empty(),
            "case {label:?}: expected no subscription before CreateLogSubscription, got {before:?}"
        );

        let created_from = epoch_seconds(SystemTime::now());
        subscribe(&router, &directory_id, &case.log_group_name, label).await;
        let created_until = epoch_seconds(SystemTime::now());

        let reported = list(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: after subscribing"),
        )
        .await;
        assert_eq!(
            reported.len(),
            1,
            "case {label:?}: expected exactly one subscription, got {reported:?}"
        );
        assert_eq!(
            reported[0]["DirectoryId"],
            json!(&directory_id),
            "case {label:?}: expected the subscription to name its directory, got {}",
            reported[0]
        );
        assert_eq!(
            reported[0]["LogGroupName"],
            json!(&case.log_group_name),
            "case {label:?}: expected the stored log group name, got {}",
            reported[0]
        );
        let created = reported[0]["SubscriptionCreatedDateTime"]
            .as_f64()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected a SubscriptionCreatedDateTime timestamp, got {}",
                    reported[0]
                )
            });
        assert!(
            (created_from - 1.0..=created_until + 1.0).contains(&created),
            "case {label:?}: expected SubscriptionCreatedDateTime within the subscribing \
             window [{created_from}, {created_until}], got {created}"
        );

        // A read does not change what the next caller sees.
        let again = list(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: repeated read"),
        )
        .await;
        assert_eq!(
            reported, again,
            "case {label:?}: expected a repeated ListLogSubscriptions to report the same \
             subscription, got {again:?}"
        );

        // A directory forwards its logs to at most one log group, so a second create is
        // refused — for the same log group and for another one alike — and the stored
        // subscription is left exactly as it was.
        for second in [case.log_group_name.clone(), "another-group".to_string()] {
            refused(
                &router,
                "CreateLogSubscription",
                json!({ "DirectoryId": &directory_id, "LogGroupName": &second }),
                "EntityAlreadyExistsException",
                &format!("{label}: second subscription to {second}"),
            )
            .await;
        }
        let after_refusal = list(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: after a refused second create"),
        )
        .await;
        assert_eq!(
            after_refusal, reported,
            "case {label:?}: expected a refused second create to change nothing, got \
             {after_refusal:?}"
        );

        // Deleting it takes the subscription away...
        ok(
            &router,
            "DeleteLogSubscription",
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: delete"),
        )
        .await;
        let after_delete = list(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: after delete"),
        )
        .await;
        assert!(
            after_delete.is_empty(),
            "case {label:?}: expected the deleted subscription to be gone, got {after_delete:?}"
        );

        // ...a second delete has nothing left to delete...
        refused(
            &router,
            "DeleteLogSubscription",
            json!({ "DirectoryId": &directory_id }),
            "EntityDoesNotExistException",
            &format!("{label}: repeated delete"),
        )
        .await;

        // ...and the directory can be subscribed again, which is a NEW subscription: its
        // created timestamp is not the deleted one's.
        let resubscribed_from = epoch_seconds(SystemTime::now());
        subscribe(
            &router,
            &directory_id,
            &case.log_group_name,
            &format!("{label}: resubscribe"),
        )
        .await;
        let resubscribed = list(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: after resubscribing"),
        )
        .await;
        assert_eq!(
            reported_pairs(&resubscribed),
            [(directory_id.clone(), case.log_group_name.clone())]
                .into_iter()
                .collect::<BTreeSet<(String, String)>>(),
            "case {label:?}: expected the directory to be subscribed again, got {resubscribed:?}"
        );
        let recreated = resubscribed[0]["SubscriptionCreatedDateTime"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            recreated >= resubscribed_from - 1.0,
            "case {label:?}: expected the new subscription's created time to be at or after \
             {resubscribed_from}, got {recreated}"
        );
    }

    // One past the upper `LogGroupName` length bound is refused by the modelled
    // constraint, before any handler runs, and stores nothing.
    let directory_id =
        create_directory(&router, minimal_create("bounds.example.com"), "bounds").await;
    let (status, parsed) = call(
        &router,
        "CreateLogSubscription",
        json!({
            "DirectoryId": &directory_id,
            "LogGroupName": "y".repeat(LOG_GROUP_NAME_MAX_LEN + 1)
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the LogGroupName length bound\": expected 400, got {status} with {parsed}"
    );
    let remaining = list(
        &router,
        json!({ "DirectoryId": &directory_id }),
        "one past the LogGroupName length bound",
    )
    .await;
    assert!(
        remaining.is_empty(),
        "case \"one past the LogGroupName length bound\": expected a refused create to store \
         nothing, got {remaining:?}"
    );
}

#[tokio::test]
async fn log_subscriptions_are_narrowed_by_the_directory_the_request_names() {
    let router = service();

    // Three directories: two forward their logs, one forwards none.
    let first = create_directory(&router, minimal_create("one.example.com"), "filters").await;
    let second = create_directory(&router, minimal_create("two.example.com"), "filters").await;
    let quiet = create_directory(&router, minimal_create("quiet.example.com"), "filters").await;
    subscribe(&router, &first, "/aws/one", "filters").await;
    subscribe(&router, &second, "two-group", "filters").await;

    let everything: BTreeSet<(String, String)> = [
        (first.clone(), "/aws/one".to_string()),
        (second.clone(), "two-group".to_string()),
    ]
    .into_iter()
    .collect();

    // The rows differ along which directory the request names: none (the whole account),
    // each subscribed directory, and a directory that holds no subscription.
    let cases: Vec<FilterCase> = vec![
        FilterCase {
            case: "no filter: every subscription in the account".to_string(),
            request: json!({}),
            expected: everything.clone(),
        },
        FilterCase {
            case: "the first directory".to_string(),
            request: json!({ "DirectoryId": &first }),
            expected: [(first.clone(), "/aws/one".to_string())]
                .into_iter()
                .collect(),
        },
        FilterCase {
            case: "the second directory".to_string(),
            request: json!({ "DirectoryId": &second }),
            expected: [(second.clone(), "two-group".to_string())]
                .into_iter()
                .collect(),
        },
        FilterCase {
            case: "a directory that forwards no logs".to_string(),
            request: json!({ "DirectoryId": &quiet }),
            expected: BTreeSet::new(),
        },
    ];

    for FilterCase {
        case,
        request,
        expected,
    } in cases
    {
        let reported = list(&router, request, &case).await;
        assert_eq!(
            reported_pairs(&reported),
            expected,
            "case {case:?}: expected the filter to narrow the result, got {reported:?}"
        );
    }

    // A deleted directory takes its own subscription with it, once its teardown has run
    // out: the account's subscriptions are then the surviving directory's.
    crate::harness::delete_directory_and_wait(
        &router,
        &second,
        "filters: delete the second directory",
    )
    .await;
    let surviving = list(&router, json!({}), "filters: after deleting a directory").await;
    assert_eq!(
        reported_pairs(&surviving),
        [(first.clone(), "/aws/one".to_string())]
            .into_iter()
            .collect::<BTreeSet<(String, String)>>(),
        "case \"after deleting a directory\": expected only the surviving directory's \
         subscription, got {surviving:?}"
    );
}

#[tokio::test]
async fn a_directory_this_account_does_not_hold_is_refused_by_every_operation() {
    let router = service();

    let existing = create_directory(&router, minimal_create("present.example.com"), "absent").await;
    subscribe(&router, &existing, "kept-group", "absent").await;

    // A directory this account never created, in several forms the modelled `DirectoryId`
    // pattern `^d-[0-9a-f]{10}$` admits...
    let mut absent: Vec<String> = vec![
        "d-0000000000".to_string(),
        "d-00000000ff".to_string(),
        "d-abcdef0123".to_string(),
        "d-ffffffffff".to_string(),
    ];
    // ...and one that existed, with a subscription, until it was deleted, so the refusal
    // follows the store rather than the shape of the id.
    let doomed = create_directory(&router, minimal_create("doomed.example.com"), "absent").await;
    subscribe(&router, &doomed, "doomed-group", "absent").await;
    crate::harness::delete_directory_and_wait(&router, &doomed, "absent: delete").await;
    absent.push(doomed.clone());

    for directory_id in &absent {
        for (operation, body) in [
            (
                "CreateLogSubscription",
                json!({ "DirectoryId": directory_id, "LogGroupName": "nowhere" }),
            ),
            (
                "DeleteLogSubscription",
                json!({ "DirectoryId": directory_id }),
            ),
            (
                "ListLogSubscriptions",
                json!({ "DirectoryId": directory_id }),
            ),
        ] {
            let case = format!("{operation} against {directory_id}");
            refused(
                &router,
                operation,
                body.clone(),
                "EntityDoesNotExistException",
                &case,
            )
            .await;
            // The refusal is built from the request: it quotes the id the caller asked
            // for, so the caller can fix the request from the message alone.
            let (_, parsed) = call(&router, operation, body).await;
            let message = parsed["message"]
                .as_str()
                .or_else(|| parsed["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(&format!("'{directory_id}'")),
                "case {case:?}: expected the message to name the requested directory, got {parsed}"
            );
        }
    }

    // A directory that exists but forwards no logs is a different failure: the delete is
    // refused for the missing SUBSCRIPTION, naming that directory.
    let unsubscribed =
        create_directory(&router, minimal_create("silent.example.com"), "absent").await;
    let (_, parsed) = call(
        &router,
        "DeleteLogSubscription",
        json!({ "DirectoryId": &unsubscribed }),
    )
    .await;
    let message = parsed["message"]
        .as_str()
        .or_else(|| parsed["Message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains(&format!("for directory: {unsubscribed}")),
        "case \"delete without a subscription\": expected the message to name the missing log \
         subscription, got {parsed}"
    );

    // Every refused call stored nothing: the account's subscriptions are the one that was
    // created.
    let reported = list(&router, json!({}), "absent: account-wide").await;
    assert_eq!(
        reported_pairs(&reported),
        [(existing.clone(), "kept-group".to_string())]
            .into_iter()
            .collect::<BTreeSet<(String, String)>>(),
        "case \"absent\": expected only the surviving subscription, got {reported:?}"
    );
}

#[tokio::test]
async fn the_listing_pages_over_the_accounts_subscriptions() {
    let router = service();

    // Five subscribed directories, so every page size below is exercised against more
    // subscriptions than it can carry.
    let mut expected: BTreeSet<(String, String)> = BTreeSet::new();
    for index in 0..5 {
        let directory_id = create_directory(
            &router,
            minimal_create(&format!("page{index}.example.com")),
            "paging",
        )
        .await;
        let log_group_name = format!("/aws/page/{index}");
        subscribe(&router, &directory_id, &log_group_name, "paging").await;
        expected.insert((directory_id, log_group_name));
    }

    // The rows differ along the page size the request asks for: one at a time, a size
    // that does not divide the total, a size equal to the total and one past it. Every
    // one of them walks the whole account exactly once.
    for limit in [1_usize, 2, 5, 6] {
        let case = format!("paging by {limit}");
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "Limit": limit });
            if let Some(cursor) = token.as_deref() {
                request["NextToken"] = json!(cursor);
            }
            let answer = ok(&router, "ListLogSubscriptions", request, &case).await;
            let page = answer["LogSubscriptions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected LogSubscriptions, got {answer}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case {case:?}: expected at most {limit} per page, got {}",
                page.len()
            );
            seen.extend(page);
            token = answer["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: expected the listing to terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            reported_pairs(&seen),
            expected,
            "case {case:?}: expected every subscription exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            expected.len(),
            "case {case:?}: expected no subscription twice, got {seen:?}"
        );
    }

    // A token this service did not issue is refused rather than served as a page.
    for (case, body) in [
        (
            "a token this service did not issue",
            json!({ "NextToken": "not-a-token" }),
        ),
        ("an empty token", json!({ "NextToken": "" })),
    ] {
        refused(
            &router,
            "ListLogSubscriptions",
            body,
            "InvalidNextTokenException",
            case,
        )
        .await;
    }

    // ...and so is a token this listing issued under another filter: a cursor cannot
    // resume a listing it was not issued for.
    let unfiltered = ok(
        &router,
        "ListLogSubscriptions",
        json!({ "Limit": 1 }),
        "a token issued unfiltered",
    )
    .await;
    let issued = string(&unfiltered, "NextToken", "a token issued unfiltered");
    let (subscribed_directory, _) = expected
        .iter()
        .next()
        .expect("the paging table created subscriptions")
        .clone();
    refused(
        &router,
        "ListLogSubscriptions",
        json!({ "DirectoryId": &subscribed_directory, "NextToken": &issued, "Limit": 1 }),
        "InvalidNextTokenException",
        "a token replayed under another filter",
    )
    .await;
}
