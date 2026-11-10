// SPDX-License-Identifier: CC0-1.0

//! `TagResource`: what a tag write leaves on a Glue resource, read back through `GetTags`.
//!
//! RULE: a tag write MERGES the tags it carries into the tag set of the one resource its
//! `ResourceArn` names — a key that resource does not hold yet is added, a key it already
//! holds takes the value this write sends, and every key the write does not name keeps the
//! value it had — for every kind of Glue resource that carries a tag set, and the merged
//! set is exactly what the tag read of that ARN reports. One tag set holds at most as many
//! entries as the modelled map that serves it, so a write that would carry a resource past
//! that is refused whole. A `ResourceArn` naming a Region other than the one serving the
//! request is a bad parameter, as is one naming no taggable Glue resource kind at all; an
//! ARN of a taggable kind that resolves to no stored resource is a miss. A refused write
//! stores nothing.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: which kind of resource is tagged,
//! whether its create recorded tags of its own, how many keys one write carries, whether
//! those keys are new or already held, the form of the key and the value — including both
//! published bounds of `TagKey` (len 1..128) and of `TagValue` (len 0..256) — how full the
//! resource's tag set already is, and which part of the ARN does not address a resource
//! this endpoint serves.
//!
//! The fixtures a tagging suite needs - the create of each taggable kind, the tag-set
//! helpers and the read that reports one - are `pub` here and shared with the removal suite
//! beside this one ([`crate::resource_untagging`]), so a tag write and a tag removal are
//! driven through one harness.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION,
};

/// The IAM role a crawler and a job are created with. A role is account-global, so one
/// backs every resource here.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-role";

/// The Regions, other than the harness's own, these tests address an endpoint in. They
/// span three partitions' worth of prefixes, so nothing here depends on one Region name.
pub const OTHER_REGIONS: [&str; 3] = ["us-west-2", "eu-west-1", "ap-southeast-1"];

/// The sentence the service answers for a `ResourceArn` that belongs to another Region.
/// Recorded wording: it names neither the ARN nor either Region.
pub const ARN_WRONG_REGION: &str = "Arn passed doesn't belong to this region.";

/// An ARN of a Glue resource, as the modelled `GlueResourceArn` pattern spells one
/// (`^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:`).
pub fn glue_arn(region: &str, account: &str, resource: &str) -> String {
    format!("arn:aws:glue:{region}:{account}:{resource}")
}

/// The ARN of `name` of the kind `segment`, as served in this harness's Region and account.
pub fn resource_arn(segment: &str, name: &str) -> String {
    glue_arn(REGION, ACCOUNT_ID, &format!("{segment}/{name}"))
}

/// The create that registers one resource of the kind `segment`, carrying `tags`.
///
/// The kinds are the ones whose create models a `Tags` map — `CreateDatabase$Tags`,
/// `CreateConnection$Tags`, `CreateCrawler$Tags` and `CreateJob$Tags` (from
/// `smithy_query operation=CreateDatabase,CreateConnection,CreateCrawler,CreateJob`) — so
/// each of them has a tag set a tag write can land in.
pub fn create_request(segment: &str, name: &str, tags: &Value) -> (&'static str, Value) {
    match segment {
        "database" => (
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": name }, "Tags": tags }),
        ),
        "connection" => (
            "CreateConnection",
            json!({
                "ConnectionInput": {
                    "Name": name,
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": {
                        "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                        "USERNAME": "etl",
                    },
                },
                "Tags": tags,
            }),
        ),
        "crawler" => (
            "CreateCrawler",
            json!({
                "Name": name,
                "Role": ROLE_ARN,
                "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
                "Tags": tags,
            }),
        ),
        "job" => (
            "CreateJob",
            json!({
                "Name": name,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
                "Tags": tags,
            }),
        ),
        other => panic!("no taggable Glue resource kind spells {other:?}"),
    }
}

/// A `TagsMap` JSON object as a sorted map, so two tag sets compare by content.
pub fn map_of(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .unwrap_or_else(|| panic!("a tag value is a string, got {value}"))
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `TagsMap` of `count` distinct keys, each with its own value.
pub fn tags_of(count: usize, prefix: &str) -> Value {
    let mut members = Map::new();
    for index in 0..count {
        members.insert(
            format!("{prefix}-key-{index:02}"),
            json!(format!("{prefix}-value-{index:02}")),
        );
    }
    Value::Object(members)
}

/// The tag set the service reports for `arn`.
pub async fn read_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let body = ok(router, "GetTags", &json!({ "ResourceArn": arn })).await;
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case}: GetTags must report a Tags map, got {body}"));
    map_of(tags)
}

