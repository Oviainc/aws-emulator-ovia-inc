// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteBucketEncryption`: RESETTING a general purpose bucket's
//! default encryption.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, body, headers, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `DeleteBucketEncryption` "resets the default encryption for the bucket as server-side
//!   encryption with Amazon S3 managed keys (SSE-S3)" (the operation's own documentation):
//!   the bucket's OWN configuration goes in full, whatever it named, so the bucket is back
//!   in the state `CreateBucket` left it in and `GetBucketEncryption` reports the documented
//!   SSE-S3 default again. The call answers the modelled 204 with an empty body.
//! * Resetting a subresource is IDEMPOTENT: a bucket carrying no configuration of its own is
//!   already in the asked-for state, so a delete against it — and a second delete after a
//!   first — still answers 204 and reports the same default.
//! * Only that one subresource is withdrawn: the bucket's versioning status, its tag set and
//!   its objects survive the delete, every object keeps the encryption it was STORED under,
//!   and a later `PutBucketEncryption` configures the bucket again.
//! * The default this restores is the one a write inherits: an object written with none of
//!   `PutObject`'s encryption members after the delete is stored under SSE-S3
//!   (`PutObjectRequest$ServerSideEncryption`, `…$BucketKeyEnabled`), with no KMS key and no
//!   Bucket Key header.
//! * `Bucket` is an @httpLabel, so a call naming a bucket this emulator does not hold
//!   answers `NoSuchBucket` (HTTP 404); an `ExpectedBucketOwner` that is not this account's
//!   answers `AccessDenied` (HTTP 403) and leaves the configuration in place.

use service_com_amazonaws_s3_amazons3::state::{
    DEFAULT_BLOCKED_ENCRYPTION_TYPES, DEFAULT_BUCKET_KEY_ENABLED, DEFAULT_SSE_ALGORITHM,
    MAX_SERVER_SIDE_ENCRYPTION_RULES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{
    as_reported, get_encryption, put_encryption, reported_rules, Rule, ENCRYPTION_TYPE_VALUES,
    SERVER_SIDE_ENCRYPTION_VALUES, XMLNS,
};
use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::object_and_tagging_roundtrip::{get_object, put_object, text};

/// A KMS key in each of the three forms `KMSMasterKeyID` documents ("you can specify the key
/// ID, key alias, or the Amazon Resource Name (ARN) of the KMS key").
const KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";
const KEY_ARN: &str = "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";
const KEY_ALIAS: &str = "alias/compliance-cmk";

/// `DELETE /<bucket>?encryption` — `DeleteBucketEncryption`, with any bound headers.
async fn delete_encryption(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "DELETE", &format!("/{bucket}?encryption"), headers, "").await
}

/// The one rule a bucket with no configuration of its own reports, spelled as the SERVICE
/// documents it rather than as this crate's constants hold it: SSE-S3 is the
/// `ServerSideEncryption` value `AES256` ("server-side encryption with Amazon S3 managed
/// keys (SSE-S3)", this operation's and `GetBucketEncryption`'s documentation), "General
/// purpose buckets - By default, S3 Bucket Key is not enabled"
/// (`ServerSideEncryptionRule$BucketKeyEnabled`), and a general purpose bucket blocks
/// server-side encryption with customer-provided keys, the `EncryptionType` value `SSE-C`
/// (`ServerSideEncryptionRule$BlockedEncryptionTypes`).
///
/// Every assertion about the restored default compares against THIS form, so a change to
/// the crate's own defaults cannot make the suite agree with itself;
/// [`the_restored_default_is_the_one_the_crate_publishes`] is what ties the two together.
fn documented_default_rule() -> Rule {
    Rule::by_default("AES256")
        .with_bucket_key(false)
        .blocking(&["SSE-C"])
}

#[test]
fn the_restored_default_is_the_one_the_crate_publishes() {
    // The constants the handlers compute the default from must be the documented default
    // above; otherwise one of the two is wrong, whichever way they were changed.
    let published = Rule::by_default(DEFAULT_SSE_ALGORITHM)
        .with_bucket_key(DEFAULT_BUCKET_KEY_ENABLED)
        .blocking(&DEFAULT_BLOCKED_ENCRYPTION_TYPES);
    assert_eq!(
        published,
        documented_default_rule(),
        "the crate's published default encryption must be the documented SSE-S3 one, got \
         {published:?}"
    );
}

/// Assert one `DELETE ?encryption` answered the modelled 204 with an empty body.
fn assert_reset(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: expected the delete to answer the modelled 204, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: a successful delete carries no body, got {:?}",
        text(answer)
    );
}

