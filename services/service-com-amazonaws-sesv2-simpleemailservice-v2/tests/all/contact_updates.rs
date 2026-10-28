// SPDX-License-Identifier: CC0-1.0

//! `UpdateContact`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the harness the contact-list suite of this test binary
//! already owns (`crate::contact_lists::Ses`), so routing, deserialization, the handler
//! and serialization are all exercised, and the write is read back through
//! `GetContact`.
//!
//! The rule these cases pin: an update to a stored contact applies each member the
//! request BINDS to the member it names and leaves what it does not bind alone — a
//! supplied preference sets the status of the one topic it names, every other stated
//! preference keeping the status it had; `UnsubscribeAll` becomes the boolean the
//! request resolved to, overriding every per-topic status in force without erasing any
//! stated preference; and a supplied attribute document replaces the stored one whole,
//! since it is one opaque document. Attribute data is accepted only as a JSON document
//! of at most 1000 characters, a preference naming a topic the list does not publish is
//! refused, and a refusal writes nothing.
//!
//! The tables below span what the rule generalises over: the preferences stored and the
//! preferences supplied (none, one of several, one not yet stated, all of them in
//! another order), the attribute document (a flat object, a nested one, the character
//! bound and one past it, multi-byte characters, text that does not parse, nothing at
//! all), the ways a topic name can fail to name a topic of the list, and both resources
//! an update can fail to find.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::contact_lists::{listed_contacts, Reply, Ses};

/// The list every case seeds, and the topics it publishes.
const LIST: &str = "newsletter";

/// The contact every case seeds.
const CONTACT: &str = "reader@example.com";

/// The topics the seeded list publishes, each with the status it gives a contact that
/// has stated none. `Topic` requires `TopicName`, `DisplayName` and
/// `DefaultSubscriptionStatus`; `SubscriptionStatus` is `OPT_IN` or `OPT_OUT`.
const TOPICS: [&str; 3] = ["offers", "weekly", "deals"];

/// The message SES answers attribute data that is not a JSON document within the
/// bound with.
const INVALID_ATTRIBUTES: &str =
    "Attributes must be valid JSON and it can contain up to 1000 characters.";

/// A `TopicPreferences` body member from (topic, status) pairs.
fn preferences(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(topic, status)| json!({"TopicName": topic, "SubscriptionStatus": status}))
            .collect(),
    )
}

