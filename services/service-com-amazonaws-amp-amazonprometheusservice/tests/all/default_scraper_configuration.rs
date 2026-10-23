// SPDX-License-Identifier: CC0-1.0

//! `GetDefaultScraperConfiguration` over HTTP: the blob it serves is ONE YAML document
//! holding the published Prometheus scrape configuration, and serving it changes nothing
//! a later caller sees.
//!
//! The suite asserts the document's STRUCTURE — the keys a Prometheus scraper reads and
//! the vocabularies Prometheus documents for them — over every entry the document
//! carries, rather than comparing bytes against the handler's own constant (which would
//! assert nothing). The YAML is parsed with `yaml-rust2`, the parser two other crates in
//! this workspace already use, so "valid YAML" is a parser's verdict and the nested
//! members (`tls_config.ca_file`, `authorization.credentials_file`) are read as the
//! nested members they are.
//!
//! The wire harness is the one [`crate::workspace_lifecycle`] owns ([`Amp`], [`Reply`]),
//! shared rather than copied: a second copy would be a second set of rules about how a
//! test reaches the emulator.
//!
//! One known divergence from the AWS published document, stated here as well as on
//! `DEFAULT_SCRAPER_CONFIGURATION` so it is visible from either side: the served blob
//! omits the three members that name the kubelet's in-cluster secret files
//! (`authorization.credentials_file` and `bearer_token_file`, both the service-account
//! token, and `tls_config.ca_file`), because emulator source may not reference a host
//! secret path. A consumer that feeds this blob to a real in-cluster Prometheus supplies
//! those three itself. [`exactly_the_documented_members_are_absent_from_the_served_document`]
//! pins exactly that gap, and goes away with the omission if the constraint is lifted.

use std::collections::BTreeSet;

use yaml_rust2::{Yaml, YamlLoader};

use crate::workspace_lifecycle::{Amp, Reply};

/// The region every call here is signed for; the operation takes no input and reports no
/// ARN, so the region only has to be one the harness can sign for.
const REGION: &str = "us-east-1";

/// `GET /scraperconfiguration` — the operation's modelled HTTP binding.
async fn default_scraper_configuration(amp: &Amp) -> Reply {
    amp.call("GET", "/scraperconfiguration", REGION, "").await
}

/// The `configuration` blob of a successful reply, decoded from the base64 the restJson1
/// serializer renders a `Blob` as.
fn configuration_bytes(reply: &Reply) -> Vec<u8> {
    assert_eq!(
        reply.status, 200,
        "GetDefaultScraperConfiguration must answer 200, got {reply:?}"
    );
    let encoded = reply.body["configuration"].as_str().unwrap_or_else(|| {
        panic!("the reply must carry a base64 `configuration` string, got {reply:?}")
    });
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("`configuration` must be base64 ({error}): {encoded:?}"))
}

/// The configuration as text: the blob's bytes are UTF-8 YAML.
fn configuration_text(reply: &Reply) -> String {
    String::from_utf8(configuration_bytes(reply))
        .unwrap_or_else(|error| panic!("`configuration` must be UTF-8 text ({error})"))
}

/// The single YAML document the configuration holds.
fn configuration_document(text: &str) -> Yaml {
    let mut documents = YamlLoader::load_from_str(text)
        .unwrap_or_else(|error| panic!("`configuration` must parse as YAML ({error}): {text}"));
    assert_eq!(
        documents.len(),
        1,
        "the configuration must be ONE YAML document, got {}",
        documents.len()
    );
    documents.remove(0)
}

/// Every `scrape_configs` entry of the configuration.
fn scrape_configs(document: &Yaml) -> Vec<Yaml> {
    document["scrape_configs"]
        .as_vec()
        .unwrap_or_else(|| panic!("`scrape_configs` must be a sequence, got {document:?}"))
        .clone()
}

/// The scrape job named `job_name`.
fn job_named(jobs: &[Yaml], job_name: &str) -> Yaml {
    jobs.iter()
        .find(|job| job["job_name"].as_str() == Some(job_name))
        .unwrap_or_else(|| panic!("case {job_name:?}: the job must be declared, got {jobs:?}"))
        .clone()
}

/// The value at `path` within `job`, walking nested mappings (`["tls_config", "ca_file"]`
/// reads the nested member, not a flattened view of the job).
fn member_at<'a>(job: &'a Yaml, path: &[&str]) -> &'a Yaml {
    let mut value = job;
    for key in path {
        value = &value[*key];
    }
    value
}

