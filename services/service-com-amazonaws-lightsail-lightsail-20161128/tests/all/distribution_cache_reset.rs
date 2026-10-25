// SPDX-License-Identifier: CC0-1.0

//! Resetting a Lightsail CDN distribution's cached content, and reading the last
//! reset back, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: `ResetDistributionCache` records ONE reset on the
//! distribution it names — the instant of the request, and a status that starts
//! out `InProgress` and settles into `Completed` on the clock — replacing the
//! record of any reset before it, and `GetDistributionLatestCacheReset` reports
//! exactly that record and writes nothing: the named distribution's last reset,
//! the most recent reset of ALL the account's distributions when the request
//! names none, and neither a timestamp nor a status for a distribution whose
//! cache has never been reset. Both operations are served by the N. Virginia
//! (`us-east-1`) endpoint ALONE, as the modelled `InvalidInputException`
//! documentation states for the domain and distribution APIs together, so a
//! request signed for any other Region is refused and records nothing.
//!
//! So the cases are TABLES: one row per published bundle plan and kind of origin
//! resource the distribution was created with, crossed with how many resets are
//! made on it; the account-wide read is driven over a set of distributions reset
//! in an order that is neither their creation order nor the store's own order of
//! their names; and the refusals are a table over both operations crossed with
//! every Region the modelled `RegionName` enum lists that is not the
//! distribution Region, plus one it does not list.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

// The harness, the wire error code, the minted-id check, the instance stand-up
// and the distribution readers this test target already exposes: a sibling suite
// drives the service through the same front door rather than standing up a
// second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The two operations under test, by the name their `X-Amz-Target` carries.
const RESET: &str = "ResetDistributionCache";
/// See [`RESET`].
const GET: &str = "GetDistributionLatestCacheReset";

/// The one Region a distribution lives in, from the modelled
/// `LightsailDistribution$location` documentation ("all distributions are
/// located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The availability zone an origin instance is created in: the first zone of the
/// distribution Region.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The `operationType` the log entry of a reset carries, from the modelled
/// `OperationType` enum (`smithy_query(query="shape",shape="OperationType")`,
/// which lists `ResetDistributionCache`).
const OPERATION_TYPE: &str = "ResetDistributionCache";

/// The `ResourceType` a distribution reports, from the modelled enum.
const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The `status` of a cache reset while the invalidation is still being applied,
/// and the one it settles into — CloudFront's own `Invalidation$Status` values,
/// which a Lightsail distribution's cache reset carries
/// (`crate::state::CACHE_RESET_STATUS_IN_PROGRESS`).
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const COMPLETED: &str = "Completed";

/// The `status` a distribution reports once its configuration has rolled out
/// (`crate::state::DISTRIBUTION_STATUS_DEPLOYED`).
const DEPLOYED: &str = "Deployed";

/// The `status` a non-terminal operation-log entry carries
/// (`crate::state::OPERATION_STATUS_STARTED`).
const STARTED: &str = "Started";

/// The wire error code (awsJson `__type`) of a refusal of a request the model
/// admits but the service does not serve.
const INVALID_INPUT: &str = "InvalidInputException";