/// One tagged resource: the kind, the name its create registers, the name its ARN spells
/// and the tags the create recorded.
struct ResourceRow {
    case: &'static str,
    segment: &'static str,
    name: String,
    /// The name the row's ARNs carry, which is the created name unless the row is there
    /// to show how that kind's names are matched.
    arn_name: String,
    tags: Value,
}

fn resource_rows() -> Vec<ResourceRow> {
    vec![
        ResourceRow {
            case: "a database whose create recorded no tags",
            segment: "database",
            name: "analytics_db".to_string(),
            arn_name: "analytics_db".to_string(),
            tags: json!({}),
        },
        ResourceRow {
            // A Hive-compatible catalog name is folded, so the ARN resolves whatever
            // case it spells the name in.
            case: "a database addressed by an ARN cased differently from its create",
            segment: "database",
            name: "Mixed_Case_DB".to_string(),
            arn_name: "MIXED_case_db".to_string(),
            tags: json!({ "owner": "catalog" }),
        },
        ResourceRow {
            case: "a connection whose create recorded one tag",
            segment: "connection",
            name: "warehouse".to_string(),
            arn_name: "warehouse".to_string(),
            tags: json!({ "owner": "data-platform" }),
        },
        ResourceRow {
            case: "a crawler whose create recorded two tags",
            segment: "crawler",
            name: "nightly-crawl".to_string(),
            arn_name: "nightly-crawl".to_string(),
            tags: json!({ "owner": "ops", "env": "dev" }),
        },
        ResourceRow {
            case: "a job definition whose create recorded one tag",
            segment: "job",
            name: "etl-usage-events".to_string(),
            arn_name: "etl-usage-events".to_string(),
            tags: json!({ "env": "test" }),
        },
    ]
}

/// One tag write of a sequence: what `TagsToAdd` carries.
struct WriteStep {
    case: &'static str,
    add: Value,
}

fn write_steps() -> Vec<WriteStep> {
    vec![
        WriteStep {
            case: "one key the resource does not hold yet",
            add: json!({ "cost-center": "1234" }),
        },
        WriteStep {
            case: "several keys at once, none of them held yet",
            add: json!({ "tier": "bronze", "owner": "analytics", "retention": "30" }),
        },
        WriteStep {
            case: "a key the resource already holds, with another value",
            add: json!({ "owner": "platform-eng" }),
        },
        WriteStep {
            case: "an empty TagsToAdd, which the modelled bound admits (len 0..50)",
            add: json!({}),
        },
        WriteStep {
            // The published lower bound of `TagValue` (len 0..256, from
            // `smithy_query shape=TagsMap`): the empty string is a value, not an absence.
            case: "a value of the shortest a TagValue may be",
            add: json!({ "optional": "" }),
        },
        WriteStep {
            // And its upper bound.
            case: "a value of the longest a TagValue may be",
            add: json!({ "blob": "v".repeat(256) }),
        },
        WriteStep {
            // The published lower bound of `TagKey` (len 1..128, same source).
            case: "a key of the shortest a TagKey may be",
            add: json!({ "k": "one-character-key" }),
        },
        WriteStep {
            // And its upper bound.
            case: "a key of the longest a TagKey may be",
            add: json!({ "K".repeat(128): "one-hundred-and-twenty-eight-character-key" }),
        },
        WriteStep {
            case: "a key already held, back to the value its create recorded",
            add: json!({ "owner": "data-platform" }),
        },
    ]
}

