// SPDX-License-Identifier: CC0-1.0

//! What the `GetVersions` catalogue states an ESX version IS, over awsJson1_0.
//!
//! RULE: every ESX version Amazon EVS reports — each `instanceTypeEsxVersions[].esxVersions`
//! entry and each `vcfVersions[].defaultEsxVersion` — is an ESXi BUILD identifier,
//! `ESXi-<vSphere release>-<build number>`, and each EC2 instance type the service provides
//! is offered the whole set of builds it offers rather than a single value; a provided VCF
//! version's default build is one of the builds every instance type that version lists is
//! offered, and a host of that version may be installed from the builds of that version's
//! own release line and from no others.
//!
//! Where the rule comes from (`smithy_query(query="shape",
//! shape="EsxVersion,VcfVersionInfo,InstanceTypeEsxVersionsInfo")`): `EsxVersion` is a
//! string of at most 128 characters with no further modelled constraint,
//! `VcfVersionInfo$defaultEsxVersion` is "The default ESX version for this VCF version. It
//! is based on Broadcom's Bill Of Materials (BOM)" — a BOM pins a BUILD — and
//! `InstanceTypeEsxVersionsInfo$esxVersions` is "The list of ESX versions offered for this
//! instance type", a LIST and not one value. "To find the available ESX versions for a
//! particular VCF version, use the GetVersions action" (`CreateEnvironmentHost` doc) is
//! what ties the two together: what the catalogue states is what a host may be installed
//! from.
//!
//! The rows differ along what the rule generalises over: which EC2 instance type is read,
//! which provided VCF version, and which offered build a host names — every build the
//! catalogue reports, including builds of a release line the environment's VCF version is
//! not part of. `tests/all/versions_catalog.rs` owns the SET of versions and instance
//! types the catalogue reports; this suite owns the form and the span of the ESX versions
//! in it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::environment_lifecycle::{call, emulator, error_type, settle, Emulator};

/// The prefix of every ESXi build identifier Amazon EVS reports.
const BUILD_PREFIX: &str = "ESXi-";

/// `EsxVersion`'s modelled bound: `@length(max: 128)`
/// (`smithy_query(query="shape", shape="EsxVersion")`).
const ESX_VERSION_MAX_LENGTH: usize = 128;

/// The vSphere release an ESXi build identifier of the VCF 5.2 release line names: VCF
/// 5.2's Bill Of Materials is built on VMware ESXi 8.0 Update 3, so an `ESXi-8.…` build is
/// one a VCF 5.2 environment can be installed from and an `ESXi-9.…` build is not.
const VCF_5_2_BUILD_PREFIX: &str = "ESXi-8.";

/// Whether `esx_version` has the form of an ESXi build identifier:
/// `ESXi-<vSphere release>-<build number>`, where the build number is the digits after
/// the last separator and the release is what sits between.
fn is_build_identifier(esx_version: &str) -> bool {
    let Some(rest) = esx_version.strip_prefix(BUILD_PREFIX) else {
        return false;
    };
    let Some((release, build)) = rest.rsplit_once(['-', '.']) else {
        return false;
    };
    !release.is_empty()
        && !build.is_empty()
        && build.bytes().all(|byte| byte.is_ascii_digit())
        && esx_version.chars().count() <= ESX_VERSION_MAX_LENGTH
}

/// The catalogue one `GetVersions` call reports: the provided versions with their default
/// build, and the builds offered per instance type.
struct Catalogue {
    /// `vcfVersion -> defaultEsxVersion`.
    versions: Vec<(String, String)>,
    /// `instanceType -> esxVersions`, in the order the response reported them.
    instance_types: Vec<(String, Vec<String>)>,
}

/// Read the catalogue, failing the test if `GetVersions` does not answer it.
async fn catalogue(router: &Emulator) -> Catalogue {
    let (status, body) = call(router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 200,
        "GetVersions: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let versions = body["vcfVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("GetVersions: no vcfVersions in {body}"))
        .iter()
        .map(|entry| {
            (
                string_at(&body, entry, "vcfVersion"),
                string_at(&body, entry, "defaultEsxVersion"),
            )
        })
        .collect();
    let instance_types = body["instanceTypeEsxVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("GetVersions: no instanceTypeEsxVersions in {body}"))
        .iter()
        .map(|entry| {
            let offered = entry["esxVersions"]
                .as_array()
                .unwrap_or_else(|| panic!("GetVersions: no esxVersions in {entry}"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("GetVersions: an esxVersion that is not a string in {entry}"))
                        .to_string()
                })
                .collect();
            (string_at(&body, entry, "instanceType"), offered)
        })
        .collect();
    Catalogue {
        versions,
        instance_types,
    }
}

/// One `@required` string member of a catalogue entry.
fn string_at(body: &Value, entry: &Value, member: &str) -> String {
    entry[member]
        .as_str()
        .unwrap_or_else(|| panic!("GetVersions: no {member} in {entry} of {body}"))
        .to_string()
}

