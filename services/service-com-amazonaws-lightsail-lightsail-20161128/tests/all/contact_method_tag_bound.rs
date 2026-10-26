// SPDX-License-Identifier: CC0-1.0

//! The tag list `CreateContactMethod` stores against a contact method, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: the `tags` of a create are caller data the service keeps
//! verbatim, bounded by the number of tags one resource can hold
//! (`crate::state::MAX_TAGS`) — so a list at or below that bound is stored whole
//! and reported by a later `GetContactMethods` in the same order, and a list past
//! it is refused with the modelled `InvalidInputException` (HTTP 400) whose
//! message names the member, the count the request carried and the bound, before
//! anything is written: the Region's contact method of that protocol is left
//! exactly as it was.
//!
//! The sibling `contact_method_notifications` suite owns the rest of the record a
//! create stores (its identity, status, location and destination); this one
//! asserts only the tag list and the bound on it.

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::{MAX_TAGS, SMS_SUPPORTED_REGIONS};

use crate::instance_lifecycle::{error_code, Emulator};

/// One protocol case: the `ContactProtocol` value, a destination that protocol
/// can reach, and a Region that serves it.
///
/// Both values the modelled `ContactProtocol` enum declares
/// (`smithy_query(query="shape",shape="ContactProtocol")`) are here, because the
/// tag list belongs to the contact method rather than to one protocol. The SMS
/// row is made in a Region that serves SMS — the first entry of the list this
/// service publishes — so the case is about the tags and not about the protocol's
/// Region restriction.
struct Protocol {
    protocol: &'static str,
    contact_endpoint: &'static str,
    /// A second destination the same protocol can reach, for the create that is
    /// refused: a refusal that stored anything would show up as this one.
    other_endpoint: &'static str,
    region: &'static str,
}

fn protocols() -> Vec<Protocol> {
    vec![
        Protocol {
            protocol: "Email",
            contact_endpoint: "first@example.com",
            other_endpoint: "second@example.com",
            region: "eu-central-1",
        },
        Protocol {
            protocol: "SMS",
            contact_endpoint: "+12065550100",
            other_endpoint: "+12065550199",
            region: SMS_SUPPORTED_REGIONS[0],
        },
    ]
}

/// The tag counts a create must accept: none at all, one, one short of the bound,
/// and the bound itself.
fn accepted_counts() -> Vec<(String, usize)> {
    vec![
        ("no tags at all".to_string(), 0),
        ("a single tag".to_string(), 1),
        ("one tag short of the bound".to_string(), MAX_TAGS - 1),
        (format!("the bound itself, {MAX_TAGS} tags"), MAX_TAGS),
    ]
}

/// The tag counts a create must refuse: one past the bound, a handful past it,
/// and an order of magnitude past it.
fn refused_counts() -> Vec<(String, usize)> {
    vec![
        ("one tag past the bound".to_string(), MAX_TAGS + 1),
        ("a handful past the bound".to_string(), MAX_TAGS + 9),
        (
            "an order of magnitude past the bound".to_string(),
            10 * MAX_TAGS,
        ),
    ]
}

/// `count` tags, in wire form. The first tag carries an empty value and the rest
/// carry one, so a case spans both value forms the modelled `Tag$value` admits.
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let value = if index == 0 {
                    String::new()
                } else {
                    format!("value-{index:03}")
                };
                json!({"key": format!("tag-{index:03}"), "value": value})
            })
            .collect(),
    )
}

/// The one contact method of `protocol` in `region`, or `None` when that Region
/// holds none.
async fn one(emulator: &Emulator, region: &str, protocol: &str) -> Option<Value> {
    let body = emulator
        .ok_at(
            None,
            Some(region),
            "GetContactMethods",
            json!({"protocols": [protocol]}),
        )
        .await;
    body["contactMethods"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("reading the {protocol} contact method in {region}: expected a contactMethods array, got {body:?}")
        })
        .first()
        .cloned()
}

