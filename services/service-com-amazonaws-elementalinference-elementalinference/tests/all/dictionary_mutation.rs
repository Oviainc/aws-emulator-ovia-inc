// SPDX-License-Identifier: CC0-1.0

//! What a MUTATION of this service's resources leaves behind, end to end over the
//! wire: `UpdateDictionary`, `DeleteDictionary`, the feed-to-dictionary reference, and
//! the status `UpdateFeed` reports.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! case:
//!
//! * a successful mutation is VISIBLE to the next read of the same resource:
//!   `UpdateDictionary` writes the members the request carried (and only those) into the
//!   stored dictionary, so `GetDictionary` and `ExportDictionaryEntries` report them;
//! * `DeleteDictionary` retires the dictionary: it reports `DELETING`, and once the
//!   removal falls due the id stops resolving, so `GetDictionary` answers 404
//!   `ResourceNotFoundException` — forever, not transiently;
//! * a dictionary is `REFERENCED`, with the referencing feed's id in `references`,
//!   exactly while some feed output's subtitling config names it, whichever call put it
//!   there; and "you cannot delete a dictionary that is referenced by a feed", which is
//!   the modelled `ConflictException` (HTTP 409);
//! * `UpdateFeed` answers HTTP 200, so it reports the feed's SETTLED status — never a
//!   transient one a caller would have to wait out.

use crate::harness::{
    call, expected_dictionary_arn, output_config, past_the_transition_span, router,
    subtitling_output,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    dictionary_referenced_message, DICTIONARY_BEING_DELETED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `message` a refusal carries.
fn message_of(error: &Value) -> &str {
    error["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal carries a message, got {error}"))
}

/// Create one dictionary and return its id.
async fn create_dictionary(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/v1/dictionary", Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "CreateDictionary({body}) answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDictionary returns an id, got {created}"))
        .to_string()
}

/// `GET /v1/dictionary/{id}`.
async fn get_dictionary(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/dictionary/{id}"), None).await
}

/// `GET /v1/dictionary/{id}/entries/export` — the reader for the entries payload
/// `CreateDictionary` and `UpdateDictionary` write.
async fn export_entries(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v1/dictionary/{id}/entries/export"),
        None,
    )
    .await
}

/// One output of the `kind` config the harness builds, which names no dictionary.
fn plain_output(name: &str, kind: &str) -> Value {
    json!({"name": name, "outputConfig": output_config(kind), "status": "ENABLED"})
}

