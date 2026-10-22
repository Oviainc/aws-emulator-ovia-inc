// SPDX-License-Identifier: CC0-1.0

//! `GetBlueprints` — the instance-image (blueprint) catalog, driven over the wire.
//!
//! Every wire case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetBlueprints` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than bypassed.
//!
//! The rules under test, asserted over the WHOLE catalog and over sets of
//! requests rather than one recorded call:
//!
//! * every reported blueprint's members are DERIVED from its catalog entry: the
//!   friendly `name` is the id title-cased (the derivation
//!   `Instance$blueprintName` reports, so an instance created from the blueprint
//!   carries the same name), `group` is the id's family — the id with its version
//!   words removed and underscores written as hyphens, as `Blueprint$group`'s own
//!   `amazon-linux` example shows — `version` is the id's version words joined
//!   with dots (absent when the id names none), `platform` is `WINDOWS` for
//!   exactly the Windows images and `LINUX_UNIX` otherwise, and `minPower` is 0,
//!   "the blueprint runs on all instance sizes";
//! * `includeInactive` is what widens the answer from the active entries to every
//!   published one, and an absent flag reports the active ones only;
//! * `appCategory` selects the catalog: a request naming a category reports the
//!   Lightsail for Research blueprints (this emulator publishes none, so that
//!   list is empty), one naming none reports the instance blueprints, and a
//!   category outside the modelled `AppCategory` enum is refused by the
//!   constraint walk with a 400 `ValidationException`;
//! * a page is the reported list from the entry `pageToken` names onward, bounded
//!   by the page size, with `nextPageToken` present only while entries remain —
//!   so the walk reports each entry in order, exactly once — and a token that
//!   names no reported blueprint is a modelled 400 `InvalidInputException`;
//! * only an ACTIVE blueprint can be created from, and the instance then reports
//!   the catalog's own name for it; and
//! * the operation reports, never mutates: identical calls answer identically.
//!
//! The paging and slicing rules are additionally exercised directly against
//! `state::blueprints_from` / `state::blueprint_page` over SYNTHETIC catalogs, at
//! page sizes the published catalog (answered whole in one page) cannot reach.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The front door every suite in this test target drives the service through, and
// the awsJson error-code reader, both already implemented beside this suite.
use super::instance_lifecycle::{error_code, Emulator};

/// The operation under test.
const OPERATION: &str = "GetBlueprints";

/// The page size the service publishes for this operation, read from the crate
/// rather than restated here, so a page that grows stays bounded by it.
const PAGE_SIZE: usize = state::BLUEPRINT_PAGE_SIZE;

/// The two values of the modelled `BlueprintType` enum
/// (`smithy_query(query="shape",shape="BlueprintType")`).
const BLUEPRINT_TYPES: &[&str] = &["os", "app"];

/// The two values of the modelled `InstancePlatform` enum
/// (`smithy_query(query="shape",shape="InstancePlatform")`).
const PLATFORMS: &[&str] = &["LINUX_UNIX", "WINDOWS"];

/// One expected catalog entry.
///
/// Source: the blueprint ids this service publishes (`state::BLUEPRINTS`, whose
/// own source is the Lightsail instance-image list), chosen to span the class the
/// derivations generalise over: an operating system and an application stack, a
/// Windows image and Linux/Unix ones, an id whose version is one word, two words
/// and three, an id carrying no version at all, and a retired (inactive) entry.
/// None of these entries is one the issue cited.
struct Expected {
    blueprint_id: &'static str,
    blueprint_type: &'static str,
    name: &'static str,
    group: &'static str,
    /// `None` where the id names no version, in which case the member must be
    /// ABSENT rather than invented.
    version: Option<&'static str>,
    platform: &'static str,
    is_active: bool,
}

