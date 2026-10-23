// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CloudTrail resource tagging — `AddTags` here, beside the
//! `RemoveTags` / `ListTags` cases this suite already carried.
//!
//! Every case goes over the wire, through the crate's shared `emulator` harness: an
//! `awsJson1_1` `POST /` with CloudTrail's own `X-Amz-Target` prefix through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a tagging operation addresses a resource ONLY by ARN, and the ARN's resource
//!   type segment decides which registry resolves the id — and therefore which
//!   modelled error a malformed ARN, an untaggable resource type and an ARN naming
//!   nothing answer with;
//! * `AddTags` merges the request's tags into the resource by key: a key the resource
//!   already carries takes the request's value, a key it does not carry is added;
//! * one request cannot name the same tag key twice, and a tag key or value outside
//!   its modelled length bound is refused — both before any resource is resolved;
//! * a resource carries at most [`state::TAGS_PER_RESOURCE_LIMIT`] tags, counted
//!   after the merge, and a refused request writes nothing;
//! * a dashboard CloudTrail has not finished loading is not ready to be tagged;
//! * an event data store resolves in its own registry, held to the Region its ARN
//!   pins, and one awaiting deletion is inactive;
//! * a trail or event data store that applies to all Regions is tagged only from its
//!   home Region;
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
//! * `ListTags` reports the stored tag set of every ARN the request names, in the
//!   order it named them, and bounds one call at the documented 20 ARNs.

use serde_json::{json, Value};
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::Tag;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    self, EventDataStoreRecord, EventDataStoreState,
};

// The router, the SigV4-scoped call and the reply assertions are the crate's shared
// harness; these cases add only what the tagging operations themselves need.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a resource to the Region it was made in.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated ARN is minted under (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// `TagKey`'s and `TagValue`'s modelled length maxima, from
/// `smithy_query(query="shape", shape="TagKey,TagValue")`: `TagKey` is `len 1..128`
/// and `TagValue` `len 1..256`.
const TAG_KEY_MAX_LEN: usize = 128;
/// See [`TAG_KEY_MAX_LEN`].
const TAG_VALUE_MAX_LEN: usize = 256;

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a case that waits for the
/// clock does not race the instant the handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

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

/// One tag, as a request carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// What these cases assert about a reply beyond the shared harness's status and error
/// code: the exact message, for the refusals whose wording is a rule.
trait TagReply {
    /// Assert this reply is the modelled failure `code` with HTTP `status` and
    /// exactly `message`.
    fn expect_error_message(&self, case: &str, status: u16, code: &str, message: &str);
}

impl TagReply for Reply {
    fn expect_error_message(&self, case: &str, status: u16, code: &str, message: &str) {
        self.expect_error(case, status, code);
        // `Reply::message` is the shared harness's reader: a modelled error's
        // `Message` member, falling back to a framework refusal's lowercase one.
        assert_eq!(
            self.message(),
            message,
            "case {case}: wrong error message, got {self:?}"
        );
    }
}

/// The `AddTags` calls and the tag reads these cases make, on the shared harness.
trait TagEmulator {
    /// `AddTags` on `resource_id` with `tags`, called from `region`.
    async fn add_tags_in(&self, region: &str, resource_id: &str, tags: &Value) -> Reply;

    /// `AddTags` on `resource_id` with `tags`, called from [`HOME_REGION`].
    async fn add_tags(&self, resource_id: &str, tags: &Value) -> Reply;

    /// The tag set the registry holds for the resource `arn` names, as `(key, value)`
    /// pairs in stored order — the same store `ListTags` reports from, read directly
    /// where a case asserts a refusal wrote nothing.
    fn stored_tags(&self, arn: &state::TaggableArn) -> Vec<(String, Option<String>)>;
}

impl TagEmulator for Emulator {
    async fn add_tags_in(&self, region: &str, resource_id: &str, tags: &Value) -> Reply {
        self.call_in(
            "AddTags",
            region,
            &json!({"ResourceId": resource_id, "TagsList": tags}),
        )
        .await
    }

