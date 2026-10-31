// SPDX-License-Identifier: CC0-1.0

//! Where a refusal a TAGGING request earns on its own face sits relative to resolving the
//! resource its `ResourceArn` names — over every level of the hierarchy this service mints
//! an ARN for, and over both writers of the tagging front end.
//!
//! RULE: a refusal decided from the members a tagging request carried — a tag key in the
//! namespace AWS reserves for itself, or a submitted tag map past a published tag bound —
//! needs nothing from the store, so it is answered BEFORE the ARN's resource is resolved:
//! the same malformed request earns the same `ValidationException` (400) whether the ARN
//! names a resource that exists or one that does not, at whichever level of the hierarchy
//! the ARN addresses, and `TagResource` and `UntagResource` classify it identically. Only a
//! request that breaks no such rule is judged by the lookup, and is then the tagging front
//! end's `NotFoundException` (404) for a resource this emulator does not hold.
//!
//! The rows span the class rather than one request:
//!
//! * the LEVEL ([`Target::kind`]) varies which resource the ARN addresses — channel group,
//!   channel, origin endpoint, harvest job — each ARN read back from the create that minted
//!   it, so no ARN in this suite is written out by hand;
//! * the EXISTENCE ([`Existence`]) varies whether that ARN names a stored resource or one
//!   that is absent at its deepest segment, which is the lookup the refusal must not wait
//!   for;
//! * the FAULT varies which request-level rule is broken and how — four spellings of a key
//!   in the reserved namespace ([`reserved_key_cases`]) and the three tag bounds
//!   ([`Bound::ALL`]), each built from the constant the handlers judge against
//!   (`state::RESERVED_TAG_KEY_PREFIX`, `state::MAX_TAGS_PER_RESOURCE`,
//!   `state::MAX_TAG_KEY_CHARS`, `state::MAX_TAG_VALUE_CHARS`), so a row cannot pass
//!   because the table disagrees with the rule;
//! * the DOOR varies where the keys arrive: `TagResource` carries them in `tags` and
//!   `UntagResource` in `tagKeys`, and every refusal names the member it was sent in.
//!
//! The third test is the control that keeps either verdict from being a constant: the same
//! requests with nothing wrong in them are judged by the lookup, so they are the 404 at an
//! absent ARN and accepted (204) at a live one.
//!
//! The sibling `request_refusal_precedence` suite asserts the same ordering at the creates
//! that take tags, with a channel-group ARN for the tagging pair; this suite is the rest of
//! the ARN levels and the parity between the two tagging writers.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, RESERVED_TAG_KEY_PREFIX,
    TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
};

// The crate's shared test harness: the control-plane client with its error-envelope reader
// and the smallest acceptable `CreateOriginEndpoint` body (`channel_lifecycle`), the three
// tagging calls and the tag-map readers (`resource_tags`), and the `CreateHarvestJob` body
// with its schedule clock (`rejection_rule_and_negative_paths`). This suite adds only what
// is specific to the ARN levels and the refusal ordering.
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use crate::resource_tags::{map_of, tag_map};

/// The hierarchy every row addresses, one resource per taggable level.
const LIVE_GROUP: &str = "tap-group";
const LIVE_CHANNEL: &str = "tap-channel";
const LIVE_ENDPOINT: &str = "tap-endpoint";
const LIVE_JOB: &str = "tap-job";

/// The deepest segment of an ARN that names nothing: it matches the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`, so the ARN stays well formed and the lookup is what would
/// fail.
const ABSENT_LEAF: &str = "tap-absent";

/// The tag every live resource is created with, so a refused request is shown to have
/// written and removed nothing.
///
/// `pub(crate)` with [`live_targets`], [`Target`], [`message`] and
/// [`assert_seeded_tags_untouched`]: a sibling suite that judges the same tagging front end
/// over the same hierarchy (`tagging_arn_account_scope`) drives it through these rather than
/// building a second chain of its own.
pub(crate) const SEEDED: (&str, &str) = ("seeded", "1");

/// An ordinary caller tag, sent BESIDE the fault so a refusal is shown to store neither
/// half, and sent alone by the control rows.
const ORDINARY: (&str, &str) = ("ordinary", "2");

/// The manifest the live endpoint publishes, and so the one its harvest job may harvest:
/// [`minimal_endpoint`] publishes exactly `index`.
const PUBLISHED_MANIFEST: &str = "index";

