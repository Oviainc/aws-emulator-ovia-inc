// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's instance metadata service (IMDS) defaults —
//! `ModifyInstanceMetadataDefaults` and `GetInstanceMetadataDefaults` — driven over the
//! wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: each member of `ModifyInstanceMetadataDefaults` names ONE
//! account-level IMDS default setting; a value from that member's own catalog sets the
//! account's default for it, the member's "no preference" value (`no-preference`, or `-1`
//! for the hop limit) CLEARS it, and a member the request leaves out leaves that setting
//! exactly as it stands — so `GetInstanceMetadataDefaults` reports the settings that have
//! a default and leaves out the ones that do not.
//!
//! So the tables below span that class rather than one recorded request: every setting the
//! request carries, every value the model's own catalog holds for each one, both published
//! bounds of the hop limit's documented range and values past each bound, clearing each
//! setting out of a fully-set account, and sequences of modifies that name different
//! members.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::error_code;
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// `no-preference` — the word every enum setting accepts for "this account states no
/// default", which removes the setting's account-level default.
const NO_PREFERENCE: &str = ec2::state::METADATA_NO_PREFERENCE;

/// The `ManagedBy` every report of this emulator's defaults carries: the settings it can
/// hold are always the account's own.
const MANAGED_BY: &str = ec2::state::MANAGED_BY_ACCOUNT;

/// Hop limits inside the documented range, as the wire carries them.
///
/// Source: `ModifyInstanceMetadataDefaultsRequest$HttpPutResponseHopLimit` — "Possible
/// values: Integers from `1` to `64`, and `-1` to indicate no preference". Both published
/// bounds are here, plus an interior value no recorded request cited; the values PAST each
/// bound are in [`the_hop_limit_range_is_the_documented_one`].
const HOP_LIMITS: &[&str] = &["1", "3", "64"];

/// One account-level IMDS default setting.
struct Setting {
    /// The ec2Query form field of `ModifyInstanceMetadataDefaults` that sets it.
    param: &'static str,
    /// The element `GetInstanceMetadataDefaults` reports it in, inside `<accountLevel>`.
    element: &'static str,
    /// The values it can be set to, as the wire carries them: for the enum settings the
    /// model's own catalog for the member the RESPONSE reports it from.
    values: &'static [&'static str],
    /// The value that states no preference for it, which clears the default.
    no_preference: &'static str,
}

