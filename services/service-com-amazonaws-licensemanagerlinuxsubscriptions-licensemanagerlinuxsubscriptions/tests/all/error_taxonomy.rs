// SPDX-License-Identifier: CC0-1.0

//! This service's ERROR TAXONOMY and its settled-status reporting, over the real wire
//! path: every call goes through the router's `dispatch`, so a refusal the framework
//! states before any handler runs is observed exactly as a client sees it.
//!
//! The rules under test, each over a SET of inputs rather than the one request that
//! exposed it:
//!
//! 1. a settled change to the service settings reports the SETTLED `Status` the service
//!    reports (`Completed`), from the call that applied it and from every later read;
//! 2. a `SourceRegions` entry that does not name an AWS Region is refused with
//!    `ValidationException` naming that entry, and the account's settings are left
//!    unchanged;
//! 3. a registration past the modelled `Tags` limit is refused with the tag APIs' own
//!    `BadRequestException` and registers nothing;
//! 4. a `@pattern` violation quotes the modelled expression WITHOUT its `^` / `$`
//!    anchors;
//! 5. an enum violation on an ELEMENT of a list is reported at the LIST member, with the
//!    element's constraint nested;
//! 6. the tag APIs answer their own taxonomy in ordered stages, each with the HTTP
//!    status that goes with its code: HTTP 400 `BadRequestException` for a `resourceArn`
//!    outside the modelled grammar, HTTP 403 `AccessDeniedException` for one naming another
//!    account, HTTP 404 `NotFoundException` for one that addresses
//!    no resource of this account, and the tag removal they perform is readable back
//!    (`tag_plane_authorization.rs` owns the denial stage in full);
//! 7. a not-found refusal on a subscription-provider ARN carries FIXED text and nothing
//!    from the request, and is idempotent.

use serde_json::{json, Value};

use crate::harness::{
    deregister, get_provider, get_settings, harness, list_providers, list_tags,
    provider_arn_for, register, seed_provider, stored_provider_count, untag, update_settings,
    Reply, ACCOUNT, RED_HAT, REGION, SIGNING_NAME,
};

/// The `Status` a settled change reports, and the value this emulator used to report in
/// its place. Both are members of the modelled `Status` enum
/// (`smithy_query(query="shape",shape="Status")`: `InProgress | Completed | Successful |
/// Failed`), which is why the wrong one went unnoticed.
const SETTLED: &str = "Completed";
const NOT_SETTLED: &str = "Successful";

/// The modelled `@pattern` of `SubscriptionProviderArn`, with the anchors the model
/// spells it with stripped — the expression a refusal must quote
/// (`smithy_query(query="shape",shape="UntagResourceRequest")`).
const PROVIDER_ARN_PATTERN: &str = r"arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,510}/[a-z0-9-\.]{1,510}";
/// The modelled `@pattern` of `SecretArn`, unanchored
/// (`smithy_query(query="operation",operation="RegisterSubscriptionProvider")`).
const SECRET_ARN_PATTERN: &str =
    r"arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}";

/// The tagging surface's text for a resource ARN it cannot parse.
const BAD_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";
/// The tagging surface's text for a resource ARN that names nothing.
const TAG_NOT_FOUND_MESSAGE: &str = "Resource was not found";
/// The fixed text a subscription-provider not-found refusal carries.
const PROVIDER_NOT_FOUND_MESSAGE: &str = "Registered Subscription Provider not found";

/// A settings request for `regions`, with both switches set.
fn settings_request(discovery: &str, regions: &[&str]) -> Value {
    json!({
        "LinuxSubscriptionsDiscovery": discovery,
        "LinuxSubscriptionsDiscoverySettings": {
            "SourceRegions": regions,
            "OrganizationIntegration": "Disabled",
        },
        "AllowUpdate": true,
    })
}

/// The `Status` member of a settings body.
fn status_of(reply: &Reply, case: &str) -> String {
    reply
        .ok(case)
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Status in {:?}", reply.body))
        .to_string()
}

