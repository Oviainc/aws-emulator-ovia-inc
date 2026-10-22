// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DevOps Guru cost-estimation lifecycle:
//! `StartCostEstimation` creating the account's estimate and `GetCostEstimation`
//! reporting it.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Emulator`] harness in `configuration_lifecycle`, so the assertions are about the
//! wire behaviour rather than handler internals.
//!
//! Each test states the RULE it pins and exercises it over a set of inputs that differ
//! along what the rule generalises over (which half of the collection is named, how
//! many entries it names, whether they are covered, the order of the calls and where
//! the clock stands).

use serde_json::{json, Value};
use std::time::Duration;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    COST_ESTIMATION_DURATION, HOURS_PER_MONTH, MAX_COST_ESTIMATION_TAG_FILTERS,
    RESOURCE_HOUR_UNIT_COST,
};

use crate::configuration_lifecycle::{cloud_formation, tags, Emulator, Reply};

/// Money is compared to well under a cent: a cost is a product of floats, so the
/// reported value may differ from the recomputed one in the last bits.
const MONEY_EPSILON: f64 = 1e-6;

/// `PUT /cost-estimation` — `StartCostEstimation`, with `ClientToken` only when the
/// case supplies one (the member is optional).
async fn start_cost_estimation(
    emulator: &Emulator,
    collection: &Value,
    client_token: Option<&str>,
) -> Reply {
    let mut body = json!({ "ResourceCollection": collection });
    if let Some(client_token) = client_token {
        body["ClientToken"] = json!(client_token);
    }
    emulator.call("PUT", "/cost-estimation", Some(&body)).await
}

/// `GET /cost-estimation` — `GetCostEstimation`, with `NextToken` in the query string
/// the model binds it to.
async fn get_cost_estimation(emulator: &Emulator, next_token: Option<&str>) -> Reply {
    let path = match next_token {
        Some(token) => format!("/cost-estimation?NextToken={token}"),
        None => "/cost-estimation".to_string(),
    };
    emulator.call("GET", &path, None).await
}

/// The monthly cost of analysing `count` active resources, recomputed in the test
/// from DevOps Guru's published per-resource-hour price and the billing month the
/// service crate documents. [`published_pricing_is_what_the_rows_assume`] pins those
/// two constants to the published numbers, so a wrong constant fails rather than
/// cancelling out here.
fn monthly_cost(count: i64) -> f64 {
    count as f64 * RESOURCE_HOUR_UNIT_COST * HOURS_PER_MONTH
}

/// The published numbers the cost rows are computed from: "$0.0028 per resource hour"
/// (<https://aws.amazon.com/devops-guru/pricing/>) over AWS's 730-hour billing month,
/// and the hand-computed monthly cost of one active resource they give.
#[test]
fn published_pricing_is_what_the_rows_assume() {
    assert_eq!(
        RESOURCE_HOUR_UNIT_COST, 0.0028,
        "case {:?}: expected DevOps Guru's published per-resource-hour price, got {RESOURCE_HOUR_UNIT_COST}",
        "published unit price"
    );
    assert_eq!(
        HOURS_PER_MONTH, 730.0,
        "case {:?}: expected AWS's 730-hour billing month, got {HOURS_PER_MONTH}",
        "billing month"
    );
    // 0.0028 USD/resource-hour x 730 hours = 2.044 USD for one active resource.
    assert!(
        (monthly_cost(1) - 2.044).abs() < MONEY_EPSILON,
        "case {:?}: expected 2.044 for one active resource, got {}",
        "one active resource for a month",
        monthly_cost(1)
    );
}

