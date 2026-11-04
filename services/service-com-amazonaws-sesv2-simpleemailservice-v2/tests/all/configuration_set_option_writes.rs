// SPDX-License-Identifier: CC0-1.0

//! The per-option writes of a configuration set, and what a read of the set reports
//! afterwards, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rules these cases pin:
//!
//! * a read of a configuration set reports an option object ONLY where a write
//!   configured it — the delivery configuration of a set nobody configured is absent
//!   from the read rather than reported as the policy a send would fall back to — and
//!   once a write configures it the read reports exactly the members that write carried;
//! * each per-option write persists what it carried under the set it names, is refused
//!   with the modelled `NotFoundException` for a name the account holds no set under (and
//!   for a dedicated IP pool the account does not hold), and leaves every option it does
//!   not carry as it was;
//! * a refusal naming a resource the account already holds, or an event destination a set
//!   does not hold, answers the service's own fixed sentence with the name from the
//!   request interpolated into it;
//! * the event types of a destination are reported in SES's own order, ascending by name,
//!   whatever order the definition listed them in.
//!
//! The tables span what the rules generalise over: which option object a write carries,
//! which members are inside it (each on its own, together, and at each published bound of
//! `MaxDeliverySeconds` plus one past it), the whole declared value set of the enums
//! reached (`TlsPolicy`, `FeatureStatus`), the order and count of the event types sent,
//! and the form of the configuration-set name a refusal interpolates.

use serde_json::{json, Value};

use crate::configuration_set_event_destinations::{reported_destination, EventDestinations};
use crate::configuration_sets::{Reply, Ses};

/// Every value `TlsPolicy` declares, from the model's own enum
/// (`smithy_query(query="shape",shape="TlsPolicy")`).
const TLS_POLICIES: [&str; 2] = ["OPTIONAL", "REQUIRE"];

/// Every value `FeatureStatus` declares, from the model's own enum
/// (`smithy_query(query="shape",shape="FeatureStatus")`).
const FEATURE_STATUSES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The published bounds of `MaxDeliverySeconds`, from the member's own range
/// (`smithy_query(query="operation",operation="PutConfigurationSetDeliveryOptions")`:
/// range 300..50400).
const MIN_DELIVERY_SECONDS: i64 = 300;
const MAX_DELIVERY_SECONDS: i64 = 50400;

/// The per-option writes of a configuration set, over the shared harness.
trait OptionWrites {
    async fn put_delivery_options(&self, set: &str, body: Value) -> Reply;
    async fn put_reputation_options(&self, set: &str, body: Value) -> Reply;
    async fn put_vdm_options(&self, set: &str, body: Value) -> Reply;
    async fn update_set(&self, body: Value) -> Reply;
    async fn create_pool(&self, pool_name: &str) -> Reply;
}

impl OptionWrites for Ses {
    /// `PutConfigurationSetDeliveryOptions`' route: `PUT
    /// /v2/email/configuration-sets/{ConfigurationSetName}/delivery-options`.
    async fn put_delivery_options(&self, set: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{set}/delivery-options"),
            Some(body),
        )
        .await
    }

    /// `PutConfigurationSetReputationOptions`' route: `PUT
    /// /v2/email/configuration-sets/{ConfigurationSetName}/reputation-options`.
    async fn put_reputation_options(&self, set: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{set}/reputation-options"),
            Some(body),
        )
        .await
    }

    /// `PutConfigurationSetVdmOptions`' route: `PUT
    /// /v2/email/configuration-sets/{ConfigurationSetName}/vdm-options`.
    async fn put_vdm_options(&self, set: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{set}/vdm-options"),
            Some(body),
        )
        .await
    }

    /// `UpdateConfigurationSet`'s route: `POST /v2/email/update-configuration-sets`,
    /// which carries the `ConfigurationSetName` in the body.
    async fn update_set(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/update-configuration-sets", Some(body))
            .await
    }

    /// `CreateDedicatedIpPool`'s route: `POST /v2/email/dedicated-ip-pools`.
    async fn create_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/dedicated-ip-pools",
            Some(json!({"PoolName": pool_name})),
        )
        .await
    }
}

