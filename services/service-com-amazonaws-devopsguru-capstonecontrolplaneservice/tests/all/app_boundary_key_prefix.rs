// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the form of the app-boundary key a client may use to define
//! this account's DevOps Guru analysis coverage.
//!
//! The suite drives the service through the shared [`Emulator`] harness in
//! `configuration_lifecycle`, so every assertion is about the wire behaviour.
//!
//! Each test states the RULE it pins and exercises it over a set of keys that differ
//! along what the rule generalises over: the case of the prefix, what follows it,
//! where in the key it appears, which characters stand in for its separators, and the
//! `Action` the key is submitted under.

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::APP_BOUNDARY_KEY_PREFIX;

use crate::configuration_lifecycle::{tags, Emulator};

/// One app-boundary key, and whether the documented prefix rule lets it define
/// coverage.
struct KeyCase {
    name: &'static str,
    key: &'static str,
    defines_coverage: bool,
}

/// The tag value the rows submit under their key, and the baseline coverage a `REMOVE`
/// row runs against — a conforming key, so a row's refusal is about the row's key.
const ROW_VALUE: &str = "coverage-value";
const BASELINE_KEY: &str = "Devops-guru-baseline";
const BASELINE_VALUE: &str = "baseline-value";

/// The key forms the rules below are checked over.
///
/// The conforming forms include both keys the documentation names as examples —
/// `DevOps-Guru-deployment-application` and `devops-guru-rds-application`, from
/// `smithy_query(query="doc",shape="UpdateTagCollectionFilter",member="AppBoundaryKey")`
/// — the bare prefix with nothing after it, and a key whose suffix is outside ASCII
/// (`AppBoundaryKey`'s pattern admits any `\p{L}`). The non-conforming forms differ in
/// HOW the prefix is missed: absent, truncated, carrying another separator, placed
/// after something else, or pushed off the front by a leading separator character.
const CASES: &[KeyCase] = &[
    KeyCase {
        name: "the prefix cased as the documentation writes it",
        key: "Devops-guru-app",
        defines_coverage: true,
    },
    KeyCase {
        name: "the documentation's mixed-case example key",
        key: "DevOps-Guru-deployment-application",
        defines_coverage: true,
    },
    KeyCase {
        name: "the documentation's lower-case example key",
        key: "devops-guru-rds-application",
        defines_coverage: true,
    },
    KeyCase {
        name: "an upper-case prefix",
        key: "DEVOPS-GURU-TEAM",
        defines_coverage: true,
    },
    KeyCase {
        name: "the prefix and nothing after it",
        key: APP_BOUNDARY_KEY_PREFIX,
        defines_coverage: true,
    },
    KeyCase {
        name: "a non-ASCII suffix after the prefix",
        key: "Devops-guru-équipe",
        defines_coverage: true,
    },
    KeyCase {
        name: "a key naming no prefix at all",
        key: "Team",
        defines_coverage: false,
    },
    KeyCase {
        name: "a key shorter than the prefix",
        key: "Devops",
        defines_coverage: false,
    },
    KeyCase {
        name: "the prefix without its trailing separator",
        key: "Devops-guru",
        defines_coverage: false,
    },
    KeyCase {
        name: "the prefix with another separator inside it",
        key: "Devops_guru-app",
        defines_coverage: false,
    },
    KeyCase {
        name: "the prefix somewhere other than the start",
        key: "my-Devops-guru-app",
        defines_coverage: false,
    },
    KeyCase {
        name: "a separator character before the prefix",
        key: " Devops-guru-app",
        defines_coverage: false,
    },
];