/// Rule 1: the call that applies a change, and every later read, report the settled
/// status — for an enable, for a disable, and for a repeat of either.
#[tokio::test]
async fn a_settled_settings_change_reports_the_settled_status() {
    let h = harness();
    // The rule generalises over the switch value and over the sequence of changes, so
    // the table walks both: enable, repeat, disable, re-enable, repeat disable.
    for (step, discovery) in [
        "Enabled", "Enabled", "Disabled", "Enabled", "Disabled", "Disabled",
    ]
    .iter()
    .enumerate()
    {
        let case = format!("step {step}: discovery {discovery}");
        let updated = update_settings(&h, &settings_request(discovery, &["us-west-2"])).await;
        let from_update = status_of(&updated, &case);
        let fetched = get_settings(&h).await;
        let from_read = status_of(&fetched, &case);
        for (source, actual) in [("update", &from_update), ("get", &from_read)] {
            assert_eq!(
                actual, SETTLED,
                "case {case:?}: {source} must report the settled Status {SETTLED}, got {actual:?}"
            );
            assert_ne!(
                actual, NOT_SETTLED,
                "case {case:?}: {source} must not report {NOT_SETTLED}"
            );
        }
        assert_eq!(
            fetched
                .body
                .get("LinuxSubscriptionsDiscovery")
                .and_then(Value::as_str),
            Some(*discovery),
            "case {case:?}: the read must report the switch the update persisted, got {:?}",
            fetched.body
        );
    }
}

/// Rule 2: every `SourceRegions` entry must name an AWS Region.
///
/// The accepted rows are entries of `state::KNOWN_REGIONS` the issue did not cite,
/// across partitions; the refused rows are values outside that catalog, each refused by
/// naming THAT entry, whichever position it sits at.
#[tokio::test]
async fn a_source_region_outside_the_region_catalog_is_refused() {
    let h = harness();

    // Accepted: single and multi-entry lists of real Regions, including GovCloud and
    // China partition codes and a Region the issue never mentions.
    let accepted: &[&[&str]] = &[
        &["il-central-1"],
        &["ap-southeast-7"],
        &["cn-north-1", "cn-northwest-1"],
        &["us-gov-east-1", "us-gov-west-1"],
        &["mx-central-1", "ca-west-1", "eu-south-2"],
    ];
    for regions in accepted {
        let case = format!("accepted {regions:?}");
        let body = update_settings(&h, &settings_request("Enabled", regions))
            .await
            .ok(&case)
            .clone();
        let settings = body
            .get("LinuxSubscriptionsDiscoverySettings")
            .unwrap_or_else(|| panic!("case {case:?}: no discovery settings in {body:?}"));
        assert_eq!(
            settings.get("SourceRegions").and_then(Value::as_array),
            Some(&regions.iter().map(|r| json!(r)).collect::<Vec<_>>()),
            "case {case:?}: expected the persisted SourceRegions, got {settings:?}"
        );
    }

    // What the last accepted update left behind: a refusal must not change it.
    let before = get_settings(&h).await.ok("settings before a refusal").clone();

    // Refused: a value outside the catalog, at the head, the middle and the tail of the
    // list, plus values that are near-misses rather than nonsense.
    let refused: &[(&[&str], &str)] = &[
        (&["not-a-real-region-9999"], "not-a-real-region-9999"),
        (&["us-east-1", "us-west-3"], "us-west-3"),
        (&["eu-fake-1", "us-east-1"], "eu-fake-1"),
        (&["us-east-1", "US-WEST-2", "eu-west-1"], "US-WEST-2"),
        (&["us-east"], "us-east"),
        (&["us-east-1.", "eu-west-1"], "us-east-1."),
    ];
    for (regions, offender) in refused {
        let case = format!("refused {regions:?}");
        let reply = update_settings(&h, &settings_request("Enabled", regions)).await;
        reply.error(&case, 400, "ValidationException");
        assert_eq!(
            reply.message(),
            format!("Invalid region name/format : {offender}"),
            "case {case:?}: the refusal must name the offending entry, got {:?}",
            reply.message()
        );
        let after = get_settings(&h).await.ok(&case).clone();
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the settings unchanged"
        );
    }
}

