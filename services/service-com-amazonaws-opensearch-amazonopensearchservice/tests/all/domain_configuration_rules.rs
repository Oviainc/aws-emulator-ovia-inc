// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the configuration a domain cannot be created or updated
//! with, for the members a descriptor of a domain omits, and for calling off a
//! scheduled service software update.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness, the baseline `CreateDomain`
//! body and the service-software seeding come from [`super::domain_lifecycle`] and
//! [`super::service_software_update`]; this suite adds no second copy of them.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! * a master password for the internal user database has to draw on four character
//!   classes (an uppercase letter, a lowercase letter, a digit and one character that
//!   is none of those), and a request whose password does not is refused with
//!   `ValidationException` (HTTP 400) and creates or changes nothing — over a table
//!   that drops each required class in turn and crosses the model's own
//!   [`MASTER_USER_PASSWORD_MINIMUM`] length floor;
//! * each storage tier rests on the one below it: the cold tier on the warm one
//!   (`ColdStorageOptions$Enabled`: "you must enable UltraWarm storage to enable cold
//!   storage"), and the warm tier on the dedicated master nodes that manage it — over
//!   the chain's matrix, with each broken link carrying the error of that link;
//! * an enabled log-publishing option has to name the CloudWatch Logs group it
//!   publishes to, over EVERY `LogType` the model declares plus maps carrying two
//!   options at once;
//! * the off-peak window cannot be asked for disabled, at create or at update
//!   (`OffPeakWindowOptions$Enabled`);
//! * a domain that publishes no logs omits `DomainStatus$LogPublishingOptions`
//!   altogether rather than reporting an empty map, while the configuration container
//!   every domain has is reported with its (possibly empty) map;
//! * a service software update that has been scheduled and not begun can be called
//!   off, and outside that state there is nothing to cancel.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::LOG_TYPE_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ServiceSoftware, SoftwareDeployment, MASTER_USER_PASSWORD_MINIMUM,
};

use super::domain_lifecycle::{
    baseline_create, create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT,
};
use super::service_software_update::{
    message, seed, settled_domain, start, with_update_available,
};

/// A CloudWatch Logs group ARN of the form `LogPublishingOption`'s member takes.
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/opensearch/application";

/// `CreateDomain` with the baseline body for `name`, with `member` replaced by
/// `value`.
async fn create_with(emulator: &Emulator, name: &str, member: &str, value: Value) -> Answer {
    let mut body = baseline_create(name);
    body[member] = value;
    emulator.create(body).await
}

/// The `AdvancedSecurityOptions` container that configures the internal user
/// database with `password` as the master credential.
fn fine_grained_access(password: &str) -> Value {
    json!({
        "Enabled": true,
        "InternalUserDatabaseEnabled": true,
        "MasterUserOptions": { "MasterUserName": "adminuser", "MasterUserPassword": password },
    })
}

/// Assert that `answer` is a modelled 400 refusal carrying `code` — the error the
/// service answers this class of unprovisionable configuration with.
pub(crate) fn assert_refusal(answer: &Answer, code: &str, case: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (400, code),
        "case {case}: expected a 400 {code}, got {} / {:?} with {:?}",
        answer.status,
        error_code(answer),
        answer.body
    );
}

/// Assert that the store holds no domain under `name` — what a refused create must
/// leave behind.
pub(crate) async fn assert_no_domain(emulator: &Emulator, name: &str) {
    let answer = emulator.describe(name).await;
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case DescribeDomain({name:?}): a refused create must store no domain, got {} with {:?}",
        answer.status,
        answer.body
    );
}

