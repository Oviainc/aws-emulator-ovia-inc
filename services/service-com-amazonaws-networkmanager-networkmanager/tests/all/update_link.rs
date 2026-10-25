// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateLink`, driven over HTTP through the real router.
//!
//! The rule this suite pins is the one `update_site` pins for a site: an update of an
//! existing resource writes ONLY the members the request carries — an absent member
//! leaves the stored value as it was, a supplied one replaces it (an empty string
//! included: "To remove information for any of the parameters, specify an empty string",
//! `smithy_query query="doc" shape="UpdateLink"`) — and rewrites nothing else, so the
//! link keeps its id, its ARN, its `CreatedAt` and its tags. Addressing a link the store
//! does not hold under the path's global network is the missing-resource answer (404
//! `ResourceNotFoundException`) and stores nothing, and a descriptor outside its
//! documented constraints is refused (400 `ValidationException`) exactly as `CreateLink`
//! refuses it.
//!
//! Because the write lands in the store rather than in the answer alone, every reader of
//! the record answers from it: a link updated here leaves the listing of the descriptor it
//! no longer holds and joins the listing of the one it now does, keeps the site and the
//! global network it was created under, and is the same record however often one update is
//! repeated — while a later update replaces only the members it carries.
//!
//! Nothing reaches into `crate::state`: a link is only ever read back through `GetLinks`.
//! The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{
    error_code, expected_scoped_arn, ids_of, Emulator, PAST_SETTLE,
};

/// The members every seeded link starts with, so a row that supplies none of them has
/// values to leave alone.
const BASE_DESCRIPTION: &str = "baseline link";
/// See [`BASE_DESCRIPTION`].
const BASE_TYPE: &str = "broadband";
/// See [`BASE_DESCRIPTION`].
const BASE_PROVIDER: &str = "AnyCompany Networks";

/// The documented maximum length of a link's `Type` and `Provider`, in characters, and
/// the characters the documentation forbids in them: "Constraints: Maximum length of 128
/// characters. Cannot include the following characters: | \ ^" (`smithy_query query="doc"
/// shape="UpdateLinkRequest" member="Provider"`, the same sentence for `Type`).
const MAX_DESCRIPTOR_CHARS: usize = 128;
/// See [`MAX_DESCRIPTOR_CHARS`].
const FORBIDDEN_DESCRIPTOR_CHARS: [char; 3] = ['|', '\\', '^'];

/// The modelled maximum length of `Description` (`ConstrainedString`, len 0..256).
const MAX_DESCRIPTION_CHARS: usize = 256;

/// The `Bandwidth` every seeded link starts with.
fn base_bandwidth() -> Value {
    json!({ "UploadSpeed": 10, "DownloadSpeed": 20 })
}

/// The `Tags` every seeded link carries, which no update may rewrite.
fn base_tags() -> Value {
    json!([{ "Key": "env", "Value": "test" }])
}

/// One row of the update table: the request body, and the four mutable members the stored
/// record must hold afterwards.
struct UpdateCase {
    case: &'static str,
    body: Value,
    description: Value,
    link_type: Value,
    provider: Value,
    bandwidth: Value,
}

