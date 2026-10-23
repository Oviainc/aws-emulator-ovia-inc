// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CloudTrail resource tagging (`RemoveTags` / `ListTags`).
//!
//! Every case goes over the wire through the suite's shared [`Emulator`] — the same
//! `MultiServiceRouter` the production router serves, under the dotted `X-Amz-Target`
//! prefix every AWS SDK sends — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `RemoveTags` drops exactly the KEYS the request names from the resource its
//!   `ResourceId` ARN names, whatever value each key was paired with, leaves every
//!   other key in place, and is `@idempotent`: a key the resource does not carry
//!   changes nothing;
//! * a `ResourceId` that is not one of the four taggable ARN forms is refused with
//!   the kind-specific invalid-ARN error the model declares, and a CloudTrail ARN
//!   naming a resource type that carries no tags with
//!   `ResourceTypeNotSupportedException`;
//! * a well-formed ARN naming no stored resource is refused with the kind-specific
//!   not-found error, HTTP 400, worded as CloudTrail words that kind;
//! * a tag list naming one key twice is refused with `InvalidTagParameterException`,
//!   as is a tag key or value outside its modelled length;
//! * `ListTags` reports the stored tag set of every ARN the request names, in the
//!   order it named them, and bounds one call at the documented 20 ARNs.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::Tag;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    self, EventDataStoreRecord, EventDataStoreState,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, so the rows differ in the Region their ARNs pin.
const OTHER_REGION: &str = "us-west-2";

/// The account id the emulator mints ARNs under (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The tagging requests these cases make, as methods on the shared emulator.
trait TaggingCalls {
    /// Create a trail carrying `tags` in `region`, and hand back its ARN.
    async fn trail_with_tags(&self, name: &str, region: &str, tags: &[Value]) -> String;
    /// Create a dashboard carrying `tags` in `region`, and hand back its ARN.
    async fn dashboard_with_tags(&self, name: &str, region: &str, tags: &[Value]) -> String;
    /// `RemoveTags` the tags in `tags` from `resource_id`.
    async fn remove_tags(&self, resource_id: &str, tags: &[Value]) -> Reply;
    /// The tag set `ListTags` reports for `resource_id`.
    async fn tags_of(&self, case: &str, resource_id: &str) -> Value;
}

impl TaggingCalls for Emulator {
    async fn trail_with_tags(&self, name: &str, region: &str, tags: &[Value]) -> String {
        let reply = self
            .call_in(
                "CreateTrail",
                region,
                &json!({
                    "Name": name,
                    "S3BucketName": "an-emulator-bucket",
                    "TagsList": tags,
                }),
            )
            .await;
        reply
            .expect_ok(&format!("create trail {name}"))
            .get("TrailARN")
            .and_then(Value::as_str)
            .expect("the create reports the trail ARN")
            .to_string()
    }

    async fn dashboard_with_tags(&self, name: &str, region: &str, tags: &[Value]) -> String {
        let reply = self
            .call_in(
                "CreateDashboard",
                region,
                &json!({"Name": name, "TagsList": tags}),
            )
            .await;
        reply
            .expect_ok(&format!("create dashboard {name}"))
            .get("DashboardArn")
            .and_then(Value::as_str)
            .expect("the create reports the dashboard ARN")
            .to_string()
    }

    async fn remove_tags(&self, resource_id: &str, tags: &[Value]) -> Reply {
        self.call(
            "RemoveTags",
            &json!({"ResourceId": resource_id, "TagsList": tags}),
        )
        .await
    }

    async fn tags_of(&self, case: &str, resource_id: &str) -> Value {
        let reply = self
            .call("ListTags", &json!({"ResourceIdList": [resource_id]}))
            .await;
        let body = reply.expect_ok(case);
        let entries = body
            .get("ResourceTagList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: ListTags reports no ResourceTagList"));
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected one entry for one ARN, got {entries:?}"
        );
        assert_eq!(
            entries[0].get("ResourceId").and_then(Value::as_str),
            Some(resource_id),
            "case {case}: expected the entry to name the ARN asked for, got {:?}",
            entries[0]
        );
        entries[0]
            .get("TagsList")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()))
    }
}

