// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tagging slice: `TagResource`, `UntagResource` and
//! `ListTagsForResource`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    MAX_TAGS_PER_RESOURCE, RSLVR_CODE_RESOURCE_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator_with_outposts, error_type, expect_error, ok, rack_outpost, string_member,
    Emulator, ACCOUNT, REGION,
};

/// Every resource of this service a tag operation addresses, named by the
/// resource-type segment of its ARN.
///
/// `TagResource$ResourceArn`'s doc names the Get and List commands an ARN comes
/// from, so the resources whose records carry an `Arn` are the taggable ones;
/// these four are the ones this emulator can create. A row per kind is what
/// keeps the rule from being asserted on one resource only.
const RESOURCE_KINDS: [&str; 4] = [
    "resolver-endpoint",
    "resolver-rule",
    "outpost-resolver",
    "firewall-domain-list",
];

/// Create one resource of `kind` (with `tags` on the create request, when the
/// create takes any) and answer the `Arn` it reports.
async fn create_resource(
    router: &Emulator,
    kind: &str,
    creator: &str,
    tags: Option<Value>,
) -> String {
    let (operation, member, mut body) = match kind {
        "resolver-endpoint" => (
            "CreateResolverEndpoint",
            "ResolverEndpoint",
            {
                // `SubnetId` is length 1..32, so the subnets are named from the
                // first characters of the creator id. They are subnets of the
                // caller's VPC, so the account's own Amazon EC2 store holds them
                // before the endpoint is placed in them.
                let short: String = creator.chars().take(20).collect();
                let subnets = [format!("sn-{short}-a"), format!("sn-{short}-b")];
                router.hold_subnets(&subnets);
                json!({
                    "CreatorRequestId": creator,
                    "Direction": "OUTBOUND",
                    "SecurityGroupIds": ["sg-00000000000000001"],
                    "IpAddresses": [
                        { "SubnetId": subnets[0], "Ip": "10.0.0.10" },
                        { "SubnetId": subnets[1], "Ip": "10.0.1.10" },
                    ],
                })
            },
        ),
        "resolver-rule" => (
            "CreateResolverRule",
            "ResolverRule",
            json!({
                "CreatorRequestId": creator,
                "Name": creator,
                "RuleType": "SYSTEM",
                "DomainName": format!("{creator}.example.com"),
            }),
        ),
        "outpost-resolver" => (
            "CreateOutpostResolver",
            "OutpostResolver",
            json!({
                "CreatorRequestId": creator,
                "Name": creator,
                "PreferredInstanceType": "m5.large",
                // A Resolver on an Outpost sits on an Outpost the account has,
                // so one is racked for it through the Outposts API and its own
                // minted `OutpostArn` is what this create names.
                "OutpostArn": rack_outpost(router, REGION, creator).await,
            }),
        ),
        "firewall-domain-list" => (
            "CreateFirewallDomainList",
            "FirewallDomainList",
            json!({ "CreatorRequestId": creator, "Name": creator }),
        ),
        other => panic!("case {other:?}: this service has no such taggable resource"),
    };
    if let Some(tags) = tags {
        body.as_object_mut()
            .expect("the create body is a JSON object")
            .insert("Tags".to_string(), tags);
    }

    let created = ok(router, operation, body).await;
    let resource = created
        .get(member)
        .unwrap_or_else(|| panic!("case {kind:?}: {operation} must answer a {member}, got {created}"));
    let arn = string_member(resource, "Arn");
    assert!(
        arn.starts_with(&format!(
            "arn:aws:route53resolver:{REGION}:{ACCOUNT}:{kind}/"
        )),
        "case {kind:?}: the created resource must report a {kind} ARN rooted in the signed region, got {arn:?}"
    );
    arn
}

/// A `Tags` list, as the wire carries one.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// A `TagResource` request body.
fn tag_request(resource_arn: &str, tags: &[(String, String)]) -> Value {
    json!({ "ResourceArn": resource_arn, "Tags": tag_list(tags) })
}

