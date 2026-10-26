// SPDX-License-Identifier: CC0-1.0

//! `GetProfile`: the profile of the account this emulator serves, driven over the
//! wire through the same front door a client uses.
//!
//! The rule under test: `GetProfile` reports the account's profile from the ONE
//! profile record this service's store holds — its `profileType` is the partner one
//! exactly while that record holds a partner program membership and the Lightsailor
//! one otherwise, the `partner` object is carried exactly in the first case and
//! carries that membership's own enrolment instant, tier and status — and reporting
//! it changes nothing a later caller sees.
//!
//! The input shape declares no member, so the request itself has no dimension to
//! vary: what the rows below vary is what the STORE holds — no membership, and a
//! membership in each tier the modelled `TierName` enum admits (including the ones
//! nothing else in this suite names), with no tier at all, in each status
//! `PartnerStatus` admits, enrolled at a different instant per row. The read-invariance
//! case varies when a read happens, what else the store holds, and which Region's
//! endpoint the read is made at.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::{
    PARTNER_STATUS_VALUES, PROFILE_TYPE_VALUES, REGION_NAME_VALUES, TIER_NAME_VALUES,
};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, StoredAccountProfile, StoredPartnerMembership, PARTNER_STATUS_ACTIVE,
    PARTNER_STATUS_SUSPENDED, PROFILE_TYPE_LIGHTSAILOR, PROFILE_TYPE_LIGHTSAIL_PARTNER,
};

use crate::instance_lifecycle::Emulator;

/// A plausible instant to be enrolled in the partner program at, which the rows
/// below walk away from so no two of them share one.
const ENROLLED_AT_MS: i64 = 1_700_000_000_000;

/// A day in milliseconds, so one row's enrolment instant is a different DAY from
/// the next one's rather than a different millisecond.
const ONE_DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// The members a reported `partner` object may carry, in wire spelling.
///
/// SOURCE: the modelled `PartnerInfo` shape
/// (`smithy_query(query="shape",shape="PartnerInfo")`) — `enrolledAt` and `status`
/// required, `tierName` optional. Written out here independently of the service's
/// own types, so these rows pin the published form rather than restating whatever
/// the code holds.
const REQUIRED_PARTNER_MEMBERS: &[&str] = &["enrolledAt", "status"];
const OPTIONAL_PARTNER_MEMBER: &str = "tierName";

/// One row: what the store holds, and the case name every assertion carries.
struct Row {
    case: String,
    membership: Option<StoredPartnerMembership>,
}

/// The rows: the not-enrolled account, and one membership per (tier, status) pair
/// the modelled enums admit — the tier absent as well as each value of `TierName`,
/// crossed with each value of `PartnerStatus`, each row enrolled on its own day.
fn rows() -> Vec<Row> {
    let mut rows = vec![Row {
        case: "an account that is not enrolled".to_string(),
        membership: None,
    }];

    // `None` first: `PartnerInfo$tierName` is not required, so a membership in no
    // tier is a case of the rule, not a malformed row.
    let tiers: Vec<Option<&str>> = std::iter::once(None)
        .chain(TIER_NAME_VALUES.iter().copied().map(Some))
        .collect();
    for (index, (tier, status)) in tiers
        .iter()
        .flat_map(|tier| PARTNER_STATUS_VALUES.iter().map(move |status| (*tier, *status)))
        .enumerate()
    {
        let day = i64::try_from(index).expect("the row index fits");
        rows.push(Row {
            case: format!(
                "a membership in tier {} with status {status}",
                tier.unwrap_or("<none>")
            ),
            membership: Some(StoredPartnerMembership {
                enrolled_at_ms: ENROLLED_AT_MS + day * ONE_DAY_MS,
                tier_name: tier.map(str::to_string),
                status: status.to_string(),
            }),
        });
    }
    rows
}

/// Put `membership` in the store's account profile record.
///
/// Nothing in this service's API enrols an account in the Lightsail partner
/// program — enrolment happens outside Lightsail — so the store is where a case
/// about an enrolled account is set up, through the same public state API a
/// handler reads, so the rule is exercised against the ONE persisted record rather
/// than a copy.
fn enrol(emulator: &Emulator, membership: Option<StoredPartnerMembership>) {
    state::with_state(emulator.engine(), |store| {
        store.account_profile.partner = membership;
    })
    .expect("the suite's engine is the shared in-memory one");
}

/// Whether `reported` — a timestamp on the wire, in seconds since the epoch — is
/// the instant `expected_ms` names.
fn is_instant(reported: Option<f64>, expected_ms: i64) -> bool {
    reported.is_some_and(|secs| (secs * 1_000.0 - expected_ms as f64).abs() < 1.0)
}

