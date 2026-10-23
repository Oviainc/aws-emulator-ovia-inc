// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's `@timestampFormat("date-time")` members.
//!
//! RULE: a structure member the model marks `@timestampFormat("date-time")` travels on
//! the restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING — never as the epoch
//! NUMBER the protocol's default timestamp encoding would produce — and the string names
//! exactly the instant the service holds for it.
//!
//! `smithy_query(query="shape", shape="EmailContact")` reports
//! `"timestampFormat": "date-time"` on `creationTime` and `updateTime` and on nothing
//! else in this service's surface, so `EmailContact` is where the rule bites — on every
//! path that reports one (`GetEmailContact`, and the `EmailContacts` list of
//! `ListEmailContacts`).
//!
//! The suite checks the rule twice over, at both ends of the mechanism that carries it:
//!
//! * on the WIRE, over a table of stored instants and both read paths, including the
//!   round trip back to the instant the store holds (so a well-formed string naming the
//!   wrong moment fails too);
//! * on the SCHEMA, because the member schema's `timestamp_format()` is the single thing
//!   the JSON codec consults (`vendor/smithy-schema-server/src/json.rs::write_timestamp`)
//!   — with the service's non-timestamp members as control rows.
//!
//! The harness (router, request signing, response parsing) is the one
//! `email_contacts.rs` already builds; this suite drives it rather than copying it.

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_notificationscontacts_notificationscontacts::ops::types::EmailContact;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::{
    self, contact_arn, EmailContactRecord, CONTACT_ID_LEN, STATUS_ACTIVE, STATUS_INACTIVE,
};

use crate::email_contacts::{
    activate, create_ok, emulator, get_ok, issued_code, list, send_code, Emulator,
};

/// The two members the model marks `@timestampFormat("date-time")`
/// (`smithy_query(query="shape", shape="EmailContact")`).
const DATE_TIME_MEMBERS: [&str; 2] = ["creationTime", "updateTime"];

/// Assert `member` of `contact` is the RFC 3339 date-time string the rule requires, and
/// answer the instant it names in epoch milliseconds.
///
/// The failure messages name the two ways the rule is broken: a non-string (which is what
/// the protocol default's epoch NUMBER is) and a string no date-time parser accepts.
fn date_time_millis(case: &str, contact: &Value, member: &str) -> i64 {
    let value = contact
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} is @required, got {contact:?}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             an RFC 3339 string and never the protocol-default epoch number, got {value:?}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} = {text:?} must parse as an RFC 3339 date-time: {error}")
    });
    // Smithy's `date-time` is RFC 3339 with a `Z` offset, so the two designators are part
    // of the form a generated SDK's parser is handed.
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case:?}: {member} must carry the T separator and the Z offset, got {text:?}"
    );
    parsed
        .to_millis()
        .unwrap_or_else(|error| panic!("case {case:?}: {member} = {text:?} is unrepresentable: {error}"))
}

/// Store one contact directly, so a row can pin an instant the wall clock will not hand
/// out. The store is what every read path answers from, so this is the same record a
/// create would have left — only the instants are chosen rather than observed.
fn store_contact(em: &Emulator, id: &str, status: &str, creation_millis: i64, update_millis: i64) -> String {
    assert_eq!(
        id.len(),
        CONTACT_ID_LEN,
        "a stored id must satisfy the modelled [a-z0-9]{{27}} segment, got {id:?}"
    );
    let arn = contact_arn(id);
    state::service_state(&*em.engine)
        .expect("the emulator's own engine must expose this service's store")
        .lock()
        .insert_contact(EmailContactRecord {
            arn: arn.clone(),
            name: format!("row-{id}"),
            address: format!("{id}@example.com"),
            status: status.to_string(),
            creation_time_millis: creation_millis,
            update_time_millis: update_millis,
            ..EmailContactRecord::default()
        });
    arn
}