const EXPECTED: &[Expected] = &[
    Expected {
        blueprint_id: "amazon_linux_2023",
        blueprint_type: "os",
        name: "Amazon Linux 2023",
        group: "amazon-linux",
        version: Some("2023"),
        platform: "LINUX_UNIX",
        is_active: true,
    },
    Expected {
        blueprint_id: "centos_stream_9",
        blueprint_type: "os",
        name: "Centos Stream 9",
        group: "centos-stream",
        version: Some("9"),
        platform: "LINUX_UNIX",
        is_active: true,
    },
    Expected {
        blueprint_id: "ubuntu_22_04",
        blueprint_type: "os",
        name: "Ubuntu 22 04",
        group: "ubuntu",
        version: Some("22.04"),
        platform: "LINUX_UNIX",
        is_active: true,
    },
    Expected {
        blueprint_id: "ubuntu_18_04",
        blueprint_type: "os",
        name: "Ubuntu 18 04",
        group: "ubuntu",
        version: Some("18.04"),
        platform: "LINUX_UNIX",
        is_active: false,
    },
    Expected {
        blueprint_id: "debian_10",
        blueprint_type: "os",
        name: "Debian 10",
        group: "debian",
        version: Some("10"),
        platform: "LINUX_UNIX",
        is_active: false,
    },
    Expected {
        blueprint_id: "windows_server_2019",
        blueprint_type: "os",
        name: "Windows Server 2019",
        group: "windows-server",
        version: Some("2019"),
        platform: "WINDOWS",
        is_active: true,
    },
    Expected {
        blueprint_id: "wordpress",
        blueprint_type: "app",
        name: "Wordpress",
        group: "wordpress",
        version: None,
        platform: "LINUX_UNIX",
        is_active: true,
    },
    Expected {
        blueprint_id: "lamp_8_bitnami",
        blueprint_type: "app",
        name: "Lamp 8 Bitnami",
        group: "lamp-bitnami",
        version: Some("8"),
        platform: "LINUX_UNIX",
        is_active: true,
    },
    Expected {
        blueprint_id: "plesk_ubuntu_18_0_35",
        blueprint_type: "app",
        name: "Plesk Ubuntu 18 0 35",
        group: "plesk-ubuntu",
        version: Some("18.0.35"),
        platform: "LINUX_UNIX",
        is_active: true,
    },
    Expected {
        blueprint_id: "cpanel_whm_almalinux",
        blueprint_type: "app",
        name: "Cpanel Whm Almalinux",
        group: "cpanel-whm-almalinux",
        version: None,
        platform: "LINUX_UNIX",
        is_active: true,
    },
];

// ---------------------------------------------------------------------------
// The derivations, restated here as the rule rather than read off the crate
// ---------------------------------------------------------------------------

/// Whether a word of a blueprint id names a version: one that begins with a
/// digit.
fn is_version_word(word: &str) -> bool {
    word.starts_with(|first: char| first.is_ascii_digit())
}

/// The family of a blueprint id: its non-version words, hyphen-joined.
fn want_group(blueprint_id: &str) -> String {
    blueprint_id
        .split('_')
        .filter(|word| !word.is_empty() && !is_version_word(word))
        .collect::<Vec<_>>()
        .join("-")
}

/// The version a blueprint id names: its version words, dot-joined, or `None`.
fn want_version(blueprint_id: &str) -> Option<String> {
    let version = blueprint_id
        .split('_')
        .filter(|word| is_version_word(word))
        .collect::<Vec<_>>()
        .join(".");
    (!version.is_empty()).then_some(version)
}

/// The friendly name of a blueprint id: its words title-cased, space-joined.
fn want_name(blueprint_id: &str) -> String {
    blueprint_id
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The platform of a blueprint id: Windows for exactly the Windows images.
fn want_platform(blueprint_id: &str) -> &'static str {
    if blueprint_id.starts_with("windows") {
        "WINDOWS"
    } else {
        "LINUX_UNIX"
    }
}

