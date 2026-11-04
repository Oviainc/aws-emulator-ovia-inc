// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's DEFAULT ENCRYPTION:
//! `PutBucketEncryption` stores the configuration and `GetBucketEncryption` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketEncryption` "configures default encryption and Amazon S3 Bucket Keys for an
//!   existing bucket" (the operation's own documentation): the
//!   `ServerSideEncryptionConfiguration` payload becomes the bucket's configuration IN
//!   FULL — a replacement, not a merge — and the call answers the modelled 200 with an
//!   empty body.
//! * `GetBucketEncryption` reports exactly those rules, in the order they were applied,
//!   with every member the request named and no member it did not.
//! * "By default, all buckets have a default encryption configuration that uses
//!   server-side encryption with Amazon S3 managed keys (SSE-S3)"
//!   (`GetBucketEncryption`'s documentation), "General purpose buckets - By default, S3
//!   Bucket Key is not enabled" (`ServerSideEncryptionRule$BucketKeyEnabled`), and a
//!   general purpose bucket blocks the customer-key encryption type
//!   (`…$BlockedEncryptionTypes`): a bucket no put has configured reports one `AES256` rule
//!   with the Bucket Key off and that type blocked.
//! * "This parameter is allowed if and only if `SSEAlgorithm` is set to `aws:kms` or
//!   `aws:kms:dsse`" (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID`): a rule naming a
//!   KMS key beside any other algorithm is refused with `InvalidArgument` (HTTP 400).
//! * An encryption call naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404), the shape both handlers widen in; one naming an
//!   `ExpectedBucketOwner` that is not the account's answers HTTP 403 `AccessDenied`.
//! * Every refused payload leaves the stored configuration exactly as it was.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    DEFAULT_BLOCKED_ENCRYPTION_TYPES, DEFAULT_BUCKET_KEY_ENABLED, DEFAULT_SSE_ALGORITHM,
    MAX_SERVER_SIDE_ENCRYPTION_RULES, SSE_C_ENCRYPTION_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
