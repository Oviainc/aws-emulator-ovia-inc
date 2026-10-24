// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetLens`: the read of ONE lens of the catalog this
//! emulator serves.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. A custom lens is reported from its OWN record: the ARN its import minted, the name
//!    and description of the lens DOCUMENT it carries, the account that imported it and
//!    the tags assigned to it, with `LensVersion` naming the version that content is
//!    published as — the lens's draft until a version has been published from it. Rows
//!    differ along the shape of the imported document (its name, whether it carries a
//!    description, how many pillars and questions it defines) and along the tags the
//!    import assigned, because every lens has to be reported from the lens itself.
//! 2. A request naming a `LensVersion` is answered from THAT version's content, which a
//!    later import of the lens does not change, while a request naming none is answered
//!    from the lens's current content. Rows differ along the shape of the replacement
//!    document.
//! 3. A lens Amazon Web Services publishes is read by its alias or by its account-less
//!    ARN, and is described by ITS OWN entry in the official lens catalog — the same
//!    entry `ListLenses` reports it from — with the content members omitted only for a
//!    published alias the catalog carries no entry for. The rows are the whole published
//!    alias catalog, spanning both classes.
//! 4. A `LensAlias` or `LensVersion` that resolves to no lens is a 404
//!    `ResourceNotFoundException` — an alias no lens is published under, a custom ARN no
//!    lens is stored under, an ARN naming another account's lens, a version never
//!    published — and the modelled length bounds are refused at the boundary.
//! 5. Reading a lens changes nothing a later caller sees: repeated reads answer the same
//!    body and the listing of the catalog is unmoved. A lens is readable from the moment
//!    its import is ACCEPTED, as the listing reports it from then on.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`; the import and
//! publish wire helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{encode_path_segment, official_lens_arn, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The Region these tests sign for (the harness's SigV4 credential scope).
const REGION: &str = "us-west-2";

/// `GET /lenses/{LensAlias}`, the wire form of `GetLens`, with the optional `LensVersion`
/// bound to the query string. The alias is percent-encoded because a custom lens is named
/// by its ARN, which carries `:` and `/` inside ONE path segment.
async fn get_lens(client: &Client, alias: &str, version: Option<&str>) -> Answer {
    let path = format!("/lenses/{}", encode_path_segment(alias));
    let uri = match version {
        Some(version) => format!("{path}?LensVersion={}", encode_path_segment(version)),
        None => path,
    };
    client.send("GET", &uri, None).await
}

/// `GET /lenses`, the wire form of `ListLenses` — the catalog read a `GetLens` must not
/// move.
async fn list_lenses(client: &Client) -> Answer {
    client.send("GET", "/lenses?MaxResults=50", None).await
}

/// The `Lens` of a successful read.
fn lens(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLens 200, got {answer:?}"
    );
    answer
        .body
        .get("Lens")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry a Lens, got {answer:?}"))
}

fn text(lens: &Value, name: &str) -> String {
    lens.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Rule 1. Every shape of imported lens is read back from its own record, and the version
/// it reports is the draft until a version is published from that content.
#[tokio::test]
async fn a_custom_lens_is_read_back_from_its_own_record() {
    // (case, document, tags the import assigns)
    let cases: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "one-pillar-one-question",
            lens_document("Minimal read lens", None, &[("governance", 1, 2)]),
            None,
        ),
        (
            "described-and-tagged",
            lens_document(
                "Described read lens",
                Some("the questions company policy adds to a review"),
                &[("security", 2, 3)],
            ),
            Some(json!({ "Env": "test", "Team": "core" })),
        ),
        (
            "many-pillars",
            lens_document(
                "Wide read lens",
                Some("several pillars of questions"),
                &[("security", 1, 2), ("costOptimization", 3, 2), ("reliability", 2, 4)],
            ),
            None,
        ),
        (
            "non-ascii-name",
            lens_document("Lentille d'évaluation", None, &[("durabilité", 2, 2)]),
            Some(json!({ "équipe": "plateforme" })),
        ),
    ];

    let client = Client::new();
    let mut imported: Vec<(&str, Value, Option<Value>, String)> = Vec::new();
    for (case, document, tags) in &cases {
        let answer = import(
            &client,
            &document.to_string(),
            None,
            &format!("read-{case}"),
            tags.clone(),
        )
        .await;
        imported.push((case, document.clone(), tags.clone(), imported_arn(&answer, case)));
    }
    wait_for_the_import_to_land();

    for (case, document, tags, arn) in &imported {
        let answer = get_lens(&client, arn, None).await;
        let read = lens(&answer, case);
        assert_eq!(
            text(&read, "LensArn"),
            *arn,
            "case {case:?}: expected the ARN the import minted, got {read:?}"
        );
        assert_eq!(
            text(&read, "Name"),
            document["name"].as_str().unwrap_or_default(),
            "case {case:?}: the name comes from the imported document, got {read:?}"
        );
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            document.get("description").and_then(Value::as_str),
            "case {case:?}: the description comes from the imported document, got {read:?}"
        );
        assert_eq!(
            text(&read, "Owner"),
            engine::emulator_account_id(),
            "case {case:?}: the owner is the importing account, got {read:?}"
        );
        assert_eq!(
            text(&read, "LensVersion"),
            state::LENS_VERSION_DRAFT,
            "case {case:?}: a lens no version was published from reports its draft, got {read:?}"
        );
        assert_eq!(
            read.get("Tags"),
            Some(&tags.clone().unwrap_or_else(|| json!({}))),
            "case {case:?}: the tags the import assigned must be reported, got {read:?}"
        );
        assert!(
            read.get("ShareInvitationId").is_none(),
            "case {case:?}: a lens this account imported was not reached through a share \
             invitation, got {read:?}"
        );

        // Publishing a version of that same content moves what the read reports as the
        // lens's version, and nothing else about the lens.
        let published = create_version(&client, arn, "1.0", Some(true), &format!("pub-{case}")).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the publish must succeed, got {published:?}"
        );
        let after = lens(&get_lens(&client, arn, None).await, case);
        assert_eq!(
            text(&after, "LensVersion"),
            "1.0",
            "case {case:?}: the lens reports the version its content is published as, got \
             {after:?}"
        );
        assert_eq!(
            text(&after, "Name"),
            text(&read, "Name"),
            "case {case:?}: publishing does not rename the lens, got {after:?}"
        );
    }
}

