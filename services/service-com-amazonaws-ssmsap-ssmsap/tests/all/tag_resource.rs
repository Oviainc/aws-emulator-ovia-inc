// SPDX-License-Identifier: CC0-1.0

//! `TagResource` over the wire: attaching tags to a resource this service holds.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing of `POST /tags/{resourceArn}`, the boundary constraint
//! walk, the handler and the response serialization all run exactly as they do in the
//! emulator binary. Nothing reaches into `crate::state` directly: a tag is only believed
//! when a later `ListTagsForResource` (or `GetApplication`) call reports it back.
//!
//! The rule under test: `TagResource` MERGES the tags it carries into the set persisted
//! for the resource its ARN names — a key the resource does not hold is added, a key it
//! holds takes the value the call carries, and a key the call does not name keeps the
//! value it had — so every later read reports that merged set; and an ARN naming no
//! resource of this account and Region is refused as not found (HTTP 404) with nothing
//! persisted.
//!
//! The tables span what that rule generalises over: tag count (one, several), key form
//! across the modelled `TagKey` character class, value length at both modelled bounds,
//! the kind of resource the ARN names (application, database), the relation between the
//! keys a call carries and the keys already held (disjoint, overlapping, identical,
//! none at all), repetition, and the ways an ARN can name nothing (never registered,
//! deregistered, another Region, a database the registration never named, another
//! service's resource).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::{MAX_TAGS_PER_RESOURCE, RESOURCE_NOT_FOUND_MESSAGE};

use crate::application_lifecycle::{settle, tag_pairs, Emulator, Reply};
use crate::resource_tags::{
    application_arn, database_arn, list_tags, list_tags_in_region, register, registration,
    tags_path, TAG_KEY_FORMS, TAG_VALUE_MAX_LENGTH,
};
use crate::validation_message_rendering::message;

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a resource can be tagged at a Region it was not
/// registered in.
const OTHER_REGION: &str = "us-west-2";

/// The error code and status an absent resource is answered with: the class the
/// ARN-addressed tagging routes really carry, which is NOT the modelled
/// `ResourceNotFoundException` shape name.
const TAGGING_NOT_FOUND_CODE: &str =
    service_com_amazonaws_ssmsap_ssmsap::scaffolded::NOT_FOUND_CODE;

/// The modelled error code and status a request outside the store's own ceiling is
/// answered with (`smithy_query(query="operation", operation="TagResource")`:
/// `ValidationException` `@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `tags` body member of a `TagResource` request: the model names the member
/// `tags` and declares no `jsonName` for it.
pub(crate) fn tags_body(tags: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), json!(value));
    }
    json!({ "tags": Value::Object(map) })
}

/// `TagResource` for `resource_arn` at `region`'s endpoint: the modelled
/// `POST /tags/{resourceArn}`.
///
/// `pub(crate)` because the suites of this target share one harness: the
/// `UntagResource` suite (`tests/all/untag_resource.rs`) attaches the tags it then
/// deletes through this same route.
pub(crate) async fn tag_in_region(
    emulator: &Emulator,
    region: &str,
    resource_arn: &str,
    tags: &[(String, String)],
) -> Reply {
    emulator
        .send_request(
            "POST",
            region,
            &tags_path(resource_arn),
            Some(tags_body(tags)),
        )
        .await
}

/// `TagResource` for `resource_arn` at [`REGION`]'s endpoint.
pub(crate) async fn tag(
    emulator: &Emulator,
    resource_arn: &str,
    tags: &[(String, String)],
) -> Reply {
    tag_in_region(emulator, REGION, resource_arn, tags).await
}

/// Assert the call was accepted, and that it answered with the EMPTY output the model
/// gives `TagResourceResponse` rather than inventing a body for it.
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from TagResource, got {reply:?}"
    );
    let members = reply.body.as_object().map_or(0, Map::len);
    assert_eq!(
        members, 0,
        "case {case:?}: TagResourceResponse models no members, got {:?}",
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

/// A `(key, value)` list from borrowed pairs, so a row can be written literally.
pub(crate) fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct tags, keyed from the modelled `TagKey` character class
/// (`smithy_query(query="shape", shape="TagKey")`: letters, digits and `-` are all in it)
/// and valued inside the modelled `TagValue` length — a set of a size a row chooses.
///
/// `pub(crate)` because the suites of this target share one harness: the registration
/// suite (`tests/all/registration_tag_ceiling.rs`) fills a resource to the same ceiling
/// through `RegisterApplication$Tags`, and both must fill it with the same set.
pub(crate) fn numbered_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// One row of the "what a call persists" table.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// The tags the call carries, which is the set the resource must then hold.
    tags: Vec<(String, String)>,
}