/// The contact `arn` names, as `ListEmailContacts` reports it.
async fn listed(em: &Emulator, arn: &str) -> Value {
    let (status, body) = list(em, "").await;
    assert_eq!(status, 200, "ListEmailContacts must answer 200, got {body:?}");
    body.get("emailContacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListEmailContacts must return emailContacts, got {body:?}"))
        .iter()
        .find(|contact| contact.get("arn").and_then(Value::as_str) == Some(arn))
        .cloned()
        .unwrap_or_else(|| panic!("ListEmailContacts must report {arn}, got {body:?}"))
}

/// RULE on the wire, over the span of instants a stored contact can carry and over both
/// paths that report one.
///
/// The rows differ along what the rule generalises over: where the instant sits relative
/// to the epoch (before it, on it, far after it), whether it carries a fractional second
/// at all, whether that fraction ends in a zero digit, whether the two members hold the
/// same instant or different ones, and which status the record is in.
#[tokio::test]
async fn every_date_time_member_is_an_rfc_3339_string_naming_the_stored_instant() {
    let em = emulator();

    // (case, id, status, creationTime millis, updateTime millis)
    let rows: [(&str, &str, &str, i64, i64); 7] = [
        (
            "the Unix epoch itself",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaa",
            STATUS_INACTIVE,
            0,
            0,
        ),
        (
            "one millisecond before the epoch",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbb",
            STATUS_INACTIVE,
            -1,
            -1,
        ),
        (
            "a whole second, with no fractional part",
            "ccccccccccccccccccccccccccc",
            STATUS_ACTIVE,
            1_700_000_000_000,
            1_700_000_000_000,
        ),
        (
            "a millisecond fraction",
            "ddddddddddddddddddddddddddd",
            STATUS_ACTIVE,
            1_792_729_178_653,
            1_792_729_178_653,
        ),
        (
            "a fraction whose last digit is zero",
            "eeeeeeeeeeeeeeeeeeeeeeeeeee",
            STATUS_INACTIVE,
            1_792_729_178_650,
            1_792_729_178_650,
        ),
        (
            "an update strictly later than the creation",
            "fffffffffffffffffffffffffff",
            STATUS_ACTIVE,
            1_700_000_000_001,
            1_792_729_178_999,
        ),
        (
            "a far-future instant",
            "0123456789012345678901234zz",
            STATUS_INACTIVE,
            4_102_444_800_000,
            4_102_444_800_123,
        ),
    ];

    for (case, id, status, creation_millis, update_millis) in rows {
        let arn = store_contact(&em, id, status, creation_millis, update_millis);

        // Both read paths report the same structure through the same serializer, so the
        // rule is asserted on each of them.
        for (path, contact) in [
            ("GetEmailContact", get_ok(&em, &arn).await),
            ("ListEmailContacts", listed(&em, &arn).await),
        ] {
            let case = format!("{case} via {path}");
            for (member, expected) in DATE_TIME_MEMBERS
                .iter()
                .zip([creation_millis, update_millis])
            {
                let got = date_time_millis(&case, &contact, member);
                assert_eq!(
                    got, expected,
                    "case {case:?}: {member} must name the stored instant {expected} ms, \
                     got {got} ms from {:?}",
                    contact.get(member)
                );
            }
        }
    }
}