///
/// `pub(crate)` with the two value lists below it: the delete suite beside this one
/// ([`super::bucket_encryption_delete`]) spells its payloads and spans the published
/// catalogs through these rather than keeping a second copy that could drift from the
/// model's own order.
pub(crate) const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Every value the `ServerSideEncryption` enum publishes, in the model's own order
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` reports
/// `values: [AES256, aws:fsx, aws:backup, aws:kms, aws:kms:dsse]`).
pub(crate) const SERVER_SIDE_ENCRYPTION_VALUES: [&str; 5] =
    ["AES256", "aws:fsx", "aws:backup", "aws:kms", "aws:kms:dsse"];

/// Every value the `EncryptionType` enum publishes
/// (`smithy_query(query="shape",shape="EncryptionType")` reports `values: [NONE, SSE-C]`).
pub(crate) const ENCRYPTION_TYPE_VALUES: [&str; 2] = ["NONE", "SSE-C"];

/// One `ServerSideEncryptionRule`, as a request carries it and as a report brings it back.
///
/// The same type on both sides, so a round-trip assertion compares the rule that was sent
/// with the rule that was reported rather than with a hand-spelled expectation.
///
/// `pub(crate)`, with the three helpers below it: the default-encryption suite
/// ([`super::object_default_encryption`]) configures the buckets whose default an object
/// inherits through these rather than spelling the payload a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) sse_algorithm: Option<String>,
    pub(crate) kms_master_key_id: Option<String>,
    pub(crate) bucket_key_enabled: Option<bool>,
    pub(crate) blocked_encryption_types: Option<Vec<String>>,
}

impl Rule {
    /// A rule whose `ApplyServerSideEncryptionByDefault` names `algorithm`.
    pub(crate) fn by_default(algorithm: &str) -> Self {
        Self {
            sse_algorithm: Some(algorithm.to_string()),
            ..Self::default()
        }
    }

    /// A rule that names only `BucketKeyEnabled` — every member of
    /// `ServerSideEncryptionRule` is optional on its own.
    pub(crate) fn bucket_key_only(enabled: bool) -> Self {
        Self {
            bucket_key_enabled: Some(enabled),
            ..Self::default()
        }
    }

    pub(crate) fn with_key(mut self, key: &str) -> Self {
        self.kms_master_key_id = Some(key.to_string());
        self
    }

    pub(crate) fn with_bucket_key(mut self, enabled: bool) -> Self {
        self.bucket_key_enabled = Some(enabled);
        self
    }

    pub(crate) fn blocking(mut self, types: &[&str]) -> Self {
        self.blocked_encryption_types =
            Some(types.iter().map(|kind| kind.to_string()).collect());
        self
    }

    /// This rule as `GetBucketEncryption` REPORTS it.
    ///
    /// `BucketKeyEnabled` is a switch with a documented off state ("General purpose
    /// buckets - By default, S3 Bucket Key is not enabled",
    /// `ServerSideEncryptionRule$BucketKeyEnabled`), so a rule stored without it is
    /// reported with it OFF rather than with the member dropped. `BlockedEncryptionTypes`
    /// is a bucket-level SETTING with a default of its own, which a rule that says nothing
    /// about it does not turn off, so such a rule is reported with the default blocked set.
    /// Every round-trip assertion compares against this form, so what is pinned is the
    /// configuration a reader sees rather than the bytes a writer sent.
    pub(crate) fn as_reported(&self) -> Self {
        Self {
            bucket_key_enabled: Some(self.bucket_key_enabled.unwrap_or(false)),
            blocked_encryption_types: Some(
                self.blocked_encryption_types.clone().unwrap_or_else(|| {
                    DEFAULT_BLOCKED_ENCRYPTION_TYPES
                        .iter()
                        .map(|blocked| (*blocked).to_string())
                        .collect()
                }),
            ),
            ..self.clone()
        }
    }
}

/// Every rule of `rules` as `GetBucketEncryption` reports it ([`Rule::as_reported`]).
pub(crate) fn as_reported(rules: &[Rule]) -> Vec<Rule> {
    rules.iter().map(Rule::as_reported).collect()
}

/// The `ServerSideEncryptionConfiguration` payload for `rules`, spelled the way a client's
/// XML writer spells it: `ServerSideEncryptionConfiguration$Rules` is `xmlFlattened` with
/// `xmlName` `Rule`, and `BlockedEncryptionTypes$EncryptionType` likewise with `xmlName`
/// `EncryptionType` (`smithy_query(query="shape",shape="ServerSideEncryptionConfiguration"`
/// and `…shape="BlockedEncryptionTypes"`).
fn configuration_payload(rules: &[Rule]) -> String {
    let mut body = format!("<ServerSideEncryptionConfiguration xmlns=\"{XMLNS}\">");
    for rule in rules {
        body.push_str("<Rule>");
        if let Some(algorithm) = &rule.sse_algorithm {
            body.push_str("<ApplyServerSideEncryptionByDefault>");
            body.push_str(&format!("<SSEAlgorithm>{algorithm}</SSEAlgorithm>"));
            if let Some(key) = &rule.kms_master_key_id {
                body.push_str(&format!("<KMSMasterKeyID>{key}</KMSMasterKeyID>"));
            }
            body.push_str("</ApplyServerSideEncryptionByDefault>");
        }
        if let Some(enabled) = rule.bucket_key_enabled {
            body.push_str(&format!("<BucketKeyEnabled>{enabled}</BucketKeyEnabled>"));
        }
        if let Some(blocked) = &rule.blocked_encryption_types {
            body.push_str("<BlockedEncryptionTypes>");
            for kind in blocked {
                body.push_str(&format!("<EncryptionType>{kind}</EncryptionType>"));
            }
            body.push_str("</BlockedEncryptionTypes>");
        }
        body.push_str("</Rule>");
    }
    body.push_str("</ServerSideEncryptionConfiguration>");
    body
}

/// `PUT /<bucket>?encryption` — `PutBucketEncryption`.
pub(crate) async fn put_encryption(
    router: &MultiServiceRouter,
    bucket: &str,
    rules: &[Rule],
) -> Answer {
    put_encryption_payload(router, bucket, &configuration_payload(rules), &[]).await
}

/// `PutBucketEncryption` with the payload spelled out, for a container no [`Rule`] can
/// express, and with any extra bound headers.
async fn put_encryption_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?encryption"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?encryption` — `GetBucketEncryption`.
pub(crate) async fn get_encryption(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?encryption"), &[], "").await
}

/// The rules a `GetBucketEncryption` body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the ownership
/// controls suite beside this one reads a container, rather than by slicing the body.
pub(crate) fn reported_rules(answer: &Answer) -> Vec<Rule> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut rules: Vec<Rule> = Vec::new();
    let mut element = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if element == "Rule" {
                    rules.push(Rule::default());
                }
                if element == "BlockedEncryptionTypes" {
                    if let Some(rule) = rules.last_mut() {
                        rule.blocked_encryption_types = Some(Vec::new());
                    }
                }
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let Some(rule) = rules.last_mut() else {
                    continue;
                };
                match element.as_str() {
                    "SSEAlgorithm" => rule.sse_algorithm = Some(value),
                    "KMSMasterKeyID" => rule.kms_master_key_id = Some(value),
                    "BucketKeyEnabled" => rule.bucket_key_enabled = Some(value == "true"),
                    "EncryptionType" => rule
                        .blocked_encryption_types
                        .get_or_insert_with(Vec::new)
                        .push(value),
                    _ => {}
                }
            }
            Event::End(_) => element.clear(),
            Event::Eof => break,
            _ => {}
        }
    }
    rules
}