/// One tier of the modelled `TierName` enum, for a case that needs an enrolled
/// account but is not about which tier it is in. Taken from the enum rather than
/// written out, so it is still a modelled tier whatever that enum grows to hold;
/// `None` only if the enum ever declares no value at all, which is itself a
/// membership the rule admits.
fn a_modelled_tier() -> Option<String> {
    TIER_NAME_VALUES.last().map(|tier| (*tier).to_string())
}

/// The members `object` carries, for an assertion about the shape of a response.
fn members(object: &Value) -> BTreeSet<String> {
    object
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// Assert that `body` is the profile the store holding `membership` must report.
fn assert_reports(case: &str, body: &Value, membership: Option<&StoredPartnerMembership>) {
    let profile_type = body["profileType"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a profileType, got {body:?}"));
    assert!(
        PROFILE_TYPE_VALUES.contains(&profile_type),
        "case {case}: expected a modelled ProfileType, got {profile_type:?}"
    );
    let expected_type = if membership.is_some() {
        PROFILE_TYPE_LIGHTSAIL_PARTNER
    } else {
        PROFILE_TYPE_LIGHTSAILOR
    };
    assert_eq!(
        profile_type, expected_type,
        "case {case}: expected profileType {expected_type:?}, got {profile_type:?}"
    );

    let Some(membership) = membership else {
        assert!(
            body["partner"].is_null(),
            "case {case}: an account that is not enrolled reports no partner membership, \
             got {body:?}"
        );
        return;
    };

    let partner = &body["partner"];
    let reported = members(partner);
    let mut expected: BTreeSet<String> = REQUIRED_PARTNER_MEMBERS
        .iter()
        .map(|member| (*member).to_string())
        .collect();
    if membership.tier_name.is_some() {
        expected.insert(OPTIONAL_PARTNER_MEMBER.to_string());
    }
    assert_eq!(
        reported, expected,
        "case {case}: expected exactly the members the membership holds, got {partner:?}"
    );

    assert!(
        is_instant(partner["enrolledAt"].as_f64(), membership.enrolled_at_ms),
        "case {case}: expected the stored enrolment instant ({} ms), got {:?}",
        membership.enrolled_at_ms,
        partner["enrolledAt"]
    );

    let status = partner["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a partner status, got {partner:?}"));
    assert!(
        PARTNER_STATUS_VALUES.contains(&status),
        "case {case}: expected a modelled PartnerStatus, got {status:?}"
    );
    assert_eq!(
        status,
        membership.status.as_str(),
        "case {case}: expected the stored status, got {status:?}"
    );

    let tier = partner[OPTIONAL_PARTNER_MEMBER].as_str();
    assert_eq!(
        tier,
        membership.tier_name.as_deref(),
        "case {case}: expected the stored tier, got {tier:?}"
    );
    if let Some(tier) = tier {
        assert!(
            TIER_NAME_VALUES.contains(&tier),
            "case {case}: expected a modelled TierName, got {tier:?}"
        );
    }
}

/// RULE: the profile of an account nothing has enrolled is the not-enrolled one —
/// the Lightsailor profile type, with no partner membership reported.
#[tokio::test]
async fn a_fresh_accounts_profile_is_the_not_enrolled_one() {
    let emulator = Emulator::new();
    let body = emulator.ok("GetProfile", json!({})).await;
    assert_reports("a fresh account", &body, None);
    assert_eq!(
        members(&body),
        ["profileType".to_string()].into_iter().collect(),
        "case a fresh account: expected only the required profileType, got {body:?}"
    );
}

/// RULE: the reported profile is the one the store holds — the partner profile type
/// exactly while a membership is held, and that membership's own enrolment instant,
/// tier and status.
#[tokio::test]
async fn the_reported_profile_is_the_one_the_store_holds() {
    for row in rows() {
        let emulator = Emulator::new();
        enrol(&emulator, row.membership.clone());
        let body = emulator.ok("GetProfile", json!({})).await;
        assert_reports(&row.case, &body, row.membership.as_ref());
    }
}

/// RULE: the profile belongs to the ACCOUNT, so it is the same whichever Region's
/// endpoint the request is made at — every Region the modelled `RegionName` enum
/// admits, and a request carrying no credential scope at all.
#[tokio::test]
async fn the_profile_is_the_same_in_every_region() {
    let emulator = Emulator::new();
    let membership = StoredPartnerMembership {
        enrolled_at_ms: ENROLLED_AT_MS,
        // A tier taken from the modelled enum rather than written out, so this row
        // keeps naming a modelled tier however that enum changes.
        tier_name: a_modelled_tier(),
        status: PARTNER_STATUS_SUSPENDED.to_string(),
    };
    enrol(&emulator, Some(membership.clone()));

    let unscoped = emulator.ok("GetProfile", json!({})).await;
    assert_reports("no credential scope", &unscoped, Some(&membership));

    for region in REGION_NAME_VALUES {
        let (status, body) = emulator
            .call_in(Some(region), "GetProfile", json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case region {region}: GetProfile must succeed, got {status} {body:?}"
        );
        assert_reports(&format!("region {region}"), &body, Some(&membership));
        assert_eq!(
            body, unscoped,
            "case region {region}: expected the same account profile as elsewhere, \
             got {body:?} after {unscoped:?}"
        );
    }
}

/// RULE: reporting the profile is a READ — repeated reads agree, an unrelated
/// mutation does not move the profile, and the read consumes nothing the store
/// holds.
#[tokio::test]
async fn reading_the_profile_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let membership = StoredPartnerMembership {
        enrolled_at_ms: ENROLLED_AT_MS,
        tier_name: a_modelled_tier(),
        status: PARTNER_STATUS_ACTIVE.to_string(),
    };
    enrol(&emulator, Some(membership.clone()));

    let first = emulator.ok("GetProfile", json!({})).await;
    let again = emulator.ok("GetProfile", json!({})).await;
    assert_eq!(
        again, first,
        "case a repeated read: two reads of the profile must agree, got {again:?} after \
         {first:?}"
    );

    // A mutation of unrelated state, then the same read: the profile is not a
    // function of what resources the account holds. `CreateContainerService`
    // answers HTTP 201, its modelled success status.
    let (status, created) = emulator
        .call(
            "CreateContainerService",
            json!({"serviceName": "profile-reader", "power": "nano", "scale": 1}),
        )
        .await;
    assert_eq!(
        status, 201,
        "case the unrelated mutation: expected HTTP 201, got {status} {created:?}"
    );

    let after_mutation = emulator.ok("GetProfile", json!({})).await;
    assert_eq!(
        after_mutation, first,
        "case after an unrelated mutation: the profile must not depend on what resources \
         the store holds, got {after_mutation:?} after {first:?}"
    );

    // The membership the reads were served from is still exactly what was stored:
    // a read neither retires it nor moves its status.
    let held = state::with_state(emulator.engine(), |store| store.account_profile.partner.clone())
        .expect("the suite's engine is the shared in-memory one");
    assert_eq!(
        held,
        Some(membership),
        "case store unchanged: expected the stored membership to survive the reads, got \
         {held:?}"
    );

    // And the container service the mutation created survived the reads too.
    let service = emulator
        .ok(
            "GetContainerServices",
            json!({"serviceName": "profile-reader"}),
        )
        .await;
    assert_eq!(
        service["containerServices"][0]["containerServiceName"].as_str(),
        Some("profile-reader"),
        "case store unchanged: expected the created container service to survive the reads, \
         got {service:?}"
    );
}

