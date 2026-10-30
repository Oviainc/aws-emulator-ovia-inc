// SPDX-License-Identifier: CC0-1.0

//! `ListActivityTypes`, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! router the emulator mounts in production.
//!
//! The rule every case here asserts: the listing answers from the addressed domain's
//! STORED activity-type records — exactly those whose own `RegistrationStatus` is the
//! requested one and, when the request names a `name`, exactly the versions registered
//! under that name — each reported as the same `ActivityTypeInfo` the per-type reader
//! reports, ordered ascending by name (then version, so two versions of one name hold a
//! reproducible position), reversed under `reverseOrder`, and cut into pages of
//! `maximumPageSize` with a `nextPageToken` that resumes where the page ended.
//!
//! Each table row varies what that rule generalises over: the form and length of the
//! identity (plain, digits, embedded space, non-ASCII, upper case, the modelled maximum
//! lengths), how many versions one name carries, which records are deprecated, which
//! status and name the request selects on, the page size, and the ordering. No row is
//! privileged.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, register_domain_body, string_at, Emulator,
};

/// The modelled maximum length of `ActivityType$name`
/// (`smithy_query(query="shape",shape="com.amazonaws.swf#ActivityType")`: `name` targets
/// `Name`, len 1..256).
const MAX_TYPE_NAME_LEN: usize = 256;

/// The modelled maximum length of `ActivityType$version` (same query: `version` targets
/// `Version`, len 1..64).
const MAX_TYPE_VERSION_LEN: usize = 64;

/// The two modelled `RegistrationStatus` values
/// (`smithy_query(query="shape",shape="com.amazonaws.swf#RegistrationStatus")`), which
/// are also the two values `registrationStatus` selects on.
const REGISTERED: &str = "REGISTERED";
const DEPRECATED: &str = "DEPRECATED";

/// One activity type the suite registers. `deprecated` rows are deprecated after
/// registration, so the two statuses are both populated from stored records.
struct SeedType {
    name: String,
    version: String,
    deprecated: bool,
    /// Optional `RegisterActivityType` members, so a listed `ActivityTypeInfo` is
    /// checked over records that do and do not carry a description.
    description: Option<&'static str>,
}

fn seed_types() -> Vec<SeedType> {
    let row = |name: String, version: String, deprecated: bool, description| SeedType {
        name,
        version,
        deprecated,
        description,
    };
    vec![
        // Three versions of one name, whose lexicographic order ("1.0" < "10.0" < "2.0")
        // is not their numeric one: the order is over the stored strings.
        row("checkout".into(), "1.0".into(), false, Some("first")),
        row("checkout".into(), "10.0".into(), true, None),
        row("checkout".into(), "2.0".into(), false, None),
        row("7".into(), "0001".into(), false, Some("digits only")),
        row("pack items".into(), "v 1".into(), true, None),
        row("活動タイプ".into(), "ß-1".into(), false, Some("unicode")),
        row("ZZ-upper".into(), "1".into(), true, None),
        row(
            "n".repeat(MAX_TYPE_NAME_LEN),
            "v".repeat(MAX_TYPE_VERSION_LEN),
            false,
            None,
        ),
    ]
}

/// The identities of `types` with `status`, in the order the listing must report them
/// without `reverseOrder`: ascending by name, then by version.
fn expected_identities(types: &[SeedType], status: &str, name: Option<&str>) -> Vec<(String, String)> {
    let mut selected: Vec<(String, String)> = types
        .iter()
        .filter(|seed| {
            let seed_status = if seed.deprecated { DEPRECATED } else { REGISTERED };
            seed_status == status && name.is_none_or(|wanted| seed.name == wanted)
        })
        .map(|seed| (seed.name.clone(), seed.version.clone()))
        .collect();
    selected.sort();
    selected
}