/// The `DomainStatus` `DescribeDomain` reports for `name`.
async fn described(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

#[tokio::test]
async fn a_master_password_must_meet_the_published_complexity_requirement() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the LENGTH, across the
    // published floor (one short, exactly at it, past it), and which required
    // character class the password is missing (each of the four dropped in turn),
    // plus a special character outside ASCII punctuation.
    let floor = MASTER_USER_PASSWORD_MINIMUM;
    let rows: Vec<(&str, &str, bool)> = vec![
        ("every class, past the floor", "Str0ng!Pass", true),
        ("every class, at the floor", "Ab1!defg", true),
        ("every class, one short of the floor", "Ab1!def", false),
        ("no special character", "Abcdefgh1234", false),
        ("no uppercase letter", "abcdefgh1!", false),
        ("no lowercase letter", "ABCDEFGH1!", false),
        ("no digit", "Abcdefgh!", false),
        ("a non-ASCII special character", "Abcdefg1\u{a7}", true),
    ];

    for (index, (case, password, accepted)) in rows.iter().enumerate() {
        let name = format!("fgac-create-{index}");
        let answer = create_with(
            &emulator,
            &name,
            "AdvancedSecurityOptions",
            fine_grained_access(password),
        )
        .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case CreateDomain({case}, {} chars): expected 200, got {} with {:?}",
                password.chars().count(),
                answer.status,
                answer.body
            );
            // The credential is `@sensitive`: it is stored, never reported back.
            let reported = answer.body.to_string();
            assert!(
                !reported.contains(password),
                "case CreateDomain({case}): the response must not carry the master password, \
                 got {reported}"
            );
            let described = described(&emulator, &name).await.to_string();
            assert!(
                !described.contains(password),
                "case DescribeDomain({case}): the descriptor must not carry the master \
                 password, got {described}"
            );
        } else {
            assert_refusal(&answer, "ValidationException", case);
            let refusal = message(&answer, case);
            if password.chars().count() >= floor {
                assert!(
                    refusal.contains("uppercase letter")
                        && refusal.contains("lowercase letter")
                        && refusal.contains("number")
                        && refusal.contains("special character")
                        && !refusal.contains(password),
                    "case CreateDomain({case}): the refusal must state the requirement and never \
                     the password, got {refusal:?}"
                );
            } else {
                // A password shorter than the floor breaks `MasterUserPassword`'s own
                // modelled `@length`, so the boundary states it before the handler
                // runs — same status and code, worded by the framework's constraint
                // walk, which names the member.
                assert!(
                    refusal.contains("masterUserPassword"),
                    "case CreateDomain({case}): the refusal must name the member, got {refusal:?}"
                );
            }
            assert_no_domain(&emulator, &name).await;
        }
    }

    // The same rule holds on the call that CHANGES a domain's fine-grained access
    // control, so a weak password cannot be reached by updating into it.
    let name = "fgac-update";
    create_baseline(&emulator, name).await;
    // The service applies one change of a domain at a time, so each accepted change
    // has to land before the next row's update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (case, password, accepted) in rows {
        let answer = emulator
            .update_config(
                name,
                json!({ "AdvancedSecurityOptions": fine_grained_access(password) }),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case UpdateDomainConfig({case}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            tokio::time::sleep(SETTLE_WAIT).await;
        } else {
            assert_refusal(&answer, "ValidationException", case);
        }
    }
}

