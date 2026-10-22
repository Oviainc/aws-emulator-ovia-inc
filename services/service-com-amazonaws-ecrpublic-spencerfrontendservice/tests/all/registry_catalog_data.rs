// SPDX-License-Identifier: CC0-1.0

//! The registry-level catalog data (`PutRegistryCatalogData` / `GetRegistryCatalogData`),
//! driven end to end over the wire through the same front door a client uses.
//!
//! The rule under test: a public registry carries exactly ONE `RegistryCatalogData`, whose
//! `displayName` is whatever the last accepted write stored (absent while no write has
//! stored one), reported verbatim by every later read and left untouched by a refused
//! write. The tables below span the class the rule generalises over — every script, width
//! and length the modelled `RegistryDisplayName` (`length 0..100`, per
//! `smithy_query(query="shape", shape="RegistryDisplayName")`) admits, both published
//! bounds and one past the upper bound — rather than one recorded display name.

use serde_json::{json, Value};

use crate::repository_image_lifecycle::{call, ok, refused, service};

/// The modelled upper bound on `RegistryDisplayName`
/// (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`).
const MAX_DISPLAY_NAME_LEN: usize = 100;

/// The `displayName` a response reports: `None` when the member is absent, which is how a
/// registry with no display name must answer.
fn display_name(body: &Value, case: &str) -> Option<String> {
    let catalog = &body["registryCatalogData"];
    assert!(
        catalog.is_object(),
        "case {case:?}: expected a registryCatalogData object, got {body}"
    );
    match &catalog["displayName"] {
        Value::Null => None,
        Value::String(name) => Some(name.clone()),
        other => panic!("case {case:?}: expected displayName to be a string, got {other}"),
    }
}

/// Display names spanning what the modelled `RegistryDisplayName` admits: both published
/// length bounds, every script width, a combining sequence (which must not be normalised),
/// an astral-plane code point, and whitespace-bearing text.
fn display_name_cases() -> Vec<(&'static str, String)> {
    vec![
        ("the published lower bound: the empty name", String::new()),
        ("a single ascii character", "a".to_string()),
        (
            "ascii words with separators",
            "Emulator Public Registry (team-7)".to_string(),
        ),
        ("digits only", "1234567890".to_string()),
        (
            "latin-1 punctuation and accents",
            "Café ACME · Containers".to_string(),
        ),
        (
            "a decomposed combining sequence, which must survive unnormalised",
            "Cafe\u{301} Containers".to_string(),
        ),
        ("cjk text", "日本語のレジストリ".to_string()),
        ("right-to-left text", "مرحبا بالعالم".to_string()),
        (
            "astral-plane code points",
            "containers 🚀🐳 by 𝒜𝒞𝑀𝐸".to_string(),
        ),
        (
            "interior whitespace, including a newline and a tab",
            "line one\nline\ttwo".to_string(),
        ),
        (
            "json-significant characters",
            r#"{"quoted": "name"} \ /"#.to_string(),
        ),
        (
            "the published upper bound: 100 ascii characters",
            "n".repeat(MAX_DISPLAY_NAME_LEN),
        ),
        (
            "the published upper bound in multi-byte characters",
            "名".repeat(MAX_DISPLAY_NAME_LEN),
        ),
    ]
}

#[tokio::test]
async fn an_unset_registry_catalog_data_reports_no_display_name_and_reading_it_changes_nothing() {
    let router = service();
    let case = "a registry nothing has written to";

    // The output member is @required, so the object is always there; the registry simply
    // carries no display name until a write stores one.
    for read in 1..=3 {
        let body = ok(&router, "GetRegistryCatalogData", json!({}), case).await;
        assert_eq!(
            display_name(&body, case),
            None,
            "case {case:?}: read {read} must report no displayName, got {body}"
        );
    }

    // A read is a report: it must not have minted a record that a later write then
    // merges into. The first write still lands as the whole catalog.
    let written = ok(
        &router,
        "PutRegistryCatalogData",
        json!({"displayName": "after the reads"}),
        case,
    )
    .await;
    assert_eq!(
        display_name(&written, case).as_deref(),
        Some("after the reads"),
        "case {case:?}: the first write must report what it stored, got {written}"
    );
}

