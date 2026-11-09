// SPDX-License-Identifier: CC0-1.0

//! What a READ of a preferred-resource preference reports as the recommendation candidates.
//!
//! The rule: `EffectivePreferredResource.effectiveIncludeList` is "the expanded version of
//! your preferred resource's include list" — the include list resolved against the catalog of
//! Amazon EC2 instance types (an include list the request leaves out standing for every
//! candidate, because "if this isn't specified, all supported resources are included by
//! default"), with every instance type the exclude list names taken out of it. It is
//! RESOLVED, never an echo of what was written: that is what lets a client confirm an
//! exclusion took effect, or render which instance types are still candidates.
//!
//! The table spans the forms a preference takes — exact values, a family wildcard, the
//! catch-all, an exclude list on its own, and an include and an exclude list together — and
//! the expectation of each row is computed from the Amazon EC2 model's own `InstanceType`
//! value set, never from one recorded answer. The invariants the rule implies are asserted on
//! every row: every candidate is an instance type the catalog names, and none of them is one
//! the exclude list names.

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use crate::harness::{
    call, effective_for, emulator, get_preferences, instance_arn, opt_in, put_preference, ACCOUNT,
};

/// The catalog a preferred-resource value is resolved against: every Amazon EC2 instance
/// type, from the Amazon EC2 model's own `InstanceType` value set — read from that model
/// rather than written out here, so a type Amazon EC2 adds is covered the day it appears.
fn catalog() -> &'static [&'static str] {
    ec2::ops::types::INSTANCE_TYPE_VALUES
}

/// Every instance type in the catalog whose name begins with one of `prefixes`, in the order
/// the service reports candidates in — the catalog's own order. The empty prefix names every
/// instance type, which is what the catch-all include list expands to.
fn named_by(prefixes: &[&str]) -> Vec<String> {
    let mut named: Vec<String> = catalog()
        .iter()
        .filter(|instance_type| {
            prefixes
                .iter()
                .any(|prefix| instance_type.starts_with(*prefix))
        })
        .map(|instance_type| (*instance_type).to_string())
        .collect();
    named.sort();
    named
}

/// The candidates an `includeList` of `included` and an `excludeList` of `excluded` leave:
/// what the catalog names under the include prefixes, minus what it names under the exclude
/// prefixes.
fn candidates(included: &[&str], excluded: &[&str]) -> Vec<String> {
    let removed = named_by(excluded);
    named_by(included)
        .into_iter()
        .filter(|candidate| !removed.contains(candidate))
        .collect()
}