    async fn add_tags(&self, resource_id: &str, tags: &Value) -> Reply {
        self.add_tags_in(HOME_REGION, resource_id, tags).await
    }

    fn stored_tags(&self, arn: &state::TaggableArn) -> Vec<(String, Option<String>)> {
        let state = self.state();
        let mut store = state.lock();
        store
            .resource_tags_mut(arn)
            .map(|tags| {
                tags.iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A trail, event data store, dashboard or channel ARN, as the model's example ARN
/// formats write them.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// `TagsList` as the wire carries it: a key with a value, or a key alone ("If you
/// specify a key without a value, the tag will be created with the specified key and
/// a value of null").
fn tags(pairs: &[(&str, Option<&str>)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"Key": key, "Value": value}),
                None => json!({"Key": key}),
            })
            .collect(),
    )
}

/// Create a trail called `name` in `region`, and hand back its ARN.
async fn trail(emulator: &Emulator, region: &str, name: &str, multi_region: bool) -> String {
    emulator
        .call_in(
            "CreateTrail",
            region,
            &json!({
                "Name": name,
                "S3BucketName": "tagging-bucket",
                "IsMultiRegionTrail": multi_region,
            }),
        )
        .await
        .expect_ok(&format!("CreateTrail {name} in {region}"));
    arn(region, "trail", name)
}

/// Create a dashboard called `name` in `region` and wait out the load the clock
/// owes, so it is `CREATED` and ready to be tagged; hand back its ARN.
async fn dashboard(emulator: &Emulator, region: &str, name: &str) -> String {
    emulator
        .call_in("CreateDashboard", region, &json!({"Name": name}))
        .await
        .expect_ok(&format!("CreateDashboard {name} in {region}"));
    tokio::time::sleep(past_settle()).await;
    arn(region, "dashboard", name)
}

/// One tag set as a table row writes it: each tag's key, and the value it carries if
/// any.
type TagPairs = Vec<(&'static str, Option<&'static str>)>;

/// One row of the merge table: the case name, the Region the calls are made from, the
/// resource the ARN names, the first call's tags, the second call's tags, and the tag
/// set the resource then holds.
type MergeRow = (
    &'static str,
    &'static str,
    state::TaggableArn,
    TagPairs,
    TagPairs,
    TagPairs,
);

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