/// RULE: the profile TYPE follows from the membership rather than being stored
/// beside it, so the two can never disagree — and the membership a profile reports
/// is the one it holds, member for member.
///
/// Asserted on the store's own profile record, which no wire read can distinguish
/// from a second field kept in step by hand.
#[test]
fn the_profile_type_follows_the_stored_membership() {
    for row in rows() {
        let case = &row.case;
        let profile = StoredAccountProfile {
            partner: row.membership.clone(),
        };
        let expected_type = if row.membership.is_some() {
            PROFILE_TYPE_LIGHTSAIL_PARTNER
        } else {
            PROFILE_TYPE_LIGHTSAILOR
        };
        assert_eq!(
            profile.profile_type(),
            expected_type,
            "case {case}: expected profile type {expected_type:?}, got {:?}",
            profile.profile_type()
        );

        let view = profile.partner_view();
        match &row.membership {
            None => assert!(
                view.is_none(),
                "case {case}: an account holding no membership reports none, got {view:?}"
            ),
            Some(membership) => {
                let view = view.unwrap_or_else(|| {
                    panic!("case {case}: a held membership must be reported, got nothing")
                });
                assert_eq!(
                    view.enrolled_at,
                    state::timestamp(membership.enrolled_at_ms),
                    "case {case}: expected the stored enrolment instant, got {:?}",
                    view.enrolled_at
                );
                assert_eq!(
                    view.tier_name, membership.tier_name,
                    "case {case}: expected the stored tier, got {:?}",
                    view.tier_name
                );
                assert_eq!(
                    view.status, membership.status,
                    "case {case}: expected the stored status, got {:?}",
                    view.status
                );
            }
        }
    }
}