/// The wire error code of Lightsail's rejection of a name that addresses
/// nothing, and the `code` its body carries — which a Lightsail SDK client
/// branches on rather than on the HTTP status.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The refusal the modelled `InvalidInputException` documentation states
/// verbatim, so its text is a requirement and not an example:
/// `smithy_query(query="doc",shape="InvalidInputException")` reads "Domain and
/// distribution APIs are only available in the N. Virginia (us-east-1) Amazon
/// Web Services Region. Please set your Amazon Web Services Region configuration
/// to us-east-1 to create, view, or edit these resources."
const REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                       (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                       Services Region configuration to us-east-1 to create, view, or edit \
                       these resources.";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list,
/// so a case can sit outside the catalog: a scope naming a Region Lightsail does
/// not serve at all is no more the distribution Region than any other.
const OFF_ENUM_REGION: &str = "il-central-1";

/// Every Region one of these calls can be made in that is NOT the distribution
/// Region.
///
/// Taken from the crate's own `REGION_NAME_VALUES` — the modelled `RegionName`
/// enum's values (`smithy_query(query="shape",shape="RegionName")`) — minus the
/// distribution Region, so the table cannot drift from the model as the catalog
/// grows, plus [`OFF_ENUM_REGION`].
fn other_regions() -> Vec<&'static str> {
    assert!(
        REGION_NAME_VALUES.contains(&DISTRIBUTION_REGION),
        "case the Region catalog: expected the modelled RegionName enum to list the \
         distribution Region, got {REGION_NAME_VALUES:?}"
    );
    assert!(
        !REGION_NAME_VALUES.contains(&OFF_ENUM_REGION),
        "case the Region catalog: expected {OFF_ENUM_REGION} to sit OUTSIDE the modelled \
         RegionName enum, got {REGION_NAME_VALUES:?}"
    );
    let mut regions: Vec<&'static str> = REGION_NAME_VALUES
        .iter()
        .copied()
        .filter(|region| *region != DISTRIBUTION_REGION)
        .collect();
    regions.push(OFF_ENUM_REGION);
    regions
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// What kind of Lightsail resource a distribution's origin is, from the modelled
/// `Origin` documentation: "An origin can be a Lightsail instance, bucket,
/// container service, or load balancer."
#[derive(Clone, Copy, Debug)]
enum OriginKind {
    Instance,
    Bucket,
}

/// Stand up the origin resource `name` of this kind in the distribution Region.
async fn create_origin(emulator: &Emulator, kind: OriginKind, name: &str) {
    match kind {
        OriginKind::Instance => create_running_instance(emulator, name, ORIGIN_ZONE).await,
        OriginKind::Bucket => {
            emulator
                .ok(
                    "CreateBucket",
                    json!({"bucketName": name, "bundleId": "small_1_0"}),
                )
                .await;
        }
    }
}

/// Create the distribution `name` on `bundle_id`, pulling from the origin
/// resource `origin_name`, and wait until it reports [`DEPLOYED`].
///
/// The wait is what makes a later assertion about the distribution's own status
/// meaningful: a reset of its cached content must leave the deployed
/// configuration exactly as it was.
async fn create_deployed_distribution(
    emulator: &Emulator,
    name: &str,
    bundle_id: &str,
    origin_name: &str,
) {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "bundleId": bundle_id,
                "origin": {
                    "name": origin_name,
                    "regionName": DISTRIBUTION_REGION,
                    "protocolPolicy": "http-only",
                },
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;
    wait_for_distribution_status(emulator, name, DEPLOYED).await;
}

/// Reset the cached content of the distribution `name`, answering the whole
/// response.
async fn reset_cache(emulator: &Emulator, name: &str) -> Value {
    emulator.ok(RESET, json!({"distributionName": name})).await
}

/// The last cache reset `GetDistributionLatestCacheReset` reports: of the
/// distribution `name` names, or of every distribution of the account when it
/// names none.
async fn latest_reset(emulator: &Emulator, name: Option<&str>) -> Value {
    let body = match name {
        Some(name) => json!({"distributionName": name}),
        None => json!({}),
    };
    emulator.ok(GET, body).await
}

/// The `createTime` a response reports, as epoch seconds.
fn create_time(case: &str, body: &Value) -> f64 {
    body["createTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a createTime, got {body:?}"))
}

/// Poll until the last cache reset of `name` reports [`COMPLETED`], answering
/// the whole response.
///
/// The invalidation is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_completed_reset(emulator: &Emulator, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = latest_reset(emulator, Some(name)).await;
        if reported["status"].as_str() == Some(COMPLETED) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected the last cache reset of {name} to be {COMPLETED} within 5s, \
             got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// How many reset entries the operation log holds for the distribution `name`.