/// Create one feed carrying `outputs` and return its id.
async fn create_feed(router: &MultiServiceRouter, name: &str, outputs: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": name, "outputs": outputs})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: CreateFeed answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// The `references` list of a dictionary response, as a sorted `Vec` so a comparison
/// does not depend on the order feeds happen to be stored in.
fn references_of(dictionary: &Value) -> Vec<String> {
    let mut ids: Vec<String> = dictionary["references"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// One row of the UpdateDictionary table: which members the PATCH carries.
struct PatchCase {
    label: &'static str,
    patch: Value,
}

/// A row per combination of the optional members `UpdateDictionary` takes — "if not
/// specified, the <member> is not changed" — including the patch that carries none, and
/// a language row per value the `DictionaryLanguage` enum admits
/// (`smithy_query(query="shape",shape="DictionaryLanguage")`: deu, eng, fra, ita, por,
/// spa; the base record below is `eng`, so each row moves it elsewhere).
fn patch_cases() -> Vec<PatchCase> {
    vec![
        PatchCase {
            label: "name only",
            patch: json!({"name": "basedictv2"}),
        },
        PatchCase {
            label: "language only, to deu",
            patch: json!({"language": "deu"}),
        },
        PatchCase {
            label: "entries only",
            patch: json!({"entries": "patched-term\npatched-other\n"}),
        },
        PatchCase {
            label: "entries emptied",
            patch: json!({"entries": ""}),
        },
        PatchCase {
            label: "name and language, to fra",
            patch: json!({"name": "renamed-to-fra", "language": "fra"}),
        },
        PatchCase {
            label: "every member, to spa",
            patch: json!({
                "name": "renamed3",
                "language": "spa",
                "entries": "spa-term\n",
            }),
        },
        PatchCase {
            label: "no member at all",
            patch: json!({}),
        },
    ]
}

/// UpdateDictionary writes the supplied members back to the stored dictionary — so the
/// next GetDictionary (and ExportDictionaryEntries) reports them, and the members the
/// patch did not carry are unchanged.
#[tokio::test]
async fn update_dictionary_writes_the_supplied_members_into_state() {
    let router = router();
    let base_name = "basedict";
    let base_language = "eng";
    let base_entries = "base-term\nbase-other\n";

    // One dictionary per row, all created before the single wait, so the table costs
    // one transition span rather than one per row.
    let cases = patch_cases();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..cases.len() {
        let id = create_dictionary(
            &router,
            json!({
                "name": base_name,
                "language": base_language,
                "entries": base_entries,
                "tags": {"env": "probe", "row": format!("r{index}")},
            }),
        )
        .await;
        ids.push(id);
    }
    past_the_transition_span();

    for (case, id) in cases.iter().zip(&ids) {
        let label = case.label;
        let expected_name = case.patch["name"].as_str().unwrap_or(base_name).to_string();
        let expected_language = case.patch["language"]
            .as_str()
            .unwrap_or(base_language)
            .to_string();
        let expected_entries = case.patch["entries"]
            .as_str()
            .unwrap_or(base_entries)
            .to_string();

        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/v1/dictionary/{id}"),
            Some(case.patch.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateDictionary answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["id"],
            json!(id),
            "case {label:?}: the update keeps the dictionary id, got {updated}"
        );
        assert_eq!(
            updated["arn"],
            json!(expected_dictionary_arn(id)),
            "case {label:?}: the update keeps the dictionary arn, got {updated}"
        );
        assert_eq!(
            updated["name"],
            json!(expected_name),
            "case {label:?}: the update response carries the stored name, got {:?}",
            updated["name"]
        );
        assert_eq!(
            updated["language"],
            json!(expected_language),
            "case {label:?}: the update response carries the stored language, got {:?}",
            updated["language"]
        );
        assert_eq!(
            updated["status"],
            json!("AVAILABLE"),
            "case {label:?}: an update of a settled, unreferenced dictionary leaves it \
             AVAILABLE, got {:?}",
            updated["status"]
        );
        assert_eq!(
            references_of(&updated),
            Vec::<String>::new(),
            "case {label:?}: no feed references this dictionary, got {:?}",
            updated["references"]
        );

        // The mutation is in STORAGE, not just in the response.
        let (status, fetched) = get_dictionary(&router, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetDictionary after the update answers 200, got {status} with \
             {fetched}"
        );
        assert_eq!(
            fetched, updated,
            "case {label:?}: GetDictionary reports exactly the updated record, got {fetched}"
        );
        assert_eq!(
            fetched["tags"]["env"],
            json!("probe"),
            "case {label:?}: UpdateDictionary takes no tags, so the dictionary keeps its \
             own, got {:?}",
            fetched["tags"]
        );

        let (status, exported) = export_entries(&router, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ExportDictionaryEntries answers 200, got {status} with {exported}"
        );
        let reported_entries = exported["entries"].as_str().unwrap_or_default();
        assert_eq!(
            reported_entries, expected_entries,
            "case {label:?}: the exported entries are the stored ones, got {reported_entries:?}"
        );
    }
}

/// UpdateDictionary refuses what it cannot apply, and persists nothing when it does: an
/// id no live dictionary has is a 404, and a value outside a modelled constraint is a
/// 400.
#[tokio::test]
async fn update_dictionary_refuses_an_unknown_id_and_a_value_outside_the_model() {
    let router = router();
    let id = create_dictionary(
        &router,
        json!({"name": "livedict", "language": "eng", "entries": "live\n"}),
    )
    .await;
    past_the_transition_span();

    // Every id here satisfies the modelled `DictionaryId` (`^[a-zA-Z0-9]+$`, 1..19), so
    // the boundary admits it and the handler is what refuses it: the shortest
    // admissible id, the longest, and one in between.
    for unknown in ["a", "0000000000000000000", "Unknown123"] {
        assert_ne!(unknown, id, "the unknown ids are not the live dictionary's");
        let label = format!("PATCH /v1/dictionary/{unknown}");
        let (status, error) = call(
            &router,
            "PATCH",
            &format!("/v1/dictionary/{unknown}"),
            Some(json!({"name": "ghost"})),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: an unknown dictionary id is a 404, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ResourceNotFoundException"),
            "case {label:?}: that 404 is a ResourceNotFoundException, got {error}"
        );
    }

    // A value outside a modelled constraint: a language the `DictionaryLanguage` enum
    // does not list (in the wrong case, and a BCP-47 tag), a name the `ResourceName`
    // pattern rejects, and an entries payload one past `length max 40960`.
    let cases = [
        ("language not in the enum", json!({"language": "eng-us"})),
        ("language in the wrong case", json!({"language": "ENG"})),
        ("name the ResourceName pattern rejects", json!({"name": "-x"})),
        (
            "entries one past the modelled maximum",
            json!({"entries": "w".repeat(40961)}),
        ),
    ];
    for (label, patch) in cases {
        let (status, error) = call(
            &router,
            "PATCH",
            &format!("/v1/dictionary/{id}"),
            Some(patch),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: the update is refused with 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
    }

    // None of those refusals wrote anything.
    let (status, fetched) = get_dictionary(&router, &id).await;
    assert_eq!(
        (status, &fetched["name"], &fetched["language"]),
        (200, &json!("livedict"), &json!("eng")),
        "a refused update changes nothing, got {status} with {fetched}"
    );
}

/// DeleteDictionary retires the dictionary: it reports DELETING, repeating the call
/// inside that window is idempotent, and once the removal falls due the id stops
/// resolving — while every other dictionary is left alone.
#[tokio::test]
async fn delete_dictionary_retires_the_dictionary_from_state() {
    // Deleting one of several dictionaries must leave the others alone, so the table
    // spans which dictionary of how many is deleted.
    for (count, target) in [(1usize, 0usize), (3, 0), (3, 2)] {
        let label = format!("deleting dictionary {target} of {count}");
        let router = router();
        let mut ids: Vec<String> = Vec::new();
        for index in 0..count {
            let id = create_dictionary(
                &router,
                json!({"name": format!("dict{index}"), "language": "eng"}),
            )
            .await;
            ids.push(id);
        }
        let id = ids[target].clone();

        let (status, deleted) = call(&router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {label:?}: DeleteDictionary answers 202, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["id"],
            json!(id),
            "case {label:?}: the delete response names the dictionary, got {deleted}"
        );
        assert_eq!(
            deleted["arn"],
            json!(expected_dictionary_arn(&id)),
            "case {label:?}: the delete response carries the dictionary arn, got {deleted}"
        );
        assert_eq!(
            deleted["status"],
            json!("DELETING"),
            "case {label:?}: a deletion accepted with 202 reports DELETING, got {deleted}"
        );

        // Inside the window the dictionary is still reported, in its transient status.
        let (status, deleting) = get_dictionary(&router, &id).await;
        assert_eq!(
            (status, &deleting["status"]),
            (200, &json!("DELETING")),
            "case {label:?}: a dictionary whose deletion is pending reads DELETING, got \
             {status} with {deleting}"
        );
        // And repeating the delete inside the window changes nothing.
        let (status, repeat) = call(&router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            (status, &repeat["status"]),
            (202, &json!("DELETING")),
            "case {label:?}: repeating the delete is idempotent, got {status} with {repeat}"
        );
        // An update cannot cancel a removal the clock already owes: it is refused with
        // the modelled ConflictException carrying that rule's invariant sentence.
        let (status, conflict) = call(
            &router,
            "PATCH",
            &format!("/v1/dictionary/{id}"),
            Some(json!({"name": "toolate"})),
        )
        .await;
        assert_eq!(
            (status, &conflict["__type"]),
            (409, &json!("ConflictException")),
            "case {label:?}: updating a dictionary that is being deleted is a 409, got {status} \
             with {conflict}"
        );
        assert_eq!(
            message_of(&conflict),
            DICTIONARY_BEING_DELETED_MESSAGE,
            "case {label:?}: that refusal is the rule's invariant sentence, got {:?}",
            conflict["message"]
        );

        past_the_transition_span();
        for probe in 0..2 {
            let (status, gone) = get_dictionary(&router, &id).await;
            assert_eq!(
                status, 404,
                "case {label:?}: probe {probe}: once the deletion is due GetDictionary is a \
                 404, got {status} with {gone}"
            );
            assert_eq!(
                gone["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: probe {probe}: that 404 is a ResourceNotFoundException, got \
                 {gone}"
            );
        }

        for other in ids.iter().filter(|other| **other != id) {
            let (status, fetched) = get_dictionary(&router, other).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the other dictionaries are untouched, got {status} with \
                 {fetched}"
            );
        }

        // An id that never existed answers the same 404 as a deleted one.
        let (status, error) = call(&router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            (status, &error["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {label:?}: deleting a dictionary that is gone is a 404, got {status} with \
             {error}"
        );
    }
}

/// One row of the reference table: how the feed's outputs are built, and whether that
/// makes the dictionary referenced.
struct ReferenceCase {
    label: &'static str,
    /// How the reference gets onto the feed: "create", "update" or "associate".
    via: &'static str,
    referenced: bool,
}

/// A dictionary is REFERENCED, with the feed's id in `references`, exactly while some
/// feed output's subtitling config names it — whichever call put that output there —
/// and AVAILABLE again as soon as nothing names it.
#[tokio::test]
async fn a_dictionary_is_referenced_exactly_while_a_feed_output_names_it() {
    let rows = [
        ReferenceCase {
            label: "CreateFeed names the dictionary",
            via: "create",
            referenced: true,
        },
        ReferenceCase {
            label: "UpdateFeed names the dictionary",
            via: "update",
            referenced: true,
        },
        ReferenceCase {
            label: "AssociateFeed appends an output naming the dictionary",
            via: "associate",
            referenced: true,
        },
        ReferenceCase {
            label: "a subtitling output that names no dictionary",
            via: "subtitling-without-dictionary",
            referenced: false,
        },
        ReferenceCase {
            label: "outputs of every other config kind",
            via: "other-kinds",
            referenced: false,
        },
        ReferenceCase {
            label: "a feed naming a DIFFERENT dictionary",
            via: "other-dictionary",
            referenced: false,
        },
    ];

    for row in rows {
        let label = row.label;
        let router = router();
        let id = create_dictionary(
            &router,
            json!({"name": "referenceddict", "language": "eng"}),
        )
        .await;
        let other_id = create_dictionary(
            &router,
            json!({"name": "otherdict", "language": "fra"}),
        )
        .await;
        past_the_transition_span();

        let feed_id = match row.via {
            "create" => {
                create_feed(
                    &router,
                    "referencing-feed",
                    json!([subtitling_output("subs1", Some(&id))]),
                )
                .await
            }
            "update" => {
                let feed_id =
                    create_feed(&router, "to-be-updated", json!([plain_output("crop1", "cropping")]))
                        .await;
                let (status, updated) = call(
                    &router,
                    "PUT",
                    &format!("/v1/feed/{feed_id}"),
                    Some(json!({
                        "name": "to-be-updated",
                        "outputs": [subtitling_output("subs1", Some(&id))],
                    })),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
                );
                feed_id
            }
            "associate" => {
                let feed_id = create_feed(
                    &router,
                    "to-be-associated",
                    json!([plain_output("crop1", "cropping")]),
                )
                .await;
                past_the_transition_span();
                let (status, associated) = call(
                    &router,
                    "POST",
                    &format!("/v1/feed/{feed_id}/associate"),
                    Some(json!({
                        "associatedResourceName": "live-media",
                        "outputs": [subtitling_output("subs1", Some(&id))],
                    })),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: AssociateFeed answers 200, got {status} with {associated}"
                );
                feed_id
            }
            "subtitling-without-dictionary" => {
                create_feed(
                    &router,
                    "no-dictionary-feed",
                    json!([subtitling_output("subs1", None)]),
                )
                .await
            }
            "other-kinds" => {
                create_feed(
                    &router,
                    "other-kinds-feed",
                    json!([
                        plain_output("crop1", "cropping"),
                        plain_output("clip1", "clipping"),
                        plain_output("meta1", "contextualMetadata"),
                    ]),
                )
                .await
            }
            "other-dictionary" => {
                create_feed(
                    &router,
                    "other-dictionary-feed",
                    json!([subtitling_output("subs1", Some(&other_id))]),
                )
                .await
            }
            other => panic!("no such reference row: {other}"),
        };

        let (status, dictionary) = get_dictionary(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetDictionary answers 200, got {status} with {dictionary}"
        );
        let expected_status = if row.referenced {
            "REFERENCED"
        } else {
            "AVAILABLE"
        };
        assert_eq!(
            dictionary["status"],
            json!(expected_status),
            "case {label:?}: the dictionary reports {expected_status}, got {:?}",
            dictionary["status"]
        );
        let expected_references: Vec<String> = if row.referenced {
            vec![feed_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            references_of(&dictionary),
            expected_references,
            "case {label:?}: references lists exactly the feeds that name it, got {:?}",
            dictionary["references"]
        );

        // Taking the reference away puts the dictionary back to AVAILABLE: the status
        // tracks the feeds, it is not a latch.
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{feed_id}"),
            Some(json!({
                "name": "dereferenced-feed",
                "outputs": [plain_output("crop1", "cropping")],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the dereferencing UpdateFeed answers 200, got {status} with \
             {updated}"
        );
        let (_, released) = get_dictionary(&router, &id).await;
        assert_eq!(
            released["status"],
            json!("AVAILABLE"),
            "case {label:?}: with no feed naming it the dictionary is AVAILABLE again, got {:?}",
            released["status"]
        );
        assert_eq!(
            references_of(&released),
            Vec::<String>::new(),
            "case {label:?}: and references is empty again, got {:?}",
            released["references"]
        );
    }
}

/// Every feed that names the dictionary appears in `references`, and a feed that goes
/// away stops appearing.
#[tokio::test]
async fn references_lists_every_feed_that_names_the_dictionary() {
    let router = router();
    let id = create_dictionary(&router, json!({"name": "shareddict", "language": "eng"})).await;

    let mut feed_ids: Vec<String> = Vec::new();
    for index in 0..3 {
        feed_ids.push(
            create_feed(
                &router,
                &format!("sharing-feed-{index}"),
                json!([subtitling_output("subs1", Some(&id))]),
            )
            .await,
        );
    }
    // A dictionary still settling reports its LIFECYCLE status, even though the feeds
    // naming it are already in `references`: CREATING is what a caller must wait out
    // before the dictionary is usable at all.
    let (_, settling) = get_dictionary(&router, &id).await;
    assert_eq!(
        settling["status"],
        json!("CREATING"),
        "a dictionary still settling reports CREATING, got {:?}",
        settling["status"]
    );
    past_the_transition_span();
    let (_, dictionary) = get_dictionary(&router, &id).await;
    let mut expected = feed_ids.clone();
    expected.sort();
    assert_eq!(
        references_of(&dictionary),
        expected,
        "every feed naming the dictionary is in references, got {:?}",
        dictionary["references"]
    );
    assert_eq!(
        dictionary["status"],
        json!("REFERENCED"),
        "a dictionary named by three feeds is REFERENCED, got {:?}",
        dictionary["status"]
    );

    // A feed whose deletion has been ACCEPTED is still a live record, so it stays in
    // `references` — the list is a faithful view of the feeds GetFeed reports — and it
    // drops out once the removal falls due.
    let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{}", feed_ids[1]), None).await;
    assert_eq!(status, 202, "DeleteFeed answers 202, got {status}");
    let (_, accepted) = get_dictionary(&router, &id).await;
    assert_eq!(
        references_of(&accepted),
        expected,
        "a feed whose deletion is only accepted is still in references, got {:?}",
        accepted["references"]
    );
    past_the_transition_span();
    let (_, after) = get_dictionary(&router, &id).await;
    expected.retain(|feed| *feed != feed_ids[1]);
    assert_eq!(
        references_of(&after),
        expected,
        "a feed whose removal has fallen due drops out of references, got {:?}",
        after["references"]
    );
}

/// "You cannot delete a dictionary that is referenced by a feed. You must first remove
/// the dictionary reference from the feed's subtitling configuration." — the modelled
/// `ConflictException` (HTTP 409), and the delete is accepted once the reference is
/// gone.
#[tokio::test]
async fn delete_dictionary_is_refused_while_a_feed_references_it() {
    // One row per way the reference is removed again: rewriting the feed's outputs, and
    // deleting the feed outright.
    for removal in ["update-feed", "delete-feed"] {
        let router = router();
        let id = create_dictionary(&router, json!({"name": "indict", "language": "eng"})).await;
        let feed_id = create_feed(
            &router,
            "holding-feed",
            json!([subtitling_output("subs1", Some(&id))]),
        )
        .await;
        past_the_transition_span();

        let (status, error) = call(&router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 409,
            "case {removal:?}: deleting a referenced dictionary is a 409, got {status} with \
             {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ConflictException"),
            "case {removal:?}: that 409 is a ConflictException, got {error}"
        );
        assert_eq!(
            message_of(&error),
            dictionary_referenced_message(1),
            "case {removal:?}: the refusal reports the one feed that references it, got {:?}",
            error["message"]
        );
        assert!(
            !message_of(&error).contains(&feed_id),
            "case {removal:?}: the refusal must not report the feed read out of state, got {:?}",
            error["message"]
        );
        // The refused delete did not start a deletion.
        let (status, still_there) = get_dictionary(&router, &id).await;
        assert_eq!(
            (status, &still_there["status"]),
            (200, &json!("REFERENCED")),
            "case {removal:?}: the refused delete left the dictionary REFERENCED, got {status} \
             with {still_there}"
        );

        match removal {
            "update-feed" => {
                let (status, _) = call(
                    &router,
                    "PUT",
                    &format!("/v1/feed/{feed_id}"),
                    Some(json!({
                        "name": "holding-feed",
                        "outputs": [plain_output("crop1", "cropping")],
                    })),
                )
                .await;
                assert_eq!(status, 200, "case {removal:?}: UpdateFeed answers 200");
            }
            "delete-feed" => {
                // The feed's deletion is only ACCEPTED here, not complete: a feed on
                // its way out holds the dictionary back no longer, so a teardown does
                // not have to wait for the feed's removal to fall due before the
                // dictionary may go — even though the retiring feed is still a live
                // record and so still appears in `references`.
                let (status, _) =
                    call(&router, "DELETE", &format!("/v1/feed/{feed_id}"), None).await;
                assert_eq!(status, 202, "case {removal:?}: DeleteFeed answers 202");
                let (_, retiring) = get_dictionary(&router, &id).await;
                assert_eq!(
                    references_of(&retiring),
                    vec![feed_id.clone()],
                    "case {removal:?}: the retiring feed is still a live record in \
                     references, got {:?}",
                    retiring["references"]
                );
            }
            other => panic!("no such removal row: {other}"),
        }

        let (status, deleted) = call(&router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {removal:?}: with the reference removed the delete is accepted, got {status} \
             with {deleted}"
        );
        past_the_transition_span();
        let (status, gone) = get_dictionary(&router, &id).await;
        assert_eq!(
            (status, &gone["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {removal:?}: and the id stops resolving, got {status} with {gone}"
        );
    }
}

/// UpdateFeed answers HTTP 200 — the overwrite is complete when the call returns — so it
/// reports the status the feed HOLDS, and the stored feed is left in that same status:
/// an update never parks a feed in a transient status, and it neither cancels nor
/// accelerates a transition the clock already owes.
#[tokio::test]
async fn update_feed_reports_the_status_the_feed_holds() {
    // One row per status a feed can be in when the update arrives — every settled
    // status the service produces — with rows differing in how many outputs the payload
    // carries. `expected_now` is what the update and an immediate read report;
    // `expected_later` is what the clock leaves once anything it owed has fallen due.
    let rows: [(&str, &str, &str, Value); 4] = [
        (
            "a feed read right after its creation",
            "AVAILABLE",
            "AVAILABLE",
            json!([plain_output("crop1", "cropping")]),
        ),
        (
            "a settled, unassociated feed",
            "AVAILABLE",
            "AVAILABLE",
            json!([
                plain_output("crop1", "cropping"),
                plain_output("clip1", "clipping"),
            ]),
        ),
        (
            "a feed a resource holds",
            "ACTIVE",
            "ACTIVE",
            json!([plain_output("meta1", "contextualMetadata")]),
        ),
        (
            "a feed whose resource has been released",
            "ARCHIVED",
            "ARCHIVED",
            json!([subtitling_output("subs1", None)]),
        ),
    ];

    for (label, expected_now, expected_later, outputs) in rows {
        let router = router();
        let feed_id = create_feed(
            &router,
            "status-feed",
            json!([plain_output("crop1", "cropping")]),
        )
        .await;

        if expected_now == "ACTIVE" || expected_now == "ARCHIVED" {
            let (status, _) = call(
                &router,
                "POST",
                &format!("/v1/feed/{feed_id}/associate"),
                Some(json!({"associatedResourceName": "held-media", "outputs": []})),
            )
            .await;
            assert_eq!(status, 200, "case {label:?}: AssociateFeed answers 200");
        }
        if expected_now == "ARCHIVED" {
            let (status, _) = call(
                &router,
                "POST",
                &format!("/v1/feed/{feed_id}/disassociate"),
                Some(json!({"associatedResourceName": "held-media"})),
            )
            .await;
            assert_eq!(status, 200, "case {label:?}: DisassociateFeed answers 200");
        }

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{feed_id}"),
            Some(json!({"name": "updated-feed", "outputs": outputs})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["status"],
            json!(expected_now),
            "case {label:?}: the update reports the status the feed holds, got {:?}",
            updated["status"]
        );
        assert_eq!(
            updated["name"],
            json!("updated-feed"),
            "case {label:?}: the update carries the new name, got {:?}",
            updated["name"]
        );

        // The status in the response is the stored one: a read straight after reports
        // it, and the clock then does exactly what it already owed — no more.
        let (status, immediately) = call(&router, "GET", &format!("/v1/feed/{feed_id}"), None).await;
        assert_eq!(
            (status, &immediately["status"]),
            (200, &json!(expected_now)),
            "case {label:?}: GetFeed straight after the update reports the same status, got \
             {status} with {immediately}"
        );
        past_the_transition_span();
        let (_, later) = call(&router, "GET", &format!("/v1/feed/{feed_id}"), None).await;
        assert_eq!(
            later["status"],
            json!(expected_later),
            "case {label:?}: the update left owed exactly what the clock already owed, got {:?}",
            later["status"]
        );
        assert_eq!(
            later["name"],
            json!("updated-feed"),
            "case {label:?}: GetFeed reports the stored new name, got {:?}",
            later["name"]
        );
    }
}