/// Rule 3: a registration carrying a tag set past the modelled `Tags` maximum is refused
/// with the tagging surface's own code, and leaves nothing registered.
///
/// The ACCEPTED create path (and the statuses it reports) is
/// `tests/all/subscription_provider_registration.rs`; what this pins is the refusal and
/// that it writes nothing. The rows differ along the secret ARN's grammar (partition,
/// empty region/account, the `[^/]` secret-name class) and along how far past the bound
/// the tag set sits.
#[tokio::test]
async fn an_over_limit_tag_set_is_refused_and_registers_nothing() {
    let h = harness();

    let secret_arns = [
        "arn:aws:secretsmanager:us-east-1:123456789012:secret:rhel-token",
        "arn:aws-us-gov:secretsmanager:us-gov-west-1:123456789012:secret:rhsm-offline",
        "arn:aws:secretsmanager:::secret:token",
        "arn:aws:secretsmanager:eu-west-1:000000000000:secret:RedHat_Offline+Token=1",
    ];
    // One past the modelled `Tags` maximum (`@length 0..50`), and well past it.
    for tag_count in [51_usize, 60, 120] {
        for secret_arn in secret_arns {
            let case = format!("register {secret_arn} with {tag_count} tags");
            let tags: serde_json::Map<String, Value> = (0..tag_count)
                .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
                .collect();
            let reply = register(
                &h,
                &json!({
                    "SubscriptionProviderSource": RED_HAT,
                    "SecretArn": secret_arn,
                    "Tags": Value::Object(tags),
                }),
            )
            .await;
            reply.error(&case, 400, "BadRequestException");
            assert_eq!(
                reply.message(),
                "The resultant tag set must not have more than 50 user tags.",
                "case {case:?}: expected the modelled tag limit stated, got {:?}",
                reply.message()
            );
            assert_eq!(
                stored_provider_count(&h),
                0,
                "case {case:?}: a refused registration must leave the store empty"
            );
            let listed = list_providers(&h, &json!({})).await;
            assert_eq!(
                listed
                    .ok(&case)
                    .get("RegisteredSubscriptionProviders")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0),
                "case {case:?}: the list must report no registration, got {:?}",
                listed.body
            );
        }
    }
}

