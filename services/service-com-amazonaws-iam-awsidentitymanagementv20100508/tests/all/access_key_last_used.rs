// SPDX-License-Identifier: CC0-1.0

//! When an access key of this account was last USED, end to end over the awsQuery wire.
//!
//! RULE: `GetAccessKeyLastUsed` reports the most recent call the account recorded as made
//! with the key it names — the instant that call was served as `LastUsedDate`, the
//! namespace of the service that served it as `ServiceName` and the Region it was served
//! in as `Region` — beside the name of the IAM user that owns the key; a key the account
//! holds that no recorded call was made with reports no `LastUsedDate` and `N/A` for both
//! of the other two; and an access key id the account holds no key under is not found.
//! Nothing about the report depends on which call is asked about it: it is a read, so two
//! reads of one key answer the same.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the owning
//! user's name alphabet and case, how many keys that user holds, which operation the key
//! was used for, the Region the using call was signed for, the service that served the
//! recorded call and the Region that record carries, and the form of an access key id the
//! account holds no key under.
//!
//! The awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way. The cases
//! that need a use by ANOTHER service write it into the account's shared management-event
//! log directly — that log is where every serving service records its calls, and this
//! crate holds only IAM.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_common::management_events::{
    ManagementEventLog, ServedApiCall, PRINCIPAL_TYPE_IAM_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::grant_action;
use crate::user_lifecycle::{
    call, call_signed_in_region, element, elements, error_code, router_with_engine,
};

/// What `AccessKeyLastUsed$ServiceName` and `AccessKeyLastUsed$Region` carry for a key
/// that has not been used, from those members' own documentation ("the value of this
/// field is \"N/A\"" when "an access key exists but has not been used since IAM started
/// tracking this information").
///
/// Restated here as the published value rather than imported from the crate, so the
/// assertion cannot follow the code wherever it goes.
const NEVER_USED: &str = "N/A";

/// The namespace an IAM call is reported under: IAM's own endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: iam`).
const IAM_NAMESPACE: &str = "iam";

/// The Region an IAM call is reported in. IAM is a GLOBAL service — one endpoint, ARNs
/// with an empty Region segment — so its calls are recorded in the partition's leader
/// Region whichever Region a caller signed for.
const IAM_REGION: &str = "us-east-1";

/// One owning user: the name the account holds it under, and the case the create spells.
///
/// The two differ on most rows because IAM entity names "are not distinguished by case"
/// (`CreateUser$UserName`), so the report must name the user as the account holds it.
struct Owner {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `CreateAccessKey` spells; the same entity, differing only in case.
    requested_name: &'static str,
}

/// Rows spanning the whole `^[\w+=,.@-]+$` alphabet the name constraint admits
/// (`smithy_query(query="operation", operation="CreateAccessKey")`), both cases of the
/// stored name, and creates that fold the name up, down and not at all.
const OWNERS: &[Owner] = &[
    Owner {
        stored_name: "alice",
        requested_name: "alice",
    },
    Owner {
        stored_name: "Bob",
        requested_name: "bob",
    },
    Owner {
        stored_name: "carol_7",
        requested_name: "CAROL_7",
    },
    Owner {
        stored_name: "dave.x",
        requested_name: "dave.X",
    },
    Owner {
        stored_name: "Eve+1",
        requested_name: "eve+1",
    },
    Owner {
        stored_name: "frank=2",
        requested_name: "frank=2",
    },
    Owner {
        stored_name: "grace@3",
        requested_name: "GRACE@3",
    },
    Owner {
        stored_name: "heidi-4",
        requested_name: "heidi-4",
    },
];

/// How many access keys one IAM user may hold, from AWS's "IAM and STS quotas" page
/// ("Access keys per IAM user: 2") — so the table covers a user holding its whole
/// allowance rather than only the first key.
const MAX_ACCESS_KEYS_PER_USER: usize = 2;

