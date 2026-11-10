// SPDX-License-Identifier: CC0-1.0

//! The configuration check CATALOGUE this service publishes, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: the prose asserted here is written out independently of
//! the table the handlers project from, so a change to that table fails these rows.
//!
//! RULE under test: the configuration check catalogue is SERVICE-OWNED and fixed — each
//! value of the model's `ConfigurationCheckType` enum has one published `Name` and
//! `Description` (`ConfigurationCheckDefinition$Name` is "the name of the configuration
//! check", `$Description` "a description of what the configuration check validates"), the
//! same text for every account, every Region, every page it is read on and every reader of
//! the catalogue, whatever the account has registered or run.
//!
//! The rows span what the rule generalises over: every check in the model's catalogue (not
//! one cited check), several regional endpoints, a store holding nothing and a store
//! holding a registered application with runs against it, page sizes that put a definition
//! on a page of its own, repeated reads, and the THREE readers of the catalogue — the
//! definitions listing, the reply a `StartConfigurationChecks` call returns, and the record
//! a later `GetConfigurationCheckOperation` reports.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONFIGURATION_CHECK_TYPE_VALUES;

use crate::application_lifecycle::{Emulator, Reply};
use crate::configuration_checks::{read_record, started, string_field};

/// The regional endpoints the catalogue is read at: it is the service's own, so it cannot
/// differ between them, and more than one row is what shows that.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

/// What the service publishes for ONE check: its `ConfigurationCheckType`, its `Name` and
/// its `Description`.
#[derive(Debug)]
struct Published {
    id: &'static str,
    name: &'static str,
    description: &'static str,
}

/// The published catalogue, written out HERE rather than read from the table the handlers
/// project from, so these rows pin the text itself.
///
/// Source: the configuration check catalogue AWS Systems Manager for SAP serves — the
/// `Name`/`Description` every account and Region is answered with for the three
/// `ConfigurationCheckType` values the model declares (`smithy_query(query="shape",
/// shape="ConfigurationCheckType")`: SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03). The model
/// documents the MEMBERS, not their values, so the text is the service's.
const PUBLISHED: &[Published] = &[
    Published {
        id: "SAP_CHECK_01",
        name: "SAP EC2 Instance Type Selection",
        description: "Checks any EC2 Instances Associated with this SAP Application, and \
                      evaluates whether the Instance Type complies with SAP Certification \
                      requirements and that necessary hardware settings are in place.",
    },
    Published {
        id: "SAP_CHECK_02",
        name: "SAP EBS Storage Configuration",
        description: "Checks that the EBS Storage Configuration on Amazon EC2 Instances is \
                      appropriate for the SAP workload.",
    },
    Published {
        id: "SAP_CHECK_03",
        name: "SAP on AWS Pacemaker Configuration",
        description: "Checks whether the Pacemaker Configuration for the SAP Application \
                      follows best practices.",
    },
];

/// The published entry for `check`, or `None` for a check this table states nothing for.
fn published(check: &str) -> Option<&'static Published> {
    PUBLISHED.iter().find(|entry| entry.id == check)
}

/// The `ConfigurationChecks` of a definitions listing, keyed by `Id` -> (`Name`,
/// `Description`).
fn definitions(reply: &Reply) -> BTreeMap<String, (String, String)> {
    reply
        .member("ConfigurationChecks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|definition| {
            (
                string_field(definition, "Id"),
                (
                    string_field(definition, "Name"),
                    string_field(definition, "Description"),
                ),
            )
        })
        .collect()
}

/// Read the whole catalogue at `region`, following every page at `max_results` per page
/// when one is given.
async fn whole_catalogue(
    emulator: &Emulator,
    case: &str,
    region: &str,
    max_results: Option<i32>,
) -> BTreeMap<String, (String, String)> {
    let mut body = json!({});
    if let Some(limit) = max_results {
        body.as_object_mut()
            .expect("the body is a JSON object")
            .insert("MaxResults".to_string(), json!(limit));
    }
    let mut catalogue: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut pages = 0;
    loop {
        let reply = emulator
            .send_in_region(region, "/list-configuration-check-definitions", body.clone())
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {pages}: expected 200 from \
             ListConfigurationCheckDefinitions, got {reply:?}"
        );
        for (id, prose) in definitions(&reply) {
            assert!(
                catalogue.insert(id.clone(), prose).is_none(),
                "case {case:?} page {pages}: {id:?} was reported twice over the pages"
            );
        }
        pages += 1;
        assert!(
            pages <= CONFIGURATION_CHECK_TYPE_VALUES.len() + 1,
            "case {case:?}: the paging must terminate, got {pages} pages holding {} \
             definitions",
            catalogue.len()
        );
        match reply.member("NextToken").and_then(Value::as_str) {
            Some(token) => {
                body.as_object_mut()
                    .expect("the body is a JSON object")
                    .insert("NextToken".to_string(), json!(token));
            }
            None => return catalogue,
        }
    }
}