/// RULE: a tag key submitted to `UpdateResourceCollection` to define the account's
/// analysis coverage must begin, ignoring case, with the prefix the model's
/// `AppBoundaryKey` documentation requires; a request naming a key that does not is
/// refused with `ValidationException` (400) naming that member, whatever the `Action`,
/// and the stored coverage is left exactly as it was.
///
/// The rows are the key forms in [`CASES`], each submitted under both `Action` values:
/// `ADD` against an account covering nothing, `REMOVE` against an account covering a
/// conforming key.
#[tokio::test]
async fn a_coverage_key_must_begin_with_the_documented_prefix() {
    for action in ["ADD", "REMOVE"] {
        for case in CASES {
            let emulator = Emulator::new();
            let baseline: Vec<(String, Vec<String>)> = if action == "REMOVE" {
                let seeded = emulator
                    .update_resource_collection(
                        "ADD",
                        tags(&[(BASELINE_KEY, &[BASELINE_VALUE])]),
                    )
                    .await;
                assert_eq!(
                    seeded.status, 200,
                    "case {:?} ({action}): the baseline coverage must be accepted, got {} {}",
                    case.name, seeded.status, seeded.raw
                );
                vec![(
                    BASELINE_KEY.to_string(),
                    vec![BASELINE_VALUE.to_string()],
                )]
            } else {
                Vec::new()
            };

            let reply = emulator
                .update_resource_collection(action, tags(&[(case.key, &[ROW_VALUE])]))
                .await;

            let expect_status = if case.defines_coverage { 200 } else { 400 };
            assert_eq!(
                reply.status, expect_status,
                "case {:?} ({action}, key {:?}): expected HTTP {expect_status}, got {} {}",
                case.name, case.key, reply.status, reply.raw
            );

            let expect_tags: Vec<(String, Vec<String>)> =
                match (case.defines_coverage, action) {
                    (true, "ADD") => {
                        vec![(case.key.to_string(), vec![ROW_VALUE.to_string()])]
                    }
                    _ => baseline.clone(),
                };
            let actual_tags = emulator.all_tag_collections().await;
            assert_eq!(
                actual_tags, expect_tags,
                "case {:?} ({action}, key {:?}): expected coverage {expect_tags:?}, got {actual_tags:?}",
                case.name, case.key
            );

            if case.defines_coverage {
                continue;
            }
            assert!(
                reply.names_error("ValidationException"),
                "case {:?} ({action}, key {:?}): expected a ValidationException, got {}",
                case.name,
                case.key,
                reply.raw
            );
            let message = reply
                .at("/Message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains(APP_BOUNDARY_KEY_PREFIX) && message.contains(case.key),
                "case {:?} ({action}, key {:?}): expected the message to name the key and the required prefix, got {message:?}",
                case.name,
                case.key
            );
            let field = reply
                .at("/Fields/0/Name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                field, "ResourceCollection.Tags[].AppBoundaryKey",
                "case {:?} ({action}, key {:?}): expected the refused member to be named, got {field:?} in {}",
                case.name,
                case.key,
                reply.raw
            );
        }
    }
}

/// RULE (the same rule, from the other side): because a key outside the documented
/// prefix never becomes coverage, nothing the emulator derives from coverage reports it
/// — and the refused request persists nothing else it carried either, so its
/// CloudFormation half is not half-applied.
///
/// The derived readers are a row set in their own right: the collection read, the
/// monitored-resource listing and the account-health counts, which is every reader the
/// coverage store feeds for the tag half.
#[tokio::test]
async fn a_key_without_the_prefix_is_coverage_for_no_reader() {
    let emulator = Emulator::new();
    let refused = emulator
        .update_resource_collection(
            "ADD",
            json!({
                "CloudFormation": { "StackNames": ["late-stack"] },
                "Tags": [{ "AppBoundaryKey": "Team", "TagValues": ["payments"] }],
            }),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected HTTP 400, got {} {}",
        "an ADD naming a key outside the prefix", refused.status, refused.raw
    );

    let covered = emulator.all_tag_collections().await;
    assert!(
        covered.is_empty(),
        "case {:?}: expected no tag coverage, got {covered:?}",
        "GetResourceCollection(AWS_TAGS)"
    );
    let stacks = emulator.all_stack_names().await;
    assert!(
        stacks.is_empty(),
        "case {:?}: expected the refused request to persist no stack either, got {stacks:?}",
        "GetResourceCollection(AWS_CLOUD_FORMATION)"
    );

    // `POST /monitoredResources` — `ListMonitoredResources`'s http binding.
    let listed = emulator
        .call("POST", "/monitoredResources", Some(&json!({})))
        .await;
    assert_eq!(
        listed.status, 200,
        "case {:?}: the listing must succeed, got {}",
        "ListMonitoredResources", listed.raw
    );
    let resources = listed
        .at("/MonitoredResourceIdentifiers")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        resources, 0,
        "case {:?}: expected no monitored resource, got {} in {}",
        "ListMonitoredResources", resources, listed.raw
    );

    // `GET /accounts/health` — `DescribeAccountHealth`'s http binding.
    let health = emulator.call("GET", "/accounts/health", None).await;
    assert_eq!(
        health.status, 200,
        "case {:?}: the health read must succeed, got {}",
        "DescribeAccountHealth", health.raw
    );
    for member in ["/AnalyzedResourceCount", "/ResourceHours"] {
        let counted = health.at(member).and_then(Value::as_i64).unwrap_or_default();
        assert_eq!(
            counted, 0,
            "case {member:?}: expected nothing analysed, got {counted} in {}",
            health.raw
        );
    }
}