/// The one rule a bucket no `PutBucketEncryption` has configured reports: the SSE-S3
/// default with the Bucket Key off and the published encryption type blocked.
fn default_rule() -> Rule {
    Rule::by_default(DEFAULT_SSE_ALGORITHM)
        .with_bucket_key(DEFAULT_BUCKET_KEY_ENABLED)
        .blocking(&DEFAULT_BLOCKED_ENCRYPTION_TYPES)
}

#[test]
fn the_blocked_encryption_type_is_one_the_model_publishes() {
    // The default this emulator reports names an `EncryptionType` value, so it has to be
    // one of the values the enum publishes — re-derived from that list rather than trusted.
    assert!(
        ENCRYPTION_TYPE_VALUES.contains(&SSE_C_ENCRYPTION_TYPE),
        "the blocked encryption type must be one of {ENCRYPTION_TYPE_VALUES:?}, got \
         {SSE_C_ENCRYPTION_TYPE:?}"
    );
    assert_eq!(
        DEFAULT_BLOCKED_ENCRYPTION_TYPES.as_slice(),
        [SSE_C_ENCRYPTION_TYPE],
        "a new bucket blocks exactly the customer-key encryption type, got {:?}",
        DEFAULT_BLOCKED_ENCRYPTION_TYPES
    );
}