#[tokio::test]
async fn every_esx_version_the_catalogue_reports_is_an_esxi_build_identifier() {
    let router = emulator();
    let catalogue = catalogue(&router).await;

    // One row per value the catalogue states an ESX version in: every build offered for
    // every instance type, and every provided version's default.
    let mut rows: Vec<(String, String)> = Vec::new();
    for (instance_type, offered) in &catalogue.instance_types {
        for esx_version in offered {
            rows.push((
                format!("the esxVersions of {instance_type}"),
                esx_version.clone(),
            ));
        }
    }
    for (vcf_version, default) in &catalogue.versions {
        rows.push((
            format!("the defaultEsxVersion of {vcf_version}"),
            default.clone(),
        ));
    }
    assert!(
        !rows.is_empty(),
        "case \"the catalogue\": it must state at least one ESX version, got nothing"
    );

    for (case, esx_version) in rows {
        assert!(
            is_build_identifier(&esx_version),
            "case {case:?}: an ESX version Amazon EVS reports is an ESXi build identifier \
             ({BUILD_PREFIX}<release>-<build>) within {ESX_VERSION_MAX_LENGTH} characters, \
             got {esx_version:?}"
        );
    }
}

#[tokio::test]
async fn every_instance_type_is_offered_the_whole_set_of_builds_the_service_offers() {
    let router = emulator();
    let catalogue = catalogue(&router).await;

    // "The list of ESX versions offered for this instance type" is a CHOICE, not one
    // value: each instance type carries the builds the service offers, so the lists
    // agree with each other and none of them collapses to a single entry.
    let (first_type, first_offered) = catalogue
        .instance_types
        .first()
        .expect("the catalogue reports at least one instance type");
    for (instance_type, offered) in &catalogue.instance_types {
        assert!(
            offered.len() > 1,
            "case {instance_type:?}: an instance type is offered the builds the service \
             offers rather than a single value, got {offered:?}"
        );
        let distinct: BTreeSet<&str> = offered.iter().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            offered.len(),
            "case {instance_type:?}: no build may be reported twice, got {offered:?}"
        );
        assert_eq!(
            offered, first_offered,
            "case {instance_type:?}: every instance type Amazon EVS provides is offered the \
             same set of builds as {first_type:?}, expected {first_offered:?}, got {offered:?}"
        );
    }

    // And the consistency the model implies: a provided version's default build is one of
    // the builds every instance type that version lists is offered.
    for (vcf_version, default) in &catalogue.versions {
        for (instance_type, offered) in &catalogue.instance_types {
            assert!(
                offered.contains(default),
                "case {vcf_version:?} on {instance_type:?}: the default build must be among \
                 the builds the instance type is offered, expected {default:?} within \
                 {offered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_host_is_installed_from_the_builds_of_its_environments_release_line() {
    let router = emulator();
    let catalogue = catalogue(&router).await;
    let offered = catalogue
        .instance_types
        .first()
        .map(|(_, offered)| offered.clone())
        .expect("the catalogue reports at least one instance type");

    // "To find the available ESX versions for a particular VCF version, use the
    // GetVersions action": every build the catalogue reports is tried against an
    // environment of every provided VCF version, and the ones that version's own release
    // line carries are the ones a host can be installed from — the others name no build
    // Amazon EVS can install for it and are refused, naming `esxVersion`.
    for (vcf_version, default) in &catalogue.versions {
        let environment = deployed_environment(&router, vcf_version).await;
        for (index, esx_version) in offered.iter().enumerate() {
            let case = format!("{vcf_version} with the build {esx_version}");
            let host_name = format!("esx-{}-{index}", vcf_version.replace('.', "-"));
            let request = json!({
                "environmentId": environment,
                "esxVersion": esx_version,
                "host": {
                    "hostName": host_name,
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                },
            });
            let (status, body) = call(&router, "CreateEnvironmentHost", &request).await;
            let installable = esx_version.starts_with(VCF_5_2_BUILD_PREFIX);
            if installable {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a build of this version's own release line must be \
                     accepted, got {status} ({}) {body}",
                    error_type(&body)
                );
                let stored = stored_esx_version(&router, &environment, &host_name);
                assert_eq!(
                    stored.as_deref(),
                    Some(esx_version.as_str()),
                    "case {case:?}: the host must run the build it named, got {stored:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a build of another release line names none this version \
                     can be installed from, got {status} {body}"
                );
                let names: Vec<String> = body["fieldList"]
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| entry["name"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                assert!(
                    names.iter().any(|name| name == "esxVersion"),
                    "case {case:?}: the refusal must name esxVersion, got {names:?} in {body}"
                );
            }
        }

        // The default build is one of the installable ones: a host that names nothing
        // runs it.
        assert!(
            default.starts_with(VCF_5_2_BUILD_PREFIX),
            "case {vcf_version:?}: the default build belongs to this version's own release \
             line, got {default:?}"
        );
    }
}

/// Create an environment at `vcf_version` and settle the transition the clock owes it, so
/// a host can be added to it; answers its `environmentId`.
async fn deployed_environment(router: &Emulator, vcf_version: &str) -> String {
    let mut request = router.create_request(Some(&format!(
        "builds-{}",
        vcf_version.replace('.', "-").to_lowercase()
    )));
    request["vcfVersion"] = json!(vcf_version);
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {vcf_version:?}: expected HTTP 200 from the create, got {status} ({}) {body}",
        error_type(&body)
    );
    settle(router);
    body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {vcf_version:?}: no environmentId in {body}"))
        .to_string()
}

/// The ESX version a stored host RUNS, which the modelled `Host` shape carries no member
/// for, read out of this service's own store.
fn stored_esx_version(router: &Emulator, environment_id: &str, host_name: &str) -> Option<String> {
    let state = ::service_com_amazonaws_evs_amazonelasticvmwareservice::state::service_state(
        &*router.engine,
    )
    .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .and_then(|environment| environment.hosts.get(host_name))
        .and_then(|host| host.esx_version.clone())
}