/// Rule 4: a `@pattern` violation quotes the expression without the model's anchors.
///
/// One row per pattern-constrained member this service's inputs reach through a body
/// member, each with a value outside the expression.
#[tokio::test]
async fn a_pattern_violation_quotes_the_expression_without_its_anchors() {
    let h = harness();
    let bad_arn = "not-an-arn-at-all";
    let short_arn = "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:provider";

    // (case, reply, the offending value, the member path, the unanchored expression)
    let rows: Vec<(&str, Reply, &str, &str, &str)> = vec![
        (
            "GetRegisteredSubscriptionProvider",
            get_provider(&h, bad_arn).await,
            bad_arn,
            "subscriptionProviderArn",
            PROVIDER_ARN_PATTERN,
        ),
        (
            "GetRegisteredSubscriptionProvider, no resource id",
            get_provider(&h, short_arn).await,
            short_arn,
            "subscriptionProviderArn",
            PROVIDER_ARN_PATTERN,
        ),
        (
            "DeregisterSubscriptionProvider",
            deregister(&h, bad_arn).await,
            bad_arn,
            "subscriptionProviderArn",
            PROVIDER_ARN_PATTERN,
        ),
        (
            "RegisterSubscriptionProvider",
            register(
                &h,
                &json!({
                    "SubscriptionProviderSource": RED_HAT,
                    "SecretArn": "not-a-valid-secret-arn",
                }),
            )
            .await,
            "not-a-valid-secret-arn",
            "secretArn",
            SECRET_ARN_PATTERN,
        ),
        (
            "RegisterSubscriptionProvider, wrong service",
            register(
                &h,
                &json!({
                    "SubscriptionProviderSource": RED_HAT,
                    "SecretArn": "arn:aws:kms:us-east-1:000000000000:secret:k",
                }),
            )
            .await,
            "arn:aws:kms:us-east-1:000000000000:secret:k",
            "secretArn",
            SECRET_ARN_PATTERN,
        ),
    ];

    for (case, reply, value, member, pattern) in rows {
        reply.error(case, 400, "ValidationException");
        let expected = format!(
            "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {pattern}"
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
        // The point of the rule, stated on its own: the anchored form the model carries
        // never reaches the wire.
        let quoted = reply
            .message()
            .rsplit_once("pattern: ")
            .map(|(_, tail)| tail.to_string())
            .unwrap_or_default();
        assert!(
            !quoted.starts_with('^') && !quoted.ends_with('$'),
            "case {case:?}: a quoted pattern must carry no anchors, got {quoted:?}"
        );
    }
}

/// Rule 5: an enum violation on a list ELEMENT is reported at the list member, with the
/// element constraint nested.
#[tokio::test]
async fn an_enum_violation_in_a_list_is_reported_at_the_list_member() {
    let h = harness();
    // Rows differ along the offending value and the number of offending elements, which
    // is what the rule generalises over. The sole modelled value is `RedHat`
    // (`smithy_query(query="shape",shape="SubscriptionProviderSource")`).
    let rows: &[(&[&str], &str)] = &[
        (&["NotARealProvider"], "[NotARealProvider]"),
        (&["SUSE"], "[SUSE]"),
        // The enum is case-sensitive, so a differently-cased spelling is outside it.
        (&["redhat"], "[redhat]"),
        (&["SUSE", "Oracle"], "[SUSE, Oracle]"),
    ];
    for (sources, rendered) in rows {
        let case = format!("sources {sources:?}");
        let reply = list_providers(&h, &json!({ "SubscriptionProviderSources": sources })).await;
        reply.error(&case, 400, "ValidationException");
        let expected = format!(
            "1 validation error detected: Value '{rendered}' at 'subscriptionProviderSources' \
             failed to satisfy constraint: Member must satisfy constraint: \
             [Member must satisfy enum value set: [RedHat]]"
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }

    // A list mixing a modelled value with one outside the enum is reported at the same
    // member, with the same nesting, and the element index is never part of the path.
    //
    // The quoted value is the collection formed from the elements that FAILED — here
    // `[SUSE]`, not the whole `[RedHat, SUSE]` the caller sent.
    //
    // KNOWN GAP, recorded in the issue report and on `Clause::render`: the structured
    // refusal the framework hands a service carries one violation per OFFENDING element,
    // so the renderer cannot see the elements that passed. Closing it needs `Violation`
    // (vendor/smithy-schema-server/src/constraints.rs) to carry the collection value it
    // was built from; WHEN THAT LANDS, this row's expectation becomes
    // `Value '[RedHat, SUSE]'`. The assertion is exact so the gap is visible here rather
    // than hidden behind a loose match.
    let case = "sources [RedHat, SUSE]";
    let reply = list_providers(
        &h,
        &json!({ "SubscriptionProviderSources": [RED_HAT, "SUSE"] }),
    )
    .await;
    reply.error(case, 400, "ValidationException");
    let expected = "1 validation error detected: Value '[SUSE]' at 'subscriptionProviderSources' \
         failed to satisfy constraint: Member must satisfy constraint: \
         [Member must satisfy enum value set: [RedHat]]";
    assert_eq!(
        reply.message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        reply.message()
    );
    assert!(
        !reply.message().contains(".member"),
        "case {case:?}: the element path must not be reported, got {:?}",
        reply.message()
    );
}

/// Rule 6: the tag APIs' two ordered stages, and the tag removal itself.
#[tokio::test]
async fn the_tag_apis_separate_a_bad_arn_from_a_missing_resource() {
    let h = harness();

    // Stage 1 — FORMAT: values outside the modelled `resourceArn` grammar. The rows span
    // the ways a value can miss it: nothing ARN-shaped at all, too few segments, no
    // resource id, and a character outside the segment class.
    let unparseable = [
        "not-an-arn-at-all",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:SUBSCRIPTION/abc",
    ];
    for arn in unparseable {
        for (op, reply) in [
            ("UntagResource", untag(&h, arn, &["a", "b"]).await),
            ("ListTagsForResource", list_tags(&h, arn).await),
        ] {
            let case = format!("{op} with {arn}");
            reply.error(&case, 400, "BadRequestException");
            assert_eq!(
                reply.message(),
                BAD_ARN_MESSAGE,
                "case {case:?}: expected {BAD_ARN_MESSAGE:?}, got {:?}",
                reply.message()
            );
        }
    }

    // Stage 2 — AUTHORIZATION: the same two shapes of ARN with the account field naming
    // another account. Decided from the ARN, so it is the answer whether or not this account
    // holds a resource under that id, and it names the calling principal rather than
    // anything the request carried.
    let foreign = [
        format!("arn:aws:{SIGNING_NAME}:{REGION}:123456789012:subscription-provider/other"),
        format!("arn:aws:{SIGNING_NAME}:eu-central-1:999999999999:subscription-provider/other"),
    ];
    let denial = format!(
        "User: {} is not authorized to access this resource",
        service_common::caller_principal::ambient_user_arn()
    );
    for arn in &foreign {
        for (op, reply) in [
            ("UntagResource", untag(&h, arn, &["a"]).await),
            ("ListTagsForResource", list_tags(&h, arn).await),
        ] {
            let case = format!("{op} with {arn}");
            reply.error(&case, 403, "AccessDeniedException");
            assert_eq!(
                reply.message(),
                denial,
                "case {case:?}: expected {denial:?}, got {:?}",
                reply.message()
            );
        }
    }

    // Stage 3 — EXISTENCE: values inside that grammar which name THIS account and address
    // no resource of it, including ARNs of another resource kind and another service. (An
    // ARN naming another account is denied at the stage before this one, so it is not a row
    // here.)
    let unregistered = [
        provider_arn_for("55555555-5555-5555-5555-555555555555"),
        provider_arn_for("00000000-0000-4000-8000-000000000001"),
        format!("arn:aws:{SIGNING_NAME}:eu-central-1:{ACCOUNT}:subscription-provider/other"),
        format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:linux-subscription/abc"),
        "arn:aws:iam:us-west-2:000000000000:role/not-a-provider".to_string(),
    ];
    for arn in &unregistered {
        for (op, reply) in [
            ("UntagResource", untag(&h, arn, &["a"]).await),
            ("ListTagsForResource", list_tags(&h, arn).await),
        ] {
            let case = format!("{op} with {arn}");
            reply.error(&case, 404, "NotFoundException");
            assert_eq!(
                reply.message(),
                TAG_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {TAG_NOT_FOUND_MESSAGE:?}, got {:?}",
                reply.message()
            );
        }
    }

    // And the removal itself, read back through the implemented reader. The rows differ
    // along how many of the stored keys the request names, including none and all.
    let stored = [("owner", "platform"), ("env", "test"), ("team", "linux")];
    let removals: &[&[&str]] = &[
        &["owner"],
        &["env", "team"],
        // A key the resource does not carry is not an error.
        &["absent"],
        &["owner", "absent"],
        &["owner", "env", "team"],
    ];
    for (index, remove) in removals.iter().enumerate() {
        let case = format!("remove {remove:?}");
        let arn = seed_provider(
            &h,
            &format!("00000000-0000-4000-8000-{:012x}", index + 100),
            "arn:aws:secretsmanager:us-east-1:123456789012:secret:rhel-token",
            &stored,
        );
        untag(&h, &arn, remove).await.ok(&case);
        let left = list_tags(&h, &arn).await;
        let tags = left
            .ok(&case)
            .get("tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no tags in {:?}", left.body))
            .clone();
        let mut expected: Vec<&str> = stored
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| !remove.contains(key))
            .collect();
        expected.sort_unstable();
        let mut actual: Vec<&str> = tags.keys().map(String::as_str).collect();
        actual.sort_unstable();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the keys the request did not name to remain"
        );
        for key in &actual {
            assert_eq!(
                tags.get(*key).and_then(Value::as_str),
                stored
                    .iter()
                    .find(|(stored_key, _)| stored_key == key)
                    .map(|(_, value)| *value),
                "case {case:?}: the value of {key} must be the stored one"
            );
        }
    }
}