/// Assert the bucket reports exactly the SSE-S3 default, which is what a bucket that was
/// never configured and a bucket whose configuration was reset both do.
async fn assert_default_reported(router: &MultiServiceRouter, bucket: &str, case: &str) {
    let read = get_encryption(router, bucket).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected the read to answer 200, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        reported_rules(&read),
        vec![documented_default_rule()],
        "case {case:?}: expected the documented SSE-S3 default back, got {:?}",
        reported_rules(&read)
    );
}

/// The table of configurations every row of which must be reset in full.
///
/// The rows differ along what the reset rule generalises over: whether the bucket carried a
/// configuration of its own at all, WHICH `SSEAlgorithm` its rules named (every value the
/// enum publishes), which FORM the KMS key took (key id, key ARN, alias), whether the Bucket
/// Key was on, off or unnamed, which `EncryptionType` values were blocked and in which
/// order, and how MANY rules the configuration carried.
fn reset_rows() -> Vec<(&'static str, Vec<Rule>)> {
    let mut rows: Vec<(&'static str, Vec<Rule>)> = vec![
        ("no configuration was ever put", Vec::new()),
        (
            "the SSE-S3 default named explicitly",
            vec![Rule::by_default("AES256")],
        ),
        (
            "SSE-KMS with a key id and the Bucket Key on",
            vec![Rule::by_default("aws:kms")
                .with_key(KEY_ID)
                .with_bucket_key(true)],
        ),
        (
            "SSE-KMS with a fully qualified key ARN and the Bucket Key off",
            vec![Rule::by_default("aws:kms")
                .with_key(KEY_ARN)
                .with_bucket_key(false)],
        ),
        (
            "DSSE-KMS with the key by alias",
            vec![Rule::by_default("aws:kms:dsse").with_key(KEY_ALIAS)],
        ),
        (
            "a rule naming only BucketKeyEnabled",
            vec![Rule::bucket_key_only(true)],
        ),
        (
            "every published EncryptionType blocked, in the model's order",
            vec![Rule::by_default("AES256").blocking(&ENCRYPTION_TYPE_VALUES)],
        ),
        (
            "the published EncryptionType values in the other order",
            vec![Rule::by_default("aws:kms")
                .with_key(KEY_ID)
                .blocking(&["SSE-C", "NONE"])],
        ),
        (
            "two rules, reported in the order they arrived",
            vec![
                Rule::by_default("aws:kms:dsse").with_key(KEY_ARN),
                Rule::by_default("AES256").with_bucket_key(false),
            ],
        ),
        (
            "exactly as many rules as this emulator records for one bucket",
            (0..MAX_SERVER_SIDE_ENCRYPTION_RULES)
                .map(|index| Rule::bucket_key_only(index % 2 == 0))
                .collect(),
        ),
    ];
    // One row per published algorithm on its own, so the reset is exercised over the whole
    // catalog rather than the one value the recorded call configured.
    for algorithm in SERVER_SIDE_ENCRYPTION_VALUES {
        rows.push((
            "a single rule naming one published algorithm",
            vec![Rule::by_default(algorithm)],
        ));
    }
    rows
}

/// Whatever the bucket carried, the delete takes the whole configuration and the bucket
/// reports the documented SSE-S3 default again — and says so with the modelled 204.
#[tokio::test]
async fn a_delete_resets_the_bucket_to_the_sse_s3_default() {
    let router = router();

    for (index, (case, rules)) in reset_rows().into_iter().enumerate() {
        let bucket = format!("encryption-reset-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            text(&created)
        );

        if rules.is_empty() {
            assert_default_reported(&router, &bucket, case).await;
        } else {
            let put = put_encryption(&router, &bucket, &rules).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: the configuration must be applied first, got {} with body {:?}",
                put.status,
                text(&put)
            );
            let read = get_encryption(&router, &bucket).await;
            assert_eq!(
                reported_rules(&read),
                as_reported(&rules),
                "case {case:?}: the configuration must be in force before the delete, got {:?}",
                reported_rules(&read)
            );
        }

        let reset = delete_encryption(&router, &bucket, &[]).await;
        assert_reset(&reset, case);
        assert_default_reported(&router, &bucket, case).await;

        // Idempotent: the configuration is already gone, so a second delete is the same 204
        // and reports the same default.
        let again = delete_encryption(&router, &bucket, &[]).await;
        assert_reset(&again, case);
        assert_default_reported(&router, &bucket, case).await;

        // And the bucket takes a configuration again afterwards: the delete reset the
        // subresource, it did not bar it.
        let reinstated = vec![Rule::by_default("aws:kms")
            .with_key(KEY_ALIAS)
            .with_bucket_key(true)];
        let put = put_encryption(&router, &bucket, &reinstated).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: a put after the delete must answer 200, got {} with body {:?}",
            put.status,
            text(&put)
        );
        let read = get_encryption(&router, &bucket).await;
        assert_eq!(
            reported_rules(&read),
            as_reported(&reinstated),
            "case {case:?}: the reinstated configuration must be reported, got {:?}",
            reported_rules(&read)
        );
    }
}

