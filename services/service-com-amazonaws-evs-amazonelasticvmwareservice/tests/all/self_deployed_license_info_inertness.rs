// SPDX-License-Identifier: CC0-1.0

//! The license member a self-deployed environment's mode does not support, end to end
//! over awsJson1_0.
//!
//! THE RULE under test: a `CreateEnvironment` member the chosen `vcfVersion` does not
//! support is accepted and reported back as supplied, and NOTHING the service derives
//! reads a fact out of it. `licenseInfo` is "Not supported when `vcfVersion` is
//! `SELF_DEPLOYED`" (`smithy_query(query="doc", shape="CreateEnvironmentRequest",
//! member="licenseInfo")`), and in that mode "Amazon EVS provisions only the VLAN
//! subnets; no hosts are added and no VCF installation is performed … The licenseInfo,
//! hosts, vcfHostnames, siteId, and connectivityInfo parameters are not supported in
//! this mode" (`CreateEnvironment` doc) — so such an environment licenses no VCF
//! software and carries no license key at all:
//!
//! * it is no USER of the keys it was given, so the `KEY_REUSE` check of another
//!   environment carrying one of them — "Verifies that the VCF license key is not used
//!   by another Amazon EVS environment" (`Check$type`) — still passes, and neither
//!   `Environment$environmentStatus` nor `EnvironmentSummary$environmentStatus` rolls up
//!   an impairment;
//! * its OWN `KEY_REUSE` and `KEY_COVERAGE` are `UNKNOWN`, the result of a check whose
//!   facts the environment does not carry, rather than a verification performed over a
//!   member the mode does nothing with.
//!
//! The facts the mode DOES carry are unaffected: hosts added with
//! `CreateEnvironmentHost` ("After the environment is created, you can add hosts with
//! CreateEnvironmentHost", same doc) are real hosts, so `HOST_COUNT` decides on them.
//!
//! The rows of the first table differ along what the rule generalises over: which of the
//! two license members carries the shared key, whether the member is supplied at all,
//! the `EnvironmentState` the second environment has reached, and — as the controls that
//! keep the check itself honest — each installed `VcfVersion` the model declares
//! (`smithy_query(query="shape", shape="VcfVersion")`), for which the very same request
//! does make the key shared. The second table runs the account's ported core count over
//! both sides of the `KEY_COVERAGE` comparison and its exact boundary.
//!
//! The harness (`emulator`, `settle`) is the one `environment_lifecycle` owns, the
//! create and host helpers are `environment_health_checks`' and the check readers are
//! `environment_check_input_recompute`'s, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_check_input_recompute::{
    cores, get_environment, listed_summary, reported_check_results, result_of, INSTANCE_TYPE,
};
use crate::environment_health_checks::{
    add_host, create_with, hosts_json, license_info, put_ported_cores,
};
use crate::environment_lifecycle::{emulator, settle};

/// The `CheckResult` values, written out from the model's own enumeration
/// (`smithy_query(query="shape", shape="CheckResult")`), so this suite states the results
/// instead of reading them back out of the code under test.
const PASSED: &str = "PASSED";
/// See [`PASSED`].
const FAILED: &str = "FAILED";
/// See [`PASSED`].
const UNKNOWN: &str = "UNKNOWN";

/// The `VcfVersion` values, from the model's own enumeration
/// (`smithy_query(query="shape", shape="VcfVersion")`): the one mode in which Amazon EVS
/// installs no VCF, and the two versions it installs.
const SELF_DEPLOYED: &str = "SELF_DEPLOYED";
/// See [`SELF_DEPLOYED`].
const INSTALLED_VERSIONS: [&str; 2] = ["VCF-5.2.1", "VCF-5.2.2"];

/// The two license keys the environment under test carries, as the seeds
/// [`license_info`] builds a key of the modelled shape from.
const UNDER_TEST_KEYS: (&str, &str) = ("AAAAA", "BBBBB");