/// `TagKey`'s and `TagValue`'s modelled length maxima, from
/// `smithy_query(query="shape", shape="TagKey,TagValue")`: `TagKey` is `len 1..128`
/// and `TagValue` `len 1..256`.
const TAG_KEY_MAX_LEN: usize = 128;
/// See [`TAG_KEY_MAX_LEN`].
const TAG_VALUE_MAX_LEN: usize = 256;

/// One tag, as a request carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// A trail, event data store, dashboard or channel ARN, as the model's example ARN
/// formats write them.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// One row of the removal table: the tag set the resource is created with, the tags
/// the call names, and the tag set that must remain.
struct Removal {
    case: &'static str,
    initial: Vec<Value>,
    remove: Vec<Value>,
    remaining: Vec<Value>,
}

/// The removal table. Rows differ in how many keys are named, which of the stored
/// keys they hit, whether the value paired with a key matches the stored one, whether
/// the request names a value at all, and in the SIZE of the tag set.
fn removal_table() -> Vec<Removal> {
    let big: Vec<Value> = (0..20).map(|i| tag(&format!("k{i}"), "v")).collect();
    vec![
        Removal {
            case: "one key of three",
            initial: vec![tag("a", "1"), tag("b", "2"), tag("c", "3")],
            remove: vec![tag("b", "2")],
            remaining: vec![tag("a", "1"), tag("c", "3")],
        },
        Removal {
            case: "two keys of three, out of order",
            initial: vec![tag("a", "1"), tag("b", "2"), tag("c", "3")],
            remove: vec![tag("c", "3"), tag("a", "1")],
            remaining: vec![tag("b", "2")],
        },
        Removal {
            case: "every key",
            initial: vec![tag("a", "1"), tag("b", "2")],
            remove: vec![tag("a", "1"), tag("b", "2")],
            remaining: vec![],
        },
        Removal {
            case: "a key the resource does not carry",
            initial: vec![tag("a", "1")],
            remove: vec![tag("absent", "1")],
            remaining: vec![tag("a", "1")],
        },
        Removal {
            case: "no key at all",
            initial: vec![tag("a", "1")],
            remove: vec![],
            remaining: vec![tag("a", "1")],
        },
        Removal {
            case: "a stored key paired with a different value",
            initial: vec![tag("a", "1"), tag("b", "2")],
            remove: vec![tag("a", "a-value-the-resource-does-not-carry")],
            remaining: vec![tag("b", "2")],
        },
        Removal {
            case: "a key the request names with no value",
            initial: vec![tag("a", "1"), tag("b", "2")],
            remove: vec![json!({"Key": "a"})],
            remaining: vec![tag("b", "2")],
        },
        Removal {
            case: "a resource that carries no tags",
            initial: vec![],
            remove: vec![tag("a", "1")],
            remaining: vec![],
        },
        Removal {
            case: "half of twenty keys",
            initial: big.clone(),
            remove: big.iter().step_by(2).cloned().collect(),
            remaining: big
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == 1)
                .map(|(_, tag)| tag.clone())
                .collect(),
        },
    ]
}

/// RULE: `RemoveTags` drops exactly the keys the request names from the resource its
/// `ResourceId` ARN names — whatever value each key was paired with — and leaves every
/// other key in place; a second identical call changes nothing further, which is what
/// the operation's `@idempotent` trait promises.
///
/// Every row runs against BOTH taggable resources this service persists, a trail and a
/// dashboard, and the removal is read back through `ListTags`.
#[tokio::test]
async fn a_removal_drops_the_named_keys_and_leaves_the_rest() {
    let emulator = Emulator::new();

    for (index, row) in removal_table().into_iter().enumerate() {
        for kind in ["trail", "dashboard"] {
            let case = format!("{} ({kind})", row.case);
            let name = format!("tagging-{kind}-{index}").replace('-', "_");
            let resource_id = if kind == "trail" {
                emulator
                    .trail_with_tags(&name, HOME_REGION, &row.initial)
                    .await
            } else {
                emulator
                    .dashboard_with_tags(&name, HOME_REGION, &row.initial)
                    .await
            };

            let before = emulator.tags_of(&case, &resource_id).await;
            assert_eq!(
                before,
                Value::Array(row.initial.clone()),
                "case {case}: expected the created tag set {:?}, got {before:?}",
                row.initial
            );

            emulator
                .remove_tags(&resource_id, &row.remove)
                .await
                .expect_ok(&case);
            let after = emulator.tags_of(&case, &resource_id).await;
            assert_eq!(
                after,
                Value::Array(row.remaining.clone()),
                "case {case}: expected {:?} to remain, got {after:?}",
                row.remaining
            );

            // `@idempotent`: the same removal again is another success and leaves the
            // same tag set behind.
            emulator
                .remove_tags(&resource_id, &row.remove)
                .await
                .expect_ok(&format!("{case}, repeated"));
            let again = emulator.tags_of(&case, &resource_id).await;
            assert_eq!(
                again, after,
                "case {case}: a repeated removal changed the tag set to {again:?}"
            );
        }
    }
}