/// Ask for the contact method `protocol`/`contact_endpoint` in `region` with
/// `count` tags, answering the HTTP status and body of the attempt.
async fn create(
    emulator: &Emulator,
    region: &str,
    protocol: &str,
    contact_endpoint: &str,
    count: usize,
) -> (u16, Value) {
    emulator
        .call_in(
            Some(region),
            "CreateContactMethod",
            json!({
                "protocol": protocol,
                "contactEndpoint": contact_endpoint,
                "tags": tag_list(count),
            }),
        )
        .await
}

/// RULE: a tag list at or below the bound one resource can hold is caller data
/// the create keeps verbatim, so a later read reports exactly the tags the
/// request carried, in the order it carried them.
#[tokio::test]
async fn a_tag_list_within_the_bound_is_stored_whole() {
    for subject in protocols() {
        for (count_case, count) in accepted_counts() {
            let case = format!("{} with {count_case}", subject.protocol);
            // One emulator per case: a Region holds ONE contact method per
            // protocol, so a later case would otherwise replace this one's.
            let emulator = Emulator::new();
            let (status, body) = create(
                &emulator,
                subject.region,
                subject.protocol,
                subject.contact_endpoint,
                count,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {count} tags to be accepted, got {status} {body:?}"
            );

            let method = one(&emulator, subject.region, subject.protocol)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the contact method to be reported")
                });
            assert_eq!(
                method["tags"],
                tag_list(count),
                "case {case:?}: expected the tags the request carried, in order, got {:?}",
                method["tags"]
            );
            assert_eq!(
                method["tags"].as_array().map(Vec::len),
                Some(count),
                "case {case:?}: expected {count} tags, got {:?}",
                method["tags"]
            );
            assert_eq!(
                method["contactEndpoint"].as_str(),
                Some(subject.contact_endpoint),
                "case {case:?}: expected the destination the request carried, got {method:?}"
            );
        }
    }
}

/// RULE: a tag list past the bound one resource can hold is refused with the
/// modelled `InvalidInputException` (HTTP 400), whose message names the member,
/// the count the request carried and the bound — and the refusal happens before
/// anything is written, so the Region's record of that protocol is untouched and
/// a Region that held none still holds none.
#[tokio::test]
async fn a_tag_list_past_the_bound_is_refused_and_writes_nothing() {
    for subject in protocols() {
        for (count_case, count) in refused_counts() {
            let case = format!("{} with {count_case}", subject.protocol);
            let emulator = Emulator::new();

            // First: the Region holds nothing of this protocol, so a refused
            // create must leave it holding nothing.
            let (status, body) = create(
                &emulator,
                subject.region,
                subject.protocol,
                subject.contact_endpoint,
                count,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {count} tags to be refused, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException on the wire, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default().to_string();
            assert!(
                message.contains("tags"),
                "case {case:?}: expected a message naming the member that is wrong, got \
                 {message:?}"
            );
            assert!(
                message.contains(&count.to_string()) && message.contains(&MAX_TAGS.to_string()),
                "case {case:?}: expected a message naming the {count} tags asked for and the \
                 bound of {MAX_TAGS}, got {message:?}"
            );
            assert!(
                one(&emulator, subject.region, subject.protocol)
                    .await
                    .is_none(),
                "case {case:?}: the refused create must store nothing"
            );

            // Then: the same refusal over a record that already exists leaves
            // that record — its destination and its own tags — exactly as it was.
            let (status, body) = create(
                &emulator,
                subject.region,
                subject.protocol,
                subject.contact_endpoint,
                1,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create within the bound to succeed, got \
                 {status} {body:?}"
            );
            let before = one(&emulator, subject.region, subject.protocol)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the contact method to be reported")
                });

            let (status, body) = create(
                &emulator,
                subject.region,
                subject.protocol,
                subject.other_endpoint,
                count,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {count} tags to be refused over an existing record, \
                 got {status} {body:?}"
            );
            let after = one(&emulator, subject.region, subject.protocol)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the existing contact method to survive")
                });
            assert_eq!(
                after, before,
                "case {case:?}: the refused create must not replace the record, got {after:?}"
            );
        }
    }
}