async fn reset_log_entries(emulator: &Emulator, case: &str, name: &str) -> usize {
    let body = emulator
        .ok("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    assert!(
        body["nextPageToken"].is_null(),
        "case {case}: expected every log entry of {name} on one page, got {body:?}"
    );
    body["operations"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry["operationType"].as_str() == Some(OPERATION_TYPE))
                .count()
        })
        .unwrap_or_default()
}

/// Assert `answered` is the answer of a reset just made on `name`: the log entry
/// it carries is about that distribution and is not finished yet.
fn assert_reset_answer(case: &str, name: &str, answered: &Value) {
    assert_eq!(
        answered["status"].as_str(),
        Some(IN_PROGRESS),
        "case {case}: expected a reset's own answer to be {IN_PROGRESS}, got {answered:?}"
    );
    let operation = &answered["operation"];
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(RESOURCE_TYPE_DISTRIBUTION),
        "case {case}: expected the entry to be about the distribution, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the entry to name {name}, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some(STARTED),
        "case {case}: expected a {STARTED} operation, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(false),
        "case {case}: expected a non-terminal operation, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(DISTRIBUTION_REGION),
        "case {case}: expected the entry located where the distribution is, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
}

/// Assert the last cache reset of `name` is reported as absent — neither a
/// timestamp nor a status.
fn assert_no_reset(case: &str, reported: &Value) {
    assert_eq!(
        reported["createTime"],
        Value::Null,
        "case {case}: expected no timestamp before a cache reset, got {reported:?}"
    );
    assert_eq!(
        reported["status"],
        Value::Null,
        "case {case}: expected no status before a cache reset, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The reset table
// ---------------------------------------------------------------------------

/// One distribution and the resets made on it.
///
/// The rows differ along everything the rule generalises over: the published
/// bundle plan the distribution runs on, the kind of resource it pulls content
/// from, and how many times its cache is reset — the record holds the LATEST
/// reset, so a row with several proves each one replaces its predecessor.
struct Row {
    case: &'static str,
    distribution_name: &'static str,
    /// An entry of this service's published CDN plans
    /// (`crate::state::DISTRIBUTION_BUNDLE_SPECS`).
    bundle_id: &'static str,
    origin_kind: OriginKind,
    origin_name: &'static str,
    resets: usize,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one reset, the cheapest plan, an instance origin",
        distribution_name: "single-reset-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "reset-origin-instance",
        resets: 1,
    },
    Row {
        case: "two resets, the middle plan, a bucket origin",
        distribution_name: "two-reset-cdn",
        bundle_id: "medium_1_0",
        origin_kind: OriginKind::Bucket,
        origin_name: "reset-origin-bucket",
        resets: 2,
    },
    Row {
        case: "three resets, the largest plan, an instance origin",
        distribution_name: "three_reset_cdn",
        bundle_id: "large_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "reset-origin-second-instance",
        resets: 3,
    },
];

/// RULE: a reset records the instant it was requested and a status that settles
/// from `InProgress` into `Completed` on the clock, every read of that
/// distribution reports that one record, and a later reset replaces it.
#[tokio::test]
async fn a_reset_records_the_latest_cache_reset_every_read_reports() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        create_origin(&emulator, row.origin_kind, row.origin_name).await;
        create_deployed_distribution(
            &emulator,
            row.distribution_name,
            row.bundle_id,
            row.origin_name,
        )
        .await;

        // Before any reset there is nothing to report: a distribution whose
        // cache has never been reset has neither a timestamp nor a status of a
        // reset, and both result members are optional.
        let fresh = latest_reset(&emulator, Some(row.distribution_name)).await;
        assert_no_reset(case, &fresh);
        assert_eq!(
            reset_log_entries(&emulator, case, row.distribution_name).await,
            0,
            "case {case}: expected no reset in the log before one was made"
        );

        let mut previous: Option<f64> = None;
        for attempt in 1..=row.resets {
            let case = &format!("{case}, reset {attempt}");
            let answered = reset_cache(&emulator, row.distribution_name).await;
            // The call's OWN answer, so no clock has run yet: the invalidation
            // is still being applied.
            assert_reset_answer(case, row.distribution_name, &answered);
            let created = create_time(case, &answered);
            if let Some(previous) = previous {
                assert!(
                    created > previous,
                    "case {case}: expected a later timestamp than the reset before it \
                     ({previous}), got {created}"
                );
            }

            // The read reports the reset this call recorded. Its status is
            // whatever the clock has made of it by the time the read lands — the
            // in-progress value is asserted on the call's own answer above —
            // but the timestamp is the one the call reported.
            let read = latest_reset(&emulator, Some(row.distribution_name)).await;
            assert_eq!(
                read["createTime"].as_f64(),
                Some(created),
                "case {case}: expected the read to report the reset just made, got {read:?}"
            );
            let status = read["status"].as_str().unwrap_or_default();
            assert!(
                [IN_PROGRESS, COMPLETED].contains(&status),
                "case {case}: expected the status to be {IN_PROGRESS} or {COMPLETED}, got \
                 {read:?}"
            );

            // And the invalidation the clock owes arrives without anyone asking
            // more than once, on the record the reset wrote rather than on a new
            // one.
            let settled = wait_for_completed_reset(&emulator, case, row.distribution_name).await;
            assert_eq!(
                settled["createTime"].as_f64(),
                Some(created),
                "case {case}: expected the settled reset to keep its timestamp, got {settled:?}"
            );
            // A read changes nothing a later caller sees.
            let again = latest_reset(&emulator, Some(row.distribution_name)).await;
            assert_eq!(
                again, settled,
                "case {case}: expected a second read to report the same reset, got {again:?}"
            );
            previous = Some(created);
        }

        // Deleting cached content is not a configuration change: the
        // distribution the resets were made on is still the deployed one.
        let reported = distribution(&emulator, row.distribution_name).await;
        assert_eq!(
            reported["status"].as_str(),
            Some(DEPLOYED),
            "case {case}: expected a reset to leave the distribution {DEPLOYED}, got {reported:?}"
        );
        // One log entry per reset, each for the distribution it was made on.
        assert_eq!(
            reset_log_entries(&emulator, case, row.distribution_name).await,
            row.resets,
            "case {case}: expected one log entry per reset"
        );
    }

    // Every distribution of the table keeps its OWN last reset: the records do
    // not run together.
    let mut timestamps = Vec::new();
    for row in ROWS {
        let reported = latest_reset(&emulator, Some(row.distribution_name)).await;
        assert_eq!(
            reported["status"].as_str(),
            Some(COMPLETED),
            "case {}: expected its settled reset at the end, got {reported:?}",
            row.case
        );
        timestamps.push(create_time(row.case, &reported));
    }
    for (earlier, later) in timestamps.iter().zip(timestamps.iter().skip(1)) {
        assert!(
            later > earlier,
            "case the table as a whole: expected each distribution's last reset to be later \
             than the one before it ({earlier}), got {later}"
        );
    }
}