/// A `ListActivityTypes` request body. `name`, `maximumPageSize` and `nextPageToken` are
/// folded in only when given, so a row can exercise the modelled defaults.
fn list_body(
    domain: &str,
    status: &str,
    name: Option<&str>,
    reverse_order: bool,
    page_size: Option<i64>,
    token: Option<&str>,
) -> Value {
    let mut body = json!({
        "domain": domain,
        "registrationStatus": status,
        "reverseOrder": reverse_order,
    });
    let object = body.as_object_mut().expect("a JSON object");
    if let Some(name) = name {
        object.insert("name".to_string(), json!(name));
    }
    if let Some(page_size) = page_size {
        object.insert("maximumPageSize".to_string(), json!(page_size));
    }
    if let Some(token) = token {
        object.insert("nextPageToken".to_string(), json!(token));
    }
    body
}

/// The `typeInfos` a listing reported, in the order it reported them.
fn type_infos(case: &str, body: &Value) -> Vec<Value> {
    body.pointer("/typeInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected typeInfos in {body:?}"))
}

/// The `(name, version)` identities a listing reported, in order.
fn identities(case: &str, body: &Value) -> Vec<(String, String)> {
    type_infos(case, body)
        .iter()
        .map(|info| {
            (
                string_at(info, "/activityType/name"),
                string_at(info, "/activityType/version"),
            )
        })
        .collect()
}

