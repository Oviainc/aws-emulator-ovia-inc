// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` — removing tags from a CloudFront resource, end to end over HTTP.
//!
//! The rule under test: an untagging call REMOVES from the resource its ARN identifies
//! exactly the tags whose keys the request names — every key the request does not name is
//! left exactly as it was, a key the resource does not carry removes nothing (so the call
//! is idempotent in the set it leaves behind), and the resulting set is what a tag read
//! reports — for every resource kind the store holds and tags; an ARN the store holds no
//! resource for is `NoSuchResource` (404), a request naming a key no tag key can be is
//! refused (`ValidationException` 400) and leaves the resource carrying exactly the tags
//! it carried before, and a tag set is associated with the resource rather than part of
//! its configuration, so untagging mints no new version token.
//!
//! Every row below differs along what that rule generalises over: how many keys the call
//! names (none at all, an empty list, one, several, every carried key, enough to empty a
//! tag set at the published maximum), how many tags the resource carries, whether the
//! named keys are carried at all, whether the removed tag named a `Value` and whether
//! that value was empty, which characters the key uses, its length, its letter case, the
//! order the keys are named in, whether a key is named twice, and which kind of resource
//! the ARN names.
//!
//! The harness (the router, the request helpers, the element scanner, the configuration
//! table and the tag helpers) is the lifecycle, `distribution_tags` and `tag_resource`
//! suites', reused rather than copied.

use std::collections::BTreeMap;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, delete_distribution, emulator, get_distribution, past_propagation, Answer, Config,
    ACCOUNT_ID, API_VERSION,
};
use crate::distribution_tags::{
    config_for, create_distribution_with_tags, list_tags_for_resource, reported_order,
    reported_tags, tag, ReportedTags, SubmittedTag,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, managed_certificate_request, multi_tenant_distribution,
};
use crate::tag_resource::{create_key_value_store, tag_resource};

// ---------------------------------------------------------------------------
// Calling the untagging surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/tagging?Operation=Untag&Resource=<arn>`, whose payload is a
/// `TagKeys`.
///
/// The ARN travels in the query exactly as the reader's and the tagging call's do: every
/// character a CloudFront ARN uses is legal in a query value, and none of them is `&`,
/// `=` or `%`.
///
/// `pub(crate)` so the sibling suite that asserts the resource-TYPE rule over all three
/// tagging operations drives the same wire path rather than growing a copy of this.
pub(crate) async fn untag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    named: Option<&[String]>,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/tagging?Operation=Untag&Resource={arn}"),
        Some(&tag_keys_xml(named)),
        None,
    )
    .await
}

/// The `TagKeys` element a request carries. `TagKeyList`'s member is named `Key`.
///
/// `None` is a request that names no `Items` at all (`<TagKeys/>`), which is a different
/// document from one carrying an empty `Items` list — both name no key, and the rule has
/// to hold for each.
pub(crate) fn tag_keys_xml(named: Option<&[String]>) -> String {
    let Some(items) = named else {
        return "<TagKeys/>".to_string();
    };
    let mut out = String::from("<TagKeys><Items>");
    for key in items {
        out.push_str(&format!("<Key>{key}</Key>"));
    }
    out.push_str("</Items></TagKeys>");
    out
}

