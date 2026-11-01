// SPDX-License-Identifier: CC0-1.0

//! Detaching a Lightsail SSL/TLS certificate from a content delivery network
//! distribution, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `DetachCertificateFromDistribution` releases the
//! certificate the ONE stored distribution of that name serves HTTPS traffic
//! with, so every later read of that distribution reports no `certificateName`
//! and no `alternativeDomainNames`, the released certificate stops counting that
//! distribution in its `inUseResourceCount` while remaining a certificate of the
//! account, and the call answers one operation-log entry for the distribution it
//! changed. Releasing an attachment is idempotent — a distribution serving no
//! certificate of its own is left serving none — and a detach the account's
//! resources do not allow, or one made outside the Region the distribution APIs
//! are served from, is refused and detaches nothing.

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check, the instance stand-up
// and the certificate/distribution reads this test target already exposes: the
// sibling attach suite drives the service through the same front door rather
// than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    certificate, create_certificate, distribution, string_list, wait_for_distribution_status,
    wait_for_issued_certificate,
};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The `operationType` the log entry of a detach carries, from the modelled
/// `OperationType` enum (which declares a `DetachCertificateFromDistribution`
/// member).
const OPERATION_TYPE: &str = "DetachCertificateFromDistribution";

/// The `ResourceType` a distribution reports, from the modelled enum.
const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The one Region a distribution and a certificate live in, from the modelled
/// `LightsailDistribution$location` documentation ("all distributions are
/// located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The `CertificateStatus` an attachable certificate is in, from the modelled
/// enum.
const ISSUED: &str = "ISSUED";

/// The `status` of a distribution whose configuration is still rolling out, and
/// the one it settles into. See `crate::state::DISTRIBUTION_STATUS_IN_PROGRESS`.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const DEPLOYED: &str = "Deployed";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The availability zone the origin instance of every case is created in.
const ORIGIN_ZONE: &str = "us-east-1a";

// ---------------------------------------------------------------------------
// The detach table
// ---------------------------------------------------------------------------

/// How the certificate the detach releases came to be on the distribution.
///
/// Every operation that records a certificate on a distribution is a row, so
/// the release is tested against each of them rather than only against the
/// attach.
#[derive(Clone, Copy, Debug)]
enum AttachedBy {
    /// `AttachCertificateToDistribution`.
    Attach,
    /// `CreateDistribution`'s own `certificateName`.
    CreateTime,
    /// `UpdateDistribution`'s own `certificateName`.
    Update,
    /// Nothing: the distribution serves no certificate of its own.
    Nothing,
}

