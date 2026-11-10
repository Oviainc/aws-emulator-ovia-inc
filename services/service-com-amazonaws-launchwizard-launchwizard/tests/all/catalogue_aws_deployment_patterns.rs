// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the deployment patterns the service-owned workload catalogue
//! publishes, driven over HTTP through the same router the production emulator serves
//! with.
//!
//! Three rules, each asserted over a TABLE rather than over the one entry the report
//! cited:
//!
//! * a name the catalogue hands a client in one read RESOLVES in every other catalogue
//!   read — the pattern listing, the pattern read, the version listing and the version
//!   read all answer for it, and each reports the same values for the resource they
//!   share;
//! * an output member whose modelled list admits ZERO elements is reported as the empty
//!   list when the service has nothing to put in it (which members those are, and which
//!   projection declares each, is read off `smithy-model/`, so no member is asserted by
//!   hand and none is missed);
//! * a `CreateDeployment` the pattern's published schema cannot accept is refused with
//!   ONE message that states every finding: the settings the pattern does not define,
//!   the deployment-name rule, then one clause per required specification the request
//!   omits, in the order the pattern publishes its schema in.
//!
//! The rows span every workload and every deployment pattern the catalogue holds, which
//! differ in how many specifications they publish, how many of those are required and in
//! which order — the SAP HANA high-availability pattern the report cited is one row of
//! each table.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::create_deployment_finding_report::missing_specification;
use crate::deployment_update::{error_message, every_pattern, pattern_data, schema_fields};
use crate::empty_collection_members::collection_members_where;
use crate::harness::{catalogue, one_page, Harness};
use crate::resource_tagging::model;

/// The route the model binds each catalogue read to.
const PATTERNS_PATH: &str = "/listWorkloadDeploymentPatterns";
/// See [`PATTERNS_PATH`].
const VERSIONS_PATH: &str = "/listDeploymentPatternVersions";
/// See [`PATTERNS_PATH`].
const VERSION_PATH: &str = "/getDeploymentPatternVersion";
/// See [`PATTERNS_PATH`].
const WORKLOAD_PATH: &str = "/getWorkload";
/// See [`PATTERNS_PATH`].
const WORKLOADS_PATH: &str = "/listWorkloads";
/// See [`PATTERNS_PATH`].
const PATTERN_PATH: &str = "/getWorkloadDeploymentPattern";
/// See [`PATTERNS_PATH`].
const CREATE_PATH: &str = "/createDeployment";

/// The wire error code and HTTP status a refused create carries (`ValidationException`,
/// `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// The deployment pattern the report cited, and the specifications Launch Wizard reads
/// for it.
///
/// Source: the SAP deployment specifications AWS documents for a highly available SAP
/// HANA deployment, which the model links from `CreateDeployment$specifications` ("SAP
/// deployment specifications"). This is the set — and the ORDER — the service states as
/// missing when a create for that pattern supplies none of them, so it is both the
/// catalogue's required set and the clause order of the refusal.
const SAP_HANA_HA: &str = "SapHanaHA";
/// See [`SAP_HANA_HA`].
const SAP: &str = "SAP";
/// See [`SAP_HANA_HA`].
const SAP_HANA_HA_REQUIRED: [&str; 17] = [
    "DisableDeploymentRollback",
    "SaveDeploymentArtifacts",
    "KeyPairName",
    "VpcId",
    "CreateSecurityGroup",
    "Timezone",
    "EnableEbsVolumeEncryption",
    "SapSysGroupId",
    "DatabaseSystemId",
    "DatabaseInstanceNumber",
    "InstallDatabaseSoftware",
    "DatabasePrimaryHostname",
    "DatabaseSecondaryHostname",
    "DatabasePrimarySiteName",
    "DatabaseSecondarySiteName",
    "DatabaseVirtualIpAddress",
    "DatabasePacemakerTag",
];

/// A deployment pattern of ANOTHER workload, which the report did not cite: the rules
/// below are about the catalogue rather than about one entry, so each one is driven
/// through this pattern too (and through every other the catalogue holds).
const UNCITED: (&str, &str) = ("SQLServer", "SqlServerHA");

/// The specification names `pattern_data` publishes, in the order it publishes them.
fn published_names(case: &str, data: &Value) -> Vec<String> {
    schema_fields(case, data)
        .iter()
        .map(|field| {
            field["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a schema field carries no name: {field}"))
                .to_string()
        })
        .collect()
}

