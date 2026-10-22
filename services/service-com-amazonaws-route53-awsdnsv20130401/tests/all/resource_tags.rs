// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Route 53 tagging slice: `ChangeTagsForResource`
//! and the `ListTagsForResource` that reads its writes back.
//!
//! The rule under test is that a resource's tag set is a MAP from tag key to
//! tag value: `ChangeTagsForResource` removes every key in `RemoveTagKeys` and
//! then upserts every `Tag` in `AddTags`, the whole batch or none of it, and
//! `ListTagsForResource` reports exactly what the last batch left — bounded by
//! the ten tags a resource may carry. So the tables below differ along what
//! that rule generalises over: the number of tags in a batch, the value and
//! length of a key and a value, whether a key is new, edited, removed or both
//! added and removed, and whether the batch fits the bound — never merely along
//! which zone is tagged.
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` uses, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{
    call, create_zone_ok, escape_xml, router, unescape_xml, Reply,
};

// ---------------------------------------------------------------------------
// Request builders and response readers
// ---------------------------------------------------------------------------

/// A `ChangeTagsForResource` body. An empty list is OMITTED rather than sent as
/// an empty element, because `TagList`/`TagKeyList` are `len 1..10`.
fn change_tags_body(add: &[(&str, &str)], remove: &[&str]) -> String {
    let add_block = if add.is_empty() {
        String::new()
    } else {
        let tags: String = add
            .iter()
            .map(|(key, value)| {
                format!(
                    "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                    escape_xml(key),
                    escape_xml(value)
                )
            })
            .collect();
        format!("<AddTags>{tags}</AddTags>")
    };
    let remove_block = if remove.is_empty() {
        String::new()
    } else {
        let keys: String = remove
            .iter()
            .map(|key| format!("<Key>{}</Key>", escape_xml(key)))
            .collect();
        format!("<RemoveTagKeys>{keys}</RemoveTagKeys>")
    };
    format!(
        "<ChangeTagsForResourceRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         {add_block}{remove_block}</ChangeTagsForResourceRequest>"
    )
}

/// Apply one tag batch to a resource.
async fn change_tags(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_id: &str,
    add: &[(&str, &str)],
    remove: &[&str],
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/tags/{resource_type}/{resource_id}"),
        &change_tags_body(add, remove),
    )
    .await
}

/// Read a resource's tags back.
async fn list_tags(router: &MultiServiceRouter, resource_type: &str, resource_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/tags/{resource_type}/{resource_id}"),
        "",
    )
    .await
}

/// The text of `<name>…</name>` inside one `<Tag>` element, or `None` when the
/// element is absent. `<name/>` and `<name></name>` both read as the EMPTY
/// string.
///
/// Parsed from the raw body rather than read out of `Reply::doc`, because the
/// shared flattener drops an element carrying no text and a tag value may
/// legitimately be the empty string.
///
/// `pub(crate)` so a sibling suite in this test target reads an element that
/// may be empty through the SAME reader rather than growing a second copy.
pub(crate) fn element_text(block: &str, name: &str) -> Option<String> {
    if block.contains(&format!("<{name}/>")) {
        return Some(String::new());
    }
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(unescape_xml(&block[start..end]))
}

/// Whether a reply carries a `Tags` container at all, empty or not. `<Tags/>`,
/// `<Tags></Tags>` and a container with children all count.
fn carries_tags_container(reply: &Reply) -> bool {
    reply.body.contains("<Tags/>") || reply.body.contains("<Tags>")
}

/// The `(Key, Value)` pairs of every `<Tag>` in a reply, in document order.
fn tag_pairs(reply: &Reply) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut rest = reply.body.as_str();
    while let Some(start) = rest.find("<Tag>") {
        let block_start = start + "<Tag>".len();
        let end = rest[block_start..]
            .find("</Tag>")
            .map(|offset| block_start + offset)
            .unwrap_or_else(|| panic!("unterminated <Tag> element in {}", reply.body));
        let block = &rest[block_start..end];
        let key = element_text(block, "Key")
            .unwrap_or_else(|| panic!("a <Tag> must carry a <Key>; body {}", reply.body));
        let value = element_text(block, "Value")
            .unwrap_or_else(|| panic!("a <Tag> must carry a <Value>; body {}", reply.body));
        pairs.push((key, value));
        rest = &rest[end + "</Tag>".len()..];
    }
    pairs
}

/// The tag keys a resource carries, in the order they are reported.
fn tag_keys(reply: &Reply) -> Vec<String> {
    tag_pairs(reply).into_iter().map(|(key, _)| key).collect()
}

/// `expected` as owned pairs, for comparison against [`tag_pairs`].
fn owned(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// ChangeTagsForResource / ListTagsForResource
// ---------------------------------------------------------------------------

/// One row of the tag-batch table: the batch to apply, and the WHOLE tag set
/// the resource must carry once it has been applied.
#[derive(Debug, Clone, Copy)]
struct BatchRow {
    case: &'static str,
    add: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    /// Every tag the resource carries afterwards, in reported (tag-key) order.
    expected: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn tag_batches_add_edit_and_delete_tags_and_read_back() {
    // The rule: a resource's tags are a map from key to value; one batch
    // removes `RemoveTagKeys` and then upserts `AddTags`, and the next read
    // reports exactly that map. Each row applies to the SAME zone, so the row
    // also proves the previous batch was persisted rather than echoed.
    let rows = [
        BatchRow {
            case: "one tag is added",
            add: &[("Owner", "dns-team")],
            remove: &[],
            expected: &[("Owner", "dns-team")],
        },
        BatchRow {
            case: "several tags are added in one batch",
            add: &[("Env", "prod"), ("Cost-Center", "1234"), ("Team", "sre")],
            remove: &[],
            expected: &[
                ("Cost-Center", "1234"),
                ("Env", "prod"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            case: "adding a key the resource already carries edits its value",
            add: &[("Env", "staging")],
            remove: &[],
            expected: &[
                ("Cost-Center", "1234"),
                ("Env", "staging"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            // `TagValue` is `len 0..256`, so the empty value is the low bound.
            case: "a tag value may be empty",
            add: &[("Audit", "")],
            remove: &[],
            expected: &[
                ("Audit", ""),
                ("Cost-Center", "1234"),
                ("Env", "staging"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            case: "a tag value may carry XML special characters",
            add: &[("Note", "a&b<c>d\"e'f")],
            remove: &[],
            expected: &[
                ("Audit", ""),
                ("Cost-Center", "1234"),
                ("Env", "staging"),
                ("Note", "a&b<c>d\"e'f"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            case: "a named key is removed",
            add: &[],
            remove: &["Cost-Center"],
            expected: &[
                ("Audit", ""),
                ("Env", "staging"),
                ("Note", "a&b<c>d\"e'f"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            case: "removing a key the resource does not carry changes nothing",
            add: &[],
            remove: &["Absent-Key"],
            expected: &[
                ("Audit", ""),
                ("Env", "staging"),
                ("Note", "a&b<c>d\"e'f"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            case: "a key in both lists keeps the added value",
            add: &[("Env", "dev")],
            remove: &["Env", "Note"],
            expected: &[
                ("Audit", ""),
                ("Env", "dev"),
                ("Owner", "dns-team"),
                ("Team", "sre"),
            ],
        },
        BatchRow {
            case: "every tag can be removed, leaving no tag set",
            add: &[],
            remove: &["Audit", "Env", "Owner", "Team"],
            expected: &[],
        },
    ];

    let router = router();
    let zone = create_zone_ok(&router, "tagged.example.com", "tags-round-trip").await;

    for row in rows {
        let changed = change_tags(&router, "hostedzone", &zone, row.add, row.remove).await;
        assert_eq!(
            changed.status, 200,
            "case {:?}: expected 200 from ChangeTagsForResource, got {} with body {}",
            row.case, changed.status, changed.body
        );

        let listed = list_tags(&router, "hostedzone", &zone).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {} with body {}",
            row.case, listed.status, listed.body
        );
        assert_eq!(
            tag_pairs(&listed),
            owned(row.expected),
            "case {:?}: the tag set read back must be the one the batch left; body {}",
            row.case,
            listed.body
        );
        assert_eq!(
            listed.first("ResourceTagSet/ResourceType"),
            Some("hostedzone"),
            "case {:?}: the tag set must name the resource type; body {}",
            row.case,
            listed.body
        );
        assert_eq!(
            listed.first("ResourceTagSet/ResourceId"),
            Some(zone.as_str()),
            "case {:?}: the tag set must name the resource; body {}",
            row.case,
            listed.body
        );
        // A tag read reports the `ResourceTagSet/Tags` container
        // unconditionally, so a resource that carries no tags answers with an
        // EMPTY container rather than with no element at all: a client walking
        // it reads an empty collection instead of a missing node. (`TagList`'s
        // `@length min 1` bounds what a REQUEST may send.)
        assert!(
            carries_tags_container(&listed),
            "case {:?}: the tag set must carry a Tags container; body {}",
            row.case,
            listed.body
        );
        if row.expected.is_empty() {
            assert!(
                !listed.body.contains("<Tag>"),
                "case {:?}: an untagged resource's Tags container must be empty; body {}",
                row.case,
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn an_untagged_resource_reports_an_empty_tags_container() {
    // The rule, on the two ways a resource comes to carry no tags: it was never
    // tagged, and it was tagged and then emptied. Both report the container.
    let router = router();
    let never_tagged = create_zone_ok(&router, "never.example.com", "never-tagged").await;
    let emptied = create_zone_ok(&router, "emptied.example.com", "emptied-tags").await;

    let changed = change_tags(&router, "hostedzone", &emptied, &[("Owner", "dns-team")], &[]).await;
    assert_eq!(
        changed.status, 200,
        "tagging {emptied}: expected 200, got {} with body {}",
        changed.status, changed.body
    );
    let changed = change_tags(&router, "hostedzone", &emptied, &[], &["Owner"]).await;
    assert_eq!(
        changed.status, 200,
        "untagging {emptied}: expected 200, got {} with body {}",
        changed.status, changed.body
    );

    for (case, zone) in [
        ("a zone that was never tagged", &never_tagged),
        ("a zone whose every tag was removed", &emptied),
    ] {
        let listed = list_tags(&router, "hostedzone", zone).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {} with body {}",
            listed.status, listed.body
        );
        assert!(
            carries_tags_container(&listed),
            "case {case:?}: an untagged resource must still carry a Tags container; body {}",
            listed.body
        );
        assert_eq!(
            tag_pairs(&listed),
            Vec::<(String, String)>::new(),
            "case {case:?}: the container must be empty; body {}",
            listed.body
        );
    }
}

/// One row of the length-bound table.
#[derive(Debug, Clone)]
struct LengthRow {
    case: &'static str,
    key: String,
    value: String,
    /// The HTTP status the request must answer with.
    status: u16,
}

#[tokio::test]
async fn tag_key_and_value_lengths_are_bounded_by_the_model() {
    // The bounds come from the model, not from this test: `TagKey` is
    // `len 0..128` and `TagValue` is `len 0..256`
    // (smithy_query(query="shape",shape="TagKey,TagValue")). Each bound is
    // exercised AT the bound and one character past it.
    let rows = [
        LengthRow {
            case: "a key at the 128-character maximum",
            key: "k".repeat(128),
            value: "v".to_string(),
            status: 200,
        },
        LengthRow {
            case: "a key one character past the maximum",
            key: "k".repeat(129),
            value: "v".to_string(),
            status: 400,
        },
        LengthRow {
            case: "a value at the 256-character maximum",
            key: "at-value-bound".to_string(),
            value: "v".repeat(256),
            status: 200,
        },
        LengthRow {
            case: "a value one character past the maximum",
            key: "past-value-bound".to_string(),
            value: "v".repeat(257),
            status: 400,
        },
    ];

    let router = router();
    let zone = create_zone_ok(&router, "bounds.example.com", "tag-length-bounds").await;

    for row in &rows {
        let changed = change_tags(
            &router,
            "hostedzone",
            &zone,
            &[(row.key.as_str(), row.value.as_str())],
            &[],
        )
        .await;
        assert_eq!(
            changed.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, changed.status, changed.body
        );

        let listed = list_tags(&router, "hostedzone", &zone).await;
        let carried = tag_pairs(&listed).contains(&(row.key.clone(), row.value.clone()));
        assert_eq!(
            carried,
            row.status == 200,
            "case {:?}: a tag is carried only if its batch was accepted; body {}",
            row.case,
            listed.body
        );
    }
}

/// One row of the tag-count table: a batch over a resource that already carries
/// tags, and what the resource holds once the batch has been answered.
#[derive(Debug, Clone, Copy)]
struct CountRow {
    case: &'static str,
    add: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    status: u16,
    /// The tag keys the resource carries afterwards, in reported order.
    expected_keys: &'static [&'static str],
}

#[tokio::test]
async fn a_resource_carries_at_most_the_documented_ten_tags() {
    // The rule: a resource's tag set never exceeds ten tags, and a batch that
    // would overshoot is refused WHOLE, leaving the set as it was.
    // `ChangeTagsForResource$AddTags`: "You can add a maximum of 10 tags to a
    // health check or a hosted zone", and `ResourceTagSet$Tags` is `len 1..10`.
    const TEN: &[(&str, &str)] = &[
        ("k0", "0"),
        ("k1", "1"),
        ("k2", "2"),
        ("k3", "3"),
        ("k4", "4"),
        ("k5", "5"),
        ("k6", "6"),
        ("k7", "7"),
        ("k8", "8"),
        ("k9", "9"),
    ];

    let rows = [
        CountRow {
            case: "ten tags fit",
            add: TEN,
            remove: &[],
            status: 200,
            expected_keys: &["k0", "k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"],
        },
        CountRow {
            case: "an eleventh tag is refused",
            add: &[("k10", "10")],
            remove: &[],
            status: 400,
            expected_keys: &["k0", "k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"],
        },
        CountRow {
            case: "removing one tag makes room for one",
            add: &[("k10", "10")],
            remove: &["k0"],
            status: 200,
            expected_keys: &["k1", "k10", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"],
        },
        CountRow {
            case: "a batch that would still overshoot is refused whole",
            add: &[("k11", "11"), ("k12", "12")],
            remove: &["k1"],
            status: 400,
            expected_keys: &["k1", "k10", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"],
        },
    ];

    let router = router();
    let zone = create_zone_ok(&router, "bounded.example.com", "tag-count-bound").await;

    for row in rows {
        let changed = change_tags(&router, "hostedzone", &zone, row.add, row.remove).await;
        assert_eq!(
            changed.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, changed.status, changed.body
        );
        if row.status == 400 {
            assert_eq!(
                changed.error_code(),
                "InvalidInput",
                "case {:?}: a batch past the tag maximum is InvalidInput; body {}",
                row.case,
                changed.body
            );
        }

        let listed = list_tags(&router, "hostedzone", &zone).await;
        assert_eq!(
            tag_keys(&listed),
            row.expected_keys
                .iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: a refused batch must leave the tag set untouched; body {}",
            row.case,
            listed.body
        );
    }
}

/// One row of the refusal table.
struct RefusalRow {
    case: &'static str,
    method: &'static str,
    /// The request URI, with `{zone}` standing for the existing zone's id.
    uri: &'static str,
    body: String,
    status: u16,
    /// The expected wire error code, or `""` to assert only the status.
    code: &'static str,
}

#[tokio::test]
async fn a_tag_request_against_an_unknown_resource_is_refused() {
    // The rule: a tag request resolves its resource first and reports the
    // not-found error of the resource TYPE it names, and a batch that asks for
    // no change at all is invalid input.
    let rows = [
        RefusalRow {
            case: "changing tags on an unknown hosted zone",
            method: "POST",
            uri: "/2013-04-01/tags/hostedzone/ZNOSUCHZONE0001",
            body: change_tags_body(&[("Owner", "dns-team")], &[]),
            status: 404,
            code: "NoSuchHostedZone",
        },
        RefusalRow {
            case: "listing tags of an unknown hosted zone",
            method: "GET",
            uri: "/2013-04-01/tags/hostedzone/ZNOSUCHZONE0001",
            body: String::new(),
            status: 404,
            code: "NoSuchHostedZone",
        },
        RefusalRow {
            // No health check can exist: `CreateHealthCheck` is still a
            // scaffold, so the store holds none.
            case: "changing tags on a health check",
            method: "POST",
            uri: "/2013-04-01/tags/healthcheck/8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c",
            body: change_tags_body(&[("Owner", "dns-team")], &[]),
            status: 404,
            code: "NoSuchHealthCheck",
        },
        RefusalRow {
            case: "listing tags of a health check",
            method: "GET",
            uri: "/2013-04-01/tags/healthcheck/8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c",
            body: String::new(),
            status: 404,
            code: "NoSuchHealthCheck",
        },
        RefusalRow {
            case: "a batch that names neither list asks for no change",
            method: "POST",
            uri: "/2013-04-01/tags/hostedzone/{zone}",
            body: change_tags_body(&[], &[]),
            status: 400,
            code: "InvalidInput",
        },
        RefusalRow {
            // `TagList` is `len 1..10`, so an empty `AddTags` is outside the
            // model's own bound; the boundary validator owns the wording.
            case: "a batch carrying an empty AddTags list",
            method: "POST",
            uri: "/2013-04-01/tags/hostedzone/{zone}",
            body: "<ChangeTagsForResourceRequest \
                   xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\"><AddTags></AddTags>\
                   </ChangeTagsForResourceRequest>"
                .to_string(),
            status: 400,
            code: "",
        },
    ];

    let router = router();
    let zone = create_zone_ok(&router, "refusals.example.com", "tag-refusals").await;

    for row in &rows {
        let uri = row.uri.replace("{zone}", &zone);
        let reply = call(&router, row.method, &uri, &row.body).await;
        assert_eq!(
            reply.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, reply.status, reply.body
        );
        if !row.code.is_empty() {
            assert_eq!(
                reply.error_code(),
                row.code,
                "case {:?}: expected error code {}, got body {}",
                row.case,
                row.code,
                reply.body
            );
        }
    }

    // None of those refusals tagged the existing zone.
    let listed = list_tags(&router, "hostedzone", &zone).await;
    assert_eq!(
        tag_pairs(&listed),
        Vec::<(String, String)>::new(),
        "a refused request must not tag the zone; body {}",
        listed.body
    );
}

#[tokio::test]
async fn a_tag_set_belongs_to_one_resource_and_dies_with_it() {
    // The rule: tags are held BY the resource — tagging one resource leaves
    // every other one alone, and a deleted resource takes its tags with it.
    let router = router();
    let first = create_zone_ok(&router, "first.example.com", "tag-owner-first").await;
    let second = create_zone_ok(&router, "second.example.com", "tag-owner-second").await;

    for (zone, tag) in [(&first, ("Owner", "first")), (&second, ("Owner", "second"))] {
        let changed = change_tags(&router, "hostedzone", zone, &[tag], &[]).await;
        assert_eq!(
            changed.status, 200,
            "tagging {zone}: expected 200, got {} with body {}",
            changed.status, changed.body
        );
    }

    for (zone, expected) in [(&first, "first"), (&second, "second")] {
        let listed = list_tags(&router, "hostedzone", zone).await;
        assert_eq!(
            tag_pairs(&listed),
            owned(&[("Owner", expected)]),
            "zone {zone} must carry only its own tags; body {}",
            listed.body
        );
    }

    // A freshly created zone holds only its default SOA and NS records, so it
    // is deletable — and its tags go with it.
    let deleted = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{first}"),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "deleting {first}: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    let gone = list_tags(&router, "hostedzone", &first).await;
    assert_eq!(
        gone.status, 404,
        "a deleted zone has no tag set: expected 404, got {} with body {}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error_code(),
        "NoSuchHostedZone",
        "a deleted zone's tags report NoSuchHostedZone; body {}",
        gone.body
    );

    // The surviving zone is untouched by the delete.
    let survivor = list_tags(&router, "hostedzone", &second).await;
    assert_eq!(
        tag_pairs(&survivor),
        owned(&[("Owner", "second")]),
        "the surviving zone keeps its tags; body {}",
        survivor.body
    );
}