/// An `UntagResource` request body.
fn untag_request(resource_arn: &str, keys: &[String]) -> Value {
    json!({ "ResourceArn": resource_arn, "TagKeys": keys })
}

/// One page of `ListTagsForResource`, as `(tags, next token)`.
async fn tag_page(
    router: &MultiServiceRouter,
    resource_arn: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (Vec<(String, String)>, Option<String>) {
    let mut body = json!({ "ResourceArn": resource_arn });
    let object = body.as_object_mut().expect("the body is a JSON object");
    if let Some(max_results) = max_results {
        object.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        object.insert("NextToken".to_string(), json!(next_token));
    }

    let page = ok(router, "ListTagsForResource", body).await;
    let tags = page
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListTagsForResource must report a Tags list, got {page}"))
        .iter()
        .map(|tag| (string_member(tag, "Key"), string_member(tag, "Value")))
        .collect();
    let next = page
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (tags, next)
}

/// Every tag of a resource, read through as many `ListTagsForResource` pages as
/// the cursor hands out.
async fn all_tags(
    router: &MultiServiceRouter,
    resource_arn: &str,
    max_results: Option<i32>,
) -> Vec<(String, String)> {
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let (tags, token) = tag_page(router, resource_arn, max_results, next.as_deref()).await;
        collected.extend(tags);
        match token {
            Some(token) => next = Some(token),
            None => return collected,
        }
    }
}

/// A `(key, value)` pair, so a table row reads as the wire form does.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One write of the lifecycle table: what the call hands over, and the whole tag
/// set the resource must report afterwards.
enum Write {
    Tag(Vec<(String, String)>),
    Untag(Vec<String>),
}

struct Row {
    case: &'static str,
    write: Write,
    expected: Vec<(String, String)>,
}

