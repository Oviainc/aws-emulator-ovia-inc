// SPDX-License-Identifier: CC0-1.0

//! The `SubscriptionProviderStatus` lifecycle of a registration, over the real wire path
//! (every call goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be).
//!
//! The rule under test: a registration the service took starts in the non-terminal status
//! the create call reports and, once the clock owes the transition, reaches the status that
//! means the resource is USABLE — reported identically by every operation that reports it,
//! with the derived members that status presupposes populated and nothing to report about
//! the token. Concretely:
//!
//! 1. for the span the create call opens the registration is `PENDING`, with no
//!    `SubscriptionProviderStatusMessage` and no `LastSuccessfulDataRetrievalTime` (there
//!    has been no retrieval yet), and no read moves it;
//! 2. once the span has passed, WHICHEVER operation reports a status first materializes the
//!    transition to `ACTIVE`, and every other reporting operation then agrees — the status
//!    is a property of the stored record, not of the operation that reported it;
//! 3. the `LastSuccessfulDataRetrievalTime` an `ACTIVE` registration reports is an
//!    ISO-8601 UTC instant inside the window between the registration call and the read,
//!    and it does not move on a later read or because the resource was tagged.
//!
//! The rows span what the rule generalises over: how many registrations the account holds,
//! which of the reporting operations (`GetRegisteredSubscriptionProvider`,
//! `ListRegisteredSubscriptionProviders`, the `@idempotent` repeat of
//! `RegisterSubscriptionProvider`) is the one that first observes the transition, the
//! `SecretArn` the registration names, and the Region the request is signed for.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::harness::{
    call_in_region, get_provider, harness, is_iso8601_utc, list_providers, listed, member,
    provider_arn, register, tag, wait_for_token_validation, ACTIVE, PENDING, RED_HAT,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::iso8601;

/// Secret ARNs spanning the modelled `SecretArn` pattern
/// `^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$`
/// (`smithy_query(query="operation",operation="RegisterSubscriptionProvider")`): a plain
/// one, another partition, the empty `{0,63}` region and account segments, and a secret
/// name using characters the `[^/]` class allows.
const SECRET_ARNS: &[&str] = &[
    "arn:aws:secretsmanager:us-east-1:123456789012:secret:rhel-token",
    "arn:aws-us-gov:secretsmanager:us-gov-west-1:123456789012:secret:rhsm-offline",
    "arn:aws:secretsmanager:::secret:token",
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:RedHat_Offline+Token=1",
    "arn:aws:secretsmanager:ap-southeast-2:123456789012:secret:rhel.token+2",
];

/// Which reporting operation is the first one called after the transition falls due, and so
/// the one that has to materialize it.
#[derive(Debug, Clone, Copy)]
enum FirstReporter {
    /// `GetRegisteredSubscriptionProvider`.
    Get,
    /// `ListRegisteredSubscriptionProviders`.
    List,
    /// The `@idempotent` repeat of `RegisterSubscriptionProvider`, which answers the
    /// registration already made and the status it has reached.
    RepeatedRegistration,
}

/// A registration request for `secret_arn` carrying `tag_count` generated tags.
fn request(secret_arn: &str, tag_count: usize) -> Value {
    let mut body = json!({
        "SubscriptionProviderSource": RED_HAT,
        "SecretArn": secret_arn,
    });
    if tag_count > 0 {
        let tags: serde_json::Map<String, Value> = (0..tag_count)
            .map(|index| (format!("key-{index:02}"), json!(format!("value-{index:02}"))))
            .collect();
        body["Tags"] = Value::Object(tags);
    }
    body
}

/// Rule 1: the span the create call opens is never zero — a caller reading straight after
/// the registration still sees the transient status, and reading does not move it.
#[tokio::test]
async fn a_fresh_registration_is_pending_before_the_transition_is_due() {
    let h = harness();

    for (index, secret_arn) in SECRET_ARNS.iter().enumerate() {
        let case = format!("register {secret_arn}");
        let registered = register(&h, &request(secret_arn, index)).await;
        assert_eq!(
            member(&registered, &case, "SubscriptionProviderStatus"),
            Some(PENDING),
            "case {case:?}: the registration call must report {PENDING}, got {:?}",
            registered.body
        );
        let arn = provider_arn(&registered, &case);

        for attempt in 0..3 {
            let case = format!("{case}, read {attempt} before the transition is due");
            let got = get_provider(&h, &arn).await;
            assert_eq!(
                member(&got, &case, "SubscriptionProviderStatus"),
                Some(PENDING),
                "case {case:?}: a read must not move the status, got {:?}",
                got.body
            );
            for absent in [
                "SubscriptionProviderStatusMessage",
                "LastSuccessfulDataRetrievalTime",
            ] {
                assert!(
                    got.ok(&case).get(absent).is_none_or(Value::is_null),
                    "case {case:?}: expected no {absent} while {PENDING}, got {:?}",
                    got.body
                );
            }
        }
    }
}