/// The Kubernetes service-discovery roles a scrape job's `kubernetes_sd_configs`
/// declares.
fn sd_roles(job: &Yaml) -> Vec<String> {
    job["kubernetes_sd_configs"]
        .as_vec()
        .unwrap_or_else(|| panic!("a job's `kubernetes_sd_configs` must be a sequence: {job:?}"))
        .iter()
        .map(|sd| {
            sd["role"]
                .as_str()
                .unwrap_or_else(|| panic!("a service-discovery entry must carry a `role`: {sd:?}"))
                .to_string()
        })
        .collect()
}

/// The configuration is one YAML document carrying the two top-level keys a Prometheus
/// configuration file is built from, and a scrape interval in Prometheus's duration form.
#[tokio::test]
async fn the_default_configuration_is_one_prometheus_yaml_document() {
    let amp = Amp::new();
    let text = configuration_text(&default_scraper_configuration(&amp).await);
    let document = configuration_document(&text);

    assert!(
        document.as_hash().is_some(),
        "the document's root must be a mapping, got {document:?}"
    );
    for key in ["global", "scrape_configs"] {
        assert!(
            !document[key].is_badvalue(),
            "case {key:?}: the configuration must carry this top-level key, got {document:?}"
        );
    }

    let interval = document["global"]["scrape_interval"]
        .as_str()
        .unwrap_or_else(|| panic!("`global.scrape_interval` must be a scalar: {document:?}"));
    // Prometheus duration grammar: a number followed by one of these units
    // (https://prometheus.io/docs/prometheus/latest/configuration/configuration/#configuration-file).
    let units = ["ms", "s", "m", "h", "d", "w", "y"];
    let digits = interval
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(interval.len());
    let (amount, unit) = interval.split_at(digits);
    assert!(
        !amount.is_empty(),
        "`global.scrape_interval` must start with a count, got {interval:?}"
    );
    assert!(
        units.contains(&unit),
        "`global.scrape_interval` must end in one Prometheus duration unit of {units:?}, \
         got {interval:?}"
    );

    assert!(
        !scrape_configs(&document).is_empty(),
        "the configuration must declare at least one scrape job, got {document:?}"
    );
}

/// The scrape jobs the published default configuration declares, each with the Kubernetes
/// service-discovery role it scrapes. The table spans the document's jobs: pod metrics,
/// container metrics (cAdvisor on the node), the API server's endpoints and kube-proxy's
/// pods — four jobs over three distinct roles.
#[tokio::test]
async fn every_published_scrape_job_is_present_with_its_discovery_role() {
    // (job_name, the roles its `kubernetes_sd_configs` declares)
    let table: &[(&str, &[&str])] = &[
        ("pod_exporter", &["pod"]),
        ("cadvisor", &["node"]),
        ("kubernetes-apiservers", &["endpoints"]),
        ("kube-proxy", &["pod"]),
    ];

    let amp = Amp::new();
    let text = configuration_text(&default_scraper_configuration(&amp).await);
    let jobs = scrape_configs(&configuration_document(&text));

    for (job_name, expected_roles) in table {
        let job = job_named(&jobs, job_name);
        let actual = sd_roles(&job);
        assert_eq!(
            actual,
            expected_roles
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "case {job_name:?}: expected service-discovery roles {expected_roles:?}, got {actual:?}"
        );
    }

    assert_eq!(
        jobs.len(),
        table.len(),
        "expected {} scrape jobs, got {}: {jobs:?}",
        table.len(),
        jobs.len()
    );
}

