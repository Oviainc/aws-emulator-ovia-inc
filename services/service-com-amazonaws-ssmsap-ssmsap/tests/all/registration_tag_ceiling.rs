// SPDX-License-Identifier: CC0-1.0

//! The `Tags` a `RegisterApplication` call carries, written the one way this service
//! writes tags.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing of `POST /register-application`, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they
//! do in the emulator binary. Nothing reaches into `crate::state` directly: a tag is only
//! believed when a later `ListTagsForResource` or `GetApplication` call reports it back.
//!
//! The rule under test: a tag write is a tag write whichever operation carries it, so the
//! `Tags` of a registration are attached under the same per-resource ceiling as the tags
//! `TagResource` attaches — a call that would leave one resource holding more than
//! [`MAX_TAGS_PER_RESOURCE`] tags is refused with the modelled `ValidationException`
//! (HTTP 400) and persists nothing, so no accepted call can leave the store holding a
//! tag set a later tagging call would then be refused for.
//!
//! The table spans what that rule generalises over: how many tags one call carries,
//! from none through each side of the ceiling to well past it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_TAGS_PER_RESOURCE;

use crate::application_lifecycle::{tag_pairs, Emulator};
use crate::resource_tags::{application_arn, list_tags, registration};
use crate::tag_resource::{numbered_tags, pairs, tag};
use crate::validation_message_rendering::message;

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// The modelled error code a request outside the store's own per-resource tag ceiling is
/// answered with (`smithy_query(query="operation", operation="RegisterApplication")`:
/// `ValidationException` `@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One row: how many tags the registration carries, and whether that is a tag write this
/// service accepts.
struct Case {
    /// How an assertion names the row.
    label: &'static str,
    application_id: &'static str,
    /// How many tags the `RegisterApplication` body carries.
    count: usize,
    /// Whether the registration is accepted, which is whether the set it carries fits
    /// under the per-resource ceiling.
    accepted: bool,
}

/// The rows: tag counts either side of the ceiling, including none at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no tags",
            application_id: "reg-tags-none",
            count: 0,
            accepted: true,
        },
        Case {
            label: "one tag",
            application_id: "reg-tags-one",
            count: 1,
            accepted: true,
        },
        Case {
            label: "one tag below the ceiling",
            application_id: "reg-tags-below",
            count: MAX_TAGS_PER_RESOURCE - 1,
            accepted: true,
        },
        Case {
            label: "exactly at the ceiling",
            application_id: "reg-tags-at",
            count: MAX_TAGS_PER_RESOURCE,
            accepted: true,
        },
        Case {
            label: "one tag past the ceiling",
            application_id: "reg-tags-over-one",
            count: MAX_TAGS_PER_RESOURCE + 1,
            accepted: false,
        },
        Case {
            label: "ten tags past the ceiling",
            application_id: "reg-tags-over-ten",
            count: MAX_TAGS_PER_RESOURCE + 10,
            accepted: false,
        },
        Case {
            label: "twice the ceiling",
            application_id: "reg-tags-double",
            count: MAX_TAGS_PER_RESOURCE * 2,
            accepted: false,
        },
    ]
}

/// The `ApplicationId`s `ListApplications` reports at [`REGION`]'s endpoint.
async fn registered_ids(emulator: &Emulator) -> Vec<String> {
    let listed = emulator.send("/list-applications", json!({})).await;
    listed
        .member("Applications")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the `Tags` of a registration are attached under the same per-resource ceiling as
/// any other tag write, so a registration whose set fits is persisted whole and one that
/// would take the resource past the ceiling is refused with the modelled
/// `ValidationException` (HTTP 400) and persists nothing — neither the tags nor the
/// application.
#[tokio::test]
async fn a_registrations_tags_are_written_under_the_per_resource_ceiling() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let tags = numbered_tags(case.count);
        let arn = application_arn(REGION, case.application_id);
        let reply = emulator
            .send(
                "/register-application",
                registration(case.application_id, &tags, None),
            )
            .await;

        if case.accepted {
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected 200 from RegisterApplication, got {reply:?}"
            );

            // The set the registration carried is the set every read of that resource
            // reports.
            let expected: BTreeMap<String, String> = tags.iter().cloned().collect();
            let read = list_tags(&emulator, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: expected 200 from ListTagsForResource, got {read:?}"
            );
            let reported = tag_pairs(read.member("tags"));
            assert_eq!(
                reported, expected,
                "case {label:?}: expected the registered set of {} tags, got {} ({reported:?})",
                expected.len(),
                reported.len()
            );

            let fetched = emulator
                .send(
                    "/get-application",
                    json!({ "ApplicationId": case.application_id }),
                )
                .await;
            let by_get_application = tag_pairs(fetched.member("Tags"));
            assert_eq!(
                by_get_application, expected,
                "case {label:?}: expected GetApplication to report the same {} tags, got {} \
                 ({by_get_application:?})",
                expected.len(),
                by_get_application.len()
            );
            continue;
        }

        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            reply.code()
        );
        // The refusal states the ceiling, the resource it would be exceeded for, and the
        // number of tags the resource would have been left holding.
        let stated = message(&reply);
        for part in [
            arn.clone(),
            MAX_TAGS_PER_RESOURCE.to_string(),
            case.count.to_string(),
        ] {
            assert!(
                stated.contains(&part),
                "case {label:?}: expected the refusal to state {part:?}, got {stated:?}"
            );
        }

        // A refused call writes nothing: no tags for the ARN it named, and no
        // application either.
        let read = list_tags(&emulator, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: a refused registration persists no tags, got {read:?}"
        );
        let ids = registered_ids(&emulator).await;
        assert!(
            !ids.contains(&case.application_id.to_string()),
            "case {label:?}: a refused registration persists no application, but \
             ListApplications reports {ids:?}"
        );
    }
}