/// One row of the merge table: what the resource already holds, what the call carries,
/// and what it must hold afterwards.
struct MergeCase<'a> {
    /// How an assertion names the row.
    label: &'a str,
    application_id: &'a str,
    /// The tags the registration persisted, before the call under test.
    held: &'a [(&'a str, &'a str)],
    /// The tags the `TagResource` call carries.
    carried: &'a [(&'a str, &'a str)],
    /// The set the resource must hold once the call has been served.
    expected: &'a [(&'a str, &'a str)],
}

/// The rows: one registration each, spanning tag count, key form and value length.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one tag",
            application_id: "tag-one",
            tags: pairs(&[("env", "dev")]),
        },
        Case {
            name: "three tags",
            application_id: "tag-three",
            tags: pairs(&[("env", "prod"), ("owner", "sap-team"), ("tier", "1")]),
        },
        Case {
            name: "keys spanning the modelled TagKey class",
            application_id: "tag-key-forms",
            // The key catalogue the `ListTagsForResource` suite reads off the modelled
            // `TagKey` character class, so both suites test the same class.
            tags: TAG_KEY_FORMS
                .iter()
                .enumerate()
                .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
                .collect(),
        },
        Case {
            name: "a value at each modelled TagValue length bound",
            application_id: "tag-value-bounds",
            tags: vec![
                ("shortest".to_string(), "x".to_string()),
                ("longest".to_string(), "v".repeat(TAG_VALUE_MAX_LENGTH)),
            ],
        },
    ]
}

/// RULE: the tags a `TagResource` call carries are persisted for the resource its ARN
/// names, so every later read of that resource reports them.
#[tokio::test]
async fn the_tags_a_call_carries_are_persisted_for_the_resource_it_names() {
    let emulator = Emulator::new();

    for case in cases() {
        register(
            &emulator,
            case.name,
            registration(case.application_id, &[], None),
        )
        .await;
        let arn = application_arn(REGION, case.application_id);

        // Nothing has tagged it yet, so the read that follows the call reports exactly
        // what the call attached.
        let before = tags_held(&emulator, case.name, &arn).await;
        assert!(
            before.is_empty(),
            "case {:?}: an untagged registration holds no tags, got {before:?}",
            case.name
        );

        let reply = tag(&emulator, &arn, &case.tags).await;
        assert_accepted(case.name, &reply);

        let expected: BTreeMap<String, String> = case.tags.iter().cloned().collect();
        let held = tags_held(&emulator, case.name, &arn).await;
        assert_eq!(
            held, expected,
            "case {:?}: expected the tagged set {expected:?}, got {held:?}",
            case.name
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
            "case {:?}: expected GetApplication to report the same tags {expected:?}, got \
             {by_get_application:?}",
            case.name
        );
    }
}

/// RULE: the tag set belongs to the RESOURCE the ARN names, so tagging one of a
/// registration's databases leaves the application's own set alone, and the other way
/// round.
#[tokio::test]
async fn each_resource_holds_the_set_tagged_on_its_own_arn() {
    let emulator = Emulator::new();

    // Rows: how many databases the registration names, which is what the rule
    // generalises over here.
    let rows: &[(&str, &str, &[&str])] = &[
        ("one database", "tag-db-one", &["HDB"]),
        ("three databases", "tag-db-three", &["SYSTEMDB", "TNT1", "TNT2"]),
    ];

    for (name, application_id, database_names) in rows {
        register(
            &emulator,
            name,
            registration(application_id, &[], Some(database_names)),
        )
        .await;

        let app_arn = application_arn(REGION, application_id);
        let app_tags = pairs(&[("scope", "application")]);
        assert_accepted(name, &tag(&emulator, &app_arn, &app_tags).await);

        for database_id in *database_names {
            let arn = database_arn(REGION, application_id, database_id);
            let database_tags = pairs(&[("scope", "database"), ("db", database_id)]);
            let case = format!("{name}/{database_id}");
            assert_accepted(&case, &tag(&emulator, &arn, &database_tags).await);

            let held = tags_held(&emulator, &case, &arn).await;
            let expected: BTreeMap<String, String> = database_tags.iter().cloned().collect();
            assert_eq!(
                held, expected,
                "case {case:?}: expected the database's own set {expected:?}, got {held:?}"
            );
        }

        // Tagging the databases changed nothing about the application's own set.
        let held = tags_held(&emulator, name, &app_arn).await;
        let expected: BTreeMap<String, String> = app_tags.iter().cloned().collect();
        assert_eq!(
            held, expected,
            "case {name:?}: expected the application to keep its own set {expected:?}, got \
             {held:?}"
        );
    }
}