/// Rule 2. A version-pinned read answers the content that version captured; the
/// unpinned read follows the lens's current content.
#[tokio::test]
async fn a_version_pinned_read_answers_the_content_that_version_captured() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "same-shape",
            lens_document("Replaced read lens", None, &[("governance", 1, 2)]),
        ),
        (
            "described-replacement",
            lens_document(
                "Grown read lens",
                Some("now with more pillars"),
                &[("governance", 1, 2), ("security", 3, 3)],
            ),
        ),
        (
            "fewer-questions",
            lens_document("Shrunk read lens", None, &[("governance", 1, 1)]),
        ),
    ];

    for (case, replacement) in cases {
        let client = Client::new();
        let original = lens_document(
            "Original read lens",
            Some("the content version 1.0 captures"),
            &[("governance", 2, 2)],
        );
        let arn = imported_arn(
            &import(
                &client,
                &original.to_string(),
                None,
                &format!("pin-{case}"),
                None,
            )
            .await,
            case,
        );
        wait_for_the_import_to_land();
        let published = create_version(&client, &arn, "1.0", Some(true), &format!("v1-{case}")).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the first publish must succeed, got {published:?}"
        );

        let update = import(
            &client,
            &replacement.to_string(),
            Some(&arn),
            &format!("reimport-{case}"),
            None,
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: the update import must be accepted, got {update:?}"
        );
        wait_for_the_import_to_land();

        let pinned = lens(&get_lens(&client, &arn, Some("1.0")).await, case);
        assert_eq!(
            text(&pinned, "Name"),
            original["name"].as_str().unwrap_or_default(),
            "case {case:?}: version 1.0 must keep the content it captured, got {pinned:?}"
        );
        assert_eq!(
            pinned.get("Description").and_then(Value::as_str),
            original.get("description").and_then(Value::as_str),
            "case {case:?}: version 1.0 must keep its own description, got {pinned:?}"
        );
        assert_eq!(
            text(&pinned, "LensVersion"),
            "1.0",
            "case {case:?}: a pinned read reports the version it was asked for, got {pinned:?}"
        );

        let current = lens(&get_lens(&client, &arn, None).await, case);
        assert_eq!(
            text(&current, "Name"),
            replacement["name"].as_str().unwrap_or_default(),
            "case {case:?}: the lens's current content is the new draft, got {current:?}"
        );
        assert_eq!(
            text(&current, "LensVersion"),
            state::LENS_VERSION_DRAFT,
            "case {case:?}: content no version was published from is a draft, got {current:?}"
        );

        // Publishing the replacement makes the unpinned read report that version, and
        // leaves the pinned read where it was.
        let next = create_version(&client, &arn, "2.0", Some(false), &format!("v2-{case}")).await;
        assert_eq!(
            next.status, 200,
            "case {case:?}: the second publish must succeed, got {next:?}"
        );
        let republished = lens(&get_lens(&client, &arn, None).await, case);
        assert_eq!(
            text(&republished, "LensVersion"),
            "2.0",
            "case {case:?}: the lens now reports the version just published, got {republished:?}"
        );
        let unchanged = lens(&get_lens(&client, &arn, Some("1.0")).await, case);
        assert_eq!(
            unchanged, pinned,
            "case {case:?}: a published version is immutable, got {unchanged:?}"
        );
    }
}

