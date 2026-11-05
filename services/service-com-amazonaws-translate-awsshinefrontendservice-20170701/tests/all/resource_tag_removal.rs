// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UntagResource`: which of a resource's tags one
//! call removes, and what it leaves standing.
//!
//! Every test drives the service the way the router does, through the harness
//! `terminology_lifecycle` already builds (`router`, `call`, `error_code`,
//! `message`, `import`), so the wire protocol, the boundary constraint
//! validation and the handlers are all exercised, and what one request writes is
//! what the next request reads.
//!
//! The tests assert the RULE over a set of inputs rather than one recorded
//! request: the number of keys one call names (none, one, some, all, half of a
//! resource at the tag limit), keys the resource does not carry, a key
//! duplicated in the request, a key differing only in case, an order the
//! resource does not hold its tags in, and the same call repeated.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::terminology_lifecycle::{
    call, error_code, expected_arn, import, listed_tags, message, router, tag_list, tag_pairs,
    EN_FR_CSV,
};

/// The documented tag limit for one resource: "There can be a maximum of 50
/// tags (both existing and pending) associated with a specific resource"
/// (`smithy_query shape=TagResourceRequest member=Tags`).
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One row of the table: the tags the resource is left holding after the listed
/// keys are removed from the tags it was given.
struct Row {
    case: &'static str,
    held: Vec<(String, String)>,
    remove: Vec<String>,
    left: Vec<(String, String)>,
}

fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| (*key).to_string()).collect()
}

/// Import a terminology under `name` and leave it holding `held`.
async fn tagged_resource(
    router: &MultiServiceRouter,
    name: &str,
    held: &[(String, String)],
) -> String {
    let (status, imported) = import(router, name, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "{name}: ImportTerminology answered {imported}");
    let arn = expected_arn(name, EN_FR_CSV);
    let (status, answered) = call(
        router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tag_list(held)}),
    )
    .await;
    assert_eq!(status, 200, "{name}: TagResource answered {answered}");
    arn
}

fn rows() -> Vec<Row> {
    // A resource holding as many tags as it may hold, so a call can name half
    // of its keys and the other half must survive.
    let at_the_limit: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("k{index:02}"), format!("v{index}")))
        .collect();
    let half_the_keys: Vec<String> = at_the_limit
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, (key, _))| key.clone())
        .collect();
    let half_left: Vec<(String, String)> = at_the_limit
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, pair)| pair.clone())
        .collect();

    vec![
        Row {
            case: "no key named",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
            remove: Vec::new(),
            left: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        Row {
            case: "one of several keys",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
            remove: keys(&["owner"]),
            left: tag_pairs(&[("env", "prod"), ("cost", "42")]),
        },
        Row {
            case: "several keys at once",
            held: tag_pairs(&[
                ("env", "prod"),
                ("owner", "platform"),
                ("cost", "42"),
                ("team", "nlp"),
            ]),
            remove: keys(&["env", "cost", "team"]),
            left: tag_pairs(&[("owner", "platform")]),
        },
        Row {
            case: "every key the resource holds",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
            remove: keys(&["env", "owner"]),
            left: Vec::new(),
        },
        Row {
            case: "a key the resource does not carry",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
            remove: keys(&["absent"]),
            left: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        Row {
            case: "a held key beside one the resource does not carry",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
            remove: keys(&["owner", "absent"]),
            left: tag_pairs(&[("env", "prod")]),
        },
        Row {
            case: "a key differing from a held one only in case",
            held: tag_pairs(&[("Env", "prod"), ("owner", "platform")]),
            remove: keys(&["env", "OWNER"]),
            left: tag_pairs(&[("Env", "prod"), ("owner", "platform")]),
        },
        Row {
            case: "keys named in the reverse of the held order",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
            remove: keys(&["cost", "env"]),
            left: tag_pairs(&[("owner", "platform")]),
        },
        Row {
            case: "one key named twice in the same call",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
            remove: keys(&["env", "env"]),
            left: tag_pairs(&[("owner", "platform")]),
        },
        Row {
            case: "a key whose stored value is empty",
            held: tag_pairs(&[("env", ""), ("owner", "platform")]),
            remove: keys(&["env"]),
            left: tag_pairs(&[("owner", "platform")]),
        },
        Row {
            case: "half the keys of a resource at the tag limit",
            held: at_the_limit,
            remove: half_the_keys,
            left: half_left,
        },
    ]
}

/// One call removes exactly the tags whose key it names and leaves every other
/// tag the resource carries as it was, whatever the number of keys it names and
/// whether or not the resource carries them; naming a key twice, or naming one
/// the resource does not carry, removes no more and is not a refusal, so the
/// same call repeated leaves the same tag set.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    let router = router();

    for (index, row) in rows().into_iter().enumerate() {
        let name = format!("untag-{index:02}-terms");
        let arn = tagged_resource(&router, &name, &row.held).await;
        let expected: Vec<Value> = row
            .left
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();

        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"ResourceArn": arn, "TagKeys": Value::Array(
                row.remove.iter().map(|key| json!(key)).collect()
            )}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagResource answered {status} {answered}",
            row.case
        );
        assert_eq!(
            error_code(&answered),
            "<no __type>",
            "case {:?}: a removal names no error, got {answered}",
            row.case
        );

        let held = listed_tags(&router, row.case, &arn).await;
        assert_eq!(
            held, expected,
            "case {:?}: expected the resource to be left holding {expected:?}, got {held:?}",
            row.case
        );

        // The same call again: removal is addressed by key, so a key that is
        // already gone removes nothing more.
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"ResourceArn": arn, "TagKeys": Value::Array(
                row.remove.iter().map(|key| json!(key)).collect()
            )}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the repeated removal answered {status} {answered}",
            row.case
        );
        let held_again = listed_tags(&router, row.case, &arn).await;
        assert_eq!(
            held_again, expected,
            "case {:?}: the repeated removal must leave {expected:?}, got {held_again:?}",
            row.case
        );
    }
}

