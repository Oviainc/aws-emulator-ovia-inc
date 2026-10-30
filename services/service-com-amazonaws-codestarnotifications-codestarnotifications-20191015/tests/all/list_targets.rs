// SPDX-License-Identifier: CC0-1.0

//! What `ListTargets` reports about the account's notification-rule targets, over the
//! wire.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * a target of the account is reported ONCE, whatever number of rules subscribe to its
//!   address, with the address, the type and the `TargetStatus` the registry holds;
//! * `Filters` are ORed within one filter name and ANDed across names, over every one of
//!   `ListTargetsFilterName`'s values;
//! * whatever page size a client asks for, following `NextToken` walks every target
//!   exactly once;
//! * `MaxResults` outside its modelled range, a `NextToken` this service did not mint,
//!   an oversized filter list and a filter name the enum does not list are refused;
//! * a listing is served by a regional endpoint, so it names only the targets the rules
//!   of its own region subscribe to;
//! * unsubscribing the last rule from an address drops the target from the listing.
//!
//! The harness (`Emulator`, `error_code`, `string`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};

use crate::notification_rule_lifecycle::{error_code, string, Emulator};

/// The target types this suite subscribes, which are `TargetSummary$TargetType`'s own
/// documented values ("Amazon Q Developer in chat applications topics are specified as
/// `SNS`", clients for Slack as `AWSChatbotSlack`, for Microsoft Teams as
/// `AWSChatbotMicrosoftTeams`) — none of them the one the issue behind this suite cited
/// on its own.
const TARGET_TYPES: [&str; 3] = ["SNS", "AWSChatbotSlack", "AWSChatbotMicrosoftTeams"];

/// One target of the seeded account: the address, and the type it was subscribed with.
struct SeededTarget {
    address: String,
    target_type: &'static str,
}