#[tokio::test]
async fn every_accepted_display_name_round_trips_verbatim_through_a_later_read() {
    let router = service();

    for (case, name) in display_name_cases() {
        let written = ok(
            &router,
            "PutRegistryCatalogData",
            json!({"displayName": name}),
            case,
        )
        .await;
        assert_eq!(
            display_name(&written, case).as_deref(),
            Some(name.as_str()),
            "case {case:?}: the write must report the name it stored, got {written}"
        );

        // Read back through the reporting operation — twice, because a read must not
        // change what the next caller sees.
        for read in 1..=2 {
            let got = ok(&router, "GetRegistryCatalogData", json!({}), case).await;
            assert_eq!(
                display_name(&got, case).as_deref(),
                Some(name.as_str()),
                "case {case:?}: read {read} must report the stored name, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn a_display_name_of_every_admitted_length_round_trips_and_one_past_the_bound_is_refused() {
    let router = service();

    // The size dimension, swept exhaustively over the modelled range: every length the
    // 0..100 bound admits, in a repeating multi-script pattern so each length carries a
    // different byte count as well as a different code-point count.
    let alphabet: Vec<char> = "aZ9 _.-é名🚀".chars().collect();
    for length in 0..=MAX_DISPLAY_NAME_LEN {
        let name: String = (0..length).map(|i| alphabet[i % alphabet.len()]).collect();
        let case = format!("a display name of {length} code points");

        let written = ok(
            &router,
            "PutRegistryCatalogData",
            json!({"displayName": name}),
            &case,
        )
        .await;
        assert_eq!(
            display_name(&written, &case).as_deref(),
            Some(name.as_str()),
            "case {case:?}: the write must report the name it stored, got {written}"
        );

        let got = ok(&router, "GetRegistryCatalogData", json!({}), &case).await;
        assert_eq!(
            display_name(&got, &case).as_deref(),
            Some(name.as_str()),
            "case {case:?}: the read must report the stored name, got {got}"
        );
    }

    // One past the published bound: refused with the one client-input error this model
    // declares for the operation (`InvalidParameterException`, HTTP 400) — and the
    // registry keeps what the last ACCEPTED write stored.
    let at_the_bound: String = (0..MAX_DISPLAY_NAME_LEN)
        .map(|i| alphabet[i % alphabet.len()])
        .collect();
    for (case, name) in [
        (
            "one ascii character past the bound",
            "n".repeat(MAX_DISPLAY_NAME_LEN + 1),
        ),
        (
            "one multi-byte character past the bound",
            "名".repeat(MAX_DISPLAY_NAME_LEN + 1),
        ),
        (
            "far past the bound",
            "n".repeat(MAX_DISPLAY_NAME_LEN * 10),
        ),
    ] {
        refused(
            &router,
            "PutRegistryCatalogData",
            json!({"displayName": name}),
            "InvalidParameterException",
            case,
        )
        .await;

        let got = ok(&router, "GetRegistryCatalogData", json!({}), case).await;
        assert_eq!(
            display_name(&got, case).as_deref(),
            Some(at_the_bound.as_str()),
            "case {case:?}: a refused write must leave the stored name alone, got {got}"
        );
    }
}

#[tokio::test]
async fn the_registry_carries_one_catalog_whose_last_write_wins() {
    let router = service();

    // A sequence of writes over the same single catalog: after each one, the registry
    // reports that write's name and nothing else — there is no second record and no
    // accumulation.
    let sequence = [
        Some("first name"),
        Some("second name"),
        Some(""),
        Some("日本語"),
        // A write that names no displayName is still a PUT of the whole catalog, so it
        // leaves the registry with no display name.
        None,
        Some("named again"),
        None,
    ];

    for (step, expected) in sequence.iter().enumerate() {
        let case = format!("write {step} of the sequence");
        let body = match expected {
            Some(name) => json!({"displayName": name}),
            None => json!({}),
        };
        let written = ok(&router, "PutRegistryCatalogData", body, &case).await;
        assert_eq!(
            display_name(&written, &case).as_deref(),
            *expected,
            "case {case:?}: the write must report the catalog it stored, got {written}"
        );

        let got = ok(&router, "GetRegistryCatalogData", json!({}), &case).await;
        assert_eq!(
            display_name(&got, &case).as_deref(),
            *expected,
            "case {case:?}: the read must report only the last write, got {got}"
        );
    }
}

#[tokio::test]
async fn concurrent_writes_leave_exactly_one_of_them_stored() {
    let router = service();
    let case = "four concurrent writes to the one catalog";

    let candidates = [
        "concurrent name 0",
        "concurrent name 1",
        "concurrent name 2",
        "concurrent name 3",
    ];
    let router = &router;
    let write = |name: &'static str| async move {
        let body = ok(
            router,
            "PutRegistryCatalogData",
            json!({"displayName": name}),
            case,
        )
        .await;
        display_name(&body, case)
    };

    // Every write must be accepted, and each must report a name that was actually
    // submitted — no interleaving may merge two writes into one answer.
    let reported = tokio::join!(
        write(candidates[0]),
        write(candidates[1]),
        write(candidates[2]),
        write(candidates[3]),
    );
    for answer in [reported.0, reported.1, reported.2, reported.3] {
        let answer = answer
            .unwrap_or_else(|| panic!("case {case:?}: every concurrent write must store a name"));
        assert!(
            candidates.contains(&answer.as_str()),
            "case {case:?}: a write reported {answer:?}, which no caller submitted"
        );
    }

    // Last write wins: one of the submitted names is stored, and it stays stored.
    let first = ok(router, "GetRegistryCatalogData", json!({}), case).await;
    let settled = display_name(&first, case)
        .unwrap_or_else(|| panic!("case {case:?}: the catalog must carry a name, got {first}"));
    assert!(
        candidates.contains(&settled.as_str()),
        "case {case:?}: the stored name {settled:?} was never submitted"
    );
    let second = ok(router, "GetRegistryCatalogData", json!({}), case).await;
    assert_eq!(
        display_name(&second, case).as_deref(),
        Some(settled.as_str()),
        "case {case:?}: the settled name must be stable across reads, got {second}"
    );
}

#[tokio::test]
async fn an_unmodelled_member_on_a_write_is_ignored_and_the_catalog_is_unaffected() {
    let router = service();
    let case = "a write carrying a member the model does not declare";

    ok(
        &router,
        "PutRegistryCatalogData",
        json!({"displayName": "modelled name", "registryId": "000000000000"}),
        case,
    )
    .await;
    let got = ok(&router, "GetRegistryCatalogData", json!({}), case).await;
    assert_eq!(
        display_name(&got, case).as_deref(),
        Some("modelled name"),
        "case {case:?}: only the modelled displayName is stored, got {got}"
    );

    // `GetRegistryCatalogDataRequest` declares no members, so a read that carries one is
    // still the same report.
    let with_member = ok(
        &router,
        "GetRegistryCatalogData",
        json!({"registryId": "000000000000"}),
        case,
    )
    .await;
    assert_eq!(
        display_name(&with_member, case).as_deref(),
        Some("modelled name"),
        "case {case:?}: the read reports the stored name, got {with_member}"
    );
}

#[tokio::test]
async fn the_registry_catalog_is_not_a_repository_catalog() {
    let router = service();
    let case = "a registry display name beside a repository";

    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        case,
    )
    .await;
    ok(
        &router,
        "PutRegistryCatalogData",
        json!({"displayName": "registry level"}),
        case,
    )
    .await;

    // Creating or deleting a repository touches nothing in the registry's own catalog:
    // `RegistryCatalogData` is "the metadata for a public registry", one per registry.
    ok(
        &router,
        "DeleteRepository",
        json!({"repositoryName": "team/app"}),
        case,
    )
    .await;
    let got = ok(&router, "GetRegistryCatalogData", json!({}), case).await;
    assert_eq!(
        display_name(&got, case).as_deref(),
        Some("registry level"),
        "case {case:?}: repository lifecycle must not change the registry catalog, got {got}"
    );

    // And a registry display name is not a repository catalog: the repository-level
    // reader is a separate operation, still its own gap issue.
    let (status, parsed) = call(
        &router,
        "GetRepositoryCatalogData",
        json!({"repositoryName": "team/app"}),
    )
    .await;
    assert_eq!(
        status, 501,
        "case {case:?}: the repository-level reader is still a scaffold, got {status} \
         with {parsed}"
    );
}