/// Create the configuration set `name`, which must answer 200.
async fn create_set(ses: &Ses, name: &str) {
    let created = ses.create_set(json!({"ConfigurationSetName": name})).await;
    assert_eq!(
        created.status,
        200,
        "creating {name:?}: expected 200, got {} ({})",
        created.status,
        created.message()
    );
}

/// The reported configuration set `name`, which must be a 200.
async fn read_set(ses: &Ses, name: &str) -> Reply {
    let read = ses.get_set(name).await;
    assert_eq!(
        read.status,
        200,
        "reading {name:?}: expected 200, got {} ({})",
        read.status,
        read.message()
    );
    read
}

/// One row of the delivery-options table: the body the write carries, and the
/// `DeliveryOptions` a read must report afterwards.
struct DeliveryCase {
    case: &'static str,
    set: &'static str,
    body: Value,
    reported: Value,
}

/// The rows of the delivery-options table: each member on its own, the members together,
/// each published bound of `MaxDeliverySeconds`, and every declared `TlsPolicy`.
fn delivery_cases() -> Vec<DeliveryCase> {
    let mut cases = vec![
        DeliveryCase {
            case: "the delivery bound alone, at the published minimum",
            set: "delivery-min",
            body: json!({"MaxDeliverySeconds": MIN_DELIVERY_SECONDS}),
            reported: json!({"MaxDeliverySeconds": MIN_DELIVERY_SECONDS}),
        },
        DeliveryCase {
            case: "the delivery bound alone, at the published maximum",
            set: "delivery-max",
            body: json!({"MaxDeliverySeconds": MAX_DELIVERY_SECONDS}),
            reported: json!({"MaxDeliverySeconds": MAX_DELIVERY_SECONDS}),
        },
        DeliveryCase {
            case: "both members together",
            set: "delivery_both_9",
            body: json!({"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800}),
            reported: json!({"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800}),
        },
    ];
    // One row per declared `TlsPolicy`, so the whole value set is written and read back
    // rather than one chosen value.
    for policy in TLS_POLICIES {
        cases.push(DeliveryCase {
            case: "the TLS policy alone",
            set: match policy {
                "REQUIRE" => "delivery-tls-require",
                _ => "delivery-tls-optional",
            },
            body: json!({"TlsPolicy": policy}),
            reported: json!({"TlsPolicy": policy}),
        });
    }
    cases
}

#[tokio::test]
async fn delivery_options_are_reported_only_where_a_write_configured_them() {
    let ses = Ses::new();

    for case in delivery_cases() {
        create_set(&ses, case.set).await;

        // Before any write of the delivery configuration the set carries none, so the
        // read reports no member at all — not the policy a send would fall back to.
        let before = read_set(&ses, case.set).await;
        assert_eq!(
            before.member("DeliveryOptions"),
            Value::Null,
            "case {:?}: a set nobody configured delivery for must report no DeliveryOptions, \
             got {:?}",
            case.case,
            before.member("DeliveryOptions")
        );

        let written = ses.put_delivery_options(case.set, case.body.clone()).await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: expected 200 from PutConfigurationSetDeliveryOptions, got {} ({})",
            case.case,
            written.status,
            written.message()
        );
        assert!(
            written.is_empty_body(),
            "case {:?}: PutConfigurationSetDeliveryOptionsResponse declares no members, got {:?}",
            case.case,
            written.member("DeliveryOptions")
        );

        let after = read_set(&ses, case.set).await;
        assert_eq!(
            after.member("DeliveryOptions"),
            case.reported,
            "case {:?}: expected DeliveryOptions {:?}, got {:?}",
            case.case,
            case.reported,
            after.member("DeliveryOptions")
        );

        // A read reports the set without changing it, so a second read answers the same
        // configuration.
        let again = read_set(&ses, case.set).await;
        assert_eq!(
            again.member("DeliveryOptions"),
            case.reported,
            "case {:?}: a read must not change what a later reader sees, got {:?}",
            case.case,
            again.member("DeliveryOptions")
        );
    }
}

#[tokio::test]
async fn a_delivery_bound_outside_the_published_range_is_refused_and_stores_nothing() {
    let ses = Ses::new();
    let set = "delivery-bounds";
    create_set(&ses, set).await;

    // One second past each published bound: the member's range is 300..=50400, so these
    // two values are the nearest ones outside it.
    for (case, seconds) in [
        ("one second below the published minimum", MIN_DELIVERY_SECONDS - 1),
        ("one second above the published maximum", MAX_DELIVERY_SECONDS + 1),
    ] {
        let refused = ses
            .put_delivery_options(set, json!({"MaxDeliverySeconds": seconds}))
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException for {seconds}, got {} ({})",
            refused.status,
            refused.message()
        );
        let read = read_set(&ses, set).await;
        assert_eq!(
            read.member("DeliveryOptions"),
            Value::Null,
            "case {case:?}: a refused write must store nothing, got {:?}",
            read.member("DeliveryOptions")
        );
    }
}

#[tokio::test]
async fn a_delivery_write_carrying_no_members_leaves_the_set_with_none() {
    let ses = Ses::new();
    let set = "delivery-cleared";
    create_set(&ses, set).await;

    let written = ses
        .put_delivery_options(set, json!({"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 900}))
        .await;
    assert_eq!(
        written.status,
        200,
        "writing the delivery configuration: expected 200, got {} ({})",
        written.status,
        written.message()
    );
    assert_eq!(
        read_set(&ses, set).await.member("DeliveryOptions"),
        json!({"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 900}),
        "the written delivery configuration must be read back"
    );

    // The members are the whole state this write owns, so a write that states none of
    // them leaves the set carrying no delivery configuration — the same reading the VDM
    // write takes of a request that states no preferences, and what R1 reports.
    let cleared = ses.put_delivery_options(set, json!({})).await;
    assert_eq!(
        cleared.status,
        200,
        "a write carrying no members: expected 200, got {} ({})",
        cleared.status,
        cleared.message()
    );
    assert_eq!(
        read_set(&ses, set).await.member("DeliveryOptions"),
        Value::Null,
        "a write carrying no members must leave the set with no DeliveryOptions"
    );

    // The same reading on the create path: an option object that configures nothing is a
    // set that carries no delivery configuration.
    let created = ses
        .create_set(json!({"ConfigurationSetName": "delivery-empty", "DeliveryOptions": {}}))
        .await;
    assert_eq!(
        created.status,
        200,
        "creating with an empty delivery object: expected 200, got {} ({})",
        created.status,
        created.message()
    );
    assert_eq!(
        read_set(&ses, "delivery-empty").await.member("DeliveryOptions"),
        Value::Null,
        "a create whose delivery object configures nothing must report no DeliveryOptions"
    );
}

#[tokio::test]
async fn the_sending_pool_a_write_names_must_be_one_the_account_holds() {
    let ses = Ses::new();
    let set = "pool-association";
    create_set(&ses, set).await;
    let pool = "warm-pool";

    // The pool does not exist yet, so the association names a resource that is not there
    // and the set is left as it was.
    let refused = ses
        .put_delivery_options(set, json!({"SendingPoolName": pool}))
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (404, "NotFoundException"),
        "a pool the account does not hold: expected 404 NotFoundException, got {} ({})",
        refused.status,
        refused.message()
    );
    assert_eq!(
        refused.message(),
        format!("The requested pool <{pool}> does not exist."),
        "a pool the account does not hold: unexpected refusal sentence {:?}",
        refused.message()
    );
    let read = read_set(&ses, set).await;
    assert_eq!(
        read.member("DeliveryOptions"),
        Value::Null,
        "a refused association must store nothing, got {:?}",
        read.member("DeliveryOptions")
    );

    // With the pool in the account the same association is accepted and read back.
    assert_eq!(
        ses.create_pool(pool).await.status,
        200,
        "creating the dedicated IP pool must answer 200"
    );
    let written = ses
        .put_delivery_options(set, json!({"TlsPolicy": "REQUIRE", "SendingPoolName": pool}))
        .await;
    assert_eq!(
        written.status,
        200,
        "associating the pool: expected 200, got {} ({})",
        written.status,
        written.message()
    );
    let read = read_set(&ses, set).await;
    assert_eq!(
        read.member("DeliveryOptions"),
        json!({"TlsPolicy": "REQUIRE", "SendingPoolName": pool}),
        "the association must be read back, got {:?}",
        read.member("DeliveryOptions")
    );

    // The same rule on the create path: a set cannot be created associated with a pool
    // the account does not hold, and nothing is stored under its name.
    let created = ses
        .create_set(json!({
            "ConfigurationSetName": "pool-at-create",
            "DeliveryOptions": {"SendingPoolName": "no-such-pool"},
        }))
        .await;
    assert_eq!(
        (created.status, created.error_code().as_str()),
        (404, "NotFoundException"),
        "a create naming an absent pool: expected 404 NotFoundException, got {} ({})",
        created.status,
        created.message()
    );
    let read = ses.get_set("pool-at-create").await;
    assert_eq!(
        (read.status, read.error_code().as_str()),
        (404, "NotFoundException"),
        "a refused create must store nothing, got {} ({})",
        read.status,
        read.message()
    );
}

#[tokio::test]
async fn reputation_metrics_are_collected_until_a_write_turns_them_off() {
    let ses = Ses::new();
    let set = "reputation-switch";
    create_set(&ses, set).await;

    let created = read_set(&ses, set).await;
    assert!(
        created.flag("/ReputationOptions/ReputationMetricsEnabled"),
        "a set nobody configured must report its reputation metrics as collected, got {:?}",
        created.member("ReputationOptions")
    );
    let fresh_start = created.pointer("/ReputationOptions/LastFreshStart");
    assert!(
        fresh_start.is_some(),
        "a stored set must report when its reputation metrics were last given a fresh start, \
         got {:?}",
        created.member("ReputationOptions")
    );

    // The switch takes the value each write carries, in either direction and repeatedly.
    for (case, enabled) in [
        ("turned off", false),
        ("turned off again", false),
        ("turned back on", true),
    ] {
        let written = ses
            .put_reputation_options(set, json!({"ReputationMetricsEnabled": enabled}))
            .await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: expected 200 from PutConfigurationSetReputationOptions, got {} ({})",
            written.status,
            written.message()
        );
        let read = read_set(&ses, set).await;
        assert_eq!(
            read.flag("/ReputationOptions/ReputationMetricsEnabled"),
            enabled,
            "case {case:?}: expected ReputationMetricsEnabled {enabled}, got {:?}",
            read.member("ReputationOptions")
        );
        // The switch is not a fresh start: the instant the metrics were last given one
        // does not move because collection was turned on or off.
        assert_eq!(
            read.pointer("/ReputationOptions/LastFreshStart"),
            fresh_start,
            "case {case:?}: the fresh-start instant must not move, got {:?}",
            read.member("ReputationOptions")
        );
    }
}

/// One row of the VDM table: the options a write carries, and what a read must report.
struct VdmCase {
    case: String,
    set: String,
    body: Value,
    reported: Value,
}

/// The rows of the VDM table: each nested member on its own and both together, over the
/// whole declared `FeatureStatus` value set, plus a write that carries no options at all.
fn vdm_cases() -> Vec<VdmCase> {
    let mut cases = vec![VdmCase {
        case: "no VDM options at all".to_string(),
        set: "vdm-none".to_string(),
        body: json!({}),
        reported: Value::Null,
    }];
    for (index, metrics) in FEATURE_STATUSES.iter().enumerate() {
        cases.push(VdmCase {
            case: format!("the dashboard alone, engagement metrics {metrics}"),
            set: format!("vdm-dashboard-{index}"),
            body: json!({"VdmOptions": {"DashboardOptions": {"EngagementMetrics": metrics}}}),
            reported: json!({"DashboardOptions": {"EngagementMetrics": metrics}}),
        });
        for delivery in FEATURE_STATUSES {
            cases.push(VdmCase {
                case: format!(
                    "both, engagement metrics {metrics} and optimized shared delivery {delivery}"
                ),
                set: format!("vdm_{metrics}_{delivery}").to_lowercase(),
                body: json!({
                    "VdmOptions": {
                        "DashboardOptions": {"EngagementMetrics": metrics},
                        "GuardianOptions": {"OptimizedSharedDelivery": delivery},
                    },
                }),
                reported: json!({
                    "DashboardOptions": {"EngagementMetrics": metrics},
                    "GuardianOptions": {"OptimizedSharedDelivery": delivery},
                }),
            });
        }
    }
    for (index, delivery) in FEATURE_STATUSES.iter().enumerate() {
        cases.push(VdmCase {
            case: format!("the guardian alone, optimized shared delivery {delivery}"),
            set: format!("vdm-guardian-{index}"),
            body: json!({"VdmOptions": {"GuardianOptions": {"OptimizedSharedDelivery": delivery}}}),
            reported: json!({"GuardianOptions": {"OptimizedSharedDelivery": delivery}}),
        });
    }
    cases
}

#[tokio::test]
async fn the_vdm_preferences_a_write_carries_are_read_back() {
    let ses = Ses::new();

    for case in vdm_cases() {
        create_set(&ses, &case.set).await;
        let before = read_set(&ses, &case.set).await;
        assert_eq!(
            before.member("VdmOptions"),
            Value::Null,
            "case {:?}: a set nobody configured VDM for must report no VdmOptions, got {:?}",
            case.case,
            before.member("VdmOptions")
        );

        let written = ses.put_vdm_options(&case.set, case.body.clone()).await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: expected 200 from PutConfigurationSetVdmOptions, got {} ({})",
            case.case,
            written.status,
            written.message()
        );
        let read = read_set(&ses, &case.set).await;
        assert_eq!(
            read.member("VdmOptions"),
            case.reported,
            "case {:?}: expected VdmOptions {:?}, got {:?}",
            case.case,
            case.reported,
            read.member("VdmOptions")
        );
    }
}

#[tokio::test]
async fn a_vdm_write_carrying_no_options_leaves_the_set_with_none() {
    let ses = Ses::new();
    let set = "vdm-cleared";
    create_set(&ses, set).await;

    let written = ses
        .put_vdm_options(
            set,
            json!({
                "VdmOptions": {
                    "DashboardOptions": {"EngagementMetrics": "ENABLED"},
                    "GuardianOptions": {"OptimizedSharedDelivery": "ENABLED"},
                },
            }),
        )
        .await;
    assert_eq!(
        written.status,
        200,
        "writing the preferences: expected 200, got {} ({})",
        written.status,
        written.message()
    );
    assert_eq!(
        read_set(&ses, set).await.member("VdmOptions"),
        json!({
            "DashboardOptions": {"EngagementMetrics": "ENABLED"},
            "GuardianOptions": {"OptimizedSharedDelivery": "ENABLED"},
        }),
        "the written preferences must be read back"
    );

    // The member is the whole state this write owns, so a write that states no
    // preferences leaves the set with none: it follows the account's own again.
    let cleared = ses.put_vdm_options(set, json!({})).await;
    assert_eq!(
        cleared.status,
        200,
        "a write carrying no options: expected 200, got {} ({})",
        cleared.status,
        cleared.message()
    );
    let read = read_set(&ses, set).await;
    assert_eq!(
        read.member("VdmOptions"),
        Value::Null,
        "a write carrying no options must leave the set with no VDM preferences, got {:?}",
        read.member("VdmOptions")
    );
    // ... and the options this write does not own are untouched.
    assert!(
        read.flag("/ReputationOptions/ReputationMetricsEnabled"),
        "the VDM write must leave the reputation switch alone, got {:?}",
        read.member("ReputationOptions")
    );
    assert_eq!(
        read.member("DeliveryOptions"),
        Value::Null,
        "the VDM write must configure no delivery options, got {:?}",
        read.member("DeliveryOptions")
    );
}

#[tokio::test]
async fn a_feature_status_outside_its_modelled_value_set_is_refused_by_the_vdm_write() {
    let ses = Ses::new();
    let set = "vdm-refusals";
    create_set(&ses, set).await;

    for (case, body) in [
        (
            "a status the model declares nowhere",
            json!({"VdmOptions": {"DashboardOptions": {"EngagementMetrics": "ON"}}}),
        ),
        (
            "a declared value in the wrong case",
            json!({"VdmOptions": {"GuardianOptions": {"OptimizedSharedDelivery": "Enabled"}}}),
        ),
        (
            "the empty string is no declared value",
            json!({"VdmOptions": {"DashboardOptions": {"EngagementMetrics": ""}}}),
        ),
    ] {
        let refused = ses.put_vdm_options(set, body).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} ({})",
            refused.status,
            refused.message()
        );
        let read = read_set(&ses, set).await;
        assert_eq!(
            read.member("VdmOptions"),
            Value::Null,
            "case {case:?}: a refused write must store nothing, got {:?}",
            read.member("VdmOptions")
        );
    }
}

#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_leaves_the_others() {
    let ses = Ses::new();
    let set = "updated-set";
    let created = ses
        .create_set(json!({
            "ConfigurationSetName": set,
            "VdmOptions": {"DashboardOptions": {"EngagementMetrics": "ENABLED"}},
            "Tags": [{"Key": "owner", "Value": "option-writes"}],
        }))
        .await;
    assert_eq!(
        created.status,
        200,
        "creating the configuration set: expected 200, got {} ({})",
        created.status,
        created.message()
    );

    // Each row is an update of the one member this operation carries; the set's other
    // options must read back exactly as the create left them.
    for (case, security) in [
        (
            "the default scheme, which adds no S/MIME signature",
            json!({"SigningScheme": {"DefaultScheme": {}}}),
        ),
        (
            "the S/MIME scheme with a declared signature format",
            json!({"SigningScheme": {"SmimeScheme": {"SignatureFormat": "DETACHED"}}}),
        ),
        (
            "the options object present but naming no scheme",
            json!({}),
        ),
    ] {
        let updated = ses
            .update_set(json!({
                "ConfigurationSetName": set,
                "MessageSecurityOptions": security,
            }))
            .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected 200 from UpdateConfigurationSet, got {} ({})",
            updated.status,
            updated.message()
        );
        assert!(
            updated.is_empty_body(),
            "case {case:?}: UpdateConfigurationSetResponse declares no members, got {:?}",
            updated.member("MessageSecurityOptions")
        );

        let read = read_set(&ses, set).await;
        assert_eq!(
            read.member("MessageSecurityOptions"),
            security,
            "case {case:?}: expected MessageSecurityOptions {security:?}, got {:?}",
            read.member("MessageSecurityOptions")
        );
        assert_eq!(
            read.member("VdmOptions"),
            json!({"DashboardOptions": {"EngagementMetrics": "ENABLED"}}),
            "case {case:?}: the update must leave the VDM preferences alone, got {:?}",
            read.member("VdmOptions")
        );
        assert_eq!(
            read.member("Tags"),
            json!([{"Key": "owner", "Value": "option-writes"}]),
            "case {case:?}: the update must leave the tags alone, got {:?}",
            read.member("Tags")
        );
    }

    // An update that carries no attribute at all is a partial update of nothing: the set
    // keeps the message-security options the previous update wrote.
    let kept = read_set(&ses, set).await.member("MessageSecurityOptions");
    let empty = ses.update_set(json!({"ConfigurationSetName": set})).await;
    assert_eq!(
        empty.status,
        200,
        "an update carrying no attribute: expected 200, got {} ({})",
        empty.status,
        empty.message()
    );
    assert_eq!(
        read_set(&ses, set).await.member("MessageSecurityOptions"),
        kept,
        "an omitted attribute must be left unchanged, got {kept:?}"
    );

    // A value outside the modelled value set of the enum inside the signing scheme is
    // refused, and the update writes nothing: the set keeps what it held.
    for (case, format) in [
        ("a format the model declares nowhere", "ATTACHED"),
        ("a declared value in the wrong case", "Detached"),
        ("the empty string is no declared value", ""),
    ] {
        let refused = ses
            .update_set(json!({
                "ConfigurationSetName": set,
                "MessageSecurityOptions": {
                    "SigningScheme": {"SmimeScheme": {"SignatureFormat": format}},
                },
            }))
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException for {format:?}, got {} ({})",
            refused.status,
            refused.message()
        );
        assert_eq!(
            read_set(&ses, set).await.member("MessageSecurityOptions"),
            kept,
            "case {case:?}: a refused update must write nothing"
        );
    }
}

