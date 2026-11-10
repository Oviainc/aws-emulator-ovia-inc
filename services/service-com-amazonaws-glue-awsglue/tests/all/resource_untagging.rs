// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`: what a tag removal leaves on a Glue resource, read back through
//! `GetTags`.
//!
//! RULE: a tag removal DROPS from the tag set of the one resource its `ResourceArn` names
//! exactly the keys `TagsToRemove` lists - a key that resource holds goes together with its
//! value, a key it does not hold leaves the set as it was, and every key the request does
//! not name keeps the value it had - for every kind of Glue resource that carries a tag set,
//! and what is left is exactly what the tag read of that ARN reports. A `ResourceArn`
//! naming a Region other than the one serving the request is a bad parameter, as is one
//! naming no taggable Glue resource kind at all; an ARN of a taggable kind that resolves to
//! no stored resource is a miss. A refused removal changes nothing.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The resources, their
//! creates and the tag read are the fixtures the tag-write suite beside this one declares
//! ([`crate::resource_tagging`]), so a write and a removal of one ARN are driven the same
//! way.
//!
//! The rows differ along what the rule generalises over: which kind of resource is untagged,
//! how many keys one request names, whether those keys are held or not held, whether a key
//! is named twice in one request, the form of the key and of the value it drops - including
//! both published bounds of `TagKey` (len 1..128) and of `TagValue` (len 0..256) - how many
//! of the resource's tags are left afterwards, both published bounds of `TagKeysList`
//! (len 0..50) and one key past its maximum, and which part of the ARN does not address a
//! resource this endpoint serves.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::MAX_TAGS_PER_RESOURCE;

use crate::resource_tagging::{
    create_request, glue_arn, map_of, read_tags, resource_arn, tags_of, ARN_WRONG_REGION,
    OTHER_REGIONS,
};
use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// How many keys one `TagsToRemove` may list.
///
/// Source: the modelled `TagKeysList` bound on `UntagResourceRequest$TagsToRemove`
/// (len 0..50, from `smithy_query operation=UntagResource`).
const MAX_KEYS_PER_REMOVAL: usize = 50;

/// A key of the shortest a `TagKey` may be (len 1..128, from `smithy_query shape=TagsMap`).
const SHORTEST_KEY: &str = "k";

/// The tag set every resource under test is created with: keys of several forms, including
/// both published bounds of `TagKey`, and values including both published bounds of
/// `TagValue` (len 0..256, same source).
fn initial_tags() -> Value {
    json!({
        "owner": "data-platform",
        "env": "dev",
        "cost-center": "1234",
        SHORTEST_KEY: "one-character-key",
        "K".repeat(128): "one-hundred-and-twenty-eight-character-key",
        "optional": "",
        "blob": "v".repeat(256),
    })
}

/// One resource a removal is tried against: the kind, the name its create registers and the
/// name its ARN spells.
struct ResourceRow {
    case: &'static str,
    segment: &'static str,
    name: &'static str,
    /// The name the row's ARN carries, which is the created name unless the row is there to
    /// show how that kind's names are matched.
    arn_name: &'static str,
}

fn resource_rows() -> Vec<ResourceRow> {
    vec![
        ResourceRow {
            case: "a database",
            segment: "database",
            name: "untag_analytics_db",
            arn_name: "untag_analytics_db",
        },
        ResourceRow {
            // A Hive-compatible catalog name is folded, so the ARN resolves whatever case
            // it spells the name in.
            case: "a database addressed by an ARN cased differently from its create",
            segment: "database",
            name: "Untag_Mixed_Case_DB",
            arn_name: "UNTAG_mixed_case_db",
        },
        ResourceRow {
            case: "a connection",
            segment: "connection",
            name: "untag-warehouse",
            arn_name: "untag-warehouse",
        },
        ResourceRow {
            case: "a crawler",
            segment: "crawler",
            name: "untag-nightly-crawl",
            arn_name: "untag-nightly-crawl",
        },
        ResourceRow {
            case: "a job definition",
            segment: "job",
            name: "untag-etl-usage-events",
            arn_name: "untag-etl-usage-events",
        },
    ]
}

/// One removal of a sequence: what `TagsToRemove` lists.
struct RemovalStep {
    case: &'static str,
    remove: Vec<String>,
}

