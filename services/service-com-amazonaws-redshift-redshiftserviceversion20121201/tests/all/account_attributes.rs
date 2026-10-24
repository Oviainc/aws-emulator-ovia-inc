// SPDX-License-Identifier: CC0-1.0

//! The account attributes `DescribeAccountAttributes` reports, end to end over
//! the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns and the nested-element scanner the maintenance-track suite owns rather
//! than keeping second copies of either.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request:
//!
//! * the answer is the service's published account-attribute catalog, each
//!   attribute carrying its own value list, reported in catalog order;
//! * `AttributeNames` SELECTS from that catalog — a request naming none reports
//!   every attribute, a name the account carries no attribute for selects
//!   nothing, and naming one twice still reports it once;
//! * an attribute is an allowance of the account, not a count of what it holds,
//!   so neither a read nor a create moves it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::Emulator;
use crate::maintenance_tracks::elements;

/// The catalog this service publishes, written out here rather than read off the
/// production helper so a wrong entry fails this suite: each attribute's name
/// and the values it carries, in catalog order.
///
/// Source for the entry: `smithy_query(query="doc",
/// shape="ModifyClusterMaintenanceMessage", member="DeferMaintenanceDuration")`
/// — "An integer indicating the duration of the maintenance window in days. …
/// The duration must be 60 days or less." — the one allowance this service
/// publishes for an account, named in the lower-case hyphenated form Amazon
/// Redshift names an account attribute by.
const PUBLISHED_ATTRIBUTES: [(&str, &[&str]); 1] =
    [("max-defer-maintenance-duration", &["60"])];

/// Names the catalog publishes no attribute for, spanning the ways a name can be
/// wrong: wrong case, underscores for hyphens, only a prefix of a published
/// name, one that only extends it, and a name of another shape entirely.
const UNPUBLISHED_ATTRIBUTES: [&str; 5] = [
    "Max-Defer-Maintenance-Duration",
    "max_defer_maintenance_duration",
    "max-defer-maintenance",
    "max-defer-maintenance-durations",
    "no-such-account-attribute",
];

/// Every `AccountAttribute` a `DescribeAccountAttributes` body carries, in the
/// order the answer lists them: each attribute's name and its value list.
///
/// `AttributeList`'s member carries `xmlName: AccountAttribute`, so one element
/// per attribute, and `AttributeValueList`'s carries
/// `xmlName: AttributeValueTarget`, each holding one `AttributeValue`.
fn listed_attributes(xml: &str) -> Vec<(Option<String>, Vec<String>)> {
    elements(xml, "AccountAttribute")
        .into_iter()
        .map(|attribute| {
            let values = elements(&attribute, "AttributeValueTarget")
                .into_iter()
                .filter_map(|target| elements(&target, "AttributeValue").into_iter().next())
                .collect();
            (
                elements(&attribute, "AttributeName").into_iter().next(),
                values,
            )
        })
        .collect()
}

/// The names a body lists, in answer order.
fn listed_names(xml: &str) -> Vec<String> {
    listed_attributes(xml)
        .into_iter()
        .filter_map(|(name, _)| name)
        .collect()
}

/// The whole published catalog as the wire carries it.
fn expected_catalog() -> Vec<(Option<String>, Vec<String>)> {
    PUBLISHED_ATTRIBUTES
        .iter()
        .map(|(name, values)| {
            (
                Some((*name).to_string()),
                values.iter().map(|value| (*value).to_string()).collect(),
            )
        })
        .collect()
}

/// The form parameters that ask for these attribute names: the awsQuery
/// encoding of `AttributeNames`, whose member carries `xmlName: AttributeName`.
fn name_params(names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                format!("AttributeNames.AttributeName.{}", index + 1),
                (*name).to_string(),
            )
        })
        .collect()
}

/// Ask for these attribute names over the wire.
async fn describe(emulator: &Emulator, names: &[&str]) -> crate::cluster_lifecycle::Answer {
    let params = name_params(names);
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("DescribeAccountAttributes", &borrowed)
        .await
}