/// The rows of the update table. They differ along WHICH mutable members the request
/// carries (every one, a single one, none at all), and along the form of the value: an
/// empty string (the documented removal), the published maximum lengths, a non-ASCII and
/// multi-line value, and a `Bandwidth` carrying one speed, no speed and the largest
/// `Integer`.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "every mutable member supplied",
            body: json!({
                "Description": "updated link",
                "Type": "mpls",
                "Provider": "Example Telecom",
                "Bandwidth": { "UploadSpeed": 100, "DownloadSpeed": 200 },
            }),
            description: json!("updated link"),
            link_type: json!("mpls"),
            provider: json!("Example Telecom"),
            bandwidth: json!({ "UploadSpeed": 100, "DownloadSpeed": 200 }),
        },
        UpdateCase {
            case: "Description only, so the other members are left alone",
            body: json!({ "Description": "description only" }),
            description: json!("description only"),
            link_type: json!(BASE_TYPE),
            provider: json!(BASE_PROVIDER),
            bandwidth: base_bandwidth(),
        },
        UpdateCase {
            case: "Bandwidth only, replacing the whole structure with one speed",
            body: json!({ "Bandwidth": { "UploadSpeed": 5 } }),
            description: json!(BASE_DESCRIPTION),
            link_type: json!(BASE_TYPE),
            provider: json!(BASE_PROVIDER),
            bandwidth: json!({ "UploadSpeed": 5 }),
        },
        UpdateCase {
            case: "an empty Bandwidth removes both speeds",
            body: json!({ "Bandwidth": {} }),
            description: json!(BASE_DESCRIPTION),
            link_type: json!(BASE_TYPE),
            provider: json!(BASE_PROVIDER),
            bandwidth: json!({}),
        },
        UpdateCase {
            case: "no mutable member at all, so nothing changes",
            body: json!({}),
            description: json!(BASE_DESCRIPTION),
            link_type: json!(BASE_TYPE),
            provider: json!(BASE_PROVIDER),
            bandwidth: base_bandwidth(),
        },
        UpdateCase {
            case: "empty strings remove the information the descriptors held",
            body: json!({ "Description": "", "Type": "", "Provider": "" }),
            description: json!(""),
            link_type: json!(""),
            provider: json!(""),
            bandwidth: base_bandwidth(),
        },
        UpdateCase {
            case: "descriptors at their published maximum lengths",
            body: json!({
                "Description": "d".repeat(MAX_DESCRIPTION_CHARS),
                "Type": "t".repeat(MAX_DESCRIPTOR_CHARS),
                "Provider": "p".repeat(MAX_DESCRIPTOR_CHARS),
            }),
            description: json!("d".repeat(MAX_DESCRIPTION_CHARS)),
            link_type: json!("t".repeat(MAX_DESCRIPTOR_CHARS)),
            provider: json!("p".repeat(MAX_DESCRIPTOR_CHARS)),
            bandwidth: base_bandwidth(),
        },
        UpdateCase {
            case: "multi-line and non-ascii values, and the largest Integer speeds",
            body: json!({
                "Description": "line one\nline two \u{1f310}",
                "Type": "fibre \u{1f310}",
                "Provider": "Ünternet",
                "Bandwidth": { "UploadSpeed": i32::MAX, "DownloadSpeed": i32::MAX },
            }),
            description: json!("line one\nline two \u{1f310}"),
            link_type: json!("fibre \u{1f310}"),
            provider: json!("Ünternet"),
            bandwidth: json!({ "UploadSpeed": i32::MAX, "DownloadSpeed": i32::MAX }),
        },
    ]
}

/// Create a link seeded with the base members above, and return its minted id.
async fn create_seeded_link(emulator: &Emulator, network: &str, site: &str) -> String {
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/links"),
            json!({
                "SiteId": site,
                "Description": BASE_DESCRIPTION,
                "Type": BASE_TYPE,
                "Provider": BASE_PROVIDER,
                "Bandwidth": base_bandwidth(),
                "Tags": base_tags(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateLink: expected 200, got {status} with {created}"
    );
    created["Link"]["LinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateLink: no LinkId in {created}"))
        .to_string()
}

/// Read one link back through `GetLinks`, the implemented reader for this resource.
async fn read_link(emulator: &Emulator, network: &str, link_id: &str) -> Value {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/links?linkIds={link_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetLinks({link_id}): expected 200, got {status} with {body}"
    );
    let links = body["Links"]
        .as_array()
        .unwrap_or_else(|| panic!("GetLinks({link_id}): no Links array in {body}"));
    assert_eq!(
        links.len(),
        1,
        "GetLinks({link_id}): expected exactly that link, got {body}"
    );
    links[0].clone()
}

/// `UpdateLink` on one link of `network`.
async fn update_link(emulator: &Emulator, network: &str, link_id: &str, body: Value) -> (u16, Value) {
    emulator
        .patch(&format!("/global-networks/{network}/links/{link_id}"), body)
        .await
}

/// A `Link` without its `State`, so two readings of one record can be compared member
/// for member without racing the transition the clock owes.
fn without_state(link: &Value) -> Value {
    let mut copy = link.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("State");
    }
    copy
}