// ---------------------------------------------------------------------------
// The account-wide read
// ---------------------------------------------------------------------------

/// The distributions the account-wide read is driven over, in CREATION order.
const ACCOUNT_NAMES: &[&str] = &["zulu-cdn", "alpha-cdn", "mike-cdn"];

/// The order their caches are reset in: neither the creation order above nor the
/// store's own (name) order, so an answer can only come from WHEN each reset was
/// made.
const RESET_ORDER: &[&str] = &["mike-cdn", "zulu-cdn", "alpha-cdn"];

/// RULE: a request that names no distribution reports the most recent cache
/// reset of every distribution the account holds, while each distribution's own
/// read keeps reporting its own last reset.
#[tokio::test]
async fn an_omitted_name_reports_the_latest_reset_of_every_distribution() {
    let emulator = Emulator::new();
    // One origin serves every distribution here: which resource a distribution
    // pulls from is not what this rule is about.
    create_origin(&emulator, OriginKind::Instance, "account-wide-origin").await;
    for name in ACCOUNT_NAMES {
        create_deployed_distribution(&emulator, name, "small_1_0", "account-wide-origin").await;
    }

    // No distribution has been reset, so the account has no latest reset.
    assert_no_reset(
        "the account before any reset",
        &latest_reset(&emulator, None).await,
    );

    let mut done: Vec<(&str, f64)> = Vec::new();
    for name in RESET_ORDER {
        let case = &format!("the account after resetting {name}");
        let answered = reset_cache(&emulator, name).await;
        assert_reset_answer(case, name, &answered);
        let created = create_time(case, &answered);

        // The account-wide read reports the reset just made: it is the latest.
        let account = latest_reset(&emulator, None).await;
        assert_eq!(
            account["createTime"].as_f64(),
            Some(created),
            "case {case}: expected the latest reset of all the account's distributions, got \
             {account:?}"
        );

        // Every distribution reset before this one keeps its own, older record,
        // and one never reset still reports nothing.
        for (earlier, earlier_created) in &done {
            let own = latest_reset(&emulator, Some(earlier)).await;
            assert_eq!(
                own["createTime"].as_f64(),
                Some(*earlier_created),
                "case {case}: expected {earlier} to keep its own earlier reset, got {own:?}"
            );
        }
        for never in ACCOUNT_NAMES
            .iter()
            .filter(|candidate| *candidate != name && !done.iter().any(|(c, _)| c == *candidate))
        {
            assert_no_reset(
                &format!("{case}, reading {never}"),
                &latest_reset(&emulator, Some(never)).await,
            );
        }

        // Settled before the next row, so the resets fall in a known order
        // rather than inside one millisecond of each other.
        let settled = wait_for_completed_reset(&emulator, case, name).await;
        assert_eq!(
            settled["createTime"].as_f64(),
            Some(created),
            "case {case}: expected the settled reset to keep its timestamp, got {settled:?}"
        );
        done.push((name, created));
    }

    // The last reset made is the one the account-wide read answers, with the
    // status that reset has now — not the record of the distribution that was
    // created first or sorts first.
    let (last, last_created) = done
        .last()
        .copied()
        .expect("the table reset at least one distribution");
    let account = latest_reset(&emulator, None).await;
    assert_eq!(
        account["createTime"].as_f64(),
        Some(last_created),
        "case the account at the end: expected the reset of {last}, got {account:?}"
    );
    assert_eq!(
        account["status"].as_str(),
        Some(COMPLETED),
        "case the account at the end: expected the settled status of {last}'s reset, got \
         {account:?}"
    );
}