fn removal_steps() -> Vec<RemovalStep> {
    vec![
        RemovalStep {
            case: "one key the resource holds",
            remove: vec!["env".to_string()],
        },
        RemovalStep {
            case: "a key the resource does not hold",
            remove: vec!["never-set".to_string()],
        },
        RemovalStep {
            case: "several keys at once, some held and some not",
            remove: vec![
                "owner".to_string(),
                "never-set".to_string(),
                "cost-center".to_string(),
            ],
        },
        RemovalStep {
            case: "an empty TagsToRemove, which the modelled bound admits (len 0..50)",
            remove: Vec::new(),
        },
        RemovalStep {
            case: "the shortest key a TagKey may be, named twice in one request",
            remove: vec![SHORTEST_KEY.to_string(), SHORTEST_KEY.to_string()],
        },
        RemovalStep {
            case: "the longest key a TagKey may be",
            remove: vec!["K".repeat(128)],
        },
        RemovalStep {
            case: "a key whose value is the shortest a TagValue may be",
            remove: vec!["optional".to_string()],
        },
        RemovalStep {
            case: "the last key the resource holds, whose value is the longest a TagValue may be",
            remove: vec!["blob".to_string()],
        },
        RemovalStep {
            case: "every key again, with none of them left to drop",
            remove: vec![
                "owner".to_string(),
                "env".to_string(),
                "cost-center".to_string(),
                "blob".to_string(),
            ],
        },
    ]
}

/// A tag removal drops exactly the keys it names from the tag set of the resource its ARN
/// names, for every kind of Glue resource that carries one, leaves every key it does not
/// name alone, is idempotent, and leaves every other resource untouched.
#[tokio::test]
async fn a_tag_removal_drops_exactly_the_named_keys() {
    let router = emulator();
    let rows = resource_rows();
    let steps = removal_steps();
    let initial = initial_tags();

    // Every resource exists before the first removal, so a removal that landed on the wrong
    // record would be visible as another resource's tag set changing.
    let mut expected: Vec<BTreeMap<String, String>> = Vec::new();
    for row in &rows {
        let (operation, body) = create_request(row.segment, row.name, &initial);
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: {operation} must answer 200, got {status} with {answer}",
            row.case
        );
        expected.push(map_of(&initial));
    }

    for (index, row) in rows.iter().enumerate() {
        let arn = resource_arn(row.segment, row.arn_name);
        let got = read_tags(&router, row.case, &arn).await;
        assert_eq!(
            got, expected[index],
            "case {:?}: the create's tags are what the read reports, got {got:?}",
            row.case
        );

        for step in &steps {
            let case = format!("{} / {}", row.case, step.case);
            let before = expected[index].clone();

            // Twice over: a removal names the keys that must be gone afterwards, so the
            // second call is served and leaves the same set as the first.
            for attempt in ["first", "repeated"] {
                let case = format!("{case} ({attempt} call)");
                let (status, answer) = call(
                    &router,
                    "UntagResource",
                    &json!({ "ResourceArn": arn, "TagsToRemove": step.remove }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: UntagResource must answer 200, got {status} with {answer}"
                );
                assert!(
                    answer.as_object().is_none_or(serde_json::Map::is_empty),
                    "case {case}: UntagResource models no output member, got {answer}"
                );

                for key in &step.remove {
                    expected[index].remove(key);
                }
                let got = read_tags(&router, &case, &arn).await;
                assert_eq!(
                    got, expected[index],
                    "case {case}: the removal must leave what was held minus the keys it \
                     named, got {got:?}"
                );
                for key in &step.remove {
                    assert!(
                        !got.contains_key(key),
                        "case {case}: the key {key:?} the removal named must be gone, got \
                         {got:?}"
                    );
                }
                for (key, value) in &before {
                    if step.remove.contains(key) {
                        continue;
                    }
                    assert_eq!(
                        got.get(key),
                        Some(value),
                        "case {case}: the key {key:?} the removal did not name must keep its \
                         value, got {got:?}"
                    );
                }

                // No other resource's tag set moved.
                for (other, bystander) in rows.iter().enumerate() {
                    if other == index {
                        continue;
                    }
                    let bystander_arn = resource_arn(bystander.segment, bystander.arn_name);
                    let got = read_tags(&router, &case, &bystander_arn).await;
                    assert_eq!(
                        got, expected[other],
                        "case {case}: the removal must leave {:?} alone, got {got:?}",
                        bystander.case
                    );
                }
            }
        }

        let left = read_tags(&router, row.case, &arn).await;
        assert!(
            left.is_empty(),
            "case {:?}: removing every key must leave an empty tag set, got {left:?}",
            row.case
        );
    }
}

/// One `TagsToRemove` length row: how many tags the resource is created with, how many keys
/// the request lists, and whether the request is served.
struct ListBoundRow {
    case: &'static str,
    name: &'static str,
    held: usize,
    /// Keys the request lists, of the ones the resource holds.
    listed: usize,
    served: bool,
}

