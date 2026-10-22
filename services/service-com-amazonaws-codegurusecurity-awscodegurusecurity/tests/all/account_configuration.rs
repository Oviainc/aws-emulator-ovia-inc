// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the CodeGuru Security account encryption configuration:
//! `UpdateAccountConfiguration` -> `GetAccountConfiguration`, the not-found refusal an
//! account with no configuration record answers an update with, and the effect the
//! stored configuration has on `CreateUploadUrl`'s required upload headers.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), reusing
//! the harness of `crate::scan_lifecycle`, so the restJson1 wire shape, the HTTP status
//! of each modelled error and the shared engine store are exercised exactly as a client
//! would exercise them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::scan_lifecycle::{
    artifact_for, call, create_scan, create_upload_url, harness, store_of, Answer,
};

/// KMS key ARNs this emulator must accept, one per form the modelled
/// `EncryptionConfig$kmsKeyArn` pattern admits
/// (`^arn:aws:kms:[\S]+:[\d]{12}:key\/(([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})|(mrk-[0-9a-zA-Z]{32}))$`,
/// from `smithy_query(query="shape",shape="EncryptionConfig")`): a single-region key id,
/// a key in another region and account, and a multi-region (`mrk-`) key id.
const SINGLE_REGION_KEY: &str = "arn:aws:kms:us-west-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";
const OTHER_REGION_KEY: &str =
    "arn:aws:kms:eu-central-1:210987654321:key/abcdef01-2345-6789-abcd-ef0123456789";
const MULTI_REGION_KEY: &str =
    "arn:aws:kms:ap-southeast-2:999999999999:key/mrk-80bd8ecdcd4342aebd84b7dc9da5a3cf";

/// `GET /accountConfiguration/get`, the modelled route of `GetAccountConfiguration`.
async fn get_account_configuration(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/accountConfiguration/get", None).await
}

/// `PUT /updateAccountConfiguration` carrying `encryption_config` as the required
/// `encryptionConfig` member.
async fn update_account_configuration(
    router: &MultiServiceRouter,
    encryption_config: Value,
) -> Answer {
    call(
        router,
        "PUT",
        "/updateAccountConfiguration",
        Some(json!({ "encryptionConfig": encryption_config })),
    )
    .await
}

/// The `encryptionConfig.kmsKeyArn` an answer reports, or `None` when the configuration
/// is empty (the AWS-managed key).
fn reported_key(case: &str, answer: &Answer) -> Option<String> {
    let config = answer.get("encryptionConfig");
    assert!(
        config.is_object(),
        "case {case:?}: encryptionConfig must be present as an object, got {}",
        answer.json
    );
    config["kmsKeyArn"].as_str().map(str::to_string)
}

/// The `encryptionConfig` body for a request that configures `key`, or that asks for the
/// AWS-managed key by passing an `encryptionConfig` with nothing in it.
fn encryption_config_body(key: Option<&str>) -> Value {
    match key {
        Some(key) => json!({ "kmsKeyArn": key }),
        None => json!({}),
    }
}

/// Every form of a well-formed `encryptionConfig` payload: each key form the modelled
/// pattern admits, and the empty configuration that asks for the AWS-managed key.
const WELL_FORMED_CONFIGS: &[(&str, Option<&str>)] = &[
    ("a single-region key", Some(SINGLE_REGION_KEY)),
    ("a key in another region and account", Some(OTHER_REGION_KEY)),
    ("a multi-region key", Some(MULTI_REGION_KEY)),
    ("an empty configuration", None),
];

/// Onboard the account by minting its first code artifact, the way a client that is
/// about to scan a resource does, and return that artifact's `codeArtifactId`.
async fn onboard_with_artifact(router: &MultiServiceRouter, scan_name: &str) -> String {
    artifact_for(router, scan_name).await
}

