// SPDX-License-Identifier: CC0-1.0

//! The length table every suite of the filter-maximum class shares, and the refusal such a
//! filter must answer past its bound.
//!
//! This module holds no test of its own: it states ONE rule for the suites that do, so
//! none of them can disagree with another about the bound, the lengths exercised, the
//! other parameters crossed with them, or what the refusal carries.
//!
//! The rule: a list-valued filter member whose documentation publishes a maximum number of
//! values is refused — with the modelled `ValidationException` (HTTP 400) naming that
//! member, and no page at all — as soon as the request carries more than that many values.
//! Every such member of this service publishes the same number ("One or more site IDs. The
//! maximum is 10.", `smithy_query query="doc" shape="GetSitesRequest" member="SiteIds"`,
//! and the same sentence on `GetLinksRequest$LinkIds`, `GetDevicesRequest$DeviceIds`,
//! `DescribeGlobalNetworksRequest$GlobalNetworkIds` and
//! `GetTransitGatewayRegistrationsRequest$TransitGatewayArns`), and the modelled list
//! shapes express it nowhere — they carry a length on their member STRING only
//! (`smithy_query shape=SiteIdList,LinkIdList,DeviceIdList`) — so the refusal is the
//! handler's to answer.
//!
//! The suites that consume this table: `crate::filter_id_list_maximum` (every id-list
//! filter, by length), `crate::site_ids_filter_maximum` (`SiteIds` and
//! `TransitGatewayArns`, crossed with the rest of the query string) and
//! `crate::link_ids_filter_maximum` (`LinkIds` and `DeviceIds`, crossed with the value
//! selectors their listings take).

use serde_json::{json, Value};

use crate::global_network_lifecycle::error_code;

/// The published maximum number of values one of these filter members may carry (see this
/// module's own documentation for the sentence that publishes it and the members it is
/// published on).
pub(crate) const MAX_FILTER_VALUES: usize = 10;

/// The largest page size the model allows: `MaxResults` is `range 1..500`
/// (`smithy_query query="operation" operation="GetSites"`), so a request at the top of
/// that range is inside the modelled constraint and still cannot lift the filter's bound.
const LARGEST_PAGE_SIZE: usize = 500;

/// How many resources of each kind a fixture creates: two past the bound, so a request at
/// the bound, one past it and one two past it can all be built from DISTINCT values.
pub(crate) const FIXTURE_COUNT: usize = MAX_FILTER_VALUES + 2;

/// One row of the length table: how many filter values the request carries, and whether
/// the row cycles the available values to reach that count (so values repeat — the bound
/// counts the values the list carries, not the distinct resources they name).
pub(crate) struct Row {
    pub(crate) name: &'static str,
    pub(crate) count: usize,
    pub(crate) repeating: bool,
}

/// The rows, spanning the whole class the rule generalises over: the filter absent, well
/// inside the bound, one below it, exactly at it, one past it, two past it, and far past
/// it with repeated values.
pub(crate) const ROWS: [Row; 7] = [
    Row {
        name: "filter absent",
        count: 0,
        repeating: false,
    },
    Row {
        name: "one value",
        count: 1,
        repeating: false,
    },
    Row {
        name: "one below the maximum",
        count: MAX_FILTER_VALUES - 1,
        repeating: false,
    },
    Row {
        name: "exactly the maximum",
        count: MAX_FILTER_VALUES,
        repeating: false,
    },
    Row {
        name: "one past the maximum",
        count: MAX_FILTER_VALUES + 1,
        repeating: false,
    },
    Row {
        name: "two past the maximum",
        count: MAX_FILTER_VALUES + 2,
        repeating: false,
    },
    Row {
        name: "far past the maximum, with repeated values",
        count: MAX_FILTER_VALUES * 3,
        repeating: true,
    },
];

impl Row {
    /// The values this row sends, taken from `available`: a prefix of it, or — when the
    /// row repeats — as many values as the row asks for, cycling through it.
    pub(crate) fn values(&self, available: &[String]) -> Vec<String> {
        if self.repeating {
            return available
                .iter()
                .cycle()
                .take(self.count)
                .map(String::clone)
                .collect();
        }
        assert!(
            self.count <= available.len(),
            "case {}: the fixture holds {} values, too few for {}",
            self.name,
            available.len(),
            self.count
        );
        available[..self.count].to_vec()
    }
}

/// One other query parameter the request may carry beside the filter.
pub(crate) struct Extra {
    pub(crate) name: &'static str,
    pub(crate) query: &'static str,
    /// The page size this extra asks for, when it asks for one.
    pub(crate) limit: Option<usize>,
    /// Whether this extra carries a page token this service never issued.
    pub(crate) forged_token: bool,
}

/// The extras: nothing else at all, the smallest page size, the largest the model allows,
/// and a forged page token (the other refusal these listings can answer).
pub(crate) const EXTRAS: [Extra; 4] = [
    Extra {
        name: "no other parameter",
        query: "",
        limit: None,
        forged_token: false,
    },
    Extra {
        name: "a page size of one",
        query: "maxResults=1",
        limit: Some(1),
        forged_token: false,
    },
    Extra {
        name: "the largest page size the model allows",
        query: "maxResults=500",
        limit: Some(LARGEST_PAGE_SIZE),
        forged_token: false,
    },
    Extra {
        name: "a page token this service never issued",
        query: "nextToken=not-a-token-this-service-issued",
        limit: None,
        forged_token: true,
    },
];

/// Whether the filter values a row sends name stored resources or nothing at all.
pub(crate) struct Kind {
    pub(crate) name: &'static str,
    /// True when the values name nothing stored, so the listing selects nothing.
    pub(crate) unknown: bool,
}

/// The two kinds: a fixture's own ids, and ids of the minted shape that name nothing.
pub(crate) const KINDS: [Kind; 2] = [
    Kind {
        name: "stored ids",
        unknown: false,
    },
    Kind {
        name: "ids naming nothing",
        unknown: true,
    },
];

/// Assert `body` is the modelled refusal of the filter member `member` past its published
/// maximum, carrying no page in `list`.
pub(crate) fn assert_filter_refusal(
    case: &str,
    status: u16,
    body: &Value,
    member: &str,
    list: &str,
) {
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("FieldValidationFailed"),
        "case {case}: expected a field-level Reason, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!(member),
        "case {case}: the refusal must name {member}, got {body}"
    );
    assert!(
        body["Message"].as_str().is_some_and(|text| text
            .contains(member)
            && text.contains(&MAX_FILTER_VALUES.to_string())),
        "case {case}: the message must name {member} and the maximum, got {body}"
    );
    assert!(
        body[list].is_null(),
        "case {case}: a refused request must carry no page, got {body}"
    );
}

/// [`FIXTURE_COUNT`] ids of the minted shape (`<kind>-<17 lowercase hex>`) that name
/// nothing: the leading `f` puts them above anything the monotonic id counter can mint.
pub(crate) fn unknown_ids(kind: &str) -> Vec<String> {
    (0..FIXTURE_COUNT)
        .map(|index| format!("{kind}-f{index:016x}"))
        .collect()
}