/// Rules 2 and 3: whichever operation runs first after the transition falls due reports the
/// usable status, every other reporter agrees, and the retrieval time the status reports on
/// is inside the window the registration opened.
#[tokio::test]
async fn whichever_operation_reports_first_sees_the_registration_usable() {
    // Rows: the operation that first observes the transition, and how many registrations
    // the account holds while it does.
    let rows: &[(FirstReporter, usize)] = &[
        (FirstReporter::Get, 1),
        (FirstReporter::Get, 5),
        (FirstReporter::List, 1),
        (FirstReporter::List, 3),
        (FirstReporter::RepeatedRegistration, 2),
        (FirstReporter::RepeatedRegistration, 5),
    ];

    for (reporter, providers) in rows.iter().copied() {
        let case = format!("{providers} registration(s), first reported by {reporter:?}");
        // A harness of its own per row, so each row's account holds only its own
        // registrations.
        let h = harness();

        // The window the retrieval time has to fall inside: from just before the
        // registration call to just after the read below. A second of slack on each end
        // covers this suite rendering the bounds at whole-second precision, which is the
        // precision the member carries.
        let opened = iso8601(SystemTime::now() - Duration::from_secs(1));
        let mut arns = Vec::new();
        for (index, secret_arn) in SECRET_ARNS.iter().enumerate().take(providers) {
            let case = format!("{case}, registration {index}");
            arns.push(provider_arn(
                &register(&h, &request(secret_arn, index)).await,
                &case,
            ));
        }

        // Nothing is called while the clock passes the span; the transition is owed anyway.
        wait_for_token_validation();

        // The FIRST call after it fell due is the row's reporter, so it is that operation
        // which has to materialize the transition.
        match reporter {
            FirstReporter::Get => {
                for arn in &arns {
                    let got = get_provider(&h, arn).await;
                    assert_eq!(
                        member(&got, &case, "SubscriptionProviderStatus"),
                        Some(ACTIVE),
                        "case {case:?}: the first read of {arn} must report {ACTIVE}, got {:?}",
                        got.body
                    );
                }
            }
            FirstReporter::List => {
                let listing = list_providers(&h, &json!({})).await;
                for (arn, _, _, status) in listed(&listing, &case) {
                    assert_eq!(
                        status, ACTIVE,
                        "case {case:?}: the first list must report {ACTIVE} for {arn}, got \
                         {status:?}"
                    );
                }
            }
            FirstReporter::RepeatedRegistration => {
                for (index, arn) in arns.iter().enumerate() {
                    let again = register(&h, &request(SECRET_ARNS[index], 0)).await;
                    assert_eq!(
                        provider_arn(&again, &case),
                        *arn,
                        "case {case:?}: a repeat must answer the registration already made"
                    );
                    assert_eq!(
                        member(&again, &case, "SubscriptionProviderStatus"),
                        Some(ACTIVE),
                        "case {case:?}: a repeat must report the status the registration has \
                         reached, got {:?}",
                        again.body
                    );
                }
            }
        }

        // Every reporting operation now agrees, and the derived members are what the
        // status presupposes.
        let listing = list_providers(&h, &json!({})).await;
        let listed_rows = listed(&listing, &case);
        for (index, arn) in arns.iter().enumerate() {
            let case = format!("{case}, registration {index}");
            let got = get_provider(&h, arn).await;
            assert_eq!(
                member(&got, &case, "SubscriptionProviderStatus"),
                Some(ACTIVE),
                "case {case:?}: expected {ACTIVE} from the read, got {:?}",
                got.body
            );
            assert!(
                got.ok(&case)
                    .get("SubscriptionProviderStatusMessage")
                    .is_none_or(Value::is_null),
                "case {case:?}: a usable registration has nothing to report about its token, \
                 got {:?}",
                got.body
            );
            let retrieved = retrieval_time(&got.body, &case);
            let closed = iso8601(SystemTime::now() + Duration::from_secs(1));
            assert!(
                retrieved.as_str() >= opened.as_str() && retrieved.as_str() <= closed.as_str(),
                "case {case:?}: expected a retrieval time inside [{opened}, {closed}], got \
                 {retrieved:?}"
            );

            let listed_row = listed_rows
                .iter()
                .find(|(listed_arn, _, _, _)| listed_arn == arn)
                .unwrap_or_else(|| panic!("case {case:?}: {arn} is missing from {listed_rows:?}"));
            assert_eq!(
                listed_row.3, ACTIVE,
                "case {case:?}: the list must agree with the read, got {:?}",
                listed_row.3
            );
            let listed_entry = listing
                .ok(&case)
                .get("RegisteredSubscriptionProviders")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no providers in {:?}", listing.body))
                .iter()
                .find(|entry| entry.get("SubscriptionProviderArn") == Some(&json!(arn)))
                .unwrap_or_else(|| panic!("case {case:?}: {arn} is missing from the list"))
                .clone();
            assert_eq!(
                retrieval_time(&listed_entry, &case),
                retrieved,
                "case {case:?}: the list must report the retrieval time the read did"
            );

            // A later read, and a change to the resource that has nothing to do with the
            // retrieval, leave both members where they were.
            tag(&h, arn, &json!({ "owner": "platform" })).await.ok(&case);
            let after = get_provider(&h, arn).await;
            assert_eq!(
                member(&after, &case, "SubscriptionProviderStatus"),
                Some(ACTIVE),
                "case {case:?}: the terminal status must stay put, got {:?}",
                after.body
            );
            assert_eq!(
                retrieval_time(&after.body, &case),
                retrieved,
                "case {case:?}: the retrieval time must not move between reads"
            );
        }
    }
}

