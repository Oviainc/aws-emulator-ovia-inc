// SPDX-License-Identifier: CC0-1.0

//! The PRESENCE of an optional member in a create's idempotency fingerprint, driven over
//! HTTP through the same router the emulator boots.
//!
//! Every create in this service is `@idempotent` and binds `ClientToken` with
//! `@idempotencyToken` on `x-amzn-client-token`
//! (`smithy_query(query="operation", operation="CreateChannelGroup,CreateChannel,CreateOriginEndpoint")`),
//! so a repeat under the same token is answered from the attempt that token already
//! made: unchanged it replays, changed it is 409 `ConflictException` /
//! `IDEMPOTENT_PARAMETER_MISMATCH`.
//!
//! The rule this suite asserts: whether an OPTIONAL member was carried at all is part of
//! the request, so a repeat under the same token that adds a member the first attempt
//! omitted — or drops one it carried — is a CHANGED request, whatever value the member
//! holds, and is refused rather than replayed. It matters most where a member's own type
//! admits the value the service would have applied for it: `Description` is
//! `@length 0..1024` (so `""` is a value a request may carry) and
//! `StartoverWindowSeconds` is `@range 0..1209600` (so `0` is), while what a request omits
//! is materialised to the service's own default — an absent window becomes 900 seconds, so
//! an omitted member and a submitted `0` ask for two DIFFERENT endpoints.
//!
//! The rows span all three create levels, both directions (added and dropped), a member
//! whose default differs from the submitted value and one where the two materialise to the
//! same stored value, strings, enums and an integer, including the published bounds of the
//! integer's range.
//!
//! `control_plane_validation` covers the same three levels varying a PRESENT description,
//! and `harvest_job_idempotency` the fourth create; this suite is the presence-vs-absence
//! row of each.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// The hierarchy every row hangs its resource off, and the name of the resource under
/// test — all matching the model's `ResourceName` pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "opt-group";
const CHANNEL: &str = "opt-channel";
const RESOURCE: &str = "opt-resource";

/// The one token every attempt of a row carries: the identity of the attempt, which is
/// what makes the second request a repeat rather than a second create.
const TOKEN: &str = "fixed-client-token";

/// The sentence the service refuses a changed repeat with.
const MISMATCH_SENTENCE: &str = "You've specified a client token with your request using \
                                 parameter values that differ from a previous request that \
                                 used the same client token.";

/// The three levels a create exists at, each binding `ClientToken` the same way.
#[derive(Debug, Clone, Copy)]
enum Level {
    Group,
    Channel,
    Endpoint,
}

impl Level {
    /// The collection the create posts to.
    fn collection(self) -> String {
        match self {
            Self::Group => "/channelGroup".to_string(),
            Self::Channel => format!("/channelGroup/{GROUP}/channel"),
            Self::Endpoint => {
                format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint")
            }
        }
    }

    /// The member the created resource is named by.
    fn name_member(self) -> &'static str {
        match self {
            Self::Group => "ChannelGroupName",
            Self::Channel => "ChannelName",
            Self::Endpoint => "OriginEndpointName",
        }
    }

    /// The smallest acceptable body at this level, before a row adds its own members.
    fn base(self) -> Value {
        match self {
            Self::Group | Self::Channel => json!({}),
            Self::Endpoint => minimal_endpoint("TS"),
        }
    }
}

/// One row of the table: which member's PRESENCE the two attempts differ in.
struct Row {
    label: &'static str,
    level: Level,
    /// The member under test, as it travels on the wire.
    member: &'static str,
    /// What the first attempt carried for it; `None` means it left the member out.
    first: Option<Value>,
    /// What the repeat under the same token carries for it.
    repeat: Option<Value>,
    /// Members every attempt of the row carries, so the member under test is one this
    /// resource may hold at all (`OutputLockingMode` is only configurable on a CMAF or
    /// MULTIVIEW channel).
    fixed: &'static [(&'static str, &'static str)],
    /// Whether the two requests materialise to the SAME stored value for `member`. On a
    /// row where they do, the refusal is about the request the token was reused for, not
    /// about the record it would have produced.
    same_stored_value: bool,
}