/// The `nextPageToken` of a listing response, if it issued one.
fn next_token(body: &Value) -> Option<String> {
    body.get("nextPageToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn register_domain(emulator: &Emulator, case: &str, domain: &str) {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
}

/// Register one type, and deprecate it when the row asks for it.
async fn register_type(emulator: &Emulator, case: &str, domain: &str, seed: &SeedType) {
    let mut body = json!({ "domain": domain, "name": seed.name, "version": seed.version });
    if let Some(description) = seed.description {
        body.as_object_mut()
            .expect("a JSON object")
            .insert("description".to_string(), json!(description));
    }
    emulator.ok(case, "RegisterActivityType", &body).await;
    if seed.deprecated {
        emulator
            .ok(
                case,
                "DeprecateActivityType",
                &json!({
                    "domain": domain,
                    "activityType": { "name": seed.name, "version": seed.version },
                }),
            )
            .await;
    }
}

/// Seed one domain holding every row of the table.
async fn seed(emulator: &Emulator, domain: &str, types: &[SeedType]) {
    register_domain(emulator, domain, domain).await;
    for seed in types {
        register_type(emulator, domain, domain, seed).await;
    }
}

/// The `typeInfo` the per-type reader reports for one identity.
async fn described_info(emulator: &Emulator, case: &str, domain: &str, identity: &(String, String)) -> Value {
    let described = emulator
        .ok(
            case,
            "DescribeActivityType",
            &json!({
                "domain": domain,
                "activityType": { "name": identity.0, "version": identity.1 },
            }),
        )
        .await;
    described
        .pointer("/typeInfo")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a typeInfo in {described:?}"))
}

#[tokio::test]
async fn a_listing_reports_the_stored_types_of_the_requested_status_and_name() {
    let emulator = Emulator::new();
    let domain = "listing";
    let types = seed_types();
    seed(&emulator, domain, &types).await;

    // Every (status, name) selection the table can express: both statuses unfiltered,
    // then each stored name under each status, plus a name no record carries.
    let mut selections: Vec<(String, Option<String>)> = vec![
        (REGISTERED.to_string(), None),
        (DEPRECATED.to_string(), None),
    ];
    let mut names: Vec<String> = types.iter().map(|seed| seed.name.clone()).collect();
    names.push("no-such-activity".to_string());
    names.dedup();
    for status in [REGISTERED, DEPRECATED] {
        for name in &names {
            selections.push((status.to_string(), Some(name.clone())));
        }
    }

    for (status, name) in &selections {
        let case = format!("status {status} / name {name:?}");
        let expected = expected_identities(&types, status, name.as_deref());
        let body = list_body(domain, status, name.as_deref(), false, None, None);
        let response = emulator.ok(&case, "ListActivityTypes", &body).await;
        let reported = identities(&case, &response);
        assert_eq!(
            reported, expected,
            "case {case:?}: the listing reports the stored records of that status and \
             name, ascending by name then version, got {reported:?}"
        );
        assert_eq!(
            next_token(&response),
            None,
            "case {case:?}: a listing that reported every match issues no nextPageToken, \
             got {response:?}"
        );

        // Each reported info is the record the per-type reader reports, so nothing in
        // the listing is synthesised for it.
        for (index, identity) in expected.iter().enumerate() {
            let described = described_info(&emulator, &case, domain, identity).await;
            let listed = &type_infos(&case, &response)[index];
            assert_eq!(
                listed, &described,
                "case {case:?}: the listed info of {identity:?} is the stored record the \
                 reader reports, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn reverse_order_reports_the_same_matches_in_descending_order() {
    let emulator = Emulator::new();
    let domain = "ordering";
    let types = seed_types();
    seed(&emulator, domain, &types).await;

    for status in [REGISTERED, DEPRECATED] {
        for name in [None, Some("checkout")] {
            let case = format!("status {status} / name {name:?} / reverseOrder");
            let mut expected = expected_identities(&types, status, name);
            expected.reverse();
            let response = emulator
                .ok(
                    &case,
                    "ListActivityTypes",
                    &list_body(domain, status, name, true, None, None),
                )
                .await;
            let reported = identities(&case, &response);
            assert_eq!(
                reported, expected,
                "case {case:?}: reverseOrder reports the same matches descending, \
                 got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn paging_a_listing_visits_every_match_exactly_once_in_either_order() {
    let emulator = Emulator::new();
    let domain = "paging";
    let types = seed_types();
    seed(&emulator, domain, &types).await;

    let total = types.len();
    let total_i64 = i64::try_from(total).expect("the table fits in an i64");
    for status in [REGISTERED, DEPRECATED] {
        for reverse_order in [false, true] {
            // Page sizes spanning the class: one at a time, a size that does not divide
            // the match count, a size equal to it and one above it. 0 is the modelled
            // default ("not specified"), which means every match.
            for page_size in [1_i64, 2, 3, total_i64, total_i64 + 1, 0] {
                let case =
                    format!("status {status} / reverseOrder {reverse_order} / page {page_size}");
                let mut expected = expected_identities(&types, status, None);
                if reverse_order {
                    expected.reverse();
                }

                let mut walked: Vec<(String, String)> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0_usize;
                loop {
                    pages += 1;
                    assert!(
                        pages <= total + 2,
                        "case {case:?}: paging must terminate, got {pages} pages after \
                         {walked:?}"
                    );
                    let response = emulator
                        .ok(
                            &case,
                            "ListActivityTypes",
                            &list_body(
                                domain,
                                status,
                                None,
                                reverse_order,
                                Some(page_size),
                                token.as_deref(),
                            ),
                        )
                        .await;
                    let page = identities(&case, &response);
                    if page_size > 0 {
                        let limit = usize::try_from(page_size).expect("a positive page size");
                        assert!(
                            page.len() <= limit,
                            "case {case:?}: a page holds at most maximumPageSize matches, \
                             got {page:?}"
                        );
                    }
                    walked.extend(page);
                    token = next_token(&response);
                    if token.is_none() {
                        break;
                    }
                }

                assert_eq!(
                    walked, expected,
                    "case {case:?}: paging visits every match exactly once, in the \
                     listing's own order, got {walked:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_page_token_is_refused_under_the_other_ordering() {
    let emulator = Emulator::new();
    let domain = "token-ordering";
    let types = seed_types();
    seed(&emulator, domain, &types).await;

    for status in [REGISTERED, DEPRECATED] {
        for reverse_order in [false, true] {
            let case = format!("status {status} / reverseOrder {reverse_order}");
            let first = emulator
                .ok(
                    &case,
                    "ListActivityTypes",
                    &list_body(domain, status, None, reverse_order, Some(1), None),
                )
                .await;
            let token = next_token(&first)
                .unwrap_or_else(|| panic!("case {case:?}: expected a token in {first:?}"));

            // "Keep all other arguments unchanged": the token carries the ordering it was
            // issued for, so replaying it under the other one is refused rather than
            // silently skipping or repeating types.
            emulator
                .refused(
                    &case,
                    "ListActivityTypes",
                    &list_body(domain, status, None, !reverse_order, Some(1), Some(&token)),
                    "ValidationException",
                )
                .await;

            // So is a token this listing never issued.
            for forged in ["not-a-token", "74660000", "tf7a7a"] {
                emulator
                    .refused(
                        &format!("{case} / forged {forged:?}"),
                        "ListActivityTypes",
                        &list_body(
                            domain,
                            status,
                            None,
                            reverse_order,
                            Some(1),
                            Some(forged),
                        ),
                        "ValidationException",
                    )
                    .await;
            }
        }
    }
}

#[tokio::test]
async fn a_listing_is_scoped_to_one_domain() {
    let emulator = Emulator::new();
    let home = "home-domain";
    let other = "other-domain";
    let types = seed_types();
    seed(&emulator, home, &types).await;
    register_domain(&emulator, other, other).await;

    // A domain holding no activity type at all reports an empty list, not a fault.
    for status in [REGISTERED, DEPRECATED] {
        let case = format!("empty domain / status {status}");
        let response = emulator
            .ok(
                &case,
                "ListActivityTypes",
                &list_body(other, status, None, false, None, None),
            )
            .await;
        assert_eq!(
            identities(&case, &response),
            Vec::new(),
            "case {case:?}: a domain holding no matching type reports an empty list, \
             got {response:?}"
        );
        assert_eq!(
            next_token(&response),
            None,
            "case {case:?}: an empty listing issues no nextPageToken, got {response:?}"
        );
    }

    // One row registered in the other domain is reported only there.
    let shared = &types[0];
    register_type(&emulator, "cross-domain", other, shared).await;
    for (domain, expected) in [
        (home, expected_identities(&types, REGISTERED, None)),
        (
            other,
            vec![(shared.name.clone(), shared.version.clone())],
        ),
    ] {
        let case = format!("cross-domain / {domain}");
        let response = emulator
            .ok(
                &case,
                "ListActivityTypes",
                &list_body(domain, REGISTERED, None, false, None, None),
            )
            .await;
        assert_eq!(
            identities(&case, &response),
            expected,
            "case {case:?}: each domain lists only its own records, got {response:?}"
        );
    }

    // A domain no record is stored under is the modelled fault, naming that domain.
    for status in [REGISTERED, DEPRECATED] {
        let case = format!("unknown domain / status {status}");
        let (http_status, body) = emulator
            .call(
                "ListActivityTypes",
                &list_body("no-such-domain", status, None, false, None, None),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (http_status, code.as_str()),
            (400, "UnknownResourceFault"),
            "case {case:?}: expected 400 UnknownResourceFault, got {http_status} {code} \
             with {body:?}"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body:?}"));
        assert_eq!(
            message, "Unknown domain: no-such-domain",
            "case {case:?}: the fault names the requested domain, got {message:?}"
        );
    }
}

#[tokio::test]
async fn listing_repeatedly_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let domain = "read-only";
    let types = seed_types();
    seed(&emulator, domain, &types).await;

    for status in [REGISTERED, DEPRECATED] {
        let case = format!("status {status}");
        let body = list_body(domain, status, None, false, None, None);
        let first = emulator.ok(&case, "ListActivityTypes", &body).await;
        for round in 2..=3 {
            let round_case = format!("{case} / read #{round}");
            // A page taken in between must not move the unpaged listing either.
            emulator
                .ok(
                    &round_case,
                    "ListActivityTypes",
                    &list_body(domain, status, None, false, Some(1), None),
                )
                .await;
            let again = emulator.ok(&round_case, "ListActivityTypes", &body).await;
            assert_eq!(
                again, first,
                "case {round_case:?}: a read reports the same records a later caller \
                 sees, got {again:?}"
            );
        }

        // And the status the listing selected on is still the records' own: a listing
        // moves nothing.
        for identity in expected_identities(&types, status, None) {
            let described = described_info(&emulator, &case, domain, &identity).await;
            assert_eq!(
                string_at(&described, "/status"),
                status,
                "case {case:?}: {identity:?} keeps its stored status, got {described:?}"
            );
        }
    }
}