/// The ids the published catalog holds, in its own order, filtered the way
/// `includeInactive` filters.
fn published_ids(include_inactive: bool) -> Vec<String> {
    state::BLUEPRINTS
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .map(|spec| spec.blueprint_id.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// This suite's reads, over the shared harness
// ---------------------------------------------------------------------------

/// One page of the catalog: the page's blueprints and its `nextPageToken`.
async fn page(emulator: &Emulator, request: Value) -> (Vec<Value>, Option<String>) {
    let case = request.clone();
    let body = emulator.ok(OPERATION, request).await;
    let blueprints = body["blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a blueprints array, got {body:?}"))
        .clone();
    let next = body["nextPageToken"].as_str().map(str::to_string);
    (blueprints, next)
}

/// Every blueprint the walk from `request` reports, in page order, together with
/// the size of each page it took.
async fn walk(emulator: &Emulator, request: Value) -> (Vec<Value>, Vec<usize>) {
    let mut blueprints = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let (reported, next) = page(emulator, body).await;
        page_sizes.push(reported.len());
        blueprints.extend(reported);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case {request:?}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (blueprints, page_sizes)
}

/// A member of a blueprint, as a string, or a panic naming the case.
fn member(blueprint: &Value, name: &str) -> String {
    blueprint[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {blueprint:?}: expected a string {name}"))
        .to_string()
}

/// The reported ids of a list of blueprints.
fn ids(blueprints: &[Value]) -> Vec<String> {
    blueprints
        .iter()
        .map(|blueprint| member(blueprint, "blueprintId"))
        .collect()
}

// ---------------------------------------------------------------------------
// The reported members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_reported_blueprint_derives_its_members_from_its_catalog_entry() {
    let emulator = Emulator::new();
    let (blueprints, _) = walk(&emulator, json!({"includeInactive": true})).await;

    assert!(
        !blueprints.is_empty(),
        "case whole catalog: expected at least one blueprint, got none"
    );

    for blueprint in &blueprints {
        let case = member(blueprint, "blueprintId");
        assert!(
            !case.is_empty(),
            "case {blueprint:?}: expected a non-empty blueprintId"
        );

        let blueprint_type = member(blueprint, "type");
        assert!(
            BLUEPRINT_TYPES.contains(&blueprint_type.as_str()),
            "case {case:?}: type must be one of {BLUEPRINT_TYPES:?}, got {blueprint_type:?}"
        );

        assert_eq!(
            member(blueprint, "name"),
            want_name(&case),
            "case {case:?}: name must be the id title-cased, got {blueprint:?}"
        );
        assert_eq!(
            member(blueprint, "group"),
            want_group(&case),
            "case {case:?}: group must be the id's family, got {blueprint:?}"
        );
        assert_eq!(
            blueprint["version"].as_str().map(str::to_string),
            want_version(&case),
            "case {case:?}: version must be the id's version words joined with dots, \
             got {blueprint:?}"
        );

        let platform = member(blueprint, "platform");
        assert!(
            PLATFORMS.contains(&platform.as_str()),
            "case {case:?}: platform must be one of {PLATFORMS:?}, got {platform:?}"
        );
        assert_eq!(
            platform,
            want_platform(&case),
            "case {case:?}: Windows is the platform of exactly the Windows images, \
             got {blueprint:?}"
        );

        assert!(
            blueprint["isActive"].is_boolean(),
            "case {case:?}: expected a boolean isActive, got {blueprint:?}"
        );
        assert_eq!(
            blueprint["minPower"],
            json!(0),
            "case {case:?}: every published blueprint runs on all instance sizes, which \
             minPower 0 states, got {blueprint:?}"
        );

        // The description is derived from the entry's own name and type, so it
        // names the blueprint and says which kind of image it is.
        let description = member(blueprint, "description");
        assert!(
            description.contains(&want_name(&case)),
            "case {case:?}: expected the description to name the blueprint, got {description:?}"
        );
        let expected_kind = if blueprint_type == "app" {
            "application stack"
        } else {
            "operating system image"
        };
        assert!(
            description.contains(expected_kind),
            "case {case:?}: expected a {expected_kind} description, got {description:?}"
        );

        // Lightsail's per-image revision code and its product and licence links
        // are not data this catalog publishes, and all three members are
        // optional: they must be ABSENT rather than invented.
        for absent in ["versionCode", "productUrl", "licenseUrl", "appCategory"] {
            assert!(
                blueprint[absent].is_null(),
                "case {case:?}: {absent} is not published by this catalog, so it must be \
                 absent, got {blueprint:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_published_sample_entries_report_their_expected_members() {
    let emulator = Emulator::new();
    let (blueprints, _) = walk(&emulator, json!({"includeInactive": true})).await;
    let reported = ids(&blueprints);

    for want in EXPECTED {
        let case = want.blueprint_id;
        let found = blueprints
            .iter()
            .find(|blueprint| member(blueprint, "blueprintId") == case)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the catalog to carry it, got {reported:?}")
            });

        assert_eq!(
            member(found, "type"),
            want.blueprint_type,
            "case {case:?}: wrong type, got {found:?}"
        );
        assert_eq!(
            member(found, "name"),
            want.name,
            "case {case:?}: wrong name, got {found:?}"
        );
        assert_eq!(
            member(found, "group"),
            want.group,
            "case {case:?}: wrong group, got {found:?}"
        );
        assert_eq!(
            found["version"].as_str(),
            want.version,
            "case {case:?}: wrong version, got {found:?}"
        );
        assert_eq!(
            member(found, "platform"),
            want.platform,
            "case {case:?}: wrong platform, got {found:?}"
        );
        assert_eq!(
            found["isActive"],
            json!(want.is_active),
            "case {case:?}: wrong isActive, got {found:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// includeInactive
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inactive_blueprints_are_reported_only_when_the_request_asks_for_them() {
    let emulator = Emulator::new();

    // Rows: the flag absent, sent false, and sent true.
    for (request, include_inactive) in [
        (json!({}), false),
        (json!({"includeInactive": false}), false),
        (json!({"includeInactive": true}), true),
    ] {
        let case = request.clone();
        let (blueprints, _) = walk(&emulator, request).await;
        let reported = ids(&blueprints);

        assert_eq!(
            reported,
            published_ids(include_inactive),
            "case {case:?}: expected the published catalog in its own order, got {reported:?}"
        );
        if !include_inactive {
            for blueprint in &blueprints {
                assert_eq!(
                    blueprint["isActive"],
                    json!(true),
                    "case {case:?}: an answer that did not ask for inactive blueprints \
                     carries active ones only, got {blueprint:?}"
                );
            }
        }
    }

    // The entries the flag adds are exactly the inactive ones, and they are
    // added where the catalog holds them rather than appended.
    let (active_only, _) = walk(&emulator, json!({})).await;
    let (everything, _) = walk(&emulator, json!({"includeInactive": true})).await;
    let active_ids = ids(&active_only);
    let all_ids = ids(&everything);
    let added: Vec<&String> = all_ids
        .iter()
        .filter(|id| !active_ids.contains(id))
        .collect();
    let inactive_published: Vec<String> = state::BLUEPRINTS
        .iter()
        .filter(|spec| !spec.is_active)
        .map(|spec| spec.blueprint_id.to_string())
        .collect();
    assert_eq!(
        added.len(),
        inactive_published.len(),
        "case includeInactive: expected the inactive entries {inactive_published:?} to be \
         what the flag adds, got {added:?}"
    );
    for id in &inactive_published {
        assert!(
            all_ids.contains(id),
            "case includeInactive: expected {id:?} among {all_ids:?}"
        );
        assert!(
            !active_ids.contains(id),
            "case default: expected {id:?} to be withheld, got {active_ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// appCategory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_naming_an_app_category_reports_the_research_catalog() {
    let emulator = Emulator::new();

    let (blueprints, next) = page(&emulator, json!({"appCategory": "LfR"})).await;
    // This emulator publishes no Lightsail for Research blueprint, so the
    // Research catalog is empty — and an empty list is still a list, with no
    // next page to walk to.
    assert!(
        blueprints.is_empty(),
        "case appCategory LfR: this emulator publishes no Research blueprint, so the \
         Research catalog is empty, got {blueprints:?}"
    );
    assert_eq!(
        blueprints.len(),
        state::LFR_BLUEPRINTS.len(),
        "case appCategory LfR: expected the published Research catalog, got {blueprints:?}"
    );
    assert_eq!(
        next, None,
        "case appCategory LfR: a last page carries no nextPageToken, got {next:?}"
    );

    // The category SELECTS the catalog rather than filtering the standard one, so
    // a request naming one can never report an instance blueprint: the two
    // catalogs the selection answers from are disjoint.
    let standard_catalog: Vec<&str> = state::blueprint_catalog(None)
        .iter()
        .map(|spec| spec.blueprint_id)
        .collect();
    let research_catalog: Vec<&str> = state::blueprint_catalog(Some("LfR"))
        .iter()
        .map(|spec| spec.blueprint_id)
        .collect();
    assert_eq!(
        standard_catalog,
        published_ids(true),
        "case appCategory absent: expected the instance blueprints, got {standard_catalog:?}"
    );
    for id in &research_catalog {
        assert!(
            !standard_catalog.contains(id),
            "case appCategory LfR: the Research catalog must not hold an instance \
             blueprint, got {id:?}"
        );
    }

    // And the instance catalog is unaffected by a Research read.
    let (standard, _) = walk(&emulator, json!({})).await;
    assert_eq!(
        ids(&standard),
        published_ids(false),
        "case after a Research read: the instance catalog must be unchanged"
    );
    for id in &ids(&blueprints) {
        assert!(
            !published_ids(true).contains(id),
            "case appCategory LfR: an instance blueprint must not be reported as a \
             Research one, got {id:?}"
        );
    }
}

#[tokio::test]
async fn an_app_category_outside_the_modelled_enum_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a category can fail to name an `AppCategory`
    // member: wrong case, a longer form, an unrelated word, an empty string and
    // a padded value. The member carries the modelled enum's value set, so the
    // refusal is the constraint walk's `ValidationException`, before the handler.
    for category in ["lfr", "LFR", "LightsailForResearch", "", "LfR "] {
        let (status, body) = emulator
            .call(OPERATION, json!({"appCategory": category}))
            .await;
        assert_eq!(
            status, 400,
            "case appCategory {category:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case appCategory {category:?}: expected the modelled enum's value set to be \
             enforced, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_page_token_resumes_the_catalog_at_the_blueprint_it_names() {
    let emulator = Emulator::new();

    for request in [json!({}), json!({"includeInactive": true})] {
        let case = request.clone();
        let (whole, page_sizes) = walk(&emulator, request.clone()).await;
        let all_ids = ids(&whole);

        // Each page is bounded by the published page size, and `nextPageToken` is
        // present exactly while entries remain: so every page but the last is
        // full, and the pages account for every entry.
        for (index, size) in page_sizes.iter().enumerate() {
            assert!(
                *size <= PAGE_SIZE,
                "case {case:?} page {index}: a page carries at most {PAGE_SIZE} blueprints, \
                 got {size}"
            );
            if index + 1 != page_sizes.len() {
                assert_eq!(
                    *size, PAGE_SIZE,
                    "case {case:?} page {index}: a page followed by another must be full, \
                     got {size}"
                );
            }
        }
        assert_eq!(
            all_ids.len(),
            page_sizes.iter().sum::<usize>(),
            "case {case:?}: the pages must account for every blueprint, got {page_sizes:?}"
        );
        let unique: BTreeSet<&String> = all_ids.iter().collect();
        assert_eq!(
            unique.len(),
            all_ids.len(),
            "case {case:?}: the walk reported a blueprint twice: {all_ids:?}"
        );

        // Every reported id is a token, including the first and the last: the
        // page it opens is the reported list from there on.
        for (index, token) in all_ids.iter().enumerate() {
            let mut body = request.clone();
            body["pageToken"] = json!(token);
            let (suffix, _) = walk(&emulator, body).await;
            assert_eq!(
                ids(&suffix),
                all_ids[index..],
                "case {case:?} pageToken {token:?}: expected the reported list from there on"
            );
        }

        // An absent token and an empty one both ask for the first page.
        for token in [None, Some("")] {
            let mut body = request.clone();
            if let Some(token) = token {
                body["pageToken"] = json!(token);
            }
            let (first_page, _) = page(&emulator, body).await;
            assert_eq!(
                ids(&first_page),
                all_ids[..page_sizes[0]],
                "case {case:?} pageToken {token:?}: expected the first page"
            );
        }
    }
}

#[tokio::test]
async fn a_page_token_that_names_no_reported_blueprint_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a token can fail to name an entry of the
    // reported list: an id that is not published, a family without its version, a
    // version that is not served, the group form, whitespace, an opaque cursor
    // from elsewhere — and an id that IS published but is withheld from this
    // request, which the default request withholds because it is inactive.
    for token in [
        "not_a_blueprint",
        "ubuntu",
        "ubuntu_19_04",
        "amazon-linux",
        " ",
        "AAAAAAAA",
        "debian_10",
    ] {
        let (status, body) = emulator.call(OPERATION, json!({"pageToken": token})).await;
        assert_eq!(
            status, 400,
            "case pageToken {token:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case pageToken {token:?}: expected the modelled InvalidInputException, got {body:?}"
        );
    }

    // The same token, on a request that DOES report that blueprint, opens a page
    // at it: the token is resolved against the list the request asks for.
    let (resumed, _) = page(
        &emulator,
        json!({"pageToken": "debian_10", "includeInactive": true}),
    )
    .await;
    assert_eq!(
        ids(&resumed).first().map(String::as_str),
        Some("debian_10"),
        "case pageToken debian_10 with includeInactive: expected the page to open at it, \
         got {resumed:?}"
    );
}

// ---------------------------------------------------------------------------
// Paging and slicing over synthetic catalogs
// ---------------------------------------------------------------------------

/// A synthetic catalog: two active entries, a retired one between them and two
/// more, so a page boundary can fall either side of the inactive entry.
const SYNTHETIC: &[state::BlueprintSpec] = &[
    state::BlueprintSpec {
        blueprint_id: "synthetic_os_1",
        blueprint_type: state::BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    state::BlueprintSpec {
        blueprint_id: "synthetic_os_2",
        blueprint_type: state::BLUEPRINT_TYPE_OS,
        is_active: false,
    },
    state::BlueprintSpec {
        blueprint_id: "synthetic_app",
        blueprint_type: state::BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    state::BlueprintSpec {
        blueprint_id: "windows_synthetic_2030",
        blueprint_type: state::BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    state::BlueprintSpec {
        blueprint_id: "synthetic_retired_3",
        blueprint_type: state::BLUEPRINT_TYPE_APP,
        is_active: false,
    },
];

#[test]
fn the_slice_rule_holds_over_a_catalog_that_mixes_active_and_retired_entries() {
    for include_inactive in [false, true] {
        let reported = state::blueprints_from(SYNTHETIC, include_inactive, None);
        let want: Vec<&str> = SYNTHETIC
            .iter()
            .filter(|spec| include_inactive || spec.is_active)
            .map(|spec| spec.blueprint_id)
            .collect();
        let got: Vec<String> = reported
            .iter()
            .map(|blueprint| blueprint.blueprint_id.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            got, want,
            "case includeInactive {include_inactive}: expected the catalog's own order, \
             got {got:?}"
        );
        for blueprint in &reported {
            assert_eq!(
                blueprint.app_category, None,
                "case includeInactive {include_inactive}: a standard blueprint carries no \
                 appCategory, got {blueprint:?}"
            );
        }
    }

    // A catalog read for a category stamps every entry with it: that is the
    // category of the catalog the entry came from.
    for blueprint in state::blueprints_from(SYNTHETIC, true, Some("LfR")) {
        assert_eq!(
            blueprint.app_category.as_deref(),
            Some("LfR"),
            "case appCategory LfR: expected the entry to carry its category, got {blueprint:?}"
        );
    }
}

#[test]
fn a_paged_walk_reports_every_entry_once_at_any_page_size() {
    let reported = state::blueprints_from(SYNTHETIC, true, None);
    let all_ids: Vec<String> = reported
        .iter()
        .map(|blueprint| blueprint.blueprint_id.clone().unwrap_or_default())
        .collect();

    // Rows: page sizes below, at and above the catalog's size, plus 0 — which
    // cannot mean "an empty page forever", so it carries one entry.
    for page_size in [0usize, 1, 2, 3, 5, 9] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (page, next) = state::blueprint_page(&reported, page_size, token.as_deref())
                .unwrap_or_else(|| {
                    panic!("case page size {page_size}: token {token:?} must name an entry")
                });
            assert!(
                !page.is_empty(),
                "case page size {page_size}: a page over a non-empty catalog carries at \
                 least one entry"
            );
            assert!(
                page.len() <= page_size.max(1),
                "case page size {page_size}: a page carries at most that many entries, \
                 got {}",
                page.len()
            );
            walked.extend(
                page.iter()
                    .map(|blueprint| blueprint.blueprint_id.clone().unwrap_or_default()),
            );
            pages += 1;
            assert!(
                pages <= all_ids.len(),
                "case page size {page_size}: the walk did not terminate, got {walked:?}"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, all_ids,
            "case page size {page_size}: expected every entry once, in order"
        );
    }

    // A token that names no entry of the list has no page.
    for token in ["synthetic", "nope", ""] {
        assert!(
            state::blueprint_page(&reported, 2, Some(token)).is_none(),
            "case pageToken {token:?}: expected no page"
        );
    }
}

// ---------------------------------------------------------------------------
// The catalog against the rest of the service
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_instance_can_be_created_from_an_active_reported_blueprint_only() {
    let emulator = Emulator::new();
    let (reported, _) = walk(&emulator, json!({"includeInactive": true})).await;

    // Rows: an operating system, a Windows image, an application stack — and a
    // retired entry, which "Use active blueprints when creating new instances"
    // refuses.
    for (index, case) in ["amazon_linux_2023", "windows_server_2019", "wordpress", "debian_10"]
        .iter()
        .enumerate()
    {
        let blueprint = reported
            .iter()
            .find(|blueprint| member(blueprint, "blueprintId") == *case)
            .unwrap_or_else(|| panic!("case {case:?}: expected the catalog to carry it"));
        let is_active = blueprint["isActive"] == json!(true);
        let name = format!("blueprint-row-{index}");
        let body = json!({
            "instanceNames": [name],
            "availabilityZone": "us-east-1a",
            "blueprintId": case,
            "bundleId": "micro_3_0",
        });

        if !is_active {
            let (status, refused) = emulator.call("CreateInstances", body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an inactive blueprint cannot be created from, got {refused:?}"
            );
            // `CreateInstances` answers a blueprint that cannot be launched from
            // with Lightsail's `DoesNotExist` envelope — HTTP 400, wire error
            // `NotFoundException`, "does not exist or is deprecated" — which is
            // the refusal a retired entry earns.
            assert_eq!(
                error_code(&refused),
                "NotFoundException",
                "case {case:?}: expected the modelled NotFoundException, got {refused:?}"
            );
            continue;
        }

        emulator.ok("CreateInstances", body).await;
        let read = emulator
            .ok("GetInstance", json!({"instanceName": name}))
            .await;
        let instance = &read["instance"];
        assert_eq!(
            instance["blueprintId"].as_str(),
            Some(*case),
            "case {case:?}: expected the instance to name its blueprint, got {instance:?}"
        );
        // The catalog's `name` for a blueprint is the name an instance created
        // from it reports: one derivation, not two.
        assert_eq!(
            instance["blueprintName"].as_str(),
            blueprint["name"].as_str(),
            "case {case:?}: expected the catalog's own name, got {instance:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let first = emulator.ok(OPERATION, json!({})).await;
    let second = emulator.ok(OPERATION, json!({})).await;
    let after_other_reads = {
        let _ = walk(&emulator, json!({"includeInactive": true})).await;
        let _ = emulator.call(OPERATION, json!({"appCategory": "LfR"})).await;
        let _ = emulator
            .call(OPERATION, json!({"pageToken": "not_a_blueprint"}))
            .await;
        emulator.ok(OPERATION, json!({})).await
    };

    assert_eq!(
        first, second,
        "case repeated read: the catalog must answer identically, got {first:?} then {second:?}"
    );
    assert_eq!(
        first, after_other_reads,
        "case read after other reads: the catalog must answer identically, got {first:?} \
         then {after_other_reads:?}"
    );
}
