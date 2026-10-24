// SPDX-License-Identifier: CC0-1.0

//! What `ListTagsForResource` reports for ONE resource, over the parts of a tag set that
//! a byte-oriented or family-oriented store gets wrong.
//!
//! The rule under test (the same one [`crate::resource_tags`] pins over the resource
//! KINDS): `ListTagsForResource` is a read that answers 200 with the tag list stored on
//! the single resource its `resourceArn` names, pair for pair, so
//!
//! * the key and value text comes back unchanged whatever Unicode it is written in —
//!   `Tag$key`/`Tag$value` accept any `\p{L}`, `\p{N}` or `\p{Z}` character
//!   (`smithy_query(query="shape", shape="Tag")`) and their maxima are counted in
//!   "Unicode characters in UTF-8" (the `Tag` doc), not in bytes;
//! * a tag whose value is the empty string is a tag the resource CARRIES, so it is
//!   reported with that empty value, and that is a different answer from a resource that
//!   carries no tags at all (`Tags` has `min: 0`, so the empty list is a legal answer in
//!   its own right);
//! * the resource a tag set belongs to is the one record the ARN names, so two task
//!   definition REVISIONS of one family each report their own list.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION};
use crate::task_lifecycle::container;

/// A container image, as a container definition names one.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The `{key, value}` pairs a reported tag list carries, in the order the response holds
/// them, sorted so two readers of one tag set compare equal whatever order their maps
/// happened to yield.
fn pairs(tags: &[Value], case: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|tag| {
            let key = tag["key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag with no key: {tag}"));
            let value = tag["value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag with no value: {tag}"));
            (key.to_string(), value.to_string())
        })
        .collect();
    pairs.sort();
    pairs
}

/// The pairs a `tags` request list carries, in the same sorted form as [`pairs`].
fn expected(tags: &Value, case: &str) -> Vec<(String, String)> {
    pairs(
        tags.as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the row's tags are a list, got {tags}")),
        case,
    )
}

/// The tags `ListTagsForResource` reports for one ARN, as a sorted pair list.
async fn listed(
    router: &MultiServiceRouter,
    resource_arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let body = ok(
        router,
        "ListTagsForResource",
        json!({ "resourceArn": resource_arn }),
        case,
    )
    .await;
    pairs(&list(&body, "tags", case), case)
}

/// Register one revision of `family` carrying `tags` and return its ARN.
async fn register_tagged(
    router: &MultiServiceRouter,
    family: &str,
    tags: &Value,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container("app", IMAGE)],
            "tags": tags,
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Create a cluster carrying `tags` and return its ARN.
async fn create_tagged_cluster(
    router: &MultiServiceRouter,
    name: &str,
    tags: &Value,
    case: &str,
) -> String {
    let body = ok(
        router,
        "CreateCluster",
        json!({ "clusterName": name, "tags": tags }),
        case,
    )
    .await;
    string(&body["cluster"], "clusterArn", case)
}