fn list_bound_rows() -> Vec<ListBoundRow> {
    vec![
        ListBoundRow {
            case: "an empty list, the published minimum of TagKeysList",
            name: "untag_list_bound_empty",
            held: 3,
            listed: 0,
            served: true,
        },
        ListBoundRow {
            case: "a list one short of the published maximum of TagKeysList",
            name: "untag_list_bound_one_short",
            held: MAX_TAGS_PER_RESOURCE,
            listed: MAX_KEYS_PER_REMOVAL - 1,
            served: true,
        },
        ListBoundRow {
            case: "a full tag set emptied in one request, at the published maximum",
            name: "untag_list_bound_full",
            held: MAX_TAGS_PER_RESOURCE,
            listed: MAX_KEYS_PER_REMOVAL,
            served: true,
        },
        ListBoundRow {
            case: "a list one key past the published maximum of TagKeysList",
            name: "untag_list_bound_one_past",
            held: MAX_TAGS_PER_RESOURCE,
            listed: MAX_KEYS_PER_REMOVAL + 1,
            served: false,
        },
    ]
}

/// A removal listing as many keys as the modelled list admits is served and drops every one
/// of them; a request past that bound is refused at the boundary and drops nothing.
#[tokio::test]
async fn a_removal_is_bounded_by_the_modelled_key_list() {
    let router = emulator();

    for row in list_bound_rows() {
        let held = tags_of(row.held, "held");
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": row.name }, "Tags": held }),
        )
        .await;
        let arn = resource_arn("database", row.name);
        let before = read_tags(&router, row.case, &arn).await;
        assert_eq!(
            before.len(),
            row.held,
            "case {:?}: the create must record {} tags, got {before:?}",
            row.case,
            row.held
        );

        // The keys the resource holds come first, then names it holds none of, so a row
        // listing more keys than the resource has tags is still exactly `listed` long.
        let mut remove: Vec<String> = before.keys().take(row.listed).cloned().collect();
        for index in remove.len()..row.listed {
            remove.push(format!("absent-key-{index:02}"));
        }
        assert_eq!(
            remove.len(),
            row.listed,
            "case {:?}: the request must list {} keys, got {}",
            row.case,
            row.listed,
            remove.len()
        );

        let (status, answer) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": arn, "TagsToRemove": remove }),
        )
        .await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {:?}: a removal within the modelled list bound is served, got {status} \
                 with {answer}",
                row.case
            );
            let mut left = before.clone();
            for key in &remove {
                left.remove(key);
            }
            let got = read_tags(&router, row.case, &arn).await;
            assert_eq!(
                got, left,
                "case {:?}: the served removal must leave what it did not name, got {got:?}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: a request outside the modelled constraints is refused at the \
                 boundary with 400, got {status} with {answer}",
                row.case
            );
            assert!(
                error_code(&answer).ends_with("ValidationException"),
                "case {:?}: expected the boundary's ValidationException, got {} with {}",
                row.case,
                error_code(&answer),
                error_message(&answer)
            );
            let got = read_tags(&router, row.case, &arn).await;
            assert_eq!(
                got, before,
                "case {:?}: the refused removal must drop nothing, got {got:?}",
                row.case
            );
        }
    }
}

/// What [`error_message`] answers for a refusal that carries NO prose at all, which is how
/// the tagging surface refuses an ARN that is none of its resources: the code alone, with the
/// modelled `Message` member absent from the body.
const NO_PROSE: &str = "<no Message>";

/// One refused removal: the Region the request is served in, the ARN it names, and the
/// refusal it earns.
struct RefusalRow {
    case: String,
    region: &'static str,
    arn: String,
    code: &'static str,
    /// The wording the refusal must carry, or `None` for a row whose wording is the per-kind
    /// sentence another row of the same kind already pins.
    message: Option<String>,
}

/// The resources every refusal row is tried beside, so a refusal that dropped a tag anyway
/// is visible as one of their tag sets changing.
fn bystanders() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("database", "untag_refusal_db", json!({ "owner": "catalog" })),
        (
            "connection",
            "untag-refusal-connection",
            json!({ "owner": "etl" }),
        ),
        ("crawler", "untag-refusal-crawler", json!({ "env": "dev" })),
        ("job", "untag-refusal-job", json!({ "env": "test" })),
    ]
}