/// RULE: `AddTags` merges a request's tags into the resource its ARN names — an
/// existing key takes the request's value, a new key is appended — and the merged set
/// is what the registry then holds.
///
/// The rows differ in resource TYPE (trail, dashboard), in Region, in tag COUNT, in
/// whether a tag carries a value at all, and in whether the second call overwrites,
/// appends, or does both.
#[tokio::test]
async fn add_tags_merges_the_request_into_the_resource_its_arn_names() {
    let emulator = Emulator::new();

    // The resources the rows tag: two trails in different Regions and a dashboard.
    trail(&emulator, HOME_REGION, "merge-trail", false).await;
    trail(&emulator, OTHER_REGION, "merge-trail-west", false).await;
    dashboard(&emulator, HOME_REGION, "merge-dashboard").await;

    // (case, the Region the call is made from, the target, the first call's tags, the
    // second call's tags, the tag set the resource then holds)

    let rows: Vec<MergeRow> = vec![
        (
            "one tag on a trail, then a second key",
            HOME_REGION,
            state::TaggableArn::Trail {
                region: HOME_REGION.to_string(),
                name: "merge-trail".to_string(),
            },
            vec![("team", Some("security"))],
            vec![("owner", Some("alice"))],
            vec![("team", Some("security")), ("owner", Some("alice"))],
        ),
        (
            "three tags on a trail in another Region, then an overwrite of the middle key",
            OTHER_REGION,
            state::TaggableArn::Trail {
                region: OTHER_REGION.to_string(),
                name: "merge-trail-west".to_string(),
            },
            vec![
                ("a", Some("1")),
                ("b", Some("2")),
                ("c", Some("3")),
            ],
            vec![("b", Some("two"))],
            vec![("a", Some("1")), ("b", Some("two")), ("c", Some("3"))],
        ),
        (
            "a key without a value on a dashboard, then the same key with one",
            HOME_REGION,
            state::TaggableArn::Dashboard {
                region: HOME_REGION.to_string(),
                name: "merge-dashboard".to_string(),
            },
            vec![("cost-centre", None)],
            vec![("cost-centre", Some("rnd")), ("stage", None)],
            vec![("cost-centre", Some("rnd")), ("stage", None)],
        ),
    ];

    for (case, region, target, first, second, held) in rows {
        let resource_id = match &target {
            state::TaggableArn::Trail { region, name } => arn(region, "trail", name),
            state::TaggableArn::Dashboard { region, name } => arn(region, "dashboard", name),
            other => unreachable!("row {case}: unexpected target {other:?}"),
        };

        emulator
            .add_tags_in(region, &resource_id, &tags(&first))
            .await
            .expect_ok(&format!("{case}: the first AddTags"));
        // Read back over the WIRE, through `ListTags`: the tag set a client sees.
        let after_first = emulator
            .tags_of(&format!("{case}: after the first AddTags"), &resource_id)
            .await;
        assert_eq!(
            after_first,
            tags(&first),
            "case {case:?}: the first AddTags did not persist its tags, got {after_first:?}"
        );

        emulator
            .add_tags_in(region, &resource_id, &tags(&second))
            .await
            .expect_ok(&format!("{case}: the second AddTags"));
        let actual = emulator
            .tags_of(&format!("{case}: after the second AddTags"), &resource_id)
            .await;
        assert_eq!(
            actual,
            tags(&held),
            "case {case:?}: wrong merged tag set, got {actual:?}"
        );
    }

    // Each resource holds only its own row's tags: tagging one did not touch another.
    let home_trail_tags = emulator
        .tags_of(
            "tagging one resource must not touch another",
            &arn(HOME_REGION, "trail", "merge-trail"),
        )
        .await;
    assert_eq!(
        home_trail_tags,
        tags(&[("team", Some("security")), ("owner", Some("alice"))]),
        "case tagging one resource must not touch another: the home-Region trail holds \
         {home_trail_tags:?}"
    );
}

/// RULE: an event data store ARN resolves in the event data store registry, held to
/// the Region the ARN pins — a store the account holds is tagged, one awaiting
/// deletion is inactive, and an id or Region naming none is not found.
#[tokio::test]
async fn an_event_data_store_is_tagged_out_of_the_registry_that_holds_it() {
    let emulator = Emulator::new();
    // The registry is seeded directly: `CreateEventDataStore` is still a scaffold
    // with its own issue, and `lake_queries.rs` seeds it the same way.
    let live = "11111111-2222-3333-4444-555555555555";
    let deleting = "66666666-7777-8888-9999-aaaaaaaaaaaa";
    for (id, status) in [
        (live, state::EventDataStoreState::Enabled),
        (deleting, state::EventDataStoreState::PendingDeletion),
    ] {
        emulator.state().lock().event_data_stores.insert(
            id.to_string(),
            state::EventDataStoreRecord {
                id: id.to_string(),
                name: format!("store-{id}"),
                region: HOME_REGION.to_string(),
                status,
                ..Default::default()
            },
        );
    }
    let set = tags(&[("team", Some("security")), ("stage", None)]);

    let live_arn = arn(HOME_REGION, "eventdatastore", live);
    emulator
        .add_tags(&live_arn, &set)
        .await
        .expect_ok("an event data store the account holds");
    let actual = emulator
        .tags_of("an event data store the account holds", &live_arn)
        .await;
    assert_eq!(
        actual,
        tags(&[("team", Some("security")), ("stage", None)]),
        "case an event data store the account holds: wrong tag set, got {actual:?}"
    );

    // (case, the ARN, the expected error code)
    for (case, resource_id, code) in [
        (
            "an event data store awaiting deletion",
            arn(HOME_REGION, "eventdatastore", deleting),
            "InactiveEventDataStoreException",
        ),
        (
            "the same id addressed in another Region",
            arn(OTHER_REGION, "eventdatastore", live),
            "EventDataStoreNotFoundException",
        ),
        (
            "an id the registry does not hold",
            arn(
                HOME_REGION,
                "eventdatastore",
                "a1b9f92c-fa85-40de-9198-dd92e0c55524",
            ),
            "EventDataStoreNotFoundException",
        ),
    ] {
        emulator
            .add_tags(&resource_id, &set)
            .await
            .expect_error(case, 400, code);
    }
}