/// The strings a reported list member holds.
fn reported_list(member: &Value) -> Vec<String> {
    member
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A written preference is read back with its include and exclude lists RESOLVED into the
/// candidate set they leave, through the preference reader and the effective-preference
/// reader alike.
#[tokio::test]
async fn a_preferred_resource_read_reports_the_resolved_candidate_set() {
    // (case, the preferredResources entry written, the candidates it leaves)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "an exclude list naming one exact instance type",
            json!({"name": "Ec2InstanceTypes", "excludeList": ["m6g.large"]}),
            candidates(&[""], &["m6g.large"]),
        ),
        (
            "an exclude list naming a whole family",
            json!({"name": "Ec2InstanceTypes", "excludeList": ["t2"]}),
            candidates(&[""], &["t2"]),
        ),
        (
            "an exclude list naming two families",
            json!({"name": "Ec2InstanceTypes", "excludeList": ["t2", "t3a"]}),
            candidates(&[""], &["t2", "t3a"]),
        ),
        (
            "an include list of exact instance types",
            json!({"name": "Ec2InstanceTypes", "includeList": ["c5.large", "m5.xlarge"]}),
            candidates(&["c5.large", "m5.xlarge"], &[]),
        ),
        (
            "an include list naming a family",
            json!({"name": "Ec2InstanceTypes", "includeList": ["m6g"]}),
            candidates(&["m6g"], &[]),
        ),
        (
            "an include list naming a family with an explicit wildcard",
            json!({"name": "Ec2InstanceTypes", "includeList": ["r7g*"]}),
            candidates(&["r7g"], &[]),
        ),
        (
            "the catch-all include list",
            json!({"name": "Ec2InstanceTypes", "includeList": ["*"]}),
            candidates(&[""], &[]),
        ),
        (
            "an include list and an exclude list together",
            json!({
                "name": "Ec2InstanceTypes",
                "includeList": ["m6g"],
                "excludeList": ["m6g.large"],
            }),
            candidates(&["m6g"], &["m6g.large"]),
        ),
    ];

    let scope = json!({"name": "AccountId", "value": ACCOUNT});

    for (case, entry, wanted) in rows {
        // A fresh emulator per row: what is read back is this row's write, not the last.
        let router = emulator();
        opt_in(&router).await;

        put_preference(
            &router,
            &json!({
                "resourceType": "Ec2Instance",
                "scope": scope.clone(),
                "preferredResources": [entry.clone()],
            }),
        )
        .await;

        let read = get_preferences(&router, "Ec2Instance", Some(&scope)).await;
        let reported = &read["recommendationPreferencesDetails"][0]["preferredResources"][0];
        let effective = reported_list(&reported["effectiveIncludeList"]);
        assert_eq!(
            effective, wanted,
            "case {case:?}: expected {} resolved candidates for {entry}, got {} in {reported}",
            wanted.len(),
            effective.len()
        );

        // The invariants the rule implies, on every row: a candidate is an instance type the
        // catalog names, and no candidate is one the exclude list names.
        let excluded = reported_list(&reported["excludeList"]);
        for candidate in &effective {
            assert!(
                catalog().contains(&candidate.as_str()),
                "case {case:?}: {candidate:?} is not an instance type the catalog names, got \
                 {reported}"
            );
            for exclusion in &excluded {
                assert!(
                    !candidate.starts_with(exclusion.trim_end_matches('*')),
                    "case {case:?}: {candidate:?} is excluded by {exclusion:?}, got {reported}"
                );
            }
        }

        // The stored lists are reported as written beside the resolution, so a client can see
        // both what it asked for and what that asks of the candidates.
        for member in ["includeList", "excludeList"] {
            assert_eq!(
                reported[member],
                entry[member],
                "case {case:?}: expected the stored {member} {} to be read back, got {reported}",
                entry[member]
            );
        }

        // The effective-preference reader resolves the same account-scoped preference the same
        // way, so the two readers cannot disagree about the candidates.
        let resource = instance_arn("i-0abcdef1234567890");
        let in_force = effective_for(&router, &resource).await;
        assert_eq!(
            reported_list(&in_force["preferredResources"][0]["effectiveIncludeList"]),
            wanted,
            "case {case:?}: expected the effective read to resolve the same candidates, got \
             {in_force}"
        );
    }
}

/// The values one list may hold is bounded by what the member publishes — "You can specify
/// up to 1000 values in this list" — so the resolution has a bounded number of values to
/// resolve: the bound itself is accepted and resolves, one past it is refused with the
/// modelled error and nothing is stored.
#[tokio::test]
async fn a_list_holds_at_most_the_values_the_member_publishes() {
    // From `PreferredResource$includeList` / `$excludeList`: "You can specify up to 1000
    // values in this list."
    const PUBLISHED_MAX_VALUES: usize = 1000;
    let scope = json!({"name": "AccountId", "value": ACCOUNT});

    // (case, how many values, the member they are written on, the expected status)
    let rows: Vec<(&str, usize, &str, u16)> = vec![
        ("the published bound on includeList", PUBLISHED_MAX_VALUES, "includeList", 200),
        ("one past the bound on includeList", PUBLISHED_MAX_VALUES + 1, "includeList", 400),
        ("the published bound on excludeList", PUBLISHED_MAX_VALUES, "excludeList", 200),
        ("one past the bound on excludeList", PUBLISHED_MAX_VALUES + 1, "excludeList", 400),
    ];

    for (case, count, member, wanted) in rows {
        let router = emulator();
        opt_in(&router).await;

        // Distinct values, so the row differs from the one before it in COUNT alone.
        let values: Vec<String> = (0..count).map(|at| format!("m5.size{at}")).collect();
        let mut entry = json!({"name": "Ec2InstanceTypes"});
        entry[member] = json!(values);
        let (status, body) = call(
            &router,
            "PutRecommendationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "scope": scope.clone(),
                "preferredResources": [entry],
            }),
        )
        .await;
        assert_eq!(
            status, wanted,
            "case {case:?}: expected {count} values on {member} to answer {wanted}, got \
             {status}: {body}"
        );
        if wanted == 400 {
            assert_eq!(
                body["__type"].as_str(),
                Some("InvalidParameterValueException"),
                "case {case:?}: expected the modelled bad-input error, got {body}"
            );
        }

        let read = get_preferences(&router, "Ec2Instance", Some(&scope)).await;
        let stored = read["recommendationPreferencesDetails"]
            .as_array()
            .map_or(0, Vec::len);
        let expected_records = usize::from(wanted == 200);
        assert_eq!(
            stored, expected_records,
            "case {case:?}: expected {expected_records} stored record(s), got {read}"
        );
    }
}

