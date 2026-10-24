// SPDX-License-Identifier: CC0-1.0

//! The CREATE path of this service's primary resource: `RegisterSubscriptionProvider`,
//! and the lifecycle it opens, driven over the real wire path (every call goes through the
//! router's `dispatch`, so each request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be).
//!
//! The rules under test, each asserted over a SET of inputs rather than the one request
//! that exposed the defect:
//!
//! 1. a registration naming a `SubscriptionProviderSource` and a pattern-valid `SecretArn`
//!    CREATES the subscription-provider resource: HTTP 200 carrying a minted
//!    `SubscriptionProviderArn`, the registered source, and the
//!    `SubscriptionProviderStatus` of the registration action — `PENDING`, because the
//!    validation of the access token runs behind the call — and the resource is readable
//!    back through `GetRegisteredSubscriptionProvider`,
//!    `ListRegisteredSubscriptionProviders` and `ListTagsForResource`;
//! 2. the status the clock owes the registration lands WITHOUT any caller doing anything:
//!    a registration this emulator cannot validate settles into the modelled failure
//!    status `INVALID` with a `SubscriptionProviderStatusMessage` that says so, never into
//!    `ACTIVE` (nothing here accepted a token), and a read before it falls due reports
//!    `PENDING` twice — reading does not move it;
//! 3. `RegisterSubscriptionProvider` is `@idempotent`: a repeat naming a (source, secret)
//!    pair the account already registered answers THAT registration rather than minting a
//!    second resource, while a different secret is a registration of its own;
//! 4. the whole lifecycle closes over the created resource: tag it, read its tags, then
//!    deregister it and every later caller sees it gone;
//! 5. a repeat whose RESULTANT tag set would pass the modelled `Tags` maximum is refused
//!    with the tagging surface's `BadRequestException` and stores nothing;
//! 6. a registration past the number of registrations an account's store holds
//!    ([`MAX_REGISTERED_PROVIDERS`]) is refused with the operation's modelled
//!    `ValidationException` and leaves the store as it was.
//!
//! The tables span what each rule generalises over: the `SecretArn` grammar (partition,
//! empty region/account segments, the `[^/]` secret-name class), the size of the tag set
//! (none, one, three, the modelled maximum of 50), the signed Region the ARN is minted in,
//! and the number of registrations the account holds.

use serde_json::{json, Value};

use crate::harness::{
    call_in_region, deregister, get_provider, harness, list_providers, list_tags, listed, member,
    provider_arn, register, stored_provider_count, tag, wait_for_token_validation, Harness, Reply,
    ACCOUNT, ACTIVE, INVALID, PENDING, RED_HAT, REGION, SIGNING_NAME,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::MAX_REGISTERED_PROVIDERS;

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
];

/// The modelled `Tags` maximum (`@length 0..50`).
const MAX_TAGS: usize = 50;

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

/// A variant of `secret_arn` naming a DIFFERENT secret, so a table row registers a
/// resource of its own rather than repeating the previous row's registration (which
/// `@idempotent` answers with the registration already made —
/// [`a_repeated_registration_is_the_same_resource`]). The suffix lands in the secret-name
/// segment, whose modelled class is `[^/]{1,1023}`.
fn distinct_secret(secret_arn: &str, suffix: usize) -> String {
    format!("{secret_arn}-{suffix}")
}

/// The tag keys [`request`] puts on a registration carrying `tag_count` tags.
fn expected_tag_keys(tag_count: usize) -> Vec<String> {
    (0..tag_count).map(|index| format!("key-{index:02}")).collect()
}

/// Whether `arn` has the FORM the modelled `SubscriptionProviderArn` pattern requires:
/// `arn:` then four segments of `[a-z0-9-\.]` and a `<type>/<id>` resource whose two
/// halves are drawn from the same class
/// (`^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,510}/[a-z0-9-\.]{1,510}$`,
/// `smithy_query(query="operation",operation="GetRegisteredSubscriptionProvider")`).
/// Checked here rather than taken on trust because a minted ARN that misses the pattern
/// could not be passed back to any operation that addresses the resource.
fn matches_provider_arn_pattern(arn: &str) -> bool {
    let class = |segment: &str, max: usize| {
        !segment.is_empty()
            && segment.len() <= max
            && segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    };
    let Some(rest) = arn.strip_prefix("arn:") else {
        return false;
    };
    let segments: Vec<&str> = rest.split(':').collect();
    let [partition, service, region, account, resource] = segments.as_slice() else {
        return false;
    };
    let Some((kind, id)) = resource.split_once('/') else {
        return false;
    };
    [partition, service, region, account]
        .iter()
        .all(|segment| class(segment, 63))
        && class(kind, 510)
        && class(id, 510)
}