/// The delete is scoped to the encryption subresource: everything else the bucket holds is
/// exactly as it was, and each stored object keeps the encryption it was written under.
#[tokio::test]
async fn a_delete_leaves_the_rest_of_the_bucket_alone() {
    // The object below is written while a KMS default is in force, so the default has to
    // name a key KMS really holds: a write stored under a KMS key resolves that reference
    // against KMS before storing anything ([`super::kms_key_references`]).
    let router = super::kms_key_references::router_with_kms();
    let (_, key_arn) =
        super::kms_key_references::created_key(&router, "the key this default names").await;
    let key_arn = key_arn.as_str();
    let bucket = "encryption-reset-scope";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );

    let versioning = format!(
        "<VersioningConfiguration xmlns=\"{XMLNS}\"><Status>Enabled</Status>\
         </VersioningConfiguration>"
    );
    let versioned = send(
        &router,
        "PUT",
        &format!("/{bucket}?versioning"),
        &[],
        &versioning,
    )
    .await;
    assert_eq!(
        versioned.status, 200,
        "the bucket must take its versioning status, got {} with body {:?}",
        versioned.status,
        text(&versioned)
    );
    let tagging = format!(
        "<Tagging xmlns=\"{XMLNS}\"><TagSet><Tag><Key>team</Key><Value>storage</Value></Tag>\
         </TagSet></Tagging>"
    );
    let tagged = send(&router, "PUT", &format!("/{bucket}?tagging"), &[], &tagging).await;
    assert_eq!(
        tagged.status, 204,
        "the bucket must take its tag set, got {} with body {:?}",
        tagged.status,
        text(&tagged)
    );

    let configured = vec![Rule::by_default("aws:kms")
        .with_key(key_arn)
        .with_bucket_key(true)];
    let put = put_encryption(&router, bucket, &configured).await;
    assert_eq!(
        put.status, 200,
        "the configuration must be applied first, got {} with body {:?}",
        put.status,
        text(&put)
    );

    // An object written while the KMS default is in force, so what it was STORED under can
    // be shown to survive the reset.
    let stored = put_object(&router, bucket, "keep.txt", b"keep me\n", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    let reset = delete_encryption(&router, bucket, &[]).await;
    assert_reset(&reset, "a furnished bucket");
    assert_default_reported(&router, bucket, "a furnished bucket").await;

    let read_versioning = send(&router, "GET", &format!("/{bucket}?versioning"), &[], "").await;
    assert_eq!(
        read_versioning.element("Status").as_deref(),
        Some("Enabled"),
        "expected the versioning status to survive the delete, got {:?}",
        text(&read_versioning)
    );
    let read_tags = send(&router, "GET", &format!("/{bucket}?tagging"), &[], "").await;
    assert_eq!(
        read_tags.element("Value").as_deref(),
        Some("storage"),
        "expected the tag set to survive the delete, got {:?}",
        text(&read_tags)
    );
    let object = get_object(&router, bucket, "keep.txt", "", &[]).await;
    assert_eq!(
        object.status, 200,
        "expected the stored object to survive the delete, got {} with body {:?}",
        object.status,
        text(&object)
    );
    assert_eq!(
        object.header("x-amz-server-side-encryption").as_deref(),
        Some("aws:kms"),
        "expected the object to keep the encryption it was stored under, got {:?}",
        object.header("x-amz-server-side-encryption")
    );
    assert_eq!(
        object
            .header("x-amz-server-side-encryption-aws-kms-key-id")
            .as_deref(),
        Some(key_arn),
        "expected the object to keep the key it was stored under, got {:?}",
        object.header("x-amz-server-side-encryption-aws-kms-key-id")
    );
}