#[tokio::test]
async fn update_link_writes_only_the_supplied_members_and_round_trips_through_get_links() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-link").await;
    let site = emulator.create_site(&network, "hq").await;
    let rows = update_cases();

    // One identically seeded link per row, plus a sibling no row names.
    let mut links = Vec::new();
    for _ in &rows {
        links.push(create_seeded_link(&emulator, &network, &site).await);
    }
    let sibling = create_seeded_link(&emulator, &network, &site).await;

    // The PENDING -> AVAILABLE transition the create owes is past before the first
    // update, so an update that restarted that clock would be visible here.
    std::thread::sleep(PAST_SETTLE);

    for (row, link_id) in rows.iter().zip(&links) {
        let case = row.case;
        let before = read_link(&emulator, &network, link_id).await;
        let created_at = before["CreatedAt"].clone();

        let (status, answer) = update_link(&emulator, &network, link_id, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {answer}"
        );
        let link = &answer["Link"];

        for (member, want) in [
            ("Description", &row.description),
            ("Type", &row.link_type),
            ("Provider", &row.provider),
            ("Bandwidth", &row.bandwidth),
        ] {
            assert_eq!(
                &link[member], want,
                "case {case}: expected {member} {want}, got {link}"
            );
        }

        // Everything the update does not take as input is the record it already was.
        assert_eq!(
            link["LinkId"],
            json!(link_id),
            "case {case}: LinkId must be the addressed link, got {link}"
        );
        assert_eq!(
            link["LinkArn"],
            json!(expected_scoped_arn("link", &network, link_id)),
            "case {case}: the ARN must not be rewritten, got {link}"
        );
        assert_eq!(
            link["SiteId"],
            json!(site.clone()),
            "case {case}: the site back-reference must not be rewritten, got {link}"
        );
        assert_eq!(
            link["GlobalNetworkId"],
            json!(network.clone()),
            "case {case}: the global network back-reference must not be rewritten, got {link}"
        );
        assert_eq!(
            link["CreatedAt"], created_at,
            "case {case}: CreatedAt must stay the create's instant, got {link}"
        );
        assert_eq!(
            link["Tags"],
            base_tags(),
            "case {case}: the tags must not be rewritten, got {link}"
        );
        assert_eq!(
            link["State"],
            json!("UPDATING"),
            "case {case}: an update answers the link in its transient state, got {link}"
        );

        // The answer is the stored record: GetLinks reports the same link, member for
        // member. `State` is compared on its own, since the transition the update owes
        // can fall due between the two calls.
        let stored = read_link(&emulator, &network, link_id).await;
        assert_eq!(
            without_state(&stored),
            without_state(link),
            "case {case}: GetLinks must answer the updated record, got {stored}"
        );

        // Once that transition is due, the link is settled again — and the members the
        // update wrote are still what it wrote.
        std::thread::sleep(PAST_SETTLE);
        let settled = read_link(&emulator, &network, link_id).await;
        assert_eq!(
            settled["State"],
            json!("AVAILABLE"),
            "case {case}: the link must settle once the transition is due, got {settled}"
        );
        assert_eq!(
            settled["Description"], row.description,
            "case {case}: the written Description must survive the transition, got {settled}"
        );
        assert_eq!(
            settled["Bandwidth"], row.bandwidth,
            "case {case}: the written Bandwidth must survive the transition, got {settled}"
        );

        // The sibling nobody addressed still holds what it was created with.
        let untouched = read_link(&emulator, &network, &sibling).await;
        assert_eq!(
            untouched["Description"],
            json!(BASE_DESCRIPTION),
            "case {case}: an update must not touch another link, got {untouched}"
        );
        assert_eq!(
            untouched["Bandwidth"],
            base_bandwidth(),
            "case {case}: an update must not touch another link, got {untouched}"
        );
    }
}