/// One distribution the detach is made against.
///
/// The rows differ along everything the rule generalises over: how many domains
/// the attached certificate published (and so how many alternate domain names
/// the detach withdraws), which operation made the attachment, whether the
/// distribution had finished deploying when the detach was made, and whether
/// there was an attachment to release at all.
struct Row {
    case: &'static str,
    distribution_name: &'static str,
    bundle_id: &'static str,
    origin_name: &'static str,
    /// The certificate to attach, when the row attaches one.
    certificate_name: Option<&'static str>,
    domain: &'static str,
    alternatives: &'static [&'static str],
    attached_by: AttachedBy,
    /// The domains the distribution publishes just before the detach.
    published_before: &'static [&'static str],
    /// Whether the distribution is waited into `Deployed` before the detach: the
    /// operation's documentation makes the detach conditional on nothing.
    wait_deployed: bool,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a one-domain certificate attached by the attach",
        distribution_name: "single-detach-cdn",
        bundle_id: "small_1_0",
        origin_name: "detach-origin-single",
        certificate_name: Some("single-detach-cert"),
        domain: "detach.example.com",
        alternatives: &[],
        attached_by: AttachedBy::Attach,
        published_before: &["detach.example.com"],
        wait_deployed: true,
    },
    Row {
        case: "a four-domain certificate attached by the attach",
        distribution_name: "many-detach-cdn",
        bundle_id: "large_1_0",
        origin_name: "detach-origin-many",
        certificate_name: Some("many_detach_cert"),
        domain: "alpha.detach.example.org",
        alternatives: &[
            "beta.detach.example.org",
            "gamma.detach.example.org",
            "delta.detach.example.org",
        ],
        attached_by: AttachedBy::Attach,
        published_before: &[
            "alpha.detach.example.org",
            "beta.detach.example.org",
            "gamma.detach.example.org",
            "delta.detach.example.org",
        ],
        wait_deployed: true,
    },
    Row {
        case: "a certificate the create named, whose domains the create published",
        distribution_name: "create-detach-cdn",
        bundle_id: "small_1_0",
        origin_name: "detach-origin-create",
        certificate_name: Some("create-detach-cert"),
        domain: "create.detach.example.com",
        alternatives: &["www.create.detach.example.com"],
        attached_by: AttachedBy::CreateTime,
        published_before: &[
            "create.detach.example.com",
            "www.create.detach.example.com",
        ],
        wait_deployed: true,
    },
    Row {
        case: "a certificate the update attached, detached while still deploying",
        distribution_name: "update-detach-cdn",
        bundle_id: "medium_1_0",
        origin_name: "detach-origin-update",
        certificate_name: Some("update-detach-cert"),
        domain: "update.detach.example.net",
        alternatives: &["cdn.update.detach.example.net"],
        attached_by: AttachedBy::Update,
        published_before: &[
            "update.detach.example.net",
            "cdn.update.detach.example.net",
        ],
        wait_deployed: false,
    },
    Row {
        case: "a distribution serving no certificate of its own",
        distribution_name: "bare-detach-cdn",
        bundle_id: "small_1_0",
        origin_name: "detach-origin-bare",
        certificate_name: None,
        domain: "unused.example.com",
        alternatives: &[],
        attached_by: AttachedBy::Nothing,
        published_before: &[],
        wait_deployed: true,
    },
];

/// Stand up `row`'s origin, certificate and distribution, and make the
/// attachment the row describes.
async fn setup(emulator: &Emulator, row: &Row) {
    create_running_instance(emulator, row.origin_name, ORIGIN_ZONE).await;
    if let Some(certificate_name) = row.certificate_name {
        create_certificate(emulator, certificate_name, row.domain, row.alternatives).await;
        wait_for_issued_certificate(emulator, certificate_name).await;
    }

    let mut body = json!({
        "distributionName": row.distribution_name,
        "bundleId": row.bundle_id,
        "origin": {"name": row.origin_name, "protocolPolicy": "http-only"},
        "defaultCacheBehavior": {"behavior": "cache"},
    });
    if let (AttachedBy::CreateTime, Some(certificate_name)) =
        (row.attached_by, row.certificate_name)
    {
        body["certificateName"] = json!(certificate_name);
    }
    emulator.ok("CreateDistribution", body).await;

    match (row.attached_by, row.certificate_name) {
        (AttachedBy::Attach, Some(certificate_name)) => {
            emulator
                .ok(
                    "AttachCertificateToDistribution",
                    json!({
                        "distributionName": row.distribution_name,
                        "certificateName": certificate_name,
                    }),
                )
                .await;
        }
        (AttachedBy::Update, Some(certificate_name)) => {
            emulator
                .ok(
                    "UpdateDistribution",
                    json!({
                        "distributionName": row.distribution_name,
                        "certificateName": certificate_name,
                    }),
                )
                .await;
        }
        _ => {}
    }
}

/// Assert the log entry `answered` carries everything a detach of
/// `distribution_name` must report, and that the log kept it.
async fn assert_logged_detach(emulator: &Emulator, case: &str, answered: &Value) {
    let operation = answered["operation"].clone();
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(RESOURCE_TYPE_DISTRIBUTION),
        "case {case}: expected the entry to be about the distribution, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Started"),
        "case {case}: expected a Started operation, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(false),
        "case {case}: expected a non-terminal operation, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(DISTRIBUTION_REGION),
        "case {case}: expected the entry located where the distribution is, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    // The entry PERSISTED: the log reports the same one when asked for its id.
    let by_id = emulator
        .ok("GetOperation", json!({"operationId": id}))
        .await["operation"]
        .clone();
    for member in ["operationType", "resourceType", "resourceName"] {
        assert_eq!(
            by_id[member], operation[member],
            "case {case}: expected GetOperation to agree on {member}, got {by_id:?}"
        );
    }
}

