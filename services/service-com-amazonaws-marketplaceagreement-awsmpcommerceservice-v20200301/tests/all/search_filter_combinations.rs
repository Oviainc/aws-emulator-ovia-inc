// SPDX-License-Identifier: CC0-1.0

//! Which filter LISTS `SearchAgreements` searches on, end to end over HTTP.
//!
//! `selector_values.rs` pins the rule about one filter's VALUE and
//! `validation_envelope.rs` the rules about one filter's shape. The rule here is
//! about the list as a whole, which `SearchAgreementsInput$filters` states twice
//! (`smithy_query(query="doc", shape="SearchAgreementsInput",
//! member="filters")`):
//!
//! * "A proposer can use any combination of the preceding filters along with
//!   `AgreementType`, which is required" — so every search carries the
//!   agreement-type filter, whatever else it names and whatever party type it is
//!   scoped to, since every combination the member lists for the acceptor begins
//!   with it too.
//! * "The following filter combinations are supported when the `PartyType` is
//!   `Acceptor`" — so a search scoped to that party type may add only the names
//!   one of those combinations adds, each of them optionally with the end-time
//!   filters ("To filter by `EndTime`, you can use `BeforeEndTime`,
//!   `AfterEndTime`, or both").
//!
//! A list outside those rules is refused with HTTP 400 `ValidationException`
//! carrying the modelled `UNSUPPORTED_FILTERS` reason and the sentence that
//! states the combination rather than one bad member — and refused before any
//! agreement is scanned, so every row runs against a store holding an agreement
//! and against one holding nothing, where a rule applied only while scanning
//! would answer an empty page instead.
//!
//! The rows differ along what the rule generalises over: which names the list
//! carries, how many, in which order, which party type the search is scoped to
//! (the acceptor, the proposer, and no scope at all), and whether the end-time
//! filters are added — so the same names that are refused under the acceptor
//! scope are shown to be SERVED under the other two.
//!
//! The harness (`emulator`, `call`, `agreement_over`, `term`) is the one
//! `agreement_lifecycle` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{agreement_over, call, emulator, term, Answer};

/// Every filter name `SearchAgreementsInput$filters` documents, in the order the
/// member lists them.
const DOCUMENTED_FILTER_NAMES: &[&str] = &[
    "ResourceIdentifier",
    "ResourceType",
    "PartyType",
    "AcceptorAccountId",
    "OfferId",
    "Status",
    "BeforeEndTime",
    "AfterEndTime",
    "BeforeStartTime",
    "AfterStartTime",
    "BeforeLastUpdateTime",
    "AfterLastUpdateTime",
    "AgreementType",
    "OfferSetId",
    "EndTimeBehaviorType",
    "EndTimeBehaviorReasonCode",
    "InitialAgreementId",
    "LicenseArn",
];

/// The filter every search must carry.
const AGREEMENT_TYPE: &str = "AgreementType";

/// The filter a search is scoped to a party type by.
const PARTY_TYPE: &str = "PartyType";

/// The two names the member writes `EndTime` in its combination list, which
/// every combination of that list admits with and without.
const END_TIME: &[&str] = &["BeforeEndTime", "AfterEndTime"];

/// The names one documented acceptor combination adds BESIDE the agreement type
/// it begins with — the end-time filters left to [`END_TIME`], since the member
/// lists every combination both with and without them.
const SUPPORTED_FOR_ACCEPTOR: &[&[&str]] = &[
    &[],
    &["Status"],
    &["ResourceIdentifier"],
    &["ResourceIdentifier", "Status"],
    &["ResourceType"],
    &["OfferId"],
    &["OfferId", "Status"],
    &["OfferSetId"],
    &["OfferSetId", "Status"],
];