/// Create one rule per target in `region`, each rule carrying exactly one target, and
/// return the targets in the order they were first registered.
///
/// `label` keeps the rule names and the addresses of one call distinct from another's.
async fn seed_targets(
    emulator: &Emulator,
    region: &str,
    label: &str,
    count: usize,
) -> Vec<SeededTarget> {
    let mut seeded = Vec::with_capacity(count);
    for index in 0..count {
        let target_type = TARGET_TYPES[index % TARGET_TYPES.len()];
        let address = address_for(region, label, index, target_type);
        let (status, created) = emulator
            .post_in(
                region,
                "/createNotificationRule",
                &json!({
                    "Name": format!("{label}-{index}"),
                    "Resource": format!("arn:aws:codecommit:{region}:123456789012:Repo{index}"),
                    "DetailType": "BASIC",
                    "EventTypeIds": ["codecommit-repository-pull-request-created"],
                    "Targets": [{ "TargetType": target_type, "TargetAddress": address }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: seeding a rule must succeed, got {status} with {created}"
        );
        seeded.push(SeededTarget {
            address,
            target_type,
        });
    }
    seeded
}

/// An address of the form the target type uses: an SNS topic ARN for `SNS`, a chat
/// applications configuration ARN for the two client types. The region is part of the
/// address either way, so no two regions of one suite subscribe the same address.
fn address_for(region: &str, label: &str, index: usize, target_type: &str) -> String {
    match target_type {
        "SNS" => format!("arn:aws:sns:{region}:123456789012:{label}-topic-{index}"),
        other => format!(
            "arn:aws:chatbot::123456789012:chat-configuration/{}/{label}-{region}-{index}",
            other.to_ascii_lowercase()
        ),
    }
}

/// `ListTargets` in `region`, walking every page, as `(address, type, status)` triples in
/// the order they were served.
async fn list_all(
    emulator: &Emulator,
    case: &str,
    region: &str,
    request: &Value,
) -> Vec<(String, String, String)> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut body = request.clone();
        if let Some(value) = &token {
            body["NextToken"] = json!(value);
        }
        let (status, listed) = emulator.post_in(region, "/listTargets", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from /listTargets, got {status} with {listed}"
        );
        if let Some(max) = request.get("MaxResults").and_then(Value::as_u64) {
            let served = listed
                .get("Targets")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            assert!(
                served as u64 <= max,
                "case {case:?}: a page must not exceed MaxResults, got {listed}"
            );
        }
        seen.extend(summaries_of(&listed));
        pages += 1;
        assert!(
            pages <= 50,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// The `Targets` member as `(address, type, status)` triples.
fn summaries_of(body: &Value) -> Vec<(String, String, String)> {
    body.get("Targets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|summary| {
                    (
                        string(summary, "TargetAddress"),
                        string(summary, "TargetType"),
                        string(summary, "TargetStatus"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The addresses of a listing, in the order they were served.
fn addresses_of(body: &Value) -> Vec<String> {
    summaries_of(body)
        .into_iter()
        .map(|(address, _, _)| address)
        .collect()
}

/// RULE: a listing reports every target the region's rules subscribe to, each ONCE, with
/// the address and type it was subscribed with and the status the registry holds for it —
/// however many rules share the address.
///
/// The rows differ along what the rule generalises over: how many targets the account
/// holds and how many rules subscribe to the shared address.
#[tokio::test]
async fn every_subscribed_target_is_reported_once_with_its_stored_members() {
    for (case, count, sharing_rules) in [
        ("one target, one rule", 1usize, 0usize),
        ("three targets, one more rule on the first", 3, 1),
        ("five targets, three more rules on the first", 5, 3),
    ] {
        let emulator = Emulator::new();
        let seeded = seed_targets(&emulator, "us-east-1", "shared", count).await;

        // Extra rules subscribing to the FIRST seeded address: the registry holds one
        // target per address, so the listing must not report it once per rule.
        for extra in 0..sharing_rules {
            let created = emulator
                .ok(
                    case,
                    "/createNotificationRule",
                    &json!({
                        "Name": format!("sharer-{extra}"),
                        "Resource": format!("arn:aws:codebuild:us-east-1:123456789012:project/p{extra}"),
                        "DetailType": "FULL",
                        "EventTypeIds": ["codebuild-project-build-state-failed"],
                        "Targets": [{
                            "TargetType": seeded[0].target_type,
                            "TargetAddress": seeded[0].address,
                        }],
                    }),
                )
                .await;
            assert!(
                !string(&created, "Arn").is_empty(),
                "case {case:?}: the sharing rule must be created, got {created}"
            );
        }

        let served = list_all(&emulator, case, "us-east-1", &json!({})).await;
        let expected: Vec<(String, String, String)> = seeded
            .iter()
            .map(|target| {
                (
                    target.address.clone(),
                    target.target_type.to_string(),
                    // A subscription this service creates is usable when the call
                    // returns, so it is reported with the modelled terminal status.
                    "ACTIVE".to_string(),
                )
            })
            .collect();
        assert_eq!(
            served, expected,
            "case {case:?}: expected every target once with its stored members, got {served:?}"
        );
    }
}

/// RULE: `Filters` are ORed within one filter name and ANDed across names, over each of
/// `ListTargetsFilterName`'s values (`TARGET_ADDRESS`, `TARGET_TYPE`, `TARGET_STATUS`);
/// a value nothing holds matches nothing.
#[tokio::test]
async fn filters_or_within_a_name_and_and_across_names() {
    let emulator = Emulator::new();
    // Six targets, so each of the three types is held by exactly two of them.
    let seeded = seed_targets(&emulator, "us-east-1", "filtered", 6).await;
    let sns: Vec<String> = seeded
        .iter()
        .filter(|target| target.target_type == "SNS")
        .map(|target| target.address.clone())
        .collect();
    let slack: Vec<String> = seeded
        .iter()
        .filter(|target| target.target_type == "AWSChatbotSlack")
        .map(|target| target.address.clone())
        .collect();
    let all: Vec<String> = seeded
        .iter()
        .map(|target| target.address.clone())
        .collect();

    // (case, Filters, the addresses expected, in registration order)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filters lists every target",
            json!([]),
            all.clone(),
        ),
        (
            "one address",
            json!([{ "Name": "TARGET_ADDRESS", "Value": seeded[0].address }]),
            vec![seeded[0].address.clone()],
        ),
        (
            "two addresses are ORed",
            json!([
                { "Name": "TARGET_ADDRESS", "Value": seeded[0].address },
                { "Name": "TARGET_ADDRESS", "Value": seeded[4].address },
            ]),
            vec![seeded[0].address.clone(), seeded[4].address.clone()],
        ),
        ("one type", json!([{ "Name": "TARGET_TYPE", "Value": "SNS" }]), sns.clone()),
        (
            "two types are ORed",
            json!([
                { "Name": "TARGET_TYPE", "Value": "SNS" },
                { "Name": "TARGET_TYPE", "Value": "AWSChatbotSlack" },
            ]),
            all.iter()
                .filter(|address| sns.contains(address) || slack.contains(address))
                .cloned()
                .collect(),
        ),
        (
            "the status every subscription carries",
            json!([{ "Name": "TARGET_STATUS", "Value": "ACTIVE" }]),
            all.clone(),
        ),
        (
            "a modelled status nothing holds",
            json!([{ "Name": "TARGET_STATUS", "Value": "UNREACHABLE" }]),
            vec![],
        ),
        (
            "an address AND its own type",
            json!([
                { "Name": "TARGET_ADDRESS", "Value": seeded[0].address },
                { "Name": "TARGET_TYPE", "Value": seeded[0].target_type },
            ]),
            vec![seeded[0].address.clone()],
        ),
        (
            "an address AND a type it does not carry",
            json!([
                { "Name": "TARGET_ADDRESS", "Value": seeded[0].address },
                { "Name": "TARGET_TYPE", "Value": "AWSChatbotMicrosoftTeams" },
            ]),
            vec![],
        ),
        (
            "two addresses ORed, ANDed with one of their types",
            json!([
                { "Name": "TARGET_ADDRESS", "Value": seeded[0].address },
                { "Name": "TARGET_ADDRESS", "Value": seeded[1].address },
                { "Name": "TARGET_TYPE", "Value": seeded[1].target_type },
            ]),
            vec![seeded[1].address.clone()],
        ),
        (
            "an address nothing holds",
            json!([{
                "Name": "TARGET_ADDRESS",
                "Value": "arn:aws:sns:us-east-1:123456789012:nothing-subscribed",
            }]),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let listed = emulator
            .ok(case, "/listTargets", &json!({ "Filters": filters }))
            .await;
        let got = addresses_of(&listed);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

/// RULE: whatever page size a client asks for, following `NextToken` walks every target
/// exactly once, in the account's registration order.
///
/// The rows are the modelled minimum page size, sizes either side of the collection, the
/// whole collection in one page, the modelled maximum, and a request with no `MaxResults`
/// at all.
#[tokio::test]
async fn paging_with_any_max_results_walks_every_target_once() {
    let emulator = Emulator::new();
    let seeded = seed_targets(&emulator, "us-east-1", "paged", 7).await;
    let expected: Vec<String> = seeded
        .iter()
        .map(|target| target.address.clone())
        .collect();

    for max_results in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        let served: Vec<String> = list_all(&emulator, &case, "us-east-1", &request)
            .await
            .into_iter()
            .map(|(address, _, _)| address)
            .collect();
        assert_eq!(
            served, expected,
            "case {case:?}: paging must yield every target once, in registration order"
        );
    }
}

/// RULE: a page's `NextToken` is present exactly while targets remain, and resumes where
/// the previous page stopped.
#[tokio::test]
async fn a_next_token_is_served_exactly_while_targets_remain() {
    let emulator = Emulator::new();
    let seeded = seed_targets(&emulator, "us-east-1", "tokened", 3).await;

    let first = emulator
        .ok("page 1", "/listTargets", &json!({ "MaxResults": 2 }))
        .await;
    assert_eq!(
        addresses_of(&first),
        vec![seeded[0].address.clone(), seeded[1].address.clone()],
        "case \"page 1\": expected the first two targets, got {first}"
    );
    let token = string(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "case \"page 1\": a token must be served while targets remain, got {first}"
    );

    let second = emulator
        .ok(
            "page 2",
            "/listTargets",
            &json!({ "MaxResults": 2, "NextToken": token }),
        )
        .await;
    assert_eq!(
        addresses_of(&second),
        vec![seeded[2].address.clone()],
        "case \"page 2\": expected the last target, got {second}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case \"page 2\": the last page carries no token, got {second}"
    );
}

/// RULE: a request outside what the model and this service accept is refused with a 400
/// of the modelled shape — `MaxResults` outside its `@range(min: 1, max: 100)`, a
/// `NextToken` this service did not mint, a filter name `ListTargetsFilterName` does not
/// list, and more `Filters` entries than one request may carry.
#[tokio::test]
async fn a_request_outside_the_modelled_shape_is_refused() {
    let emulator = Emulator::new();
    seed_targets(&emulator, "us-east-1", "refused", 2).await;

    // (case, request, the status expected, the error code expected when it is refused)
    let long_filters: Vec<Value> = (0..33)
        .map(|index| {
            json!({
                "Name": "TARGET_ADDRESS",
                "Value": format!("arn:aws:sns:us-east-1:123456789012:topic-{index}"),
            })
        })
        .collect();
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        ("MaxResults below the modelled minimum", json!({ "MaxResults": 0 }), 400, "ValidationException"),
        ("MaxResults at the modelled minimum", json!({ "MaxResults": 1 }), 200, ""),
        ("MaxResults at the modelled maximum", json!({ "MaxResults": 100 }), 200, ""),
        ("MaxResults past the modelled maximum", json!({ "MaxResults": 101 }), 400, "ValidationException"),
        // Each token below satisfies `NextToken`'s `^[\w/+=]+$`, so it reaches the handler.
        ("a token this service did not mint", json!({ "NextToken": "notarealtoken" }), 400, "InvalidNextTokenException"),
        ("a token that is not hex", json!({ "NextToken": "AAAA====" }), 400, "InvalidNextTokenException"),
        ("a one-character token", json!({ "NextToken": "7" }), 400, "InvalidNextTokenException"),
        (
            "a filter name the enum does not list",
            json!({ "Filters": [{ "Name": "TARGET_NAME", "Value": "SNS" }] }),
            400,
            "ValidationException",
        ),
        (
            "a filter name of another listing's enum",
            json!({ "Filters": [{ "Name": "RESOURCE", "Value": "SNS" }] }),
            400,
            "ValidationException",
        ),
        ("one filter more than a request may carry", json!({ "Filters": long_filters }), 400, "ValidationException"),
        (
            "the most filters a request may carry",
            json!({
                "Filters": (0..32)
                    .map(|index| json!({
                        "Name": "TARGET_ADDRESS",
                        "Value": format!("arn:aws:sns:us-east-1:123456789012:topic-{index}"),
                    }))
                    .collect::<Vec<Value>>(),
            }),
            200,
            "",
        ),
    ];

    for (case, request, expected_status, expected_code) in rows {
        let (status, parsed) = emulator.post("/listTargets", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {parsed}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&parsed),
                expected_code,
                "case {case:?}: got {parsed}"
            );
        }
    }
}

/// RULE: a listing is served by a regional endpoint, so it names only the targets the
/// rules of its own region subscribe to.
#[tokio::test]
async fn each_region_lists_only_its_own_targets() {
    let emulator = Emulator::new();

    // The rows differ in the region and in how many targets it holds.
    let regions = [("us-east-1", 2usize), ("eu-west-1", 1), ("ap-south-1", 3)];
    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (region, count) in regions {
        let seeded = seed_targets(&emulator, region, "regional", count).await;
        expected.push((
            region,
            seeded
                .iter()
                .map(|target| target.address.clone())
                .collect(),
        ));
    }

    for (region, addresses) in &expected {
        let served: Vec<String> = list_all(&emulator, region, region, &json!({}))
            .await
            .into_iter()
            .map(|(address, _, _)| address)
            .collect();
        assert_eq!(
            &served, addresses,
            "case {region:?}: a region lists exactly its own targets, got {served:?}"
        );
    }

    // And a region that holds no rule at all holds no target.
    let served = list_all(&emulator, "an unused region", "us-west-2", &json!({})).await;
    assert!(
        served.is_empty(),
        "case \"an unused region\": expected no targets, got {served:?}"
    );
}

/// RULE: the registry holds a target while at least one rule subscribes to its address,
/// so `Subscribe` makes it appear and unsubscribing the LAST rule from it makes it
/// disappear — the listing is computed from the store, not from what a create once said.
#[tokio::test]
async fn subscribing_and_unsubscribing_moves_a_target_in_and_out_of_the_listing() {
    let emulator = Emulator::new();
    let seeded = seed_targets(&emulator, "us-east-1", "lifecycle", 1).await;
    let listed = emulator.ok("seeded", "/listTargets", &json!({})).await;
    let first_rule_targets = addresses_of(&listed);

    // Two rules, so the second subscription proves the address survives the first
    // unsubscribe and only disappears when the last rule lets it go.
    let second = emulator
        .ok(
            "second rule",
            "/createNotificationRule",
            &json!({
                "Name": "lifecycle-second",
                "Resource": "arn:aws:codecommit:us-east-1:123456789012:Other",
                "DetailType": "BASIC",
                "EventTypeIds": ["codecommit-repository-pull-request-created"],
                "Targets": [],
            }),
        )
        .await;
    let second_arn = string(&second, "Arn");
    let late_address = "arn:aws:sns:us-east-1:123456789012:lifecycle-late";

    // A late subscription appears in the listing.
    emulator
        .ok(
            "subscribe",
            "/subscribe",
            &json!({
                "Arn": second_arn,
                "Target": { "TargetType": "SNS", "TargetAddress": late_address },
            }),
        )
        .await;
    let listed = emulator.ok("subscribe", "/listTargets", &json!({})).await;
    let mut expected = first_rule_targets.clone();
    expected.push(late_address.to_string());
    assert_eq!(
        addresses_of(&listed),
        expected,
        "case \"subscribe\": a newly subscribed address joins the listing, got {listed}"
    );

    // The same address on a SECOND rule is still ONE target.
    emulator
        .ok(
            "share",
            "/subscribe",
            &json!({
                "Arn": second_arn,
                "Target": { "TargetType": "SNS", "TargetAddress": seeded[0].address },
            }),
        )
        .await;
    let listed = emulator.ok("share", "/listTargets", &json!({})).await;
    assert_eq!(
        addresses_of(&listed),
        expected,
        "case \"share\": a shared address is one target, got {listed}"
    );

    // Unsubscribing the second rule leaves the first rule's subscription standing.
    emulator
        .ok(
            "unsubscribe one of two",
            "/unsubscribe",
            &json!({ "Arn": second_arn, "TargetAddress": seeded[0].address }),
        )
        .await;
    let listed = emulator
        .ok("unsubscribe one of two", "/listTargets", &json!({}))
        .await;
    assert_eq!(
        addresses_of(&listed),
        expected,
        "case \"unsubscribe one of two\": the address another rule holds stays, got {listed}"
    );

    // Unsubscribing the LAST rule from the late address drops the target.
    emulator
        .ok(
            "unsubscribe the last",
            "/unsubscribe",
            &json!({ "Arn": second_arn, "TargetAddress": late_address }),
        )
        .await;
    let listed = emulator
        .ok("unsubscribe the last", "/listTargets", &json!({}))
        .await;
    assert_eq!(
        addresses_of(&listed),
        first_rule_targets,
        "case \"unsubscribe the last\": the released address leaves the listing, got {listed}"
    );
}

/// RULE: listing is REPORTING — two identical listings report the same targets, and a
/// listing changes nothing another caller then sees.
#[tokio::test]
async fn listing_twice_reports_the_same_targets() {
    let emulator = Emulator::new();
    seed_targets(&emulator, "us-east-1", "stable", 4).await;

    let first = list_all(&emulator, "first read", "us-east-1", &json!({})).await;
    let paged = list_all(
        &emulator,
        "a paged read in between",
        "us-east-1",
        &json!({ "MaxResults": 1 }),
    )
    .await;
    let second = list_all(&emulator, "second read", "us-east-1", &json!({})).await;

    assert_eq!(
        first, second,
        "case \"two reads\": a read must not change what the next caller sees, got \
         {first:?} then {second:?}"
    );
    assert_eq!(
        paged, first,
        "case \"a paged read in between\": paging reports the same targets, got {paged:?}"
    );
}