/// RULE: a detach releases the certificate the named distribution serves HTTPS
/// traffic with, so every later read reports no certificate and none of that
/// certificate's domains, and the certificate stops counting that distribution.
#[tokio::test]
async fn detaching_releases_the_certificate_the_distribution_served() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        setup(&emulator, row).await;

        // What the detach has to undo, as the DISTRIBUTION reports it.
        let before = distribution(&emulator, row.distribution_name).await;
        assert_eq!(
            before["certificateName"].as_str(),
            row.certificate_name,
            "case {case}: expected the certificate the setup attached, got {before:?}"
        );
        assert_eq!(
            string_list(&before["alternativeDomainNames"]),
            row.published_before,
            "case {case}: expected the domains {:?} before the detach, got {before:?}",
            row.published_before
        );
        if let Some(certificate_name) = row.certificate_name {
            let in_use = certificate(&emulator, certificate_name).await;
            assert_eq!(
                in_use["certificateDetail"]["inUseResourceCount"].as_i64(),
                Some(1),
                "case {case}: expected the certificate in use by the distribution before the \
                 detach, got {in_use:?}"
            );
        }
        if row.wait_deployed {
            wait_for_distribution_status(&emulator, row.distribution_name, DEPLOYED).await;
        }

        let answered = emulator
            .ok(
                "DetachCertificateFromDistribution",
                json!({"distributionName": row.distribution_name}),
            )
            .await;
        assert_eq!(
            answered["operation"]["resourceName"].as_str(),
            Some(row.distribution_name),
            "case {case}: expected the entry to name {}, got {answered:?}",
            row.distribution_name
        );
        assert_logged_detach(&emulator, case, &answered).await;

        // The release, as the DISTRIBUTION reports it: it names no certificate
        // and publishes none of that certificate's domains, so it accepts
        // traffic for none of them.
        let detached = distribution(&emulator, row.distribution_name).await;
        assert_eq!(
            detached["certificateName"],
            Value::Null,
            "case {case}: expected no certificate after the detach, got {detached:?}"
        );
        assert!(
            string_list(&detached["alternativeDomainNames"]).is_empty(),
            "case {case}: expected no alternative domain names after the detach, got {detached:?}"
        );
        // And it survives the deployment the clock owed the distribution.
        let deployed =
            wait_for_distribution_status(&emulator, row.distribution_name, DEPLOYED).await;
        assert_eq!(
            deployed["certificateName"],
            Value::Null,
            "case {case}: expected the detach to survive the deployment, got {deployed:?}"
        );
        assert!(
            string_list(&deployed["alternativeDomainNames"]).is_empty(),
            "case {case}: expected no alternative domain names once deployed, got {deployed:?}"
        );

        // And as the CERTIFICATE reports it: it is still a certificate of the
        // account — a detach releases the attachment, it does not delete the
        // certificate — and it is in use by one resource fewer.
        if let Some(certificate_name) = row.certificate_name {
            let released = certificate(&emulator, certificate_name).await;
            assert_eq!(
                released["certificateName"].as_str(),
                Some(certificate_name),
                "case {case}: expected the certificate to still be held, got {released:?}"
            );
            assert_eq!(
                released["certificateDetail"]["status"].as_str(),
                Some(ISSUED),
                "case {case}: expected the released certificate to stay {ISSUED}, got {released:?}"
            );
            assert_eq!(
                released["certificateDetail"]["inUseResourceCount"].as_i64(),
                Some(0),
                "case {case}: expected the released certificate in use by 0 resources, got \
                 {released:?}"
            );
        }
    }

    // Every distribution the table detached from, as the LISTING reports them:
    // the same release the named read reports, for every one of them.
    let listed = emulator.ok("GetDistributions", json!({})).await["distributions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        ROWS.len(),
        "case the listing: expected {} distributions, got {listed:?}",
        ROWS.len()
    );
    for entry in &listed {
        assert_eq!(
            entry["certificateName"],
            Value::Null,
            "case the listing: expected every detached distribution to name no certificate, got \
             {entry:?}"
        );
        assert!(
            string_list(&entry["alternativeDomainNames"]).is_empty(),
            "case the listing: expected no alternative domain names, got {entry:?}"
        );
    }
}

