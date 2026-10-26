// SPDX-License-Identifier: CC0-1.0

//! The published maximum on `GetLinks$LinkIds`, judged on the REQUEST and nothing else,
//! driven over HTTP.
//!
//! The rule under test: a list-valued filter member whose documentation publishes a
//! maximum number of values is refused — with the modelled `ValidationException`
//! (HTTP 400) naming that member, and no page at all — as soon as the request carries more
//! than that many values, whatever else the request selects on and whether or not the
//! values name anything stored. `LinkIds` is "One or more link IDs. The maximum is 10."
//! (`smithy_query query="doc" shape="GetLinksRequest" member="LinkIds"`) and the modelled
//! `LinkIdList` expresses the bound nowhere — it carries a length on its member STRING
//! only (`smithy_query shape=LinkIdList`) — so the refusal is the handler's to answer.
//!
//! `crate::filter_id_list_maximum` spans the five id-list filters by COUNT alone, and
//! `crate::site_ids_filter_maximum` adds the page size, the page token and values that
//! name nothing — both over the shared table in `crate::filter_maximum_table`. This suite
//! covers what neither does: the two listings whose query string
//! also carries VALUE selectors beside the id filter — `GetLinks` (`SiteId`, `Type`,
//! `Provider`) and `GetDevices` (`SiteId`) — because the bound belongs to the member, not
//! to what the rest of the request selects: no selector lifts it, and none is answered
//! ahead of it.
//!
//! The table is the shared one `crate::filter_maximum_table` defines (the filter
//! absent, inside the bound, at it, past it, and far past it with repeated values; each
//! against the fixture's own ids and against ids that name nothing; each with no other
//! parameter, the smallest page size, the largest the model allows, and a page token this
//! service never issued), crossed here with the selectors of the listing under test.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! query bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::json;

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::link_lifecycle::create_link_ok;
use crate::resource_tagging::escape;
use crate::filter_maximum_table::{
    assert_filter_refusal, unknown_ids, Extra, EXTRAS, FIXTURE_COUNT, KINDS, MAX_FILTER_VALUES,
    ROWS,
};

/// The link descriptors the fixture spreads over its links, so a `Type` or a `Provider`
/// selector matches SOME of them: both are free-form strings of at most 128 characters
/// (`smithy_query query="doc" shape="CreateLinkRequest" member="Provider"`), and the
/// provider carries a space to exercise the query binding's escaping.
const TYPES: [&str; 2] = ["broadband", "mpls"];
/// See [`TYPES`].
const PROVIDERS: [&str; 2] = ["AnyCompany Networks", "Example Telecom"];

/// One link of the fixture: its minted id and the selector values it was created with.
struct LinkFixture {
    id: String,
    /// Which of the fixture's two sites the link hangs off, by index.
    site: usize,
    link_type: &'static str,
    provider: &'static str,
}

/// One combination of the VALUE selectors `GetLinks` takes beside `LinkIds`.
///
/// `SiteId` is never combined with `Type` or `Provider`: "If you specify the site ID, you
/// cannot specify the type or provider in the same request. You can specify the type and
/// provider in the same request." (`smithy_query query="doc" shape="GetLinks"`), which the
/// handler refuses on its own and `link_lifecycle` already covers.
struct Selector {
    name: &'static str,
    /// Which fixture site to select on, by index.
    site: Option<usize>,
    link_type: Option<&'static str>,
    provider: Option<&'static str>,
}

/// The selector combinations: none at all, each selector alone, the two the documentation
/// allows together, and one that matches no stored link.
const SELECTORS: [Selector; 6] = [
    Selector {
        name: "no selector",
        site: None,
        link_type: None,
        provider: None,
    },
    Selector {
        name: "the first site",
        site: Some(0),
        link_type: None,
        provider: None,
    },
    Selector {
        name: "the second site",
        site: Some(1),
        link_type: None,
        provider: None,
    },
    Selector {
        name: "one type",
        site: None,
        link_type: Some(TYPES[0]),
        provider: None,
    },
    Selector {
        name: "one type and one provider",
        site: None,
        link_type: Some(TYPES[1]),
        provider: Some(PROVIDERS[1]),
    },
    Selector {
        name: "a type no link carries",
        site: None,
        link_type: Some("dark-fiber"),
        provider: None,
    },
];