/// A tag write merges its tags into the tag set of the resource its ARN names, for every
/// kind of Glue resource that carries one, and leaves every other resource alone.
#[tokio::test]
async fn a_tag_write_merges_into_the_named_resources_own_tag_set() {
    let router = emulator();
    let rows = resource_rows();
    let steps = write_steps();

    // Every resource exists before the first write, so a write that landed on the wrong
    // record would be visible as another resource's tag set changing.
    let mut expected: Vec<BTreeMap<String, String>> = Vec::new();
    for row in &rows {
        let (operation, body) = create_request(row.segment, &row.name, &row.tags);
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: {operation} must answer 200, got {status} with {answer}",
            row.case
        );
        expected.push(map_of(&row.tags));
    }

    for (index, row) in rows.iter().enumerate() {
        let arn = resource_arn(row.segment, &row.arn_name);
        // What the create recorded is what the read reports before any write.
        let got = read_tags(&router, row.case, &arn).await;
        assert_eq!(
            got, expected[index],
            "case {:?}: the create's tags are what the read reports, got {got:?}",
            row.case
        );

        for step in &steps {
            let case = format!("{} / {}", row.case, step.case);
            let added = map_of(&step.add);
            let before = expected[index].clone();

            let (status, answer) = call(
                &router,
                "TagResource",
                &json!({ "ResourceArn": arn, "TagsToAdd": step.add }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: TagResource must answer 200, got {status} with {answer}"
            );
            assert!(
                answer
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty),
                "case {case}: TagResource models no output member, got {answer}"
            );

            for (key, value) in &added {
                expected[index].insert(key.clone(), value.clone());
            }
            let got = read_tags(&router, &case, &arn).await;
            assert_eq!(
                got, expected[index],
                "case {case}: the write must leave the merge of what was held and what it \
                 sent, got {got:?}"
            );
            for (key, value) in &added {
                assert_eq!(
                    got.get(key),
                    Some(value),
                    "case {case}: the key {key:?} the write named must take the value it sent, \
                     got {got:?}"
                );
            }
            for (key, value) in &before {
                if added.contains_key(key) {
                    continue;
                }
                assert_eq!(
                    got.get(key),
                    Some(value),
                    "case {case}: the key {key:?} the write did not name must keep its value, \
                     got {got:?}"
                );
            }

            // No other resource's tag set moved.
            for (other, bystander) in rows.iter().enumerate() {
                if other == index {
                    continue;
                }
                let bystander_arn = resource_arn(bystander.segment, &bystander.arn_name);
                let got = read_tags(&router, &case, &bystander_arn).await;
                assert_eq!(
                    got, expected[other],
                    "case {case}: the write must leave {:?} alone, got {got:?}",
                    bystander.case
                );
            }
        }
    }
}

/// One tag-bound row: how many tags the resource is created with, how many of the keys
/// the write sends are new, and whether the write is served.
struct BoundRow {
    case: &'static str,
    name: &'static str,
    held: usize,
    /// Keys the write sends that the resource does not hold, beyond the ones it does.
    fresh: usize,
    /// Keys the write re-sends of the ones the resource already holds.
    rewritten: usize,
    served: bool,
}

fn bound_rows() -> Vec<BoundRow> {
    vec![
        BoundRow {
            case: "an untagged resource tagged to exactly the bound in one write",
            name: "bound_empty_to_full",
            held: 0,
            fresh: MAX_TAGS_PER_RESOURCE,
            rewritten: 0,
            served: true,
        },
        BoundRow {
            case: "a resource at the bound, every key of the write already held",
            name: "bound_full_rewrite",
            held: MAX_TAGS_PER_RESOURCE,
            fresh: 0,
            rewritten: MAX_TAGS_PER_RESOURCE,
            served: true,
        },
        BoundRow {
            case: "a resource one short of the bound, taken to it",
            name: "bound_one_short",
            held: MAX_TAGS_PER_RESOURCE - 1,
            fresh: 1,
            rewritten: 0,
            served: true,
        },
        BoundRow {
            case: "a resource at the bound, one key past it",
            name: "bound_one_past",
            held: MAX_TAGS_PER_RESOURCE,
            fresh: 1,
            rewritten: 0,
            served: false,
        },
        BoundRow {
            case: "a resource holding one tag, a write of the bound's worth of new keys",
            name: "bound_one_plus_full",
            held: 1,
            fresh: MAX_TAGS_PER_RESOURCE,
            rewritten: 0,
            served: false,
        },
    ]
}