/// RULE: releasing an attachment is idempotent and reversible — a detach of a
/// distribution that serves no certificate of its own leaves it serving none,
/// and a later attach records a certificate again, so the member round-trips
/// through both calls however often each is made.
#[tokio::test]
async fn detaching_is_idempotent_and_an_attach_records_a_certificate_again() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "cycle-origin", ORIGIN_ZONE).await;
    create_certificate(
        &emulator,
        "cycle-cert",
        "cycle.example.com",
        &["www.cycle.example.com"],
    )
    .await;
    wait_for_issued_certificate(&emulator, "cycle-cert").await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "cycle-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "cycle-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;

    /// One call of the cycle: whether it attaches, and what the distribution
    /// must report afterwards.
    struct Step {
        case: &'static str,
        attach: bool,
        certificate_name: Option<&'static str>,
        domains: &'static [&'static str],
        in_use: i64,
    }

    // The steps span the orderings the rule has to hold over: a detach before
    // any attach, two detaches in a row, an attach after a detach, and a detach
    // after that attach.
    let steps: &[Step] = &[
        Step {
            case: "a detach before any attach",
            attach: false,
            certificate_name: None,
            domains: &[],
            in_use: 0,
        },
        Step {
            case: "a second detach in a row",
            attach: false,
            certificate_name: None,
            domains: &[],
            in_use: 0,
        },
        Step {
            case: "an attach after the detaches",
            attach: true,
            certificate_name: Some("cycle-cert"),
            domains: &["cycle.example.com", "www.cycle.example.com"],
            in_use: 1,
        },
        Step {
            case: "a detach after the attach",
            attach: false,
            certificate_name: None,
            domains: &[],
            in_use: 0,
        },
        Step {
            case: "an attach after that detach",
            attach: true,
            certificate_name: Some("cycle-cert"),
            domains: &["cycle.example.com", "www.cycle.example.com"],
            in_use: 1,
        },
    ];

    for step in steps {
        let case = step.case;
        let answered = if step.attach {
            emulator
                .ok(
                    "AttachCertificateToDistribution",
                    json!({"distributionName": "cycle-cdn", "certificateName": "cycle-cert"}),
                )
                .await
        } else {
            emulator
                .ok(
                    "DetachCertificateFromDistribution",
                    json!({"distributionName": "cycle-cdn"}),
                )
                .await
        };
        // Either call IS a configuration change, so it rolls out like any
        // other: the entry it answers is non-terminal.
        assert_eq!(
            answered["operation"]["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected the change to be rolling out, got {answered:?}"
        );
        let reported = distribution(&emulator, "cycle-cdn").await;
        assert_eq!(
            reported["certificateName"].as_str(),
            step.certificate_name,
            "case {case}: expected the certificate {:?}, got {reported:?}",
            step.certificate_name
        );
        assert_eq!(
            string_list(&reported["alternativeDomainNames"]),
            step.domains,
            "case {case}: expected the domains {:?}, got {reported:?}",
            step.domains
        );
        let in_use = certificate(&emulator, "cycle-cert").await;
        assert_eq!(
            in_use["certificateDetail"]["inUseResourceCount"].as_i64(),
            Some(step.in_use),
            "case {case}: expected the certificate in use by {} resources, got {in_use:?}",
            step.in_use
        );
    }
}