impl Selector {
    /// The query parameters this selector adds, bound the way a client binds `@httpQuery`.
    fn query_parts(&self, sites: &[String]) -> Vec<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(index) = self.site {
            parts.push(format!("siteId={}", escape(&sites[index])));
        }
        if let Some(link_type) = self.link_type {
            parts.push(format!("type={}", escape(link_type)));
        }
        if let Some(provider) = self.provider {
            parts.push(format!("provider={}", escape(provider)));
        }
        parts
    }

    /// Whether `link` is one this selector keeps: an absent selector matches every link.
    fn matches(&self, link: &LinkFixture) -> bool {
        self.site.is_none_or(|index| link.site == index)
            && self
                .link_type
                .is_none_or(|link_type| link.link_type == link_type)
            && self
                .provider
                .is_none_or(|provider| link.provider == provider)
    }
}

/// The `GetLinks` URI: one `linkIds` parameter per filter value, then the selector's
/// parameters, then the row's other parameter.
fn links_uri(
    network: &str,
    values: &[String],
    selector: &Selector,
    sites: &[String],
    extra: &Extra,
) -> String {
    let mut query: Vec<String> = values
        .iter()
        .map(|id| format!("linkIds={}", escape(id)))
        .collect();
    query.extend(selector.query_parts(sites));
    if !extra.query.is_empty() {
        query.push(extra.query.to_string());
    }
    let base = format!("/global-networks/{network}/links");
    if query.is_empty() {
        base
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

#[tokio::test]
async fn the_link_ids_filter_maximum_is_judged_on_the_request_alone() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("link filter bound").await;
    let sites = vec![
        emulator.create_site(&network, "first site").await,
        emulator.create_site(&network, "second site").await,
    ];

    // The selector values are spread over the links on three different cycles, so no
    // selector is a stand-in for another and a filtered page is never the whole fixture.
    let mut links: Vec<LinkFixture> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        let site = index % sites.len();
        let link_type = TYPES[(index / 2) % TYPES.len()];
        let provider = PROVIDERS[index % PROVIDERS.len()];
        let id = create_link_ok(
            &emulator,
            &network,
            &sites[site],
            &json!({
                "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 },
                "Type": link_type,
                "Provider": provider,
            }),
        )
        .await;
        links.push(LinkFixture {
            id,
            site,
            link_type,
            provider,
        });
    }
    let stored: Vec<String> = links.iter().map(|link| link.id.clone()).collect();
    let unknown = unknown_ids("link");

    for (position, selector) in SELECTORS.iter().enumerate() {
        let selected: Vec<&LinkFixture> =
            links.iter().filter(|link| selector.matches(link)).collect();
        for kind in &KINDS {
            let available = if kind.unknown { &unknown } else { &stored };
            for row in &ROWS {
                for extra in &EXTRAS {
                    // The forged page token is about the ORDER of two refusals rather than
                    // about the selectors, so one selector carries that crossing instead of
                    // every one of them repeating it.
                    if extra.forged_token && position > 0 {
                        continue;
                    }
                    let case = format!(
                        "{}/{}/{}/{}",
                        selector.name, kind.name, row.name, extra.name
                    );
                    let values = row.values(available);
                    let uri = links_uri(&network, &values, selector, &sites, extra);
                    let (status, body) = emulator.get(&uri).await;

                    if values.len() > MAX_FILTER_VALUES {
                        // Past the published maximum the length is the answer: no selector
                        // lifts it, and nothing the listing would have resolved — the
                        // selected links, the page token — is answered ahead of it.
                        assert_filter_refusal(&case, status, &body, "LinkIds", "Links");
                        continue;
                    }

                    if extra.forged_token {
                        // Inside the bound the token is reached, and the refusal names it
                        // — which is what shows the refusal above is the LENGTH.
                        assert_eq!(
                            status, 400,
                            "case {case}: expected 400, got {status} with {body}"
                        );
                        assert_eq!(
                            error_code(&body),
                            "ValidationException",
                            "case {case}: expected ValidationException, got {body}"
                        );
                        assert_eq!(
                            body["Fields"][0]["Name"],
                            json!("NextToken"),
                            "case {case}: the refusal must name NextToken, got {body}"
                        );
                        continue;
                    }

                    // At or inside the bound the listing is computed: the selectors narrow
                    // the global network's links, the id filter narrows them further (to
                    // nothing, for ids that name nothing), and a page size bounds the page.
                    assert_eq!(
                        status, 200,
                        "case {case}: expected 200, got {status} with {body}"
                    );
                    let mut expected: Vec<String> = if values.is_empty() {
                        selected.iter().map(|link| link.id.clone()).collect()
                    } else if kind.unknown {
                        Vec::new()
                    } else {
                        selected
                            .iter()
                            .filter(|link| values.contains(&link.id))
                            .map(|link| link.id.clone())
                            .collect()
                    };
                    let full = expected.len();
                    if let Some(limit) = extra.limit {
                        expected.truncate(limit);
                    }
                    assert_eq!(
                        ids_of(&body, "Links", "LinkId"),
                        expected,
                        "case {case}: expected exactly {expected:?}, got {body}"
                    );
                    assert_eq!(
                        body["NextToken"].is_string(),
                        expected.len() < full,
                        "case {case}: a token must be carried only when links remain, got {body}"
                    );
                }
            }
        }
    }

    // A request past the bound that ALSO breaks the documented SiteId/Type exclusion is
    // still refused, and still carries no page: neither doc-only rule can be escaped by
    // breaking the other. The two are answered in the order the handler states them — the
    // exclusion, which is a rule about the selectors, ahead of the filter's length — so the
    // refusal names `Type`, the member the exclusion is broken on.
    let values: Vec<String> = stored[..=MAX_FILTER_VALUES].to_vec();
    let query: Vec<String> = values
        .iter()
        .map(|id| format!("linkIds={}", escape(id)))
        .collect();
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/links?{}&siteId={}&type={}",
            query.join("&"),
            escape(&sites[0]),
            escape(TYPES[0])
        ))
        .await;
    let case = "past the maximum with SiteId and Type together";
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!("Type"),
        "case {case}: the refusal must name Type, got {body}"
    );
    assert!(
        body["Message"]
            .as_str()
            .is_some_and(|text| text.contains("SiteId")),
        "case {case}: the message must name SiteId, got {body}"
    );
    assert!(
        body["Links"].is_null(),
        "case {case}: a refused request must carry no page, got {body}"
    );

    // Nothing any refused request carried changed what a later caller sees.
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/links"))
        .await;
    assert_eq!(
        status, 200,
        "case unfiltered read after the refusals: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_of(&body, "Links", "LinkId"),
        stored,
        "case unfiltered read after the refusals: expected every created link, got {body}"
    );
}

