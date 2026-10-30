// SPDX-License-Identifier: CC0-1.0

//! The mutual exclusion `GetLinks` documents, decided on what the request SPECIFIES.
//!
//! "If you specify the site ID, you cannot specify the type or provider in the same
//! request. You can specify the type and provider in the same request."
//! (`smithy_query query="doc" shape="GetLinks"`) — a rule the model cannot express as a
//! constraint, so the handler refuses it. What the rule turns on is what each member
//! specifies, and `SiteId` is an OPTIONAL REFERENCE member modelled len 0..50 over
//! `^[\s\S]*$` (`smithy_query query="operation" operation="GetLinks"`), so the empty
//! string reaches the handler and, under the one reading this crate gives such a member
//! (`service_com_amazonaws_networkmanager_networkmanager::state::optional_reference`, the
//! rule the sibling `crate::empty_optional_reference` pins on the writers), names NO site: a request spelling it empty
//! specifies no site, so it conflicts with nothing and is answered exactly as the request
//! omitting the member is. `Type` and `Provider` are DESCRIPTIVE members whose writer
//! stores what the request spells verbatim, the empty string included, so each is
//! specified whenever the request carries it — the same reading their filters give.
//!
//! The table spans what the rule generalises over: the four spellings of the reference
//! member (omitted, empty, naming a stored site, naming a site nothing is stored under)
//! against every combination of the two descriptive members (absent, carrying a value, and
//! carrying the empty string), with the served rows asserted against the links the filters
//! select and the refused rows against the fields the refusal names.
//!
//! Nothing here reaches into `crate::state`: every link is created and read back over HTTP.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::link_lifecycle::create_link_ok;

/// An id of the shape this service mints for a site that nothing is stored under, so a
/// filter naming it still SPECIFIES a site (it is not the empty string) while selecting no
/// link.
const UNSTORED_SITE: &str = "site-00000000000000099";