/// The tag set `arn` reports, read back through `ListTagsForResource`.
async fn tags_of(router: &MultiServiceRouter, arn: &str, case: &str) -> ReportedTags {
    let answer = list_tags_for_resource(router, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListTagsForResource on {arn} to answer 200, got {} ({})",
        answer.status, answer.raw
    );
    reported_tags(answer.root())
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One removal: the tag set the resource is created with, and the keys an
/// `UntagResource` call then names.
#[derive(Debug, Clone)]
struct RemovalCase {
    case: &'static str,
    /// The resource's tag set at create time. `None` creates it with no `Items` at all.
    carried: Vec<SubmittedTag>,
    /// The keys the `UntagResource` call names. `None` names no `Items` at all.
    named: Option<Vec<String>>,
}

impl RemovalCase {
    /// The tag set the resource carries before the removal.
    fn before(&self) -> ReportedTags {
        self.carried.clone().into_iter().collect()
    }

    /// The tag set a read must report after the removal: what the resource carried,
    /// without the keys the call named.
    fn expected(&self) -> ReportedTags {
        let mut expected = self.before();
        for key in self.named.clone().unwrap_or_default() {
            expected.remove(&key);
        }
        expected
    }

    fn named(&self) -> Option<&[String]> {
        self.named.as_deref()
    }
}

/// `count` distinct tags whose keys are prefixed with `prefix`.
fn generated(prefix: &str, count: usize) -> Vec<SubmittedTag> {
    (0..count)
        .map(|index| tag(&format!("{prefix}-{index:03}"), Some(&format!("value-{index}"))))
        .collect()
}

/// The keys of `tags`, as a request names them.
fn keys_of(tags: &[SubmittedTag]) -> Vec<String> {
    tags.iter().map(|(key, _)| key.clone()).collect()
}

fn named(keys: &[&str]) -> Option<Vec<String>> {
    Some(keys.iter().map(|key| (*key).to_string()).collect())
}

/// The key the special-character and maximum-length rows use, built from what `TagKey`
/// documents: "valid characters include `a-z`, `A-Z`, `0-9`, space, and the special
/// characters `_ - . : / = + @`", with a length "between 1 and 128 characters" (its
/// minimum and its maximum are each a row below, and one past the maximum is a row of
/// [`keys_no_tag_key_can_be`]).
const SPECIAL_KEY: &str = "cost.center:unit/01=a+b-c@d e";

/// The message `NoSuchResource` carries: the wording this error class answers with,
/// which names no part of the request.
///
/// Written out here rather than imported from the crate, so the SENTENCE is what every
/// suite asserting this refusal is held to; `pub(crate)` so there is one such reading.
pub(crate) const NO_SUCH_RESOURCE_MESSAGE: &str =
    "The specified resource doesn't exist. Update your request and try again.";

/// The table every removal assertion runs over.
fn removal_table() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "no-items-at-all-changes-nothing",
            carried: vec![tag("environment", Some("production"))],
            named: None,
        },
        RemovalCase {
            case: "empty-items-changes-nothing",
            carried: vec![tag("environment", Some("production"))],
            named: Some(Vec::new()),
        },
        RemovalCase {
            case: "one-carried-key-of-several",
            carried: vec![
                tag("environment", Some("production")),
                tag("keep", Some("yes")),
            ],
            named: named(&["environment"]),
        },
        RemovalCase {
            case: "the-only-carried-key",
            carried: vec![tag("environment", Some("production"))],
            named: named(&["environment"]),
        },
        RemovalCase {
            case: "a-key-the-resource-does-not-carry",
            carried: vec![tag("keep", Some("yes"))],
            named: named(&["never-carried"]),
        },
        RemovalCase {
            case: "a-key-of-an-untagged-resource",
            carried: Vec::new(),
            named: named(&["environment"]),
        },
        RemovalCase {
            case: "a-key-whose-value-was-left-out",
            carried: vec![tag("team", None), tag("keep", Some("yes"))],
            named: named(&["team"]),
        },
        RemovalCase {
            case: "a-key-whose-value-is-empty",
            carried: vec![tag("owner", Some("")), tag("keep", Some("yes"))],
            named: named(&["owner"]),
        },
        RemovalCase {
            case: "every-carried-key",
            carried: vec![
                tag("a", Some("1")),
                tag("b", None),
                tag("c", Some("")),
            ],
            named: named(&["a", "b", "c"]),
        },
        RemovalCase {
            case: "some-carried-some-not",
            carried: vec![
                tag("a", Some("1")),
                tag("b", Some("2")),
                tag("c", Some("3")),
            ],
            named: named(&["b", "absent", "also-absent"]),
        },
        RemovalCase {
            case: "special-characters",
            carried: vec![
                tag(SPECIAL_KEY, Some("a b_c-d.e:f/g=h+i@j")),
                tag("keep", Some("yes")),
            ],
            named: Some(vec![SPECIAL_KEY.to_string()]),
        },
        RemovalCase {
            case: "unicode-letters",
            carried: vec![tag("proyecto", Some("producción")), tag("keep", Some("yes"))],
            named: named(&["proyecto"]),
        },
        RemovalCase {
            case: "key-at-minimum-length",
            carried: vec![tag("k", Some("v")), tag("keep", Some("yes"))],
            named: named(&["k"]),
        },
        RemovalCase {
            case: "key-at-maximum-length",
            carried: vec![
                tag(&"k".repeat(128), Some("v")),
                tag("keep", Some("yes")),
            ],
            named: Some(vec!["k".repeat(128)]),
        },
        RemovalCase {
            // A tag key is case sensitive, so a key differing only in case names a
            // DIFFERENT tag and the other one stays.
            case: "a-key-differing-only-in-case",
            carried: vec![tag("Env", Some("upper")), tag("env", Some("lower"))],
            named: named(&["env"]),
        },
        RemovalCase {
            // Named in descending key order, so a row cannot be passing by removing in
            // the order the request happens to list.
            case: "keys-named-in-descending-order",
            carried: vec![
                tag("app", Some("checkout")),
                tag("mode", None),
                tag("zone", Some("eu")),
            ],
            named: named(&["zone", "mode"]),
        },
        RemovalCase {
            // Removal is idempotent, so a key named twice names the SAME removal.
            case: "a-key-named-twice",
            carried: vec![tag("stage", Some("alpha")), tag("keep", Some("yes"))],
            named: named(&["stage", "stage"]),
        },
        RemovalCase {
            case: "empties-a-tag-set-at-the-maximum-count",
            carried: generated("carried", MAX_TAGS_PER_RESOURCE),
            named: Some(keys_of(&generated("carried", MAX_TAGS_PER_RESOURCE))),
        },
        RemovalCase {
            case: "half-of-a-tag-set-at-the-maximum-count",
            carried: generated("carried", MAX_TAGS_PER_RESOURCE),
            named: Some(
                keys_of(&generated("carried", MAX_TAGS_PER_RESOURCE))
                    .into_iter()
                    .step_by(2)
                    .collect(),
            ),
        },
    ]
}

