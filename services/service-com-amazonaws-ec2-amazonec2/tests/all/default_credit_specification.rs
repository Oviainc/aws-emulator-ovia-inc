// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's default credit option for CPU usage of a burstable
//! performance instance family — `ModifyDefaultCreditSpecification` and
//! `GetDefaultCreditSpecification` — driven over the wire the service actually speaks:
//! `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the default credit option of one burstable instance family is an
//! account-level setting OF A REGION, so a report of it answers the option the last modify
//! set for THAT family in THAT Region, and AWS's own default for the family until one does;
//! the modify takes only the two documented options and writes nothing when it refuses a
//! request.
//!
//! So the tables below span that class rather than one recorded request: every family the
//! model's own `UnlimitedSupportedInstanceFamily` catalog holds, crossed with both
//! documented credit options, several Regions, repeated reads, and the values a modify
//! refuses.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message};
use super::wire::{child, child_text, document, param, Param};

/// The Region most calls here are signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// Other Regions the setting is exercised in, so "a setting of a Region" is asserted across
/// Regions rather than in one.
const OTHER_REGIONS: [&str; 2] = ["eu-west-1", "ap-southeast-2"];

/// AWS's own default credit option for each burstable family, which is what an account that
/// has never modified the family reports.
///
/// Source: Amazon EC2 User Guide, "Burstable performance instances" — "T2 instances launch
/// as `standard` by default", while "T3, T3a and T4g instances launch as `unlimited` by
/// default"; a later unlimited-supported family follows T3 rather than T2. The family list
/// is the model's own `UnlimitedSupportedInstanceFamily` catalog
/// (`smithy_query(query="shape",shape="UnlimitedSupportedInstanceFamily")`), which
/// [`the_table_covers_the_whole_modelled_catalog`] pins this table against — so `t8i`, a
/// family no recorded request cites, is covered here too.
const AWS_DEFAULTS: [(&str, &str); 5] = [
    ("t2", "standard"),
    ("t3", "unlimited"),
    ("t3a", "unlimited"),
    ("t4g", "unlimited"),
    ("t8i", "unlimited"),
];

/// The two credit options a default credit specification can hold: "Valid values are
/// `standard` and `unlimited`" (`InstanceFamilyCreditSpecification$CpuCredits`).
const CPU_CREDITS: [&str; 2] = ["standard", "unlimited"];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, signed for `region`, as its HTTP status and XML body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, action, params).await
}

/// One call signed for [`REGION`].
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call_in(router, REGION, action, params).await
}

/// The `<instanceFamilyCreditSpecification>` a response carries, by element name.
fn specification(case: &str, body: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let reported = child(parsed.root_element(), "instanceFamilyCreditSpecification")
        .unwrap_or_else(|| {
            panic!("case {case:?}: no <instanceFamilyCreditSpecification> in the body {body}")
        });
    reported
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

/// The specification a family and a credit option make, as a response reports it.
fn expected(family: &str, cpu_credits: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("instanceFamily".to_string(), family.to_string()),
        ("cpuCredits".to_string(), cpu_credits.to_string()),
    ])
}

/// What `GetDefaultCreditSpecification` reports for `family` in `region`.
async fn reported(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    family: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call_in(
        router,
        region,
        "GetDefaultCreditSpecification",
        &[param("InstanceFamily", family)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetDefaultCreditSpecification for {family}, \
         got {status} with body {body}"
    );
    specification(case, &body)
}

/// `ModifyDefaultCreditSpecification` for `family` in `region`, required to succeed, as the
/// specification it reports.
async fn modify_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    family: &str,
    cpu_credits: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call_in(
        router,
        region,
        "ModifyDefaultCreditSpecification",
        &[
            param("InstanceFamily", family),
            param("CpuCredits", cpu_credits),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyDefaultCreditSpecification for \
         {family}={cpu_credits}, got {status} with body {body}"
    );
    specification(case, &body)
}

/// The same modify, signed for [`REGION`].
async fn modify(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    cpu_credits: &str,
) -> BTreeMap<String, String> {
    modify_in(router, REGION, case, family, cpu_credits).await
}

// ---------------------------------------------------------------------------
// The catalog
// ---------------------------------------------------------------------------

/// [`AWS_DEFAULTS`] states a default for every family the model declares and for no other:
/// a family the model gains cannot slip past the tables below untested.
#[tokio::test]
async fn the_table_covers_the_whole_modelled_catalog() {
    let case = "the modelled instance-family catalog";
    let mut tabled: Vec<&str> = AWS_DEFAULTS.iter().map(|(family, _)| *family).collect();
    tabled.sort_unstable();
    let mut modelled = ec2::ops::types::UNLIMITED_SUPPORTED_INSTANCE_FAMILY_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case {case:?}: expected a default for every modelled family, got {tabled:?} \
         against {modelled:?}"
    );
}