/// The default the delete restores is the one the NEXT write inherits: SSE-S3, with no KMS
/// key and no Bucket Key header.
#[tokio::test]
async fn a_write_after_the_delete_inherits_the_restored_default() {
    let router = router();

    // Each row configures a different default, so what is pinned is "the write after the
    // reset inherits SSE-S3" rather than the undoing of one configuration.
    for (index, (case, rules)) in [
        (
            "a KMS default by ARN with the Bucket Key on",
            vec![Rule::by_default("aws:kms")
                .with_key(KEY_ARN)
                .with_bucket_key(true)],
        ),
        (
            "a DSSE-KMS default by key id",
            vec![Rule::by_default("aws:kms:dsse").with_key(KEY_ID)],
        ),
        (
            "a default naming only the Bucket Key",
            vec![Rule::bucket_key_only(true)],
        ),
        (
            "two rules, the second of which names the algorithm",
            vec![
                Rule::bucket_key_only(false),
                Rule::by_default("aws:kms").with_key(KEY_ALIAS),
            ],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let bucket = format!("encryption-reset-inherited-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );
        let put = put_encryption(&router, &bucket, &rules).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the configuration must be applied first, got {} with body {:?}",
            put.status,
            text(&put)
        );

        let reset = delete_encryption(&router, &bucket, &[]).await;
        assert_reset(&reset, case);

        let written = put_object(&router, &bucket, "after.txt", b"written after the reset\n", &[])
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must answer 200, got {} with body {:?}",
            written.status,
            text(&written)
        );
        for (operation, answer) in [
            ("PutObject", written),
            ("GetObject", get_object(&router, &bucket, "after.txt", "", &[]).await),
        ] {
            assert_eq!(
                answer.header("x-amz-server-side-encryption").as_deref(),
                // The documented algorithm, not the crate's constant: SSE-S3 is `AES256`.
                Some("AES256"),
                "case {case:?}/{operation}: a write after the reset inherits the SSE-S3 \
                 default, got {:?}",
                answer.header("x-amz-server-side-encryption")
            );
            for header in [
                "x-amz-server-side-encryption-aws-kms-key-id",
                "x-amz-server-side-encryption-bucket-key-enabled",
            ] {
                assert_eq!(
                    answer.header(header),
                    None,
                    "case {case:?}/{operation}: the SSE-S3 default carries no {header}, got {:?}",
                    answer.header(header)
                );
            }
        }
    }
}

/// A delete against a bucket this emulator does not hold is `NoSuchBucket` (HTTP 404), never
/// the 204 of a delete that reset something — and it creates nothing.
#[tokio::test]
async fn a_delete_against_a_bucket_this_emulator_does_not_hold_is_no_such_bucket() {
    let router = router();
    let held = "held-encryption-reset-bucket";
    create_bucket(&router, held, None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it, an
    // extension of it, one differing only in case (bucket names are case-sensitive), and a
    // dotted name.
    for bucket in [
        "absent-encryption-reset-bucket",
        "held",
        "held-encryption-reset-bucket-2",
        "Held-Encryption-Reset-Bucket",
        "another.dotted.name",
    ] {
        let refused = delete_encryption(&router, bucket, &[]).await;
        assert_eq!(
            refused.status, 404,
            "case {bucket:?}: expected HTTP 404 for a bucket this emulator does not hold, got \
             {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: expected the modelled error code, got {:?}",
            refused.error_code()
        );

        // The refusal created nothing: the read is refused the same way.
        let read = get_encryption(&router, bucket).await;
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: a refused delete must not have created the bucket, got {:?}",
            read.error_code()
        );
    }

    // The bucket that IS held resets normally, so the rows above failed on the lookup rather
    // than on the operation being unreachable.
    let reset = delete_encryption(&router, held, &[]).await;
    assert_reset(&reset, "the held bucket");
    assert_default_reported(&router, held, "the held bucket").await;
}

/// The owner gate the delete carries: an `ExpectedBucketOwner` that is not this account's is
/// `AccessDenied` (HTTP 403) and resets nothing; the account's own id is served.
#[tokio::test]
async fn a_delete_for_an_unexpected_bucket_owner_is_refused() {
    let router = router();
    let bucket = "encryption-reset-owner-gate";
    create_bucket(&router, bucket, None, None).await;

    let stored = vec![Rule::by_default("aws:kms")
        .with_key(KEY_ID)
        .with_bucket_key(true)];
    let put = put_encryption(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        text(&put)
    );

    // Account ids that are not this emulator's: another twelve-digit id, this account's with
    // its first digit changed, and a value that is not an account id at all.
    let account = engine::emulator_account_id();
    let other_account = format!("9{}", &account[1..]);
    for expected in ["111122223333", other_account.as_str(), "not-an-account"] {
        let refused =
            delete_encryption(&router, bucket, &[("x-amz-expected-bucket-owner", expected)]).await;
        assert_eq!(
            refused.status, 403,
            "case {expected:?}: an ExpectedBucketOwner that is not the owner must be refused \
             with HTTP 403, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "AccessDenied",
            "case {expected:?}: expected the documented error code, got {:?}",
            refused.error_code()
        );

        let unchanged = get_encryption(&router, bucket).await;
        assert_eq!(
            reported_rules(&unchanged),
            as_reported(&stored),
            "case {expected:?}: a refused delete must leave the configuration untouched, got \
             {:?}",
            reported_rules(&unchanged)
        );
    }

    // The account that DOES own the bucket is served, so the rows above were refused on the
    // owner check rather than on the header being rejected outright.
    let reset =
        delete_encryption(&router, bucket, &[("x-amz-expected-bucket-owner", account)]).await;
    assert_reset(&reset, "this account's id");
    assert_default_reported(&router, bucket, "this account's id").await;
}