/// Create the user `stored_name` and answer the ids of the access keys minted for it,
/// each created under the name `requested_name` spells.
async fn keys_of(router: &MultiServiceRouter, owner: &Owner, count: usize) -> Vec<String> {
    let case = format!("{} requested as {}", owner.stored_name, owner.requested_name);
    let (status, created) = call(router, "CreateUser", &[("UserName", owner.stored_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
    );
    let mut minted = Vec::new();
    for index in 0..count {
        let (status, key) = call(
            router,
            "CreateAccessKey",
            &[("UserName", owner.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateAccessKey {index} must succeed, got {status}: {key}"
        );
        minted.push(element(&key, "AccessKeyId"));
    }
    minted
}

/// `GetAccessKeyLastUsed` for `access_key_id`, as the HTTP status and the XML body.
///
/// Driven UNSIGNED, so the call is not itself a use of the key it asks about: what the
/// report says about a key is then only ever the doing of the calls the case made with
/// it.
async fn last_used(router: &MultiServiceRouter, access_key_id: &str) -> (u16, String) {
    call(
        router,
        "GetAccessKeyLastUsed",
        &[("AccessKeyId", access_key_id)],
    )
    .await
}

/// The epoch seconds of `rendered`, an awsQuery timestamp.
fn epoch_seconds(case: &str, rendered: &str) -> i64 {
    aws_smithy_types::DateTime::from_str(rendered, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected an ISO-8601 timestamp, got {rendered:?}: {error}")
        })
        .secs()
}

/// The wall clock as whole epoch seconds.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the instant fits an i64")
}

/// RULE: a key the account holds that no recorded call was made with has never been used:
/// no `LastUsedDate` at all, and `N/A` for both the service and the Region — reported
/// beside the name of the user that owns it, in the case the account holds that name.
#[tokio::test]
async fn a_key_no_call_was_made_with_reports_no_use() {
    let (router, _engine) = router_with_engine();

    for owner in OWNERS {
        let keys = keys_of(&router, owner, MAX_ACCESS_KEYS_PER_USER).await;
        for (index, access_key_id) in keys.iter().enumerate() {
            let case = format!(
                "{} requested as {}, key {index}",
                owner.stored_name, owner.requested_name
            );
            let (status, body) = last_used(&router, access_key_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "UserName"),
                owner.stored_name,
                "case {case:?}: expected the owner's stored name, got {:?}",
                element(&body, "UserName")
            );
            assert_eq!(
                element(&body, "ServiceName"),
                NEVER_USED,
                "case {case:?}: expected {NEVER_USED} for an unused key, got {:?}",
                element(&body, "ServiceName")
            );
            assert_eq!(
                element(&body, "Region"),
                NEVER_USED,
                "case {case:?}: expected {NEVER_USED} for an unused key, got {:?}",
                element(&body, "Region")
            );
            assert!(
                elements(&body, "LastUsedDate").is_empty(),
                "case {case:?}: expected no LastUsedDate for an unused key, got {body}"
            );

            // Asking again cannot change what the next caller sees.
            let (status, again) = last_used(&router, access_key_id).await;
            assert_eq!(
                (status, element(&again, "ServiceName")),
                (200, NEVER_USED.to_string()),
                "case {case:?}: a second read answered differently: {again}"
            );
        }
    }
}

/// One use of a key through a real call: the operation it was used for, the IAM action a
/// grant has to name for that call to be served, and the Region the using call signs for.
struct Use {
    /// The awsQuery `Action` the key is used for.
    action: &'static str,
    /// The IAM action the caller needs, since IAM authorizes its own API.
    iam_action: &'static str,
    /// The Region the using call's SigV4 credential scope names.
    signed_in: &'static str,
}

/// Rows spanning several operations of the service — a single-entity read and four
/// listings over different collections — and four Regions a caller can sign for,
/// including IAM's own.
const USES: &[Use] = &[
    Use {
        action: "GetUser",
        iam_action: "iam:GetUser",
        signed_in: "us-east-1",
    },
    Use {
        action: "ListAccessKeys",
        iam_action: "iam:ListAccessKeys",
        signed_in: "eu-west-1",
    },
    Use {
        action: "ListUserPolicies",
        iam_action: "iam:ListUserPolicies",
        signed_in: "ap-southeast-2",
    },
    Use {
        action: "ListUserTags",
        iam_action: "iam:ListUserTags",
        signed_in: "us-west-2",
    },
    Use {
        action: "ListGroupsForUser",
        iam_action: "iam:ListGroupsForUser",
        signed_in: "sa-east-1",
    },
];

/// RULE: a call made WITH a key is a use of it, so the report names the instant that call
/// was served, the namespace of the service that served it and the Region it was served
/// in — whichever operation the key was used for, and whichever Region the using caller
/// signed for, because the Region reported is the one the serving service recorded.
#[tokio::test]
async fn a_call_made_with_a_key_is_reported_as_its_last_use() {
    let (router, _engine) = router_with_engine();

    for (row, used) in USES.iter().enumerate() {
        let case = format!("{} signed in {}", used.action, used.signed_in);
        let owner = Owner {
            stored_name: match row {
                0 => "user.zero",
                1 => "USER_ONE",
                2 => "user+two",
                3 => "user=three",
                _ => "user@four",
            },
            requested_name: match row {
                0 => "USER.ZERO",
                1 => "user_one",
                2 => "user+two",
                3 => "USER=THREE",
                _ => "user@four",
            },
        };
        let keys = keys_of(&router, &owner, 1).await;
        let access_key_id = keys.first().expect("one key was minted");
        grant_action(&router, &case, owner.stored_name, used.iam_action).await;

        let before = now_seconds();
        let (status, probed) = call_signed_in_region(
            &router,
            Some(access_key_id),
            used.signed_in,
            used.action,
            &[("UserName", owner.stored_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call made with the key must be served, got {status}: {probed}"
        );
        let after = now_seconds();

        let (status, body) = last_used(&router, access_key_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "UserName"),
            owner.stored_name,
            "case {case:?}: expected the owner's stored name, got {:?}",
            element(&body, "UserName")
        );
        assert_eq!(
            element(&body, "ServiceName"),
            IAM_NAMESPACE,
            "case {case:?}: expected the serving service's namespace, got {:?}",
            element(&body, "ServiceName")
        );
        assert_eq!(
            element(&body, "Region"),
            IAM_REGION,
            "case {case:?}: expected the Region the call was served in, got {:?}",
            element(&body, "Region")
        );
        let rendered = element(&body, "LastUsedDate");
        let reported = epoch_seconds(&case, &rendered);
        assert!(
            (before..=after).contains(&reported),
            "case {case:?}: expected a LastUsedDate inside [{before}, {after}], got \
             {rendered:?} ({reported})"
        );

        // The key's OTHER keyholder-independent facts are untouched by the read, and a
        // second read answers the same instant rather than the clock.
        let (status, again) = last_used(&router, access_key_id).await;
        assert_eq!(
            (status, element(&again, "LastUsedDate")),
            (200, rendered.clone()),
            "case {case:?}: a second read moved the reported instant: {again}"
        );
    }
}

/// A call another service of this account served, as that service's recorder reports one:
/// signed with `access_key_id`, served in `region` and recorded under the `eventSource`
/// `source`.
fn served_elsewhere(source: &str, region: &str, access_key_id: &str) -> ServedApiCall {
    ServedApiCall {
        event_name: "SomeCall".to_string(),
        event_source: source.to_string(),
        aws_region: region.to_string(),
        read_only: true,
        access_key_id: Some(access_key_id.to_string()),
        user_name: None,
        principal_type: PRINCIPAL_TYPE_IAM_USER.to_string(),
        principal_id: None,
        identity_provider: None,
        session: None,
        account_id: engine::emulator_account_id().to_string(),
        user_agent: None,
        resources: Vec::new(),
        request_parameters: serde_json::Value::Null,
        response_elements: None,
        error_code: None,
        error_message: None,
    }
}

/// One recorded use by another service: the namespace the report must name, the
/// `eventSource` the record carries, the Region it was filed in, and how many seconds
/// after the base instant the call was served.
struct Elsewhere {
    /// The service namespace the report must name.
    service: &'static str,
    /// The `eventSource` the record carries.
    source: &'static str,
    /// The Region the record was filed in.
    region: &'static str,
    /// Seconds after the base instant the call was served at.
    after: u64,
}

/// Rows spanning four services and four Regions, written in an order that is NOT the
/// order of their instants: the third row is served LATER than the fourth, so a report
/// that simply took the latest record written would name the wrong one. The last row's
/// source is not in the `<prefix>.amazonaws.com` form, which is reported whole because
/// no namespace is invented for a source that does not carry one.
const ELSEWHERE: &[Elsewhere] = &[
    Elsewhere {
        service: "s3",
        source: "s3.amazonaws.com",
        region: "us-west-2",
        after: 10,
    },
    Elsewhere {
        service: "sts",
        source: "sts.amazonaws.com",
        region: "eu-west-1",
        after: 70,
    },
    Elsewhere {
        service: "dynamodb",
        source: "dynamodb.amazonaws.com",
        region: "ap-southeast-1",
        after: 130,
    },
    Elsewhere {
        service: "lambda",
        source: "lambda.amazonaws.com",
        region: "us-east-2",
        after: 100,
    },
    Elsewhere {
        service: "health.aws.internal",
        source: "health.aws.internal",
        region: "eu-central-1",
        after: 190,
    },
];

/// RULE: the use reported is the MOST RECENT recorded one, whichever service of the
/// account served it and whichever Region it was served in — so a later call replaces an
/// earlier one in the report and an earlier one does not displace a later one.
#[tokio::test]
async fn the_most_recent_recorded_use_is_the_one_reported() {
    let (router, engine) = router_with_engine();
    let owner = Owner {
        stored_name: "Keyholder",
        requested_name: "keyholder",
    };
    let keys = keys_of(&router, &owner, 1).await;
    let access_key_id = keys.first().expect("one key was minted");

    // A whole-second instant an hour ago, so every seeded time survives the wire
    // (whole seconds) exactly and the window below needs no rounding.
    let base = UNIX_EPOCH + Duration::from_secs(u64::try_from(now_seconds() - 3_600).unwrap_or(0));

    let mut latest: Option<&Elsewhere> = None;
    for seeded in ELSEWHERE {
        let case = format!("{} in {} at +{}s", seeded.source, seeded.region, seeded.after);
        engine.state::<ManagementEventLog>().lock().record(
            served_elsewhere(seeded.source, seeded.region, access_key_id),
            base + Duration::from_secs(seeded.after),
        );
        if latest.is_none_or(|held| held.after < seeded.after) {
            latest = Some(seeded);
        }
        let expected = latest.expect("a row was seeded");

        let (status, body) = last_used(&router, access_key_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            (
                element(&body, "ServiceName"),
                element(&body, "Region"),
                epoch_seconds(&case, &element(&body, "LastUsedDate")),
            ),
            (
                expected.service.to_string(),
                expected.region.to_string(),
                i64::try_from(
                    (base + Duration::from_secs(expected.after))
                        .duration_since(UNIX_EPOCH)
                        .expect("the instant is after the epoch")
                        .as_secs()
                )
                .expect("the instant fits an i64"),
            ),
            "case {case:?}: expected the most recent recorded use ({} in {} at +{}s), got \
             {body}",
            expected.service,
            expected.region,
            expected.after
        );
        // The owner is reported whichever service the use was served by.
        assert_eq!(
            element(&body, "UserName"),
            owner.stored_name,
            "case {case:?}: expected the owner's stored name, got {:?}",
            element(&body, "UserName")
        );
    }
}

/// RULE: an access key is reachable only through the user that holds it, so an access key
/// id the account holds no key under names nothing to report on: HTTP 404 `NoSuchEntity`,
/// naming the id the request carried.
#[tokio::test]
async fn an_id_the_account_holds_no_key_under_is_not_found() {
    let (router, _engine) = router_with_engine();
    let owner = Owner {
        stored_name: "holder",
        requested_name: "holder",
    };
    let keys = keys_of(&router, &owner, 1).await;
    let held = keys.first().expect("one key was minted").clone();

    // A key the account held and then deleted is an id it holds no key under too, which
    // is a row rather than a special case.
    let (status, deleted) = call(
        &router,
        "DeleteAccessKey",
        &[("UserName", owner.stored_name), ("AccessKeyId", &held)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture DeleteAccessKey must succeed, got {status}: {deleted}"
    );

    // Rows spanning the deleted id, an id of AWS's published example form, an id this
    // emulator never issued, an id at the 16-character minimum `accessKeyIdType` admits
    // and one at its 128-character maximum.
    let absent: Vec<String> = vec![
        held,
        "AKIAIOSFODNN7EXAMPLE".to_string(),
        "EMULATORAKAAAAAAAAAA".to_string(),
        "A".repeat(16),
        "Z".repeat(128),
    ];

    for access_key_id in &absent {
        let (status, refused) = last_used(&router, access_key_id).await;
        assert_eq!(
            status, 404,
            "case {access_key_id:?}: expected 404, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NoSuchEntity",
            "case {access_key_id:?}: expected NoSuchEntity, got {:?}",
            error_code(&refused)
        );
        assert_eq!(
            element(&refused, "Message"),
            format!("The Access Key with id {access_key_id} cannot be found."),
            "case {access_key_id:?}: expected the family's wording for an absent key, got {:?}",
            element(&refused, "Message")
        );
    }
}

/// RULE: IAM authorizes its own API, so a caller is told what a key was used for only
/// where its identity-based policies allow the read — and a caller that may not read it
/// learns that, never whether the key exists.
#[tokio::test]
async fn the_read_is_decided_against_the_callers_policies() {
    let (router, _engine) = router_with_engine();
    let owner = Owner {
        stored_name: "gated",
        requested_name: "gated",
    };
    let keys = keys_of(&router, &owner, 1).await;
    let access_key_id = keys.first().expect("one key was minted");

    // Signed by the owner's own key, which holds no policy yet: nothing allows the read.
    let (status, refused) = call_signed_in_region(
        &router,
        Some(access_key_id),
        IAM_REGION,
        "GetAccessKeyLastUsed",
        &[("AccessKeyId", access_key_id)],
    )
    .await;
    assert_eq!(
        status, 403,
        "case a caller holding no policy: expected 403, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "AccessDenied",
        "case a caller holding no policy: expected AccessDenied, got {:?}",
        error_code(&refused)
    );
    assert!(
        elements(&refused, "ServiceName").is_empty() && elements(&refused, "Region").is_empty(),
        "case a caller holding no policy: expected the refusal to report no usage, got {refused}"
    );

    // With the grant the action needs, the same read is served.
    grant_action(
        &router,
        "a caller holding the grant",
        owner.stored_name,
        "iam:GetAccessKeyLastUsed",
    )
    .await;
    let (status, served) = call_signed_in_region(
        &router,
        Some(access_key_id),
        IAM_REGION,
        "GetAccessKeyLastUsed",
        &[("AccessKeyId", access_key_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case a caller holding the grant: expected 200, got {status}: {served}"
    );
    assert_eq!(
        element(&served, "UserName"),
        owner.stored_name,
        "case a caller holding the grant: expected the owner's stored name, got {:?}",
        element(&served, "UserName")
    );
}