/// The specification names `pattern_data` marks required, in published order.
fn required_names(case: &str, data: &Value) -> Vec<String> {
    schema_fields(case, data)
        .iter()
        .filter(|field| field["required"].as_str() == Some("Yes"))
        .map(|field| {
            field["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a schema field carries no name: {field}"))
                .to_string()
        })
        .collect()
}

/// The `deploymentPatternVersionName` of a reported version summary.
fn version_name(case: &str, summary: &Value) -> String {
    summary["deploymentPatternVersionName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a version carries no name: {summary}"))
        .to_string()
}

#[tokio::test]
async fn the_catalogue_publishes_the_documented_sap_deployment_patterns() {
    let harness = Harness::new();

    // The pattern the report cited is one the catalogue PUBLISHES: it is in the listing
    // for its workload, so a client discovers it rather than having to know it.
    let case = format!("{SAP} patterns");
    let (summaries, _) = one_page(
        &harness,
        &case,
        PATTERNS_PATH,
        json!({"workloadName": SAP}),
        "workloadDeploymentPatterns",
    )
    .await;
    let listed: Vec<String> = summaries
        .iter()
        .map(|summary| {
            summary["deploymentPatternName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        listed.contains(&SAP_HANA_HA.to_string()),
        "case {case:?}: the catalogue must publish the {SAP_HANA_HA:?} pattern, got {listed:?}"
    );

    // And it publishes the specifications Launch Wizard reads for it, in that order:
    // the schema is the only machine-readable statement of what a create has to carry.
    let case = format!("{SAP}/{SAP_HANA_HA}");
    let data = pattern_data(&harness, &case, SAP, SAP_HANA_HA).await;
    let required = required_names(&case, &data);
    assert_eq!(
        required,
        SAP_HANA_HA_REQUIRED.to_vec(),
        "case {case:?}: expected the documented required specifications in published order, \
         got {required:?}"
    );

    // Every published name is a `DeploymentSpecifications` key (len 3..256,
    // `^[a-zA-Z0-9-:]+$`), so a discovered name can be taken straight to
    // `CreateDeployment`, and no name is published twice.
    let published = published_names(&case, &data);
    let distinct: BTreeSet<&String> = published.iter().collect();
    assert_eq!(
        distinct.len(),
        published.len(),
        "case {case:?}: a specification name must be published once, got {published:?}"
    );
    for name in &published {
        assert!(
            (3..=256).contains(&name.len())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':'),
            "case {case:?}: {name:?} is not a DeploymentSpecifications key"
        );
    }

    // A pattern of another workload, which the report did not cite, publishes a schema
    // of its own: the catalogue tells its patterns apart rather than answering each one
    // with a single canned schema.
    let (workload, pattern) = UNCITED;
    let other = format!("{workload}/{pattern}");
    let other_data = pattern_data(&harness, &other, workload, pattern).await;
    let other_required = required_names(&other, &other_data);
    assert_ne!(
        other_required, required,
        "case {other:?}: two patterns must publish their own required sets, got \
         {other_required:?}"
    );
    assert!(
        !other_required.is_empty(),
        "case {other:?}: this pattern must require the settings it cannot choose for the \
         caller, got {other_required:?}"
    );
}

#[tokio::test]
async fn every_discovered_pattern_resolves_in_every_other_catalogue_read() {
    let harness = Harness::new();

    // One row per (workload, pattern) the catalogue reports — the output of discovery,
    // fed straight back in, which is what a client that round-trips its own discovery
    // does.
    for (workload, pattern) in every_pattern(&harness).await {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        assert_eq!(
            data["deploymentPatternName"].as_str(),
            Some(pattern.as_str()),
            "case {case:?}: the pattern read must answer for the discovered name, got {data}"
        );
        let current = data["deploymentPatternVersionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the pattern reports no version: {data}"))
            .to_string();
        // One catalogue version per entry: the workload version a pattern is published
        // at is the pattern version it reports.
        assert_eq!(
            data["workloadVersionName"].as_str(),
            Some(current.as_str()),
            "case {case:?}: the entry's two version members must agree, got {data}"
        );

        // The version listing answers for the pair, leads with the version the pattern
        // reports, and pages with a token while versions remain.
        let (versions, token) = one_page(
            &harness,
            &case,
            VERSIONS_PATH,
            json!({"workloadName": workload, "deploymentPatternName": pattern}),
            "deploymentPatternVersions",
        )
        .await;
        assert!(
            !versions.is_empty(),
            "case {case:?}: a published pattern must publish at least one version"
        );
        assert_eq!(
            version_name(&case, &versions[0]),
            current,
            "case {case:?}: the version the pattern reports must be the first one listed, got \
             {:?}",
            versions[0]
        );
        assert!(
            token.is_none(),
            "case {case:?}: a page holding every version must hand back no token, got {token:?}"
        );

        // Rows differ along the page size the token rule generalises over: one at a
        // time, and the whole set.
        for size in [1usize, versions.len()] {
            let case = format!("{case} at maxResults {size}");
            let (first, token) = one_page(
                &harness,
                &case,
                VERSIONS_PATH,
                json!({
                    "workloadName": workload,
                    "deploymentPatternName": pattern,
                    "maxResults": size,
                }),
                "deploymentPatternVersions",
            )
            .await;
            assert_eq!(
                first.len(),
                size,
                "case {case:?}: a full page must carry maxResults items, got {}",
                first.len()
            );
            assert_eq!(
                token.is_some(),
                size < versions.len(),
                "case {case:?}: a token must be handed back exactly while versions remain, got \
                 {token:?}"
            );
            if let Some(token) = token {
                // `NextToken` is len 1..2048 and opaque: it is only ever a value a
                // previous page handed back.
                assert!(
                    (1..=2048).contains(&token.len()),
                    "case {case:?}: a continuation token must fit NextToken, got {token:?}"
                );
                let resumed = format!("{case} resumed");
                let (next, _) = one_page(
                    &harness,
                    &resumed,
                    VERSIONS_PATH,
                    json!({
                        "workloadName": workload,
                        "deploymentPatternName": pattern,
                        "maxResults": size,
                        "nextToken": token,
                    }),
                    "deploymentPatternVersions",
                )
                .await;
                assert_eq!(
                    version_name(&resumed, &next[0]),
                    version_name(&resumed, &versions[size]),
                    "case {resumed:?}: a token must resume after the version the page ended on"
                );
            }
        }

        // And every listed version resolves through the version read, which reports it
        // exactly as the listing did: the two views read one record.
        for summary in &versions {
            let name = version_name(&case, summary);
            let case = format!("{case}@{name}");
            let read = harness
                .ok(
                    &case,
                    VERSION_PATH,
                    json!({
                        "workloadName": workload,
                        "deploymentPatternName": pattern,
                        "deploymentPatternVersionName": name,
                    }),
                )
                .await;
            assert_eq!(
                &read["deploymentPatternVersion"], summary,
                "case {case:?}: the version read must report the listed version, got {read}"
            );
        }
    }
}