/// The sentence an error envelope carries: the modelled `ValidationException` spells its
/// member `Message`, while the tagging front end's own refusals (the two codes the model
/// does not declare) carry the protocol's `message`, so a row reading either one asks for
/// both.
pub(crate) fn message(body: &Value) -> &str {
    ["Message", "message"]
        .iter()
        .find_map(|member| body.get(*member).and_then(Value::as_str))
        .unwrap_or("")
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Whether the ARN under test names the stored resource or one that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Existence {
    Live,
    Absent,
}

impl Existence {
    const ALL: [Existence; 2] = [Existence::Live, Existence::Absent];
}

/// One taggable level: the ARN the service minted for the resource it holds there, and the
/// same ARN with its deepest name replaced by one nothing was created under.
pub(crate) struct Target {
    pub(crate) kind: &'static str,
    pub(crate) live: String,
    pub(crate) absent: String,
}

impl Target {
    fn arn(&self, existence: Existence) -> &str {
        match existence {
            Existence::Live => &self.live,
            Existence::Absent => &self.absent,
        }
    }
}

/// `minted` with its deepest path segment replaced by [`ABSENT_LEAF`].
///
/// Every ARN this service mints ends in `<type>/<name>`, so swapping the name leaves an ARN
/// of the same level and the same partition, Region and account — the lookup is then the
/// only thing that can fail. Derived from the minted string rather than assembled here, so
/// no part of an ARN in this suite is written out by hand.
fn with_absent_leaf(minted: &str) -> String {
    let (prefix, leaf) = minted
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("a minted ARN ends in <type>/<name>: {minted}"));
    assert!(
        !leaf.is_empty(),
        "a minted ARN names its resource after the last slash: {minted}"
    );
    format!("{prefix}/{ABSENT_LEAF}")
}

/// The `Arn` a create reported.
fn arn_of(case: &str, created: &Value) -> String {
    created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create must report an Arn, got {created}"))
        .to_string()
}

/// A channel group -> channel -> origin endpoint -> harvest job chain, every level created
/// carrying [`SEEDED`] and nothing else, and one [`Target`] per level.
pub(crate) async fn live_targets(emulator: &Emulator) -> Vec<Target> {
    let seeded = map_of(&[SEEDED]);

    // `@jsonName("tags")` on `CreateChannelGroupRequest$Tags` and `CreateChannelRequest$Tags`;
    // the member's own name on `CreateOriginEndpointRequest$Tags` and
    // `CreateHarvestJobRequest$Tags` (source: `smithy_query(query="operation",
    // operation="CreateChannelGroup,CreateChannel,CreateOriginEndpoint,CreateHarvestJob")`).
    let (status, group) = emulator
        .create_group(LIVE_GROUP, json!({"tags": seeded}))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup {LIVE_GROUP} expected 200, got {status} {group}"
    );
    let (status, channel) = emulator
        .create_channel(LIVE_GROUP, LIVE_CHANNEL, json!({"tags": seeded}))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel {LIVE_CHANNEL} expected 200, got {status} {channel}"
    );
    let mut endpoint_body = minimal_endpoint("TS");
    endpoint_body["Tags"] = json!(seeded);
    let (status, endpoint) = emulator
        .create_endpoint(LIVE_GROUP, LIVE_CHANNEL, LIVE_ENDPOINT, endpoint_body)
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint {LIVE_ENDPOINT} expected 200, got {status} {endpoint}"
    );
    // A window that opens in a minute and runs for an hour, so nothing the clock owes the
    // job ends it mid-test.
    let start = epoch_seconds_from_now(60.0);
    let mut job_body = harvest_body(start, start + 3600.0, &[PUBLISHED_MANIFEST]);
    job_body["HarvestJobName"] = json!(LIVE_JOB);
    job_body["Tags"] = json!(seeded);
    let (status, job) = emulator
        .call(
            "POST",
            &format!(
                "/channelGroup/{LIVE_GROUP}/channel/{LIVE_CHANNEL}/originEndpoint\
                 /{LIVE_ENDPOINT}/harvestJob"
            ),
            Some(job_body),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateHarvestJob {LIVE_JOB} expected 200, got {status} {job}"
    );

    [
        ("channel group", group),
        ("channel", channel),
        ("origin endpoint", endpoint),
        ("harvest job", job),
    ]
    .into_iter()
    .map(|(kind, created)| {
        let live = arn_of(kind, &created);
        Target {
            kind,
            absent: with_absent_leaf(&live),
            live,
        }
    })
    .collect()
}