/// Rule 1: a registration creates the resource, and every member of it is readable back.
#[tokio::test]
async fn a_registration_creates_a_readable_subscription_provider() {
    let h = harness();
    let mut created: Vec<(String, String, usize)> = Vec::new();

    for secret_arn in SECRET_ARNS {
        for tag_count in [0_usize, 1, 3, MAX_TAGS] {
            // Each row registers its own secret: a repeat of one already registered is the
            // idempotent replay, which is a rule of its own.
            let secret_arn = &distinct_secret(secret_arn, tag_count);
            let case = format!("register {secret_arn} with {tag_count} tags");
            let registered = register(&h, &request(secret_arn, tag_count)).await;
            let arn = provider_arn(&registered, &case);

            assert!(
                matches_provider_arn_pattern(&arn),
                "case {case:?}: the minted ARN must satisfy the modelled \
                 SubscriptionProviderArn pattern, got {arn:?}"
            );
            assert!(
                arn.starts_with(&format!(
                    "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/"
                )),
                "case {case:?}: expected a subscription-provider ARN of the signed Region \
                 and the emulator account, got {arn:?}"
            );
            assert_eq!(
                member(&registered, &case, "SubscriptionProviderSource"),
                Some(RED_HAT),
                "case {case:?}: the registration must report the source it registered, got {:?}",
                registered.body
            );
            assert_eq!(
                member(&registered, &case, "SubscriptionProviderStatus"),
                Some(PENDING),
                "case {case:?}: a fresh registration must report {PENDING}, got {:?}",
                registered.body
            );
            assert!(
                !created.iter().any(|(stored, _, _)| stored == &arn),
                "case {case:?}: each registration must mint its own ARN, got a repeat of {arn:?}"
            );

            // Read back through the implemented reader: every member is what the
            // registration persisted.
            let got = get_provider(&h, &arn).await;
            for (field, want) in [
                ("SubscriptionProviderArn", arn.as_str()),
                ("SubscriptionProviderSource", RED_HAT),
                ("SecretArn", secret_arn.as_str()),
                ("SubscriptionProviderStatus", PENDING),
            ] {
                assert_eq!(
                    member(&got, &case, field),
                    Some(want),
                    "case {case:?}: expected {field} {want:?} from the read, got {:?}",
                    got.body
                );
            }
            // Nothing was retrieved from the third-party provider, and nothing is wrong
            // with the token while it is still being checked.
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

            // The tags the registration carried are the resource's tag set.
            let tags = list_tags(&h, &arn).await;
            let mut keys: Vec<String> = tags
                .ok(&case)
                .get("tags")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("case {case:?}: no tags in {:?}", tags.body))
                .keys()
                .cloned()
                .collect();
            keys.sort();
            assert_eq!(
                keys,
                expected_tag_keys(tag_count),
                "case {case:?}: expected the registration's tag keys to be readable"
            );

            created.push((arn, secret_arn.clone(), tag_count));
        }
    }

    // And the list reports exactly what was registered, in registration order.
    let case = "list every registration";
    let rows = listed(&list_providers(&h, &json!({})).await, case);
    assert_eq!(
        rows.iter()
            .map(|(arn, source, secret, _)| (arn.clone(), source.clone(), secret.clone()))
            .collect::<Vec<_>>(),
        created
            .iter()
            .map(|(arn, secret, _)| (arn.clone(), RED_HAT.to_string(), secret.clone()))
            .collect::<Vec<_>>(),
        "case {case:?}: the list must report exactly the created registrations, in order"
    );
}