#[tokio::test]
async fn updating_the_configuration_of_an_account_that_has_none_is_not_found() {
    // The rule: UpdateAccountConfiguration updates the account's EXISTING configuration
    // record and never brings one into existence, so an account CodeGuru Security holds
    // no configuration for is refused with the modelled ResourceNotFoundException (HTTP
    // 404, `errorKind` client, from
    // `smithy_query(query="operation",operation="UpdateAccountConfiguration")`).
    //
    // The rows differ along the request's own `encryptionConfig`: every key form the
    // modelled pattern admits, and the empty configuration that asks for the AWS-managed
    // key. The refusal is a property of the account's state, not of the payload, so all
    // of them are refused the same way.
    for (case, key) in WELL_FORMED_CONFIGS {
        let (engine, router) = harness();

        let answer = update_account_configuration(&router, encryption_config_body(*key)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {}",
            answer.status, answer.json
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} {}",
            answer.error_type, answer.json
        );
        let message = answer.json["message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must carry a populated message, got {}",
            answer.json
        );
        assert_eq!(
            answer.json["resourceType"].as_str(),
            Some("accountConfiguration"),
            "case {case:?}: the refusal must name the resource type, got {}",
            answer.json
        );
        assert_eq!(
            answer.json["resourceId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the refusal must name the account, got {}",
            answer.json
        );

        // A refused update configures nothing: no record appears, and the reader still
        // answers the documented default.
        let stored = store_of(&engine).lock().account_configuration.clone();
        assert!(
            stored.is_none(),
            "case {case:?}: a refused update must create no configuration, got {stored:?}"
        );
        let read = get_account_configuration(&router).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAccountConfiguration expected 200, got {} {}",
            read.status, read.json
        );
        let reported = reported_key(case, &read);
        assert_eq!(
            reported, None,
            "case {case:?}: expected an empty encryptionConfig, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn account_configuration_defaults_to_the_aws_managed_key() {
    let (engine, router) = harness();

    // "By default, CodeGuru Security uses an AWS-managed key for encryption ... If you do
    // not specify a customer-managed key, returns empty"
    // (`GetAccountConfigurationResponse$encryptionConfig`). Read twice: the operation is
    // `@readonly`, so the second read must report exactly what the first did — and a read
    // neither configures a key nor onboards the account.
    for read in 1..=2 {
        let answer = get_account_configuration(&router).await;
        assert_eq!(
            answer.status, 200,
            "read {read}: expected 200, got {} {}",
            answer.status, answer.json
        );
        let reported = reported_key("no configured key", &answer);
        assert_eq!(
            reported, None,
            "read {read}: expected an empty encryptionConfig, got {reported:?} in {}",
            answer.json
        );
        let stored = store_of(&engine).lock().account_configuration.clone();
        assert!(
            stored.is_none(),
            "read {read}: reading the configuration must create no record, got {stored:?}"
        );
    }

    // An onboarded account still uses the AWS-managed key until an update names one: the
    // record exists, and the reported configuration is still empty.
    onboard_with_artifact(&router, "defaults-scan").await;
    let answer = get_account_configuration(&router).await;
    let reported = reported_key("onboarded, no key named", &answer);
    assert_eq!(
        reported, None,
        "an onboarded account with no key named must report an empty encryptionConfig, \
         got {reported:?}"
    );
    let stored = store_of(&engine).lock().account_configuration.clone();
    let stored = stored.expect("the first code artifact onboards the account");
    assert_eq!(
        stored.encryption_kms_key_arn, None,
        "onboarding must configure no key, got {:?}",
        stored.encryption_kms_key_arn
    );
    assert!(
        stored.created_at_millis > 0,
        "the record must be stamped with the instant it was created, got {}",
        stored.created_at_millis
    );
}

#[tokio::test]
async fn each_resource_creating_call_onboards_the_account_for_updates() {
    // The configuration record appears when the account starts using CodeGuru Security.
    // The rows differ along WHICH resource-creating call the account makes first — a code
    // artifact alone, and a code artifact followed by a scan recorded on it — and in both
    // cases the update that was refused before is accepted afterwards.
    let cases: &[(&str, bool)] = &[
        ("a code artifact alone", false),
        ("a code artifact and a scan", true),
    ];

    for (case, also_scan) in cases {
        let (engine, router) = harness();
        let scan_name = "onboarding-scan";

        let refused =
            update_account_configuration(&router, encryption_config_body(Some(SINGLE_REGION_KEY)))
                .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: before any resource exists the update must be refused, got {} {}",
            refused.status, refused.json
        );

        let artifact = onboard_with_artifact(&router, scan_name).await;
        if *also_scan {
            let created = create_scan(&router, json!({ "scanName": scan_name }), &artifact).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateScan expected 200, got {} {}",
                created.status, created.json
            );
        }

        let accepted =
            update_account_configuration(&router, encryption_config_body(Some(MULTI_REGION_KEY)))
                .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: after {case} the update must be accepted, got {} {}",
            accepted.status, accepted.json
        );
        let echoed = reported_key(case, &accepted);
        assert_eq!(
            echoed.as_deref(),
            Some(MULTI_REGION_KEY),
            "case {case:?}: expected the configured key back, got {echoed:?}"
        );
        let stored = store_of(&engine).lock().encryption_kms_key_arn();
        assert_eq!(
            stored.as_deref(),
            Some(MULTI_REGION_KEY),
            "case {case:?}: the store must hold the configured key, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn get_account_configuration_reports_the_key_the_last_update_committed() {
    let (engine, router) = harness();
    // The account holds a CodeGuru Security resource, so it has a configuration record to
    // update.
    onboard_with_artifact(&router, "roundtrip-scan").await;

    // The rule: GetAccountConfiguration reports the key the last
    // UpdateAccountConfiguration committed, and an empty configuration when none is
    // configured. The rows differ along the FORM of the key (single-region id,
    // multi-region `mrk-` id), its region and account, and the PRESENCE of the member —
    // an `encryptionConfig` with no `kmsKeyArn` asks for the AWS-managed key again
    // ("If you previously specified a customer-managed KMS key and want CodeGuru
    // Security to use an AWS-managed key for encryption instead, pass nothing").
    let cases: &[(&str, Option<&str>)] = &[
        ("configure a single-region key", Some(SINGLE_REGION_KEY)),
        ("replace it with a key in another region and account", Some(OTHER_REGION_KEY)),
        ("replace it with a multi-region key", Some(MULTI_REGION_KEY)),
        ("repeat the same multi-region key", Some(MULTI_REGION_KEY)),
        ("clear it back to the AWS-managed key", None),
        ("clear it again", None),
        ("configure a key after clearing", Some(SINGLE_REGION_KEY)),
    ];

    for (case, key) in cases {
        let updated = update_account_configuration(&router, encryption_config_body(*key)).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateAccountConfiguration expected 200, got {} {}",
            updated.status, updated.json
        );
        let echoed = reported_key(case, &updated);
        assert_eq!(
            echoed.as_deref(),
            *key,
            "case {case:?}: UpdateAccountConfiguration expected {key:?}, got {echoed:?}"
        );

        // Read it back twice: the committed value, and the same value again because a
        // read changes nothing.
        for read in 1..=2 {
            let answer = get_account_configuration(&router).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} read {read}: expected 200, got {} {}",
                answer.status, answer.json
            );
            let reported = reported_key(case, &answer);
            assert_eq!(
                reported.as_deref(),
                *key,
                "case {case:?} read {read}: expected {key:?}, got {reported:?}"
            );
        }

        let stored = store_of(&engine).lock().encryption_kms_key_arn();
        assert_eq!(
            stored.as_deref(),
            *key,
            "case {case:?}: the store must hold {key:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn the_configured_key_drives_the_required_upload_headers() {
    let (_engine, router) = harness();

    // The configuration is ONE value: CreateUploadUrl derives the server-side-encryption
    // headers of a minted upload target from the same store GetAccountConfiguration
    // reports, so configuring a key adds them and clearing it removes them again. The
    // first minted artifact is also what onboards the account, so the first row needs no
    // update at all.
    let cases: &[(&str, Option<&str>)] = &[
        ("no key configured", None),
        ("single-region key", Some(SINGLE_REGION_KEY)),
        ("multi-region key", Some(MULTI_REGION_KEY)),
        ("cleared again", None),
    ];

    let mut onboarded = false;
    for (case, key) in cases {
        if onboarded {
            let updated = update_account_configuration(&router, encryption_config_body(*key)).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: UpdateAccountConfiguration expected 200, got {} {}",
                updated.status, updated.json
            );
        }

        let minted = create_upload_url(&router, "encryption-config-scan").await;
        assert_eq!(
            minted.status, 200,
            "case {case:?}: CreateUploadUrl expected 200, got {} {}",
            minted.status, minted.json
        );
        onboarded = true;
        let headers = minted.get("requestHeaders").clone();
        assert_eq!(
            headers["Content-Type"].as_str(),
            Some("application/zip"),
            "case {case:?}: the archive content type must always be required, got {headers}"
        );
        let algorithm = headers["x-amz-server-side-encryption"]
            .as_str()
            .map(str::to_string);
        let header_key = headers["x-amz-server-side-encryption-aws-kms-key-id"]
            .as_str()
            .map(str::to_string);
        let expected_algorithm = key.map(|_| "aws:kms".to_string());
        assert_eq!(
            algorithm, expected_algorithm,
            "case {case:?}: expected encryption algorithm {expected_algorithm:?}, got {algorithm:?} in {headers}"
        );
        assert_eq!(
            header_key.as_deref(),
            *key,
            "case {case:?}: expected the upload headers to name {key:?}, got {header_key:?} in {headers}"
        );

        // And the reader agrees with what the upload headers were built from.
        let reported = reported_key(case, &get_account_configuration(&router).await);
        assert_eq!(
            reported.as_deref(),
            *key,
            "case {case:?}: GetAccountConfiguration expected {key:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_kms_key_arn_outside_the_modelled_pattern_is_refused_and_changes_nothing() {
    let (engine, router) = harness();

    // Onboard the account and configure a key first, so a refused update is observable as
    // "the previous key is still the one configured" and not merely as "nothing was
    // configured".
    onboard_with_artifact(&router, "validation-scan").await;
    let seeded = update_account_configuration(
        &router,
        encryption_config_body(Some(SINGLE_REGION_KEY)),
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "seeding a key: expected 200, got {} {}",
        seeded.status, seeded.json
    );

    // Each row breaks exactly one clause of the modelled `EncryptionConfig$kmsKeyArn`
    // constraint (length 1..2048 and the ARN pattern cited at the top of this file): a
    // value that is no ARN at all, a non-KMS service, an account id that is not twelve
    // digits, a resource that is not `key/<id>`, a key id outside the hex alphabet, a
    // multi-region id of the wrong length, and the empty string the minimum length
    // forbids.
    let refused: &[(&str, &str)] = &[
        ("not an ARN", "my-kms-key"),
        (
            "another service's ARN",
            "arn:aws:s3:us-west-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "account id too short",
            "arn:aws:kms:us-west-2:12345:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "an alias rather than a key",
            "arn:aws:kms:us-west-2:123456789012:alias/my-key",
        ),
        (
            "key id outside the hex alphabet",
            "arn:aws:kms:us-west-2:123456789012:key/ZZZZabcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "multi-region key id of the wrong length",
            "arn:aws:kms:us-west-2:123456789012:key/mrk-80bd8ecd",
        ),
        ("the empty string", ""),
    ];

    for (case, value) in refused {
        let answer =
            update_account_configuration(&router, encryption_config_body(Some(value))).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for {value:?}, got {} {}",
            answer.status, answer.json
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: expected ValidationException for {value:?}, got {:?} {}",
            answer.error_type,
            answer.json
        );

        let stored = store_of(&engine).lock().encryption_kms_key_arn();
        assert_eq!(
            stored.as_deref(),
            Some(SINGLE_REGION_KEY),
            "case {case:?}: a refused update must leave the configured key, got {stored:?}"
        );
        let reported = reported_key(case, &get_account_configuration(&router).await);
        assert_eq!(
            reported.as_deref(),
            Some(SINGLE_REGION_KEY),
            "case {case:?}: GetAccountConfiguration must still report the configured key, got {reported:?}"
        );
    }
}