#[tokio::test]
async fn the_device_ids_filter_maximum_is_judged_past_its_own_site_selector() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("device filter bound").await;
    let sites = [
        emulator.create_site(&network, "first site").await,
        emulator.create_site(&network, "second site").await,
    ];

    // `DeviceIds` publishes the same maximum ("One or more device IDs. The maximum is
    // 10.", `smithy_query query="doc" shape="GetDevicesRequest" member="DeviceIds"`) and
    // `GetDevices` takes one value selector, `SiteId`.
    let mut devices: Vec<(String, usize)> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        let site = index % sites.len();
        let id = create_device_ok(
            &emulator,
            &network,
            json!({ "SiteId": sites[site], "Description": format!("device {index}") }),
        )
        .await;
        devices.push((id, site));
    }
    let stored: Vec<String> = devices.iter().map(|(id, _)| id.clone()).collect();
    let unknown = unknown_ids("device");

    for site in [None, Some(0), Some(1)] {
        let selected: Vec<String> = devices
            .iter()
            .filter(|(_, index)| site.is_none_or(|want| *index == want))
            .map(|(id, _)| id.clone())
            .collect();
        for kind in &KINDS {
            let available = if kind.unknown { &unknown } else { &stored };
            for row in &ROWS {
                let case = format!("site {site:?}/{}/{}", kind.name, row.name);
                let values = row.values(available);
                let mut query: Vec<String> = values
                    .iter()
                    .map(|id| format!("deviceIds={}", escape(id)))
                    .collect();
                if let Some(index) = site {
                    query.push(format!("siteId={}", escape(&sites[index])));
                }
                let base = format!("/global-networks/{network}/devices");
                let uri = if query.is_empty() {
                    base
                } else {
                    format!("{base}?{}", query.join("&"))
                };
                let (status, body) = emulator.get(&uri).await;

                if values.len() > MAX_FILTER_VALUES {
                    assert_filter_refusal(&case, status, &body, "DeviceIds", "Devices");
                    continue;
                }

                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} with {body}"
                );
                let expected: Vec<String> = if values.is_empty() {
                    selected.clone()
                } else if kind.unknown {
                    Vec::new()
                } else {
                    selected
                        .iter()
                        .filter(|id| values.contains(id))
                        .cloned()
                        .collect()
                };
                assert_eq!(
                    ids_of(&body, "Devices", "DeviceId"),
                    expected,
                    "case {case}: expected exactly {expected:?}, got {body}"
                );
            }
        }
    }
}