/// Rule 1, continued: the ARN is minted in the Region the request was signed for, so the
/// resource a Region's caller created is the one its reads address.
#[tokio::test]
async fn the_minted_arn_carries_the_signed_region() {
    let h = harness();
    // Rows spanning partitions and Region-code shapes; the signed Region is the only thing
    // that differs between them.
    for (index, region) in ["us-west-2", "eu-west-1", "ap-southeast-7", "us-gov-east-1"]
        .into_iter()
        .enumerate()
    {
        let case = format!("register signed for {region}");
        let registered = call_in_region(
            &h,
            region,
            "/subscription/RegisterSubscriptionProvider",
            &request(&distinct_secret(SECRET_ARNS[0], index), 1),
        )
        .await;
        let arn = provider_arn(&registered, &case);
        assert_eq!(
            arn.split(':').nth(3),
            Some(region),
            "case {case:?}: expected the signed Region in the minted ARN, got {arn:?}"
        );
        assert!(
            matches_provider_arn_pattern(&arn),
            "case {case:?}: the minted ARN must satisfy the modelled pattern, got {arn:?}"
        );
    }
}

/// Rule 2: the transition the clock owes lands on its own, settles into the modelled
/// failure status with a reason, and is terminal.
#[tokio::test]
async fn the_registration_settles_into_the_modelled_failure_status() {
    let h = harness();

    // One registration per `SecretArn` row: the rule holds for every registration, not
    // for one shape of secret.
    let mut arns = Vec::new();
    for secret_arn in SECRET_ARNS {
        let case = format!("register {secret_arn}");
        let arn = provider_arn(&register(&h, &request(secret_arn, 2)).await, &case);

        // Before the transition falls due the registration is PENDING — and READING it
        // does not move it: two reads in a row agree.
        for attempt in 0..2 {
            let case = format!("{case}, read {attempt} before the status is due");
            assert_eq!(
                member(&get_provider(&h, &arn).await, &case, "SubscriptionProviderStatus"),
                Some(PENDING),
                "case {case:?}: a read must not move the status"
            );
        }
        arns.push(arn);
    }

    // Nothing is called while the clock passes the span; the transition is owed anyway.
    wait_for_token_validation();

    for arn in &arns {
        for attempt in 0..2 {
            let case = format!("{arn} settled, read {attempt}");
            let got = get_provider(&h, arn).await;
            assert_eq!(
                member(&got, &case, "SubscriptionProviderStatus"),
                Some(INVALID),
                "case {case:?}: a registration nothing could validate must settle {INVALID}, \
                 got {:?}",
                got.body
            );
            assert_ne!(
                member(&got, &case, "SubscriptionProviderStatus"),
                Some(ACTIVE),
                "case {case:?}: nothing accepted a token, so {ACTIVE} must never be reported"
            );
            let message = member(&got, &case, "SubscriptionProviderStatusMessage")
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: an {INVALID} registration must carry a \
                         SubscriptionProviderStatusMessage, got {:?}",
                        got.body
                    )
                });
            assert!(
                !message.is_empty() && !message.contains(arn.as_str()),
                "case {case:?}: the status message must say why the token was not \
                 validated and carry nothing from the request, got {message:?}"
            );
            // Still nothing retrieved from the provider, so the member stays absent.
            assert!(
                got.ok(&case)
                    .get("LastSuccessfulDataRetrievalTime")
                    .is_none_or(Value::is_null),
                "case {case:?}: expected no LastSuccessfulDataRetrievalTime, got {:?}",
                got.body
            );
        }
    }

    // The list agrees with the reads: the status is a property of the stored record, not
    // of the operation that reports it.
    let case = "list after the registrations settled";
    for (arn, _, _, status) in listed(&list_providers(&h, &json!({})).await, case) {
        assert_eq!(
            status, INVALID,
            "case {case:?}: expected {INVALID} for {arn} from the list, got {status:?}"
        );
    }
}

