// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` over the wire: deleting named tags from a resource this service holds.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing of `DELETE /tags/{resourceArn}`, the `@httpQuery`
//! binding of `tagKeys`, the boundary constraint walk, the handler and the response
//! serialization all run exactly as they do in the emulator binary. Nothing reaches into
//! `crate::state` directly: a tag is only believed gone when a later
//! `ListTagsForResource` (or `GetApplication`) call stops reporting it.
//!
//! The rule under test: `UntagResource` removes from the set persisted for the resource
//! its ARN names exactly the keys the call names — a key the call does not name keeps
//! the value it had, and a key the resource does not hold removes nothing, so the
//! modelled `@idempotent` call leaves the same set however often it is repeated — and an
//! ARN naming no resource of this account and Region is refused as not found (HTTP 404)
//! with nothing changed.
//!
//! The tables span what that rule generalises over: how many keys the call names (one,
//! several, every key held), the relation between the keys named and the keys held
//! (a subset, all of them, disjoint, overlapping), key form across the modelled `TagKey`
//! character class, value length at both modelled bounds, the kind of resource the ARN
//! names (application, database), and the ways an ARN can name nothing (never
//! registered, deregistered, another Region, a database the registration never named,
//! another service's resource).

use std::collections::BTreeMap;

use serde_json::{json, Map};
use service_com_amazonaws_ssmsap_ssmsap::state::RESOURCE_NOT_FOUND_MESSAGE;

use crate::application_lifecycle::{settle, tag_pairs, Emulator, Reply};
use crate::resource_tags::{
    application_arn, database_arn, list_tags, list_tags_in_region, register, registration,
    tags_path, TAG_KEY_FORMS, TAG_VALUE_MAX_LENGTH,
};
use crate::tag_resource::{pairs, tag};
use crate::validation_message_rendering::message;

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a resource can be addressed at a Region it was not
/// registered in.
const OTHER_REGION: &str = "us-west-2";

/// The error code and status an absent resource is answered with: the class the
/// ARN-addressed tagging routes really carry, which is NOT the modelled
/// `ResourceNotFoundException` shape name.
const TAGGING_NOT_FOUND_CODE: &str =
    service_com_amazonaws_ssmsap_ssmsap::scaffolded::NOT_FOUND_CODE;

/// Percent-encode one query-parameter value.
///
/// The emulator decodes a query component as `application/x-www-form-urlencoded`, so
/// everything outside RFC 3986's unreserved set travels as `%XX` — a literal `+` in a
/// `TagKey` would otherwise arrive as a space.
fn query_encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// `UntagResource`'s URI for `resource_arn` and `tag_keys`: the modelled
/// `DELETE /tags/{resourceArn}`, whose `tagKeys` is a list bound to the query string, so
/// each key travels as its own `tagKeys=` parameter.
///
/// `pub(crate)` because the suites of this target share one harness: the suite that
/// pins which error channel each tagging call answers on
/// (`tests/all/tagging_error_channel.rs`) addresses this same route rather than
/// spelling the query binding a second time.
pub(crate) fn untag_path(resource_arn: &str, tag_keys: &[String]) -> String {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", query_encoded(key)))
        .collect();
    format!("{}?{}", tags_path(resource_arn), query.join("&"))
}

/// `UntagResource` for `resource_arn` at `region`'s endpoint. The operation binds every
/// member to the URI, so the request carries no body at all.
///
/// `pub(crate)` for the same reason [`untag`] is.
pub(crate) async fn untag_in_region(
    emulator: &Emulator,
    region: &str,
    resource_arn: &str,
    tag_keys: &[String],
) -> Reply {
    emulator
        .send_request("DELETE", region, &untag_path(resource_arn, tag_keys), None)
        .await
}

/// `UntagResource` for `resource_arn` at [`REGION`]'s endpoint.
///
/// `pub(crate)` because the suites of this target share one harness: the suite that
/// pins which ARNs this service resolves (`tests/all/component_arn_resolution.rs`)
/// deletes through this same route rather than spelling it a second time.
pub(crate) async fn untag(emulator: &Emulator, resource_arn: &str, tag_keys: &[String]) -> Reply {
    untag_in_region(emulator, REGION, resource_arn, tag_keys).await
}