#[tokio::test]
async fn a_write_to_a_name_the_account_does_not_hold_is_refused_as_not_found() {
    let ses = Ses::new();
    // A set that does exist, so each refusal below is about the name asked for rather
    // than about an empty store.
    create_set(&ses, "present-set").await;

    for name in ["absent-set", "Present-Set", "present_set"] {
        let refusals = [
            (
                "the delivery configuration",
                ses.put_delivery_options(name, json!({"TlsPolicy": "REQUIRE"}))
                    .await,
            ),
            (
                "the reputation switch",
                ses.put_reputation_options(name, json!({"ReputationMetricsEnabled": true}))
                    .await,
            ),
            (
                "the VDM preferences",
                ses.put_vdm_options(
                    name,
                    json!({"VdmOptions": {"DashboardOptions": {"EngagementMetrics": "ENABLED"}}}),
                )
                .await,
            ),
            (
                "the message-security options",
                ses.update_set(json!({
                    "ConfigurationSetName": name,
                    "MessageSecurityOptions": {"SigningScheme": {"DefaultScheme": {}}},
                }))
                .await,
            ),
        ];
        for (case, refused) in refusals {
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (404, "NotFoundException"),
                "case {case:?} ({name}): expected 404 NotFoundException, got {} ({})",
                refused.status,
                refused.message()
            );
            assert_eq!(
                refused.message(),
                format!("Configuration set <{name}> does not exist."),
                "case {case:?} ({name}): unexpected refusal sentence {:?}",
                refused.message()
            );
        }
        // The write refused for one name wrote nothing under the name that does exist.
        let read = read_set(&ses, "present-set").await;
        assert_eq!(
            read.member("DeliveryOptions"),
            Value::Null,
            "a refused write must not configure another set, got {:?}",
            read.member("DeliveryOptions")
        );
    }
}

