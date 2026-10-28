// SPDX-License-Identifier: CC0-1.0

//! How far `DeleteConfigurationSet` reaches, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, label decoding, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: the delete removes the configuration-set record the path
//! names and everything configured UNDER it (its options, its tags, its event
//! destinations), and nothing else — it rewrites no other resource's record. An email
//! identity's default configuration set is a name a client wrote onto the identity and
//! `GetEmailIdentity` reports back as a plain `ConfigurationSetName` string
//! (`smithy_query(query="operation",operation="GetEmailIdentity")` models it with no
//! constraint tying it to a live set), so after the set is deleted every identity that
//! named it still reports that name verbatim — an inert name no set answers to — and only
//! a later `PutEmailIdentityConfigurationSetAttributes` changes it: one naming another set
//! replaces it, one carrying no name clears it. The same holds in the other direction:
//! deleting an email identity leaves the configuration set it named resolvable.
//!
//! The tables below span what the rule generalises over: the form of the deleted set's
//! name (one character, hyphens, underscores with a digit, the longest the documented
//! rule allows), what was configured under it (nothing, tags, several option objects, one
//! event destination, several), how many identities named it (none, one, several, every
//! identity spelling the sibling suite covers) and both identity kinds.

use serde_json::{json, Value};

use crate::configuration_set_event_destinations::{
    destination_cases, reported_destinations, EventDestinations,
};
use crate::configuration_set_sending_options::put_sending;
use crate::configuration_sets::{Reply, Ses};
use crate::email_identity_configuration_set_association::{
    reported_set, set_names, IdentityConfigurationSet, IDENTITIES,
};

/// The set every case keeps alongside the one it deletes, so each assertion separates
/// "the delete removed the set it named" from "the delete removed more than that".
const BYSTANDER_SET: &str = "bystander_set_1";

/// An identity every case associates with [`BYSTANDER_SET`], and one it leaves on no set
/// at all: neither ever names the set a case deletes, so what the delete left them with
/// is what it did to a record it had no business touching. Both spellings are outside the
/// sibling suite's table, so no case writes one identity twice.
const IDENTITY_ON_BYSTANDER_SET: &str = "bystander@example.net";
const IDENTITY_ON_NO_SET: &str = "unassociated.example.net";

/// One row: a configuration set to create and then delete, what is configured under it,
/// and which identities name it when the delete lands.
struct DeletionCase {
    case: &'static str,
    /// The `CreateConfigurationSet` body, which carries `name` as its set name.
    create_body: Value,
    name: String,
    /// Event destinations created under the set before the delete, as `(name,
    /// definition)` — taken from the sibling suite's table of modelled destination kinds.
    destinations: Vec<(&'static str, Value)>,
    /// The identities that name the set when it is deleted.
    referrers: Vec<&'static str>,
}

/// The rows of the deletion table.
fn deletion_cases() -> Vec<DeletionCase> {
    // The documented name forms, from the sibling association suite: one character,
    // hyphenated, underscored with a digit, and the longest `ConfigurationSetName` the
    // member's own documentation allows.
    let names = set_names();
    let destinations = destination_cases();
    vec![
        DeletionCase {
            case: "one-character name, nothing configured under it, three identities name it",
            create_body: json!({"ConfigurationSetName": names[0]}),
            name: names[0].clone(),
            destinations: Vec::new(),
            referrers: IDENTITIES[..3].to_vec(),
        },
        DeletionCase {
            case: "hyphenated name, tagged and with options, one identity names it",
            create_body: json!({
                "ConfigurationSetName": names[1],
                "TrackingOptions": {"CustomRedirectDomain": "click.example.com"},
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800},
                "SendingOptions": {"SendingEnabled": false},
                "Tags": [{"Key": "team", "Value": "marketing"}],
            }),
            name: names[1].clone(),
            destinations: vec![(destinations[0].name, destinations[0].definition.clone())],
            referrers: vec![IDENTITIES[0]],
        },
        DeletionCase {
            case: "underscored name with a digit, two event destinations, every identity \
                   spelling names it",
            create_body: json!({
                "ConfigurationSetName": names[2],
                "SuppressionOptions": {
                    "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                    "SuppressionScope": "ACCOUNT",
                },
            }),
            name: names[2].clone(),
            destinations: vec![
                (destinations[1].name, destinations[1].definition.clone()),
                (destinations[2].name, destinations[2].definition.clone()),
            ],
            referrers: IDENTITIES.to_vec(),
        },
        DeletionCase {
            case: "the longest allowed name, no identity names it",
            create_body: json!({"ConfigurationSetName": names[3]}),
            name: names[3].clone(),
            destinations: Vec::new(),
            referrers: Vec::new(),
        },
    ]
}