/// The `hosts` member of a create request that deploys an environment at exactly the
/// minimum host count (`CreateEnvironmentRequest$hosts` is modelled `@length(4..4)`),
/// with every host named after `prefix` — "DNS hostnames for hosts must be unique across
/// Amazon EVS environments" (`HostInfoForCreate$hostName` doc).
fn minimum_hosts(prefix: &str) -> Value {
    let names: Vec<String> = (0..evs::MINIMUM_HOST_COUNT)
        .map(|ordinal| format!("{prefix}-{ordinal}"))
        .collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    hosts_json(&borrowed, INSTANCE_TYPE)
}

/// The `environmentStatus` of a reported environment or summary.
fn status_of(reported: &Value) -> String {
    reported["environmentStatus"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

/// One row of the carrier table: the environment created BESIDE the one under test — the
/// `vcfVersion` it is created with, the license keys its `licenseInfo` carries (or `None`
/// for a request that does not carry the member at all), and whether the transition its
/// create owes it is settled before anything is read — together with the `KEY_REUSE`
/// result the environment under test then reports, and the one that second environment
/// reports for itself (`None` where it is still being created, so no check of it has
/// run).
struct CarrierRow {
    case: &'static str,
    vcf_version: &'static str,
    keys: Option<(&'static str, &'static str)>,
    settle_beside: bool,
    expected: &'static str,
    expected_beside: Option<&'static str>,
}

#[tokio::test]
async fn a_self_deployed_environment_carries_none_of_the_license_keys_it_was_given() {
    let rows = vec![
        CarrierRow {
            case: "a self-deployed environment carrying both of the keys",
            vcf_version: SELF_DEPLOYED,
            keys: Some(UNDER_TEST_KEYS),
            settle_beside: true,
            expected: PASSED,
            expected_beside: Some(UNKNOWN),
        },
        CarrierRow {
            case: "a self-deployed environment sharing only the solution key",
            vcf_version: SELF_DEPLOYED,
            keys: Some(("AAAAA", "CCCCC")),
            settle_beside: true,
            expected: PASSED,
            expected_beside: Some(UNKNOWN),
        },
        CarrierRow {
            case: "a self-deployed environment carrying the shared key as its vSAN key",
            vcf_version: SELF_DEPLOYED,
            keys: Some(("DDDDD", "AAAAA")),
            settle_beside: true,
            expected: PASSED,
            expected_beside: Some(UNKNOWN),
        },
        CarrierRow {
            case: "a self-deployed environment that is still being created",
            vcf_version: SELF_DEPLOYED,
            keys: Some(UNDER_TEST_KEYS),
            settle_beside: false,
            expected: PASSED,
            expected_beside: None,
        },
        CarrierRow {
            case: "a self-deployed environment carrying no license member at all",
            vcf_version: SELF_DEPLOYED,
            keys: None,
            settle_beside: true,
            expected: PASSED,
            expected_beside: Some(UNKNOWN),
        },
        // The controls: the SAME request in a mode that DOES install VCF shares the key,
        // so the check still reports reuse where there is reuse.
        CarrierRow {
            case: "an installed environment carrying both of the keys",
            vcf_version: INSTALLED_VERSIONS[0],
            keys: Some(UNDER_TEST_KEYS),
            settle_beside: true,
            expected: FAILED,
            expected_beside: Some(FAILED),
        },
        CarrierRow {
            case: "an installed environment of the other version sharing only the vSAN key",
            vcf_version: INSTALLED_VERSIONS[1],
            keys: Some(("EEEEE", "BBBBB")),
            settle_beside: true,
            expected: FAILED,
            expected_beside: Some(FAILED),
        },
        CarrierRow {
            case: "an installed environment that is still being created",
            vcf_version: INSTALLED_VERSIONS[0],
            keys: Some(UNDER_TEST_KEYS),
            settle_beside: false,
            expected: FAILED,
            expected_beside: None,
        },
        CarrierRow {
            case: "an installed environment carrying keys of its own",
            vcf_version: INSTALLED_VERSIONS[0],
            keys: Some(("FFFFF", "GGGGG")),
            settle_beside: true,
            expected: PASSED,
            expected_beside: Some(PASSED),
        },
    ];

    for row in rows {
        let CarrierRow {
            case,
            vcf_version,
            keys,
            settle_beside,
            expected,
            expected_beside,
        } = row;
        // One store per row, so each row's keys are compared against that row's
        // environments only.
        let router = emulator();
        // The environment under test: an installed one, deployed at the minimum host
        // count so its other checks pass and the roll-up follows `KEY_REUSE` alone.
        let under_test = create_with(
            &router,
            case,
            "under-test",
            json!({
                "licenseInfo": license_info(UNDER_TEST_KEYS.0, UNDER_TEST_KEYS.1),
                "hosts": minimum_hosts("under-test"),
            }),
        )
        .await;
        settle(&router);

        let mut extra = json!({ "vcfVersion": vcf_version });
        if let Some((solution, vsan)) = keys {
            extra["licenseInfo"] = license_info(solution, vsan);
        }
        let beside = create_with(&router, case, "beside", extra).await;
        if settle_beside {
            settle(&router);
        }

        let reported = get_environment(&router, case, &under_test).await;
        let checks = reported_check_results(&reported);
        let actual = result_of(&checks, evs::CHECK_TYPE_KEY_REUSE);
        assert_eq!(
            actual, expected,
            "case {case:?}: beside an environment with {keys:?} on {vcf_version}: expected \
             the environment under test to report KEY_REUSE {expected}, got {actual} in \
             {checks:?}"
        );
        // The roll-up both reporting shapes carry: an environment whose checks all
        // passed or were undecidable is not impaired.
        let expected_status = if expected == FAILED { FAILED } else { PASSED };
        let status = status_of(&reported);
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected environmentStatus {expected_status} over {checks:?}, got \
             {status}"
        );
        let summary = listed_summary(&router, case, &under_test).await;
        let summary_status = status_of(&summary);
        assert_eq!(
            summary_status, expected_status,
            "case {case:?}: expected the listed summary to report environmentStatus \
             {expected_status}, got {summary_status} in {summary}"
        );
        // An impairment is dated, so a check that is not failing carries no
        // `impairedSince` at all.
        let impaired = reported["checks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|check| check["type"] == json!(evs::CHECK_TYPE_KEY_REUSE))
            .map(|check| check["impairedSince"].clone())
            .collect::<Vec<Value>>();
        assert_eq!(
            impaired.len(),
            1,
            "case {case:?}: expected exactly one KEY_REUSE check, got {impaired:?}"
        );
        assert_eq!(
            !impaired[0].is_null(),
            expected == FAILED,
            "case {case:?}: a {expected} KEY_REUSE check carries impairedSince {}",
            impaired[0]
        );

        let beside_reported = get_environment(&router, case, &beside).await;
        // The member is ACCEPTED and reported back exactly as supplied, whatever the
        // mode does with it.
        match keys {
            Some((solution, vsan)) => assert_eq!(
                beside_reported["licenseInfo"],
                license_info(solution, vsan),
                "case {case:?}: expected the licenseInfo supplied to be reported as supplied, \
                 got {}",
                beside_reported["licenseInfo"]
            ),
            None => assert!(
                beside_reported["licenseInfo"].is_null(),
                "case {case:?}: an environment created with no licenseInfo reports none, got {}",
                beside_reported["licenseInfo"]
            ),
        }
        let beside_checks = reported_check_results(&beside_reported);
        match expected_beside {
            Some(expected_beside) => {
                let actual_beside = result_of(&beside_checks, evs::CHECK_TYPE_KEY_REUSE);
                assert_eq!(
                    actual_beside, expected_beside,
                    "case {case:?}: expected the second environment to report KEY_REUSE \
                     {expected_beside} for itself, got {actual_beside} in {beside_checks:?}"
                );
            }
            None => assert!(
                beside_checks.is_empty(),
                "case {case:?}: an environment that is still being created has run no check, \
                 got {beside_checks:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_self_deployed_environments_own_license_checks_are_unknown_whatever_the_account_ported() {
    let minimum_hosts_count =
        u64::try_from(evs::MINIMUM_HOST_COUNT).expect("a host count fits a u64");
    // The cores the hosts of each environment of this table run on, which is what a VCF
    // license has to cover.
    let required = minimum_hosts_count * cores(INSTANCE_TYPE);
    // Rows: the cores the account states it has ported to Amazon EVS — unset, and both
    // sides of the comparison plus its exact boundary — against the `KEY_COVERAGE` an
    // INSTALLED environment with the same hosts and the same keys then reports.
    let rows: Vec<(&str, Option<u64>, &str)> = vec![
        ("no ported core count at all", None, UNKNOWN),
        (
            "one core short of the hosts' cores",
            Some(required - 1),
            FAILED,
        ),
        ("exactly the hosts' cores", Some(required), PASSED),
        ("far above the hosts' cores", Some(required * 4), PASSED),
    ];

    for (case, ported, expected_installed) in rows {
        let router = emulator();
        if let Some(ported) = ported {
            put_ported_cores(&router, case, ported).await;
        }
        // The self-deployed environment: the license member its mode does not support,
        // and the hosts that mode adds — with `CreateEnvironmentHost`, once the
        // environment is created.
        let self_deployed = create_with(
            &router,
            case,
            "self-deployed",
            json!({
                "vcfVersion": SELF_DEPLOYED,
                "licenseInfo": license_info(UNDER_TEST_KEYS.0, UNDER_TEST_KEYS.1),
            }),
        )
        .await;
        settle(&router);
        for ordinal in 0..evs::MINIMUM_HOST_COUNT {
            add_host(
                &router,
                case,
                &self_deployed,
                &format!("self-deployed-{ordinal}"),
                INSTANCE_TYPE,
            )
            .await;
        }
        // The control: an installed environment carrying the same keys and the same
        // hosts, in the same account and Region.
        let installed = create_with(
            &router,
            case,
            "installed",
            json!({
                "licenseInfo": license_info(UNDER_TEST_KEYS.0, UNDER_TEST_KEYS.1),
                "hosts": minimum_hosts("installed"),
            }),
        )
        .await;
        settle(&router);

        let reported = get_environment(&router, case, &self_deployed).await;
        let checks = reported_check_results(&reported);
        for check_type in [evs::CHECK_TYPE_KEY_REUSE, evs::CHECK_TYPE_KEY_COVERAGE] {
            let actual = result_of(&checks, check_type);
            assert_eq!(
                actual, UNKNOWN,
                "case {case:?}: a self-deployed environment carries no license key, so \
                 {check_type} is undecidable: expected {UNKNOWN}, got {actual} in {checks:?}"
            );
        }
        // What the mode DOES carry still decides: the hosts added after the create are
        // real hosts of the environment.
        let host_count = result_of(&checks, evs::CHECK_TYPE_HOST_COUNT);
        assert_eq!(
            host_count, PASSED,
            "case {case:?}: {} hosts added with CreateEnvironmentHost meet the minimum host \
             count: expected {PASSED}, got {host_count} in {checks:?}",
            evs::MINIMUM_HOST_COUNT
        );
        let status = status_of(&reported);
        assert_eq!(
            status, PASSED,
            "case {case:?}: expected environmentStatus {PASSED} over {checks:?}, got {status}"
        );

        let installed_reported = get_environment(&router, case, &installed).await;
        let installed_checks = reported_check_results(&installed_reported);
        let key_coverage = result_of(&installed_checks, evs::CHECK_TYPE_KEY_COVERAGE);
        assert_eq!(
            key_coverage, expected_installed,
            "case {case:?}: {required} core(s) required against {ported:?} ported: expected the \
             installed environment to report KEY_COVERAGE {expected_installed}, got \
             {key_coverage} in {installed_checks:?}"
        );
        let key_reuse = result_of(&installed_checks, evs::CHECK_TYPE_KEY_REUSE);
        assert_eq!(
            key_reuse, PASSED,
            "case {case:?}: the self-deployed environment is no other user of these keys: \
             expected the installed environment to report KEY_REUSE {PASSED}, got {key_reuse} \
             in {installed_checks:?}"
        );
    }
}