#[tokio::test]
async fn the_duplicate_name_refusal_states_the_name_in_angle_brackets() {
    let ses = Ses::new();

    // The name forms `ConfigurationSetName` documents: "up to 64 alphanumeric characters,
    // including letters, numbers, hyphens (-) and underscores (_) only".
    let longest = "d".repeat(64);
    for name in [
        "duplicate",
        "Duplicate-Set-2",
        "duplicate_set_3",
        "9",
        longest.as_str(),
    ] {
        create_set(&ses, name).await;
        let duplicate = ses.create_set(json!({"ConfigurationSetName": name})).await;
        assert_eq!(
            (duplicate.status, duplicate.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "case {name:?}: expected 400 AlreadyExistsException, got {} ({})",
            duplicate.status,
            duplicate.message()
        );
        assert_eq!(
            duplicate.message(),
            format!("Configuration set <{name}> already exists."),
            "case {name:?}: unexpected refusal sentence {:?}",
            duplicate.message()
        );
    }
}

/// One row of the event-type table: the order the definition lists the types in, and the
/// order a read reports them in.
struct EventTypeCase {
    case: &'static str,
    sent: Vec<&'static str>,
    reported: Vec<&'static str>,
}

/// The rows of the event-type table: the orders and counts a definition's list can arrive
/// in. The values are the model's own `EventType` enum
/// (`smithy_query(query="shape",shape="EventType")`), whose declaration order (SEND,
/// REJECT, BOUNCE, COMPLAINT, DELIVERY, OPEN, CLICK, RENDERING_FAILURE, DELIVERY_DELAY,
/// SUBSCRIPTION) is not the order a read reports them in.
fn event_type_cases() -> Vec<EventTypeCase> {
    vec![
        EventTypeCase {
            case: "one event type",
            sent: vec!["SEND"],
            reported: vec!["SEND"],
        },
        EventTypeCase {
            case: "two types listed in the model's declaration order",
            sent: vec!["SEND", "BOUNCE"],
            reported: vec!["BOUNCE", "SEND"],
        },
        EventTypeCase {
            case: "the same two types listed the other way round",
            sent: vec!["BOUNCE", "SEND"],
            reported: vec!["BOUNCE", "SEND"],
        },
        EventTypeCase {
            case: "three types listed in the model's declaration order",
            sent: vec!["REJECT", "COMPLAINT", "OPEN"],
            reported: vec!["COMPLAINT", "OPEN", "REJECT"],
        },
        EventTypeCase {
            case: "a type whose name is a prefix of another's, listed after it",
            sent: vec!["DELIVERY_DELAY", "DELIVERY"],
            reported: vec!["DELIVERY", "DELIVERY_DELAY"],
        },
        EventTypeCase {
            case: "every modelled type, listed in reverse declaration order",
            sent: vec![
                "SUBSCRIPTION",
                "DELIVERY_DELAY",
                "RENDERING_FAILURE",
                "CLICK",
                "OPEN",
                "DELIVERY",
                "COMPLAINT",
                "BOUNCE",
                "REJECT",
                "SEND",
            ],
            reported: vec![
                "BOUNCE",
                "CLICK",
                "COMPLAINT",
                "DELIVERY",
                "DELIVERY_DELAY",
                "OPEN",
                "REJECT",
                "RENDERING_FAILURE",
                "SEND",
                "SUBSCRIPTION",
            ],
        },
    ]
}

#[tokio::test]
async fn the_event_types_of_a_destination_are_reported_in_the_order_ses_reports_them() {
    let ses = Ses::new();
    let set = "event-type-order";
    create_set(&ses, set).await;

    for (index, case) in event_type_cases().into_iter().enumerate() {
        let name = format!("destination-{index}");
        let created = ses
            .create_destination(
                set,
                &name,
                json!({"Enabled": true, "MatchingEventTypes": case.sent}),
            )
            .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected 200 from CreateConfigurationSetEventDestination, got {} ({})",
            case.case,
            created.status,
            created.message()
        );

        let reported = reported_destination(&ses, set, &name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the destination is not reported at all", case.case));
        assert_eq!(
            reported.get("MatchingEventTypes").cloned(),
            Some(json!(case.reported)),
            "case {:?}: expected MatchingEventTypes {:?}, got {reported:?}",
            case.case,
            case.reported
        );
    }
}

#[tokio::test]
async fn a_destination_a_set_does_not_hold_is_refused_naming_the_set() {
    let ses = Ses::new();
    let set = "holds-destinations";
    create_set(&ses, set).await;
    let held = "held";
    assert_eq!(
        ses.create_destination(set, held, json!({"MatchingEventTypes": ["SEND"]}))
            .await
            .status,
        200,
        "attaching the destination must answer 200"
    );
    // A destination the set held and no longer holds is one the set does not hold.
    assert_eq!(
        ses.delete_destination(set, "gone").await.status,
        404,
        "a name the set never held must be refused"
    );

    for (case, name) in [
        ("a name the set never held", "never-there"),
        ("a name differing only in case", "HELD"),
        ("a name differing in its separators", "he_ld"),
    ] {
        let refused = ses.delete_destination(set, name).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} ({})",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.message(),
            format!("No EventDestination found for {set}"),
            "case {case:?}: unexpected refusal sentence {:?}",
            refused.message()
        );
    }

    // The destination the set does hold is still there: a refused delete removed nothing.
    assert!(
        reported_destination(&ses, set, held).await.is_some(),
        "a refused delete must leave the destination the set holds in place"
    );
}