/// Rule 2, across Regions: the transition belongs to the registration, not to the Region
/// the request that observes it was signed for.
#[tokio::test]
async fn the_transition_lands_for_a_registration_of_any_region() {
    let h = harness();
    let regions = ["us-west-2", "eu-west-1", "ap-southeast-7", "us-gov-east-1"];

    let mut arns = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let case = format!("register signed for {region}");
        let registered = call_in_region(
            &h,
            region,
            "/subscription/RegisterSubscriptionProvider",
            &request(SECRET_ARNS[index], 1),
        )
        .await;
        assert_eq!(
            member(&registered, &case, "SubscriptionProviderStatus"),
            Some(PENDING),
            "case {case:?}: the registration call must report {PENDING}, got {:?}",
            registered.body
        );
        arns.push((*region, provider_arn(&registered, &case)));
    }

    wait_for_token_validation();

    for (region, arn) in &arns {
        let case = format!("read {arn} signed for {region}");
        let got = call_in_region(
            &h,
            region,
            "/subscription/GetRegisteredSubscriptionProvider",
            &json!({ "SubscriptionProviderArn": arn }),
        )
        .await;
        assert_eq!(
            member(&got, &case, "SubscriptionProviderStatus"),
            Some(ACTIVE),
            "case {case:?}: expected {ACTIVE}, got {:?}",
            got.body
        );
        assert!(
            !retrieval_time(&got.body, &case).is_empty(),
            "case {case:?}: expected a retrieval time, got {:?}",
            got.body
        );
    }
}

/// The `LastSuccessfulDataRetrievalTime` of a body that must carry one, checked to be the
/// ISO-8601 UTC instant at whole-second precision (`YYYY-MM-DDTHH:MM:SSZ`) this service
/// renders its timestamp member as — the member is a plain `String` in the model, so a
/// client parses the value itself and the form is part of the contract.
fn retrieval_time(body: &Value, case: &str) -> String {
    let value = body
        .get("LastSuccessfulDataRetrievalTime")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no LastSuccessfulDataRetrievalTime in {body:?}")
        })
        .to_string();
    assert!(
        is_iso8601_utc(&value),
        "case {case:?}: expected an ISO-8601 UTC instant, got {value:?}"
    );
    value
}