/// Name sets the member does NOT list for the acceptor: the one the issue
/// reported (an acceptor account id), the pairing of a resource type with a
/// status (every other identifying attribute is listed with `Status`, that one
/// is not), two identifying attributes at once, and each remaining documented
/// name — all of them names a PROPOSER may combine freely, which is what the
/// rows below check on the other two scopes.
const UNSUPPORTED_FOR_ACCEPTOR: &[&[&str]] = &[
    &["AcceptorAccountId"],
    &["ResourceType", "Status"],
    &["OfferId", "OfferSetId"],
    &["ResourceIdentifier", "ResourceType"],
    &["BeforeStartTime"],
    &["AfterStartTime"],
    &["BeforeLastUpdateTime"],
    &["AfterLastUpdateTime"],
    &["EndTimeBehaviorType"],
    &["EndTimeBehaviorReasonCode"],
    &["InitialAgreementId"],
    &["LicenseArn"],
    &["Status", "LicenseArn"],
];

/// The sentence the service states a filter list it does not support with.
const UNSUPPORTED_MESSAGE: &str = "Provided combination of filters is not supported.";

/// A value `name` can select on that `FilterValue` admits (`@length(1, 256)`,
/// `^[A-Za-z0-9.,+:/_-]+$`), taken from the closed set its attribute publishes
/// where it has one. WHAT a filter selects is `agreement_lifecycle`'s rule; here
/// the value only has to be one the service would search on, so that a refusal
/// can come from the combination alone.
fn value_for(name: &str) -> &'static str {
    match name {
        "ResourceIdentifier" => "res-0000000000000001",
        "ResourceType" => "SaaSProduct",
        "PartyType" => "Proposer",
        "AcceptorAccountId" => "000000000000",
        "OfferId" => "ap-offer1",
        "OfferSetId" => "os-0000000000000001",
        "Status" => "ACTIVE",
        "AgreementType" => "PurchaseAgreement",
        "EndTimeBehaviorType" => "RENEW",
        "EndTimeBehaviorReasonCode" => "NO_RENEWAL_TERM",
        "InitialAgreementId" => "agmt-0000000000000001",
        "LicenseArn" => "arn:aws:license-manager:us-east-1:000000000000:license/l-1",
        // Every `Before…`/`After…` filter selects on an instant.
        _ => "2999-01-01T00:00:00Z",
    }
}

/// The filter list naming `names`, each with a value its attribute admits. A
/// `PartyType` entry carries `scope` instead, so one list builder serves every
/// scope the rows span.
fn filters_of(names: &[&str], scope: Option<&str>) -> Value {
    let mut filters: Vec<Value> = names
        .iter()
        .map(|name| json!({"name": name, "values": [value_for(name)]}))
        .collect();
    if let Some(scope) = scope {
        filters.push(json!({"name": PARTY_TYPE, "values": [scope]}));
    }
    Value::Array(filters)
}

/// Two stores, so a rule applied only while scanning cannot pass: one holding an
/// agreement, and one holding nothing at all.
async fn stores() -> Vec<(&'static str, MultiServiceRouter)> {
    let held = emulator();
    agreement_over(&held, "ap-offer1", vec![term("eula.v1")]).await;
    vec![
        ("a store holding an agreement", held),
        ("an empty store", emulator()),
    ]
}

/// The search `filters` asks for, sent with nothing else that could be refused.
async fn search(router: &MultiServiceRouter, filters: &Value) -> Answer {
    call(router, "SearchAgreements", json!({"filters": filters})).await
}

/// Assert `answer` is the modelled refusal of a filter list the service does not
/// support, naming the list rather than one of its elements.
fn assert_unsupported(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        "UNSUPPORTED_FILTERS",
        "case {case:?}: expected reason UNSUPPORTED_FILTERS, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        UNSUPPORTED_MESSAGE,
        "case {case:?}: expected {UNSUPPORTED_MESSAGE:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        "filters",
        "case {case:?}: no single element of the list is the wrong one, so the entry names \
         the list, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get("agreementViewSummaries").is_none(),
        "case {case:?}: a refused search reports nothing, got {:?}",
        answer.body
    );
}