/// RULE: a distribution holds its LATEST reset, so a reset made while an earlier
/// one is still being applied supersedes it.
#[tokio::test]
async fn a_reset_supersedes_one_still_in_progress() {
    let case = "a reset superseding one in progress";
    let emulator = Emulator::new();
    create_origin(&emulator, OriginKind::Instance, "supersede-origin").await;
    create_deployed_distribution(&emulator, "supersede-cdn", "small_1_0", "supersede-origin")
        .await;

    let first = reset_cache(&emulator, "supersede-cdn").await;
    assert_reset_answer(case, "supersede-cdn", &first);
    let first_created = create_time(case, &first);
    // Far enough apart for the two requests to fall in different milliseconds,
    // the resolution the store's timestamps carry, and well inside the span the
    // first invalidation is owed.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let second = reset_cache(&emulator, "supersede-cdn").await;
    assert_reset_answer(case, "supersede-cdn", &second);
    let second_created = create_time(case, &second);
    assert!(
        second_created > first_created,
        "case {case}: expected the second reset to be later than the first ({first_created}), \
         got {second_created}"
    );

    // The read reports the second reset, not the one it superseded.
    let read = latest_reset(&emulator, Some("supersede-cdn")).await;
    assert_eq!(
        read["createTime"].as_f64(),
        Some(second_created),
        "case {case}: expected the latest reset, got {read:?}"
    );
    // And the record the clock settles is that same one.
    let settled = wait_for_completed_reset(&emulator, case, "supersede-cdn").await;
    assert_eq!(
        settled["createTime"].as_f64(),
        Some(second_created),
        "case {case}: expected the settled reset to be the latest one, got {settled:?}"
    );
    // Both calls were served, so both are in the log.
    assert_eq!(
        reset_log_entries(&emulator, case, "supersede-cdn").await,
        2,
        "case {case}: expected one log entry per reset"
    );
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// RULE: a request outside what the service serves — a distribution the account
/// does not hold, a reset that names no distribution, or either call made
/// anywhere but the distribution Region — is refused and records nothing.
#[tokio::test]
async fn the_cache_reset_apis_refuse_what_the_service_does_not_serve() {
    let emulator = Emulator::new();
    create_origin(&emulator, OriginKind::Instance, "refusal-origin").await;
    create_deployed_distribution(&emulator, "refusal-cdn", "small_1_0", "refusal-origin").await;

    // A name this account holds no distribution under, for both operations:
    // Lightsail's `DoesNotExist` envelope.
    for operation in [RESET, GET] {
        let case = &format!("{operation} naming a distribution the account does not hold");
        let (status, body) = emulator
            .call(operation, json!({"distributionName": "no-such-cdn"}))
            .await;
        assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body:?}");
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case}: expected {NOT_FOUND}, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the {DOES_NOT_EXIST} code, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("The Distribution does not exist: no-such-cdn"),
            "case {case}: expected the message to name the resource type and the name, got \
             {body:?}"
        );
    }

    // A reset that names no distribution: the call deletes the cached content of
    // ONE distribution and documents no behaviour for an omitted name, unlike
    // the read, so there is no cache it could reset.
    let case = "a reset naming no distribution";
    let (status, body) = emulator.call(RESET, json!({})).await;
    assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body:?}");
    assert_eq!(
        error_code(&body),
        INVALID_INPUT,
        "case {case}: expected {INVALID_INPUT}, got {body:?}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("distributionName")),
        "case {case}: expected the message to name the member, got {body:?}"
    );

    // Both operations are served by the N. Virginia endpoint ALONE.
    for region in other_regions() {
        for operation in [RESET, GET] {
            let case = &format!("{operation} in {region}");
            let (status, body) = emulator
                .call_in(
                    Some(region),
                    operation,
                    json!({"distributionName": "refusal-cdn"}),
                )
                .await;
            assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body:?}");
            assert_eq!(
                error_code(&body),
                INVALID_INPUT,
                "case {case}: expected {INVALID_INPUT}, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(REFUSAL),
                "case {case}: expected the documented refusal, got {body:?}"
            );
        }
    }

    // Nothing any of those refusals touched was recorded: the distribution's
    // cache has still never been reset, and its log holds no reset.
    assert_no_reset(
        "the refused requests",
        &latest_reset(&emulator, Some("refusal-cdn")).await,
    );
    assert_no_reset(
        "the refused requests, read account-wide",
        &latest_reset(&emulator, None).await,
    );
    assert_eq!(
        reset_log_entries(&emulator, "the refused requests", "refusal-cdn").await,
        0,
        "case the refused requests: expected no reset in the log"
    );

    // And the same two calls in the distribution Region are served.
    let case = "the distribution Region named explicitly";
    let answered = emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            RESET,
            json!({"distributionName": "refusal-cdn"}),
        )
        .await;
    assert_reset_answer(case, "refusal-cdn", &answered);
    let read = emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            GET,
            json!({"distributionName": "refusal-cdn"}),
        )
        .await;
    assert_eq!(
        read["createTime"].as_f64(),
        Some(create_time(case, &answered)),
        "case {case}: expected the read to report the reset just made, got {read:?}"
    );
}
