// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetWebAccountSettings`: the account's web function
//! quotas and the usage its stored resources add up to.
//!
//! Every assertion goes over the wire through the same [`Emulator`] harness the
//! `web_function_lifecycle` suite uses — only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected, so a
//! `functionCount` that is not computed from persisted state cannot pass.
//!
//! The rule under test is that `accountUsage.functionCount` is the number of web
//! functions stored for the account, in every state, and that
//! `accountQuotas.maxRevisionsPerFunction` is the bound the revision create path
//! refuses at. The tests are tables: the rows differ along the axes the rule
//! generalises over (how many functions are stored, the form of their names,
//! whether they carry a revision, which ones are deleted, how long after the
//! create the settings are read).

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::state::PROVISIONING_DELAY;

use super::web_function_lifecycle::{build_config, role_arn, Emulator, Reply};

/// The region every test signs with.
const REGION: &str = "us-east-1";

/// The largest per-endpoint request rate the model documents as supported:
/// "The value must be one of the following supported values: 0, 100, ... or
/// 10000. The maximum effective value is also bounded by your account-level
/// maximum total rate limit." (source: `smithy_query(query="shape",
/// shape="ThrottleConfig")`), so the account-level total can be no smaller.
const LARGEST_SUPPORTED_ENDPOINT_RATE_LIMIT: i64 = 10_000;

/// `GET /web-account-settings` — the operation's modelled URI
/// (`smithy_query(query="operation", operation="GetWebAccountSettings")`:
/// `GET /2025-03-07/web-account-settings`).
async fn settings(emulator: &Emulator, case: &str) -> Reply {
    let reply = emulator
        .send("GET", "/web-account-settings", REGION, None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: GetWebAccountSettings expected 200, got {} with {:?}",
        reply.status, reply.body
    );
    reply
}

/// `accountUsage.functionCount` as the wire answer reports it.
fn function_count(reply: &Reply, case: &str) -> i64 {
    reply
        .field("accountUsage")
        .and_then(|usage| usage.get("functionCount"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: accountUsage.functionCount is required, got {:?}",
                reply.body
            )
        })
}

/// One member of `accountQuotas`, which the model makes `@required`.
fn quota(reply: &Reply, member: &str, case: &str) -> i64 {
    reply
        .field("accountQuotas")
        .and_then(|quotas| quotas.get(member))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: accountQuotas.{member} is required, got {:?}",
                reply.body
            )
        })
}

/// Create a web function named `name`, optionally with an initial revision.
async fn create_function(emulator: &Emulator, name: &str, with_revision: bool) {
    let mut body = json!({ "functionName": name });
    if with_revision {
        body["revisionConfig"] = json!({
            "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
            "serviceConfig": { "executionRoleArn": role_arn(name) },
        });
    }
    let reply = emulator.send("PUT", "/web-functions", REGION, Some(body)).await;
    assert_eq!(
        reply.status, 202,
        "case {name}: CreateWebFunction expected 202, got {} with {:?}",
        reply.status, reply.body
    );
}

/// Delete the web function named `name`.
async fn delete_function(emulator: &Emulator, name: &str) {
    let reply = emulator
        .send("DELETE", &format!("/web-functions/{name}"), REGION, None)
        .await;
    assert_eq!(
        reply.status, 204,
        "case {name}: DeleteWebFunction expected 204, got {} with {:?}",
        reply.status, reply.body
    );
}