#[tokio::test]
async fn each_storage_tier_rests_on_the_one_below_it() {
    let emulator = Emulator::new();

    // A cluster that can carry the warm tier: a non-burstable data-node family with
    // dedicated master nodes. The warm instance type is one of the three the model's
    // `OpenSearchWarmPartitionInstanceType` declares
    // (`smithy_query(query="shape", shape="OpenSearchWarmPartitionInstanceType")`).
    let warm = json!({
        "InstanceType": "r6g.large.search",
        "InstanceCount": 2,
        "DedicatedMasterEnabled": true,
        "DedicatedMasterType": "r6g.large.search",
        "DedicatedMasterCount": 3,
        "WarmEnabled": true,
        "WarmType": "ultrawarm1.medium.search",
        "WarmCount": 2,
    });

    // Rows span the chain, one row per broken link and one per link that holds: the
    // cold tier with no warm one under it (by omission and by an explicit false), the
    // warm tier without the dedicated master nodes that manage it, and the
    // configurations that are served. Each refusal carries the error of ITS link, so a
    // caller tells them apart.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "cold enabled, no warm member",
            json!({
                "InstanceType": "t3.small.search",
                "InstanceCount": 1,
                "ColdStorageOptions": { "Enabled": true },
            }),
            Some("BaseException"),
        ),
        (
            "cold enabled, warm disabled",
            json!({
                "InstanceType": "r6g.large.search",
                "InstanceCount": 2,
                "WarmEnabled": false,
                "ColdStorageOptions": { "Enabled": true },
            }),
            Some("BaseException"),
        ),
        (
            "warm enabled, no dedicated masters",
            json!({
                "InstanceType": "r6g.large.search",
                "InstanceCount": 2,
                "WarmEnabled": true,
                "WarmType": "ultrawarm1.medium.search",
                "WarmCount": 2,
            }),
            Some("ValidationException"),
        ),
        (
            "cold enabled on top of warm",
            {
                let mut cluster = warm.clone();
                cluster["ColdStorageOptions"] = json!({ "Enabled": true });
                cluster
            },
            None,
        ),
        (
            "cold disabled, no warm member",
            json!({
                "InstanceType": "t3.small.search",
                "InstanceCount": 1,
                "ColdStorageOptions": { "Enabled": false },
            }),
            None,
        ),
        ("warm enabled, no cold container", warm.clone(), None),
        (
            "neither tier",
            json!({ "InstanceType": "t3.small.search", "InstanceCount": 1 }),
            None,
        ),
    ];

    for (index, (case, cluster, refused_with)) in rows.into_iter().enumerate() {
        let name = format!("tier-{index}");
        let answer = create_with(&emulator, &name, "ClusterConfig", cluster.clone()).await;
        match refused_with {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case CreateDomain({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let reported = described(&emulator, &name).await["ClusterConfig"].clone();
                assert_eq!(
                    reported["ColdStorageOptions"],
                    super::domain_lifecycle::completed("ClusterConfig", &cluster)
                        ["ColdStorageOptions"],
                    "case DescribeDomain({case}): the stored tiering must be the one created, \
                     completed with the service's own default where the create named none, got \
                     {reported:?}"
                );
            }
            Some(code) => {
                assert_refusal(&answer, code, case);
                assert_no_domain(&emulator, &name).await;
            }
        }
    }

    // The same dependency holds on an update, and it is judged on the configuration
    // the change would LEAVE the domain with: enabling the cold tier alone is refused
    // on a domain with no warm tier and leaves its cluster configuration untouched,
    // while enabling both tiers in one call is served.
    let name = "tier-update";
    create_baseline(&emulator, name).await;
    // The change the create started has to land before an update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;
    let before = described(&emulator, name).await["ClusterConfig"].clone();
    let answer = emulator
        .update_config(
            name,
            json!({
                "ClusterConfig": {
                    "InstanceType": "r6g.large.search",
                    "InstanceCount": 2,
                    "ColdStorageOptions": { "Enabled": true },
                },
            }),
        )
        .await;
    assert_refusal(&answer, "BaseException", "UpdateDomainConfig(cold alone)");
    assert_eq!(
        described(&emulator, name).await["ClusterConfig"],
        before,
        "case UpdateDomainConfig(cold alone): a refused update must change nothing"
    );
    let answer = emulator
        .update_config(name, {
            let mut cluster = warm.clone();
            cluster["ColdStorageOptions"] = json!({ "Enabled": true });
            json!({ "ClusterConfig": cluster })
        })
        .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateDomainConfig(both tiers): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn an_enabled_log_publishing_option_must_name_its_log_group() {
    let emulator = Emulator::new();

    // The catalog is the whole `LogType` enum the model declares
    // (`smithy_query(query="shape", shape="LogType")`), exposed as `LOG_TYPE_VALUES`:
    // the rule holds for every log type, not only the application logs the report
    // named. Each type is driven enabled without a group, enabled with one, and
    // disabled without one.
    for log_type in LOG_TYPE_VALUES.iter().copied() {
        let rows: Vec<(&str, Value, bool)> = vec![
            ("enabled with no log group", json!({ "Enabled": true }), false),
            (
                "enabled with an empty log group",
                json!({ "Enabled": true, "CloudWatchLogsLogGroupArn": "" }),
                false,
            ),
            (
                "enabled with a log group",
                json!({ "Enabled": true, "CloudWatchLogsLogGroupArn": LOG_GROUP }),
                true,
            ),
            ("disabled with no log group", json!({ "Enabled": false }), true),
            ("no Enabled member", json!({ "CloudWatchLogsLogGroupArn": LOG_GROUP }), true),
        ];
        for (index, (case, option, accepted)) in rows.into_iter().enumerate() {
            let name = format!("logs-{}-{index}", log_type.to_lowercase().replace('_', "-"));
            let answer = create_with(
                &emulator,
                &name,
                "LogPublishingOptions",
                json!({ log_type: option }),
            )
            .await;
            let case = format!("{log_type}: {case}");
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case CreateDomain({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_refusal(&answer, "ValidationException", &case);
                assert!(
                    message(&answer, &case).contains(log_type),
                    "case CreateDomain({case}): the refusal must name the offending log type, \
                     got {:?}",
                    message(&answer, &case)
                );
                assert_no_domain(&emulator, &name).await;
            }
        }
    }

    // A map carrying several options is refused when ANY of them publishes to
    // nowhere, whichever position it sits in, and names the first offending type so
    // one request is refused with one sentence. Each map is driven through the create
    // and through the call that CHANGES a domain's log publishing.
    let updated = "logs-update";
    create_baseline(&emulator, updated).await;
    // The change the create started has to land before an update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;
    let maps: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "both enabled without a group",
            json!({
                "AUDIT_LOGS": { "Enabled": true },
                "INDEX_SLOW_LOGS": { "Enabled": true },
            }),
            Some("AUDIT_LOGS"),
        ),
        (
            "one configured, one not",
            json!({
                "INDEX_SLOW_LOGS": { "Enabled": true, "CloudWatchLogsLogGroupArn": LOG_GROUP },
                "SEARCH_SLOW_LOGS": { "Enabled": true },
            }),
            Some("SEARCH_SLOW_LOGS"),
        ),
        (
            "both configured",
            json!({
                "INDEX_SLOW_LOGS": { "Enabled": true, "CloudWatchLogsLogGroupArn": LOG_GROUP },
                "SEARCH_SLOW_LOGS": { "Enabled": true, "CloudWatchLogsLogGroupArn": LOG_GROUP },
            }),
            None,
        ),
    ];
    for (index, (case, options, offending)) in maps.into_iter().enumerate() {
        let name = format!("logs-map-{index}");
        let answer = create_with(&emulator, &name, "LogPublishingOptions", options.clone()).await;
        match offending {
            Some(log_type) => {
                assert_refusal(&answer, "ValidationException", case);
                assert!(
                    message(&answer, case).contains(log_type),
                    "case CreateDomain({case}): expected the refusal to name {log_type}, got {:?}",
                    message(&answer, case)
                );
                assert_no_domain(&emulator, &name).await;
            }
            None => assert_eq!(
                answer.status, 200,
                "case CreateDomain({case}): expected 200, got {} with {:?}",
                answer.status, answer.body
            ),
        }

        // And the same map on the call that CHANGES a domain's log publishing.
        let answer = emulator
            .update_config(updated, json!({ "LogPublishingOptions": options }))
            .await;
        match offending {
            Some(_) => assert_refusal(&answer, "ValidationException", case),
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case UpdateDomainConfig({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                // An accepted change lands before the next row starts another.
                tokio::time::sleep(SETTLE_WAIT).await;
            }
        }
    }
}

#[tokio::test]
async fn the_off_peak_window_cannot_be_asked_for_disabled() {
    let emulator = Emulator::new();

    let window = json!({ "WindowStartTime": { "Hours": 2, "Minutes": 30 } });
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "disabled with a window",
            json!({ "Enabled": false, "OffPeakWindow": window.clone() }),
            false,
        ),
        ("disabled alone", json!({ "Enabled": false }), false),
        (
            "enabled with a window",
            json!({ "Enabled": true, "OffPeakWindow": window.clone() }),
            true,
        ),
        ("enabled alone", json!({ "Enabled": true }), true),
        (
            "a window and no Enabled member",
            json!({ "OffPeakWindow": window.clone() }),
            true,
        ),
    ];

    for (index, (case, options, accepted)) in rows.into_iter().enumerate() {
        let name = format!("window-{index}");
        let answer = create_with(&emulator, &name, "OffPeakWindowOptions", options).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case CreateDomain({case}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_refusal(&answer, "ValidationException", case);
            assert_no_domain(&emulator, &name).await;
        }
    }

    // A domain's window cannot be turned off by an update either, and the refused
    // call leaves the window it had.
    let name = "window-update";
    create_with(
        &emulator,
        name,
        "OffPeakWindowOptions",
        json!({ "Enabled": true, "OffPeakWindow": window.clone() }),
    )
    .await;
    // The change the create started has to land before an update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;
    let before = described(&emulator, name).await["OffPeakWindowOptions"].clone();
    let answer = emulator
        .update_config(name, json!({ "OffPeakWindowOptions": { "Enabled": false } }))
        .await;
    assert_refusal(
        &answer,
        "ValidationException",
        "UpdateDomainConfig(disable the window)",
    );
    assert_eq!(
        described(&emulator, name).await["OffPeakWindowOptions"],
        before,
        "case UpdateDomainConfig(disable the window): a refused update must change nothing"
    );
    let moved = json!({
        "Enabled": true,
        "OffPeakWindow": { "WindowStartTime": { "Hours": 23, "Minutes": 0 } },
    });
    let answer = emulator
        .update_config(name, json!({ "OffPeakWindowOptions": moved.clone() }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateDomainConfig(move the window): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        described(&emulator, name).await["OffPeakWindowOptions"], moved,
        "case DescribeDomain(window-update): the window the update set must be the one reported"
    );
}