/// A removal addresses one resource: the ARN it names, not another resource
/// holding the same keys, and not the resource's own stored content.
#[tokio::test]
async fn untag_resource_touches_only_the_resource_its_arn_names() {
    let router = router();
    let shared = tag_pairs(&[("env", "prod"), ("owner", "platform")]);
    let first = tagged_resource(&router, "first-untag-terms", &shared).await;
    let second = tagged_resource(&router, "second-untag-terms", &shared).await;

    let (status, answered) = call(
        &router,
        "UntagResource",
        &json!({"ResourceArn": first, "TagKeys": ["env"]}),
    )
    .await;
    assert_eq!(status, 200, "UntagResource answered {status} {answered}");

    assert_eq!(
        listed_tags(&router, "the named resource", &first).await,
        vec![json!({"Key": "owner", "Value": "platform"})],
        "the named resource loses the named key"
    );
    assert_eq!(
        listed_tags(&router, "another resource holding the same keys", &second).await,
        vec![
            json!({"Key": "env", "Value": "prod"}),
            json!({"Key": "owner", "Value": "platform"}),
        ],
        "another resource holding the same keys keeps them"
    );

    // The resource itself is untouched: only its tags changed.
    let (status, fetched) = call(
        &router,
        "GetTerminology",
        &json!({"Name": "first-untag-terms"}),
    )
    .await;
    assert_eq!(status, 200, "GetTerminology answered {fetched}");
    assert_eq!(
        fetched
            .pointer("/TerminologyProperties/TermCount")
            .and_then(Value::as_i64),
        Some(2),
        "removing a tag must not change the stored terminology, got {fetched}"
    );
}

/// An ARN this account holds no resource for is a not-found refusal, never a
/// removal that quietly succeeds — whichever keys it names.
#[tokio::test]
async fn untag_resource_refuses_an_arn_naming_no_resource() {
    let router = router();
    // A stored resource exists, so the refusal is about the ARN rather than
    // about an empty store.
    let arn = tagged_resource(
        &router,
        "present-untag-terms",
        &tag_pairs(&[("env", "prod")]),
    )
    .await;

    let unknown = expected_arn("no-such-terms", b"no-such-file");
    let rows: [(&str, Value); 3] = [
        ("no key named", json!([])),
        ("one key", json!(["env"])),
        ("several keys", json!(["env", "owner", "cost"])),
    ];
    for (case, tag_keys) in rows {
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"ResourceArn": unknown, "TagKeys": tag_keys}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a not-found refusal, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: got {answered}"
        );
        assert_eq!(
            message(&answered),
            "The resource you are looking for has not been found. Review the resource \
             you're looking for and see if a different resource will accomplish your \
             needs before retrying the revised request.",
            "case {case:?}: got {answered}"
        );
    }

    assert_eq!(
        listed_tags(&router, "the resource that is there", &arn).await,
        vec![json!({"Key": "env", "Value": "prod"})],
        "a refused removal leaves every stored resource's tags as they were"
    );
}
