// SPDX-License-Identifier: CC0-1.0

//! The unsupported-filter-NAME rule has ONE statement in this crate —
//! `state::filter_name_refusal` — and both filter lists this service pages are judged
//! and worded by it.
//!
//! RULE: a filter narrows a result set by a named criterion, so a filter list is read
//! only when every filter names one of the modelled filter-key values of that list (its
//! `@enum` key shape, read here off the generated `*_FILTER_KEY_VALUES` constants); a
//! name outside that vocabulary is refused with the modelled `ValidationException`
//! (HTTP 400) naming the offending name and the names the list supports, never dropped —
//! dropping it would answer a broader result set than the caller asked for.
//!
//! The suite exists because that check and its sentence used to be written out once per
//! list handler, each against its own values constant: the rows below drive the one
//! statement with BOTH vocabularies and drive each list end-to-end through the operation
//! that reaches it, so the two can no longer answer different refusals — or admit
//! different names.
//!
//! The rows differ along what the rule generalises over: WHICH list (and so which
//! operation, vocabulary and filter-value shape) is read, the FORM of the unsupported
//! name (empty, the other letter case, a prefix of a supported name, a supported name
//! with a character appended or a separator changed, one far longer than any supported
//! name, and the key of this service's other filter list), the POSITION of the offending
//! filter among several, and — the admitting half — every name each vocabulary does
//! support.

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::{
    DEPLOYMENT_FILTER_KEY_VALUES, DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES,
};
use service_com_amazonaws_launchwizard_launchwizard::state::filter_name_refusal;

use crate::deployment_lifecycle::create;
use crate::deployment_update::one_pattern_per_workload;
use crate::harness::Harness;

/// The wire error code and HTTP status a refused filter name carries
/// (`ValidationException`, `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// The sentence the ONE rule refuses a name outside `supported` with: the offending name,
/// and the vocabulary that list does support. Written once here, and required of both
/// vocabularies below.
fn expected_refusal(name: &str, supported: &[&str]) -> String {
    format!(
        "'{name}' is not a supported filter name; supported names are {}.",
        supported.join(", ")
    )
}

/// `value` with every ASCII letter in the other case — a name that differs from a
/// supported one only in case, which filter names are documented to be sensitive to
/// ("Filter names are case-sensitive", `DeploymentFilter$name`).
fn swap_case(value: &str) -> String {
    value
        .chars()
        .map(|letter| {
            if letter.is_ascii_uppercase() {
                letter.to_ascii_lowercase()
            } else {
                letter.to_ascii_uppercase()
            }
        })
        .collect()
}

/// The unsupported names to drive against `supported`, derived from `supported` itself
/// and from `other` (the vocabulary of this service's other filter list) so the table
/// spans FORMS of an unsupported name rather than one written-out string. Rows that a
/// vocabulary happens to support are dropped, so the same derivation serves either list.
fn unsupported_names(supported: &[&str], other: &[&str]) -> Vec<(String, String)> {
    let first = supported[0];
    let mut rows: Vec<(String, String)> = vec![
        ("the empty name".to_string(), String::new()),
        (
            "a supported name in the other letter case".to_string(),
            swap_case(first),
        ),
        (
            "a supported name with a trailing space".to_string(),
            format!("{first} "),
        ),
        (
            "a supported name with one character appended".to_string(),
            format!("{first}X"),
        ),
        (
            "a prefix of a supported name".to_string(),
            first[..first.len() / 2].to_string(),
        ),
        (
            "a supported name with its separator changed".to_string(),
            first.replace('_', "-"),
        ),
        (
            "a name far longer than any supported one".to_string(),
            "x".repeat(4 * first.len() + 16),
        ),
    ];
    for name in other {
        rows.push((
            "the filter key of this service's other filter list".to_string(),
            (*name).to_string(),
        ));
    }
    rows.retain(|(_, name)| !supported.contains(&name.as_str()));
    assert!(
        rows.len() >= 5,
        "the table must span several forms of an unsupported name for {supported:?}, got {rows:?}"
    );
    rows
}

/// One filter list this service pages: the vocabulary that scopes it and the request
/// that reaches it.
struct ListUnderTest {
    case: String,
    /// The route the model binds the operation reading this list to.
    path: &'static str,
    /// The member a page of this list's items travels on.
    items: &'static str,
    /// The modelled filter-key values of this list.
    supported: &'static [&'static str],
    /// The other list's vocabulary, which this one must not accept.
    other: &'static [&'static str],
    /// The request body without any `filters` member.
    base: Value,
    /// A filter value inside the modelled value shape of this list's filters
    /// (`DeploymentFilterValue` is `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)`;
    /// `DeploymentPatternVersionFilterValue` is `^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$`).
    value: &'static str,
}

impl ListUnderTest {
    /// This list's request, carrying `filters`.
    fn request(&self, filters: Value) -> Value {
        let mut object = self
            .base
            .as_object()
            .expect("a request body is an object")
            .clone();
        object.insert("filters".to_string(), filters);
        Value::Object(object)
    }

    /// One filter naming `name`, with a value inside this list's modelled value shape.
    fn filter(&self, name: &str) -> Value {
        json!({"name": name, "values": [self.value]})
    }

    /// The items `body` reports, requiring HTTP 200.
    async fn listed(&self, harness: &Harness, case: &str, body: Value) -> Vec<Value> {
        let reply = harness.ok(case, self.path, body).await;
        reply[self.items]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} reported no {}: {reply}",
                    self.path, self.items
                )
            })
            .clone()
    }
}