/// The two credit options this suite drives are the two the service accepts, so neither
/// side can gain a value the other does not know about.
#[tokio::test]
async fn the_credit_options_are_the_documented_pair() {
    let case = "the documented credit options";
    let mut tabled = CPU_CREDITS.to_vec();
    tabled.sort_unstable();
    let mut served = ec2::state::CPU_CREDITS_VALUES.to_vec();
    served.sort_unstable();
    assert_eq!(
        tabled, served,
        "case {case:?}: expected the suite's credit options to be the served pair, got \
         {tabled:?} against {served:?}"
    );
}

// ---------------------------------------------------------------------------
// The report, and the round trip
// ---------------------------------------------------------------------------

/// An account that has never called `ModifyDefaultCreditSpecification` is reported at AWS's
/// own default for each family: `standard` for T2, `unlimited` for the later
/// unlimited-supported families.
///
/// The rows are every family in [`AWS_DEFAULTS`], each on a fresh account, so each row
/// differs along the family the rule generalises over.
#[tokio::test]
async fn a_fresh_account_reports_the_aws_default_for_each_family() {
    for (family, cpu_credits) in AWS_DEFAULTS {
        let case = format!("a fresh account asked for {family}");
        let router = emulator();
        let got = reported(&router, REGION, &case, family).await;
        let want = expected(family, cpu_credits);
        assert_eq!(
            got, want,
            "case {case:?}: expected the AWS default for {family}, got {got:?} against \
             {want:?}"
        );
    }
}

/// The rule: a modify sets the default credit option of the family it names, and the get
/// reads it back — the modify's own response reporting exactly what the get will.
///
/// The rows are every modelled family crossed with both documented credit options, each on
/// a fresh account, and each option applied TWICE so a repeat is shown to answer the same
/// specification rather than toggling anything.
#[tokio::test]
async fn a_modify_sets_the_default_and_the_get_reads_it_back() {
    for (family, _) in AWS_DEFAULTS {
        for cpu_credits in CPU_CREDITS {
            let case = format!("{family}={cpu_credits}");
            let router = emulator();
            let want = expected(family, cpu_credits);

            for attempt in 1..=2 {
                let case = format!("{case}, modify {attempt}");
                let answered = modify(&router, &case, family, cpu_credits).await;
                assert_eq!(
                    answered, want,
                    "case {case:?}: expected the modify to report what it set, got \
                     {answered:?} against {want:?}"
                );
                let got = reported(&router, REGION, &case, family).await;
                assert_eq!(
                    got, want,
                    "case {case:?}: expected the get to read back what the modify set, got \
                     {got:?} against {want:?}"
                );
            }
        }
    }
}

/// A modify names ONE family, so every other family keeps the default it had: the families
/// of a Region are set independently of each other.
///
/// The rows are every family set in turn on one account, to the option that is NOT its AWS
/// default, with every family's report checked after each step.
#[tokio::test]
async fn a_family_the_modify_does_not_name_keeps_its_default() {
    let router = emulator();
    let mut want: BTreeMap<&str, &str> = AWS_DEFAULTS.iter().copied().collect();

    for (family, aws_default) in AWS_DEFAULTS {
        // The option the family does NOT launch with by default, so each step is a change.
        let flipped = CPU_CREDITS
            .iter()
            .copied()
            .find(|option| *option != aws_default)
            .expect("the documented pair holds another option");
        let case = format!("after {family}={flipped}");
        modify(&router, &case, family, flipped).await;
        want.insert(family, flipped);

        for (other, option) in &want {
            let case = format!("{case}, reading {other}");
            let got = reported(&router, REGION, &case, other).await;
            let expected = expected(other, option);
            assert_eq!(
                got, expected,
                "case {case:?}: expected every family set so far to keep its option, got \
                 {got:?} against {expected:?}"
            );
        }
    }
}