/// RULE: `AddTags` is `@idempotent` — repeating a call with the same tags leaves the
/// same tag set, so a key is held once however often it is added.
#[tokio::test]
async fn adding_the_same_tag_twice_leaves_one_tag() {
    let emulator = Emulator::new();
    let resource_id = trail(&emulator, HOME_REGION, "idempotent-trail", false).await;
    let set = tags(&[("team", Some("security")), ("stage", Some("prod"))]);

    for attempt in 1..=3 {
        emulator
            .add_tags(&resource_id, &set)
            .await
            .expect_ok(&format!("AddTags attempt {attempt}"));
        let actual = emulator
            .tags_of(&format!("AddTags attempt {attempt}"), &resource_id)
            .await;
        assert_eq!(
            actual,
            tags(&[("team", Some("security")), ("stage", Some("prod"))]),
            "case attempt {attempt}: a repeated AddTags changed the tag set, got {actual:?}"
        );
    }
}

/// RULE: an ARN that names no resource this account holds is refused with the
/// not-found error its RESOURCE TYPE models, and an ARN that is not a CloudTrail
/// resource ARN at all with the ARN-invalid error of the type it was written as.
///
/// The rows span every resource type the `AddTagsRequest$ResourceId` doc publishes,
/// every ARN-invalid error `AddTags` models, and the resource-type and
/// not-a-CloudTrail-ARN cases — including a type (`import`) the issue did not cite.
#[tokio::test]
async fn an_arn_naming_no_taggable_resource_is_refused_by_its_resource_type() {
    let emulator = Emulator::new();
    // A trail and a dashboard that DO exist, so the rows below fail on the resource
    // they name rather than on an empty registry.
    trail(&emulator, HOME_REGION, "a-real-trail", false).await;
    dashboard(&emulator, HOME_REGION, "a-real-dashboard").await;

    // (case, ResourceId, expected HTTP status, expected error code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a trail ARN naming no trail",
            arn(HOME_REGION, "trail", "no-such-trail"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a trail ARN in a Region that holds no trail of that name",
            arn(OTHER_REGION, "trail", "a-real-trail"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a dashboard ARN naming no dashboard",
            arn(HOME_REGION, "dashboard", "no-such-dashboard"),
            400,
            "ResourceNotFoundException",
        ),
        (
            // CloudTrail Lake event data stores have no registry in this emulator
            // yet (`CreateEventDataStore` is a scaffold), so every well-formed event
            // data store ARN names nothing.
            "an event data store ARN naming no event data store",
            arn(
                OTHER_REGION,
                "eventdatastore",
                "a1b9f92c-fa85-40de-9198-dd92e0c55524",
            ),
            400,
            "EventDataStoreNotFoundException",
        ),
        (
            "a channel ARN naming no channel",
            arn(
                OTHER_REGION,
                "channel",
                "39032a83-30fe-4581-8bd3-351632c2be72",
            ),
            400,
            "ChannelNotFoundException",
        ),
        (
            // An event data store id and a channel id are OPAQUE to this service: the
            // model constrains the whole ARN and says nothing further about the id, so
            // an id of any shape names a store or channel — and names none when the
            // registry holds none.
            "an event data store ARN whose id is not a UUID",
            arn(OTHER_REGION, "eventdatastore", "not-a-uuid"),
            400,
            "EventDataStoreNotFoundException",
        ),
        (
            "a channel ARN with no channel id",
            arn(OTHER_REGION, "channel", ""),
            400,
            "ChannelARNInvalidException",
        ),
        (
            "a bare trail name rather than an ARN",
            "a-real-trail".to_string(),
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{HOME_REGION}:{ACCOUNT}:trail/a-real-trail"),
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "a CloudTrail ARN with no resource type at all",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:a-real-trail"),
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "a trail ARN whose trail name breaks the documented name rules",
            arn(HOME_REGION, "trail", "no"),
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "a CloudTrail ARN naming a resource type CloudTrail does not tag",
            arn(HOME_REGION, "import", "01234567890"),
            400,
            "ResourceTypeNotSupportedException",
        ),
    ];

    for (case, resource_id, status, code) in rows {
        let reply = emulator
            .add_tags(&resource_id, &tags(&[("team", Some("security"))]))
            .await;
        reply.expect_error(case, status, code);
    }
}