/// RULE: a detach naming a distribution this account does not hold is refused
/// with Lightsail's `DoesNotExist` envelope and detaches nothing — no other
/// distribution loses the certificate it is serving.
#[tokio::test]
async fn a_detach_naming_no_distribution_of_the_account_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "guarded-origin", ORIGIN_ZONE).await;
    create_certificate(&emulator, "guarded-cert", "guarded.example.com", &[]).await;
    wait_for_issued_certificate(&emulator, "guarded-cert").await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "guarded-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "guarded-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;
    emulator
        .ok(
            "AttachCertificateToDistribution",
            json!({"distributionName": "guarded-cdn", "certificateName": "guarded-cert"}),
        )
        .await;

    // Names spanning what a request can carry and no distribution answers to: a
    // name nothing was ever created under, the name of a resource of ANOTHER
    // kind this account does hold, and a name a deleted distribution had.
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "gone-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "guarded-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;
    emulator
        .ok("DeleteDistribution", json!({"distributionName": "gone-cdn"}))
        .await;
    for name in ["ghost-cdn", "guarded-origin", "gone-cdn"] {
        let (status, body) = emulator
            .call(
                "DetachCertificateFromDistribution",
                json!({"distributionName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case the name {name}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "NotFoundException",
            "case the name {name}: expected the wire error code NotFoundException, got {code} \
             {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case the name {name}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(name)),
            "case the name {name}: expected a message naming what was not found, got {body:?}"
        );
        let guarded = distribution(&emulator, "guarded-cdn").await;
        assert_eq!(
            guarded["certificateName"].as_str(),
            Some("guarded-cert"),
            "case the name {name}: expected the refusal to detach nothing, got {guarded:?}"
        );
        assert_eq!(
            string_list(&guarded["alternativeDomainNames"]),
            vec!["guarded.example.com".to_string()],
            "case the name {name}: expected the refusal to withdraw no domain, got {guarded:?}"
        );
    }
}

/// RULE: Lightsail serves the distribution APIs from the N. Virginia endpoint
/// alone — the modelled `InvalidInputException` documentation says to set the
/// Region to us-east-1 "to create, view, or edit these resources" — so a detach
/// signed for any other Region is refused and detaches nothing.
#[tokio::test]
async fn a_detach_outside_the_distribution_region_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "region-detach-origin", ORIGIN_ZONE).await;
    create_certificate(
        &emulator,
        "region-detach-cert",
        "region.detach.example.com",
        &[],
    )
    .await;
    wait_for_issued_certificate(&emulator, "region-detach-cert").await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "region-detach-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "region-detach-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;
    let attached = emulator
        .ok(
            "AttachCertificateToDistribution",
            json!({
                "distributionName": "region-detach-cdn",
                "certificateName": "region-detach-cert",
            }),
        )
        .await;
    assert_eq!(
        attached["operation"]["status"].as_str(),
        Some("Started"),
        "case the setup attach: expected a Started operation, got {attached:?}"
    );

    // Rows spanning the Regions a caller might sign for: another Region this
    // service serves resources in, and one it does not.
    for region in ["us-west-2", "eu-central-1", "mars-central-1"] {
        let (status, body) = emulator
            .call_in(
                Some(region),
                "DetachCertificateFromDistribution",
                json!({"distributionName": "region-detach-cdn"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case signed for {region}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case signed for {region}: expected InvalidInputException, got {body:?}"
        );
        let guarded = distribution(&emulator, "region-detach-cdn").await;
        assert_eq!(
            guarded["certificateName"].as_str(),
            Some("region-detach-cert"),
            "case signed for {region}: expected the refusal to detach nothing, got {guarded:?}"
        );
    }

    // And the same call in the distribution Region is served, leaving the
    // distribution rolling the release out.
    let answered = emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            "DetachCertificateFromDistribution",
            json!({"distributionName": "region-detach-cdn"}),
        )
        .await;
    assert_eq!(
        answered["operation"]["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case signed for {DISTRIBUTION_REGION}: expected the detach to be served, got \
         {answered:?}"
    );
    let detached = distribution(&emulator, "region-detach-cdn").await;
    assert_eq!(
        detached["certificateName"],
        Value::Null,
        "case signed for {DISTRIBUTION_REGION}: expected the certificate released, got \
         {detached:?}"
    );
    // The status the change rolls out through is asserted on the entry the call
    // itself answered, not on this read, which the clock may already have
    // settled.
    assert_eq!(
        answered["operation"]["isTerminal"].as_bool(),
        Some(false),
        "case signed for {DISTRIBUTION_REGION}: expected a change still rolling out (the \
         distribution leaves {IN_PROGRESS} only once it is {DEPLOYED}), got {answered:?}"
    );
}