/// RULE: a `ResourceId` that is not one of the four taggable ARN forms names no
/// resource and is refused with the invalid-ARN error the model declares for the kind
/// it was written as — HTTP 400, naming the ARN it refused — and a CloudTrail ARN
/// naming a resource type that carries no tags with
/// `ResourceTypeNotSupportedException`.
///
/// The rows differ in WHICH part of the ARN is wrong (shape, field count, service,
/// Region, account, resource type, id) and in the kind the value names.
#[tokio::test]
async fn an_arn_that_names_no_taggable_resource_is_refused_as_its_own_kind() {
    let emulator = Emulator::new();
    // (case, ResourceId, the modelled error code, the refusal text)
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "not an ARN at all",
            "my-trail".to_string(),
            "CloudTrailARNInvalidException",
            "The ARN my-trail is invalid.",
        ),
        (
            "an ARN with too few fields",
            "arn:aws:cloudtrail:us-east-1:trail/my-trail".to_string(),
            "CloudTrailARNInvalidException",
            "The ARN arn:aws:cloudtrail:us-east-1:trail/my-trail is invalid.",
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{HOME_REGION}:{ACCOUNT}:trail/my-trail"),
            "CloudTrailARNInvalidException",
            "The ARN arn:aws:s3:us-east-1:000000000000:trail/my-trail is invalid.",
        ),
        (
            "a resource segment with no type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:my-trail"),
            "CloudTrailARNInvalidException",
            "The ARN arn:aws:cloudtrail:us-east-1:000000000000:my-trail is invalid.",
        ),
        (
            "a trail ARN whose name breaks the trail-name rules",
            arn(HOME_REGION, "trail", "my--trail"),
            "CloudTrailARNInvalidException",
            "The ARN arn:aws:cloudtrail:us-east-1:000000000000:trail/my--trail is invalid.",
        ),
        (
            "a dashboard ARN whose name breaks the dashboard-name rules",
            arn(HOME_REGION, "dashboard", "no spaces allowed"),
            "CloudTrailARNInvalidException",
            "The ARN arn:aws:cloudtrail:us-east-1:000000000000:dashboard/no spaces allowed \
             is invalid.",
        ),
        (
            "a channel ARN with no id",
            arn(HOME_REGION, "channel", ""),
            "ChannelARNInvalidException",
            "The channel ARN arn:aws:cloudtrail:us-east-1:000000000000:channel/ is invalid.",
        ),
        (
            "a channel ARN with no Region",
            format!("arn:aws:cloudtrail::{ACCOUNT}:channel/a-channel"),
            "ChannelARNInvalidException",
            "The channel ARN arn:aws:cloudtrail::000000000000:channel/a-channel is invalid.",
        ),
        (
            "a channel ARN with no account",
            format!("arn:aws:cloudtrail:{OTHER_REGION}::channel/a-channel"),
            "ChannelARNInvalidException",
            "The channel ARN arn:aws:cloudtrail:us-west-2::channel/a-channel is invalid.",
        ),
        (
            "an event data store ARN with no id",
            arn(OTHER_REGION, "eventdatastore", ""),
            "EventDataStoreARNInvalidException",
            "The event data store ARN \
             arn:aws:cloudtrail:us-west-2:000000000000:eventdatastore/ is invalid.",
        ),
        (
            "an event data store ARN with no partition",
            format!("arn::cloudtrail:{HOME_REGION}:{ACCOUNT}:eventdatastore/EXAMPLE"),
            "EventDataStoreARNInvalidException",
            "The event data store ARN \
             arn::cloudtrail:us-east-1:000000000000:eventdatastore/EXAMPLE is invalid.",
        ),
        (
            // `query/` is a CloudTrail resource the tagging operations do not carry
            // tags for; `ResourceTypeNotSupportedException` is modelled for exactly
            // this ("the specified resource type is not supported by CloudTrail").
            "a CloudTrail ARN naming an untaggable resource type",
            arn(HOME_REGION, "query", "a1b2c3"),
            "ResourceTypeNotSupportedException",
            "The resource type query in the ARN \
             arn:aws:cloudtrail:us-east-1:000000000000:query/a1b2c3 is not supported by \
             CloudTrail: tags are carried by a trail, an event data store, a dashboard or \
             a channel.",
        ),
    ];

    for (case, resource_id, code, message) in rows {
        let written = emulator.remove_tags(&resource_id, &[tag("a", "1")]).await;
        written.expect_error(&format!("RemoveTags: {case}"), 400, code);
        assert_eq!(
            written.message(),
            message,
            "case {case}: expected the message {message:?}, got {written:?}"
        );
        // The same reading of `ResourceId` serves the reader, so the same value is
        // refused the same way there.
        let listed = emulator
            .call("ListTags", &json!({"ResourceIdList": [resource_id]}))
            .await;
        listed.expect_error(&format!("ListTags: {case}"), 400, code);
        assert_eq!(
            listed.message(),
            message,
            "case {case}: expected ListTags to word it the same, got {listed:?}"
        );
    }
}