#[tokio::test]
async fn a_domain_that_publishes_no_logs_omits_the_member_and_keeps_the_container() {
    let emulator = Emulator::new();

    // Rows differ along what the rule turns on: whether the create left the option
    // out, set it to an empty map, or put a log type in it. The first two leave the
    // domain publishing NOTHING, which is the case the descriptor omits the member
    // for; the third is the only one with something to report.
    let configured = json!({
        "ES_APPLICATION_LOGS": { "Enabled": true, "CloudWatchLogsLogGroupArn": LOG_GROUP },
    });
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("never configured", None),
        ("configured and empty", Some(json!({}))),
        ("configured", Some(configured.clone())),
    ];

    for (index, (case, options)) in rows.into_iter().enumerate() {
        let name = format!("logs-presence-{index}");
        let mut body = baseline_create(&name);
        match options.clone() {
            Some(value) => body["LogPublishingOptions"] = value,
            None => {
                body.as_object_mut()
                    .expect("the create body is an object")
                    .remove("LogPublishingOptions");
            }
        }
        let answer = emulator.create(body).await;
        assert_eq!(
            answer.status, 200,
            "case CreateDomain({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        // What the domain publishes: the entries it holds, and no member at all when
        // it holds none.
        let published = options
            .clone()
            .filter(|value| value.as_object().is_some_and(|map| !map.is_empty()));
        for (operation, reported) in [
            ("CreateDomain", answer.body["DomainStatus"].clone()),
            ("DescribeDomain", described(&emulator, &name).await),
        ] {
            assert_eq!(
                reported.get("LogPublishingOptions"),
                published.as_ref(),
                "case {operation}({case}): expected the member to be {}, got {reported:?}",
                match &published {
                    Some(value) => format!("{value}"),
                    None => "absent".to_string(),
                }
            );
        }

        // The CONFIGURATION container is a different member: every domain has one, so
        // `DescribeDomainConfig` reports it with the map the domain holds — empty
        // rather than absent — beside the option's own status.
        let answer = emulator.describe_config(&name).await;
        assert_eq!(
            answer.status, 200,
            "case DescribeDomainConfig({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let container = answer.body["DomainConfig"]["LogPublishingOptions"].clone();
        assert_eq!(
            container.get("Options"),
            Some(&options.clone().unwrap_or_else(|| json!({}))),
            "case DescribeDomainConfig({case}): expected the container to report the stored map, \
             got {container:?}"
        );
        // The option's own status comes with it: a created option is at the revision a
        // create leaves every option at, and the log-publishing configuration of a
        // domain that publishes no log type is `Active` — nothing is being applied for
        // it — while one that holds a log type is applied with the rest of the
        // configuration the create is provisioning.
        let publishes = options
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|types| !types.is_empty());
        let expected_state = if publishes { "Processing" } else { "Active" };
        assert!(
            container["Status"]["UpdateVersion"].as_i64() == Some(2)
                && container["Status"]["State"].as_str() == Some(expected_state),
            "case DescribeDomainConfig({case}): expected the container to carry the option's \
             status at revision 2 in state {expected_state}, got {container:?}"
        );
    }

    // And an update that configures the option on a domain created without it makes
    // the member appear, so presence tracks what the domain publishes rather than the
    // call that read it. The change its create started has to land first: a domain
    // that is still being provisioned refuses a further change.
    let name = "logs-presence-0";
    tokio::time::sleep(SETTLE_WAIT).await;
    let answer = emulator
        .update_config(name, json!({ "LogPublishingOptions": configured.clone() }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateDomainConfig(configure log publishing): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        described(&emulator, name).await.get("LogPublishingOptions"),
        Some(&configured),
        "case DescribeDomain({name:?}): the configured member must be reported"
    );
}

#[tokio::test]
async fn only_a_scheduled_service_software_update_can_be_cancelled() {
    let emulator = Emulator::new();

    // A domain the store holds nothing under has no update to call off, and that is
    // the operation's modelled missing-resource refusal rather than a 501: the
    // operation models `ResourceNotFoundException`
    // (`smithy_query(query="operation",operation="CancelServiceSoftwareUpdate")`).
    let answer = cancel(&emulator, "does-not-exist-xyz").await;
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case CancelServiceSoftwareUpdate(no such domain): expected the modelled \
         ResourceNotFoundException, got {} with {:?}",
        answer.status,
        answer.body
    );

    // A domain as `CreateDomain` left it has nothing scheduled, so there is nothing to
    // cancel and the record is left exactly as it was.
    let idle = "cancel-idle";
    settled_domain(&emulator, idle).await;
    let answer = cancel(&emulator, idle).await;
    assert_refusal(
        &answer,
        "ValidationException",
        "CancelServiceSoftwareUpdate(nothing scheduled)",
    );

    // A deployment already in flight is past the point a cancel can reach: "you can
    // only perform this operation ... when the domain's UpdateStatus is
    // PENDING_UPDATE".
    let deploying = "cancel-deploying";
    settled_domain(&emulator, deploying).await;
    seed(
        &emulator,
        deploying,
        ServiceSoftware {
            deployment: Some(SoftwareDeployment {
                target_version: "R20250601".to_string(),
                completes_at: SystemTime::now() + Duration::from_secs(60),
                rollback: false,
            }),
            ..with_update_available("R20250101", "R20250601")
        },
    );
    let answer = cancel(&emulator, deploying).await;
    assert_refusal(
        &answer,
        "ValidationException",
        "CancelServiceSoftwareUpdate(deployment in flight)",
    );

    // An update asked for at an instant still ahead has been requested and not begun,
    // which is the one state it can be called off in. Rows differ by how far ahead the
    // start was put.
    for (index, ahead) in [Duration::from_secs(3_600), Duration::from_secs(30 * 86_400)]
        .into_iter()
        .enumerate()
    {
        let name = format!("cancel-pending-{index}");
        settled_domain(&emulator, &name).await;
        seed(&emulator, &name, with_update_available("R20250101", "R20250601"));
        let desired = i64::try_from(
            (SystemTime::now() + ahead)
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("the test clock is after the epoch")
                .as_millis(),
        )
        .expect("an instant of this clock fits a Long");
        let scheduled = start(
            &emulator,
            &name,
            json!({ "ScheduleAt": "TIMESTAMP", "DesiredStartTime": desired }),
        )
        .await;
        assert_eq!(
            (
                scheduled.status,
                scheduled.body["ServiceSoftwareOptions"]["UpdateStatus"].as_str(),
                scheduled.body["ServiceSoftwareOptions"]["Cancellable"].as_bool(),
            ),
            (200, Some("PENDING_UPDATE"), Some(true)),
            "case StartServiceSoftwareUpdate({name:?}, {ahead:?} ahead): expected a cancellable \
             scheduled update, got {:?}",
            scheduled.body
        );

        let answer = cancel(&emulator, &name).await;
        assert_eq!(
            answer.status, 200,
            "case CancelServiceSoftwareUpdate({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let options = answer.body["ServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                options["CurrentVersion"].as_str(),
                options["NewVersion"].as_str(),
                options["UpdateStatus"].as_str(),
                options["UpdateAvailable"].as_bool(),
                options["Cancellable"].as_bool(),
            ),
            (
                Some("R20250101"),
                Some("R20250601"),
                Some("ELIGIBLE"),
                Some(true),
                Some(false),
            ),
            "case CancelServiceSoftwareUpdate({name:?}): the cancelled release must be available \
             again and nothing scheduled, got {options:?}"
        );

        // The answer is the STORED record, so a later read agrees with it, and a
        // second cancel has nothing left to call off.
        assert_eq!(
            described(&emulator, &name).await["ServiceSoftwareOptions"], options,
            "case DescribeDomain({name:?}): the descriptor must report the cancelled record"
        );
        let answer = cancel(&emulator, &name).await;
        assert_refusal(
            &answer,
            "ValidationException",
            "CancelServiceSoftwareUpdate(already cancelled)",
        );
    }
}

/// `POST /2021-01-01/opensearch/serviceSoftwareUpdate/cancel`.
async fn cancel(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/serviceSoftwareUpdate/cancel"),
            Some(json!({ "DomainName": name })),
        )
        .await
}