#[tokio::test]
async fn an_update_while_the_create_is_still_pending_answers_the_update_it_is_working_on() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-link").await;
    let site = emulator.create_site(&network, "hq").await;
    // No wait: the create's PENDING -> AVAILABLE transition is still outstanding here.
    let link = create_seeded_link(&emulator, &network, &site).await;
    let fresh = read_link(&emulator, &network, &link).await;
    assert_eq!(
        fresh["State"],
        json!("PENDING"),
        "a freshly created link must still be settling, got {fresh}"
    );

    let (status, answer) = update_link(
        &emulator,
        &network,
        &link,
        json!({ "Description": "updated while pending" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update of a settling link: expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["Link"]["State"],
        json!("UPDATING"),
        "the later of the two outstanding requests is what the link reports, got {answer}"
    );

    // Once both transitions are due, the link is settled and holds what the update wrote.
    std::thread::sleep(PAST_SETTLE);
    let settled = read_link(&emulator, &network, &link).await;
    assert_eq!(
        settled["State"],
        json!("AVAILABLE"),
        "the link must settle once both transitions are due, got {settled}"
    );
    assert_eq!(
        settled["Description"],
        json!("updated while pending"),
        "the update must still be what the link holds, got {settled}"
    );
}

#[tokio::test]
async fn update_link_on_a_link_the_store_does_not_hold_is_not_found_and_stores_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-link").await;
    let site = emulator.create_site(&network, "hq").await;
    let link = create_seeded_link(&emulator, &network, &site).await;
    // A second global network, so a link addressed under the wrong parent is a link that
    // exists — just not there.
    let other_network = emulator.create_global_network("sibling").await;

    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_link = "link-00000000000000099".to_string();

    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a global network that was never minted",
            unknown_network.clone(),
            link.clone(),
            "global-network",
            unknown_network.clone(),
        ),
        (
            "a link that was never minted",
            network.clone(),
            unknown_link.clone(),
            "link",
            unknown_link.clone(),
        ),
        (
            "an existing link under another global network",
            other_network.clone(),
            link.clone(),
            "link",
            link.clone(),
        ),
    ];

    for (case, path_network, path_link, resource_type, resource_id) in &cases {
        let (status, body) = update_link(
            &emulator,
            path_network,
            path_link,
            json!({ "Description": "must not be stored" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(*resource_type),
            "case {case}: expected ResourceType {resource_type:?}, got {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(resource_id.as_str()),
            "case {case}: expected ResourceId {resource_id:?}, got {body}"
        );
    }

    // None of those refusals wrote anything: the live link is what it was created with.
    let stored = read_link(&emulator, &network, &link).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "a refused update must store nothing, got {stored}"
    );
}

/// One row of the reader-visibility table: the update's body, and the descriptors the
/// stored record must be found under afterwards.
struct VisibilityCase {
    case: &'static str,
    body: Value,
    link_type: String,
    provider: String,
}

/// The descriptors a link of this suite's visibility table is seeded with. They carry no
/// space, so a row can name them in the query string of a filtered `GetLinks` as they are.
const FILTERABLE_TYPE: &str = "broadband";
/// See [`FILTERABLE_TYPE`].
const FILTERABLE_PROVIDER: &str = "AnyCompanyNetworks";