/// RULE on the wire for the instants the SERVICE itself mints: what a create stored and
/// what an activation moved are both date-time strings, on both read paths.
///
/// This is the end-to-end row the table above cannot cover, because the instants come
/// from the clock rather than from the test.
#[tokio::test]
async fn the_instants_a_create_and_an_activation_mint_are_date_time_strings() {
    let em = emulator();
    let arn = create_ok(&em, "wire.form", "wire.form@example.com").await;

    let created = get_ok(&em, &arn).await;
    let created_at = date_time_millis("after the create", &created, "creationTime");
    let updated_at = date_time_millis("after the create", &created, "updateTime");
    assert_eq!(
        created.get("status").and_then(Value::as_str),
        Some(STATUS_INACTIVE),
        "a new contact starts inactive, got {created:?}"
    );

    let (status, body) = send_code(&em, &arn).await;
    assert_eq!(status, 200, "SendActivationCode must answer 200, got {body:?}");
    let code = issued_code(&em, &arn).expect("SendActivationCode must store an activation code");
    let (status, body) = activate(&em, &arn, &code).await;
    assert_eq!(status, 200, "ActivateEmailContact must answer 200, got {body:?}");

    for (path, contact) in [
        ("GetEmailContact", get_ok(&em, &arn).await),
        ("ListEmailContacts", listed(&em, &arn).await),
    ] {
        let case = format!("after the activation via {path}");
        assert_eq!(
            date_time_millis(&case, &contact, "creationTime"),
            created_at,
            "case {case:?}: an activation must not move creationTime, got {contact:?}"
        );
        assert!(
            date_time_millis(&case, &contact, "updateTime") > updated_at,
            "case {case:?}: an activation must move updateTime past {updated_at} ms, \
             got {contact:?}"
        );
    }
}

/// RULE on the schema: every member the model marks `@timestampFormat("date-time")`
/// reports that format, and a member the model gives no `@timestampFormat` reports none —
/// which is what makes the codec write a string for the first kind and nothing special
/// for the rest.
///
/// This also pins `EmailContact`'s hand-written schema (see the note in
/// `src/ops/types/shared_types.rs`) against the model: the wire name and the
/// `member_index` of every member, in the order
/// `smithy_query(query="shape", shape="EmailContact")` lists them.
#[test]
fn the_email_contact_schema_matches_the_model_member_for_member() {
    // (member index, wire name, whether the model marks it @timestampFormat("date-time"))
    // — the model's own member list for `EmailContact`. The four non-timestamp members are
    // the control rows: they must report NO format, so the assertion is about the trait and
    // not about every member schema.
    let members: [(usize, &str, bool, &aws_smithy_schema::Schema); 6] = [
        (0, "arn", false, &EmailContact::M_ARN),
        (1, "name", false, &EmailContact::M_NAME),
        (2, "address", false, &EmailContact::M_ADDRESS),
        (3, "status", false, &EmailContact::M_STATUS),
        (4, "creationTime", true, &EmailContact::M_CREATION_TIME),
        (5, "updateTime", true, &EmailContact::M_UPDATE_TIME),
    ];

    for (index, wire_name, is_date_time, schema) in members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        if is_date_time {
            assert!(
                matches!(format, Some(TimestampFormat::DateTime)),
                "case {wire_name:?}: the model marks this member \
                 @timestampFormat(\"date-time\"), so its schema must carry that format, \
                 got {format:?}"
            );
        } else {
            assert!(
                format.is_none(),
                "case {wire_name:?}: the model gives this member no @timestampFormat, \
                 got {format:?}"
            );
        }
        assert_eq!(
            schema.member_index(),
            Some(index),
            "case {wire_name:?}: the member must sit at the model's index {index}, got {:?}",
            schema.member_index()
        );
        assert_eq!(
            schema.member_name(),
            Some(wire_name),
            "case {wire_name:?}: the member's wire name must be the model's, got {:?}",
            schema.member_name()
        );
    }

    // The structure's own members, read off `SCHEMA` rather than off the consts above, so
    // a member left out of the structure schema is caught too.
    let on_schema: Vec<Option<&str>> = EmailContact::SCHEMA
        .members()
        .iter()
        .map(|member| member.member_name())
        .collect();
    let expected: Vec<Option<&str>> = members
        .iter()
        .map(|(_, wire_name, _, _)| Some(*wire_name))
        .collect();
    assert_eq!(
        on_schema, expected,
        "EmailContact::SCHEMA must name every modelled member in order, got {on_schema:?}"
    );
}