/// The keys of `keys` as owned strings, so a row can be written literally.
fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// Assert the call was accepted, and that it answered with the EMPTY output the model
/// gives `UntagResourceResponse` rather than inventing a body for it.
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from UntagResource, got {reply:?}"
    );
    let members = reply.body.as_object().map_or(0, Map::len);
    assert_eq!(
        members, 0,
        "case {case:?}: UntagResourceResponse models no members, got {:?}",
        reply.body
    );
}

/// The tag set `ListTagsForResource` reports for `resource_arn`, asserting the read
/// itself was served.
async fn tags_held(emulator: &Emulator, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let reply = list_tags(emulator, resource_arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {reply:?}"
    );
    tag_pairs(reply.member("tags"))
}

/// One row of the removal table: what the resource holds, which keys the call names,
/// and what it must hold afterwards.
struct Case {
    /// How an assertion names the row.
    label: &'static str,
    application_id: &'static str,
    /// The tags the resource holds before the call under test.
    held: Vec<(String, String)>,
    /// The keys the `UntagResource` call names.
    removed: Vec<String>,
    /// The set the resource must hold once the call has been served.
    expected: Vec<(String, String)>,
}

/// The rows, spanning how many keys a call names and how they relate to the keys held,
/// plus key form across the modelled `TagKey` class and value length at both modelled
/// `TagValue` bounds.
fn cases() -> Vec<Case> {
    let three = pairs(&[("env", "prod"), ("owner", "sap-team"), ("tier", "1")]);
    // Every key the modelled class admits, as the `ListTagsForResource` suite reads them
    // off `TagKey`; the call names every other one, so both the removed and the kept
    // keys span the class.
    let key_forms: Vec<(String, String)> = TAG_KEY_FORMS
        .iter()
        .enumerate()
        .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
        .collect();
    let removed_forms: Vec<String> = key_forms
        .iter()
        .step_by(2)
        .map(|(key, _)| key.clone())
        .collect();
    let kept_forms: Vec<(String, String)> = key_forms
        .iter()
        .skip(1)
        .step_by(2)
        .cloned()
        .collect();

    vec![
        Case {
            label: "one key of several",
            application_id: "untag-one-of-several",
            held: three.clone(),
            removed: keys(&["owner"]),
            expected: pairs(&[("env", "prod"), ("tier", "1")]),
        },
        Case {
            label: "several keys at once",
            application_id: "untag-several",
            held: three.clone(),
            removed: keys(&["env", "tier"]),
            expected: pairs(&[("owner", "sap-team")]),
        },
        Case {
            label: "every key held",
            application_id: "untag-all",
            held: three.clone(),
            removed: keys(&["env", "owner", "tier"]),
            expected: Vec::new(),
        },
        Case {
            label: "a key the resource does not hold",
            application_id: "untag-absent",
            held: pairs(&[("env", "prod")]),
            removed: keys(&["owner"]),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "keys held and keys not held together",
            application_id: "untag-mixed",
            held: three,
            removed: keys(&["env", "nothing-holds-this"]),
            expected: pairs(&[("owner", "sap-team"), ("tier", "1")]),
        },
        Case {
            label: "keys spanning the modelled TagKey class",
            application_id: "untag-key-forms",
            held: key_forms,
            removed: removed_forms,
            expected: kept_forms,
        },
        Case {
            label: "a value at each modelled TagValue length bound",
            application_id: "untag-value-bounds",
            held: vec![
                ("shortest".to_string(), "x".to_string()),
                ("longest".to_string(), "v".repeat(TAG_VALUE_MAX_LENGTH)),
            ],
            removed: keys(&["shortest"]),
            expected: vec![("longest".to_string(), "v".repeat(TAG_VALUE_MAX_LENGTH))],
        },
    ]
}