/// Whatever the jobs are, each one satisfies what Prometheus requires of a scrape config:
/// a unique `job_name`, service-discovery roles drawn from the documented set, and
/// relabel rules whose `action` is one Prometheus defines.
#[tokio::test]
async fn every_scrape_job_satisfies_the_prometheus_vocabularies() {
    // The `role` values `kubernetes_sd_config` documents
    // (https://prometheus.io/docs/prometheus/latest/configuration/configuration/#kubernetes_sd_config).
    let roles = [
        "node",
        "service",
        "pod",
        "endpoints",
        "endpointslice",
        "ingress",
    ];
    // The `action` values `relabel_config` documents, same page.
    let actions = [
        "replace",
        "keep",
        "drop",
        "hashmod",
        "labelmap",
        "labeldrop",
        "labelkeep",
        "lowercase",
        "uppercase",
        "keepequal",
        "dropequal",
    ];

    let amp = Amp::new();
    let text = configuration_text(&default_scraper_configuration(&amp).await);
    let jobs = scrape_configs(&configuration_document(&text));

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for job in &jobs {
        let job_name = job["job_name"]
            .as_str()
            .unwrap_or_else(|| panic!("every scrape job must carry a `job_name`: {job:?}"))
            .to_string();
        assert!(
            seen.insert(job_name.clone()),
            "case {job_name:?}: a scrape job name must be unique across the configuration"
        );

        for role in sd_roles(job) {
            assert!(
                roles.contains(&role.as_str()),
                "case {job_name:?}: service-discovery role {role:?} is not one of {roles:?}"
            );
        }

        let Some(relabels) = job["relabel_configs"].as_vec() else {
            continue;
        };
        for relabel in relabels {
            // `action` is optional and defaults to `replace` (same page), so a rule that
            // omits it is a `replace`.
            let action = relabel["action"].as_str().unwrap_or("replace").to_string();
            assert!(
                actions.contains(&action.as_str()),
                "case {job_name:?}: relabel action {action:?} is not one of {actions:?}"
            );
            if !relabel["source_labels"].is_badvalue() {
                let labels = relabel["source_labels"].as_vec().unwrap_or_else(|| {
                    panic!("case {job_name:?}: `source_labels` must be a sequence: {relabel:?}")
                });
                assert!(
                    labels.iter().all(|label| label.as_str().is_some()),
                    "case {job_name:?}: every `source_labels` entry must be a label name: {labels:?}"
                );
            }
        }
    }
}

/// The three members of the published document this emulator deliberately does not serve,
/// pinned per job and per nested path so the divergence cannot widen unnoticed: a later
/// edit either restores them — and deletes this test with the omission — or leaves them
/// out, but cannot drop a fourth member by accident. The reason is on
/// `DEFAULT_SCRAPER_CONFIGURATION` and in this file's header.
#[tokio::test]
async fn exactly_the_documented_members_are_absent_from_the_served_document() {
    // (job_name, the member's path within that job) — absent by design.
    let omitted: &[(&str, &[&str])] = &[
        ("cadvisor", &["authorization", "credentials_file"]),
        ("kubernetes-apiservers", &["bearer_token_file"]),
        ("kubernetes-apiservers", &["tls_config", "ca_file"]),
    ];
    // What the published document carries on those same jobs and this emulator keeps, so
    // the test fails if the gap grows past the three members above.
    let kept: &[(&str, &[&str])] = &[
        ("cadvisor", &["scheme"]),
        ("cadvisor", &["relabel_configs"]),
        ("kubernetes-apiservers", &["scheme"]),
        ("kubernetes-apiservers", &["tls_config"]),
        ("kubernetes-apiservers", &["tls_config", "insecure_skip_verify"]),
        ("kubernetes-apiservers", &["relabel_configs"]),
    ];

    let amp = Amp::new();
    let text = configuration_text(&default_scraper_configuration(&amp).await);
    let jobs = scrape_configs(&configuration_document(&text));

    for (job_name, path) in omitted {
        let job = job_named(&jobs, job_name);
        assert!(
            member_at(&job, path).is_badvalue(),
            "case {job_name:?} {path:?}: this member is absent from the served document \
             by design, got {:?}",
            member_at(&job, path)
        );
    }
    for (job_name, path) in kept {
        let job = job_named(&jobs, job_name);
        assert!(
            !member_at(&job, path).is_badvalue(),
            "case {job_name:?} {path:?}: the published document carries this member and \
             the emulator keeps it, got {job:?}"
        );
    }
}

/// The operation is a read: repeated calls answer the same document, and calling it
/// changes nothing a later caller of another operation sees.
#[tokio::test]
async fn serving_the_configuration_changes_no_state() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "observability").await;

    let before = amp
        .call("GET", &format!("/workspaces/{workspace_id}"), REGION, "")
        .await;
    let listed_before = amp.call("GET", "/workspaces", REGION, "").await;

    let first = configuration_bytes(&default_scraper_configuration(&amp).await);
    for round in 1..=3u32 {
        let again = configuration_bytes(&default_scraper_configuration(&amp).await);
        assert_eq!(
            again, first,
            "case round {round}: every call must serve the same default configuration"
        );
    }

    let after = amp
        .call("GET", &format!("/workspaces/{workspace_id}"), REGION, "")
        .await;
    let listed_after = amp.call("GET", "/workspaces", REGION, "").await;
    assert_eq!(
        after.body["workspace"], before.body["workspace"],
        "reading the default configuration must not change a stored workspace: \
         {before:?} then {after:?}"
    );
    assert_eq!(
        listed_after.body["workspaces"], listed_before.body["workspaces"],
        "reading the default configuration must not change the workspace listing: \
         {listed_before:?} then {listed_after:?}"
    );
}