/// RULE: a well-formed taggable ARN that names no stored resource is refused with the
/// kind-specific not-found error the model declares — HTTP 400, with a message built
/// from the request — never as a success that removed nothing.
///
/// A channel and an event data store answer their own not-found shape; a trail and a
/// dashboard, for which the tagging operations declare no kind-specific shape, answer
/// the generic `ResourceNotFoundException`. The refusal text is the one CloudTrail
/// words for that KIND: a trail by an identifier and the account the call is made for,
/// a channel or an event data store by the ARN the caller passed, a dashboard by one
/// sentence naming nothing. The trail wording's subject is the one thing the write and
/// the read paths differ in: `RemoveTags` names the trail's NAME, `ListTags` the ARN.
#[tokio::test]
async fn a_well_formed_arn_naming_no_resource_is_not_found() {
    let emulator = Emulator::new();
    // A stored trail and dashboard, so a row's failure is "that ARN names nothing",
    // not "the registry is empty".
    emulator
        .trail_with_tags("a-stored-trail", HOME_REGION, &[tag("a", "1")])
        .await;
    emulator
        .dashboard_with_tags("a_stored_dashboard", HOME_REGION, &[tag("a", "1")])
        .await;

    let channel = arn(
        OTHER_REGION,
        "channel",
        "39032a83-30fe-4581-8bd3-351632c2be72",
    );
    let numeric_channel = arn(HOME_REGION, "channel", "01234567890");
    let store = arn(
        HOME_REGION,
        "eventdatastore",
        "EXAMPLE-f852-4e8f-8bd1-bcf6cEXAMPLE",
    );
    let unknown_dashboard = arn(HOME_REGION, "dashboard", "no_such_dashboard");
    let dashboard_elsewhere = arn(OTHER_REGION, "dashboard", "a_stored_dashboard");
    let unknown_trail = arn(HOME_REGION, "trail", "no.such.trail");
    let trail_elsewhere = arn(OTHER_REGION, "trail", "a-stored-trail");

    // (case, ResourceId, the text a tag WRITE words the refusal with, the text
    // `ListTags` words it with, the code `RemoveTags` answers, the code `ListTags`
    // answers). The two texts differ only where CloudTrail's own do: the subject of
    // an unknown TRAIL's wording.
    let rows: Vec<(&str, &String, String, String, &str, &str)> = vec![
        (
            "a channel ARN",
            &channel,
            format!("The channel {channel} was not found."),
            format!("The channel {channel} was not found."),
            "ChannelNotFoundException",
            "ChannelNotFoundException",
        ),
        (
            "a channel ARN with an opaque numeric id",
            &numeric_channel,
            format!("The channel {numeric_channel} was not found."),
            format!("The channel {numeric_channel} was not found."),
            "ChannelNotFoundException",
            "ChannelNotFoundException",
        ),
        (
            "an event data store ARN",
            &store,
            format!("The event data store {store} was not found."),
            format!("The event data store {store} was not found."),
            "EventDataStoreNotFoundException",
            "EventDataStoreNotFoundException",
        ),
        (
            "a trail ARN naming no trail",
            &unknown_trail,
            format!("Unknown trail: no.such.trail for the user: {ACCOUNT}"),
            format!("Unknown trail: {unknown_trail} for the user: {ACCOUNT}"),
            "ResourceNotFoundException",
            "ResourceNotFoundException",
        ),
        (
            "a stored trail's name in another Region",
            &trail_elsewhere,
            format!("Unknown trail: a-stored-trail for the user: {ACCOUNT}"),
            format!("Unknown trail: {trail_elsewhere} for the user: {ACCOUNT}"),
            "ResourceNotFoundException",
            "ResourceNotFoundException",
        ),
        (
            "a dashboard ARN naming no dashboard",
            &unknown_dashboard,
            "No matching dashboard found.".to_string(),
            "No matching dashboard found.".to_string(),
            "ResourceNotFoundException",
            "ResourceNotFoundException",
        ),
        (
            "a stored dashboard's name in another Region",
            &dashboard_elsewhere,
            "No matching dashboard found.".to_string(),
            "No matching dashboard found.".to_string(),
            "ResourceNotFoundException",
            "ResourceNotFoundException",
        ),
    ];

    for (case, resource_id, written, listed_text, remove_code, list_code) in rows {
        let reply = emulator.remove_tags(resource_id, &[tag("a", "1")]).await;
        reply.expect_error(&format!("RemoveTags: {case}"), 400, remove_code);
        assert_eq!(
            reply.message(),
            written,
            "case {case}: expected the message {written:?}, got {reply:?}"
        );

        let listed = emulator
            .call("ListTags", &json!({"ResourceIdList": [resource_id]}))
            .await;
        listed.expect_error(&format!("ListTags: {case}"), 400, list_code);
        assert_eq!(
            listed.message(),
            listed_text,
            "case {case}: expected the message {listed_text:?}, got {listed:?}"
        );
    }
}