/// `prefix` with its first character upper-cased, so a mixed-case spelling of the reserved
/// namespace is derived from the published prefix rather than written out.
fn capitalised(prefix: &str) -> String {
    let mut chars = prefix.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The spellings of a key in the reserved namespace, as `(label, key)`. The rows differ in
/// what follows the prefix and in the ASCII case it is spelled with — the namespace belongs
/// to the service either way — and every one is built from
/// `state::RESERVED_TAG_KEY_PREFIX`.
fn reserved_key_cases() -> Vec<(&'static str, String)> {
    vec![
        ("the reserved prefix alone", RESERVED_TAG_KEY_PREFIX.to_string()),
        (
            "a system key AWS itself writes",
            format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name"),
        ),
        (
            "the prefix in upper case",
            format!("{}owner", RESERVED_TAG_KEY_PREFIX.to_uppercase()),
        ),
        (
            "the prefix in mixed case",
            format!("{}cost-centre", capitalised(RESERVED_TAG_KEY_PREFIX)),
        ),
    ]
}

/// A tag bound a SUBMITTED map can be past on its own, without anything stored being read:
/// merging only ever adds keys and never shortens one.
#[derive(Debug, Clone, Copy)]
enum Bound {
    /// More tags than one resource may carry.
    Ceiling,
    /// A tag key longer than a tag key may be.
    KeyChars,
    /// A tag value longer than a tag value may be.
    ValueChars,
}

impl Bound {
    const ALL: [Bound; 3] = [Bound::Ceiling, Bound::KeyChars, Bound::ValueChars];

    /// A tag map one step past this bound, always beside [`ORDINARY`].
    fn tags(self) -> BTreeMap<String, String> {
        let mut tags = map_of(&[ORDINARY]);
        match self {
            Self::Ceiling => {
                for index in 0..=MAX_TAGS_PER_RESOURCE {
                    tags.insert(format!("bulk-{index}"), "x".to_string());
                }
            }
            Self::KeyChars => {
                tags.insert("k".repeat(MAX_TAG_KEY_CHARS + 1), "x".to_string());
            }
            Self::ValueChars => {
                tags.insert(
                    "over-long-value".to_string(),
                    "v".repeat(MAX_TAG_VALUE_CHARS + 1),
                );
            }
        }
        tags
    }

    /// The bound's own value, which is what its refusal has to state.
    fn must_state(self) -> String {
        match self {
            Self::Ceiling => MAX_TAGS_PER_RESOURCE.to_string(),
            Self::KeyChars => MAX_TAG_KEY_CHARS.to_string(),
            Self::ValueChars => MAX_TAG_VALUE_CHARS.to_string(),
        }
    }
}

/// Assert one answer is the request-level refusal: 400, the modelled envelope, a message
/// naming the member the keys were sent in and stating the rule that was broken.
fn assert_request_level_refusal(
    case: &str,
    member: &str,
    must_state: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: a request-level tag fault must be refused with HTTP 400, got \
         {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    assert!(
        message(body).contains(&format!("request.{member}:")),
        "case {case:?}: the refusal must name the member the keys arrived in \
         ({member:?}), got {:?}",
        message(body)
    );
    assert!(
        message(body).contains(must_state),
        "case {case:?}: the refusal must state the rule ({must_state:?}), got {:?}",
        message(body)
    );
}

/// Assert the live resource at `target` still carries exactly [`SEEDED`]: no refused
/// request wrote a tag, and none removed one.
pub(crate) async fn assert_seeded_tags_untouched(emulator: &Emulator, target: &Target, case: &str) {
    let (status, listed) = emulator.list_tags(&target.live).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource on the live {} expected 200, got {status} \
         {listed}",
        target.kind
    );
    assert_eq!(
        tag_map(&listed, "tags"),
        map_of(&[SEEDED]),
        "case {case:?}: a refused request must leave the {} carrying only its created \
         tags, got {listed}",
        target.kind
    );
}

/// RULE: a tag key in the reserved namespace is refused before the ARN's resource is
/// resolved, so both tagging writers answer the same 400 at every taggable level whether
/// the ARN names a stored resource or an absent one.
#[tokio::test]
async fn a_reserved_tag_key_outranks_the_arn_lookup_at_every_taggable_level() {
    let emulator = Emulator::new();
    let targets = live_targets(&emulator).await;

    for target in &targets {
        for existence in Existence::ALL {
            let arn = target.arn(existence);
            for (label, key) in reserved_key_cases() {
                let case = format!("{label:?} at {existence:?} {} ARN", target.kind);

                // `TagResource`, which would CREATE the key.
                let mut submitted = map_of(&[ORDINARY]);
                submitted.insert(key.clone(), "v".to_string());
                let (tag_status, tag_body) = emulator.tag(arn, &submitted).await;
                assert_request_level_refusal(
                    &case,
                    "tags",
                    RESERVED_TAG_KEY_PREFIX,
                    tag_status,
                    &tag_body,
                );
                assert_eq!(
                    validation_type(&tag_body),
                    "MEMBER_INVALID",
                    "case {case:?}: TagResource expected MEMBER_INVALID, got {tag_body}"
                );

                // `UntagResource`, which would DELETE it: the same namespace rule, so the
                // same classification.
                let (untag_status, untag_body) = emulator
                    .untag(arn, &[SEEDED.0, key.as_str()])
                    .await;
                assert_request_level_refusal(
                    &case,
                    "tagKeys",
                    RESERVED_TAG_KEY_PREFIX,
                    untag_status,
                    &untag_body,
                );
                assert_eq!(
                    validation_type(&untag_body),
                    "MEMBER_INVALID",
                    "case {case:?}: UntagResource expected MEMBER_INVALID, got {untag_body}"
                );

                // The two doors share the rule, so they cannot classify one bad request two
                // ways.
                assert_eq!(
                    tag_status, untag_status,
                    "case {case:?}: TagResource answered {tag_status} and UntagResource \
                     {untag_status} for the same bad request ({tag_body}, {untag_body})"
                );
                assert_eq!(
                    error_type(&tag_body),
                    error_type(&untag_body),
                    "case {case:?}: TagResource answered {tag_body} and UntagResource \
                     {untag_body} for the same bad request"
                );

                assert_seeded_tags_untouched(&emulator, target, &case).await;
            }
        }
    }
}

/// RULE, at the other refusal a tagging request earns on its own face: a submitted tag map
/// already past a published bound is past it whatever the addressed resource holds, so
/// `TagResource` answers it before the ARN's resource is resolved, at every taggable level.
#[tokio::test]
async fn a_submitted_tag_bound_outranks_the_arn_lookup_at_every_taggable_level() {
    let emulator = Emulator::new();
    let targets = live_targets(&emulator).await;

    for target in &targets {
        for existence in Existence::ALL {
            let arn = target.arn(existence);
            for bound in Bound::ALL {
                let case = format!("{bound:?} at {existence:?} {} ARN", target.kind);
                let (status, body) = emulator.tag(arn, &bound.tags()).await;
                assert_request_level_refusal(
                    &case,
                    "tags",
                    &bound.must_state(),
                    status,
                    &body,
                );
                assert_seeded_tags_untouched(&emulator, target, &case).await;
            }
        }
    }
}

/// Control, so neither verdict above can be a constant: a tagging request that breaks no
/// request-level rule IS judged by the lookup — the not-found class at an ARN naming an
/// absent resource, and accepted at a live one, where the write then shows up on the one
/// resource the ARN named.
#[tokio::test]
async fn a_tagging_request_with_nothing_wrong_in_it_is_judged_by_the_arn_lookup() {
    let emulator = Emulator::new();
    let targets = live_targets(&emulator).await;
    let ordinary = map_of(&[ORDINARY]);

    for target in &targets {
        // An ARN whose resource is absent: both writers report the tagging front end's
        // not-found, in the words it states it with.
        for (door, status, body) in [
            ("TagResource", emulator.tag(&target.absent, &ordinary).await),
            (
                "UntagResource",
                emulator.untag(&target.absent, &[ORDINARY.0]).await,
            ),
        ]
        .map(|(door, (status, body))| (door, status, body))
        {
            let case = format!("{door} at an absent {} ARN", target.kind);
            assert_eq!(
                status, 404,
                "case {case:?}: a clean request at an ARN naming nothing stored must be \
                 the not-found class, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the tagging front end's not-found sentence, got \
                 {body}"
            );
        }

        // The same clean request at the live ARN is accepted, and the merge is visible on
        // exactly that resource.
        let case = format!("TagResource at a live {} ARN", target.kind);
        let (status, body) = emulator.tag(&target.live, &ordinary).await;
        assert_eq!(
            status, 204,
            "case {case:?}: a clean request at a stored resource must be accepted, got \
             {status} {body}"
        );
        let (status, listed) = emulator.list_tags(&target.live).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} {listed}"
        );
        assert_eq!(
            tag_map(&listed, "tags"),
            map_of(&[SEEDED, ORDINARY]),
            "case {case:?}: the accepted write must be merged into the created tags, got \
             {listed}"
        );

        // And removing it again leaves the created tags, so the control is a round trip
        // rather than a one-way write.
        let case = format!("UntagResource at a live {} ARN", target.kind);
        let (status, body) = emulator.untag(&target.live, &[ORDINARY.0]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: a clean removal at a stored resource must be accepted, got \
             {status} {body}"
        );
        assert_seeded_tags_untouched(&emulator, target, &case).await;
    }
}