/// "The default credit option is set at the account level per Amazon Web Services Region":
/// a modify signed for one Region leaves every other Region's default where it was.
///
/// The rows are the Regions, each given a different option for one family, with every
/// Region's report checked after each step — so the setting is shown to be keyed by the
/// Region of the request rather than held once for the account.
#[tokio::test]
async fn a_modify_sets_the_default_only_in_the_region_it_was_signed_for() {
    /// The family this case drives, and the option it does not launch with by default.
    const FAMILY: &str = "t3";

    let router = emulator();
    let aws_default = AWS_DEFAULTS
        .iter()
        .find(|(family, _)| *family == FAMILY)
        .map(|(_, option)| *option)
        .expect("the family table holds this family");
    let flipped = CPU_CREDITS
        .iter()
        .copied()
        .find(|option| *option != aws_default)
        .expect("the documented pair holds another option");

    let mut want: BTreeMap<&str, &str> = [REGION]
        .into_iter()
        .chain(OTHER_REGIONS)
        .map(|region| (region, aws_default))
        .collect();

    for region in [REGION].into_iter().chain(OTHER_REGIONS) {
        let case = format!("after {FAMILY}={flipped} in {region}");
        modify_in(&router, region, &case, FAMILY, flipped).await;
        want.insert(region, flipped);

        for (other, option) in &want {
            let case = format!("{case}, reading {other}");
            let got = reported(&router, other, &case, FAMILY).await;
            let expected = expected(FAMILY, option);
            assert_eq!(
                got, expected,
                "case {case:?}: expected each Region to keep its own default, got {got:?} \
                 against {expected:?}"
            );
        }
    }
}