/// RULE: a resource carries at most [`state::TAGS_PER_RESOURCE_LIMIT`] tags ("up to a
/// limit of 50"), counted after the merge — so overwriting a key it already carries
/// always fits, and a refused request leaves the stored tag set untouched.
#[tokio::test]
async fn a_resource_carries_at_most_the_documented_number_of_tags() {
    let emulator = Emulator::new();
    let resource_id = trail(&emulator, HOME_REGION, "limit-trail", false).await;
    let limit = state::TAGS_PER_RESOURCE_LIMIT;
    let key = |index: usize| format!("key-{index}");

    // Exactly the limit, in one call.
    let full: Vec<Value> = (0..limit)
        .map(|index| json!({"Key": key(index), "Value": index.to_string()}))
        .collect();
    emulator
        .add_tags(&resource_id, &Value::Array(full))
        .await
        .expect_ok(&format!("AddTags with {limit} tags"));
    // Read back over the WIRE: `ListTags` reports what the resource now carries.
    let at_limit = emulator.tags_of("the limit exactly", &resource_id).await;
    assert_eq!(
        at_limit.as_array().map(Vec::len),
        Some(limit),
        "case the limit exactly: the resource does not hold {limit} tags, got {at_limit:?}"
    );

    // Overwriting a key the resource already carries adds no tag, so it still fits.
    emulator
        .add_tags(&resource_id, &tags(&[("key-0", Some("rewritten"))]))
        .await
        .expect_ok("AddTags overwriting a held key at the limit");
    let after_overwrite = emulator
        .tags_of("overwrite at the limit", &resource_id)
        .await;
    assert_eq!(
        (
            after_overwrite.as_array().map(Vec::len),
            after_overwrite[0].get("Value").and_then(Value::as_str)
        ),
        (Some(limit), Some("rewritten")),
        "case overwrite at the limit: expected {limit} tags with key-0 rewritten, got \
         {after_overwrite:?}"
    );

    // One key past the limit is refused, and writes nothing.
    emulator
        .add_tags(&resource_id, &tags(&[("one-too-many", Some("x"))]))
        .await
        .expect_error("one tag past the limit", 400, "TagsLimitExceededException");
    let after_refusal = emulator
        .tags_of("one tag past the limit", &resource_id)
        .await;
    assert_eq!(
        after_refusal, after_overwrite,
        "case one tag past the limit: the refused request changed the tag set, got \
         {after_refusal:?}"
    );

    // And the same bound on a resource with no tags at all: the limit + 1 in ONE call
    // is refused, the limit is accepted.
    let fresh = trail(&emulator, HOME_REGION, "limit-trail-fresh", false).await;
    let fresh_target = state::TaggableArn::Trail {
        region: HOME_REGION.to_string(),
        name: "limit-trail-fresh".to_string(),
    };
    let past_limit: Vec<Value> = (0..=limit)
        .map(|index| json!({"Key": key(index), "Value": index.to_string()}))
        .collect();
    emulator
        .add_tags(&fresh, &Value::Array(past_limit))
        .await
        .expect_error(
            "the limit plus one in one call",
            400,
            "TagsLimitExceededException",
        );
    // A refusal must have written NOTHING, which the store itself answers.
    assert!(
        emulator.stored_tags(&fresh_target).is_empty(),
        "case the limit plus one in one call: the refused request wrote tags, got {:?}",
        emulator.stored_tags(&fresh_target)
    );
}