/// Create the row's set with everything configured under it, the bystander set, the
/// identities that name the row's set, one identity on the bystander set and one on no
/// set at all.
async fn set_up(ses: &Ses, row: &DeletionCase) {
    let created = ses.create_set(row.create_body.clone()).await;
    assert_eq!(
        created.status,
        200,
        "case {:?}: expected the configuration set to be created, got {} {}",
        row.case,
        created.status,
        created.message()
    );
    for (name, definition) in &row.destinations {
        let attached = ses
            .create_destination(&row.name, name, definition.clone())
            .await;
        assert_eq!(
            attached.status,
            200,
            "case {:?}: expected {name:?} to be attached to the set, got {} {}",
            row.case,
            attached.status,
            attached.message()
        );
    }
    let bystander = ses
        .create_set(json!({"ConfigurationSetName": BYSTANDER_SET}))
        .await;
    assert_eq!(
        bystander.status,
        200,
        "case {:?}: expected the bystander set to be created, got {} {}",
        row.case,
        bystander.status,
        bystander.message()
    );

    for identity in &row.referrers {
        ses.create_identity(identity).await;
        let put = ses.put_association(identity, Some(&row.name)).await;
        assert_eq!(
            put.status,
            200,
            "case {:?}: expected {identity:?} to be associated with the set, got {} {}",
            row.case,
            put.status,
            put.message()
        );
    }

    // The two identities the row never associates with the set it deletes.
    ses.create_identity(IDENTITY_ON_BYSTANDER_SET).await;
    let put = ses
        .put_association(IDENTITY_ON_BYSTANDER_SET, Some(BYSTANDER_SET))
        .await;
    assert_eq!(
        put.status,
        200,
        "case {:?}: expected the bystander identity to be associated, got {} {}",
        row.case,
        put.status,
        put.message()
    );
    ses.create_identity(IDENTITY_ON_NO_SET).await;
}

/// The members of an identity read that no clock transition owns, so two reads of an
/// untouched identity agree on them. The verification transition falls due on its own
/// span, which is why `VerificationStatus`, `VerifiedForSendingStatus`, the DKIM status
/// that moves with them and `VerificationInfo` are left out.
fn settled_identity_members(read: &Reply) -> Vec<(&'static str, Value)> {
    [
        "IdentityType",
        "FeedbackForwardingStatus",
        "Policies",
        "Tags",
        "MailFromAttributes",
        "ConfigurationSetName",
    ]
    .into_iter()
    .map(|member| (member, read.member(member)))
    .collect()
}