/// Every setting `ModifyInstanceMetadataDefaults` can set. The four enum settings take the
/// model's own generated catalogs, so a member the model gains is tested without touching
/// this file; the hop limit takes [`HOP_LIMITS`], from its documented range.
const SETTINGS: [Setting; 5] = [
    Setting {
        param: "HttpTokens",
        element: "httpTokens",
        values: ec2::ops::types::HTTP_TOKENS_STATE_VALUES,
        no_preference: NO_PREFERENCE,
    },
    Setting {
        param: "HttpEndpoint",
        element: "httpEndpoint",
        values: ec2::ops::types::INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
        no_preference: NO_PREFERENCE,
    },
    Setting {
        param: "InstanceMetadataTags",
        element: "instanceMetadataTags",
        values: ec2::ops::types::INSTANCE_METADATA_TAGS_STATE_VALUES,
        no_preference: NO_PREFERENCE,
    },
    Setting {
        param: "HttpTokensEnforced",
        element: "httpTokensEnforced",
        values: ec2::ops::types::HTTP_TOKENS_ENFORCED_STATE_VALUES,
        no_preference: NO_PREFERENCE,
    },
    Setting {
        param: "HttpPutResponseHopLimit",
        element: "httpPutResponseHopLimit",
        values: HOP_LIMITS,
        no_preference: "-1",
    },
];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The members of the `<accountLevel>` a `GetInstanceMetadataDefaults` response carries,
/// by element name — empty for an account that states no default at all.
fn account_level(case: &str, body: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let level = child(parsed.root_element(), "accountLevel").unwrap_or_else(|| {
        panic!("case {case:?}: no <accountLevel> in the response body {body}")
    });
    level
        .children()
        .filter(|node| node.is_element())
        .map(|node| {
            (
                node.tag_name().name().to_string(),
                node.text().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The account's IMDS defaults as `GetInstanceMetadataDefaults` reports them.
async fn defaults(router: &MultiServiceRouter, case: &str) -> BTreeMap<String, String> {
    let (status, body) = call(router, "GetInstanceMetadataDefaults", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetInstanceMetadataDefaults, got {status} \
         with body {body}"
    );
    account_level(case, &body)
}

/// `ModifyInstanceMetadataDefaults` with `params`, required to succeed and to report
/// `true`: "If the request succeeds, the response returns true" (`Return`).
async fn modify(router: &MultiServiceRouter, case: &str, params: &[Param]) {
    let (status, body) = call(router, "ModifyInstanceMetadataDefaults", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyInstanceMetadataDefaults, got \
         {status} with body {body}"
    );
    let returned = child_text(document(case, &body).root_element(), "return")
        .unwrap_or_else(|| panic!("case {case:?}: no <return> in the response body {body}"));
    assert_eq!(
        returned, "true",
        "case {case:?}: expected ModifyInstanceMetadataDefaults to report true, got \
         {returned:?}"
    );
}

/// One setting set to `value` through its own request member.
async fn set(router: &MultiServiceRouter, case: &str, setting: &Setting, value: &str) {
    modify(router, case, &[param(setting.param, value)]).await;
}

/// The defaults a fully-set account reports: every setting at the first value of its own
/// catalog, plus the `ManagedBy` every non-empty report carries.
fn every_setting_expected() -> BTreeMap<String, String> {
    let mut expected: BTreeMap<String, String> = SETTINGS
        .iter()
        .map(|setting| (setting.element.to_string(), setting.values[0].to_string()))
        .collect();
    expected.insert("managedBy".to_string(), MANAGED_BY.to_string());
    expected
}

/// Set every setting at once, to the first value of its own catalog.
async fn set_every_setting(router: &MultiServiceRouter, case: &str) {
    let params: Vec<Param> = SETTINGS
        .iter()
        .map(|setting| param(setting.param, setting.values[0]))
        .collect();
    modify(router, case, &params).await;
    let reported = defaults(router, case).await;
    assert_eq!(
        reported,
        every_setting_expected(),
        "case {case:?}: expected every setting to be reported after one modify that names \
         them all, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The catalogs
// ---------------------------------------------------------------------------

/// A request's catalog for a setting is the catalog the response reports it from PLUS
/// `no-preference`: the request can ask for a state the response has no word for, because
/// "no preference" is the ABSENCE of a default rather than a value of one.
///
/// The rows are every enum setting of the operation, taken from the model's own generated
/// member lists, so a member either side gains cannot slip past this suite.
#[tokio::test]
async fn a_requests_catalog_is_the_reports_catalog_plus_no_preference() {
    // (case, the catalog the request accepts, the catalog the report uses)
    let cases: [(&str, &[&str], &[&str]); 4] = [
        (
            "HttpTokens",
            ec2::ops::types::METADATA_DEFAULT_HTTP_TOKENS_STATE_VALUES,
            ec2::ops::types::HTTP_TOKENS_STATE_VALUES,
        ),
        (
            "HttpEndpoint",
            ec2::ops::types::DEFAULT_INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
            ec2::ops::types::INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
        ),
        (
            "InstanceMetadataTags",
            ec2::ops::types::DEFAULT_INSTANCE_METADATA_TAGS_STATE_VALUES,
            ec2::ops::types::INSTANCE_METADATA_TAGS_STATE_VALUES,
        ),
        (
            "HttpTokensEnforced",
            ec2::ops::types::DEFAULT_HTTP_TOKENS_ENFORCED_STATE_VALUES,
            ec2::ops::types::HTTP_TOKENS_ENFORCED_STATE_VALUES,
        ),
    ];

    for (case, requested, reported) in cases {
        let mut requested = requested.to_vec();
        requested.sort_unstable();
        let mut expected = reported.to_vec();
        expected.push(NO_PREFERENCE);
        expected.sort_unstable();
        assert_eq!(
            requested, expected,
            "case {case:?}: the request's catalog must be the report's catalog plus \
             {NO_PREFERENCE:?}, got {requested:?} against {expected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// An account that has never called `ModifyInstanceMetadataDefaults` states no default at
/// all, so the report carries an `AccountLevel` with no member in it — not even
/// `ManagedBy`, which names the entity managing settings there are none of.
#[tokio::test]
async fn a_fresh_account_states_no_imds_default() {
    let router = emulator();
    let case = "a fresh account";
    let reported = defaults(&router, case).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no account-level IMDS default, got {reported:?}"
    );
}

/// The rule: a modify that names one setting sets THAT setting's default to the value it
/// names, and the get reads it back — with `ManagedBy` `account`, and with no other
/// setting given a default.
///
/// The rows are every setting the request carries crossed with every value its own catalog
/// holds, each on a fresh account, so each row differs along the setting and the value the
/// rule generalises over.
#[tokio::test]
async fn a_modify_sets_the_default_it_names_and_the_get_reads_it_back() {
    for setting in &SETTINGS {
        for value in setting.values {
            let case = format!("{}={value}", setting.param);
            let router = emulator();
            set(&router, &case, setting, value).await;

            let expected = BTreeMap::from([
                (setting.element.to_string(), (*value).to_string()),
                ("managedBy".to_string(), MANAGED_BY.to_string()),
            ]);
            let reported = defaults(&router, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the report to carry only that setting, got \
                 {reported:?} against {expected:?}"
            );
        }
    }
}

/// A later modify that does not name a setting leaves that setting's default where it was:
/// the defaults ACCUMULATE across calls, and a value is replaced only by a call that names
/// its own member.
///
/// The rows are one setting each, applied in order to one account; after each step every
/// setting named so far must still be reported, so the sequence can be reordered or
/// extended without restating it.
#[tokio::test]
async fn a_setting_the_request_leaves_out_keeps_its_default() {
    let router = emulator();
    let mut expected: BTreeMap<String, String> = BTreeMap::new();

    for setting in &SETTINGS {
        // The LAST value of the catalog on the way in, the first on the way out, so the
        // second pass replaces a default rather than setting a fresh one.
        for value in [
            setting.values[setting.values.len() - 1],
            setting.values[0],
        ] {
            let case = format!("after {}={value}", setting.param);
            set(&router, &case, setting, value).await;
            expected.insert(setting.element.to_string(), value.to_string());
            expected.insert("managedBy".to_string(), MANAGED_BY.to_string());

            let reported = defaults(&router, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected every setting named so far to keep its default, \
                 got {reported:?} against {expected:?}"
            );
        }
    }
}

/// "To remove a parameter's account-level default setting, specify `no-preference`": the
/// no-preference value clears the default of the setting it is given for, and of no other
/// — the report simply stops carrying that member.
///
/// The rows are every setting, each cleared out of an account that has all five set, so
/// clearing is exercised on every member and against the others staying put.
#[tokio::test]
async fn no_preference_clears_only_the_default_it_names() {
    for setting in &SETTINGS {
        let case = format!("clearing {}", setting.param);
        let router = emulator();
        set_every_setting(&router, &case).await;

        modify(&router, &case, &[param(setting.param, setting.no_preference)]).await;

        let mut expected = every_setting_expected();
        expected.remove(setting.element);
        let reported = defaults(&router, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {:?} to clear only that setting, got {reported:?} \
             against {expected:?}",
            setting.no_preference
        );
    }
}

/// Clearing every setting takes the account back to stating no default at all, `ManagedBy`
/// included: the state of an account that never set one.
#[tokio::test]
async fn clearing_every_setting_leaves_no_default_at_all() {
    let router = emulator();
    let case = "clearing every setting";
    set_every_setting(&router, case).await;

    let params: Vec<Param> = SETTINGS
        .iter()
        .map(|setting| param(setting.param, setting.no_preference))
        .collect();
    modify(&router, case, &params).await;

    let reported = defaults(&router, case).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no account-level IMDS default left, got {reported:?}"
    );
}

/// `GetInstanceMetadataDefaults` is a REPORT, so reading cannot change what a later caller
/// sees: a run of gets with no modify between them all report the same defaults.
///
/// The rows are a fresh account and a fully-set one, so the invariant is checked from an
/// empty report as well as a full one.
#[tokio::test]
async fn reading_the_defaults_never_changes_them() {
    /// Gets per case — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    let fresh = emulator();
    let seeded = emulator();
    set_every_setting(&seeded, "the seeded account").await;

    for (case, router, expected) in [
        ("repeated reads of a fresh account", &fresh, BTreeMap::new()),
        (
            "repeated reads of a fully-set account",
            &seeded,
            every_setting_expected(),
        ),
    ] {
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            let reported = defaults(router, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the same defaults on every read, got \
                 {reported:?} against {expected:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The documented refusals
// ---------------------------------------------------------------------------

/// The hop limit's range is the documented one: "Possible values: Integers from `1` to
/// `64`, and `-1` to indicate no preference".
///
/// The rows are each published bound, a value just past each bound, the extremes of the
/// member's own `BoxedInteger` type, and `-1` — which is the hop limit's no-preference
/// value rather than a number of hops. A refused row leaves the account's defaults exactly
/// as they were.
#[tokio::test]
async fn the_hop_limit_range_is_the_documented_one() {
    /// The hop limit the account is seeded with, so "nothing was written" is a claim about
    /// a default that is set.
    const SEEDED: &str = "8";

    // (case, the hop limit the request carries, whether it is accepted)
    let cases: [(&str, &str, bool); 9] = [
        ("the lowest hop limit", "1", true),
        ("the highest hop limit", "64", true),
        ("one past the lowest", "0", false),
        ("one past the highest", "65", false),
        ("one below no-preference", "-2", false),
        ("the most negative integer", "-2147483648", false),
        ("the largest integer", "2147483647", false),
        ("a hop limit of a hundred", "100", false),
        // Accepted, and it CLEARS the default rather than setting it to -1.
        ("no preference", "-1", true),
    ];

    for (case, limit, accepted) in cases {
        let router = emulator();
        modify(&router, case, &[param("HttpPutResponseHopLimit", SEEDED)]).await;

        let (status, body) = call(
            &router,
            "ModifyInstanceMetadataDefaults",
            &[param("HttpPutResponseHopLimit", limit)],
        )
        .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for hop limit {limit}, got {status} with \
                 body {body}"
            );
            let expected = if limit == "-1" {
                BTreeMap::new()
            } else {
                BTreeMap::from([
                    ("httpPutResponseHopLimit".to_string(), limit.to_string()),
                    ("managedBy".to_string(), MANAGED_BY.to_string()),
                ])
            };
            let reported = defaults(&router, case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected hop limit {limit} to leave {expected:?}, got \
                 {reported:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for hop limit {limit}, got {status} with \
                 body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code,
                ec2::state::INVALID_PARAMETER_VALUE,
                "case {case:?}: expected error code {:?} for hop limit {limit}, got \
                 {code:?} with body {body}",
                ec2::state::INVALID_PARAMETER_VALUE
            );
            let reported = defaults(&router, case).await;
            let expected = BTreeMap::from([
                ("httpPutResponseHopLimit".to_string(), SEEDED.to_string()),
                ("managedBy".to_string(), MANAGED_BY.to_string()),
            ]);
            assert_eq!(
                reported, expected,
                "case {case:?}: a refused hop limit must write nothing, got {reported:?} \
                 against {expected:?}"
            );
        }
    }
}

/// Every other refusal these two operations make, each with the EC2 error code and HTTP
/// status it answers — and none of them writes anything.
///
/// The rows differ along what makes the request bad: a word outside a setting's own
/// catalog, the empty string and a catalog member in the wrong case, all refused by the
/// modelled enum constraint before the handler runs and so carrying the boundary's
/// `ValidationError`; a modify that names no setting at all, which asks for no
/// modification and is refused with EC2's `MissingParameter`; and a dry run of each
/// operation, which "would have succeeded" and so is refused with `DryRunOperation`. Every
/// one is a 400 or a 412 — never a 501, which would say the operation is unimplemented.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    set_every_setting(&router, "the refusal table's starting state").await;
    let seeded = every_setting_expected();

    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    let cases: [(&str, &str, Vec<Param>, u16, &str); 7] = [
        (
            "a word the setting's catalog does not hold",
            "ModifyInstanceMetadataDefaults",
            vec![param("HttpTokens", "mandatory")],
            400,
            "ValidationError",
        ),
        (
            "the empty string",
            "ModifyInstanceMetadataDefaults",
            vec![param("HttpEndpoint", "")],
            400,
            "ValidationError",
        ),
        (
            "a catalog member in the wrong case",
            "ModifyInstanceMetadataDefaults",
            vec![param("InstanceMetadataTags", "Enabled")],
            400,
            "ValidationError",
        ),
        (
            // The hop limit is an integer member, so the word every enum setting takes for
            // "no preference" is not a number at all: the boundary refuses it while
            // deserializing, with `MalformedInput` rather than the enum constraint's
            // `ValidationError`. Its own no-preference value is `-1`, covered by
            // [`the_hop_limit_range_is_the_documented_one`].
            "the no-preference WORD on the integer member",
            "ModifyInstanceMetadataDefaults",
            vec![param("HttpPutResponseHopLimit", NO_PREFERENCE)],
            400,
            "MalformedInput",
        ),
        (
            "a modify that names no setting",
            "ModifyInstanceMetadataDefaults",
            Vec::new(),
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a dry run of a modify that would have succeeded",
            "ModifyInstanceMetadataDefaults",
            vec![param("HttpTokens", "required"), param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a get that would have succeeded",
            "GetInstanceMetadataDefaults",
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, action, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {action}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {action}, got \
             {code:?} with body {body}"
        );
        // Each refusal leaves the defaults exactly where they were: the next row starts
        // from the same account this one did.
        let reported = defaults(&router, case).await;
        assert_eq!(
            reported, seeded,
            "case {case:?}: a refused request must write nothing, got {reported:?} against \
             {seeded:?}"
        );
    }
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally.
///
/// The rows are the writer and the reader, so the flag's off value is exercised on both
/// operations that read it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let router = emulator();
    let case = "a modify with DryRun=false";
    modify(
        &router,
        case,
        &[param("HttpTokens", "required"), param("DryRun", "false")],
    )
    .await;

    let case = "a get with DryRun=false";
    let (status, body) = call(
        &router,
        "GetInstanceMetadataDefaults",
        &[param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetInstanceMetadataDefaults, got {status} \
         with body {body}"
    );
    let expected = BTreeMap::from([
        ("httpTokens".to_string(), "required".to_string()),
        ("managedBy".to_string(), MANAGED_BY.to_string()),
    ]);
    let reported = account_level(case, &body);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the modify to have run, got {reported:?} against \
         {expected:?}"
    );
}