/// A key no tag key can be, as an `UntagResource` call names it: `(case, key, status,
/// error code)`.
///
/// Both rows are constraints the model states for `TagKey` — a length "between 1 and 128
/// characters" — so each is one PAST a published bound, and the refusal is the one the
/// framework answers for a modelled constraint.
fn keys_no_tag_key_can_be() -> Vec<(&'static str, String, u16, &'static str)> {
    vec![
        (
            "key-one-past-its-maximum-length",
            "k".repeat(129),
            400,
            "ValidationException",
        ),
        (
            "key-one-below-its-minimum-length",
            String::new(),
            400,
            "ValidationException",
        ),
    ]
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// `UntagResource` removes exactly the tags whose keys it names from the set the
/// distribution carries, and a tag read reports the result: the named keys are gone,
/// every other carried key is untouched, and the call is idempotent.
#[tokio::test]
async fn untag_resource_removes_the_keys_it_names() {
    let router = emulator();

    for (index, row) in removal_table().into_iter().enumerate() {
        let case = row.case;
        let config = config_for(index, &format!("ref-remove-{index}"));
        let created = create_distribution_with_tags(&router, &config, Some(&row.carried)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        let before = tags_of(&router, &arn, case).await;
        assert_eq!(
            before,
            row.before(),
            "case {case}: expected the create to leave {:?}, got {before:?}",
            row.before()
        );

        let untagged = untag_resource(&router, &arn, row.named()).await;
        assert_eq!(
            untagged.status, 204,
            "case {case}: expected UntagResource to answer 204, got {} ({})",
            untagged.status, untagged.raw
        );
        assert!(
            untagged.raw.trim().is_empty(),
            "case {case}: expected UntagResource to answer no body, got {:?}",
            untagged.raw
        );

        // The resulting set, read back through the reader.
        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after,
            row.expected(),
            "case {case}: expected the tag set {:?}, got {after:?}",
            row.expected()
        );
        // The same rule, stated key by key rather than as one map: no named key is
        // carried any more, and every carried key the call did not name is exactly as it
        // was.
        for key in row.named.clone().unwrap_or_default() {
            assert_eq!(
                after.get(&key),
                None,
                "case {case}: expected the named key {key:?} to be gone, got {:?}",
                after.get(&key)
            );
        }
        for (key, value) in &before {
            if row
                .named
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|removed| removed == key)
            {
                continue;
            }
            assert_eq!(
                after.get(key),
                Some(value),
                "case {case}: expected the untouched key {key:?} to still carry {value:?}, got {:?}",
                after.get(key)
            );
        }
        // A tag set is keyed by tag key, so what is left is reported in key order
        // whatever order the removal named.
        let read = list_tags_for_resource(&router, &arn).await;
        let order = reported_order(read.root());
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(
            order, sorted,
            "case {case}: expected the remaining tags in key order, got {order:?}"
        );

        // Removing the same keys again removes nothing: the call is idempotent in the set
        // it leaves behind.
        let again = untag_resource(&router, &arn, row.named()).await;
        assert_eq!(
            again.status, 204,
            "case {case}: expected the repeated UntagResource to answer 204, got {} ({})",
            again.status, again.raw
        );
        let repeated = tags_of(&router, &arn, case).await;
        assert_eq!(
            repeated, after,
            "case {case}: expected the repeat to leave {after:?}, got {repeated:?}"
        );
    }
}