/// RULE: a call MERGES into the set already held — a key not held is added, a key held
/// takes the new value, and a key the call does not name keeps the value it had.
#[tokio::test]
async fn a_call_merges_into_the_set_already_held() {
    let emulator = Emulator::new();

    // Rows spanning the relation between the keys a call carries and the keys already
    // held: disjoint, overlapping, identical, and a call naming no key at all.
    let rows: &[MergeCase] = &[
        MergeCase {
            label: "a key the resource does not hold is added",
            application_id: "merge-add",
            held: &[("env", "dev")],
            carried: &[("owner", "sap-team")],
            expected: &[("env", "dev"), ("owner", "sap-team")],
        },
        MergeCase {
            label: "a key the resource holds takes the new value",
            application_id: "merge-overwrite",
            held: &[("env", "dev")],
            carried: &[("env", "prod")],
            expected: &[("env", "prod")],
        },
        MergeCase {
            label: "an overlapping call overwrites and adds at once",
            application_id: "merge-mixed",
            held: &[("env", "dev"), ("tier", "1")],
            carried: &[("env", "prod"), ("owner", "sap-team")],
            expected: &[("env", "prod"), ("owner", "sap-team"), ("tier", "1")],
        },
        MergeCase {
            label: "the same pairs again leave the set as it was",
            application_id: "merge-same",
            held: &[("env", "dev"), ("tier", "1")],
            carried: &[("env", "dev"), ("tier", "1")],
            expected: &[("env", "dev"), ("tier", "1")],
        },
        MergeCase {
            label: "a call carrying no tags changes nothing",
            application_id: "merge-empty",
            held: &[("env", "dev")],
            carried: &[],
            expected: &[("env", "dev")],
        },
    ];

    for case in rows {
        let label = case.label;
        register(
            &emulator,
            label,
            registration(case.application_id, &pairs(case.held), None),
        )
        .await;
        let arn = application_arn(REGION, case.application_id);
        let carried = pairs(case.carried);
        let expected: BTreeMap<String, String> = pairs(case.expected).into_iter().collect();

        assert_accepted(label, &tag(&emulator, &arn, &carried).await);
        let after = tags_held(&emulator, label, &arn).await;
        assert_eq!(
            after, expected,
            "case {label:?}: expected the merged set {expected:?}, got {after:?}"
        );

        // Merging is idempotent: the same call made again leaves the same set.
        assert_accepted(label, &tag(&emulator, &arn, &carried).await);
        let repeated = tags_held(&emulator, label, &arn).await;
        assert_eq!(
            repeated, expected,
            "case {label:?}: expected the repeated call to leave {expected:?}, got {repeated:?}"
        );
    }
}

/// RULE: an ARN that names no resource of this account and Region is refused as not
/// found (HTTP 404) with the fixed sentence that reports nothing of the ARN, and the
/// refused call persists nothing.
#[tokio::test]
async fn an_arn_naming_no_resource_here_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    let held = pairs(&[("env", "dev")]);
    register(
        &emulator,
        "a registered application",
        registration("tag-present", &held, Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "an application to deregister",
        registration("tag-gone", &held, None),
    )
    .await;
    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "tag-gone" }),
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
            application_arn(REGION, "tag-never-registered"),
        ),
        (
            "an application that was deregistered",
            REGION,
            application_arn(REGION, "tag-gone"),
        ),
        (
            "a registered application tagged at another regional endpoint",
            OTHER_REGION,
            application_arn(REGION, "tag-present"),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            application_arn(OTHER_REGION, "tag-present"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, "tag-present", "NOPE"),
        ),
        (
            "a database of an application that is not registered",
            REGION,
            database_arn(REGION, "tag-never-registered", "HDB"),
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

        let reply = tag_in_region(&emulator, region, &arn, &pairs(&[("added", "no")])).await;
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
                "case {label:?}: a refused call persists nothing, but {read_region} now reports \
                 {reported:?} instead of {expected:?}"
            );
        }
    }

    // And the application that does exist still holds exactly what its registration
    // persisted: no refused call touched it.
    let expected: BTreeMap<String, String> = held.iter().cloned().collect();
    let still = tags_held(
        &emulator,
        "the registered application",
        &application_arn(REGION, "tag-present"),
    )
    .await;
    assert_eq!(
        still, expected,
        "case \"the registered application\": expected {expected:?}, got {still:?}"
    );
}