/// Both filter lists, each with a request that reaches it. The pattern-version list is
/// driven once per workload, so the vocabulary rule runs over every catalogue entry its
/// operation can be asked about.
async fn lists(harness: &Harness) -> Vec<ListUnderTest> {
    create(
        harness,
        "a deployment for the filtered list to narrow",
        "SAP",
        "SapHanaSingle",
        "filter-name-rule",
    )
    .await;
    let mut lists = vec![ListUnderTest {
        case: "ListDeployments".to_string(),
        path: "/listDeployments",
        items: "deployments",
        supported: DEPLOYMENT_FILTER_KEY_VALUES,
        other: DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES,
        base: json!({}),
        value: "SAP",
    }];
    for (workload, pattern) in one_pattern_per_workload(harness).await {
        lists.push(ListUnderTest {
            case: format!("ListDeploymentPatternVersions {workload}/{pattern}"),
            path: "/listDeploymentPatternVersions",
            items: "deploymentPatternVersions",
            supported: DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES,
            other: DEPLOYMENT_FILTER_KEY_VALUES,
            base: json!({
                "workloadName": workload,
                "deploymentPatternName": pattern,
            }),
            value: "1.0.0",
        });
    }
    lists
}

#[test]
fn both_vocabularies_are_refused_by_the_one_sentence() {
    // The same offending name, judged against each list's vocabulary: one template, with
    // only the supported names it lists differing. Rendering the vocabulary out of each
    // sentence must leave the two byte-identical, which is what "one statement of the
    // rule" means on the wire.
    const OFFENDING: &str = "no-such-filter-name";
    let vocabularies = [
        DEPLOYMENT_FILTER_KEY_VALUES,
        DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES,
    ];
    let mut templates: Vec<String> = Vec::new();
    for supported in vocabularies {
        let refusal = filter_name_refusal(supported, [OFFENDING]).unwrap_or_else(|| {
            panic!("a name outside {supported:?} must be refused, got no refusal")
        });
        let expected = expected_refusal(OFFENDING, supported);
        assert_eq!(
            refusal, expected,
            "vocabulary {supported:?}: expected {expected:?}, got {refusal:?}"
        );
        assert!(
            refusal.contains(&supported.join(", ")),
            "vocabulary {supported:?}: a refusal states the names it does support, got {refusal:?}"
        );
        templates.push(refusal.replace(&supported.join(", "), "<the supported names>"));
    }
    assert_eq!(
        templates[0], templates[1],
        "both vocabularies must be refused by one sentence, got {templates:?}"
    );
}

#[tokio::test]
async fn every_filtered_list_refuses_a_name_outside_its_vocabulary_by_the_one_rule() {
    let harness = Harness::new();
    for list in lists(&harness).await {
        let supported = list.supported;

        // The admitting half: every name this list's vocabulary holds passes the one
        // check, and the operation reading the list answers a page it narrows rather
        // than refusing — so the rows below are about the vocabulary and not about
        // filters having stopped working.
        // No `filters` member at all: `DeploymentFilterList` declares a length floor of
        // one, so an EMPTY list is a different request from an absent one.
        let unfiltered = list
            .listed(
                &harness,
                &format!("{}: no filter", list.case),
                list.base.clone(),
            )
            .await;
        for name in supported {
            let case = format!("{}: the supported name {name:?}", list.case);
            assert!(
                filter_name_refusal(supported, [*name]).is_none(),
                "case {case:?}: a modelled filter key must be admitted, got a refusal"
            );
            let page = list
                .listed(&harness, &case, list.request(json!([list.filter(name)])))
                .await;
            assert!(
                page.len() <= unfiltered.len(),
                "case {case:?}: a filter narrows, so it cannot report more than the {} \
                 unfiltered items, got {}",
                unfiltered.len(),
                page.len()
            );
        }

        for (label, name) in unsupported_names(supported, list.other) {
            let case = format!("{}: {label}", list.case);
            let expected = expected_refusal(&name, supported);

            // The one statement of the rule, as each list's handler reaches it.
            let refusal = filter_name_refusal(supported, [name.as_str()]);
            assert_eq!(
                refusal.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the refusal {expected:?}, got {refusal:?}"
            );
            // The offending name is found wherever it sits among the filters a request
            // carries, and the first offending name is the one reported.
            let after_supported =
                filter_name_refusal(supported, [supported[0], name.as_str()]);
            assert_eq!(
                after_supported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: a supported name ahead of it changes nothing, got \
                 {after_supported:?}"
            );
            let ahead_of_another = filter_name_refusal(supported, [name.as_str(), "another-name"]);
            assert_eq!(
                ahead_of_another.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: the FIRST offending name is the one reported, got \
                 {ahead_of_another:?}"
            );

            // And end-to-end: such a request is refused with the modelled exception and
            // reports no items. The name's vocabulary is also the protocol boundary's
            // business (both key shapes are `@enum`), so whichever check answers first,
            // the status, the code and the absence of a page are the same.
            let body = harness
                .refused(
                    &case,
                    list.path,
                    list.request(json!([list.filter(&name)])),
                    STATUS,
                    CODE,
                )
                .await;
            assert!(
                body.get(list.items).is_none(),
                "case {case:?}: a refused request must report no {}, got {body}",
                list.items
            );
        }
    }
}