/// What the answer to one request must be.
enum Outcome {
    /// A 200 carrying exactly these link ids.
    Served(Vec<String>),
    /// A 400 `ValidationException` naming exactly these members, in this order.
    Refused(Vec<&'static str>),
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The query string the request carries, with no leading `?`.
    query: String,
    outcome: Outcome,
}

/// `ids` sorted, so a row's expectation does not depend on the order the page reports.
fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// The links a global network holds for this table: two sites, two links each, spanning the
/// descriptive values the rows filter on — including a link whose `Type` and `Provider` are
/// the empty string, which `CreateLink` stores verbatim.
struct Fixture {
    network: String,
    hq: String,
    branch: String,
    /// At `hq`, type `broadband`, provider `P1`.
    broadband_at_hq: String,
    /// At `hq`, type `mpls`, no provider.
    mpls_at_hq: String,
    /// At `branch`, type `broadband`, provider `P2`.
    broadband_at_branch: String,
    /// At `branch`, both descriptors spelled empty.
    blank_at_branch: String,
}

async fn seed(emulator: &Emulator) -> Fixture {
    let network = emulator.create_global_network("get links exclusion").await;
    let hq = emulator.create_site(&network, "hq").await;
    let branch = emulator.create_site(&network, "branch").await;

    let bandwidth = json!({ "UploadSpeed": 10, "DownloadSpeed": 20 });
    let broadband_at_hq = create_link_ok(
        emulator,
        &network,
        &hq,
        &json!({ "Bandwidth": bandwidth.clone(), "Type": "broadband", "Provider": "P1" }),
    )
    .await;
    let mpls_at_hq = create_link_ok(
        emulator,
        &network,
        &hq,
        &json!({ "Bandwidth": bandwidth.clone(), "Type": "mpls" }),
    )
    .await;
    let broadband_at_branch = create_link_ok(
        emulator,
        &network,
        &branch,
        &json!({ "Bandwidth": bandwidth.clone(), "Type": "broadband", "Provider": "P2" }),
    )
    .await;
    let blank_at_branch = create_link_ok(
        emulator,
        &network,
        &branch,
        &json!({ "Bandwidth": bandwidth, "Type": "", "Provider": "" }),
    )
    .await;

    Fixture {
        network,
        hq,
        branch,
        broadband_at_hq,
        mpls_at_hq,
        broadband_at_branch,
        blank_at_branch,
    }
}

/// The rows the fixture feeds.
fn rows(fixture: &Fixture) -> Vec<Row> {
    let Fixture {
        hq,
        branch,
        broadband_at_hq,
        mpls_at_hq,
        broadband_at_branch,
        blank_at_branch,
        ..
    } = fixture;
    let every = vec![
        broadband_at_hq.clone(),
        mpls_at_hq.clone(),
        broadband_at_branch.clone(),
        blank_at_branch.clone(),
    ];

    vec![
        // The reference member OMITTED: the baseline every empty spelling must match.
        Row {
            case: "no filter at all",
            query: String::new(),
            outcome: Outcome::Served(every.clone()),
        },
        Row {
            case: "a type alone",
            query: "type=broadband".to_string(),
            outcome: Outcome::Served(vec![
                broadband_at_hq.clone(),
                broadband_at_branch.clone(),
            ]),
        },
        Row {
            case: "a provider alone",
            query: "provider=P1".to_string(),
            outcome: Outcome::Served(vec![broadband_at_hq.clone()]),
        },
        Row {
            case: "a type and a provider",
            query: "type=broadband&provider=P1".to_string(),
            outcome: Outcome::Served(vec![broadband_at_hq.clone()]),
        },
        Row {
            case: "an empty type alone",
            query: "type=".to_string(),
            outcome: Outcome::Served(vec![blank_at_branch.clone()]),
        },
        Row {
            case: "an empty provider alone",
            query: "provider=".to_string(),
            outcome: Outcome::Served(vec![blank_at_branch.clone()]),
        },
        // The reference member spelled EMPTY: it specifies no site, so each row answers
        // exactly what the row above it with the member omitted answers.
        Row {
            case: "an empty siteId alone",
            query: "siteId=".to_string(),
            outcome: Outcome::Served(every.clone()),
        },
        Row {
            case: "an empty siteId beside a type",
            query: "siteId=&type=broadband".to_string(),
            outcome: Outcome::Served(vec![
                broadband_at_hq.clone(),
                broadband_at_branch.clone(),
            ]),
        },
        Row {
            case: "an empty siteId beside a provider",
            query: "siteId=&provider=P1".to_string(),
            outcome: Outcome::Served(vec![broadband_at_hq.clone()]),
        },
        Row {
            case: "an empty siteId beside a type and a provider",
            query: "siteId=&type=broadband&provider=P1".to_string(),
            outcome: Outcome::Served(vec![broadband_at_hq.clone()]),
        },
        Row {
            case: "an empty siteId beside an empty type",
            query: "siteId=&type=".to_string(),
            outcome: Outcome::Served(vec![blank_at_branch.clone()]),
        },
        Row {
            case: "an empty siteId beside an empty type and an empty provider",
            query: "siteId=&type=&provider=".to_string(),
            outcome: Outcome::Served(vec![blank_at_branch.clone()]),
        },
        Row {
            case: "an empty siteId beside a type no link carries",
            query: "siteId=&type=router".to_string(),
            outcome: Outcome::Served(Vec::new()),
        },
        // The reference member NAMING a site: it specifies one, so it narrows the listing
        // on its own and conflicts with either descriptive member.
        Row {
            case: "a stored siteId alone",
            query: format!("siteId={hq}"),
            outcome: Outcome::Served(vec![broadband_at_hq.clone(), mpls_at_hq.clone()]),
        },
        Row {
            case: "the other stored siteId alone",
            query: format!("siteId={branch}"),
            outcome: Outcome::Served(vec![
                broadband_at_branch.clone(),
                blank_at_branch.clone(),
            ]),
        },
        Row {
            case: "a siteId nothing is stored under alone",
            query: format!("siteId={UNSTORED_SITE}"),
            outcome: Outcome::Served(Vec::new()),
        },
        Row {
            case: "a stored siteId beside a type",
            query: format!("siteId={hq}&type=broadband"),
            outcome: Outcome::Refused(vec!["Type"]),
        },
        Row {
            case: "a stored siteId beside a provider",
            query: format!("siteId={hq}&provider=P1"),
            outcome: Outcome::Refused(vec!["Provider"]),
        },
        Row {
            case: "a stored siteId beside a type and a provider",
            query: format!("siteId={hq}&type=broadband&provider=P1"),
            outcome: Outcome::Refused(vec!["Type", "Provider"]),
        },
        Row {
            case: "a siteId nothing is stored under beside a type",
            query: format!("siteId={UNSTORED_SITE}&type=broadband"),
            outcome: Outcome::Refused(vec!["Type"]),
        },
        // A descriptive member is specified whenever the request carries it, the empty
        // string included: its writer stores that spelling verbatim.
        Row {
            case: "a stored siteId beside an empty type",
            query: format!("siteId={hq}&type="),
            outcome: Outcome::Refused(vec!["Type"]),
        },
        Row {
            case: "a stored siteId beside an empty provider",
            query: format!("siteId={hq}&provider="),
            outcome: Outcome::Refused(vec!["Provider"]),
        },
    ]
}

/// The field names a refusal reports, in the order it reports them.
fn field_names(body: &Value) -> Vec<String> {
    body["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Fields array in {body}"))
        .iter()
        .map(|field| {
            field["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a Name in {field}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn get_links_decides_the_site_exclusion_on_the_site_the_request_names() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    let network = &fixture.network;

    for row in rows(&fixture) {
        let case = row.case;
        let uri = if row.query.is_empty() {
            format!("/global-networks/{network}/links")
        } else {
            format!("/global-networks/{network}/links?{}", row.query)
        };
        let (status, body) = emulator.get(&uri).await;
        match row.outcome {
            Outcome::Served(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: GET {uri} expected 200, got {status} with {body}"
                );
                let reported = sorted(ids_of(&body, "Links", "LinkId"));
                let expected = sorted(expected);
                assert_eq!(
                    reported, expected,
                    "case {case:?}: GET {uri} expected the links {expected:?}, got {reported:?}"
                );
            }
            Outcome::Refused(members) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: GET {uri} expected 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: GET {uri} expected a ValidationException, got {body}"
                );
                let expected_message = format!(
                    "SiteId cannot be combined with {} in the same request",
                    members.join(" or ")
                );
                assert_eq!(
                    body["Message"].as_str(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the message {expected_message:?}, got {body}"
                );
                let reported = field_names(&body);
                assert_eq!(
                    reported, members,
                    "case {case:?}: expected the refusal to name {members:?}, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_empty_site_id_leaves_the_unknown_global_network_as_the_answer() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;

    // The exclusion is the request's own members, so it is decided before any stored
    // resource is looked up. With the reference member spelled empty there is no conflict
    // left to refuse, which leaves the resource the PATH addresses as the answer: a global
    // network nothing is stored under is not found, exactly as it is for the request that
    // omits the member.
    for (case, query) in [
        ("siteId omitted", "type=broadband"),
        ("an empty siteId beside a type", "siteId=&type=broadband"),
        (
            "an empty siteId beside a type and a provider",
            "siteId=&type=broadband&provider=P1",
        ),
    ] {
        let (status, body) = emulator
            .get(&format!("/global-networks/global-network-00000000000000099/links?{query}"))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
    }
    // The fixture's own network still answers, so the 404 above is about the path and not
    // about the query string.
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{}/links?siteId=&type=broadband",
            fixture.network
        ))
        .await;
    assert_eq!(
        status, 200,
        "case \"the stored global network\": expected 200, got {status} with {body}"
    );
}