/// An untagging call is about the ARN, not about one kind of resource: a key value store
/// this emulator holds takes the same removal, reported by the same reader.
#[tokio::test]
async fn untag_resource_resolves_every_resource_kind_the_store_holds() {
    let router = emulator();

    for (index, row) in removal_table().into_iter().enumerate() {
        let case = row.case;
        let name = format!("store-{index:02}");
        let created = create_key_value_store(&router, &name, Some(&row.carried)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the key value store create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/")),
            "case {case}: expected a key value store ARN, got {arn:?}"
        );

        let untagged = untag_resource(&router, &arn, row.named()).await;
        assert_eq!(
            untagged.status, 204,
            "case {case}: expected UntagResource to answer 204 for {arn}, got {} ({})",
            untagged.status, untagged.raw
        );
        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after,
            row.expected(),
            "case {case}: expected {arn} to report {:?}, got {after:?}",
            row.expected()
        );
    }
}

/// A distribution tenant is a resource kind this service associates tags with too, so an
/// add and a removal reach its stored set through the same ARN.
#[tokio::test]
async fn untag_resource_reaches_a_distribution_tenant() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-untag-tenant").await;
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId><Name>untag-tenant</Name>\
         <Domains><member><Domain>untag.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>acme</Value></member></Parameters>\
         <Tags><Items>\
         <Tag><Key>stage</Key><Value>prod</Value></Tag>\
         <Tag><Key>keep</Key><Value>yes</Value></Tag>\
         </Items></Tags>{}\
         </CreateDistributionTenantRequest>",
        // The domain claimed above is a name in a customer's zone, so the create asks
        // CloudFront to validate it, exactly as the tenant suite's creates do.
        managed_certificate_request()
    );
    let created = create_distribution_tenant(&router, &body).await;
    assert_eq!(
        created.status, 201,
        "case tenant: expected the tenant create to answer 201, got {} ({})",
        created.status, created.raw
    );
    // A distribution tenant reports its ARN as `Arn`.
    let arn = created.root().text_at("Arn").unwrap_or_default().to_string();
    assert!(
        arn.starts_with(&format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution-tenant/")),
        "case tenant: expected a distribution tenant ARN, got {arn:?}"
    );

    let created_tags = tags_of(&router, &arn, "tenant").await;
    let expected_created: ReportedTags = vec![
        tag("keep", Some("yes")),
        tag("stage", Some("prod")),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        created_tags, expected_created,
        "case tenant: expected the create to leave {expected_created:?}, got {created_tags:?}"
    );

    // The add and the removal go through the ARN, exactly as they do for every other kind.
    let tagged = tag_resource(&router, &arn, Some(&[tag("added", Some("later"))])).await;
    assert_eq!(
        tagged.status, 204,
        "case tenant: expected TagResource to answer 204, got {} ({})",
        tagged.status, tagged.raw
    );
    let untagged = untag_resource(&router, &arn, named(&["stage", "added"]).as_deref()).await;
    assert_eq!(
        untagged.status, 204,
        "case tenant: expected UntagResource to answer 204, got {} ({})",
        untagged.status, untagged.raw
    );
    let after = tags_of(&router, &arn, "tenant").await;
    let expected: ReportedTags = vec![tag("keep", Some("yes"))].into_iter().collect();
    assert_eq!(
        after, expected,
        "case tenant: expected {expected:?}, got {after:?}"
    );

    // Removing the same keys again removes nothing.
    let again = untag_resource(&router, &arn, named(&["stage", "added"]).as_deref()).await;
    assert_eq!(
        again.status, 204,
        "case tenant: expected the repeated UntagResource to answer 204, got {} ({})",
        again.status, again.raw
    );
    let repeated = tags_of(&router, &arn, "tenant").await;
    assert_eq!(
        repeated, expected,
        "case tenant: expected the repeat to leave {expected:?}, got {repeated:?}"
    );
}