/// How many functions `ListWebFunctions` reports with no filter, walking pages.
async fn listed_functions(emulator: &Emulator, case: &str) -> usize {
    let mut total = 0usize;
    let mut next_token: Option<String> = None;
    for _ in 0..64 {
        let mut body = json!({ "maxResults": 50 });
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let reply = emulator.send("POST", "/web-functions", REGION, Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: ListWebFunctions expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        total += reply
            .field("functions")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        next_token = reply.string("nextToken");
        if next_token.is_none() {
            return total;
        }
    }
    panic!("case {case}: ListWebFunctions never stopped handing out nextToken");
}

#[tokio::test]
async fn function_count_tracks_every_create_and_every_delete() {
    let emulator = Emulator::new();

    // The rule generalises over how many functions are stored and over their
    // shape, so the rows differ along the name's form and length and along
    // whether the create also asked for a revision. The recorded unit's
    // create-then-delete pair is two of these rows.
    let cases: Vec<(&str, bool)> = vec![
        ("demo-fn", true),
        ("a", false),
        ("UPPER_and_lower-123", true),
        ("9", false),
        // The longest name the service documents: exactly 64 characters.
        (
            "f0123456789012345678901234567890123456789012345678901234567890ab",
            true,
        ),
    ];

    let empty = settings(&emulator, "no functions stored").await;
    assert_eq!(
        function_count(&empty, "no functions stored"),
        0,
        "case no functions stored: expected functionCount 0, got {:?}",
        empty.body
    );

    for (index, (name, with_revision)) in cases.iter().enumerate() {
        create_function(&emulator, name, *with_revision).await;
        let case = format!("after creating {name}");
        let reply = settings(&emulator, &case).await;
        let expected = i64::try_from(index + 1).expect("the row count fits an i64");
        assert_eq!(
            function_count(&reply, &case),
            expected,
            "case {case}: expected functionCount {expected}, got {:?}",
            reply.body
        );
    }

    // Deleting in an order unrelated to the creates: the count follows the store,
    // not the sequence of calls.
    let delete_order = [cases[2].0, cases[0].0, cases[4].0, cases[1].0, cases[3].0];
    for (index, name) in delete_order.iter().enumerate() {
        delete_function(&emulator, name).await;
        let case = format!("after deleting {name}");
        let reply = settings(&emulator, &case).await;
        let expected =
            i64::try_from(delete_order.len() - index - 1).expect("the row count fits an i64");
        assert_eq!(
            function_count(&reply, &case),
            expected,
            "case {case}: expected functionCount {expected}, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn function_count_counts_functions_in_every_state() {
    let emulator = Emulator::new();

    // "This count includes web functions in all states, not only functions in the
    // Active state" (model: shape AccountUsage, member functionCount), so a
    // function read back before its clock-driven Pending -> Active transition is
    // due counts exactly as much as one read after it.
    create_function(&emulator, "still-pending", true).await;
    let pending = settings(&emulator, "a Pending function").await;
    assert_eq!(
        function_count(&pending, "a Pending function"),
        1,
        "case a Pending function: expected functionCount 1, got {:?}",
        pending.body
    );

    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
    // The clock has made the Pending -> Active transition due by now, which
    // `GetWebFunction` reports...
    let read = emulator
        .send("GET", "/web-functions/still-pending", REGION, None)
        .await;
    assert_eq!(
        read.string("state").as_deref(),
        Some("Active"),
        "case an Active function: the function must have reached Active, got {:?}",
        read.body
    );
    // ...and the count is unmoved by it.
    let active = settings(&emulator, "an Active function").await;
    assert_eq!(
        function_count(&active, "an Active function"),
        1,
        "case an Active function: expected functionCount 1, got {:?}",
        active.body
    );

    // And the count is exactly the set the unfiltered listing reports.
    let listed = listed_functions(&emulator, "one function stored").await;
    assert_eq!(
        function_count(&active, "one function stored"),
        i64::try_from(listed).expect("the listing size fits an i64"),
        "case one function stored: functionCount must equal the {listed} function(s) \
         ListWebFunctions reports, got {:?}",
        active.body
    );
}

#[tokio::test]
async fn reading_the_settings_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    create_function(&emulator, "read-twice", false).await;

    let first = settings(&emulator, "first read").await;
    let second = settings(&emulator, "second read").await;
    assert_eq!(
        first.body, second.body,
        "case repeated read: GetWebAccountSettings must answer the same settings twice, \
         got {:?} then {:?}",
        first.body, second.body
    );

    // A read of the settings neither creates nor consumes a function.
    let listed = listed_functions(&emulator, "after two reads").await;
    assert_eq!(
        listed, 1,
        "case after two reads: expected the one stored function to survive, got {listed}"
    );
}

#[tokio::test]
async fn the_reported_quotas_are_positive_and_do_not_move_with_usage() {
    let emulator = Emulator::new();

    // Every member of `AccountQuotas` is `@required` with `@range(min: 0)`
    // (source: `smithy_query(query="shape", shape="AccountQuotas")`), and a quota
    // of zero would forbid every web function — so each member is reported, and
    // each one allows something.
    let before = settings(&emulator, "no functions stored").await;
    for member in [
        "maxTotalArmVCpus",
        "maxTotalRateLimit",
        "maxRevisionsPerFunction",
        "maxEndpointsPerFunction",
    ] {
        let actual = quota(&before, member, "no functions stored");
        assert!(
            actual > 0,
            "case no functions stored: accountQuotas.{member} must be positive, got {actual}"
        );
    }

    // The account-level total rate limit must leave every per-endpoint rate the
    // model documents reachable.
    let rate_limit = quota(&before, "maxTotalRateLimit", "no functions stored");
    assert!(
        rate_limit >= LARGEST_SUPPORTED_ENDPOINT_RATE_LIMIT,
        "case no functions stored: accountQuotas.maxTotalRateLimit must be at least the \
         largest supported per-endpoint rateLimit ({LARGEST_SUPPORTED_ENDPOINT_RATE_LIMIT}), \
         got {rate_limit}"
    );

    // Usage moves the usage, never the quotas.
    create_function(&emulator, "quota-probe", true).await;
    let after = settings(&emulator, "one function stored").await;
    assert_eq!(
        before.field("accountQuotas"),
        after.field("accountQuotas"),
        "case one function stored: the quotas must not move with usage, got {:?} then {:?}",
        before.body,
        after.body
    );
}

#[tokio::test]
async fn the_reported_revision_quota_is_the_bound_the_create_path_refuses_at() {
    let emulator = Emulator::new();
    create_function(&emulator, "revision-quota", false).await;

    let reply = settings(&emulator, "revision quota").await;
    let reported = quota(&reply, "maxRevisionsPerFunction", "revision quota");
    let reported = usize::try_from(reported).expect("the reported quota is not negative");

    // Exactly as many revisions as the quota reports are accepted...
    for index in 0..reported {
        let created = emulator
            .send(
                "POST",
                "/web-functions/revision-quota/revisions",
                REGION,
                Some(json!({
                    "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
                    "serviceConfig": { "executionRoleArn": role_arn("revision-quota") },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case revision {} of {reported}: expected 202, got {} with {:?}",
            index + 1,
            created.status,
            created.body
        );
    }

    // ...and the one past it is refused as a quota, not stored.
    let refused = emulator
        .send(
            "POST",
            "/web-functions/revision-quota/revisions",
            REGION,
            Some(json!({
                "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
                "serviceConfig": { "executionRoleArn": role_arn("revision-quota") },
            })),
        )
        .await;
    assert_eq!(
        refused.error(),
        "ServiceQuotaExceededException",
        "case one revision past the reported quota: expected the quota refusal, got {} with {:?}",
        refused.status,
        refused.body
    );

    // The refused create left the function — and so the account's usage — alone.
    let after = settings(&emulator, "after the quota refusal").await;
    assert_eq!(
        function_count(&after, "after the quota refusal"),
        1,
        "case after the quota refusal: expected functionCount 1, got {:?}",
        after.body
    );
}