/// The reported `Costs` as `(Type, Count)`, in report order.
fn reported_cost_lines(reply: &Reply) -> Vec<(String, i64)> {
    reply
        .at("/Costs")
        .and_then(Value::as_array)
        .map(|costs| {
            costs
                .iter()
                .map(|cost| {
                    (
                        cost.get("Type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        cost.get("Count").and_then(Value::as_i64).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn reported_status(reply: &Reply) -> Option<&str> {
    reply.at("/Status").and_then(Value::as_str)
}

/// Whether the read found an estimation at all, whichever side of its completion
/// instant the clock is on. Used where the row is about an estimation EXISTING, so a
/// slow machine that crosses the completion instant before the read does not change
/// what the row asserts; the transition itself has its own test.
fn reports_an_estimation(reply: &Reply) -> bool {
    matches!(reported_status(reply), Some("ONGOING") | Some("COMPLETED"))
}

fn reported_total_cost(reply: &Reply) -> f64 {
    reply
        .at("/TotalCost")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN)
}

/// `ADD` coverage for a collection, failing loudly if the setup itself is refused.
async fn add_coverage(emulator: &Emulator, collection: Value) {
    let reply = emulator
        .update_resource_collection("ADD", collection.clone())
        .await;
    assert_eq!(
        reply.status, 200,
        "coverage setup {collection:?}: expected 200, got {} {}",
        reply.status, reply.raw
    );
}

/// What one row of the resolution table expects back.
#[derive(Debug)]
enum Expect {
    /// The start is accepted and an estimation is created.
    Accepted,
    /// The start is refused with this error shape and status.
    Refused(&'static str, u16),
}

struct ResolutionCase {
    name: &'static str,
    /// Coverage to declare through `UpdateResourceCollection` first, if any.
    coverage: Option<Value>,
    collection: Value,
    expect: Expect,
    /// For a refusal that names a resource, the `(ResourceId, ResourceType)` the error
    /// must carry.
    missing: Option<(&'static str, &'static str)>,
    /// The exact `Message` the refusal must carry, where the service answers the
    /// condition with a fixed text rather than one built from the request.
    expect_message: Option<&'static str>,
}

/// RULE: a cost estimate is of the resources DevOps Guru analyses, so
/// `StartCostEstimation` resolves every resource the submitted collection names — a
/// CloudFormation stack, an app-boundary tag value — against the account's analysis
/// coverage, and refuses one the coverage does not hold with
/// `ResourceNotFoundException` (404) naming that resource; a collection that names no
/// resource at all, or more tags than the one a cost estimate may carry, is refused
/// with `ValidationException` (400). A refusal creates no estimation.
///
/// The rows differ along which half of the collection is named, whether its entries
/// are covered, how many entries it names (none, one, two) and letter case.
#[tokio::test]
async fn a_start_resolves_its_collection_against_the_analysis_coverage() {
    let cases = [
        ResolutionCase {
            name: "a stack with no coverage declared at all",
            coverage: None,
            collection: cloud_formation(&["my-stack"]),
            expect: Expect::Refused("ResourceNotFoundException", 404),
            missing: Some(("my-stack", "AWS::CloudFormation::Stack")),
            expect_message: None,
        },
        ResolutionCase {
            name: "the covered stack",
            coverage: Some(cloud_formation(&["my-stack"])),
            collection: cloud_formation(&["my-stack"]),
            expect: Expect::Accepted,
            missing: None,
            expect_message: None,
        },
        ResolutionCase {
            name: "a stack other than the covered one",
            coverage: Some(cloud_formation(&["covered-stack"])),
            collection: cloud_formation(&["other-stack"]),
            expect: Expect::Refused("ResourceNotFoundException", 404),
            missing: Some(("other-stack", "AWS::CloudFormation::Stack")),
            expect_message: None,
        },
        ResolutionCase {
            name: "a covered stack name in different letter case",
            coverage: Some(cloud_formation(&["MyStack"])),
            collection: cloud_formation(&["mystack"]),
            expect: Expect::Refused("ResourceNotFoundException", 404),
            missing: Some(("mystack", "AWS::CloudFormation::Stack")),
            expect_message: None,
        },
        ResolutionCase {
            name: "a tag no covered resource carries",
            coverage: None,
            collection: tags(&[("Devops-guru-app", &["production"])]),
            expect: Expect::Refused("ResourceNotFoundException", 404),
            missing: Some(("Devops-guru-app/production", "AWS::Tag")),
            expect_message: Some("Invalid tag in the request"),
        },
        ResolutionCase {
            name: "a covered app boundary key with an uncovered value",
            coverage: Some(tags(&[("Devops-guru-app", &["production"])])),
            collection: tags(&[("Devops-guru-app", &["staging"])]),
            expect: Expect::Refused("ResourceNotFoundException", 404),
            missing: Some(("Devops-guru-app/staging", "AWS::Tag")),
            expect_message: Some("Invalid tag in the request"),
        },
        ResolutionCase {
            name: "a covered tag filter",
            coverage: Some(tags(&[("Devops-guru-app", &["production"])])),
            collection: tags(&[("Devops-guru-app", &["production"])]),
            expect: Expect::Accepted,
            missing: None,
            expect_message: None,
        },
        ResolutionCase {
            name: "a collection naming neither half",
            coverage: Some(cloud_formation(&["my-stack"])),
            collection: json!({}),
            expect: Expect::Refused("ValidationException", 400),
            missing: None,
            expect_message: None,
        },
        ResolutionCase {
            name: "a CloudFormation filter naming no stack",
            coverage: Some(cloud_formation(&["my-stack"])),
            collection: json!({ "CloudFormation": {} }),
            expect: Expect::Refused("ValidationException", 400),
            missing: None,
            expect_message: None,
        },
        ResolutionCase {
            // "The maximum number of tags you can specify for a cost estimate is one"
            // — `TagCostEstimationResourceCollectionFilter`'s model documentation. The
            // refusal is about the request, so it needs no coverage (and an account
            // has exactly one app-boundary key, so two could never be covered).
            name: "one tag filter more than a cost estimate may carry",
            coverage: None,
            collection: tags(&[
                ("Devops-guru-app", &["production"]),
                ("Devops-guru-tier", &["web"]),
            ]),
            expect: Expect::Refused("ValidationException", 400),
            missing: None,
            expect_message: None,
        },
    ];

    assert_eq!(
        MAX_COST_ESTIMATION_TAG_FILTERS, 1,
        "the documented maximum of one tag per cost estimate is what the last row exercises, got {MAX_COST_ESTIMATION_TAG_FILTERS}"
    );

    for case in &cases {
        let emulator = Emulator::new();
        if let Some(coverage) = case.coverage.clone() {
            add_coverage(&emulator, coverage).await;
        }

        let reply = start_cost_estimation(&emulator, &case.collection, None).await;
        let read = get_cost_estimation(&emulator, None).await;

        match case.expect {
            Expect::Accepted => {
                assert_eq!(
                    reply.status, 200,
                    "case {:?}: expected the start to be accepted with 200, got {} {}",
                    case.name, reply.status, reply.raw
                );
                assert!(
                    reports_an_estimation(&read),
                    "case {:?}: expected the created estimation to read back, got {}",
                    case.name,
                    read.raw
                );
            }
            Expect::Refused(error_name, status) => {
                assert_eq!(
                    reply.status, status,
                    "case {:?}: expected HTTP {status}, got {} {}",
                    case.name, reply.status, reply.raw
                );
                assert!(
                    reply.names_error(error_name),
                    "case {:?}: expected {error_name}, got {}",
                    case.name,
                    reply.raw
                );
                if let Some((resource_id, resource_type)) = case.missing {
                    assert_eq!(
                        reply.at("/ResourceId").and_then(Value::as_str),
                        Some(resource_id),
                        "case {:?}: expected the error to name the missing resource, got {}",
                        case.name,
                        reply.raw
                    );
                    assert_eq!(
                        reply.at("/ResourceType").and_then(Value::as_str),
                        Some(resource_type),
                        "case {:?}: expected the error to name the resource's type, got {}",
                        case.name,
                        reply.raw
                    );
                }
                if let Some(expect_message) = case.expect_message {
                    assert_eq!(
                        reply.at("/Message").and_then(Value::as_str),
                        Some(expect_message),
                        "case {:?}: expected the refusal's fixed message, got {}",
                        case.name,
                        reply.raw
                    );
                }
                assert_eq!(
                    reported_status(&read),
                    None,
                    "case {:?}: a refused start must create no estimation, got {}",
                    case.name,
                    read.raw
                );
                assert!(
                    (reported_total_cost(&read) - 0.0).abs() < MONEY_EPSILON,
                    "case {:?}: expected TotalCost 0 with no estimation, got {}",
                    case.name,
                    read.raw
                );
            }
        }
    }
}

struct PricingCase {
    name: &'static str,
    /// Every coverage filter to declare through `UpdateResourceCollection` first, so a
    /// row needs no code outside the table.
    coverage: Vec<Value>,
    collection: Value,
    /// The `(Type, Count)` lines the estimate must report, in order.
    expect_lines: &'static [(&'static str, i64)],
}

/// RULE: the estimate prices the resources the collection names — one line per
/// resource type, each counting the resources of that type, priced at DevOps Guru's
/// published per-resource-hour rate over a billing month — and `TotalCost` is the sum
/// of the lines in the response.
///
/// The rows differ along which half of the collection is named and how many types the
/// estimate therefore prices (one, two).
#[tokio::test]
async fn the_estimate_prices_every_covered_resource_the_collection_resolves_to() {
    let cases = [
        PricingCase {
            name: "a covered stack",
            coverage: vec![cloud_formation(&["my-stack"])],
            collection: cloud_formation(&["my-stack"]),
            expect_lines: &[("AWS::CloudFormation::Stack", 1)],
        },
        PricingCase {
            name: "a covered tag value",
            coverage: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-app", &["production"])]),
            expect_lines: &[("AWS::Tag", 1)],
        },
        PricingCase {
            name: "both halves covered",
            coverage: vec![
                cloud_formation(&["my-stack"]),
                tags(&[("Devops-guru-app", &["production"])]),
            ],
            collection: json!({
                "CloudFormation": { "StackNames": ["my-stack"] },
                "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": ["production"] }],
            }),
            expect_lines: &[("AWS::CloudFormation::Stack", 1), ("AWS::Tag", 1)],
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        for coverage in &case.coverage {
            add_coverage(&emulator, coverage.clone()).await;
        }

        let started = start_cost_estimation(&emulator, &case.collection, None).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected the start to be accepted, got {} {}",
            case.name, started.status, started.raw
        );

        let read = get_cost_estimation(&emulator, None).await;
        let lines = reported_cost_lines(&read);
        let expected: Vec<(String, i64)> = case
            .expect_lines
            .iter()
            .map(|(resource_type, count)| (resource_type.to_string(), *count))
            .collect();
        assert_eq!(
            lines, expected,
            "case {:?}: expected cost lines {expected:?}, got {lines:?} in {}",
            case.name, read.raw
        );

        let costs = read
            .at("/Costs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (line, (resource_type, count)) in costs.iter().zip(case.expect_lines) {
            assert_eq!(
                line.get("State").and_then(Value::as_str),
                Some("ACTIVE"),
                "case {:?}: expected the {resource_type} line to count active resources, got {line}",
                case.name
            );
            let unit_cost = line.get("UnitCost").and_then(Value::as_f64).unwrap_or(f64::NAN);
            assert!(
                (unit_cost - RESOURCE_HOUR_UNIT_COST).abs() < MONEY_EPSILON,
                "case {:?}: expected the published unit cost {RESOURCE_HOUR_UNIT_COST}, got {unit_cost} in {line}",
                case.name
            );
            let cost = line.get("Cost").and_then(Value::as_f64).unwrap_or(f64::NAN);
            assert!(
                (cost - monthly_cost(*count)).abs() < MONEY_EPSILON,
                "case {:?}: expected the {resource_type} line to cost {}, got {cost} in {line}",
                case.name,
                monthly_cost(*count)
            );
        }

        let expected_total: f64 = case
            .expect_lines
            .iter()
            .map(|(_, count)| monthly_cost(*count))
            .sum();
        let total = reported_total_cost(&read);
        assert!(
            (total - expected_total).abs() < MONEY_EPSILON,
            "case {:?}: expected TotalCost {expected_total}, got {total} in {}",
            case.name,
            read.raw
        );
    }
}

/// RULE: the estimate is created asynchronously — it reports `ONGOING` with no
/// `TimeRange.EndTime` until its completion instant and `COMPLETED` with that end time
/// from then on — and reporting it is a read: it reports the collection that was
/// submitted and the costs computed at the start, and reading it again at the same
/// point of the clock answers exactly the same thing.
///
/// The steps differ along where the clock stands relative to the completion instant
/// (straight after the start, and past it) and along how many reads have happened.
#[tokio::test]
async fn an_estimation_reports_ongoing_until_it_is_due_and_completed_after() {
    let emulator = Emulator::new();
    add_coverage(&emulator, cloud_formation(&["lifecycle-stack"])).await;
    add_coverage(&emulator, tags(&[("Devops-guru-app", &["production"])])).await;

    let collection = json!({
        "CloudFormation": { "StackNames": ["lifecycle-stack"] },
        "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": ["production"] }],
    });
    let started = start_cost_estimation(&emulator, &collection, Some("lifecycletoken")).await;
    assert_eq!(
        started.status, 200,
        "starting the estimation: expected 200, got {} {}",
        started.status, started.raw
    );

    // Straight after the start the work is not due yet, so the caller sees the
    // transient status rather than a finished estimate.
    let ongoing = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        reported_status(&ongoing),
        Some("ONGOING"),
        "case {:?}: expected ONGOING straight after the start, got {}",
        "immediately after start",
        ongoing.raw
    );
    assert!(
        ongoing
            .at("/TimeRange/StartTime")
            .is_some_and(|value| value.is_number() || value.is_string()),
        "case {:?}: expected the estimation's start time to be reported, got {}",
        "immediately after start",
        ongoing.raw
    );
    assert_eq!(
        ongoing.at("/TimeRange/EndTime"),
        None,
        "case {:?}: an unfinished estimation has no end time, got {}",
        "immediately after start",
        ongoing.raw
    );
    assert_eq!(
        ongoing.strings_at("/ResourceCollection/CloudFormation/StackNames"),
        vec!["lifecycle-stack".to_string()],
        "case {:?}: expected the submitted stack to round-trip, got {}",
        "immediately after start",
        ongoing.raw
    );
    assert_eq!(
        ongoing
            .at("/ResourceCollection/Tags/0/AppBoundaryKey")
            .and_then(Value::as_str),
        Some("Devops-guru-app"),
        "case {:?}: expected the submitted tag filter to round-trip, got {}",
        "immediately after start",
        ongoing.raw
    );

    // Reading twice at the same point of the clock answers the same thing: a report
    // does not move the estimation on.
    let again = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        reported_status(&again),
        Some("ONGOING"),
        "case {:?}: a second read must not move the status on, got {}",
        "repeated read while ongoing",
        again.raw
    );
    assert_eq!(
        reported_cost_lines(&again),
        reported_cost_lines(&ongoing),
        "case {:?}: expected the same cost lines on a repeated read, got {}",
        "repeated read while ongoing",
        again.raw
    );

    // Past the completion instant the clock alone has finished the work.
    tokio::time::sleep(COST_ESTIMATION_DURATION + Duration::from_millis(250)).await;
    let completed = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        reported_status(&completed),
        Some("COMPLETED"),
        "case {:?}: expected COMPLETED once the estimation is due, got {}",
        "past the completion instant",
        completed.raw
    );
    let end_time = completed.at("/TimeRange/EndTime").cloned();
    assert!(
        end_time.is_some(),
        "case {:?}: expected a finished estimation to report its end time, got {}",
        "past the completion instant",
        completed.raw
    );
    assert_eq!(
        reported_cost_lines(&completed),
        reported_cost_lines(&ongoing),
        "case {:?}: finishing must not change the costs computed at the start, got {}",
        "past the completion instant",
        completed.raw
    );
    let expected_total = monthly_cost(2);
    let total = reported_total_cost(&completed);
    assert!(
        (total - expected_total).abs() < MONEY_EPSILON,
        "case {:?}: expected TotalCost {expected_total} for two priced types, got {total} in {}",
        "past the completion instant",
        completed.raw
    );

    let settled = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        reported_status(&settled),
        Some("COMPLETED"),
        "case {:?}: expected the finished status to stay put, got {}",
        "repeated read after completion",
        settled.raw
    );
    assert_eq!(
        settled.at("/TimeRange/EndTime").cloned(),
        end_time,
        "case {:?}: expected the same end time on every read, got {}",
        "repeated read after completion",
        settled.raw
    );
}