/// The rows. The values come from the model: `Description` is `ResourceDescription`
/// (`@length 0..1024`), `StartoverWindowSeconds` is `@range 0..1209600`, and the enum
/// members are spelled with their own enum's values (`InputType` HLS/CMAF/MULTIVIEW,
/// `OutputLockingMode` EPOCH_LOCKED/NON_EPOCH_LOCKED, `UriSeparator` UNDERSCORE/HYPHEN,
/// `StreamNameOutputMode` INDEX/PASSTHROUGH_NAME).
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a channel group's description, absent then the empty string",
            level: Level::Group,
            member: "Description",
            first: None,
            repeat: Some(json!("")),
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "a channel group's description, the empty string then absent",
            level: Level::Group,
            member: "Description",
            first: Some(json!("")),
            repeat: None,
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "a channel's description, absent then the empty string",
            level: Level::Channel,
            member: "Description",
            first: None,
            repeat: Some(json!("")),
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "a channel's input type, absent then the default the service applies",
            level: Level::Channel,
            member: "InputType",
            first: None,
            repeat: Some(json!("HLS")),
            fixed: &[],
            same_stored_value: true,
        },
        Row {
            label: "a channel's output locking mode, absent then another mode",
            level: Level::Channel,
            member: "OutputLockingMode",
            first: None,
            repeat: Some(json!("NON_EPOCH_LOCKED")),
            fixed: &[("InputType", "CMAF")],
            same_stored_value: false,
        },
        Row {
            label: "an origin endpoint's description, absent then the empty string",
            level: Level::Endpoint,
            member: "Description",
            first: None,
            repeat: Some(json!("")),
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "an origin endpoint's startover window, absent then the range's minimum",
            level: Level::Endpoint,
            member: "StartoverWindowSeconds",
            first: None,
            repeat: Some(json!(0)),
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "an origin endpoint's startover window, the range's minimum then absent",
            level: Level::Endpoint,
            member: "StartoverWindowSeconds",
            first: Some(json!(0)),
            repeat: None,
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "an origin endpoint's startover window, absent then the range's maximum",
            level: Level::Endpoint,
            member: "StartoverWindowSeconds",
            first: None,
            repeat: Some(json!(1_209_600)),
            fixed: &[],
            same_stored_value: false,
        },
        Row {
            label: "an origin endpoint's startover window, absent then the default the \
                    service applies",
            level: Level::Endpoint,
            member: "StartoverWindowSeconds",
            first: None,
            repeat: Some(json!(900)),
            fixed: &[],
            same_stored_value: true,
        },
        Row {
            label: "an origin endpoint's URI separator, absent then the default the service \
                    applies",
            level: Level::Endpoint,
            member: "UriSeparator",
            first: None,
            repeat: Some(json!("UNDERSCORE")),
            fixed: &[],
            same_stored_value: true,
        },
        Row {
            label: "an origin endpoint's stream name output mode, absent then the other mode",
            level: Level::Endpoint,
            member: "StreamNameOutputMode",
            first: None,
            repeat: Some(json!("PASSTHROUGH_NAME")),
            fixed: &[],
            same_stored_value: false,
        },
    ]
}

/// A fresh emulator holding only the parents the row's level needs, so the resource under
/// test is the only one its create could match a token against.
async fn emulator_for(level: Level, case: &str) -> Emulator {
    let emulator = Emulator::new();
    if matches!(level, Level::Channel | Level::Endpoint) {
        let (status, body) = emulator.create_group(GROUP, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the parent group expected 200, got {status} {body}"
        );
    }
    if matches!(level, Level::Endpoint) {
        let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the parent channel expected 200, got {status} {body}"
        );
    }
    emulator
}

/// The row's request, carrying `value` for the member under test when it carries it at
/// all. Deterministic in its arguments, so two calls with the same `value` are two
/// byte-identical requests.
fn request(row: &Row, value: Option<&Value>) -> Value {
    let mut body = row.level.base();
    body[row.level.name_member()] = json!(RESOURCE);
    for (member, fixed) in row.fixed {
        body[*member] = json!(fixed);
    }
    if let Some(value) = value {
        body[row.member] = value.clone();
    }
    body
}