/// The table of configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH
/// `SSEAlgorithm` a rule names (every value the enum publishes, including the three the
/// issue's recorded call never sent), which FORM the KMS key takes (key id, key ARN and
/// alias — the three
/// `smithy_query(query="doc",shape="ServerSideEncryptionByDefault",member="KMSMasterKeyID")`
/// lists), whether the Bucket Key is on, off or unnamed, which `EncryptionType` values are
/// blocked, how MANY rules the container carries, and the ORDER they arrive in.
fn round_trip_rows() -> Vec<(&'static str, Vec<Rule>)> {
    // A KMS key in each of the three documented forms.
    let key_id = "1234abcd-12ab-34cd-56ef-1234567890ab";
    let key_arn = "arn:aws:kms:us-east-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";
    let key_alias = "alias/alias-name";

    let mut rows: Vec<(&'static str, Vec<Rule>)> = vec![
        (
            "SSE-KMS with a key id and the Bucket Key on",
            vec![Rule::by_default("aws:kms")
                .with_key(key_id)
                .with_bucket_key(true)],
        ),
        (
            "SSE-KMS with a fully qualified key ARN",
            vec![Rule::by_default("aws:kms").with_key(key_arn)],
        ),
        (
            "SSE-KMS with a key alias",
            vec![Rule::by_default("aws:kms").with_key(key_alias)],
        ),
        (
            "DSSE-KMS with a key id and the Bucket Key off",
            vec![Rule::by_default("aws:kms:dsse")
                .with_key(key_id)
                .with_bucket_key(false)],
        ),
        (
            "a rule naming only BucketKeyEnabled",
            vec![Rule::bucket_key_only(true)],
        ),
        (
            "SSE-S3 blocking SSE-C",
            vec![Rule::by_default("AES256").blocking(&["SSE-C"])],
        ),
        (
            "every published EncryptionType blocked, in the model's order",
            vec![Rule::by_default("AES256").blocking(&ENCRYPTION_TYPE_VALUES)],
        ),
        (
            "the published EncryptionType values in the other order",
            vec![Rule::by_default("aws:kms")
                .with_key(key_id)
                .blocking(&["SSE-C", "NONE"])],
        ),
        (
            "two rules, reported in the order they arrived",
            vec![
                Rule::by_default("aws:kms:dsse").with_key(key_arn),
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
    // One row per published algorithm on its own — the single-rule shape every real client
    // sends, over the whole catalog rather than the one value the issue recorded.
    for algorithm in SERVER_SIDE_ENCRYPTION_VALUES {
        rows.push((
            "a single rule naming one published algorithm",
            vec![Rule::by_default(algorithm)],
        ));
    }
    rows
}

#[tokio::test]
async fn putting_an_encryption_configuration_stores_it_and_reads_back_through_get() {
    let router = router();

    for (index, (case, rules)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("encryption-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_encryption(&router, &bucket, &rules).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutBucketEncryption, got {} with \
             body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutBucketEncryption's output is Unit, so the body must be empty, \
             got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_encryption(&router, &bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketEncryption on the bucket just \
             configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported_rules(&read),
            as_reported(&rules),
            "case {case:?}: expected the stored configuration back unchanged, got {:?}",
            reported_rules(&read)
        );

        // Putting the same configuration again reports the same thing: the write is a
        // replacement, so it neither duplicates rules nor merges them.
        let again = put_encryption(&router, &bucket, &rules).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: the repeated put must succeed, got {}",
            again.status
        );
        let reread = get_encryption(&router, &bucket).await;
        assert_eq!(
            reported_rules(&reread),
            as_reported(&rules),
            "case {case:?}: a repeated put must not change what is stored, got {:?}",
            reported_rules(&reread)
        );

        // And a different configuration REPLACES: what comes back is the new container
        // alone, with nothing merged in from the rules above.
        let replacement = vec![Rule::by_default("AES256").with_bucket_key(true)];
        let second = put_encryption(&router, &bucket, &replacement).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        let replaced = get_encryption(&router, &bucket).await;
        assert_eq!(
            reported_rules(&replaced),
            as_reported(&replacement),
            "case {case:?}: the configuration must be replaced, not merged, got {:?}",
            reported_rules(&replaced)
        );
    }
}

#[tokio::test]
async fn a_bucket_no_put_has_configured_reports_the_sse_s3_default() {
    let router = router();

    // Buckets that have never been configured, one of them in a non-default Region, so the
    // answer is about the default configuration rather than about one bucket.
    for (case, bucket, constraint) in [
        (
            "a bucket created with no location constraint",
            "encryption-default-global",
            None,
        ),
        (
            "a bucket created in another Region",
            "encryption-default-regional",
            Some("eu-west-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        let read = get_encryption(&router, bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a bucket with no configuration of its own still reports the \
             default encryption, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported_rules(&read),
            vec![default_rule()],
            "case {case:?}: expected the documented SSE-S3 default, got {:?}",
            reported_rules(&read)
        );

        // Reading twice changes nothing: the read reports, it does not configure — so the
        // second read answers exactly what the first one did.
        let again = get_encryption(&router, bucket).await;
        assert_eq!(
            reported_rules(&again),
            reported_rules(&read),
            "case {case:?}: a second read must report the same configuration, got {:?}",
            reported_rules(&again)
        );

        // And the bucket still takes a put afterwards, so the reads above reported the
        // default rather than having configured anything.
        let configured = vec![Rule::by_default("aws:kms").with_key("alias/aws/s3")];
        let put = put_encryption(&router, bucket, &configured).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept an encryption configuration, got {} with \
             body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_encryption(&router, bucket).await;
        assert_eq!(
            reported_rules(&read),
            as_reported(&configured),
            "case {case:?}: the configured bucket must report what it was given, got {:?}",
            reported_rules(&read)
        );
    }
}

#[tokio::test]
async fn a_refused_encryption_payload_changes_nothing() {
    let router = router();
    let bucket = "encryption-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The configuration the bucket holds throughout: every refusal below must leave it
    // exactly as it is.
    let stored = vec![
        Rule::by_default("aws:kms")
            .with_key("1234abcd-12ab-34cd-56ef-1234567890ab")
            .with_bucket_key(true),
        Rule::by_default("AES256").blocking(&["SSE-C"]),
    ];
    let put = put_encryption(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // (case, the refused rule list, the error code when one is required). A KMS key named
    // beside a non-KMS algorithm is the documented `InvalidArgument`: "This parameter is
    // allowed if and only if SSEAlgorithm is set to aws:kms or aws:kms:dsse". The rows that
    // simply fall outside the published schema are refused with HTTP 400 whichever of the
    // boundary's and the handler's schema refusals catches them first.
    let past_the_ceiling: Vec<Rule> = (0..=MAX_SERVER_SIDE_ENCRYPTION_RULES)
        .map(|_| Rule::by_default("AES256"))
        .collect();
    let rows: Vec<(&str, Vec<Rule>, Option<&str>)> = vec![
        (
            "a KMS key beside SSE-S3",
            vec![Rule::by_default("AES256").with_key("alias/alias-name")],
            Some("InvalidArgument"),
        ),
        (
            "a KMS key beside aws:fsx",
            vec![Rule::by_default("aws:fsx").with_key("alias/alias-name")],
            Some("InvalidArgument"),
        ),
        (
            "a KMS key beside aws:backup",
            vec![Rule::by_default("aws:backup")
                .with_key("arn:aws:kms:us-east-2:111122223333:key/1234abcd")],
            Some("InvalidArgument"),
        ),
        (
            "one rule with a misplaced key among rules that are fine",
            vec![
                Rule::by_default("aws:kms").with_key("alias/alias-name"),
                Rule::by_default("AES256").with_key("alias/alias-name"),
            ],
            Some("InvalidArgument"),
        ),
        (
            "an algorithm outside the published set",
            vec![Rule::by_default("aws:kms:sse")],
            None,
        ),
        (
            "a published algorithm differing only in case",
            vec![Rule::by_default("aes256")],
            None,
        ),
        ("an empty algorithm", vec![Rule::by_default("")], None),
        (
            "an EncryptionType outside the published set",
            vec![Rule::by_default("AES256").blocking(&["SSE-KMS"])],
            None,
        ),
        ("one rule past the recorded ceiling", past_the_ceiling, None),
    ];

    for (case, rules, code) in rows {
        let refused = put_encryption(&router, bucket, &rules).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the payload to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected the documented error code, got {:?}",
                refused.error_code()
            );
        }

        let read = get_encryption(&router, bucket).await;
        assert_eq!(
            reported_rules(&read),
            as_reported(&stored),
            "case {case:?}: a refused put must leave the stored configuration untouched, \
             got {:?}",
            reported_rules(&read)
        );
    }

    // Payloads no `Rule` can express: a container naming no rule, a rule naming nothing at
    // all, and a KMS key with no algorithm beside it (`SSEAlgorithm` is REQUIRED inside
    // `ApplyServerSideEncryptionByDefault`). Each is outside the published schema, so each
    // is refused with HTTP 400 and leaves the configuration alone.
    let raw_rows: [(&str, &str); 4] = [
        (
            "a container naming no Rule",
            "<ServerSideEncryptionConfiguration/>",
        ),
        (
            "a Rule naming none of its members",
            "<ServerSideEncryptionConfiguration><Rule></Rule></ServerSideEncryptionConfiguration>",
        ),
        (
            "a KMS key with no SSEAlgorithm beside it",
            "<ServerSideEncryptionConfiguration><Rule>\
             <ApplyServerSideEncryptionByDefault>\
             <KMSMasterKeyID>alias/alias-name</KMSMasterKeyID>\
             </ApplyServerSideEncryptionByDefault></Rule></ServerSideEncryptionConfiguration>",
        ),
        (
            "a body that is not the modelled container at all",
            "<OwnershipControls><Rule><ObjectOwnership>ObjectWriter</ObjectOwnership></Rule>\
             </OwnershipControls>",
        ),
    ];
    for (case, payload) in raw_rows {
        let refused = put_encryption_payload(&router, bucket, payload, &[]).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the payload to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        let read = get_encryption(&router, bucket).await;
        assert_eq!(
            reported_rules(&read),
            as_reported(&stored),
            "case {case:?}: a refused put must leave the stored configuration untouched, \
             got {:?}",
            reported_rules(&read)
        );
    }
}

#[tokio::test]
async fn encryption_on_a_bucket_this_emulator_does_not_hold_is_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-encryption-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it, an
    // extension of it, and one differing only in case (bucket names are case-sensitive).
    // `NoSuchBucket` carries `httpError 404` in the model.
    for bucket in [
        "absent-encryption-bucket",
        "held",
        "held-encryption-bucket-2",
        "Held-Encryption-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutBucketEncryption",
                put_encryption(&router, bucket, &[Rule::by_default("AES256")]).await,
            ),
            ("GetBucketEncryption", get_encryption(&router, bucket).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this emulator \
                 does not hold, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {bucket:?}/{operation}: expected the modelled error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // Nothing above created a bucket as a side effect of configuring one that is absent.
    for bucket in ["absent-encryption-bucket", "held"] {
        let read = get_encryption(&router, bucket).await;
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: a refused put must not have created the bucket, got {:?}",
            read.error_code()
        );
    }

    // And the bucket that IS held configures normally, so the rows above failed on the
    // lookup rather than on the operations being unreachable.
    let configured = vec![Rule::by_default("aws:kms:dsse").with_key("alias/alias-name")];
    let put = put_encryption(&router, "held-encryption-bucket", &configured).await;
    assert_eq!(
        put.status, 200,
        "the held bucket must accept an encryption configuration, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = get_encryption(&router, "held-encryption-bucket").await;
    assert_eq!(
        reported_rules(&read),
        as_reported(&configured),
        "the held bucket must report the configuration it was given, got {:?}",
        reported_rules(&read)
    );
}

#[tokio::test]
async fn an_expected_bucket_owner_that_is_not_the_account_is_access_denied() {
    let router = router();
    let bucket = "encryption-expected-owner";
    create_bucket(&router, bucket, None, None).await;

    let stored = vec![Rule::by_default("AES256").with_bucket_key(true)];
    let put = put_encryption(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // Account ids that are not this emulator's: another twelve-digit id, the account id
    // with a digit changed, and a value that is not an account id at all.
    let account = engine::emulator_account_id();
    let other_account = format!("9{}", &account[1..]);
    for expected in ["111122223333", other_account.as_str(), "not-an-account"] {
        let refused = put_encryption_payload(
            &router,
            bucket,
            &configuration_payload(&[Rule::by_default("aws:kms").with_key("alias/alias-name")]),
            &[("x-amz-expected-bucket-owner", expected)],
        )
        .await;
        assert_eq!(
            refused.status, 403,
            "case {expected:?}: an ExpectedBucketOwner that is not the owner must be refused \
             with HTTP 403, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "AccessDenied",
            "case {expected:?}: expected the documented error code, got {:?}",
            refused.error_code()
        );

        let read = send(
            &router,
            "GET",
            &format!("/{bucket}?encryption"),
            &[("x-amz-expected-bucket-owner", expected)],
            "",
        )
        .await;
        assert_eq!(
            read.status, 403,
            "case {expected:?}: the read must be refused the same way, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );

        let unchanged = get_encryption(&router, bucket).await;
        assert_eq!(
            reported_rules(&unchanged),
            as_reported(&stored),
            "case {expected:?}: a refused put must leave the stored configuration \
             untouched, got {:?}",
            reported_rules(&unchanged)
        );
    }

    // The account that DOES own the bucket is served, so the rows above were refused on
    // the owner check rather than on the header being rejected outright.
    let owned = vec![Rule::by_default("aws:kms:dsse").with_key("alias/alias-name")];
    let put = put_encryption_payload(
        &router,
        bucket,
        &configuration_payload(&owned),
        &[("x-amz-expected-bucket-owner", account)],
    )
    .await;
    assert_eq!(
        put.status, 200,
        "the owning account's put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = send(
        &router,
        "GET",
        &format!("/{bucket}?encryption"),
        &[("x-amz-expected-bucket-owner", account)],
        "",
    )
    .await;
    assert_eq!(
        reported_rules(&read),
        as_reported(&owned),
        "the owning account must read the configuration back, got {:?}",
        reported_rules(&read)
    );
}