#[tokio::test]
async fn the_delete_removes_the_set_and_everything_configured_under_it() {
    for row in deletion_cases() {
        let ses = Ses::new();
        set_up(&ses, &row).await;

        let dropped = EventDestinations::delete_set(&ses, &row.name).await;
        assert_eq!(
            dropped.status,
            200,
            "case {:?}: expected 200 from DeleteConfigurationSet, got {} {}",
            row.case,
            dropped.status,
            dropped.message()
        );
        assert!(
            dropped.is_empty_body(),
            "case {:?}: DeleteConfigurationSetResponse declares no members, got {:?}",
            row.case,
            dropped.member("ConfigurationSetName")
        );

        // The record and everything under it are gone, so every call that resolves the
        // name is refused with the one message a missing set carries, and the delete is
        // not idempotent.
        let missing = format!("Configuration set <{}> does not exist.", row.name);
        for (call, reply) in [
            ("GetConfigurationSet", ses.get_set(&row.name).await),
            (
                "GetConfigurationSetEventDestinations",
                ses.get_destinations(&row.name).await,
            ),
            (
                "PutConfigurationSetSendingOptions",
                put_sending(&ses, &row.name, json!({"SendingEnabled": true})).await,
            ),
            (
                "DeleteConfigurationSet again",
                EventDestinations::delete_set(&ses, &row.name).await,
            ),
        ] {
            assert_eq!(
                reply.status,
                404,
                "case {:?}: expected 404 from {call} on the deleted set, got {} {}",
                row.case,
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                "NotFoundException",
                "case {:?}: expected NotFoundException from {call}, got {:?}",
                row.case,
                reply.error_code()
            );
            assert_eq!(
                reply.message(),
                missing,
                "case {:?}: expected {call} to answer {missing:?}, got {:?}",
                row.case,
                reply.message()
            );
        }

        // The bystander set kept its own record, with the destinations it never had.
        let kept = ses.get_set(BYSTANDER_SET).await;
        assert_eq!(
            (kept.status, kept.member("ConfigurationSetName")),
            (200, Value::String(BYSTANDER_SET.to_string())),
            "case {:?}: expected the bystander set to still resolve, got {} {}",
            row.case,
            kept.status,
            kept.message()
        );
        assert!(
            reported_destinations(&ses, BYSTANDER_SET).await.is_empty(),
            "case {:?}: expected the bystander set to hold no event destination",
            row.case
        );
    }
}

#[tokio::test]
async fn the_delete_leaves_every_identity_that_named_the_set_naming_it() {
    for row in deletion_cases() {
        let ses = Ses::new();
        set_up(&ses, &row).await;

        // What each identity record reported before the delete, so the comparison after
        // it is against the account's own state rather than a value written here.
        let mut before = Vec::new();
        for identity in &row.referrers {
            before.push((
                *identity,
                settled_identity_members(&ses.get_identity(identity).await),
            ));
        }

        let dropped = EventDestinations::delete_set(&ses, &row.name).await;
        assert_eq!(
            dropped.status,
            200,
            "case {:?}: expected the configuration set to be deleted, got {} {}",
            row.case,
            dropped.status,
            dropped.message()
        );

        for (identity, members) in &before {
            let read = ses.get_identity(identity).await;
            assert_eq!(
                read.status,
                200,
                "case {:?}: expected {identity:?} to still be readable, got {} {}",
                row.case,
                read.status,
                read.message()
            );
            // The name the client wrote survives the delete verbatim.
            assert_eq!(
                read.member("ConfigurationSetName"),
                Value::String(row.name.clone()),
                "case {:?}: expected {identity:?} to still report {:?}, got {:?}",
                row.case,
                row.name,
                read.member("ConfigurationSetName")
            );
            // And the delete rewrote nothing else on the identity either.
            assert_eq!(
                settled_identity_members(&read),
                *members,
                "case {:?}: expected the delete to leave {identity:?} as it was, got {:?} where it was {:?}",
                row.case,
                settled_identity_members(&read),
                members
            );
            // A read of an identity reports without changing, so a second read answers
            // the same name.
            assert_eq!(
                reported_set(&ses, identity, row.case).await,
                Value::String(row.name.clone()),
                "case {:?}: expected a second read of {identity:?} to report the same name",
                row.case
            );
        }

        // The identities that never named the deleted set are untouched as well.
        assert_eq!(
            reported_set(&ses, IDENTITY_ON_BYSTANDER_SET, row.case).await,
            Value::String(BYSTANDER_SET.to_string()),
            "case {:?}: expected {IDENTITY_ON_BYSTANDER_SET:?} to still report the bystander set",
            row.case
        );
        assert_eq!(
            reported_set(&ses, IDENTITY_ON_NO_SET, row.case).await,
            Value::Null,
            "case {:?}: expected {IDENTITY_ON_NO_SET:?} to report no configuration set",
            row.case
        );
    }
}