/// The members of the structure `shape` whose target is a collection that admits ZERO
/// elements: the members of THAT shape for which an empty list IS a value.
///
/// Read off `smithy-model/` — through the same model walk `empty_collection_members.rs`
/// reads the other half of the rule with ([`collection_members_where`]) — so this suite
/// cannot miss a member that gains such a target nor assert the rule for one that loses
/// it. Scoped to one shape, because what each projection has to report is the members
/// ITS OWN output shape declares.
fn open_members_of(model: &Value, shape: &str) -> BTreeSet<String> {
    let open = collection_members_where(model, |min| min == Some(0));
    let id = format!("com.amazonaws.launchwizard#{shape}");
    let declared = model["shapes"][&id]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the model declares the structure {shape}"));
    let members: BTreeSet<String> = declared
        .iter()
        .map(|(name, member)| {
            member["traits"]["smithy.api#jsonName"]
                .as_str()
                .unwrap_or(name)
                .to_string()
        })
        .filter(|name| open.contains(name))
        .collect();
    assert!(
        !members.is_empty(),
        "the model must declare a member admitting the empty list on {shape}, else the rows \
         driven through it assert nothing"
    );
    members
}

/// Assert `reported` carries every member of `expected` AS the empty list: present on
/// the wire, an array, and empty — which is what this catalogue has to report, since no
/// entry of it is account-constrained.
fn reports_as_empty_lists(case: &str, expected: &BTreeSet<String>, reported: &Value) {
    for member in expected {
        assert_eq!(
            reported.get(member.as_str()),
            Some(&json!([])),
            "case {case:?}: member {member:?} admits the empty list, so nothing to report is \
             reported as [] rather than by leaving it out, got {reported}"
        );
    }
}