/// Rule 3. Every alias AWS publishes a lens under is readable, by alias and by the
/// account-less ARN, and each is described by ITS OWN catalog entry — so a lens the
/// catalog carries reports that entry's name, version and description, and only a
/// published alias with no entry reports the ARN alone.
#[tokio::test]
async fn an_official_lens_is_read_from_the_shipped_catalog() {
    let client = Client::new();

    // The rows are every alias Amazon Web Services publishes a lens under, straight from
    // the catalog a lens name resolves against, and the expectation of each row comes from
    // that lens's own entry in `OFFICIAL_LENS_CATALOG` — the table `ListLenses` reports
    // from too, so a disagreement between the two reads fails here.
    let mut carried = 0usize;
    let mut uncarried = 0usize;
    for alias in state::AWS_OFFICIAL_LENS_ALIASES {
        if state::official_lens(alias).is_some() {
            carried += 1;
        } else {
            uncarried += 1;
        }
        let answer = get_lens(&client, alias, None).await;
        let read = lens(&answer, alias);
        assert_eq!(
            text(&read, "LensArn"),
            official_lens_arn(REGION, alias),
            "case {alias:?}: expected the account-less official ARN, got {read:?}"
        );
        assert!(
            read.get("Owner").is_none(),
            "case {alias:?}: an official lens ARN names no owner account, got {read:?}"
        );
        assert!(
            read.get("Tags").is_none(),
            "case {alias:?}: an official lens carries no tags here, got {read:?}"
        );
        assert!(
            read.get("ShareInvitationId").is_none(),
            "case {alias:?}: an official lens is not reached through a share invitation, got \
             {read:?}"
        );

        if let Some(official) = state::official_lens(alias) {
            assert_eq!(
                text(&read, "Name"),
                official.name,
                "case {alias:?}: expected the catalog entry's full name, got {read:?}"
            );
            assert_eq!(
                text(&read, "LensVersion"),
                official.version,
                "case {alias:?}: expected the catalog entry's version, got {read:?}"
            );
            assert_eq!(
                text(&read, "Description"),
                official.description,
                "case {alias:?}: expected the catalog entry's description, got {read:?}"
            );
        } else {
            for member in ["Name", "LensVersion", "Description"] {
                assert!(
                    read.get(member).is_none(),
                    "case {alias:?}: this emulator carries no catalog entry for the lens, so \
                     {member} must be omitted, got {read:?}"
                );
            }
        }

        // The ARN spelling of the same name names the same lens.
        let by_arn = get_lens(&client, &official_lens_arn(REGION, alias), None).await;
        assert_eq!(
            lens(&by_arn, alias),
            read,
            "case {alias:?}: the ARN and the alias must name the same lens, got {by_arn:?}"
        );

        // So does an ARN spelled for ANOTHER Region's endpoint: the lens is identified by
        // its alias, and the answer reports it in the Region the request was signed for.
        let elsewhere = get_lens(&client, &official_lens_arn("eu-west-1", alias), None).await;
        assert_eq!(
            lens(&elsewhere, alias),
            read,
            "case {alias:?}: the lens must be reported in the signing Region, got {elsewhere:?}"
        );

        // The version a read can pin is the one the lens's own catalog entry carries —
        // the framework lens and a specialised lens are carried at different versions —
        // and a lens with no entry has no version to pin.
        match state::official_lens(alias) {
            Some(official) => {
                let pinned = get_lens(&client, alias, Some(official.version)).await;
                assert_eq!(
                    lens(&pinned, alias),
                    read,
                    "case {alias:?}: the carried version must read as the lens itself, got \
                     {pinned:?}"
                );
            }
            None => {
                let pinned = get_lens(&client, alias, Some(state::FRAMEWORK_LENS_VERSION)).await;
                assert_eq!(
                    pinned.status, 404,
                    "case {alias:?}: no version of this lens is held here, got {pinned:?}"
                );
            }
        }
        let absent = get_lens(&client, alias, Some("1066-10-14")).await;
        assert_eq!(
            absent.status, 404,
            "case {alias:?}: a version never published must not resolve, got {absent:?}"
        );
        assert_eq!(
            absent.error_code, "ResourceNotFoundException",
            "case {alias:?}: expected ResourceNotFoundException, got {absent:?}"
        );
    }

    // Both classes of published alias were actually exercised above: one the catalog
    // carries content for, and one it does not.
    assert!(
        carried > 0 && uncarried > 0,
        "the table must span both published aliases the catalog carries ({carried}) and \
         published aliases it does not ({uncarried})"
    );
}