#[tokio::test]
async fn a_name_the_delete_left_behind_changes_only_when_the_client_writes_it() {
    // The two writes that may change it, against the surviving name: one naming another
    // set the account holds, and one naming no set at all.
    for row in deletion_cases().into_iter().filter(|row| !row.referrers.is_empty()) {
        for write in [Some(BYSTANDER_SET), None] {
            let ses = Ses::new();
            set_up(&ses, &row).await;
            EventDestinations::delete_set(&ses, &row.name).await;
            let case = format!("{:?} ({write:?})", row.case);

            for identity in &row.referrers {
                // Naming the deleted set again IS refused: this call resolves the name
                // it is given against the account's sets, and the refusal leaves the
                // surviving name where it was.
                let refused = ses.put_association(identity, Some(&row.name)).await;
                assert_eq!(
                    refused.status,
                    404,
                    "case {case}: expected naming the deleted set again to be refused for \
                     {identity:?}, got {} {}",
                    refused.status,
                    refused.message()
                );
                assert_eq!(
                    reported_set(&ses, identity, &case).await,
                    Value::String(row.name.clone()),
                    "case {case}: expected the refused write to leave {:?} on {identity:?}",
                    row.name
                );

                let written = ses.put_association(identity, write).await;
                assert_eq!(
                    written.status,
                    200,
                    "case {case}: expected the write on {identity:?} to be accepted, got {} {}",
                    written.status,
                    written.message()
                );
                let expected = match write {
                    Some(name) => Value::String(name.to_string()),
                    None => Value::Null,
                };
                assert_eq!(
                    reported_set(&ses, identity, &case).await,
                    expected,
                    "case {case}: expected {identity:?} to report {expected:?} after the write"
                );
            }
        }
    }
}

#[tokio::test]
async fn recreating_the_set_under_the_surviving_name_makes_it_live_again() {
    for row in deletion_cases().into_iter().filter(|row| !row.referrers.is_empty()) {
        let ses = Ses::new();
        set_up(&ses, &row).await;
        EventDestinations::delete_set(&ses, &row.name).await;

        // The surviving name is just a name, so creating a set under it again is a
        // create of a name the account does not hold — not a duplicate.
        let recreated = ses
            .create_set(json!({"ConfigurationSetName": row.name}))
            .await;
        assert_eq!(
            recreated.status,
            200,
            "case {:?}: expected the set to be creatable again, got {} {}",
            row.case,
            recreated.status,
            recreated.message()
        );
        // The new record carries none of the deleted one's event destinations.
        assert!(
            reported_destinations(&ses, &row.name).await.is_empty(),
            "case {:?}: expected the recreated set to hold no event destination",
            row.case
        );

        for identity in &row.referrers {
            assert_eq!(
                reported_set(&ses, identity, row.case).await,
                Value::String(row.name.clone()),
                "case {:?}: expected {identity:?} to still report {:?} once the set is back",
                row.case,
                row.name
            );
            // And the name resolves again, so a write naming it is accepted.
            let put = ses.put_association(identity, Some(&row.name)).await;
            assert_eq!(
                put.status,
                200,
                "case {:?}: expected naming the recreated set to be accepted for {identity:?}, \
                 got {} {}",
                row.case,
                put.status,
                put.message()
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_identity_leaves_the_set_it_named_alive() {
    for row in deletion_cases().into_iter().filter(|row| !row.referrers.is_empty()) {
        let ses = Ses::new();
        set_up(&ses, &row).await;

        for identity in &row.referrers {
            let dropped = ses.delete_identity(identity).await;
            assert_eq!(
                dropped.status,
                200,
                "case {:?}: expected {identity:?} to be deleted, got {} {}",
                row.case,
                dropped.status,
                dropped.message()
            );
            let read = ses.get_identity(identity).await;
            assert_eq!(
                read.status,
                404,
                "case {:?}: expected the deleted identity {identity:?} to be gone, got {} {}",
                row.case,
                read.status,
                read.message()
            );

            // The set the identity named is untouched by the identity going.
            let set = ses.get_set(&row.name).await;
            assert_eq!(
                (set.status, set.member("ConfigurationSetName")),
                (200, Value::String(row.name.clone())),
                "case {:?}: expected {:?} to still resolve after {identity:?} was deleted, got {} {}",
                row.case,
                row.name,
                set.status,
                set.message()
            );
            assert_eq!(
                reported_destinations(&ses, &row.name).await.len(),
                row.destinations.len(),
                "case {:?}: expected the set to keep its {} event destination(s)",
                row.case,
                row.destinations.len()
            );
        }
    }
}