/// Assert `catalogue` is exactly the published one: one entry per modelled check, each
/// carrying that check's published `Name` and `Description`.
fn assert_published(case: &str, catalogue: &BTreeMap<String, (String, String)>) {
    for check in CONFIGURATION_CHECK_TYPE_VALUES {
        let entry = published(check).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the model declares check {check:?}, which this suite states \
                 no published prose for"
            )
        });
        let reported = catalogue.get(*check).unwrap_or_else(|| {
            panic!("case {case:?}: expected a definition for {check:?}, got {catalogue:?}")
        });
        assert_eq!(
            reported.0, entry.name,
            "case {case:?}: expected Name {:?} for {check:?}, got {:?}",
            entry.name, reported.0
        );
        assert_eq!(
            reported.1, entry.description,
            "case {case:?}: expected Description {:?} for {check:?}, got {:?}",
            entry.description, reported.1
        );
    }
    assert_eq!(
        catalogue.len(),
        CONFIGURATION_CHECK_TYPE_VALUES.len(),
        "case {case:?}: the catalogue holds one entry per modelled check, got {catalogue:?}"
    );
}

/// RULE: the definitions listing reports the published catalogue — at every regional
/// endpoint, on every page size, read twice, and whatever the account holds.
#[tokio::test]
async fn the_definitions_listing_reports_the_published_catalogue() {
    let emulator = Emulator::new();

    // Page sizes: the whole catalogue at once, one definition per page, and a page size
    // larger than the catalogue. A definition carries the same prose wherever it lands.
    let page_sizes: &[Option<i32>] = &[
        None,
        Some(1),
        Some(2),
        Some(CONFIGURATION_CHECK_TYPE_VALUES.len() as i32),
        Some(50),
    ];

    for region in REGIONS {
        for max_results in page_sizes {
            let case = format!("{region} at {max_results:?} per page, nothing registered");
            let catalogue = whole_catalogue(&emulator, &case, region, *max_results).await;
            assert_published(&case, &catalogue);

            // Read again: a catalogue read changes nothing a later caller sees.
            let again = whole_catalogue(&emulator, &case, region, *max_results).await;
            assert_eq!(
                again, catalogue,
                "case {case:?}: two reads of the catalogue must agree, got {again:?}"
            );
        }
    }

    // A store holding a registered application with every check run against it reports
    // the SAME catalogue: what exists to run does not depend on what has run.
    let application_id = "HDB-CATALOGUE";
    emulator.with_application(application_id).await;
    let start = emulator
        .start_checks(json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        start.status, 200,
        "expected 200 from StartConfigurationChecks, got {start:?}"
    );
    for region in REGIONS {
        let case = format!("{region} after a run");
        let catalogue = whole_catalogue(&emulator, &case, region, None).await;
        assert_published(&case, &catalogue);
    }
}

/// RULE: a run of a check reports the catalogue's own `Name` and `Description` for it, so
/// the start's reply and the record a later read reports carry exactly the text the
/// definitions listing publishes.
#[tokio::test]
async fn every_reader_of_the_catalogue_reports_the_same_published_prose() {
    let emulator = Emulator::new();
    let application_id = "HDB-CATALOGUE-READERS";
    emulator.with_application(application_id).await;

    // One start per check, so each row names the check whose prose it asserts.
    for check in CONFIGURATION_CHECK_TYPE_VALUES {
        let entry = published(check).unwrap_or_else(|| {
            panic!("case {check:?}: this suite states no published prose for this check")
        });
        let start = emulator
            .start_checks(json!({
                "ApplicationId": application_id,
                "ConfigurationCheckIds": [check],
            }))
            .await;
        assert_eq!(
            start.status, 200,
            "case {check:?}: expected 200 from StartConfigurationChecks, got {start:?}"
        );
        let records = started(&start);
        assert_eq!(
            records.len(),
            1,
            "case {check:?}: expected one started record, got {start:?}"
        );
        let record = &records[0];
        assert_eq!(
            string_field(record, "ConfigurationCheckName"),
            entry.name,
            "case {check:?}: a start reports the catalogue's Name {:?}, got {record:?}",
            entry.name
        );
        assert_eq!(
            string_field(record, "ConfigurationCheckDescription"),
            entry.description,
            "case {check:?}: a start reports the catalogue's Description {:?}, got \
             {record:?}",
            entry.description
        );

        let operation_id = string_field(record, "Id");
        let read = emulator.get_check(&operation_id).await;
        assert_eq!(
            read.status, 200,
            "case {check:?}: expected 200 from GetConfigurationCheckOperation, got {read:?}"
        );
        let stored = read_record(&read);
        assert_eq!(
            string_field(&stored, "ConfigurationCheckName"),
            entry.name,
            "case {check:?}: a read reports the catalogue's Name {:?}, got {stored:?}",
            entry.name
        );
        assert_eq!(
            string_field(&stored, "ConfigurationCheckDescription"),
            entry.description,
            "case {check:?}: a read reports the catalogue's Description {:?}, got \
             {stored:?}",
            entry.description
        );

        // And the definitions listing publishes that same pair for that same id.
        let case = format!("{check} across readers");
        let catalogue = whole_catalogue(&emulator, &case, REGIONS[0], None).await;
        let listed = catalogue.get(*check).unwrap_or_else(|| {
            panic!("case {case:?}: expected a definition for {check:?}, got {catalogue:?}")
        });
        assert_eq!(
            (listed.0.as_str(), listed.1.as_str()),
            (entry.name, entry.description),
            "case {case:?}: every reader reports the same published prose, got {listed:?}"
        );

        // Let the run reach its terminal status, so the next check is not refused as a
        // conflict with one still in flight.
        crate::configuration_checks::settle().await;
    }
}