fn refusal_rows() -> Vec<RefusalRow> {
    let mut rows: Vec<RefusalRow> = Vec::new();

    // An ARN of another Region: a bad parameter, whether or not the Region it names holds
    // the resource, and whichever endpoint serves the request.
    for region in OTHER_REGIONS {
        for (segment, name, _) in bystanders() {
            rows.push(RefusalRow {
                case: format!("{segment} of this account addressed through {region}'s ARN"),
                region: REGION,
                arn: glue_arn(region, ACCOUNT_ID, &format!("{segment}/{name}")),
                code: "InvalidInputException",
                message: Some(ARN_WRONG_REGION.to_string()),
            });
            rows.push(RefusalRow {
                case: format!("{segment} of this Region addressed through {region}'s endpoint"),
                region,
                arn: resource_arn(segment, name),
                code: "InvalidInputException",
                message: Some(ARN_WRONG_REGION.to_string()),
            });
        }
        rows.push(RefusalRow {
            case: format!("a {region} ARN naming a resource nothing created"),
            region: REGION,
            arn: glue_arn(region, ACCOUNT_ID, "database/never_created"),
            code: "InvalidInputException",
            message: Some(ARN_WRONG_REGION.to_string()),
        });
    }

    // A Glue ARN whose resource portion names no kind that carries a tag set. Everything
    // outside the kinds a tagging call accepts is a bad parameter, and that refusal states
    // NO prose at all — the code alone.
    for resource in [
        "table/untag_refusal_db/orders",
        "schema/registry-1/schema-1",
        "devEndpoint/dev-1",
        "trigger/nightly",
        "userDefinedFunction/untag_refusal_db/to_upper",
        "catalog",
        "database/",
    ] {
        let arn = glue_arn(REGION, ACCOUNT_ID, resource);
        rows.push(RefusalRow {
            case: format!("an ARN whose resource portion is {resource:?}"),
            region: REGION,
            message: Some(NO_PROSE.to_string()),
            arn,
            code: "InvalidInputException",
        });
    }

    // An ARN of a taggable kind that resolves to no stored resource: a miss, worded as this
    // crate words that kind's miss wherever it fails to resolve.
    rows.push(RefusalRow {
        case: "a database name this catalog holds nothing under".to_string(),
        region: REGION,
        arn: resource_arn("database", "never_created"),
        code: "EntityNotFoundException",
        message: Some("Database never_created not found.".to_string()),
    });
    rows.push(RefusalRow {
        case: "a crawler name this account holds nothing under".to_string(),
        region: REGION,
        arn: resource_arn("crawler", "never-created"),
        code: "EntityNotFoundException",
        message: Some("Crawler entry with name never-created does not exist".to_string()),
    });
    for segment in ["connection", "job"] {
        let arn = resource_arn(segment, "never-created");
        rows.push(RefusalRow {
            case: format!("a {segment} name this account holds nothing under"),
            region: REGION,
            message: Some(format!("Entity not found for resource {arn}")),
            arn,
            code: "EntityNotFoundException",
        });
    }

    // Another account's estate: this emulator serves one account, so an ARN of another names
    // a resource it holds nothing for - worded as that kind's miss, which the
    // unresolved-name rows above pin, so these rows assert the code alone.
    for (segment, name, _) in bystanders() {
        rows.push(RefusalRow {
            case: format!("a {segment} ARN of another account"),
            region: REGION,
            arn: glue_arn(REGION, "111122223333", &format!("{segment}/{name}")),
            code: "EntityNotFoundException",
            message: None,
        });
    }

    rows
}

/// A removal whose `ResourceArn` does not address a resource this endpoint serves is refused
/// - as a bad parameter when the ARN's Region or resource type is not this endpoint's, and
/// as a miss when it resolves to no stored resource - and drops nothing.
#[tokio::test]
async fn a_removal_for_an_arn_this_endpoint_does_not_serve_is_refused() {
    let router = emulator();
    let present = bystanders();

    let mut expected: Vec<BTreeMap<String, String>> = Vec::new();
    for (segment, name, tags) in &present {
        let (operation, body) = create_request(segment, name, tags);
        ok(&router, operation, &body).await;
        expected.push(map_of(tags));
    }

    for row in refusal_rows() {
        let (status, answer) = call_in_region(
            &router,
            row.region,
            "UntagResource",
            &json!({ "ResourceArn": row.arn, "TagsToRemove": ["owner", "env"] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the removal must be refused, got {status} with {answer}",
            row.case
        );
        assert!(
            error_code(&answer).ends_with(row.code),
            "case {:?}: expected {}, got {}",
            row.case,
            row.code,
            error_code(&answer)
        );
        if let Some(message) = &row.message {
            assert_eq!(
                &error_message(&answer),
                message,
                "case {:?}: unexpected wording for the refusal",
                row.case
            );
        }

        // Nothing was dropped: every resource this endpoint does serve is as it was.
        for (index, (segment, name, _)) in present.iter().enumerate() {
            let arn = resource_arn(segment, name);
            let got = read_tags(&router, &row.case, &arn).await;
            assert_eq!(
                got, expected[index],
                "case {:?}: the refused removal must drop nothing, got {got:?} for {arn}",
                row.case
            );
        }
    }
}