/// Rule 3: `@idempotent` — a repeat of the same registration is the same resource.
#[tokio::test]
async fn a_repeated_registration_is_the_same_resource() {
    let h = harness();

    // Each row registers the same (source, secret) pair `repeats` times; the number of
    // repeats is what the rule generalises over.
    for (index, repeats) in [2_usize, 3, 5].into_iter().enumerate() {
        let secret_arn = SECRET_ARNS[index];
        let case = format!("{repeats} registrations of {secret_arn}");
        let first = provider_arn(&register(&h, &request(secret_arn, 1)).await, &case);
        let before = listed(&list_providers(&h, &json!({})).await, &case).len();

        for attempt in 1..repeats {
            let case = format!("{case}, attempt {attempt}");
            let again = register(&h, &request(secret_arn, 1)).await;
            assert_eq!(
                provider_arn(&again, &case),
                first,
                "case {case:?}: a repeat must answer the registration the first call created"
            );
            assert_eq!(
                listed(&list_providers(&h, &json!({})).await, &case).len(),
                before,
                "case {case:?}: a repeat must not add a registration"
            );
        }
    }

    // A registration naming a different secret is a registration of its own.
    let case = "a second secret";
    let first = provider_arn(&register(&h, &request(SECRET_ARNS[0], 1)).await, case);
    let other = provider_arn(&register(&h, &request(SECRET_ARNS[3], 1)).await, case);
    assert_ne!(
        first, other,
        "case {case:?}: a registration of another secret must be its own resource"
    );
    assert_eq!(
        member(&get_provider(&h, &other).await, case, "SecretArn"),
        Some(SECRET_ARNS[3]),
        "case {case:?}: the second registration must hold the secret it named"
    );
}

/// Rule 4: the lifecycle the create path opens closes — tag, read, deregister.
#[tokio::test]
async fn the_created_resource_completes_the_tag_and_deregister_lifecycle() {
    let h = harness();

    // Rows differ along the tag set the registration carried and the one a later write
    // adds, including a write that overwrites a registered key.
    let rows: &[(usize, &[(&str, &str)])] = &[
        (0, &[("owner", "platform")]),
        (1, &[("env", "test"), ("team", "linux")]),
        (3, &[("key-00", "overwritten")]),
    ];
    for (index, (tag_count, written)) in rows.iter().enumerate() {
        let case = format!("lifecycle row {index}");
        let arn = provider_arn(&register(&h, &request(SECRET_ARNS[index], *tag_count)).await, &case);

        let map: serde_json::Map<String, Value> = written
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect();
        tag(&h, &arn, &Value::Object(map)).await.ok(&case);

        let tags = list_tags(&h, &arn).await;
        let reported = tags
            .ok(&case)
            .get("tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no tags in {:?}", tags.body))
            .clone();
        for (key, value) in *written {
            assert_eq!(
                reported.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: a tag write on the created resource must be readable back"
            );
        }
        for key in expected_tag_keys(*tag_count) {
            let expected = written
                .iter()
                .find(|(written_key, _)| *written_key == key)
                .map_or_else(
                    || key.replace("key-", "value-"),
                    |(_, value)| (*value).to_string(),
                );
            assert_eq!(
                reported.get(&key).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: expected {key} to hold {expected:?} after the write"
            );
        }

        // Deregister, and the resource is gone for every later caller.
        deregister(&h, &arn).await.ok(&case);
        get_provider(&h, &arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
        list_tags(&h, &arn).await.error(&case, 404, "NotFoundException");
        let remaining = listed(&list_providers(&h, &json!({})).await, &case);
        assert!(
            !remaining.iter().any(|(listed_arn, _, _, _)| listed_arn == &arn),
            "case {case:?}: a deregistered registration must leave the list, got {remaining:?}"
        );

        // And registering the same secret again creates a NEW resource: the ARN the
        // deregistration freed is not handed out twice.
        let again = provider_arn(&register(&h, &request(SECRET_ARNS[index], 0)).await, &case);
        assert_ne!(
            again, arn,
            "case {case:?}: a registration after a deregistration must mint a fresh ARN"
        );
    }
}