/// An entry names a preferred resource LIST, so one name holds ONE list: a request naming the
/// same list twice is refused with the modelled error and nothing is stored, which is also
/// what bounds how many candidate sets a later read resolves.
#[tokio::test]
async fn a_request_names_each_preferred_resource_list_once() {
    let scope = json!({"name": "AccountId", "value": ACCOUNT});

    // (case, the entries written, the expected status)
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "one entry",
            json!([{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}]),
            200,
        ),
        (
            "the same list named twice",
            json!([
                {"name": "Ec2InstanceTypes", "includeList": ["m5.large"]},
                {"name": "Ec2InstanceTypes", "excludeList": ["t2.micro"]},
            ]),
            400,
        ),
        (
            "the same list named many times",
            json!([
                {"name": "Ec2InstanceTypes", "includeList": ["*"]},
                {"name": "Ec2InstanceTypes", "includeList": ["*"]},
                {"name": "Ec2InstanceTypes", "includeList": ["*"]},
            ]),
            400,
        ),
    ];

    for (case, entries, wanted) in rows {
        let router = emulator();
        opt_in(&router).await;

        let (status, body) = call(
            &router,
            "PutRecommendationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "scope": scope.clone(),
                "preferredResources": entries.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, wanted,
            "case {case:?}: expected {entries} to answer {wanted}, got {status}: {body}"
        );
        if wanted == 400 {
            assert_eq!(
                body["__type"].as_str(),
                Some("InvalidParameterValueException"),
                "case {case:?}: expected the modelled bad-input error, got {body}"
            );
        }

        let read = get_preferences(&router, "Ec2Instance", Some(&scope)).await;
        let stored = read["recommendationPreferencesDetails"]
            .as_array()
            .map_or(0, Vec::len);
        let expected_records = usize::from(wanted == 200);
        assert_eq!(
            stored, expected_records,
            "case {case:?}: expected {expected_records} stored record(s), got {read}"
        );
    }
}

/// An exclusion is OBSERVABLE: the excluded instance type is gone from the candidates while
/// its siblings — the other sizes of the same family — are still there, which is the whole
/// point of reading the resolved list back.
#[tokio::test]
async fn an_exclusion_removes_only_what_it_names() {
    let router = emulator();
    opt_in(&router).await;
    let scope = json!({"name": "AccountId", "value": ACCOUNT});

    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": scope.clone(),
            "preferredResources": [{
                "name": "Ec2InstanceTypes",
                "excludeList": ["m6g.large"],
            }],
        }),
    )
    .await;

    let read = get_preferences(&router, "Ec2Instance", Some(&scope)).await;
    let reported = &read["recommendationPreferencesDetails"][0]["preferredResources"][0];
    let effective = reported_list(&reported["effectiveIncludeList"]);

    assert!(
        !effective.contains(&"m6g.large".to_string()),
        "the excluded instance type is not a candidate, got {reported}"
    );
    // The siblings: every other size the catalog names in the same family.
    let siblings: Vec<String> = named_by(&["m6g."])
        .into_iter()
        .filter(|instance_type| instance_type != "m6g.large")
        .collect();
    assert!(
        !siblings.is_empty(),
        "the catalog names more than one size in the family, got {siblings:?}"
    );
    for sibling in &siblings {
        assert!(
            effective.contains(sibling),
            "expected the sibling {sibling:?} to still be a candidate, got {reported}"
        );
    }

    // Deleting the preference takes the record away, so the read reports the service's
    // default candidate set again rather than a resolution of a preference nobody holds.
    let (status, body) = call(
        &router,
        "DeleteRecommendationPreferences",
        json!({
            "resourceType": "Ec2Instance",
            "scope": scope.clone(),
            "recommendationPreferenceNames": ["PreferredResources"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the preference: expected 200, got {status}: {body}"
    );

    let read = get_preferences(&router, "Ec2Instance", Some(&scope)).await;
    assert!(
        read["recommendationPreferencesDetails"]
            .as_array()
            .is_none_or(|details| details.is_empty()),
        "expected no stored preference after the delete, got {read}"
    );
    let in_force = effective_for(&router, &instance_arn("i-0abcdef1234567890")).await;
    assert_eq!(
        reported_list(&in_force["preferredResources"][0]["effectiveIncludeList"]),
        candidates(&[""], &[]),
        "expected the default candidate set once nothing is stored, got {in_force}"
    );
}