/// RULE: "Tag key names must be unique; you cannot have two keys with the same name
/// but different values" — one request naming a key twice is refused as an invalid
/// tag parameter, whatever resource the ARN names, and writes nothing.
#[tokio::test]
async fn one_request_cannot_name_the_same_tag_key_twice() {
    let emulator = Emulator::new();
    let resource_id = trail(&emulator, HOME_REGION, "duplicate-trail", false).await;
    let target = state::TaggableArn::Trail {
        region: HOME_REGION.to_string(),
        name: "duplicate-trail".to_string(),
    };

    let rows: Vec<(&str, Value)> = vec![
        (
            "the same key twice with different values",
            tags(&[("team", Some("security")), ("team", Some("platform"))]),
        ),
        (
            "the same key twice with the same value",
            tags(&[("team", Some("security")), ("team", Some("security"))]),
        ),
        (
            "a duplicate key behind an unrelated one",
            tags(&[
                ("stage", Some("prod")),
                ("team", Some("security")),
                ("team", None),
            ]),
        ),
    ];

    for (case, set) in rows {
        emulator.add_tags(&resource_id, &set).await.expect_error_message(
            case,
            400,
            "InvalidTagParameterException",
            "Multiple tags cannot have the same key.",
        );
        let actual = emulator.stored_tags(&target);
        assert!(
            actual.is_empty(),
            "case {case:?}: the refused request wrote tags, got {actual:?}"
        );

        // The request's own tags are held to the rules BEFORE any resource is
        // resolved, so an ARN naming nothing is refused the same way.
        for (case, absent) in [
            (
                "a trail that does not exist",
                arn(HOME_REGION, "trail", "no-such-trail"),
            ),
            (
                "a dashboard that does not exist",
                arn(HOME_REGION, "dashboard", "no-such-dashboard"),
            ),
        ] {
            emulator
                .add_tags(&absent, &set)
                .await
                .expect_error_message(
                    case,
                    400,
                    "InvalidTagParameterException",
                    "Multiple tags cannot have the same key.",
                );
        }
    }
}

/// RULE: `ConflictException` is "thrown when the specified resource is not ready for
/// an operation … before CloudTrail has time to fully load the resource" — a
/// dashboard is tagged only once the clock has settled it out of `CREATING`.
#[tokio::test]
async fn a_dashboard_is_tagged_once_cloudtrail_has_finished_loading_it() {
    let emulator = Emulator::new();
    emulator
        .call("CreateDashboard", &json!({"Name": "loading-dashboard"}))
        .await
        .expect_ok("CreateDashboard");
    let resource_id = arn(HOME_REGION, "dashboard", "loading-dashboard");
    let target = state::TaggableArn::Dashboard {
        region: HOME_REGION.to_string(),
        name: "loading-dashboard".to_string(),
    };
    let set = tags(&[("team", Some("security"))]);

    // Straight after the create, the dashboard is still loading.
    emulator
        .add_tags(&resource_id, &set)
        .await
        .expect_error(
            "AddTags while the dashboard is CREATING",
            409,
            "ConflictException",
        );
    let while_creating = emulator.stored_tags(&target);
    assert!(
        while_creating.is_empty(),
        "case AddTags while the dashboard is CREATING: the refused request wrote tags, \
         got {while_creating:?}"
    );

    // Once the load the clock owed is due, the same call is accepted.
    tokio::time::sleep(past_settle()).await;
    emulator
        .add_tags(&resource_id, &set)
        .await
        .expect_ok("AddTags once the dashboard is CREATED");
    let actual = emulator
        .tags_of("AddTags once the dashboard is CREATED", &resource_id)
        .await;
    assert_eq!(
        actual,
        tags(&[("team", Some("security"))]),
        "case AddTags once the dashboard is CREATED: wrong tag set, got {actual:?}"
    );
}