/// Assert `answer` is a served search: a page, whatever it selected.
fn assert_served(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a supported combination must be searched on, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.get("agreementViewSummaries").is_some(),
        "case {case:?}: a served search answers a page, got {:?}",
        answer.body
    );
}

/// The agreement-type filter is required of every search: a list that omits it
/// is refused whatever else it names, and the same list with it added is served.
#[tokio::test]
async fn every_search_carries_the_required_agreement_type_filter() {
    for (store, router) in stores().await {
        // A search carrying no filter at all names no combination either.
        let answer = call(&router, "SearchAgreements", json!({})).await;
        assert_unsupported(&format!("no filters at all ({store})"), &answer);

        let answer = search(&router, &filters_of(&[AGREEMENT_TYPE], None)).await;
        assert_served(&format!("the required filter alone ({store})"), &answer);

        for name in DOCUMENTED_FILTER_NAMES {
            if *name == AGREEMENT_TYPE {
                continue;
            }
            let without = search(&router, &filters_of(&[*name], None)).await;
            assert_unsupported(&format!("only the {name} filter ({store})"), &without);

            let with = search(&router, &filters_of(&[*name, AGREEMENT_TYPE], None)).await;
            assert_served(
                &format!("the {name} filter beside the required one ({store})"),
                &with,
            );
        }

        // Several documented names at once, still without the required one.
        let many: Vec<&str> = DOCUMENTED_FILTER_NAMES
            .iter()
            .copied()
            .filter(|name| *name != AGREEMENT_TYPE)
            .take(4)
            .collect();
        let answer = search(&router, &filters_of(&many, None)).await;
        assert_unsupported(
            &format!("four documented filters, none of them the required one ({store})"),
            &answer,
        );
    }
}

/// A search scoped to the acceptor party type is served only for the
/// combinations the model lists for it, each with and without the end-time
/// filters — and the lists it refuses there are served under the proposer scope
/// and under no scope at all, so the rule is keyed to the scope rather than to
/// the names.
#[tokio::test]
async fn an_acceptor_scoped_search_serves_only_the_documented_combinations() {
    // Every way the member's note lets a combination name `EndTime`.
    let end_time_variants: Vec<Vec<&str>> = vec![
        vec![],
        vec![END_TIME[0]],
        vec![END_TIME[1]],
        vec![END_TIME[0], END_TIME[1]],
    ];

    for (store, router) in stores().await {
        for added in SUPPORTED_FOR_ACCEPTOR {
            for end_time in &end_time_variants {
                let mut names = vec![AGREEMENT_TYPE];
                names.extend_from_slice(added);
                names.extend_from_slice(end_time);
                let case = format!("acceptor-scoped {names:?} ({store})");
                let answer = search(&router, &filters_of(&names, Some("Acceptor"))).await;
                assert_served(&case, &answer);

                // The same names in the reverse order are the same
                // combination: the rule is about which names the list
                // carries, not where they sit.
                let reversed: Vec<&str> = names.iter().rev().copied().collect();
                let answer = search(&router, &filters_of(&reversed, Some("Acceptor"))).await;
                assert_served(&format!("{case}, in reverse order"), &answer);
            }
        }

        for added in UNSUPPORTED_FOR_ACCEPTOR {
            let mut names = vec![AGREEMENT_TYPE];
            names.extend_from_slice(added);
            let refused = search(&router, &filters_of(&names, Some("Acceptor"))).await;
            assert_unsupported(&format!("acceptor-scoped {names:?} ({store})"), &refused);

            // The proposer may combine the preceding filters freely, and a
            // search naming no party type is scoped to neither: the same names
            // are served there.
            for scope in [Some("Proposer"), None] {
                let served = search(&router, &filters_of(&names, scope)).await;
                assert_served(&format!("{scope:?}-scoped {names:?} ({store})"), &served);
            }
        }
    }
}