/// RULE: a tag list that names one key twice is a malformed request —
/// `InvalidTagParameterException` "can also occur if there are duplicate tags" — and a
/// tag key is unique for the resource it applies to, so the duplicate is refused
/// whatever the values beside it and whatever the resource.
#[tokio::test]
async fn a_tag_list_naming_one_key_twice_is_refused() {
    let emulator = Emulator::new();
    let resource_id = emulator
        .trail_with_tags(
            "duplicate-key-trail",
            HOME_REGION,
            &[tag("a", "1"), tag("b", "2")],
        )
        .await;

    let rows: Vec<(&str, Vec<Value>)> = vec![
        (
            "the same key twice with the same value",
            vec![tag("a", "1"), tag("a", "1")],
        ),
        (
            "the same key twice with different values",
            vec![tag("a", "1"), tag("a", "2")],
        ),
        (
            "the same key twice among other keys",
            vec![tag("b", "2"), tag("a", "1"), tag("c", "3"), tag("a", "9")],
        ),
        (
            "a key repeated with no value",
            vec![json!({"Key": "a"}), json!({"Key": "a"})],
        ),
    ];

    for (case, tags) in rows {
        emulator
            .remove_tags(&resource_id, &tags)
            .await
            .expect_error(case, 400, "InvalidTagParameterException");
    }

    // The refusal left the tag set alone.
    let tags = emulator.tags_of("after a refused duplicate", &resource_id).await;
    assert_eq!(
        tags,
        json!([tag("a", "1"), tag("b", "2")]),
        "expected the refused call to change nothing, got {tags:?}"
    );
}

