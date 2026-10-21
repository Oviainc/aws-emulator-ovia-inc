// SPDX-License-Identifier: CC0-1.0

//! Unit-level pins under the Voice Connector slice: the wire format its shape declares,
//! the minted-id rule, and the pagination-token codec.
//!
//! These sit beside the end-to-end suite in `voice_connector_lifecycle.rs` and pin the
//! three rules that suite can only observe indirectly, so a regression names the rule it
//! broke rather than one HTTP body that changed.

use aws_smithy_schema::traits::TimestampFormat;
use service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::ops::types::VoiceConnector;
use service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::state::{
    decode_page_token, encode_page_token, ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState,
};

/// RULE: a timestamp member whose target declares `@timestampFormat("date-time")` is
/// carried on the wire as an ISO-8601 string, not as the protocol's default epoch
/// number.
///
/// Source: `smithy_query(query="shape", shape="VoiceConnector")` reports
/// `"timestampFormat": "date-time"` on both `CreatedTimestamp` and `UpdatedTimestamp`
/// (they target `Iso8601Timestamp`).
#[test]
fn both_timestamp_members_declare_the_iso8601_wire_format() {
    for (member, schema) in [
        ("CreatedTimestamp", &VoiceConnector::M_CREATED_TIMESTAMP),
        ("UpdatedTimestamp", &VoiceConnector::M_UPDATED_TIMESTAMP),
    ] {
        let format = schema.timestamp_format().map(|t| t.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {member:?}: expected the date-time (ISO-8601) format, got {format:?}"
        );
    }
}

/// The shape's member list is what the codec walks, so its size and each member's
/// name/index must match the model's `VoiceConnector` structure.
#[test]
fn the_shape_declares_every_modelled_member_in_order() {
    // Source: `smithy_query(query="shape", shape="VoiceConnector")`.
    let expected = [
        "VoiceConnectorId",
        "AwsRegion",
        "Name",
        "OutboundHostName",
        "RequireEncryption",
        "CreatedTimestamp",
        "UpdatedTimestamp",
        "VoiceConnectorArn",
        "IntegrationType",
        "NetworkType",
    ];
    let members = VoiceConnector::SCHEMA.members();
    assert_eq!(
        members.len(),
        expected.len(),
        "case \"member count\": expected {} members, got {}",
        expected.len(),
        members.len()
    );
    for (index, name) in expected.iter().enumerate() {
        let member = members[index];
        assert_eq!(
            member.member_name(),
            Some(*name),
            "case {name:?}: expected member {index} to be {name:?}, got {:?}",
            member.member_name()
        );
        assert_eq!(
            member.member_index(),
            Some(index),
            "case {name:?}: expected member index {index}, got {:?}",
            member.member_index()
        );
    }
}

/// RULE: every minted `VoiceConnectorId` satisfies the modelled id pattern and no two
/// mints collide.
#[test]
fn minted_ids_are_distinct_and_match_the_modelled_pattern() {
    let mut state = ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState::default();
    let mut seen = std::collections::BTreeSet::new();
    for n in 0..500 {
        let id = state.mint_voice_connector_id();
        // `^([a-z0-9]{21,22}|<uuid>)$`, short alternative.
        assert!(
            matches!(id.len(), 21 | 22)
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case \"mint {n}\": expected an id matching ^[a-z0-9]{{21,22}}$, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "case \"mint {n}\": expected a fresh id, got the repeated {id:?}"
        );
    }
}

/// RULE: a `NextToken` is opaque but exactly reversible — a token this service issued
/// decodes to the position it was issued for, and anything else is rejected rather than
/// read as a position.
#[test]
fn the_page_token_codec_round_trips_and_rejects_foreign_tokens() {
    // Rows differ along length, character class and internal punctuation.
    for id in [
        "a",
        "abcdef0123456789abcde",
        "abcdef0123456789abcdef",
        "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
        "has:a:colon",
        "",
    ] {
        let token = encode_page_token(id);
        assert_ne!(
            token, id,
            "case {id:?}: the token must be opaque, not the id itself"
        );
        assert_eq!(
            decode_page_token(&token).as_deref(),
            Some(id),
            "case {id:?}: expected the token to decode back to the id, got {:?}",
            decode_page_token(&token)
        );
    }

    for foreign in [
        "",
        "not-a-token",
        "zz",
        // Valid hex, but not a payload this service tagged.
        "6465616462656566",
    ] {
        assert_eq!(
            decode_page_token(foreign),
            None,
            "case {foreign:?}: expected a foreign token to be rejected, got {:?}",
            decode_page_token(foreign)
        );
    }
}