/// One row of the "a later `TagResource` at the ceiling" table.
struct CeilingCase {
    /// How an assertion names the row.
    label: &'static str,
    /// The tags the `TagResource` call carries.
    carried: Vec<(String, String)>,
    /// The status it is answered with: accepted when every key it carries is already
    /// held (so it adds none), refused when it would take the resource past the ceiling.
    status: u16,
}

/// RULE: an accepted tag write leaves the resource holding a set the tagging APIs
/// themselves accept, so a resource filled to the ceiling BY ITS REGISTRATION answers a
/// later `TagResource` exactly as one filled to the ceiling by `TagResource` does: a call
/// that only overwrites keys already held adds none and is accepted, and a call carrying
/// a new key is refused.
#[tokio::test]
async fn a_registration_at_the_ceiling_leaves_the_tagging_apis_able_to_overwrite() {
    let emulator = Emulator::new();

    let at_ceiling = numbered_tags(MAX_TAGS_PER_RESOURCE);
    let reply = emulator
        .send(
            "/register-application",
            registration("reg-tags-ceiling", &at_ceiling, None),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"registered at the ceiling\": expected 200, got {reply:?}"
    );
    let arn = application_arn(REGION, "reg-tags-ceiling");

    // Rows: what a later `TagResource` carries, and whether it adds a key — which is what
    // decides it at the ceiling.
    let rows: Vec<CeilingCase> = vec![
        CeilingCase {
            label: "overwriting one key already held",
            carried: pairs(&[("key-0", "rewritten")]),
            status: 200,
        },
        CeilingCase {
            label: "overwriting several keys already held",
            carried: pairs(&[("key-1", "rewritten"), ("key-2", "rewritten")]),
            status: 200,
        },
        CeilingCase {
            label: "adding one new key",
            carried: pairs(&[("extra", "x")]),
            status: 400,
        },
        CeilingCase {
            label: "overwriting a held key and adding a new one",
            carried: pairs(&[("key-0", "again"), ("extra", "x")]),
            status: 400,
        },
    ];

    for case in &rows {
        let label = case.label;
        // What the resource holds before the call, so an accepted row is pinned to the
        // merge it performs and a refused one to writing nothing at all.
        let before = tag_pairs(list_tags(&emulator, &arn).await.member("tags"));
        let mut expected = before.clone();
        if case.status == 200 {
            for (key, value) in &case.carried {
                expected.insert(key.clone(), value.clone());
            }
        }

        let reply = tag(&emulator, &arn, &case.carried).await;
        assert_eq!(
            reply.status, case.status,
            "case {label:?}: expected {}, got {}: {:?}",
            case.status, reply.status, reply.body
        );

        let held = tag_pairs(list_tags(&emulator, &arn).await.member("tags"));
        assert_eq!(
            held, expected,
            "case {label:?}: expected the resource to hold {expected:?}, got {held:?}"
        );
        assert_eq!(
            held.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {label:?}: expected the resource to keep holding {MAX_TAGS_PER_RESOURCE} \
             tags, got {}",
            held.len()
        );
    }

    // The accepted overwrites are what the resource now holds, under the unchanged
    // ceiling.
    let read = list_tags(&emulator, &arn).await;
    let held = tag_pairs(read.member("tags"));
    for key in ["key-0", "key-1", "key-2"] {
        assert_eq!(
            held.get(key).map(String::as_str),
            Some("rewritten"),
            "case \"after the overwrites\": expected {key:?} to hold the overwritten value, \
             got {:?}",
            held.get(key)
        );
    }
    assert!(
        !held.contains_key("extra"),
        "case \"after the overwrites\": a refused call adds no key, got {held:?}"
    );
}