/// Rule 4. A name or version that resolves to no lens is refused, and the modelled length
/// bounds are refused at the boundary.
#[tokio::test]
async fn a_name_or_version_that_resolves_to_no_lens_is_refused() {
    let client = Client::new();
    let document = lens_document("Refused read lens", None, &[("governance", 1, 2)]).to_string();
    let arn = imported_arn(
        &import(&client, &document, None, "tok-refusals", None).await,
        "refusals",
    );
    wait_for_the_import_to_land();
    // A custom ARN that agrees on the lens id and disagrees on the owning account names a
    // different lens, so it resolves to nothing.
    let other_account = arn.replacen(
        &format!(":{}:", engine::emulator_account_id()),
        ":210987654321:",
        1,
    );

    // (case, alias, version, expected status)
    let cases: Vec<(&str, String, Option<&str>, u16)> = vec![
        // A bare alias can only name a lens AWS publishes, and none is published under
        // this one.
        ("unpublished-bare-alias", "notalens".to_string(), None, 404),
        (
            "unknown-custom-arn",
            ABSENT_LENS_ARN.to_string(),
            None,
            404,
        ),
        ("another-account", other_account, None, 404),
        ("version-never-published", arn.clone(), Some("7.7"), 404),
        // The modelled bounds: `LensAlias` is @length(1, 128) and `LensVersion`
        // @length(1, 32) (`smithy_query(query="operation", operation="GetLens")`), so one
        // character past either ceiling is refused before the handler runs.
        ("alias-one-past-the-ceiling", "a".repeat(129), None, 400),
        ("empty-version", arn.clone(), Some(""), 400),
    ];

    for (case, alias, version, status) in &cases {
        let answer = get_lens(&client, alias, *version).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        let expected_code = if *status == 404 {
            "ResourceNotFoundException"
        } else {
            "ValidationException"
        };
        assert_eq!(
            answer.error_code, expected_code,
            "case {case:?}: expected {expected_code}, got {answer:?}"
        );
        if *status == 404 {
            assert_eq!(
                answer.body.get("ResourceType").and_then(Value::as_str),
                Some(state::LENS_RESOURCE_TYPE),
                "case {case:?}: the refusal must name the LENS resource type, got {answer:?}"
            );
        }
    }

    // A version-pinned read of the version that WAS published still works, so the
    // refusals above are about the names, not about the lens.
    let published = create_version(&client, &arn, "1.0", Some(true), "tok-refusals-publish").await;
    assert_eq!(
        published.status, 200,
        "the publish must succeed, got {published:?}"
    );
    let answer = get_lens(&client, &arn, Some("1.0")).await;
    assert_eq!(
        text(&lens(&answer, "published-version"), "LensVersion"),
        "1.0",
        "the published version must read back, got {answer:?}"
    );
}

/// Rule 5. A lens is readable from the moment its import is accepted, and reading it
/// changes nothing a later caller sees.
#[tokio::test]
async fn reading_a_lens_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let document = lens_document(
        "Read-only lens",
        Some("a read must not move it"),
        &[("governance", 2, 2)],
    );
    let arn = imported_arn(
        &import(&client, &document.to_string(), None, "tok-read-only", None).await,
        "read-only",
    );

    // Straight after the import is accepted, before it has landed: the lens exists — the
    // import answered its ARN — so the read reports it, exactly as the listing does.
    let in_flight = lens(&get_lens(&client, &arn, None).await, "in-flight");
    assert_eq!(
        text(&in_flight, "Name"),
        document["name"].as_str().unwrap_or_default(),
        "case \"in-flight\": an accepted import is readable, got {in_flight:?}"
    );

    let before = list_lenses(&client).await;
    let first = get_lens(&client, &arn, None).await;
    let second = get_lens(&client, &arn, None).await;
    assert_eq!(
        first.body, second.body,
        "case \"repeated\": two reads of one lens must answer the same lens, got {second:?}"
    );
    let after = list_lenses(&client).await;
    assert_eq!(
        before.body, after.body,
        "case \"repeated\": reading a lens must not move the catalog, got {after:?}"
    );

    wait_for_the_import_to_land();
    let landed = lens(&get_lens(&client, &arn, None).await, "landed");
    assert_eq!(
        landed,
        lens(&first, "repeated"),
        "case \"landed\": the landed import carries the document already reported, got {landed:?}"
    );
}