/// A tag set holds at most as many entries as the modelled map that serves it: a write
/// that would leave the resource past that is refused whole and stores nothing, while one
/// that lands on or under it is served.
#[tokio::test]
async fn a_write_past_the_tag_bound_is_refused_and_stores_nothing() {
    let router = emulator();

    for row in bound_rows() {
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

        // The write: the re-sent keys carry a new value, so a served write is visible.
        let mut add = Map::new();
        for (index, key) in before.keys().take(row.rewritten).enumerate() {
            add.insert(key.clone(), json!(format!("rewritten-{index:02}")));
        }
        for (key, value) in map_of(&tags_of(row.fresh, "fresh")) {
            add.insert(key, json!(value));
        }
        let add = Value::Object(add);

        let (status, answer) = call(
            &router,
            "TagResource",
            &json!({ "ResourceArn": arn, "TagsToAdd": add }),
        )
        .await;
        let mut merged = before.clone();
        for (key, value) in map_of(&add) {
            merged.insert(key, value);
        }

        if row.served {
            assert_eq!(
                status, 200,
                "case {:?}: a write landing on or under the bound is served, got {status} with \
                 {answer}",
                row.case
            );
            let got = read_tags(&router, row.case, &arn).await;
            assert_eq!(
                got, merged,
                "case {:?}: the served write must leave the merged set, got {got:?}",
                row.case
            );
            assert!(
                got.len() <= MAX_TAGS_PER_RESOURCE,
                "case {:?}: a tag set holds at most {MAX_TAGS_PER_RESOURCE} entries, got {}",
                row.case,
                got.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: a write past the bound is refused, got {status} with {answer}",
                row.case
            );
            assert!(
                error_code(&answer).ends_with("InvalidInputException"),
                "case {:?}: expected InvalidInputException, got {}",
                row.case,
                error_code(&answer)
            );
            assert_eq!(
                error_message(&answer),
                format!(
                    "TagsToAdd would leave the resource holding {} tags, more than the \
                     {MAX_TAGS_PER_RESOURCE} allowed per resource",
                    merged.len()
                ),
                "case {:?}: unexpected wording for a write past the tag bound",
                row.case
            );
            let got = read_tags(&router, row.case, &arn).await;
            assert_eq!(
                got, before,
                "case {:?}: the refused write must store nothing, got {got:?}",
                row.case
            );
        }
    }
}

/// What [`error_message`] answers for a refusal that carries NO prose at all, which is how
/// the tagging surface refuses an ARN that is none of its resources: the code alone, with the
/// modelled `Message` member absent from the body.
const NO_PROSE: &str = "<no Message>";

/// One refused tag write: the Region the request is served in, the ARN it names, and the
/// refusal it earns.
struct RefusalRow {
    case: String,
    region: &'static str,
    arn: String,
    code: &'static str,
    /// The wording the refusal must carry, or `None` for a row whose wording is the
    /// per-kind sentence another row of the same kind already pins.
    message: Option<String>,
}

/// The resources every refusal row is tried beside, so a refusal that wrote anyway is
/// visible as one of their tag sets changing.
fn bystanders() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("database", "refusal_db", json!({ "owner": "catalog" })),
        ("connection", "refusal-connection", json!({ "owner": "etl" })),
        ("crawler", "refusal-crawler", json!({ "env": "dev" })),
        ("job", "refusal-job", json!({ "env": "test" })),
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
    // outside the kinds a tag write accepts is a bad parameter, and that refusal states NO
    // prose at all — the code alone.
    for resource in [
        "table/refusal_db/orders",
        "schema/registry-1/schema-1",
        "devEndpoint/dev-1",
        "trigger/nightly",
        "userDefinedFunction/refusal_db/to_upper",
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

    // An ARN of a taggable kind that resolves to no stored resource: a miss, worded as
    // this crate words that kind's miss wherever it fails to resolve.
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

    // Another account's estate: this emulator serves one account, so an ARN of another
    // names a resource it holds nothing for — worded as that kind's miss, which is what
    // the unresolved-name rows above pin, so these rows assert the code alone.
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

/// A tag write whose `ResourceArn` does not address a resource this endpoint serves is
/// refused — as a bad parameter when the ARN's Region or resource type is not this
/// endpoint's, and as a miss when it resolves to no stored resource — and stores nothing.
#[tokio::test]
async fn a_tag_write_for_an_arn_this_endpoint_does_not_serve_is_refused() {
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
            "TagResource",
            &json!({ "ResourceArn": row.arn, "TagsToAdd": { "written": "no" } }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the write must be refused, got {status} with {answer}",
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

        // Nothing was written: every resource this endpoint does serve is as it was.
        for (index, (segment, name, _)) in present.iter().enumerate() {
            let arn = resource_arn(segment, name);
            let got = read_tags(&router, &row.case, &arn).await;
            assert_eq!(
                got, expected[index],
                "case {:?}: the refused write must store nothing, got {got:?} for {arn}",
                row.case
            );
        }
    }
}