/// RULE: "You can tag a trail or event data store that applies to all Amazon Web
/// Services Regions only from the Region in which the trail or event data store was
/// created (also known as its home Region)" — so an all-Regions resource's ARN is
/// tagged from its home Region, for BOTH resource types the sentence names, while a
/// single-Region resource is unaffected.
#[tokio::test]
async fn a_resource_that_applies_to_all_regions_is_tagged_from_its_home_region() {
    let emulator = Emulator::new();
    let multi_region = trail(&emulator, HOME_REGION, "all-regions-trail", true).await;
    let single_region = trail(&emulator, OTHER_REGION, "one-region-trail", false).await;
    let set = tags(&[("team", Some("security"))]);

    // (case, the Region the call is made from, the trail ARN, expectation)
    emulator
        .add_tags_in(OTHER_REGION, &multi_region, &set)
        .await
        .expect_error(
            "a multi-Region trail tagged from another Region",
            400,
            "OperationNotPermittedException",
        );
    let untagged = emulator.stored_tags(&state::TaggableArn::Trail {
        region: HOME_REGION.to_string(),
        name: "all-regions-trail".to_string(),
    });
    assert!(
        untagged.is_empty(),
        "case a multi-Region trail tagged from another Region: the refused request wrote \
         tags, got {untagged:?}"
    );

    emulator
        .add_tags_in(HOME_REGION, &multi_region, &set)
        .await
        .expect_ok("a multi-Region trail tagged from its home Region");
    emulator
        .add_tags_in(OTHER_REGION, &single_region, &set)
        .await
        .expect_ok("a single-Region trail tagged from its home Region");

    for (case, resource_id) in [
        ("the multi-Region trail", &multi_region),
        ("the single-Region trail", &single_region),
    ] {
        let actual = emulator.tags_of(case, resource_id).await;
        assert_eq!(
            actual,
            tags(&[("team", Some("security"))]),
            "case {case}: wrong tag set, got {actual:?}"
        );
    }

    // The same rule on the other resource type the sentence names: an event data
    // store whose `multi_region_enabled` is set. The registry is seeded directly
    // because `CreateEventDataStore` is still a scaffold with its own issue.
    let store_id = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
    emulator.state().lock().event_data_stores.insert(
        store_id.to_string(),
        state::EventDataStoreRecord {
            id: store_id.to_string(),
            name: "all-regions-store".to_string(),
            region: HOME_REGION.to_string(),
            multi_region_enabled: true,
            ..Default::default()
        },
    );
    let store_arn = arn(HOME_REGION, "eventdatastore", store_id);
    let store_target = state::TaggableArn::EventDataStore {
        region: HOME_REGION.to_string(),
        id: store_id.to_string(),
    };

    emulator
        .add_tags_in(OTHER_REGION, &store_arn, &set)
        .await
        .expect_error(
            "a multi-Region event data store tagged from another Region",
            400,
            "OperationNotPermittedException",
        );
    let untagged_store = emulator.stored_tags(&store_target);
    assert!(
        untagged_store.is_empty(),
        "case a multi-Region event data store tagged from another Region: the refused \
         request wrote tags, got {untagged_store:?}"
    );

    emulator
        .add_tags_in(HOME_REGION, &store_arn, &set)
        .await
        .expect_ok("a multi-Region event data store tagged from its home Region");
    let tagged_store = emulator
        .tags_of(
            "a multi-Region event data store tagged from its home Region",
            &store_arn,
        )
        .await;
    assert_eq!(
        tagged_store,
        tags(&[("team", Some("security"))]),
        "case a multi-Region event data store tagged from its home Region: wrong tag set, \
         got {tagged_store:?}"
    );
}