/// Rule 5: a repeat whose resultant tag set would pass the modelled maximum is refused,
/// and the stored set is left as it was.
#[tokio::test]
async fn a_repeat_past_the_tag_maximum_is_refused() {
    let h = harness();

    // Rows differ along how the two tag sets add up: exactly at the bound (accepted), one
    // past it, and well past it.
    let rows: &[(usize, usize, bool)] = &[
        (MAX_TAGS - 10, 10, true),
        (MAX_TAGS, 0, true),
        (MAX_TAGS, 1, false),
        (MAX_TAGS - 5, 10, false),
    ];
    for (index, (first, extra, accepted)) in rows.iter().enumerate() {
        let case = format!("{first} registered tags plus {extra} more");
        // A fresh harness per row, so each row's account holds only its own registration.
        let h = if index == 0 { &h } else { &harness() };
        let arn = provider_arn(&register(h, &request(SECRET_ARNS[0], *first)).await, &case);

        // The repeat names the same (source, secret) pair with FURTHER keys, so the
        // resultant set is the union of both.
        let mut repeat = request(SECRET_ARNS[0], 0);
        if *extra > 0 {
            let tags: serde_json::Map<String, Value> = (0..*extra)
                .map(|offset| {
                    let index = first + offset;
                    (format!("key-{index:02}"), json!(format!("value-{index:02}")))
                })
                .collect();
            repeat["Tags"] = Value::Object(tags);
        }
        let reply = register(h, &repeat).await;

        let stored = stored_tag_keys(h, &arn, &case).await;
        if *accepted {
            assert_eq!(
                provider_arn(&reply, &case),
                arn,
                "case {case:?}: a repeat within the limit must answer the same registration"
            );
            assert_eq!(
                stored.len(),
                first + extra,
                "case {case:?}: expected the union of both tag sets, got {stored:?}"
            );
        } else {
            reply.error(&case, 400, "BadRequestException");
            assert_eq!(
                reply.message(),
                "The resultant tag set must not have more than 50 user tags.",
                "case {case:?}: expected the modelled tag limit stated, got {:?}",
                reply.message()
            );
            assert_eq!(
                stored.len(),
                *first,
                "case {case:?}: a refused repeat must leave the stored set as it was, got \
                 {stored:?}"
            );
        }
    }
}

/// The tag keys the resource `arn` carries, read back over the wire.
async fn stored_tag_keys(h: &Harness, arn: &str, case: &str) -> Vec<String> {
    let reply: Reply = list_tags(h, arn).await;
    let mut keys: Vec<String> = reply
        .ok(case)
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags in {:?}", reply.body))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Rule 6: the store does not grow past the registrations an account may hold.
///
/// Every registration up to the ceiling is taken; the next one is refused with the
/// operation's modelled error and changes nothing — and a repeat of a registration the
/// account already holds is still accepted at the ceiling, because it adds no record
/// (rule 3). Deregistering one makes room for exactly one more.
#[tokio::test]
async fn a_registration_past_the_ceiling_is_refused() {
    let h = harness();
    let secret = |index: usize| format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:t-{index}");

    let mut arns = Vec::new();
    for index in 0..MAX_REGISTERED_PROVIDERS {
        let case = format!("registration {index}");
        arns.push(provider_arn(&register(&h, &request(&secret(index), 0)).await, &case));
    }
    assert_eq!(
        stored_provider_count(&h),
        MAX_REGISTERED_PROVIDERS,
        "every registration up to the ceiling must be taken"
    );

    // Past the ceiling: refused, and nothing is written. The rows differ along the secret
    // the refused registration names.
    for index in [MAX_REGISTERED_PROVIDERS, MAX_REGISTERED_PROVIDERS + 7] {
        let case = format!("registration {index}, past the ceiling");
        let reply = register(&h, &request(&secret(index), 1)).await;
        reply.error(&case, 400, "ValidationException");
        assert!(
            reply.message().contains(&MAX_REGISTERED_PROVIDERS.to_string()),
            "case {case:?}: the refusal must state the ceiling, got {:?}",
            reply.message()
        );
        assert_eq!(
            stored_provider_count(&h),
            MAX_REGISTERED_PROVIDERS,
            "case {case:?}: a refused registration must leave the store as it was"
        );
    }

    // A repeat of one the account already holds adds no record, so it is still accepted.
    let case = "repeat at the ceiling";
    assert_eq!(
        provider_arn(&register(&h, &request(&secret(0), 0)).await, case),
        arns[0],
        "case {case:?}: a repeat must answer the registration already made"
    );

    // And deregistering one makes room for exactly one more.
    let case = "register after a deregistration";
    deregister(&h, &arns[0]).await.ok(case);
    let fresh = provider_arn(
        &register(&h, &request(&secret(MAX_REGISTERED_PROVIDERS), 0)).await,
        case,
    );
    assert!(
        !arns.contains(&fresh),
        "case {case:?}: the registration that followed a deregistration must be its own \
         resource, got {fresh:?}"
    );
    register(&h, &request(&secret(MAX_REGISTERED_PROVIDERS + 1), 0))
        .await
        .error(case, 400, "ValidationException");
}