/// RULE: a call removes exactly the keys it names from the set held for the resource its
/// ARN names — a key it does not name keeps its value, a key the resource does not hold
/// removes nothing — and every later read reports the remaining set.
#[tokio::test]
async fn a_call_removes_exactly_the_keys_it_names() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        register(
            &emulator,
            label,
            registration(case.application_id, &case.held, None),
        )
        .await;
        let arn = application_arn(REGION, case.application_id);

        // The set the registration persisted is the set the call under test acts on.
        let before = tags_held(&emulator, label, &arn).await;
        let held: BTreeMap<String, String> = case.held.iter().cloned().collect();
        assert_eq!(
            before, held,
            "case {label:?}: expected the registered set {held:?}, got {before:?}"
        );

        assert_accepted(label, &untag(&emulator, &arn, &case.removed).await);

        let expected: BTreeMap<String, String> = case.expected.iter().cloned().collect();
        let after = tags_held(&emulator, label, &arn).await;
        assert_eq!(
            after, expected,
            "case {label:?}: expected the remaining set {expected:?}, got {after:?}"
        );

        // One source of truth: the set this read reports is the set every other read of
        // the same application reports.
        let fetched = emulator
            .send(
                "/get-application",
                json!({ "ApplicationId": case.application_id }),
            )
            .await;
        let by_get_application = tag_pairs(fetched.member("Tags"));
        assert_eq!(
            by_get_application, expected,
            "case {label:?}: expected GetApplication to report the same tags {expected:?}, got \
             {by_get_application:?}"
        );

        // The modelled `@idempotent` call: naming the same keys again leaves the same
        // set, because the keys it names are no longer held.
        assert_accepted(label, &untag(&emulator, &arn, &case.removed).await);
        let repeated = tags_held(&emulator, label, &arn).await;
        assert_eq!(
            repeated, expected,
            "case {label:?}: expected the repeated call to leave {expected:?}, got {repeated:?}"
        );

        // And a key is only gone for good: tagging it again is reported again, so an
        // emptied resource is still a resource that holds tags.
        let restored = pairs(&[("after", "untagging")]);
        let reply = tag(&emulator, &arn, &restored).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200 from TagResource after untagging, got {reply:?}"
        );
        let mut with_restored = expected.clone();
        with_restored.insert("after".to_string(), "untagging".to_string());
        let held_again = tags_held(&emulator, label, &arn).await;
        assert_eq!(
            held_again, with_restored,
            "case {label:?}: expected {with_restored:?} after re-tagging, got {held_again:?}"
        );
    }
}

/// RULE: the tag set belongs to the RESOURCE the ARN names, so untagging one of a
/// registration's databases leaves the application's own set and the other databases'
/// sets alone.
#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let emulator = Emulator::new();

    // Rows: how many databases the registration names, which is what the rule
    // generalises over here.
    let rows: &[(&str, &str, &[&str])] = &[
        ("one database", "untag-db-one", &["HDB"]),
        (
            "three databases",
            "untag-db-three",
            &["SYSTEMDB", "TNT1", "TNT2"],
        ),
    ];

    let attached = pairs(&[("env", "prod"), ("owner", "sap-team")]);
    for (name, application_id, database_names) in rows {
        register(
            &emulator,
            name,
            registration(application_id, &attached, Some(database_names)),
        )
        .await;

        let app_arn = application_arn(REGION, application_id);
        for database_id in *database_names {
            let arn = database_arn(REGION, application_id, database_id);
            let reply = tag(&emulator, &arn, &attached).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}/{database_id:?}: expected 200 from TagResource, got {reply:?}"
            );
        }

        // Untag ONE database, by one of the two keys every resource of the registration
        // holds.
        let target = database_names[0];
        let target_arn = database_arn(REGION, application_id, target);
        let case = format!("{name}/{target}");
        assert_accepted(&case, &untag(&emulator, &target_arn, &keys(&["env"])).await);

        let expected_target: BTreeMap<String, String> = pairs(&[("owner", "sap-team")])
            .into_iter()
            .collect();
        let held = tags_held(&emulator, &case, &target_arn).await;
        assert_eq!(
            held, expected_target,
            "case {case:?}: expected the database's own remaining set {expected_target:?}, got \
             {held:?}"
        );

        let untouched: BTreeMap<String, String> = attached.iter().cloned().collect();
        let mut others = vec![("the application".to_string(), app_arn.clone())];
        for database_id in database_names.iter().skip(1) {
            others.push((
                (*database_id).to_string(),
                database_arn(REGION, application_id, database_id),
            ));
        }
        for (label, arn) in others {
            let still = tags_held(&emulator, &label, &arn).await;
            assert_eq!(
                still, untouched,
                "case {name:?}/{label:?}: expected the untouched set {untouched:?}, got {still:?}"
            );
        }
    }
}