#[tokio::test]
async fn the_published_account_attributes_are_what_a_read_reports() {
    let emulator = Emulator::new();

    // The catalog the store seeds matches the published set, entry for entry.
    let seeded: Vec<(Option<String>, Vec<String>)> = state::published_account_attributes()
        .into_iter()
        .map(|attribute| {
            (
                attribute.attribute_name,
                attribute
                    .attribute_values
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|target| target.attribute_value)
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        seeded,
        expected_catalog(),
        "case published catalog: expected the documented attribute set, got {seeded:?}"
    );

    // Answered before anything else exists: an account attribute is an
    // allowance the account already has, not something a resource creates.
    let answer = describe(&emulator, &[]).await;
    assert_eq!(
        answer.status, 200,
        "case read an empty emulator: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let listed = listed_attributes(&answer.body);
    assert_eq!(
        listed,
        expected_catalog(),
        "case read an empty emulator: expected {:?}, got {listed:?} from body {}",
        expected_catalog(),
        answer.body
    );

    // A read leaves nothing behind: the next caller sees exactly the same answer.
    let again = describe(&emulator, &[]).await;
    assert_eq!(
        (again.status, listed_attributes(&again.body)),
        (answer.status, listed),
        "case read twice: expected an unchanged catalog, got {} with body {}",
        again.status,
        again.body
    );
}

#[tokio::test]
async fn attribute_names_select_exactly_the_attributes_they_name() {
    let emulator = Emulator::new();

    let published: Vec<&str> = PUBLISHED_ATTRIBUTES.iter().map(|(name, _)| *name).collect();
    let first = published[0];

    // Rows differ along what the rule generalises over: how many names the
    // request carries, whether the catalog publishes each of them, and whether
    // one is repeated.
    let mut rows: Vec<(String, Vec<&str>, Vec<&str>)> = vec![
        ("names none".to_string(), Vec::new(), published.clone()),
        (
            "names every published attribute".to_string(),
            published.clone(),
            published.clone(),
        ),
        (
            "names one attribute twice".to_string(),
            vec![first, first],
            vec![first],
        ),
        (
            "names every unpublished name at once".to_string(),
            UNPUBLISHED_ATTRIBUTES.to_vec(),
            Vec::new(),
        ),
    ];
    for name in &published {
        rows.push((format!("names {name}"), vec![name], vec![name]));
    }
    for name in UNPUBLISHED_ATTRIBUTES {
        rows.push((format!("names unpublished {name}"), vec![name], Vec::new()));
        rows.push((
            format!("names {first} and unpublished {name}"),
            vec![first, name],
            vec![first],
        ));
    }

    for (case, requested, expected) in rows {
        let answer = describe(&emulator, &requested).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let names = listed_names(&answer.body);
        assert_eq!(
            names,
            expected.iter().map(|name| name.to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected {expected:?}, got {names:?} from body {}",
            answer.body
        );

        // Every attribute that came back carries the value list the catalog
        // publishes for it, whichever names the request asked for.
        let reported = listed_attributes(&answer.body);
        let expected_values: Vec<(Option<String>, Vec<String>)> = expected_catalog()
            .into_iter()
            .filter(|(name, _)| {
                name.as_deref()
                    .is_some_and(|name| expected.contains(&name))
            })
            .collect();
        assert_eq!(
            reported, expected_values,
            "case {case:?}: expected {expected_values:?}, got {reported:?} from body {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_attribute_is_an_allowance_not_a_count_of_what_the_account_holds() {
    let emulator = Emulator::new();

    let before = describe(&emulator, &[]).await;
    assert_eq!(
        before.status, 200,
        "case before any cluster: expected 200, got {} with body {}",
        before.status, before.body
    );

    // Each row fills the account up a little more; the reported attributes must
    // not move with what the account holds.
    for identifier in ["attributes-one", "attributes-two", "attributes-three"] {
        let created = emulator.create_minimal(identifier).await;
        assert_eq!(
            created.status, 200,
            "case create {identifier}: expected 200, got {} with body {}",
            created.status, created.body
        );

        let after = describe(&emulator, &[]).await;
        assert_eq!(
            (after.status, listed_attributes(&after.body)),
            (200, listed_attributes(&before.body)),
            "case read after creating {identifier}: expected the catalog unchanged, got {} with body {}",
            after.status,
            after.body
        );
    }

    // The reported deferred-maintenance allowance is the documented maximum of
    // `ModifyClusterMaintenance`'s `DeferMaintenanceDuration`, as a whole number
    // of days — so the value a caller reads is the bound the service publishes,
    // not a string chosen for the response.
    let attributes = listed_attributes(&before.body);
    let (_, values) = attributes
        .iter()
        .find(|(name, _)| {
            name.as_deref() == Some(state::ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION)
        })
        .unwrap_or_else(|| {
            panic!(
                "case deferred-maintenance allowance: expected the attribute to be reported, got {attributes:?}"
            )
        });
    let days: Vec<i32> = values
        .iter()
        .map(|value| {
            value.parse::<i32>().unwrap_or_else(|_| {
                panic!("case deferred-maintenance allowance: expected a day count, got {value:?}")
            })
        })
        .collect();
    assert_eq!(
        days,
        vec![state::MAX_DEFER_MAINTENANCE_DURATION],
        "case deferred-maintenance allowance: expected the documented maximum {}, got {days:?}",
        state::MAX_DEFER_MAINTENANCE_DURATION
    );
}