/// The lifecycle table, walked against a resource that starts with no tags.
///
/// The rows differ along what the rule generalises over: how many tags one call
/// carries, whether a key is new or already held, the form of the value (ASCII,
/// the empty string `TagValue`'s length 0..256 admits, non-ASCII text, the
/// 256-character maximum), the form of the key (`TagKey` is length 1..128, so a
/// 128-character key is tested too), one key named twice in a single call, a
/// removal of a key the resource does not carry, a repeated removal and an empty
/// removal.
fn lifecycle_rows() -> Vec<Row> {
    // The published maxima of `TagKey` (length 1..128) and `TagValue`
    // (length 0..256).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    vec![
        Row {
            case: "one tag onto a resource that carries none",
            write: Write::Tag(vec![pair("team", "net")]),
            expected: vec![pair("team", "net")],
        },
        Row {
            case: "two more keys at once, one with the empty value",
            write: Write::Tag(vec![pair("env", "prod"), pair("note", "")]),
            expected: vec![pair("team", "net"), pair("env", "prod"), pair("note", "")],
        },
        Row {
            case: "a key the resource already carries takes the new value",
            write: Write::Tag(vec![pair("team", "dns")]),
            expected: vec![pair("team", "dns"), pair("env", "prod"), pair("note", "")],
        },
        Row {
            case: "a value of non-ASCII text",
            write: Write::Tag(vec![pair("propriétaire", "équipe-réseau-αβγ-🌐")]),
            expected: vec![
                pair("team", "dns"),
                pair("env", "prod"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
            ],
        },
        Row {
            case: "the longest key and the longest value the model allows",
            write: Write::Tag(vec![pair(&longest_key, &longest_value)]),
            expected: vec![
                pair("team", "dns"),
                pair("env", "prod"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
                pair(&longest_key, &longest_value),
            ],
        },
        Row {
            case: "one key named twice in a single call keeps the last value",
            write: Write::Tag(vec![pair("stage", "one"), pair("stage", "two")]),
            expected: vec![
                pair("team", "dns"),
                pair("env", "prod"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
                pair(&longest_key, &longest_value),
                pair("stage", "two"),
            ],
        },
        Row {
            case: "a removal naming a held key and a key the resource never had",
            write: Write::Untag(vec!["env".to_string(), "absent".to_string()]),
            expected: vec![
                pair("team", "dns"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
                pair(&longest_key, &longest_value),
                pair("stage", "two"),
            ],
        },
        Row {
            case: "the same removal again, which removes nothing",
            write: Write::Untag(vec!["env".to_string(), "absent".to_string()]),
            expected: vec![
                pair("team", "dns"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
                pair(&longest_key, &longest_value),
                pair("stage", "two"),
            ],
        },
        Row {
            case: "an empty removal",
            write: Write::Untag(Vec::new()),
            expected: vec![
                pair("team", "dns"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
                pair(&longest_key, &longest_value),
                pair("stage", "two"),
            ],
        },
        Row {
            case: "an empty tag list, which changes nothing",
            write: Write::Tag(Vec::new()),
            expected: vec![
                pair("team", "dns"),
                pair("note", ""),
                pair("propriétaire", "équipe-réseau-αβγ-🌐"),
                pair(&longest_key, &longest_value),
                pair("stage", "two"),
            ],
        },
        Row {
            case: "every remaining key removed",
            write: Write::Untag(vec![
                "team".to_string(),
                "note".to_string(),
                "propriétaire".to_string(),
                longest_key.clone(),
                "stage".to_string(),
            ]),
            expected: Vec::new(),
        },
    ]
}

/// A tag write is persisted onto the resource the ARN names, and
/// `ListTagsForResource` reports exactly the tags that were written: an upsert
/// keyed on the tag key, in the order the keys were first written, and a removal
/// that drops only the keys it names.
///
/// Asserted for every taggable resource of this service, so the rule is not
/// pinned to one resource kind.
#[tokio::test]
async fn a_tag_write_is_read_back_for_every_taggable_resource() {
    let router = emulator_with_outposts();

    for kind in RESOURCE_KINDS {
        let creator = format!("tag-lifecycle-{kind}");
        let arn = create_resource(&router, kind, &creator, None).await;
        assert_eq!(
            all_tags(&router, &arn, None).await,
            Vec::new(),
            "case {kind:?}: a resource created without Tags carries none, got its tags above"
        );

        for Row {
            case,
            write,
            expected,
        } in lifecycle_rows()
        {
            match write {
                Write::Tag(tags) => {
                    let answered = ok(&router, "TagResource", tag_request(&arn, &tags)).await;
                    assert_eq!(
                        answered,
                        json!({}),
                        "case {kind:?}/{case:?}: TagResource models no output member, got {answered}"
                    );
                }
                Write::Untag(keys) => {
                    let answered = ok(&router, "UntagResource", untag_request(&arn, &keys)).await;
                    assert_eq!(
                        answered,
                        json!({}),
                        "case {kind:?}/{case:?}: UntagResource models no output member, got {answered}"
                    );
                }
            }

            let reported = all_tags(&router, &arn, None).await;
            assert_eq!(
                reported, expected,
                "case {kind:?}/{case:?}: the resource must report the tags the writes left, got {reported:?}"
            );
            // Reading tags changes nothing a later caller sees.
            assert_eq!(
                all_tags(&router, &arn, None).await,
                reported,
                "case {kind:?}/{case:?}: a second read must report the same tags, got {reported:?}"
            );
        }
    }
}

/// The tags a create request carried are the very tags `ListTagsForResource`
/// reports, and a later `TagResource` merges into that same set: a resource's
/// tags live in ONE place, whichever operation wrote them.
#[tokio::test]
async fn tags_given_at_create_are_the_tags_the_tag_operations_see() {
    let router = emulator_with_outposts();

    for kind in RESOURCE_KINDS {
        let creator = format!("tag-at-create-{kind}");
        let created_with = vec![pair("owner", "netops"), pair("note", "")];
        let arn = create_resource(
            &router,
            kind,
            &creator,
            Some(tag_list(&created_with)),
        )
        .await;

        assert_eq!(
            all_tags(&router, &arn, None).await,
            created_with,
            "case {kind:?}: the tags the create carried must be reported back"
        );

        ok(
            &router,
            "TagResource",
            tag_request(&arn, &[pair("owner", "dnsops"), pair("env", "prod")]),
        )
        .await;
        assert_eq!(
            all_tags(&router, &arn, None).await,
            vec![pair("owner", "dnsops"), pair("note", ""), pair("env", "prod")],
            "case {kind:?}: a later write must update the create's key in place and append the new one"
        );

        ok(
            &router,
            "UntagResource",
            untag_request(&arn, &["note".to_string()]),
        )
        .await;
        assert_eq!(
            all_tags(&router, &arn, None).await,
            vec![pair("owner", "dnsops"), pair("env", "prod")],
            "case {kind:?}: a removal must drop the create's key too"
        );
    }
}

/// A tag write reaches the resource the ARN names and no other, even when the
/// two resources are of the same kind.
#[tokio::test]
async fn a_tag_write_reaches_only_the_resource_its_arn_names() {
    let router = emulator_with_outposts();
    let first = create_resource(&router, "firewall-domain-list", "tag-isolation-one", None).await;
    let second = create_resource(&router, "firewall-domain-list", "tag-isolation-two", None).await;
    let other_kind = create_resource(&router, "resolver-rule", "tag-isolation-rule", None).await;

    ok(
        &router,
        "TagResource",
        tag_request(&first, &[pair("team", "net")]),
    )
    .await;

    for (case, arn) in [("the same kind", &second), ("another kind", &other_kind)] {
        assert_eq!(
            all_tags(&router, arn, None).await,
            Vec::new(),
            "case {case:?}: tagging one resource must leave the others untagged"
        );
    }
    assert_eq!(
        all_tags(&router, &first, None).await,
        vec![pair("team", "net")],
        "case \"the tagged resource\": the write must have landed on the ARN it named"
    );

    // A removal is just as targeted.
    ok(
        &router,
        "TagResource",
        tag_request(&second, &[pair("team", "dns")]),
    )
    .await;
    ok(
        &router,
        "UntagResource",
        untag_request(&second, &["team".to_string()]),
    )
    .await;
    assert_eq!(
        all_tags(&router, &first, None).await,
        vec![pair("team", "net")],
        "case \"untagging the sibling\": the first resource must keep its tag"
    );
}

/// Every tag operation reports an ARN this account holds no resource under with
/// the `ResourceNotFoundException` each of them models — never a success that
/// persists nothing, and never a tag set it invented.
#[tokio::test]
async fn an_arn_no_resource_is_stored_under_is_reported_as_missing() {
    let router = emulator_with_outposts();
    let present = create_resource(&router, "firewall-domain-list", "tag-present", None).await;
    ok(
        &router,
        "TagResource",
        tag_request(&present, &[pair("team", "net")]),
    )
    .await;

    let truncated = present[..present.len() - 1].to_string();
    for (case, arn) in [
        (
            "a Resolver endpoint ARN nothing minted",
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-endpoint/rslvr-out-0123456789abcdef"),
        ),
        (
            "a Resolver rule ARN nothing minted",
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-rule/rslvr-rr-0123456789abcdef"),
        ),
        (
            "the stored ARN in another region",
            present.replace(REGION, "eu-west-1"),
        ),
        (
            "the stored ARN in another account",
            present.replace(ACCOUNT, "123456789012"),
        ),
        ("the stored ARN with its last character dropped", truncated),
        (
            "an ARN of another service",
            "arn:aws:s3:::amzn-s3-demo-bucket".to_string(),
        ),
        (
            "a resource id rather than an ARN",
            "rslvr-fdl-0123456789abcdef".to_string(),
        ),
        ("one character", "x".to_string()),
    ] {
        // The refusal states the service error code and names the resource by
        // the id its ARN carries.
        let expected_message = format!(
            "[{RSLVR_CODE_RESOURCE_NOT_FOUND}] Can’t find the resource with ID \"{}\".",
            arn.rsplit_once('/').map_or(arn.as_str(), |(_, id)| id)
        );
        for (operation, body) in [
            ("TagResource", tag_request(&arn, &[pair("team", "net")])),
            (
                "UntagResource",
                untag_request(&arn, &["team".to_string()]),
            ),
            ("ListTagsForResource", json!({ "ResourceArn": arn })),
        ] {
            let (status, answered) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must answer HTTP 400, got {status}: {answered}"
            );
            assert!(
                error_type(&answered).contains("ResourceNotFoundException"),
                "case {case:?}: {operation} must answer ResourceNotFoundException, got {answered}"
            );
            assert_eq!(
                string_member(&answered, "Message"),
                expected_message,
                "case {case:?}: {operation}'s refusal must state the code and the requested id, got {answered}"
            );
        }
    }

    // The refusals above are about the ARN, not about refusing every call.
    assert_eq!(
        all_tags(&router, &present, None).await,
        vec![pair("team", "net")],
        "case \"the stored ARN\": the resource that does exist must still report its tags"
    );
}

/// `ListTagsForResource` honours the `@paginated` contract over ONE resource's
/// tags: a page is cut at `MaxResults`, the cursor resumes where the page
/// stopped, and a cursor this query did not issue is refused with the
/// `InvalidNextTokenException` the operation models.
#[tokio::test]
async fn the_tag_page_is_cut_at_max_results_and_resumed_by_its_cursor() {
    let router = emulator_with_outposts();
    let arn = create_resource(&router, "resolver-endpoint", "tag-paging", None).await;
    let other = create_resource(&router, "resolver-endpoint", "tag-paging-other", None).await;

    let written: Vec<(String, String)> = (0..5)
        .map(|index| pair(&format!("key-{index}"), &format!("value-{index}")))
        .collect();
    ok(&router, "TagResource", tag_request(&arn, &written)).await;
    ok(&router, "TagResource", tag_request(&other, &written)).await;

    for page_size in [1, 2, 4, 5, 100] {
        assert_eq!(
            all_tags(&router, &arn, Some(page_size)).await,
            written,
            "case \"MaxResults {page_size}\": the pages together must report every tag once, in order"
        );
    }

    let (first_page, token) = tag_page(&router, &arn, Some(2), None).await;
    assert_eq!(
        first_page,
        written[..2].to_vec(),
        "case \"first page of two\": the page must carry the first two tags"
    );
    let token = token.expect("a resource with more tags than the page size owes a NextToken");

    // A cursor is an offset into ONE resource's tags, so the cursor minted for
    // this resource is not an offset into another's.
    expect_error(
        &router,
        "ListTagsForResource",
        json!({ "ResourceArn": other, "MaxResults": 2, "NextToken": token }),
        "a cursor minted for another resource",
        "InvalidNextTokenException",
    )
    .await;
    expect_error(
        &router,
        "ListTagsForResource",
        json!({ "ResourceArn": arn, "NextToken": "not-a-cursor-this-service-minted" }),
        "a cursor nothing minted",
        "InvalidNextTokenException",
    )
    .await;

    // The last page of a resource whose tags fit in one page owes no cursor.
    let (page, token) = tag_page(&router, &arn, Some(100), None).await;
    assert_eq!(
        page, written,
        "case \"one page\": a page as large as the tag set reports all of it"
    );
    assert_eq!(
        token, None,
        "case \"one page\": the last page owes no NextToken"
    );
}

/// A tag outside the model's constraints is refused at the boundary with HTTP
/// 400, and the resource's stored tags are left exactly as they were.
///
/// The rows walk the published bounds: `TagKey` is length 1..128 and `TagValue`
/// length 0..256, so each is tested one character past its maximum and at the
/// empty string.
#[tokio::test]
async fn a_tag_outside_the_modelled_constraints_is_refused() {
    let router = emulator_with_outposts();
    let arn = create_resource(&router, "firewall-domain-list", "tag-constraints", None).await;
    let held = vec![pair("team", "net")];
    ok(&router, "TagResource", tag_request(&arn, &held)).await;

    for (case, tags) in [
        (
            "a key one character past the 128-character maximum",
            vec![pair(&"k".repeat(129), "v")],
        ),
        ("an empty key", vec![pair("", "v")]),
        (
            "a value one character past the 256-character maximum",
            vec![pair("long", &"v".repeat(257))],
        ),
        (
            "a legal tag beside an oversized one",
            vec![pair("fine", "yes"), pair(&"k".repeat(200), "v")],
        ),
    ] {
        let (status, body) = call(&router, "TagResource", tag_request(&arn, &tags)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a tag outside the modelled constraints must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );
        assert_eq!(
            all_tags(&router, &arn, None).await,
            held,
            "case {case:?}: a refused write must persist nothing"
        );
    }

    // The bounds themselves are legal, so the refusals above are about the
    // constraint and not about refusing long tags outright.
    let at_the_bound = vec![pair(&"k".repeat(128), &"v".repeat(256)), pair("empty", "")];
    ok(&router, "TagResource", tag_request(&arn, &at_the_bound)).await;
    let mut expected = held.clone();
    expected.extend(at_the_bound);
    assert_eq!(
        all_tags(&router, &arn, None).await,
        expected,
        "case \"at the bounds\": a key of 128 and a value of 256 characters are legal"
    );
}

/// A resource holds at most as many tags as the model's `TagList` reports
/// (length 0..200): a write that would take it past that is refused with the
/// `LimitExceededException` `TagResource` models, the stored tags are untouched,
/// and a write that only updates keys already held is still served at the cap.
#[tokio::test]
async fn a_write_past_the_tag_ceiling_is_refused_and_persists_nothing() {
    let router = emulator_with_outposts();
    let arn = create_resource(&router, "firewall-domain-list", "tag-ceiling", None).await;

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| pair(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect();
    ok(&router, "TagResource", tag_request(&arn, &full)).await;
    assert_eq!(
        all_tags(&router, &arn, None).await,
        full,
        "case \"at the ceiling\": every tag up to the ceiling is stored"
    );

    for (case, tags) in [
        ("one key past the ceiling", vec![pair("one-too-many", "v")]),
        (
            "a held key beside a new one",
            vec![pair("key-000", "changed"), pair("one-too-many", "v")],
        ),
    ] {
        expect_error(
            &router,
            "TagResource",
            tag_request(&arn, &tags),
            case,
            "LimitExceededException",
        )
        .await;
        assert_eq!(
            all_tags(&router, &arn, None).await,
            full,
            "case {case:?}: a refused write must leave every stored tag as it was"
        );
    }

    // Updating keys the resource already holds adds none, so it is served.
    ok(
        &router,
        "TagResource",
        tag_request(&arn, &[pair("key-000", "changed")]),
    )
    .await;
    let mut expected = full.clone();
    expected[0] = pair("key-000", "changed");
    assert_eq!(
        all_tags(&router, &arn, None).await,
        expected,
        "case \"an update at the ceiling\": a write that adds no key is served"
    );

    // Freeing a slot makes room for a new key again.
    ok(
        &router,
        "UntagResource",
        untag_request(&arn, &["key-000".to_string()]),
    )
    .await;
    ok(
        &router,
        "TagResource",
        tag_request(&arn, &[pair("one-too-many", "v")]),
    )
    .await;
    let mut expected: Vec<(String, String)> = full[1..].to_vec();
    expected.push(pair("one-too-many", "v"));
    assert_eq!(
        all_tags(&router, &arn, None).await,
        expected,
        "case \"a freed slot\": a removal makes room for another key"
    );
}