/// The rows of the reader-visibility table. They differ along WHICH descriptor the update
/// rewrites (both, `Type` alone, `Provider` alone), and along the form of the new value: a
/// plain word, one differing from the stored value only in case, and one at the published
/// maximum length.
fn visibility_cases() -> Vec<VisibilityCase> {
    vec![
        VisibilityCase {
            case: "both descriptors rewritten",
            body: json!({ "Type": "mpls", "Provider": "ExampleTelecom" }),
            link_type: "mpls".to_string(),
            provider: "ExampleTelecom".to_string(),
        },
        VisibilityCase {
            case: "Type alone rewritten",
            body: json!({ "Type": "dark-fibre" }),
            link_type: "dark-fibre".to_string(),
            provider: FILTERABLE_PROVIDER.to_string(),
        },
        VisibilityCase {
            case: "Provider alone rewritten",
            body: json!({ "Provider": "Example_Telecom-2" }),
            link_type: FILTERABLE_TYPE.to_string(),
            provider: "Example_Telecom-2".to_string(),
        },
        VisibilityCase {
            case: "a Type differing from the stored one only in case",
            body: json!({ "Type": "BROADBAND" }),
            link_type: "BROADBAND".to_string(),
            provider: FILTERABLE_PROVIDER.to_string(),
        },
        VisibilityCase {
            case: "descriptors at their published maximum lengths",
            body: json!({
                "Type": "t".repeat(MAX_DESCRIPTOR_CHARS),
                "Provider": "p".repeat(MAX_DESCRIPTOR_CHARS),
            }),
            link_type: "t".repeat(MAX_DESCRIPTOR_CHARS),
            provider: "p".repeat(MAX_DESCRIPTOR_CHARS),
        },
    ]
}

/// Create a link carrying the given descriptors, and return its minted id.
async fn create_link_with(
    emulator: &Emulator,
    network: &str,
    site: &str,
    link_type: &str,
    provider: &str,
) -> String {
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/links"),
            json!({
                "SiteId": site,
                "Description": BASE_DESCRIPTION,
                "Type": link_type,
                "Provider": provider,
                "Bandwidth": base_bandwidth(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateLink({link_type}/{provider}): expected 200, got {status} with {created}"
    );
    created["Link"]["LinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateLink: no LinkId in {created}"))
        .to_string()
}

/// The ids a `GetLinks` page carries for one query string.
async fn listed_links(emulator: &Emulator, network: &str, query: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/links?{query}"))
        .await;
    assert_eq!(
        status, 200,
        "GetLinks?{query}: expected 200, got {status} with {body}"
    );
    ids_of(&body, "Links", "LinkId")
}

#[tokio::test]
async fn an_updated_link_is_what_every_reader_of_the_record_answers() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-link").await;
    let site = emulator.create_site(&network, "hq").await;
    let rows = visibility_cases();

    // One identically seeded link per row, so a row that rewrites one descriptor leaves a
    // record that is still found under the other.
    let mut links = Vec::new();
    for _ in &rows {
        links.push(
            create_link_with(
                &emulator,
                &network,
                &site,
                FILTERABLE_TYPE,
                FILTERABLE_PROVIDER,
            )
            .await,
        );
    }

    for (row, link_id) in rows.iter().zip(&links) {
        let case = row.case;
        let (status, answer) = update_link(&emulator, &network, link_id, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {answer}"
        );

        // The update wrote the store, so the descriptor filters answer from the NEW record:
        // the link is listed under what it now holds (the two filters the documentation
        // lets a request combine), and no longer under the value it held before.
        let under_new = listed_links(
            &emulator,
            &network,
            &format!("type={}&provider={}", row.link_type, row.provider),
        )
        .await;
        assert!(
            under_new.contains(link_id),
            "case {case}: expected the link under its new descriptors, got {under_new:?}"
        );
        if row.link_type != FILTERABLE_TYPE {
            let under_old = listed_links(&emulator, &network, &format!("type={FILTERABLE_TYPE}"))
                .await;
            assert!(
                !under_old.contains(link_id),
                "case {case}: the link must leave the listing of the Type it no longer holds, \
                 got {under_old:?}"
            );
        }
        if row.provider != FILTERABLE_PROVIDER {
            let under_old =
                listed_links(&emulator, &network, &format!("provider={FILTERABLE_PROVIDER}")).await;
            assert!(
                !under_old.contains(link_id),
                "case {case}: the link must leave the listing of the Provider it no longer \
                 holds, got {under_old:?}"
            );
        }

        // What the update takes no input for is untouched, so the record keeps the site it
        // was created at and stays in the global network's unfiltered listing.
        let at_site = listed_links(&emulator, &network, &format!("siteId={site}")).await;
        assert!(
            at_site.contains(link_id),
            "case {case}: an update must not move the link off its site, got {at_site:?}"
        );
        let everything = listed_links(&emulator, &network, "").await;
        assert!(
            everything.contains(link_id),
            "case {case}: the link must stay in its global network, got {everything:?}"
        );
    }
}