#[test]
fn the_members_this_suite_sweeps_are_the_models() {
    let model = model();
    let open = collection_members_where(&model, |min| min == Some(0));
    assert!(
        open.contains("accountConstraints"),
        "the model must give accountConstraints a list that admits zero elements, got {open:?}"
    );
    // A member whose target declares a FLOOR of one entry must stay out of the set: an
    // empty list is not a value those members can take, and `empty_collection_members.rs`
    // asserts they are absent instead.
    for member in ["specifications", "conditionals", "tags"] {
        assert!(
            !open.contains(member),
            "case {member:?}: this member's list forbids zero entries, got {open:?}"
        );
    }
    // And every shape the rows below drive the rule through declares such a member, so
    // none of them is asserting nothing.
    for shape in [
        "WorkloadData",
        "WorkloadDataSummary",
        "WorkloadDeploymentPatternData",
        "WorkloadDeploymentPatternDataSummary",
    ] {
        assert!(
            open_members_of(&model, shape).contains("accountConstraints"),
            "case {shape:?}: this projection must declare accountConstraints"
        );
    }
}

#[tokio::test]
async fn a_member_whose_list_admits_zero_elements_is_reported_as_the_empty_list() {
    let harness = Harness::new();
    let model = model();
    // One column per projection that carries such a member, each asked for the members
    // ITS OWN output shape declares.
    let workload_data = open_members_of(&model, "WorkloadData");
    let workload_summary = open_members_of(&model, "WorkloadDataSummary");
    let pattern_data_members = open_members_of(&model, "WorkloadDeploymentPatternData");
    let pattern_summary = open_members_of(&model, "WorkloadDeploymentPatternDataSummary");

    // Every catalogue read, over the WHOLE catalogue: none of these entries is
    // account-constrained, so each view reports the member as `[]`.
    let listing = harness.ok("ListWorkloads", WORKLOADS_PATH, json!({})).await;
    let summaries = listing["workloads"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWorkloads must report a workloads list: {listing}"));
    assert!(
        !summaries.is_empty(),
        "the catalogue must report at least one workload: {listing}"
    );
    for summary in summaries {
        let case = format!("ListWorkloads {}", summary["workloadName"]);
        reports_as_empty_lists(&case, &workload_summary, summary);
    }

    for workload in catalogue(&harness).await {
        let case = format!("GetWorkload {workload}");
        let body = harness
            .ok(&case, WORKLOAD_PATH, json!({"workloadName": workload}))
            .await;
        reports_as_empty_lists(&case, &workload_data, &body["workload"]);

        let case = format!("{PATTERNS_PATH} {workload}");
        let body = harness
            .ok(&case, PATTERNS_PATH, json!({"workloadName": workload}))
            .await;
        let listed = body["workloadDeploymentPatterns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no patterns reported: {body}"));
        assert!(
            !listed.is_empty(),
            "case {case:?}: every catalogue workload publishes a pattern: {body}"
        );
        for summary in listed {
            let case = format!("{case} {}", summary["deploymentPatternName"]);
            reports_as_empty_lists(&case, &pattern_summary, summary);
        }
    }

    for (workload, pattern) in every_pattern(&harness).await {
        let case = format!("{PATTERN_PATH} {workload}/{pattern}");
        let body = harness
            .ok(
                &case,
                PATTERN_PATH,
                json!({"workloadName": workload, "deploymentPatternName": pattern}),
            )
            .await;
        reports_as_empty_lists(&case, &pattern_data_members, &body["workloadDeploymentPattern"]);
    }
}

/// A name the service's deployment-name rule refuses
/// (`state::DEPLOYMENT_NAME_SERVER_PATTERN`: a leading letter, then alphanumerics), and
/// the clause it raises — both stated once here, because every row below carries the
/// same bad name.
const BAD_NAME: &str = "catalogue-row";
/// See [`BAD_NAME`].
const NAME_CLAUSE: &str =
    "The value of 'deploymentName' doesn't match the regular expression pattern \
     <'^[a-zA-Z][a-zA-Z0-9]+$'>.";

/// A specification key no pattern publishes. `DeploymentSpecifications` keys are
/// `KeyString` (len 3..256, `^[a-zA-Z0-9-:]+$`), which this is inside.
const UNKNOWN_KEY: &str = "totally-invalid-spec-key";