/// Rule 7: the not-found refusal on a subscription-provider ARN is fixed text, carries
/// nothing from the request, and repeats identically.
#[tokio::test]
async fn a_not_found_provider_is_refused_with_fixed_text() {
    let h = harness();
    // Rows differ along the ARN's region, account and resource id, none of which may
    // show up in the refusal.
    let unknown = [
        provider_arn_for("22222222-2222-2222-2222-222222222222"),
        provider_arn_for("00000000-0000-4000-8000-000000000009"),
        format!("arn:aws:{SIGNING_NAME}:eu-central-1:123456789012:subscription-provider/other"),
        "arn:aws:iam:us-west-2:000000000000:role/not-a-provider".to_string(),
    ];
    for arn in &unknown {
        for attempt in 0..2 {
            for (op, reply) in [
                ("GetRegisteredSubscriptionProvider", get_provider(&h, arn).await),
                ("DeregisterSubscriptionProvider", deregister(&h, arn).await),
            ] {
                let case = format!("{op} attempt {attempt} with {arn}");
                reply.error(&case, 400, "ResourceNotFoundException");
                assert_eq!(
                    reply.message(),
                    PROVIDER_NOT_FOUND_MESSAGE,
                    "case {case:?}: expected {PROVIDER_NOT_FOUND_MESSAGE:?}, got {:?}",
                    reply.message()
                );
                let resource = arn.rsplit('/').next().unwrap_or(arn);
                assert!(
                    !reply.message().contains(resource),
                    "case {case:?}: the refusal must carry nothing from the request, got {:?}",
                    reply.message()
                );
            }
        }
    }
}