/// RULE: the store holds at most [`MAX_TAGS_PER_RESOURCE`] tags for one resource; a
/// call that would leave it holding more is refused with the modelled
/// `ValidationException` (HTTP 400) and persists nothing — and a call that only
/// overwrites keys already held is accepted at the ceiling, because it adds none.
#[tokio::test]
async fn the_per_resource_tag_ceiling_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    register(
        &emulator,
        "a registration to fill up",
        registration("tag-ceiling", &[], None),
    )
    .await;
    let arn = application_arn(REGION, "tag-ceiling");

    // One key per tag the store will hold, drawn from the modelled `TagKey` class.
    let at_ceiling = numbered_tags(MAX_TAGS_PER_RESOURCE);
    assert_accepted("filled to the ceiling", &tag(&emulator, &arn, &at_ceiling).await);
    let expected: BTreeMap<String, String> = at_ceiling.iter().cloned().collect();
    let held = tags_held(&emulator, "filled to the ceiling", &arn).await;
    assert_eq!(
        held, expected,
        "case \"filled to the ceiling\": expected {} tags, got {}",
        expected.len(),
        held.len()
    );

    // Overwriting keys already held adds nothing, so it is accepted AT the ceiling.
    let overwrite = pairs(&[("key-0", "rewritten")]);
    assert_accepted("overwriting at the ceiling", &tag(&emulator, &arn, &overwrite).await);
    let held = tags_held(&emulator, "overwriting at the ceiling", &arn).await;
    assert_eq!(
        held.get("key-0").map(String::as_str),
        Some("rewritten"),
        "case \"overwriting at the ceiling\": expected the new value, got {:?}",
        held.get("key-0")
    );
    assert_eq!(
        held.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"overwriting at the ceiling\": expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        held.len()
    );

    // Rows: how far past the ceiling the call would take the resource — one new key,
    // and a batch of them alongside a key already held.
    let rows: &[(&str, Vec<(String, String)>)] = &[
        ("one key past the ceiling", pairs(&[("one-too-many", "x")])),
        (
            "several new keys past the ceiling",
            vec![
                ("key-0".to_string(), "rewritten".to_string()),
                ("extra-a".to_string(), "x".to_string()),
                ("extra-b".to_string(), "y".to_string()),
            ],
        ),
    ];
    let before = tags_held(&emulator, "before the refusals", &arn).await;
    for (label, carried) in rows {
        let reply = tag(&emulator, &arn, carried).await;
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
        let after = tags_held(&emulator, label, &arn).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused call persists nothing, expected {before:?}, got {after:?}"
        );
    }
}

/// RULE: a tag is held FOR a resource, so it cannot outlive the registration that
/// resource came from — neither the application's own tags nor those of the databases
/// derived from it.
#[tokio::test]
async fn tags_do_not_outlive_the_registration_they_were_attached_to() {
    let emulator = Emulator::new();

    // Rows: the kinds of resource a registration brings with it, which is what the rule
    // generalises over here.
    let databases: &[&str] = &["HDB", "SYSTEMDB"];
    register(
        &emulator,
        "a registration to deregister",
        registration("tag-lifetime", &[], Some(databases)),
    )
    .await;

    let app_arn = application_arn(REGION, "tag-lifetime");
    assert_accepted(
        "the application",
        &tag(&emulator, &app_arn, &pairs(&[("env", "dev")])).await,
    );
    for database_id in databases {
        let arn = database_arn(REGION, "tag-lifetime", database_id);
        assert_accepted(
            database_id,
            &tag(&emulator, &arn, &pairs(&[("env", "dev")])).await,
        );
    }

    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "tag-lifetime" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    settle().await;

    // The same application registered again is a NEW registration: it holds the tags
    // its own registration carried, which is none of the old ones.
    register(
        &emulator,
        "the registration that replaces it",
        registration("tag-lifetime", &[], Some(databases)),
    )
    .await;

    let mut arns = vec![("the application", app_arn)];
    for database_id in databases {
        arns.push((
            database_id,
            database_arn(REGION, "tag-lifetime", database_id),
        ));
    }
    for (label, arn) in arns {
        let held = tags_held(&emulator, label, &arn).await;
        assert!(
            held.is_empty(),
            "case {label:?}: the tags of the old registration are gone, got {held:?}"
        );
    }
}