/// `GetDefaultCreditSpecification` is a REPORT, so reading cannot change what a later
/// caller sees: a run of gets with no modify between them all report the same option.
///
/// The rows are a fresh account and a modified one, so the invariant is checked from AWS's
/// own default as well as from an option the account set.
#[tokio::test]
async fn reading_the_default_never_changes_it() {
    /// Gets per case — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    /// The family this case drives.
    const FAMILY: &str = "t4g";

    let fresh = emulator();
    let modified = emulator();
    modify(&modified, "the modified account", FAMILY, "standard").await;

    for (case, router, option) in [
        ("repeated reads of a fresh account", &fresh, "unlimited"),
        (
            "repeated reads of a modified account",
            &modified,
            "standard",
        ),
    ] {
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            let got = reported(router, REGION, &case, FAMILY).await;
            let want = expected(FAMILY, option);
            assert_eq!(
                got, want,
                "case {case:?}: expected the same option on every read, got {got:?} against \
                 {want:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// The modify takes only the two documented credit options: "Valid Values: `standard` |
/// `unlimited`" (`ModifyDefaultCreditSpecificationRequest$CpuCredits`). Anything else is
/// refused with EC2's `InvalidParameterValue` and the sentence EC2 words for that parameter
/// — which names the parameter and the values it requires and quotes no value — and writes
/// nothing.
///
/// The rows differ along what makes the value bad: a word of the right shape that is not an
/// option, a documented option in the wrong case, one with surrounding space, a number, and
/// the empty string. Each is checked against an account whose family default was already
/// set, so "nothing was written" is a claim about a stored option.
#[tokio::test]
async fn a_cpu_credits_outside_the_documented_pair_is_refused() {
    /// The family these rows drive, and the option it is seeded with.
    const FAMILY: &str = "t3a";
    const SEEDED: &str = "standard";

    /// The sentence EC2 answers a refused `CpuCredits` with: it names the parameter and the
    /// two values it requires, and quotes no value at fault, so every refused value is
    /// answered with exactly this text.
    const MESSAGE: &str = "The CpuCredits parameter requires a value of either Standard or \
                           Unlimited. Change the value and try again.";

    // (case, the CpuCredits the request carries)
    let cases: [(&str, &str); 7] = [
        ("a word that is not an option", "boost"),
        ("a word that names no option at all", "none"),
        ("an option in title case", "Standard"),
        ("an option in upper case", "UNLIMITED"),
        ("an option with trailing space", "unlimited "),
        ("a number", "0"),
        ("the empty string", ""),
    ];

    for (case, cpu_credits) in cases {
        let router = emulator();
        modify(&router, case, FAMILY, SEEDED).await;

        let (status, body) = call(
            &router,
            "ModifyDefaultCreditSpecification",
            &[
                param("InstanceFamily", FAMILY),
                param("CpuCredits", cpu_credits),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for CpuCredits {cpu_credits:?}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected error code {:?} for CpuCredits {cpu_credits:?}, got \
             {code:?} with body {body}",
            ec2::state::INVALID_PARAMETER_VALUE
        );
        let message = error_message(&body);
        assert_eq!(
            message, MESSAGE,
            "case {case:?}: expected the message {MESSAGE:?} for CpuCredits \
             {cpu_credits:?}, got {message:?} with body {body}"
        );

        let got = reported(&router, REGION, case, FAMILY).await;
        let expected = expected(FAMILY, SEEDED);
        assert_eq!(
            got, expected,
            "case {case:?}: a refused modify must write nothing, got {got:?} against \
             {expected:?}"
        );
    }
}

/// An instance family outside the modelled `UnlimitedSupportedInstanceFamily` catalog is
/// refused with EC2's own `InvalidInstanceFamily` and the sentence EC2 words for it, naming
/// the family at fault — never the boundary's generic validation code, and never a 501.
///
/// The rows differ along what makes the family bad — a non-burstable family, a burstable
/// generation the catalog does not hold, a catalog member in the wrong case, a member with
/// surrounding space and the empty string — crossed with both operations that carry the
/// member, since the refusal belongs to the parameter type rather than to one operation.
#[tokio::test]
async fn a_family_outside_the_burstable_catalog_is_refused() {
    /// The sentence EC2 answers a refused `InstanceFamily` with: the family at fault, that
    /// the credit option the request is about is for burstable performance families only,
    /// and what to do about it — so the only part that varies between rows is the family
    /// the request named.
    fn message(family: &str) -> String {
        format!(
            "The {family} instance family does not support Unlimited. Only burstable \
             performance instance families support Unlimited. Change the instance family \
             and try again."
        )
    }

    // (case, the InstanceFamily the request carries)
    let families: [(&str, &str); 6] = [
        ("a non-burstable family", "m5"),
        ("an older burstable generation", "t1"),
        ("a family the catalog does not hold", "t5"),
        ("a catalog member in upper case", "T3"),
        ("a catalog member with trailing space", "t3 "),
        ("the empty string", ""),
    ];

    for (case, family) in families {
        for action in [
            "GetDefaultCreditSpecification",
            "ModifyDefaultCreditSpecification",
        ] {
            let case = format!("{case} on {action}");
            let router = emulator();
            let mut params = vec![param("InstanceFamily", family)];
            if action == "ModifyDefaultCreditSpecification" {
                params.push(param("CpuCredits", "standard"));
            }

            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for InstanceFamily {family:?}, got \
                 {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code,
                ec2::state::INVALID_INSTANCE_FAMILY,
                "case {case:?}: expected error code {:?} for InstanceFamily {family:?}, \
                 got {code:?} with body {body}",
                ec2::state::INVALID_INSTANCE_FAMILY
            );
            let reported = error_message(&body);
            let want = message(family);
            assert_eq!(
                reported, want,
                "case {case:?}: expected the message {want:?} for InstanceFamily \
                 {family:?}, got {reported:?} with body {body}"
            );
        }
    }
}

/// Every other refusal these two operations make, each with the error code and HTTP status
/// it answers — and none of them writes anything.
///
/// The rows differ along what makes the request bad: a family outside the modelled catalog,
/// on each operation, which is EC2's `InvalidInstanceFamily`; a required member the request
/// leaves out altogether, which the boundary refuses as malformed input before the handler
/// runs; a dry run of each operation, which "would have succeeded" and so is refused with
/// `DryRunOperation`; and a dry run whose credit option is refused anyway, which is answered
/// with THAT reason rather than the dry run's. Every one is a 400 or a 412 — never a 501,
/// which would say the operation is unimplemented.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_error_code_for_it() {
    /// The family the account is seeded with, so "nothing was written" is a claim about a
    /// stored option.
    const FAMILY: &str = "t2";
    const SEEDED: &str = "unlimited";

    let router = emulator();
    modify(&router, "the refusal table's starting state", FAMILY, SEEDED).await;

    // (case, the action, its form fields, the HTTP status, the error code)
    let cases: [(&str, &str, Vec<Param>, u16, &str); 7] = [
        (
            "a family the catalog does not hold, on the get",
            "GetDefaultCreditSpecification",
            vec![param("InstanceFamily", "t1")],
            400,
            ec2::state::INVALID_INSTANCE_FAMILY,
        ),
        (
            "a family the catalog does not hold, on the modify",
            "ModifyDefaultCreditSpecification",
            vec![
                param("InstanceFamily", "m5"),
                param("CpuCredits", "standard"),
            ],
            400,
            ec2::state::INVALID_INSTANCE_FAMILY,
        ),
        (
            "a get that names no family at all",
            "GetDefaultCreditSpecification",
            Vec::new(),
            400,
            "MalformedInput",
        ),
        (
            "a modify that names no credit option at all",
            "ModifyDefaultCreditSpecification",
            vec![param("InstanceFamily", FAMILY)],
            400,
            "MalformedInput",
        ),
        (
            "a dry run of a get that would have succeeded",
            "GetDefaultCreditSpecification",
            vec![param("InstanceFamily", FAMILY), param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a modify that would have succeeded",
            "ModifyDefaultCreditSpecification",
            vec![
                param("InstanceFamily", FAMILY),
                param("CpuCredits", "standard"),
                param("DryRun", "true"),
            ],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a modify whose credit option is refused",
            "ModifyDefaultCreditSpecification",
            vec![
                param("InstanceFamily", FAMILY),
                param("CpuCredits", "boost"),
                param("DryRun", "true"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
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
        // Each refusal leaves the family's default exactly where it was: the next row
        // starts from the same account this one did.
        let got = reported(&router, REGION, case, FAMILY).await;
        let want = expected(FAMILY, SEEDED);
        assert_eq!(
            got, want,
            "case {case:?}: a refused request must write nothing, got {got:?} against \
             {want:?}"
        );
    }
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally.
///
/// The rows are the writer and the reader, so the flag's off value is exercised on both
/// operations that read it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    /// The family this case drives, and the option it is set to.
    const FAMILY: &str = "t8i";
    const OPTION: &str = "standard";

    let router = emulator();
    let case = "a modify with DryRun=false";
    let (status, body) = call(
        &router,
        "ModifyDefaultCreditSpecification",
        &[
            param("InstanceFamily", FAMILY),
            param("CpuCredits", OPTION),
            param("DryRun", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyDefaultCreditSpecification, got \
         {status} with body {body}"
    );

    let case = "a get with DryRun=false";
    let (status, body) = call(
        &router,
        "GetDefaultCreditSpecification",
        &[param("InstanceFamily", FAMILY), param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetDefaultCreditSpecification, got {status} \
         with body {body}"
    );
    let got = specification(case, &body);
    let want = expected(FAMILY, OPTION);
    assert_eq!(
        got, want,
        "case {case:?}: expected the modify to have run, got {got:?} against {want:?}"
    );

    // The family is reported with the option the modify set, and the response names the
    // family it was asked about rather than any other.
    let family = child_text(
        child(document(case, &body).root_element(), "instanceFamilyCreditSpecification")
            .expect("the response carries the specification"),
        "instanceFamily",
    );
    assert_eq!(
        family.as_deref(),
        Some(FAMILY),
        "case {case:?}: expected the report to name {FAMILY}, got {family:?} with body \
         {body}"
    );
}