/// The row's create, under the one token the rule is about.
async fn create(emulator: &Emulator, level: Level, body: Value) -> (u16, Value) {
    emulator
        .call_with_headers(
            "POST",
            &level.collection(),
            Some(body),
            &[("x-amzn-client-token", TOKEN)],
        )
        .await
}

/// Read the resource under test back at `level`.
async fn read(emulator: &Emulator, level: Level) -> (u16, Value) {
    match level {
        Level::Group => emulator.get_group(RESOURCE).await,
        Level::Channel => emulator.get_channel(GROUP, RESOURCE).await,
        Level::Endpoint => emulator.get_endpoint(GROUP, CHANNEL, RESOURCE).await,
    }
}

/// The `ConflictExceptionType` discriminator of an error envelope.
fn conflict_type(body: &Value) -> &str {
    body.get("ConflictExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `Message` of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A repeat under the same token that differs only in WHETHER an optional member is
/// carried is the modelled parameter mismatch, and stores nothing; a byte-identical
/// repeat still replays.
#[tokio::test]
async fn a_repeat_that_adds_or_drops_an_optional_member_is_the_modelled_mismatch() {
    for row in rows() {
        let label = row.label;
        let emulator = emulator_for(row.level, label).await;

        let (status, first) = create(&emulator, row.level, request(&row, row.first.as_ref())).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        // The rule must not break a true retry: the SAME request under the same token is
        // still the same attempt, and is replayed verbatim.
        let (status, replay) = create(&emulator, row.level, request(&row, row.first.as_ref())).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an identical retry expected a 200 replay, got {status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {label:?}: an identical retry must replay the first response verbatim, got \
             {replay} where {first} was answered"
        );

        // The repeat differs from the first attempt only in whether it carries the member.
        let changed = request(&row, row.repeat.as_ref());
        assert_ne!(
            changed,
            request(&row, row.first.as_ref()),
            "case {label:?}: the row must actually change the request"
        );
        let (status, refused) = create(&emulator, row.level, changed.clone()).await;
        assert_eq!(
            status, 409,
            "case {label:?}: a changed repeat expected 409, got {status} {refused}"
        );
        assert_eq!(
            error_type(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            conflict_type(&refused),
            "IDEMPOTENT_PARAMETER_MISMATCH",
            "case {label:?}: expected IDEMPOTENT_PARAMETER_MISMATCH, got {refused}"
        );
        assert!(
            message(&refused).starts_with(MISMATCH_SENTENCE),
            "case {label:?}: expected the parameter-mismatch sentence, got {:?}",
            message(&refused)
        );

        // A refused repeat changes nothing: the resource still reports what the FIRST
        // attempt asked for, under the entity tag that attempt was answered with.
        let (status, stored) = read(&emulator, row.level).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first resource must still be readable, got {status} {stored}"
        );
        assert_eq!(
            stored[row.member], first[row.member],
            "case {label:?}: a refused repeat must leave {} alone, got {} where {} was stored",
            row.member, stored[row.member], first[row.member]
        );
        assert_eq!(
            stored["ETag"], first["ETag"],
            "case {label:?}: a refused repeat must not rotate the entity tag, got {} where {} \
             was answered",
            stored["ETag"], first["ETag"]
        );

        // The repeat is a perfectly good create on its own — sent first, into a store that
        // holds no such resource, it is accepted under that same token. That is what makes
        // the refusal above about the token it reused rather than about what it carried.
        let fresh = emulator_for(row.level, label).await;
        let (status, accepted) = create(&fresh, row.level, changed).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the changed request as a first create expected 200, got {status} \
             {accepted}"
        );
        if row.same_stored_value {
            assert_eq!(
                accepted[row.member], first[row.member],
                "case {label:?}: the two requests materialise to the same {}, got {} where {} \
                 was stored",
                row.member, accepted[row.member], first[row.member]
            );
        } else {
            assert_ne!(
                accepted[row.member], first[row.member],
                "case {label:?}: the two requests must store different values for {}, both \
                 reported {}",
                row.member, accepted[row.member]
            );
        }
    }
}