/// Adding and removing are inverses over the SAME stored set: a tag added by
/// `TagResource` is removed by naming its key, and what the resource carried before the
/// add is back.
#[tokio::test]
async fn untagging_inverts_the_tags_a_tagging_call_added() {
    let router = emulator();

    for (index, row) in removal_table().into_iter().enumerate() {
        let case = row.case;
        // Two tags neither this row's carried set nor its named keys mention, so the
        // removal below is exactly the inverse of this add.
        let added = vec![
            tag("added-first", Some("one")),
            tag("added-second", None),
        ];
        // A tag set already at the published maximum takes no further key — that bound is
        // the ADD's rule, asserted in the tagging suite — so a row with no room for the
        // two keys above has no add to invert here.
        if row.carried.len() + added.len() > MAX_TAGS_PER_RESOURCE {
            continue;
        }
        let config = config_for(index, &format!("ref-inverse-{index}"));
        let created = create_distribution_with_tags(&router, &config, Some(&row.carried)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        let before = tags_of(&router, &arn, case).await;

        let tagged = tag_resource(&router, &arn, Some(&added)).await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: expected TagResource to answer 204, got {} ({})",
            tagged.status, tagged.raw
        );
        let with_added = tags_of(&router, &arn, case).await;
        let mut expected_with_added = before.clone();
        for (key, value) in &added {
            expected_with_added.insert(key.clone(), value.clone());
        }
        assert_eq!(
            with_added, expected_with_added,
            "case {case}: expected the add to leave {expected_with_added:?}, got {with_added:?}"
        );

        let removed = untag_resource(&router, &arn, Some(&keys_of(&added))).await;
        assert_eq!(
            removed.status, 204,
            "case {case}: expected UntagResource to answer 204, got {} ({})",
            removed.status, removed.raw
        );
        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after, before,
            "case {case}: expected the removal to restore {before:?}, got {after:?}"
        );
    }
}