/// RULE: `ListTags` reports the stored tag set of every ARN the request names, in the
/// order the request named them, and one call names at most the documented 20 ARNs
/// ("The list has a limit of 20 ARNs").
#[tokio::test]
async fn list_tags_reports_every_named_arn_in_order_and_bounds_the_list() {
    let emulator = Emulator::new();
    let trail = emulator
        .trail_with_tags("ordered-trail", HOME_REGION, &[tag("t", "1")])
        .await;
    let dashboard = emulator
        .dashboard_with_tags("ordered_dashboard", HOME_REGION, &[tag("d", "2")])
        .await;
    let untagged = emulator
        .trail_with_tags("untagged-trail", OTHER_REGION, &[])
        .await;

    let rows: Vec<(&str, Vec<&String>)> = vec![
        ("one ARN", vec![&trail]),
        ("two ARNs", vec![&trail, &dashboard]),
        ("the same ARN twice", vec![&trail, &trail]),
        (
            "ARNs in the reverse order, one of them untagged",
            vec![&untagged, &dashboard, &trail],
        ),
        ("no ARN at all", vec![]),
    ];

    for (case, ids) in rows {
        let reply = emulator
            .call("ListTags", &json!({"ResourceIdList": ids}))
            .await;
        let body = reply.expect_ok(case);
        let reported: Vec<&str> = body
            .get("ResourceTagList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: no ResourceTagList in {body:?}"))
            .iter()
            .map(|entry| {
                entry
                    .get("ResourceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            })
            .collect();
        let expected: Vec<&str> = ids.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected the ARNs {expected:?} in order, got {body:?}"
        );
    }

    // The documented bound, and one ARN past it. Every entry is the same stored
    // trail, so the only thing that changes across the two calls is the COUNT.
    let at_limit: Vec<&String> = (0..state::LIST_TAGS_RESOURCE_LIMIT).map(|_| &trail).collect();
    emulator
        .call("ListTags", &json!({"ResourceIdList": at_limit}))
        .await
        .expect_ok("the documented maximum of 20 ARNs");

    let past_limit: Vec<&String> = (0..=state::LIST_TAGS_RESOURCE_LIMIT)
        .map(|_| &trail)
        .collect();
    emulator
        .call("ListTags", &json!({"ResourceIdList": past_limit}))
        .await
        .expect_error("one ARN past the documented maximum", 400, "ValidationException");

    // `NextToken` is "Reserved for future use" and a page carries every ARN the
    // request named, so no call this service serves ever mints one: a token the
    // caller invented continues nothing and is refused as the token it is not.
    emulator
        .call(
            "ListTags",
            &json!({"ResourceIdList": [&trail], "NextToken": "a-token-this-service-never-minted"}),
        )
        .await
        .expect_error("a caller-invented NextToken", 400, "InvalidTokenException");
}

/// RULE (rules 2 and 3 over the third kind of resource this service stores): an event
/// data store ARN addresses the store of that id IN THE REGION THE ARN PINS — the rule
/// `State::event_data_store` applies — so a removal reaches that store's tag set, a
/// read reports what is left, and the same id addressed from another Region's ARN is
/// `EventDataStoreNotFoundException`.
///
/// `CreateEventDataStore` is still a scaffold stub, so the store is seeded through the
/// SAME registry the handlers read (the shared harness's `state()`), and every
/// assertion then goes over the wire.
#[tokio::test]
async fn a_removal_reaches_an_event_data_stores_tag_set() {
    let emulator = Emulator::new();
    let store_id = "aaaaaaaa-1111-2222-3333-444444444444";
    emulator.state().lock().event_data_stores.insert(
        store_id.to_string(),
        EventDataStoreRecord {
            id: store_id.to_string(),
            name: "a-seeded-store".to_string(),
            region: HOME_REGION.to_string(),
            status: EventDataStoreState::Enabled,
            tags: vec![
                Tag {
                    key: "a".to_string(),
                    value: Some("1".to_string()),
                },
                Tag {
                    key: "b".to_string(),
                    value: Some("2".to_string()),
                },
            ],
            ..EventDataStoreRecord::default()
        },
    );

    let arn = state::event_data_store_arn(HOME_REGION, store_id);
    let before = emulator.tags_of("the seeded store", &arn).await;
    assert_eq!(
        before,
        json!([tag("a", "1"), tag("b", "2")]),
        "expected the seeded tag set, got {before:?}"
    );

    emulator
        .remove_tags(&arn, &[tag("a", "1")])
        .await
        .expect_ok("a removal from an event data store");
    let after = emulator.tags_of("after the removal", &arn).await;
    assert_eq!(
        after,
        json!([tag("b", "2")]),
        "expected only the key the removal left, got {after:?}"
    );

    // The ARN's Region is part of the address: the same id in another Region names no
    // store, on the write path and on the read path alike.
    let elsewhere = state::event_data_store_arn(OTHER_REGION, store_id);
    let refused = emulator.remove_tags(&elsewhere, &[tag("b", "2")]).await;
    refused.expect_error(
        "the store's id in another Region",
        400,
        "EventDataStoreNotFoundException",
    );
    let expected = format!("The event data store {elsewhere} was not found.");
    assert_eq!(
        refused.message(),
        expected,
        "expected the message {expected:?}, got {refused:?}"
    );
    emulator
        .call("ListTags", &json!({"ResourceIdList": [&elsewhere]}))
        .await
        .expect_error(
            "ListTags on the store's id in another Region",
            400,
            "EventDataStoreNotFoundException",
        );

    // The refused removal left the store's tag set alone.
    let unchanged = emulator.tags_of("after the refused removal", &arn).await;
    assert_eq!(
        unchanged, after,
        "expected the refused removal to change nothing, got {unchanged:?}"
    );
}

/// RULE: a tag key or value outside the length the model declares for it is refused
/// with `InvalidTagParameterException` — the shape the model declares for "the
/// specified tag key or values are not valid" — worded from the bound it broke, not
/// with the framework's generic `ValidationException`. The rule holds for every
/// operation that carries a tag, and only for the length bounds: any other refusal
/// keeps the framework's own wording.
///
/// The rows run each bound AT its maximum (accepted), one character past it and far
/// past it, for both the key and the value.
#[tokio::test]
async fn a_tag_outside_its_modelled_length_is_an_invalid_tag_parameter() {
    let emulator = Emulator::new();
    let resource_id = emulator
        .trail_with_tags("tag-length-trail", HOME_REGION, &[tag("a", "1")])
        .await;

    // (case, the tag the request carries, the refusal text, or None when the tag is
    // inside its bounds)
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "a key at the maximum",
            tag(&"k".repeat(TAG_KEY_MAX_LEN), "v"),
            None,
        ),
        (
            "a value at the maximum",
            tag("k", &"v".repeat(TAG_VALUE_MAX_LEN)),
            None,
        ),
        (
            "a key one character past the maximum",
            tag(&"k".repeat(TAG_KEY_MAX_LEN + 1), "v"),
            Some(format!(
                "Tag key length cannot be more than {TAG_KEY_MAX_LEN} characters."
            )),
        ),
        (
            "a value one character past the maximum",
            tag("k", &"v".repeat(TAG_VALUE_MAX_LEN + 1)),
            Some(format!(
                "Tag value length cannot be more than {TAG_VALUE_MAX_LEN} characters."
            )),
        ),
        (
            "a key far past the maximum",
            tag(&"k".repeat(TAG_KEY_MAX_LEN * 8), "v"),
            Some(format!(
                "Tag key length cannot be more than {TAG_KEY_MAX_LEN} characters."
            )),
        ),
        (
            "a value far past the maximum",
            tag("k", &"v".repeat(TAG_VALUE_MAX_LEN * 8)),
            Some(format!(
                "Tag value length cannot be more than {TAG_VALUE_MAX_LEN} characters."
            )),
        ),
    ];

    for (index, (case, carried, refusal)) in rows.into_iter().enumerate() {
        let removal = emulator
            .remove_tags(&resource_id, std::slice::from_ref(&carried))
            .await;
        // The same tag reaches a create, which carries its own `TagsList`: the rule is
        // the tag's, not one operation's.
        let create = emulator
            .call(
                "CreateTrail",
                &json!({
                    "Name": format!("tag-length-{index}"),
                    "S3BucketName": "an-emulator-bucket",
                    "TagsList": [carried],
                }),
            )
            .await;

        match refusal {
            None => {
                removal.expect_ok(&format!("RemoveTags: {case}"));
                create.expect_ok(&format!("CreateTrail: {case}"));
            }
            Some(message) => {
                for (operation, reply) in [("RemoveTags", &removal), ("CreateTrail", &create)] {
                    reply.expect_error(
                        &format!("{operation}: {case}"),
                        400,
                        "InvalidTagParameterException",
                    );
                    assert_eq!(
                        reply.message(),
                        message,
                        "case {operation}: {case}: expected the message {message:?}, got {reply:?}"
                    );
                }
            }
        }
    }

    // The rewording is the LENGTH bounds only: a tag key below its minimum keeps the
    // framework's own refusal, which is what this service answers for every other
    // modelled-constraint violation.
    emulator
        .remove_tags(&resource_id, &[json!({"Key": "", "Value": "v"})])
        .await
        .expect_error("a key below the minimum", 400, "ValidationException");
}