/// RULE: `ClientToken` is an idempotency token — a retry of the same request under it
/// answers from the estimation that token already started instead of starting another,
/// and the same token with a different collection is a `ConflictException` (409) while
/// that estimation is still being created, as is any start under another token; once an
/// estimation has finished it names no work in flight, so the next start replaces it,
/// whichever token it carries.
///
/// The rows differ along whether the token repeats, whether the collection repeats and
/// where the clock stands relative to the running estimation's completion.
#[tokio::test]
async fn a_second_start_is_idempotent_under_its_token_and_conflicts_otherwise() {
    let emulator = Emulator::new();
    add_coverage(&emulator, cloud_formation(&["alpha-stack"])).await;
    add_coverage(&emulator, cloud_formation(&["beta-stack"])).await;

    let alpha = cloud_formation(&["alpha-stack"]);
    let beta = cloud_formation(&["beta-stack"]);

    let first = start_cost_estimation(&emulator, &alpha, Some("tokenone")).await;
    assert_eq!(
        first.status, 200,
        "case {:?}: expected 200, got {} {}",
        "first start", first.status, first.raw
    );
    let first_read = get_cost_estimation(&emulator, None).await;
    let first_start_time = first_read.at("/TimeRange/StartTime").cloned();

    // Same token, same collection: the retry names the estimation already running.
    let retry = start_cost_estimation(&emulator, &alpha, Some("tokenone")).await;
    assert_eq!(
        retry.status, 200,
        "case {:?}: expected a retry under the same token to be accepted, got {} {}",
        "retry with the same token and collection", retry.status, retry.raw
    );
    let retried_read = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        retried_read.at("/TimeRange/StartTime").cloned(),
        first_start_time,
        "case {:?}: a retry must not restart the estimation, got {}",
        "retry with the same token and collection",
        retried_read.raw
    );

    // Same token, different collection: two requests sharing one idempotency token.
    let reused = start_cost_estimation(&emulator, &beta, Some("tokenone")).await;
    assert_eq!(
        reused.status, 409,
        "case {:?}: expected HTTP 409, got {} {}",
        "same token with a different collection", reused.status, reused.raw
    );
    assert!(
        reused.names_error("ConflictException"),
        "case {:?}: expected ConflictException, got {}",
        "same token with a different collection",
        reused.raw
    );
    assert_eq!(
        reused.at("/ResourceType").and_then(Value::as_str),
        Some("COST_ESTIMATION"),
        "case {:?}: expected the conflict to name the resource's type, got {}",
        "same token with a different collection",
        reused.raw
    );

    // A different token while the estimation is still being created.
    let concurrent = start_cost_estimation(&emulator, &beta, Some("tokentwo")).await;
    assert_eq!(
        concurrent.status, 409,
        "case {:?}: expected HTTP 409, got {} {}",
        "a different token while one is ongoing", concurrent.status, concurrent.raw
    );
    assert!(
        concurrent.names_error("ConflictException"),
        "case {:?}: expected ConflictException, got {}",
        "a different token while one is ongoing",
        concurrent.raw
    );
    let unchanged = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        unchanged.strings_at("/ResourceCollection/CloudFormation/StackNames"),
        vec!["alpha-stack".to_string()],
        "case {:?}: a refused start must leave the running estimation alone, got {}",
        "a different token while one is ongoing",
        unchanged.raw
    );

    // Once it has finished, a new start replaces it.
    tokio::time::sleep(COST_ESTIMATION_DURATION + Duration::from_millis(250)).await;
    let replacement = start_cost_estimation(&emulator, &beta, Some("tokentwo")).await;
    assert_eq!(
        replacement.status, 200,
        "case {:?}: expected a start after completion to be accepted, got {} {}",
        "a new start after completion", replacement.status, replacement.raw
    );
    let replaced = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        replaced.strings_at("/ResourceCollection/CloudFormation/StackNames"),
        vec!["beta-stack".to_string()],
        "case {:?}: expected the new collection to be reported, got {}",
        "a new start after completion",
        replaced.raw
    );
    assert!(
        reports_an_estimation(&replaced),
        "case {:?}: expected the replacement estimation to read back, got {}",
        "a new start after completion",
        replaced.raw
    );

    // A finished estimation's own token no longer names work in flight, so reusing it
    // for a different collection replaces the finished estimate instead of conflicting.
    tokio::time::sleep(COST_ESTIMATION_DURATION + Duration::from_millis(250)).await;
    let reused_after_completion = start_cost_estimation(&emulator, &alpha, Some("tokentwo")).await;
    assert_eq!(
        reused_after_completion.status, 200,
        "case {:?}: expected the finished estimation to be replaced, got {} {}",
        "the finished estimation's own token with a different collection",
        reused_after_completion.status,
        reused_after_completion.raw
    );
    let replaced_again = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        replaced_again.strings_at("/ResourceCollection/CloudFormation/StackNames"),
        vec!["alpha-stack".to_string()],
        "case {:?}: expected the newly named collection to be reported, got {}",
        "the finished estimation's own token with a different collection",
        replaced_again.raw
    );
}