/// RULE: an ARN that names no resource of this account and Region is refused as not
/// found (HTTP 404) with the fixed sentence that reports nothing of the ARN, and the
/// refused call changes nothing.
#[tokio::test]
async fn an_arn_naming_no_resource_here_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    let held = pairs(&[("env", "prod"), ("owner", "sap-team")]);
    register(
        &emulator,
        "a registered application",
        registration("untag-present", &held, Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "an application to deregister",
        registration("untag-gone", &held, None),
    )
    .await;
    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "untag-gone" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Deregistration is asynchronous: the application stops resolving when the removal
    // the clock owes it falls due.
    settle().await;

    // (label, the region the call is made in, the ARN it names)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an application never registered",
            REGION,
            application_arn(REGION, "untag-never-registered"),
        ),
        (
            "an application that was deregistered",
            REGION,
            application_arn(REGION, "untag-gone"),
        ),
        (
            "a registered application untagged at another regional endpoint",
            OTHER_REGION,
            application_arn(REGION, "untag-present"),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            application_arn(OTHER_REGION, "untag-present"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, "untag-present", "NOPE"),
        ),
        (
            "a database of an application that is not registered",
            REGION,
            database_arn(REGION, "untag-never-registered", "HDB"),
        ),
        (
            "another service's resource",
            REGION,
            "arn:aws:s3:::a-bucket/an-object".to_string(),
        ),
    ];

    for (label, region, arn) in rows {
        // What each regional endpoint reports for that ARN BEFORE the refused call. A
        // read of an ARN naming nothing is itself a 404, which reports no tags; the row
        // whose ARN names a resource at the OTHER endpoint reports that resource's own
        // set, and the refusal must leave it exactly as it is.
        let mut before: Vec<(&str, BTreeMap<String, String>)> = Vec::new();
        for read_region in [REGION, OTHER_REGION] {
            let reply = list_tags_in_region(&emulator, read_region, &arn).await;
            before.push((read_region, tag_pairs(reply.member("tags"))));
        }

        let reply = untag_in_region(&emulator, region, &arn, &keys(&["env"])).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected 404, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            TAGGING_NOT_FOUND_CODE,
            "case {label:?}: expected {TAGGING_NOT_FOUND_CODE}, got {:?}",
            reply.code()
        );
        let stated = message(&reply);
        assert_eq!(
            stated, RESOURCE_NOT_FOUND_MESSAGE,
            "case {label:?}: expected {RESOURCE_NOT_FOUND_MESSAGE:?}, got {stated:?}"
        );

        // Nothing changed for the ARN the refused call named, at either endpoint.
        for (read_region, expected) in before {
            let reply = list_tags_in_region(&emulator, read_region, &arn).await;
            let reported = tag_pairs(reply.member("tags"));
            assert_eq!(
                reported, expected,
                "case {label:?}: a refused call changes nothing, but {read_region} now reports \
                 {reported:?} instead of {expected:?}"
            );
        }
    }

    // And the resources that do exist still hold exactly what was attached to them: no
    // refused call touched them.
    let expected: BTreeMap<String, String> = held.iter().cloned().collect();
    let still = tags_held(
        &emulator,
        "the registered application",
        &application_arn(REGION, "untag-present"),
    )
    .await;
    assert_eq!(
        still, expected,
        "case \"the registered application\": expected {expected:?}, got {still:?}"
    );
}