/// A key no tag key can be is refused, and the resource keeps exactly the tags it carried
/// before the refused call.
#[tokio::test]
async fn a_key_no_tag_key_can_be_is_refused_and_changes_nothing() {
    let router = emulator();

    for (index, (case, key, status, code)) in keys_no_tag_key_can_be().into_iter().enumerate() {
        let config = config_for(index, &format!("ref-refused-{index}"));
        let carried = vec![tag("keep", Some("yes")), tag("environment", Some("test"))];
        let created = create_distribution_with_tags(&router, &config, Some(&carried)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        let before = tags_of(&router, &arn, case).await;

        // The refused key travels beside a key the resource DOES carry, so a refusal that
        // removed anything would show up in the read below.
        let refused = untag_resource(
            &router,
            &arn,
            Some(&[key.clone(), "environment".to_string()]),
        )
        .await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected {status}, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {:?}",
            refused.error_code()
        );

        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused call to leave {before:?}, got {after:?}"
        );
    }
}

/// An ARN this emulator holds no resource for is `NoSuchResource`, whichever kind of
/// CloudFront resource it names — before the resource exists, for a resource kind the
/// store holds none of, and after the resource is deleted — and such a call persists
/// nothing.
#[tokio::test]
async fn untag_resource_refuses_an_arn_no_resource_carries() {
    let router = emulator();

    // Every row is a well-formed CloudFront ARN (`ResourceARN`'s pattern is satisfied)
    // naming a kind of resource this service associates tags with, so what makes each a
    // refusal is that the store holds no resource carrying it.
    let unknown = [
        (
            "distribution-that-was-never-created",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E00000000000A"),
        ),
        (
            "key-value-store-that-was-never-created",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/absent-store"),
        ),
        (
            "connection-group-that-was-never-created",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:connection-group/absent-group"),
        ),
        (
            // Another partition: the same resource part, a different ARN.
            "another-partition",
            format!("arn:aws-cn:cloudfront::{ACCOUNT_ID}:distribution/E00000000000A"),
        ),
    ];

    for (case, arn) in &unknown {
        let refused = untag_resource(&router, arn, named(&["environment"]).as_deref()).await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected 404, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchResource",
            "case {case}: expected NoSuchResource, got {:?}",
            refused.error_code()
        );
        // The message this error class carries, which names no part of the request.
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_RESOURCE_MESSAGE,
            "case {case}: expected the message {NO_SUCH_RESOURCE_MESSAGE:?}, got {:?}",
            refused.root().find_text("Message")
        );
        // Nothing was persisted under that ARN: the read refuses it too.
        let read = list_tags_for_resource(&router, arn).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected the ARN to still name no resource, got {} ({})",
            read.status, read.raw
        );
    }

    // A tag set lives on the resource record, so a deleted resource's ARN names nothing
    // again — including for an untagging call.
    let disabled = Config {
        enabled: false,
        ..config_for(0, "ref-deleted")
    };
    let created =
        create_distribution_with_tags(&router, &disabled, Some(&[tag("keep", Some("yes"))])).await;
    assert_eq!(
        created.status, 201,
        "case deleted: expected the create to answer 201, got {} ({})",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    // The distribution was created disabled, and a delete lands only once that
    // configuration has also finished propagating, so the clock is given its span first.
    tokio::time::sleep(past_propagation()).await;
    let deleted = delete_distribution(&router, &id, Some(&etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case deleted: expected the delete to answer 204, got {} ({})",
        deleted.status, deleted.raw
    );
    let refused = untag_resource(&router, &arn, named(&["keep"]).as_deref()).await;
    assert_eq!(
        refused.status, 404,
        "case deleted: expected 404, got {} ({})",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "NoSuchResource",
        "case deleted: expected NoSuchResource, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.root().find_text("Message").unwrap_or_default(),
        NO_SUCH_RESOURCE_MESSAGE,
        "case deleted: expected the message {NO_SUCH_RESOURCE_MESSAGE:?}, got {:?}",
        refused.root().find_text("Message")
    );
}

/// A tag set is associated with the resource rather than part of its configuration:
/// untagging one resource mints no new version token, changes nothing a configuration
/// read reports, and leaves every OTHER resource's tag set alone.
#[tokio::test]
async fn untagging_touches_only_the_untagged_resource() {
    let router = emulator();

    // Three distributions, each created with its own tag set, plus a key value store, so
    // an untagging call has neighbours of both kinds to leave alone. Every distribution
    // carries the SAME key beside its own, so a removal that reached a neighbour would
    // show up below.
    let mut distributions: Vec<(String, String, String, ReportedTags)> = Vec::new();
    for index in 0..3 {
        let config = config_for(index, &format!("ref-neighbour-{index}"));
        let carried = vec![
            tag(&format!("owner-{index}"), Some("original")),
            tag("shared", Some("everywhere")),
        ];
        let created = create_distribution_with_tags(&router, &config, Some(&carried)).await;
        assert_eq!(
            created.status, 201,
            "case neighbour-{index}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        distributions.push((
            created.root().text_at("Id").unwrap_or_default().to_string(),
            created.root().text_at("ARN").unwrap_or_default().to_string(),
            created.header("etag").to_string(),
            carried.into_iter().collect::<BTreeMap<_, _>>(),
        ));
    }
    let store = create_key_value_store(
        &router,
        "neighbour-store",
        Some(&[tag("shared", Some("everywhere")), tag("kind", Some("kvs"))]),
    )
    .await;
    assert_eq!(
        store.status, 201,
        "case neighbour-store: expected the create to answer 201, got {} ({})",
        store.status, store.raw
    );
    let store_arn = store.root().text_at("ARN").unwrap_or_default().to_string();
    let store_tags_before = tags_of(&router, &store_arn, "neighbour-store").await;

    // Untag the middle one only.
    let (id, arn, etag, carried) = distributions[1].clone();
    let untagged = untag_resource(&router, &arn, named(&["shared"]).as_deref()).await;
    assert_eq!(
        untagged.status, 204,
        "case untag-the-middle: expected 204, got {} ({})",
        untagged.status, untagged.raw
    );

    // Its own configuration is untouched, and so is its version token: a tag set is not
    // part of the configuration a version names.
    let read = get_distribution(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case untag-the-middle: expected GetDistribution to answer 200, got {} ({})",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case untag-the-middle: expected the version token {etag} to be unchanged, got {:?}",
        read.header("etag")
    );
    let config = config_for(1, "ref-neighbour-1");
    let read_config = read
        .root()
        .at("DistributionConfig")
        .expect("case untag-the-middle: GetDistribution answered no config");
    config.assert_matches(read_config, "get after untagging");

    let mut expected = carried.clone();
    expected.remove("shared");
    let after = tags_of(&router, &arn, "untag-the-middle").await;
    assert_eq!(
        after, expected,
        "case untag-the-middle: expected {expected:?}, got {after:?}"
    );

    // Every neighbour still reports exactly its own tag set, the shared key included.
    for (index, (_, other_arn, _, other_tags)) in distributions.iter().enumerate() {
        if index == 1 {
            continue;
        }
        let neighbour = tags_of(&router, other_arn, "neighbour").await;
        assert_eq!(
            &neighbour, other_tags,
            "case neighbour-{index}: expected {other_arn} to still report {other_tags:?}, got {neighbour:?}"
        );
    }
    let store_tags = tags_of(&router, &store_arn, "neighbour-store").await;
    assert_eq!(
        store_tags, store_tags_before,
        "case neighbour-store: expected {store_arn} to still report {store_tags_before:?}, got {store_tags:?}"
    );
}