/// RULE: before any `StartCostEstimation` there is no estimation to report, so
/// `GetCostEstimation` answers the modelled `TotalCost` default of 0 and no other
/// member; and a `NextToken` this operation did not issue names no page of it, which is
/// a `ValidationException` (400).
///
/// The token rows differ along form: a well-formed cursor this collection never
/// issued, one whose offset is off a page boundary, and one outside the modelled
/// lowercase-hex shape.
#[tokio::test]
async fn reading_with_no_estimation_reports_nothing_and_refuses_a_foreign_token() {
    let emulator = Emulator::new();

    let empty = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        empty.status, 200,
        "case {:?}: expected 200, got {} {}",
        "no estimation started", empty.status, empty.raw
    );
    assert!(
        (reported_total_cost(&empty) - 0.0).abs() < MONEY_EPSILON,
        "case {:?}: expected the modelled TotalCost default of 0, got {}",
        "no estimation started",
        empty.raw
    );
    for member in ["/Status", "/Costs", "/ResourceCollection", "/TimeRange", "/NextToken"] {
        assert_eq!(
            empty.at(member),
            None,
            "case {:?}: expected {member} to be absent with no estimation, got {}",
            "no estimation started",
            empty.raw
        );
    }

    let foreign_tokens = [
        // Well formed, but carries another collection's scope digest.
        "00000000-0000-0000-0000-000000000000",
        // An offset that names no page boundary.
        "00000007-0000-0000-0000-000000000000",
        // Outside `UuidNextToken`'s lowercase-hex shape.
        "0000000G-0000-0000-0000-000000000000",
    ];
    for token in foreign_tokens {
        let refused = get_cost_estimation(&emulator, Some(token)).await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: expected HTTP 400, got {} {}",
            refused.status, refused.raw
        );
        assert!(
            refused.names_error("ValidationException"),
            "case {token:?}: expected ValidationException, got {}",
            refused.raw
        );
    }

    // The same refusal once an estimation exists: a cursor this read never issued
    // still names no page of it.
    add_coverage(&emulator, cloud_formation(&["token-stack"])).await;
    let started = start_cost_estimation(&emulator, &cloud_formation(&["token-stack"]), None).await;
    assert_eq!(
        started.status, 200,
        "case {:?}: expected 200, got {} {}",
        "start before the token rows", started.status, started.raw
    );
    let refused = get_cost_estimation(&emulator, Some(foreign_tokens[0])).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected HTTP 400, got {} {}",
        "foreign token against an existing estimation", refused.status, refused.raw
    );
    assert!(
        refused.names_error("ValidationException"),
        "case {:?}: expected ValidationException, got {}",
        "foreign token against an existing estimation",
        refused.raw
    );
}