#[tokio::test]
async fn tag_text_round_trips_through_every_unicode_script_the_pattern_allows() {
    let router = service();

    // The rows differ along the SCRIPT and character class the key and value are written
    // in. Each one is made only of characters `Tag$key`/`Tag$value` admit — the pattern
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` from
    // `smithy_query(query="shape", shape="Tag")`: any Unicode letter, any Unicode number,
    // any Unicode separator, and that punctuation set. None of them is ASCII-only, so a
    // store that truncated, re-encoded or byte-indexed the text would not return them.
    let rows: Vec<(&str, String, String)> = vec![
        ("ASCII, for the baseline", "env".into(), "prod".into()),
        (
            "Latin with diacritics",
            "propriétaire".into(),
            "équipe-à".into(),
        ),
        (
            "German sharp s and umlauts",
            "größe".into(),
            "straße".into(),
        ),
        ("Han ideographs", "环境".into(), "生产".into()),
        (
            "Japanese kana and kanji",
            "所有者".into(),
            "プラットフォーム".into(),
        ),
        ("Korean hangul", "소유자".into(), "플랫폼".into()),
        ("Cyrillic", "владелец".into(), "команда".into()),
        ("Greek", "περιβάλλον".into(), "παραγωγή".into()),
        ("Hebrew, written right to left", "בעלים".into(), "צוות".into()),
        ("Arabic, written right to left", "المالك".into(), "فريق".into()),
        // `\p{L}` is any Unicode LETTER. A combining mark (Devanagari's matras, Thai's
        // tone marks) is `\p{M}`, which the pattern does NOT admit, so these rows stay on
        // the letters of each script.
        (
            "Devanagari letters",
            "कमल".into(),
            "सरल".into(),
        ),
        (
            "Thai, no word separators",
            "ไทย".into(),
            "ทดสอบ".into(),
        ),
        // `\p{N}` is any Unicode number, not just the ASCII digits.
        ("Non-ASCII digits", "cost٤٢".into(), "budget১২৩".into()),
        // `\p{Z}` is any Unicode separator; the ideographic space is one.
        (
            "An ideographic space separator",
            "team\u{3000}name".into(),
            "platform\u{3000}a".into(),
        ),
        // A multi-byte key and value well inside the documented maxima of 128 and 256
        // CHARACTERS, at 3 bytes per character here: a bound counted in bytes, or text
        // sliced by byte index, would mangle or reject them.
        (
            "A long Han key and value, 60 and 200 characters",
            "鍵".repeat(60),
            "値".repeat(200),
        ),
    ];

    for (index, (case, key, value)) in rows.iter().enumerate() {
        // One cluster per row, so each row reads its own record rather than a list some
        // earlier row wrote.
        let name = format!("unicode{index}");
        let tags = json!([{ "key": key, "value": value }]);
        let arn = create_tagged_cluster(&router, &name, &tags, case).await;

        let want = expected(&tags, case);
        let got = listed(&router, &arn, case).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected ListTagsForResource({arn}) to report {want:?}, got \
             {got:?}"
        );

        // The same text, byte for byte: a comparison of pairs alone would pass on a
        // normalised or re-encoded copy, so assert the key and value themselves.
        let (got_key, got_value) = got
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected exactly one tag, got {got:?}"));
        assert_eq!(
            got_key.as_bytes(),
            key.as_bytes(),
            "case {case:?}: expected the key {key:?} unchanged, got {got_key:?}"
        );
        assert_eq!(
            got_value.as_bytes(),
            value.as_bytes(),
            "case {case:?}: expected the value {value:?} unchanged, got {got_value:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_tag_value_is_a_tag_and_no_tags_is_an_empty_list() {
    let router = service();

    // The rows differ along WHAT the create was given for `tags`: nothing at all, the
    // empty list, a single tag whose value is the empty string ("The OPTIONAL part of a
    // key-value pair", `Tag$value` with `min: 0`), and a mix of empty and non-empty
    // values — crossed with the two resource kinds whose create takes a tag list and
    // whose record is reached by a different store key (a cluster by name, a task
    // definition by family and revision). `expected_count` is how many tags the resource
    // then carries, which is what separates "carries a tag with no value" from "carries
    // no tags".
    let rows: Vec<(&str, Value, usize)> = vec![
        ("tags omitted from the request", Value::Null, 0),
        ("an empty tag list", json!([]), 0),
        (
            "one tag whose value is the empty string",
            json!([{ "key": "novalue", "value": "" }]),
            1,
        ),
        (
            "two tags, one empty value and one not",
            json!([
                { "key": "novalue", "value": "" },
                { "key": "env", "value": "prod" },
            ]),
            2,
        ),
        (
            "three tags whose values are all empty",
            json!([
                { "key": "a", "value": "" },
                { "key": "b", "value": "" },
                { "key": "c", "value": "" },
            ]),
            3,
        ),
    ];

    for (index, (case, tags, expected_count)) in rows.iter().enumerate() {
        let want = match tags {
            Value::Null => Vec::new(),
            tags => expected(tags, case),
        };
        assert_eq!(
            want.len(),
            *expected_count,
            "case {case:?}: the row says {expected_count} tags but lists {want:?}"
        );

        // A cluster, created either with the member or without it.
        let cluster_name = format!("empties{index}");
        let cluster_case = format!("{case} (cluster)");
        let request = match tags {
            Value::Null => json!({ "clusterName": cluster_name }),
            tags => json!({ "clusterName": cluster_name, "tags": tags }),
        };
        let body = ok(&router, "CreateCluster", request, &cluster_case).await;
        let cluster_arn = string(&body["cluster"], "clusterArn", &cluster_case);
        let got = listed(&router, &cluster_arn, &cluster_case).await;
        assert_eq!(
            got, want,
            "case {cluster_case:?}: expected ListTagsForResource({cluster_arn}) to report \
             {want:?}, got {got:?}"
        );
        assert_eq!(
            got.len(),
            *expected_count,
            "case {cluster_case:?}: expected {expected_count} tag(s), got {got:?}"
        );

        // A task-definition revision, the same way.
        let definition_case = format!("{case} (task definition)");
        let family = format!("empties{index}");
        let request = match tags {
            Value::Null => json!({
                "family": family,
                "containerDefinitions": [container("app", IMAGE)],
            }),
            tags => json!({
                "family": family,
                "containerDefinitions": [container("app", IMAGE)],
                "tags": tags,
            }),
        };
        let body = ok(&router, "RegisterTaskDefinition", request, &definition_case).await;
        let definition_arn = string(&body["taskDefinition"], "taskDefinitionArn", &definition_case);
        let got = listed(&router, &definition_arn, &definition_case).await;
        assert_eq!(
            got, want,
            "case {definition_case:?}: expected ListTagsForResource({definition_arn}) to \
             report {want:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn each_task_definition_revision_reports_its_own_tag_set() {
    let router = service();

    // One family, four revisions, each registered with a DIFFERENT tag set (including
    // none): the revisions share a family but are separate records, so the ARN a reader
    // names decides which list it gets. A store that kept one tag set per family, or that
    // let the newest registration win, would report the same list for all four.
    const FAMILY: &str = "revisioned";
    let per_revision: Vec<(&str, Value)> = vec![
        ("revision 1, one tag", json!([{ "key": "rev", "value": "one" }])),
        (
            "revision 2, two tags",
            json!([
                { "key": "rev", "value": "two" },
                { "key": "env", "value": "test" },
            ]),
        ),
        ("revision 3, no tags", json!([])),
        (
            "revision 4, a tag whose value is empty",
            json!([{ "key": "rev", "value": "" }]),
        ),
    ];

    let mut registered: Vec<(&str, String, Value)> = Vec::new();
    for (case, tags) in &per_revision {
        let arn = register_tagged(&router, FAMILY, tags, case).await;
        registered.push((case, arn, tags.clone()));
    }

    // Every ARN is distinct — one record per revision.
    for (left_index, (left_case, left_arn, _)) in registered.iter().enumerate() {
        for (right_case, right_arn, _) in registered.iter().skip(left_index + 1) {
            assert_ne!(
                left_arn, right_arn,
                "case {left_case:?} vs {right_case:?}: each revision has its own ARN, got \
                 {left_arn} twice"
            );
        }
    }

    // Each revision reports the list IT was registered with, and reports it again on a
    // second read (a read does not change what a later caller sees).
    for (case, arn, tags) in &registered {
        let want = expected(tags, case);
        for attempt in 1..=2 {
            let got = listed(&router, arn, case).await;
            assert_eq!(
                got, want,
                "case {case:?}: read {attempt} of ListTagsForResource({arn}) must report \
                 {want:?}, got {got:?}"
            );
        }
    }

    // A revision of this family that was never registered is not a record, so it is
    // refused rather than answered with another revision's tags. `ClientException` is the
    // refusal this service already answers for a task definition it does not hold.
    let ghost = format!(
        "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{FAMILY}:{}",
        registered.len() + 1
    );
    let case = "a revision of an existing family that was never registered";
    let (code, message) = refusal(
        &router,
        "ListTagsForResource",
        json!({ "resourceArn": ghost }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected ClientException for {ghost}, got {code} ({message:?})"
    );
}