#[tokio::test]
async fn repeating_one_update_leaves_the_same_record_and_a_later_one_wins_per_member() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-link").await;
    let site = emulator.create_site(&network, "hq").await;
    let link = create_seeded_link(&emulator, &network, &site).await;

    // Applying one update twice is the same record both times: the write depends on the
    // request alone, not on how often it has been made.
    let body = json!({ "Description": "settled", "Bandwidth": { "UploadSpeed": 42 } });
    let (first_status, first) = update_link(&emulator, &network, &link, body.clone()).await;
    let (second_status, second) = update_link(&emulator, &network, &link, body.clone()).await;
    for (case, status) in [("first apply", first_status), ("second apply", second_status)] {
        assert_eq!(status, 200, "case {case}: expected 200, got {status}");
    }
    assert_eq!(
        without_state(&first["Link"]),
        without_state(&second["Link"]),
        "repeating one update must answer the same record, got {second}"
    );

    // A later update replaces only the members it carries, so the earlier write survives
    // on every member the later request leaves out.
    let (status, answer) = update_link(
        &emulator,
        &network,
        &link,
        json!({ "Provider": "Example Telecom" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the later update: expected 200, got {status} with {answer}"
    );
    std::thread::sleep(PAST_SETTLE);
    let stored = read_link(&emulator, &network, &link).await;
    for (member, want) in [
        ("Description", json!("settled")),
        ("Provider", json!("Example Telecom")),
        ("Type", json!(BASE_TYPE)),
        ("Bandwidth", json!({ "UploadSpeed": 42 })),
    ] {
        assert_eq!(
            stored[member], want,
            "after two updates: expected {member} {want}, got {stored}"
        );
    }
}

#[tokio::test]
async fn update_link_refuses_a_descriptor_outside_its_documented_constraints() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-link").await;
    let site = emulator.create_site(&network, "hq").await;
    let link = create_seeded_link(&emulator, &network, &site).await;

    // The rows walk the published bound of each descriptor (at it, and one past it) and
    // every character the documentation forbids, on both members that carry the rule.
    let mut cases: Vec<(String, &'static str, String, u16)> = vec![
        (
            "Type at the documented maximum".to_string(),
            "Type",
            "t".repeat(MAX_DESCRIPTOR_CHARS),
            200,
        ),
        (
            "Type one past the documented maximum".to_string(),
            "Type",
            "t".repeat(MAX_DESCRIPTOR_CHARS + 1),
            400,
        ),
        (
            "Provider at the documented maximum".to_string(),
            "Provider",
            "p".repeat(MAX_DESCRIPTOR_CHARS),
            200,
        ),
        (
            "Provider one past the documented maximum".to_string(),
            "Provider",
            "p".repeat(MAX_DESCRIPTOR_CHARS + 1),
            400,
        ),
    ];
    for forbidden in FORBIDDEN_DESCRIPTOR_CHARS {
        for member in ["Type", "Provider"] {
            cases.push((
                format!("{member} carrying the forbidden character {forbidden:?}"),
                member,
                format!("fibre{forbidden}link"),
                400,
            ));
        }
    }

    for (case, member, value, want) in &cases {
        let (status, body) =
            update_link(&emulator, &network, &link, json!({ *member: value })).await;
        assert_eq!(
            status, *want,
            "case {case}: expected {want}, got {status} with {body}"
        );
        let stored = read_link(&emulator, &network, &link).await;
        if *want == 200 {
            assert_eq!(
                stored[*member],
                json!(value),
                "case {case}: an accepted value must be stored, got {stored}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: wrong error code in {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"].as_str(),
                Some(*member),
                "case {case}: the refusal must name the member at fault, got {body}"
            );
            assert_ne!(
                stored[*member],
                json!(value),
                "case {case}: a refused value must not be stored, got {stored}"
            );
        }
    }
}