#[tokio::test]
async fn a_create_the_published_schema_cannot_accept_states_every_finding_at_once() {
    let harness = Harness::new();

    for (workload, pattern) in every_pattern(&harness).await {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let required = required_names(&case, &data);
        assert!(
            !required.is_empty(),
            "case {case:?}: every pattern requires at least the settings a deployment cannot \
             be created without, got {required:?}"
        );

        // Rows differ along HOW MANY of the required settings the request supplies: none
        // of them, all but the last, and every one — so the clause list the refusal
        // states is the omitted subset rather than a fixed list.
        for supplied in [0usize, required.len() - 1, required.len()] {
            let case = format!("{case} supplying {supplied} of {} settings", required.len());
            let mut specifications: Map<String, Value> = required
                .iter()
                .take(supplied)
                .map(|name| (name.clone(), json!(value_for(&data, name))))
                .collect();
            specifications.insert(UNKNOWN_KEY.to_string(), json!("unexpected-setting-value"));

            let body = harness
                .refused(
                    &case,
                    CREATE_PATH,
                    json!({
                        "workloadName": workload,
                        "deploymentPatternName": pattern,
                        "name": BAD_NAME,
                        "specifications": Value::Object(specifications),
                        "dryRun": false,
                    }),
                    STATUS,
                    CODE,
                )
                .await;

            // The settings the pattern does not define, then the name rule, then one
            // clause per required specification still missing, in published order.
            let mut clauses = vec![
                format!("Specification {UNKNOWN_KEY} is not allowed."),
                NAME_CLAUSE.to_string(),
            ];
            clauses.extend(
                required
                    .iter()
                    .skip(supplied)
                    .map(|name| missing_specification(name)),
            );
            let expected = clauses.join(", ");
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the findings {expected:?}, got {message:?}"
            );
            // `DeploymentSpecifications` is `@sensitive`: no submitted value is replayed.
            assert!(
                !message.contains("unexpected-setting-value"),
                "case {case:?}: a finding must not echo a submitted value, got {message:?}"
            );
        }
    }
}

/// A value the published schema accepts for `name`: one of its `allowedValues` where it
/// publishes any, else any `ValueString`.
fn value_for(data: &Value, name: &str) -> String {
    let allowed = schema_fields("a published schema", data)
        .into_iter()
        .find(|field| field["name"].as_str() == Some(name))
        .and_then(|field| {
            field["allowedValues"]
                .as_array()
                .and_then(|values| values.first())
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
    allowed.unwrap_or_else(|| format!("value-for-{name}"))
}

#[tokio::test]
async fn a_refused_create_stores_nothing_and_a_conforming_one_is_accepted() {
    let harness = Harness::new();
    let case = format!("{SAP}/{SAP_HANA_HA}");
    let data = pattern_data(&harness, &case, SAP, SAP_HANA_HA).await;
    let required = required_names(&case, &data);

    // The settings the pattern asks for, filled from its own published schema — the
    // request a client builds out of discovery alone.
    let specifications: BTreeMap<String, Value> = required
        .iter()
        .map(|name| (name.clone(), json!(value_for(&data, name))))
        .collect();

    // FIRST the same request with one required setting dropped: it is refused, and the
    // store is left exactly as it was — a refused create is not a create.
    let refused_case = format!("{case} missing one required setting");
    let mut incomplete = specifications.clone();
    let dropped = required
        .last()
        .expect("the pattern requires at least one setting")
        .clone();
    incomplete.remove(&dropped);
    harness
        .refused(
            &refused_case,
            CREATE_PATH,
            json!({
                "workloadName": SAP,
                "deploymentPatternName": SAP_HANA_HA,
                "name": "catalogueRow",
                "specifications": incomplete,
                "dryRun": false,
            }),
            STATUS,
            CODE,
        )
        .await;
    let (stored_before, _) = one_page(
        &harness,
        &refused_case,
        "/listDeployments",
        json!({}),
        "deployments",
    )
    .await;
    assert!(
        stored_before.is_empty(),
        "case {refused_case:?}: a refused create must store nothing, got {stored_before:?}"
    );

    let created = harness
        .ok(
            &case,
            CREATE_PATH,
            json!({
                "workloadName": SAP,
                "deploymentPatternName": SAP_HANA_HA,
                "name": "catalogueRow",
                "specifications": specifications,
                "dryRun": false,
            }),
        )
        .await;
    let id = created["deploymentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeployment reported no id: {created}"))
        .to_string();

    // Read back through the implemented reader: the stored record names the pattern it
    // was created against and holds the settings the schema asked for.
    let read = harness
        .ok(&case, "/getDeployment", json!({"deploymentId": id}))
        .await;
    let deployment = &read["deployment"];
    assert_eq!(
        deployment["patternName"].as_str(),
        Some(SAP_HANA_HA),
        "case {case:?}: the stored deployment must name the pattern it deploys, got {deployment}"
    );
    assert_eq!(
        deployment["workloadName"].as_str(),
        Some(SAP),
        "case {case:?}: the stored deployment must name its workload, got {deployment}"
    );
    let stored = deployment["specifications"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the deployment reports no settings: {read}"));
    for (name, value) in &specifications {
        assert_eq!(
            stored.get(name),
            Some(value),
            "case {case:?}: the stored settings must hold {name:?} as submitted, got {:?}",
            stored.get(name)
        );
    }
}