/// The preferences a `GetContact` reply reports the contact as having STATED, as a set
/// of (topic, status) pairs so a case pins the statuses rather than their order.
fn stated(reply: &Reply) -> BTreeSet<(String, String)> {
    reply
        .body
        .get("TopicPreferences")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry
                            .get("TopicName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("SubscriptionStatus")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The pairs a case expects, in the same form [`stated`] reports.
fn expected_set(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    pairs
        .iter()
        .map(|(topic, status)| (topic.to_string(), status.to_string()))
        .collect()
}

/// A service holding `LIST` with [`TOPICS`] published and `CONTACT` on it, carrying the
/// preferences and attribute data supplied.
async fn seeded(stored: &[(&str, &str)], attributes: Option<&str>) -> Ses {
    let ses = Ses::new();
    let created_list = ses
        .create_list(json!({
            "ContactListName": LIST,
            "Topics": TOPICS.iter().map(|topic| json!({
                "TopicName": topic,
                "DisplayName": format!("{topic} display"),
                "DefaultSubscriptionStatus": "OPT_OUT",
            })).collect::<Vec<Value>>(),
        }))
        .await;
    assert_eq!(
        created_list.status, 200,
        "seeding the list must succeed, got {} {:?}",
        created_list.status, created_list.body
    );
    let mut body = json!({
        "EmailAddress": CONTACT,
        "TopicPreferences": preferences(stored),
    });
    if let Some(attributes) = attributes {
        body.as_object_mut()
            .expect("the seed body is an object")
            .insert("AttributesData".to_string(), json!(attributes));
    }
    let created = ses.create_contact(LIST, body).await;
    assert_eq!(
        created.status, 200,
        "seeding the contact must succeed, got {} {:?}",
        created.status, created.body
    );
    ses
}

/// `UpdateContact` on the seeded list and contact.
async fn update(ses: &Ses, body: Value) -> Reply {
    ses.call(
        "PUT",
        &format!("/v2/email/contact-lists/{LIST}/contacts/{CONTACT}"),
        Some(body),
    )
    .await
}

/// One row of the merge table.
struct MergeCase {
    case: &'static str,
    /// The preferences the contact was created with.
    stored: &'static [(&'static str, &'static str)],
    /// The `TopicPreferences` the update binds, or `None` when it binds none at all.
    supplied: Option<&'static [(&'static str, &'static str)]>,
    /// The preferences the contact must have stated afterwards.
    expected: &'static [(&'static str, &'static str)],
}

/// The merge table: what the update supplies, against what the contact had stated.
const MERGE_CASES: &[MergeCase] = &[
    MergeCase {
        case: "nothing stated yet, one preference supplied",
        stored: &[],
        supplied: Some(&[("offers", "OPT_IN")]),
        expected: &[("offers", "OPT_IN")],
    },
    MergeCase {
        case: "one of several stated preferences flipped",
        stored: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT"), ("deals", "OPT_IN")],
        supplied: Some(&[("weekly", "OPT_IN")]),
        expected: &[("offers", "OPT_IN"), ("weekly", "OPT_IN"), ("deals", "OPT_IN")],
    },
    MergeCase {
        case: "no TopicPreferences member at all",
        stored: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
        supplied: None,
        expected: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
    },
    MergeCase {
        case: "an empty TopicPreferences list",
        stored: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
        supplied: Some(&[]),
        expected: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
    },
    MergeCase {
        case: "a topic the contact had stated nothing for",
        stored: &[("offers", "OPT_IN")],
        supplied: Some(&[("deals", "OPT_OUT")]),
        expected: &[("offers", "OPT_IN"), ("deals", "OPT_OUT")],
    },
    MergeCase {
        case: "every stated preference flipped, supplied in another order",
        stored: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
        supplied: Some(&[("weekly", "OPT_IN"), ("offers", "OPT_OUT")]),
        expected: &[("offers", "OPT_OUT"), ("weekly", "OPT_IN")],
    },
    MergeCase {
        case: "a status restated as the one already in force",
        stored: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
        supplied: Some(&[("offers", "OPT_IN")]),
        expected: &[("offers", "OPT_IN"), ("weekly", "OPT_OUT")],
    },
];

#[tokio::test]
async fn an_update_sets_the_topics_it_names_and_leaves_the_others_stated_as_they_were() {
    for case in MERGE_CASES {
        let ses = seeded(case.stored, Some(r#"{"tier":"silver"}"#)).await;
        let mut body = json!({});
        if let Some(supplied) = case.supplied {
            body.as_object_mut()
                .expect("the update body is an object")
                .insert("TopicPreferences".to_string(), preferences(supplied));
        }
        let updated = update(&ses, body).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateContact, got {} {:?}",
            case.case, updated.status, updated.body
        );

        let read = ses.get_contact(LIST, CONTACT).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetContact, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            stated(&read),
            expected_set(case.expected),
            "case {:?}: expected the stated preferences {:?}, got {:?}",
            case.case,
            case.expected,
            read.body.get("TopicPreferences")
        );
        // The members this update did not bind are untouched: the attribute document
        // the contact was created with, and the list's own default preferences.
        assert_eq!(
            read.text("AttributesData"),
            r#"{"tier":"silver"}"#,
            "case {:?}: an update binding no AttributesData must leave the stored document, got {:?}",
            case.case,
            read.body.get("AttributesData")
        );
        assert_eq!(
            read.body.get("UnsubscribeAll"),
            Some(&json!(false)),
            "case {:?}: UnsubscribeAll must stay false, got {:?}",
            case.case,
            read.body.get("UnsubscribeAll")
        );
    }
}

/// A JSON object of exactly `characters` characters, padded with `filler`.
///
/// `{"pad":""}` is ten characters, so the padding is the rest.
fn padded_document(characters: usize, filler: char) -> String {
    format!(
        "{{\"pad\":\"{}\"}}",
        std::iter::repeat_n(filler, characters - 10).collect::<String>()
    )
}

#[tokio::test]
async fn attribute_data_is_accepted_only_as_a_bounded_json_document_and_replaces_the_stored_one() {
    // (case, the document the update supplies, whether SES accepts it). The bound of
    // 1000 characters is the one the refusal states, so it is pinned at the bound and
    // one character past it.
    let cases: Vec<(&str, String, bool)> = vec![
        ("a flat object", r#"{"tier":"gold"}"#.to_string(), true),
        (
            "a nested object with a list in it",
            r#"{"tier":{"level":2,"since":"2024"},"tags":["a","b"]}"#.to_string(),
            true,
        ),
        (
            "a document of exactly the 1000-character bound",
            padded_document(1000, 'x'),
            true,
        ),
        (
            "a document one character past the bound",
            padded_document(1001, 'x'),
            false,
        ),
        (
            "1000 multi-byte characters, which is past the bound only in bytes",
            padded_document(1000, 'é'),
            true,
        ),
        (
            "an object that is cut off before it closes",
            r#"{"tier": "gold""#.to_string(),
            false,
        ),
        ("text that is not JSON at all", "tier=gold".to_string(), false),
        ("an empty document", String::new(), false),
    ];

    for (case, document, accepted) in &cases {
        let stored = r#"{"tier":"silver"}"#;
        let ses = seeded(&[("offers", "OPT_IN")], Some(stored)).await;
        let updated = update(
            &ses,
            json!({
                "AttributesData": document,
                "TopicPreferences": preferences(&[("weekly", "OPT_IN")]),
            }),
        )
        .await;
        let read = ses.get_contact(LIST, CONTACT).await;
        if *accepted {
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateContact, got {} {:?}",
                updated.status, updated.body
            );
            assert_eq!(
                &read.text("AttributesData"),
                document,
                "case {case:?}: the supplied document must replace the stored one, got {:?}",
                read.body.get("AttributesData")
            );
        } else {
            assert_eq!(
                (updated.status, updated.error_code().as_str()),
                (400, "BadRequestException"),
                "case {case:?}: an unacceptable document must be refused, got {} {:?}",
                updated.status,
                updated.body
            );
            assert_eq!(
                updated.message(),
                INVALID_ATTRIBUTES,
                "case {case:?}: expected the refusal {INVALID_ATTRIBUTES:?}, got {:?}",
                updated.message()
            );
            // A refusal writes NOTHING: neither the document nor the preferences the
            // same request carried.
            assert_eq!(
                read.text("AttributesData"),
                stored,
                "case {case:?}: a refused update must leave the stored document, got {:?}",
                read.body.get("AttributesData")
            );
            assert_eq!(
                stated(&read),
                expected_set(&[("offers", "OPT_IN")]),
                "case {case:?}: a refused update must leave the stated preferences, got {:?}",
                read.body.get("TopicPreferences")
            );
        }
    }

    // The same rule holds where attribute data first arrives, so a document SES would
    // not store on an update is not stored on a create either.
    let ses = seeded(&[], None).await;
    let refused = ses
        .create_contact(
            LIST,
            json!({"EmailAddress": "other@example.com", "AttributesData": r#"{"tier": "gold""#}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str(), refused.message()),
        (400, "BadRequestException", INVALID_ATTRIBUTES.to_string()),
        "case {:?}: expected the same refusal from CreateContact, got {} {:?}",
        "a create carrying an object that is cut off",
        refused.status,
        refused.body
    );
    let missing = ses.get_contact(LIST, "other@example.com").await;
    assert_eq!(
        (missing.status, missing.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: the refused create must have stored no contact, got {} {:?}",
        "a create carrying an object that is cut off",
        missing.status,
        missing.body
    );
}

#[tokio::test]
async fn unsubscribing_from_everything_overrides_every_stated_preference_without_erasing_it() {
    let ses = seeded(&[("offers", "OPT_IN"), ("weekly", "OPT_IN")], None).await;

    // (case, what the update binds, the UnsubscribeAll in force afterwards)
    let steps: Vec<(&str, Value, bool)> = vec![
        (
            "unsubscribed from everything",
            json!({"UnsubscribeAll": true}),
            true,
        ),
        (
            "unsubscribed from everything, restated with a preference",
            json!({"UnsubscribeAll": true, "TopicPreferences": preferences(&[("deals", "OPT_IN")])}),
            true,
        ),
        (
            "subscribed again, the member supplied as false",
            json!({"UnsubscribeAll": false}),
            false,
        ),
        (
            "the member omitted, which the model defaults to false",
            json!({"TopicPreferences": preferences(&[("weekly", "OPT_IN")])}),
            false,
        ),
    ];

    let mut expected: Vec<(&str, &str)> = vec![("offers", "OPT_IN"), ("weekly", "OPT_IN")];
    for (case, body, unsubscribed) in &steps {
        if case.contains("restated with a preference") {
            expected.push(("deals", "OPT_IN"));
        }
        let updated = update(&ses, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateContact, got {} {:?}",
            updated.status, updated.body
        );

        let read = ses.get_contact(LIST, CONTACT).await;
        assert_eq!(
            read.body.get("UnsubscribeAll"),
            Some(&json!(unsubscribed)),
            "case {case:?}: expected UnsubscribeAll {unsubscribed}, got {:?}",
            read.body.get("UnsubscribeAll")
        );
        // Whatever the contact is unsubscribed from, the preferences it stated are
        // still the ones it stated.
        assert_eq!(
            stated(&read),
            expected_set(&expected),
            "case {case:?}: expected the stated preferences {expected:?}, got {:?}",
            read.body.get("TopicPreferences")
        );

        // And the override is what a listing resolves a topic's status to: opted out of
        // everything while the flag is set, opted in by its own preference once it is
        // not.
        for status in ["OPT_IN", "OPT_OUT"] {
            let filtered = ses
                .list_contacts(
                    LIST,
                    json!({
                        "Filter": {
                            "TopicFilter": {"TopicName": "offers"},
                            "FilteredStatus": status,
                        }
                    }),
                )
                .await;
            assert_eq!(
                filtered.status, 200,
                "case {case:?}: expected HTTP 200 from ListContacts, got {} {:?}",
                filtered.status, filtered.body
            );
            let selects = (status == "OPT_OUT") == *unsubscribed;
            assert_eq!(
                listed_contacts(&filtered),
                if selects {
                    vec![CONTACT.to_string()]
                } else {
                    Vec::<String>::new()
                },
                "case {case:?}: a {status} filter on a stated OPT_IN must {} the contact, got {:?}",
                if selects { "select" } else { "skip" },
                filtered.body
            );
        }
    }
}

#[tokio::test]
async fn a_preference_naming_a_topic_the_list_does_not_publish_is_refused() {
    // (case, the topic name the update states a preference for)
    let cases: [(&str, &str); 4] = [
        ("a topic the list never published", "sports"),
        ("a topic name differing only in case from a published one", "Offers"),
        ("a topic named like the list itself", LIST),
        ("a topic named like a topic with trailing space", "offers "),
    ];
    for (case, topic) in cases {
        let ses = seeded(&[("offers", "OPT_IN")], Some(r#"{"tier":"silver"}"#)).await;
        let refused = update(
            &ses,
            json!({
                "TopicPreferences": preferences(&[(topic, "OPT_OUT")]),
                "AttributesData": r#"{"tier":"gold"}"#,
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(),
            format!("List: {LIST} doesn't contain Topic: {topic}"),
            "case {case:?}: the refusal must name the topic and the list, got {:?}",
            refused.message()
        );

        // Nothing the refused request carried was written.
        let read = ses.get_contact(LIST, CONTACT).await;
        assert_eq!(
            stated(&read),
            expected_set(&[("offers", "OPT_IN")]),
            "case {case:?}: a refused update must leave the stated preferences, got {:?}",
            read.body.get("TopicPreferences")
        );
        assert_eq!(
            read.text("AttributesData"),
            r#"{"tier":"silver"}"#,
            "case {case:?}: a refused update must leave the stored document, got {:?}",
            read.body.get("AttributesData")
        );
    }
}

#[tokio::test]
async fn updating_a_contact_nothing_was_stored_under_is_refused() {
    let ses = seeded(&[("offers", "OPT_IN")], None).await;

    // (case, the path the update is sent to, the sentence the refusal carries)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a list this account holds none of",
            format!("/v2/email/contact-lists/missing/contacts/{CONTACT}"),
            "List with name: missing doesn't exist.".to_string(),
        ),
        (
            "a contact the list does not hold",
            format!("/v2/email/contact-lists/{LIST}/contacts/nobody@example.com"),
            "nobody@example.com doesn't exist in List.".to_string(),
        ),
    ];
    for (case, path, message) in &cases {
        let refused = ses
            .call("PUT", path, Some(json!({"UnsubscribeAll": true})))
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected a 404 NotFoundException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            &refused.message(),
            message,
            "case {case:?}: expected the refusal {message:?}, got {:?}",
            refused.message()
        );
    }

    // An update that fails to find a contact creates none: the listing still reports
    // only the contact that was seeded.
    let listed = ses.list_contacts(LIST, json!({})).await;
    assert_eq!(
        listed_contacts(&listed),
        vec![CONTACT.to_string()],
        "case {:?}: a refused update must have stored no contact, got {:?}",
        "after both refusals",
        listed.body
    );
}
