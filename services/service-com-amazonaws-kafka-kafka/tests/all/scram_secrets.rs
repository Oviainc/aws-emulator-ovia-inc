// SPDX-License-Identifier: CC0-1.0

//! `BatchAssociateScramSecret`, `ListScramSecrets` and
//! `BatchDisassociateScramSecret`, driven end to end over the wire through the
//! same `MultiServiceRouter` the emulator serves from (the shared
//! `kafka_harness`).
//!
//! The rules here are about the ONE set of AWS Secrets Manager secrets a cluster
//! authenticates its SASL/SCRAM clients with: an association stores exactly the
//! secrets it named and the listing reports that set, once per secret, in one
//! total order; a disassociation removes exactly the secrets it names; and a
//! request this endpoint cannot act on — a cluster ARN it cannot parse or
//! resolve, an entry that is not a secret ARN, a cluster that does not
//! authenticate SASL/SCRAM clients at all — is refused and stores nothing.
//!
//! Each suite asserts its rule over a table rather than one recorded request:
//! the rows differ along how many secrets a batch carries, their ARN partition,
//! region, account and name form, the order they are submitted in, whether the
//! batch repeats one, and along what makes a cluster hold no secrets at all.

use serde_json::{json, Value};

// The service's own refusal wording and page bound, so the suite and the
// handlers cannot drift apart on what the service says.
use service_com_amazonaws_kafka_kafka::state::{
    MAX_RESULTS_LIMIT, MAX_SCRAM_SECRETS_PER_CLUSTER, RESOURCE_NOT_FOUND_MESSAGE,
    SASL_SCRAM_NOT_ENABLED_MESSAGE,
};

// The router harness, the signing region, the account id and the URI encoder
// every suite of this crate shares, so this one adds only its own rule.
use super::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The resource URI of one cluster's SCRAM secrets: the model binds all three
/// operations to `/v1/clusters/{ClusterArn}/scram-secrets`, with the ARN
/// percent-encoded into the single `@httpLabel` path segment.
fn secrets_uri(arn: &str) -> String {
    format!("/v1/clusters/{}/scram-secrets", pct(arn))
}

/// One Secrets Manager secret ARN in this suite's own region and account, in the
/// form `SecretArnList` carries ("List of AWS Secrets Manager secret ARNs"):
/// `arn:<partition>:secretsmanager:<region>:<account>:secret:<name>`.
fn secret_arn(name: &str) -> String {
    format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}")
}

/// `BatchAssociateScramSecret` (`POST`), as an endpoint for `region` serves it.
async fn associate_in(
    emulator: &Emulator,
    region: &str,
    cluster: &str,
    secrets: &[String],
) -> Reply {
    emulator
        .call_in(
            region,
            "POST",
            &secrets_uri(cluster),
            Some(json!({ "secretArnList": secrets })),
        )
        .await
}

async fn associate(emulator: &Emulator, cluster: &str, secrets: &[String]) -> Reply {
    associate_in(emulator, REGION, cluster, secrets).await
}

/// `BatchDisassociateScramSecret`, which the model binds to the same URI with
/// `PATCH`.
async fn disassociate_in(
    emulator: &Emulator,
    region: &str,
    cluster: &str,
    secrets: &[String],
) -> Reply {
    emulator
        .call_in(
            region,
            "PATCH",
            &secrets_uri(cluster),
            Some(json!({ "secretArnList": secrets })),
        )
        .await
}

async fn disassociate(emulator: &Emulator, cluster: &str, secrets: &[String]) -> Reply {
    disassociate_in(emulator, REGION, cluster, secrets).await
}

/// `ListScramSecrets` (`GET`), with `query` the whole query string including its
/// `?`, or empty.
async fn list_in(emulator: &Emulator, region: &str, cluster: &str, query: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("{}{query}", secrets_uri(cluster)),
            None,
        )
        .await
}

async fn list(emulator: &Emulator, cluster: &str, query: &str) -> Reply {
    list_in(emulator, REGION, cluster, query).await
}

/// The `SecretArnList` a successful listing reports.
fn listed(reply: &Reply, case: &str) -> Vec<String> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListScramSecrets to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/secretArnList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected ListScramSecrets to report secretArnList, got {}",
                reply.body
            )
        })
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a listed secret is not a string: {entry}"))
                .to_string()
        })
        .collect()
}

/// Every secret the listing reports for `cluster`, following its pagination
/// tokens, with the page size `query` asks for.
async fn list_every_page(
    emulator: &Emulator,
    cluster: &str,
    page_size: Option<i32>,
    case: &str,
) -> Vec<String> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    // One page per stored secret is already more than the listing can need, so a
    // listing that never stopped handing out tokens fails here instead of
    // looping forever.
    for _ in 0..64 {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(next) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={}", pct(next)));
        }
        let reply = list(emulator, cluster, &query).await;
        let page = listed(&reply, case);
        if let Some(size) = page_size {
            assert!(
                page.len() <= usize::try_from(size).unwrap_or_default(),
                "case {case}: expected at most {size} secrets per page, got {}",
                page.len()
            );
        }
        collected.extend(page);
        token = reply
            .at("/nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case}: ListScramSecrets kept handing out pagination tokens")
}

/// Assert `reply` is the refusal the service answers a request whose parameter
/// it cannot parse: HTTP 400 `BadRequestException`.
fn assert_bad_request(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
}

/// Assert `reply` is the refusal the service answers a request naming a cluster
/// it does not hold: HTTP 404 `NotFoundException` carrying the service's one
/// fixed not-found sentence.
fn assert_not_found(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (404, "NotFoundException"),
        "case {case}: expected 404 NotFoundException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        RESOURCE_NOT_FOUND_MESSAGE,
        "case {case}: expected the service's not-found sentence, got {:?}",
        reply.error_message()
    );
}

/// One row of the association table: the secrets one batch carries, and the set
/// the cluster must then hold.
#[derive(Debug, Clone)]
struct Batch {
    case: &'static str,
    submitted: Vec<String>,
}

impl Batch {
    /// The secrets the cluster holds after this batch: the submitted ARNs as a
    /// set, in the one total order the listing pages through.
    fn expected(&self) -> Vec<String> {
        let mut expected: Vec<String> = self.submitted.clone();
        expected.sort();
        expected.dedup();
        expected
    }
}

/// The batches every suite below walks. The rows differ along what the rules
/// generalise over: how many secrets one call carries, the ARN's partition,
/// region and account, the secret name's form, the order the batch submits them
/// in, and whether it repeats one.
fn batches() -> Vec<Batch> {
    vec![
        Batch {
            case: "one-secret",
            submitted: vec![secret_arn("AmazonMSK_alpha-AbCdEf")],
        },
        Batch {
            case: "three-secrets-submitted-in-reverse-order",
            submitted: vec![
                secret_arn("AmazonMSK_zulu-003zzz"),
                secret_arn("AmazonMSK_mike-002mmm"),
                secret_arn("AmazonMSK_alpha-001aaa"),
            ],
        },
        Batch {
            case: "one-secret-repeated",
            submitted: vec![
                secret_arn("AmazonMSK_repeat-9x8y7z"),
                secret_arn("AmazonMSK_repeat-9x8y7z"),
                secret_arn("AmazonMSK_repeat-9x8y7z"),
            ],
        },
        Batch {
            case: "other-partitions-regions-and-accounts",
            submitted: vec![
                "arn:aws-us-gov:secretsmanager:us-gov-west-1:111122223333:secret:AmazonMSK_gov-1a2b3c"
                    .to_string(),
                "arn:aws-cn:secretsmanager:cn-north-1:444455556666:secret:AmazonMSK_cn-4d5e6f"
                    .to_string(),
            ],
        },
        Batch {
            // A Secrets Manager name may carry '/' and ':', which the ARN's last
            // field keeps whole.
            case: "names-carrying-a-path-and-a-colon",
            submitted: vec![
                secret_arn("AmazonMSK_team/prod/db-7g8h9i"),
                secret_arn("AmazonMSK_team:prod:db-1j2k3l"),
            ],
        },
    ]
}

/// Values that are not an AWS Secrets Manager secret ARN at all, so no batch
/// carrying one may store anything.
///
/// The requirement is the form `SecretArnList`'s documentation names —
/// `arn:<partition>:secretsmanager:<region>:<account>:secret:<name>` — and the
/// rows differ along WHICH part of it the value breaks: not an ARN at all, too
/// few fields, an empty partition, region or account, an empty resource, a
/// secret ARN naming no secret, another service's namespace, a namespace
/// differing only in case, and another resource type.
fn values_that_are_not_secret_arns() -> Vec<&'static str> {
    vec![
        "",
        "AmazonMSK_not-an-arn",
        "arn:aws:secretsmanager",
        "arn:aws:secretsmanager:us-west-2:000000000000:",
        "arn:aws:secretsmanager:us-west-2:000000000000:secret",
        "arn:aws:secretsmanager:us-west-2:000000000000:secret:",
        "arn::secretsmanager:us-west-2:000000000000:secret:AmazonMSK_no-partition",
        "arn:aws:secretsmanager::000000000000:secret:AmazonMSK_no-region",
        "arn:aws:secretsmanager:us-west-2::secret:AmazonMSK_no-account",
        "arn:aws:SecretsManager:us-west-2:000000000000:secret:AmazonMSK_wrong-case",
        "arn:aws:secretsmanager:us-west-2:000000000000:parameter:AmazonMSK_other-type",
        "arn:aws:kafka:us-west-2:000000000000:cluster/other/00000000-0000-0000-0000-000000000000-1",
        "arn:aws:ssm:us-west-2:000000000000:secret:AmazonMSK_other-service",
    ]
}

/// A cluster that authenticates SASL/SCRAM clients, SETTLED past the emulator's
/// `CREATING` -> `ACTIVE` span, so it may hold secrets: a cluster's secret set is
/// only served from a state that has the brokers holding it.
async fn scram_cluster(emulator: &Emulator, name: &str) -> String {
    let arn = emulator
        .create_ok(&ClusterRequest {
            sasl_scram: Some(true),
            ..ClusterRequest::named(name)
        })
        .await;
    sleep_past_activation();
    arn
}

/// RULE: an association stores exactly the secrets it named, and the listing
/// reports that stored set — once per secret, in one total order — whatever
/// order or multiplicity the batch submitted them in. Re-associating a stored
/// secret therefore changes nothing.
#[tokio::test]
async fn an_association_is_what_the_listing_reports() {
    let emulator = Emulator::new();
    for batch in batches() {
        let case = batch.case;
        let cluster = scram_cluster(&emulator, &format!("scram-assoc-{case}")).await;

        let empty = list(&emulator, &cluster, "").await;
        assert!(
            listed(&empty, case).is_empty(),
            "case {case}: expected a new cluster to hold no secrets, got {:?}",
            listed(&empty, case)
        );

        let reply = associate(&emulator, &cluster, &batch.submitted).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected BatchAssociateScramSecret to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/clusterArn").as_str(),
            Some(cluster.as_str()),
            "case {case}: expected the association to name the cluster it stored into, got {}",
            reply.body
        );
        assert_eq!(
            reply.at("/unprocessedScramSecrets"),
            &json!([]),
            "case {case}: expected no unprocessed secrets, got {}",
            reply.at("/unprocessedScramSecrets")
        );

        let reported = listed(&list(&emulator, &cluster, "").await, case);
        assert_eq!(
            reported,
            batch.expected(),
            "case {case}: expected the listing to report the stored set, got {reported:?}"
        );

        // Idempotent: the set already holds these secrets, so associating them
        // again leaves every listing of it unchanged.
        let again = associate(&emulator, &cluster, &batch.submitted).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected a repeated association to return 200, got {} {}",
            again.status, again.body
        );
        let after = listed(&list(&emulator, &cluster, "").await, case);
        assert_eq!(
            after,
            batch.expected(),
            "case {case}: expected a repeated association to change nothing, got {after:?}"
        );
    }
}

/// RULE: a disassociation removes exactly the secrets it names from the stored
/// set and leaves the rest of it alone, so removing a secret the cluster does
/// not hold removes nothing.
#[tokio::test]
async fn a_disassociation_removes_exactly_the_secrets_it_names() {
    let emulator = Emulator::new();
    let never_associated = secret_arn("AmazonMSK_never-associated-0z9y8x");
    for batch in batches() {
        let case = batch.case;
        let cluster = scram_cluster(&emulator, &format!("scram-disassoc-{case}")).await;
        assert_eq!(
            associate(&emulator, &cluster, &batch.submitted).await.status,
            200,
            "case {case}: the association under test must succeed first"
        );
        let stored = batch.expected();

        // A secret the cluster never held, and nothing else, is removed from
        // nothing: the stored set is untouched.
        let miss = disassociate(&emulator, &cluster, std::slice::from_ref(&never_associated)).await;
        assert_eq!(
            miss.status, 200,
            "case {case}: expected removing an unheld secret to return 200, got {} {}",
            miss.status, miss.body
        );
        assert_eq!(
            miss.at("/clusterArn").as_str(),
            Some(cluster.as_str()),
            "case {case}: expected the disassociation to name the cluster, got {}",
            miss.body
        );
        let untouched = listed(&list(&emulator, &cluster, "").await, case);
        assert_eq!(
            untouched, stored,
            "case {case}: expected removing an unheld secret to change nothing, got {untouched:?}"
        );

        // The first stored secret goes, twice over: the second call removes
        // nothing, because the set no longer holds it.
        let first = stored.first().expect("every batch stores one secret").clone();
        let remaining: Vec<String> = stored.iter().skip(1).cloned().collect();
        for round in 0..2 {
            let reply = disassociate(&emulator, &cluster, std::slice::from_ref(&first)).await;
            assert_eq!(
                reply.status, 200,
                "case {case} round {round}: expected BatchDisassociateScramSecret to return 200, \
                 got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.at("/unprocessedScramSecrets"),
                &json!([]),
                "case {case} round {round}: expected no unprocessed secrets, got {}",
                reply.at("/unprocessedScramSecrets")
            );
            let after = listed(&list(&emulator, &cluster, "").await, case);
            assert_eq!(
                after, remaining,
                "case {case} round {round}: expected only the named secret to be removed, got \
                 {after:?}"
            );
        }

        // The whole stored set goes, so the listing reports an empty page for a
        // cluster that holds nothing — not a refusal.
        assert_eq!(
            disassociate(&emulator, &cluster, &stored).await.status,
            200,
            "case {case}: expected removing every stored secret to return 200"
        );
        let emptied = listed(&list(&emulator, &cluster, "").await, case);
        assert!(
            emptied.is_empty(),
            "case {case}: expected the cluster to hold no secrets, got {emptied:?}"
        );
    }
}

/// RULE: a batch carrying a value that is not an AWS Secrets Manager secret ARN
/// is refused as a bad request, and stores none of its secrets — neither the
/// association nor the disassociation applies part of a batch.
#[tokio::test]
async fn a_value_that_is_not_a_secret_arn_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let cluster = scram_cluster(&emulator, "scram-bad-secret-arn").await;
    let good = secret_arn("AmazonMSK_good-5m6n7o");
    assert_eq!(
        associate(&emulator, &cluster, std::slice::from_ref(&good))
            .await
            .status,
        200,
        "the association that seeds the stored set must succeed first"
    );

    for value in values_that_are_not_secret_arns() {
        let case = format!("secret-arn={value:?}");
        // Alone, and behind a usable ARN: one unusable entry refuses the whole
        // batch either way.
        for batch in [
            vec![value.to_string()],
            vec![secret_arn("AmazonMSK_also-good-8p9q0r"), value.to_string()],
        ] {
            let associated = associate(&emulator, &cluster, &batch).await;
            assert_bad_request(&associated, &case);
            assert_eq!(
                associated.at("/invalidParameter").as_str(),
                Some("SecretArnList"),
                "case {case}: expected the refusal to name SecretArnList, got {}",
                associated.body
            );

            let disassociated = disassociate(&emulator, &cluster, &batch).await;
            assert_bad_request(&disassociated, &case);

            let stored = listed(&list(&emulator, &cluster, "").await, &case);
            assert_eq!(
                stored,
                vec![good.clone()],
                "case {case}: expected a refused batch to leave the stored set alone, got \
                 {stored:?}"
            );
        }
    }
}

/// RULE: a cluster holds SCRAM secrets only where its client authentication
/// turns SASL/SCRAM on, so an association against any other cluster is refused
/// and stores nothing.
#[tokio::test]
async fn a_cluster_that_does_not_authenticate_sasl_scram_holds_no_secrets() {
    let emulator = Emulator::new();
    let secrets = vec![secret_arn("AmazonMSK_rejected-1s2t3u")];

    // The rows differ along WHY the cluster authenticates no SASL/SCRAM client:
    // the create named no client authentication at all, it turned SCRAM off, it
    // turned a different SASL mechanism on, or the cluster is serverless (which
    // authenticates with IAM and has no SCRAM surface).
    let mut clusters = Vec::new();
    for (case, request) in [
        ("no-client-authentication", ClusterRequest::named("scram-off-none")),
        (
            "scram-disabled",
            ClusterRequest {
                sasl_scram: Some(false),
                ..ClusterRequest::named("scram-off-false")
            },
        ),
        (
            "iam-only",
            ClusterRequest {
                sasl_iam: Some(true),
                ..ClusterRequest::named("scram-off-iam")
            },
        ),
    ] {
        clusters.push((case, emulator.create_ok(&request).await));
    }
    clusters.push((
        "serverless",
        emulator
            .create_serverless_ok("scram-off-serverless", &[vec!["subnet-a", "subnet-b"]], Some(true))
            .await,
    ));
    // The rule under test is about the client authentication, so every row is
    // settled first: a cluster still being provisioned would be refused on its
    // STATE instead, which is the subject of its own suite below.
    sleep_past_activation();

    for (case, cluster) in clusters {
        let reply = associate(&emulator, &cluster, &secrets).await;
        assert_bad_request(&reply, case);
        assert_eq!(
            reply.error_message(),
            SASL_SCRAM_NOT_ENABLED_MESSAGE,
            "case {case}: expected the SASL/SCRAM refusal sentence, got {:?}",
            reply.error_message()
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("ClusterArn"),
            "case {case}: expected the refusal to name ClusterArn, got {}",
            reply.body
        );
        let stored = listed(&list(&emulator, &cluster, "").await, case);
        assert!(
            stored.is_empty(),
            "case {case}: expected a refused association to store nothing, got {stored:?}"
        );
    }
}

/// RULE: a `ClusterArn` the service cannot parse is a bad request, and a
/// well-formed cluster ARN this endpoint's region holds no cluster for is a
/// not-found — on all three operations, so the writers and the reader draw the
/// same line.
#[tokio::test]
async fn a_cluster_arn_this_endpoint_cannot_resolve_is_refused() {
    let emulator = Emulator::new();
    let secrets = vec![secret_arn("AmazonMSK_unresolved-4v5w6x")];

    // Values that are not an MSK cluster ARN at all: not an ARN, another
    // service's namespace, another MSK resource type, a cluster ARN missing its
    // id segment.
    for value in [
        "not-an-arn",
        "arn:aws:secretsmanager:us-west-2:000000000000:secret:AmazonMSK_not-a-cluster",
        &format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/cfg/00000000-0000-0000-0000-000000000000-1"),
        &format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/no-id-segment"),
    ] {
        let case = format!("cluster-arn={value:?}");
        assert_bad_request(&associate(&emulator, value, &secrets).await, &case);
        assert_bad_request(&disassociate(&emulator, value, &secrets).await, &case);
        assert_bad_request(&list(&emulator, value, "").await, &case);
    }

    // A well-formed cluster ARN that was never minted, and a cluster that lives
    // in another region: a regional endpoint serves only its own region's
    // clusters, so both are the same miss.
    let elsewhere = emulator
        .create_ok_in(
            "eu-central-1",
            &ClusterRequest {
                sasl_scram: Some(true),
                ..ClusterRequest::named("scram-other-region")
            },
        )
        .await;
    sleep_past_activation();
    for (case, arn) in [
        (
            "never-minted",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/never-minted/00000000-0000-0000-0000-000000000000-1"
            ),
        ),
        ("another-region", elsewhere.clone()),
    ] {
        assert_not_found(&associate(&emulator, &arn, &secrets).await, case);
        assert_not_found(&disassociate(&emulator, &arn, &secrets).await, case);
        assert_not_found(&list(&emulator, &arn, "").await, case);
    }

    // The same cluster, through the endpoint that does hold it, serves all
    // three — so the misses above are about the region, not about the request.
    assert_eq!(
        associate_in(&emulator, "eu-central-1", &elsewhere, &secrets)
            .await
            .status,
        200,
        "expected the owning region's endpoint to serve the association"
    );
    let stored = listed(
        &list_in(&emulator, "eu-central-1", &elsewhere, "").await,
        "another-region",
    );
    assert_eq!(
        stored, secrets,
        "expected the owning region's endpoint to report the stored secret, got {stored:?}"
    );
}

/// RULE: the listing pages through the stored set in one total order: every page
/// is at most `MaxResults` long, the pages concatenate to the whole set with no
/// repeat and no gap, and the last page carries no token. A token is minted for
/// one cluster's listing, so another cluster's token is not a resume point.
#[tokio::test]
async fn the_listing_pages_through_the_stored_set() {
    let emulator = Emulator::new();
    let cluster = scram_cluster(&emulator, "scram-pagination").await;
    let other = scram_cluster(&emulator, "scram-pagination-other").await;

    let secrets: Vec<String> = (0..7)
        .map(|index| secret_arn(&format!("AmazonMSK_page-{index}-0a1b2c")))
        .collect();
    assert_eq!(
        associate(&emulator, &cluster, &secrets).await.status,
        200,
        "the association that seeds the pagination table must succeed first"
    );
    assert_eq!(
        associate(&emulator, &other, &[secret_arn("AmazonMSK_other-3d4e5f")])
            .await
            .status,
        200,
        "the other cluster's association must succeed first"
    );
    let mut expected = secrets.clone();
    expected.sort();

    // The rows differ along the page size: smaller than the set, a divisor of
    // it, exactly its size, larger than it, and unnamed (the default page).
    for page_size in [Some(1), Some(2), Some(3), Some(7), Some(8), None] {
        let case = format!("maxResults={page_size:?}");
        let collected = list_every_page(&emulator, &cluster, page_size, &case).await;
        assert_eq!(
            collected, expected,
            "case {case}: expected the pages to concatenate to the stored set, got {collected:?}"
        );
    }

    // A token this listing minted for one cluster resumes only that cluster's
    // listing; anything else is not a resume point this operation issued.
    let first_page = list(&emulator, &cluster, "?maxResults=2").await;
    let token = first_page
        .at("/nextToken")
        .as_str()
        .expect("a page shorter than the stored set carries a token")
        .to_string();
    for (case, value) in [
        ("another-clusters-token", token.clone()),
        ("not-a-token", "zzz-not-a-token".to_string()),
    ] {
        let reply = list(&emulator, &other, &format!("?nextToken={}", pct(&value))).await;
        assert_bad_request(&reply, case);
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("NextToken"),
            "case {case}: expected the refusal to name NextToken, got {}",
            reply.body
        );
    }

    // The cluster's own token resumes its own listing where the first page
    // stopped.
    let resumed = list(&emulator, &cluster, &format!("?nextToken={}", pct(&token))).await;
    let rest = listed(&resumed, "resumed");
    assert_eq!(
        rest,
        expected[2..].to_vec(),
        "expected the token to resume after the first page, got {rest:?}"
    );
}

/// RULE: a cluster's secrets are a sub-resource of that cluster, so they are the
/// cluster's own and nothing else's: associating with one cluster leaves every
/// other cluster's set alone.
#[tokio::test]
async fn one_clusters_secrets_are_not_anothers() {
    let emulator = Emulator::new();
    let left = scram_cluster(&emulator, "scram-isolation-left").await;
    let right = scram_cluster(&emulator, "scram-isolation-right").await;
    let left_secrets = vec![
        secret_arn("AmazonMSK_left-a-1a1a1a"),
        secret_arn("AmazonMSK_left-b-2b2b2b"),
    ];
    let right_secrets = vec![secret_arn("AmazonMSK_right-a-3c3c3c")];

    assert_eq!(
        associate(&emulator, &left, &left_secrets).await.status,
        200,
        "the left cluster's association must succeed"
    );
    assert_eq!(
        associate(&emulator, &right, &right_secrets).await.status,
        200,
        "the right cluster's association must succeed"
    );

    for (case, cluster, mut wanted) in [
        ("left", left.clone(), left_secrets.clone()),
        ("right", right.clone(), right_secrets.clone()),
    ] {
        wanted.sort();
        let stored = listed(&list(&emulator, &cluster, "").await, case);
        assert_eq!(
            stored, wanted,
            "case {case}: expected the cluster to hold only its own secrets, got {stored:?}"
        );
    }

    // Removing the left cluster's secrets through the right one removes nothing:
    // the right cluster does not hold them.
    assert_eq!(
        disassociate(&emulator, &right, &left_secrets).await.status,
        200,
        "expected the cross-cluster removal to be accepted and remove nothing"
    );
    let mut wanted = left_secrets.clone();
    wanted.sort();
    let stored = listed(&list(&emulator, &left, "").await, "left-after-cross-removal");
    assert_eq!(
        stored, wanted,
        "expected the left cluster to keep its secrets, got {stored:?}"
    );
}

/// RULE: a cluster's secret set is bounded, so a batch that would take it past
/// the bound is refused whole and the cluster keeps exactly what it held. The
/// two rows differ along WHERE the excess comes from: one call that alone
/// carries more than the bound, and one that exceeds it only because of what
/// the cluster already holds.
#[tokio::test]
async fn a_batch_past_the_per_cluster_bound_is_refused_whole() {
    let emulator = Emulator::new();
    let cluster = scram_cluster(&emulator, "scram-ceiling").await;

    let over_the_bound: Vec<String> = (0..=MAX_SCRAM_SECRETS_PER_CLUSTER)
        .map(|index| secret_arn(&format!("AmazonMSK_over-{index:05}-0a1b2c")))
        .collect();
    let refused = associate(&emulator, &cluster, &over_the_bound).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (429, "TooManyRequestsException"),
        "case one-call-over-the-bound: expected 429 TooManyRequestsException, got {} {}",
        refused.status,
        refused.body
    );
    assert!(
        refused
            .error_message()
            .contains(&MAX_SCRAM_SECRETS_PER_CLUSTER.to_string()),
        "case one-call-over-the-bound: expected the refusal to state the bound, got {:?}",
        refused.error_message()
    );
    let stored = list_every_page(&emulator, &cluster, None, "one-call-over-the-bound").await;
    assert!(
        stored.is_empty(),
        "case one-call-over-the-bound: expected a refused batch to store none of its secrets, \
         got {} secret(s)",
        stored.len()
    );

    // Exactly the bound is accepted, so the refusal above is about the excess
    // and not about the size of the call.
    let to_the_bound: Vec<String> = (0..MAX_SCRAM_SECRETS_PER_CLUSTER)
        .map(|index| secret_arn(&format!("AmazonMSK_fill-{index:05}-3d4e5f")))
        .collect();
    assert_eq!(
        associate(&emulator, &cluster, &to_the_bound).await.status,
        200,
        "case at-the-bound: expected a batch filling the set to the bound to be accepted"
    );
    let full = list_every_page(&emulator, &cluster, None, "at-the-bound").await;
    assert_eq!(
        full.len(),
        MAX_SCRAM_SECRETS_PER_CLUSTER,
        "case at-the-bound: expected the cluster to hold the whole batch, got {} secret(s)",
        full.len()
    );

    // One more secret would take it past the bound, so it is refused and the
    // stored set is untouched.
    let one_too_many = secret_arn("AmazonMSK_one-too-many-6g7h8i");
    let refused = associate(&emulator, &cluster, std::slice::from_ref(&one_too_many)).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (429, "TooManyRequestsException"),
        "case one-past-a-full-set: expected 429 TooManyRequestsException, got {} {}",
        refused.status,
        refused.body
    );
    let after = list_every_page(&emulator, &cluster, None, "one-past-a-full-set").await;
    assert_eq!(
        after, full,
        "case one-past-a-full-set: expected the stored set to be untouched"
    );

    // Re-associating a secret the full set already holds is still accepted: it
    // adds nothing, so it cannot take the set past the bound.
    let held = full.first().expect("a full set holds a secret").clone();
    assert_eq!(
        associate(&emulator, &cluster, std::slice::from_ref(&held))
            .await
            .status,
        200,
        "case repeat-at-the-bound: expected a batch that adds nothing to be accepted"
    );
}

/// A stored secret survives the reader it is reported through being called
/// twice: the listing writes nothing, so two identical reads answer identically.
#[tokio::test]
async fn the_listing_stores_nothing() {
    let emulator = Emulator::new();
    let cluster = scram_cluster(&emulator, "scram-reads-are-reads").await;
    let secrets = vec![
        secret_arn("AmazonMSK_read-a-6f6f6f"),
        secret_arn("AmazonMSK_read-b-7g7g7g"),
    ];
    assert_eq!(
        associate(&emulator, &cluster, &secrets).await.status,
        200,
        "the association under test must succeed first"
    );
    let mut expected = secrets.clone();
    expected.sort();

    let first = listed(&list(&emulator, &cluster, "").await, "first-read");
    let second = listed(&list(&emulator, &cluster, "").await, "second-read");
    assert_eq!(
        (first.as_slice(), second.as_slice()),
        (expected.as_slice(), expected.as_slice()),
        "expected two identical reads to report the same stored set, got {first:?} then {second:?}"
    );
}

/// The JSON a successful association answers with, for the record: the stored
/// cluster's ARN and an empty `unprocessedScramSecrets`.
#[tokio::test]
async fn an_association_answers_the_stored_clusters_arn() {
    let emulator = Emulator::new();
    let cluster = scram_cluster(&emulator, "scram-response-shape").await;
    let reply = associate(
        &emulator,
        &cluster,
        &[secret_arn("AmazonMSK_shape-8h8h8h")],
    )
    .await;
    assert_eq!(
        reply.body,
        json!({ "clusterArn": cluster, "unprocessedScramSecrets": Value::Array(Vec::new()) }),
        "expected the association to answer the cluster it stored into and no unprocessed \
         secrets, got {}",
        reply.body
    );
}

/// RULE: the page size a listing accepts is the range its `MaxResults` member
/// declares — a value inside it caps the page at that many secrets and carries a
/// token exactly when secrets remain, and a value outside it is refused with the
/// service's bad-request error without listing or storing anything.
///
/// The bounds come from `MaxResults`'s `range` in this service's model
/// (`smithy_query(query="shape",shape="MaxResults")`: min 1, max
/// [`MAX_RESULTS_LIMIT`]), and the rows walk each published bound, a size inside
/// the range, and one value past each bound.
#[tokio::test]
async fn the_page_size_obeys_the_models_declared_range() {
    // The declared minimum of `MaxResults`' `range`; its maximum is the crate's
    // own `MAX_RESULTS_LIMIT`, which the listings share.
    const MIN_RESULTS: i32 = 1;

    let emulator = Emulator::new();
    let cluster = scram_cluster(&emulator, "scram-page-size-range").await;

    let secrets: Vec<String> = (0..5)
        .map(|index| secret_arn(&format!("AmazonMSK_range-{index}-4a5b6c")))
        .collect();
    assert_eq!(
        associate(&emulator, &cluster, &secrets).await.status,
        200,
        "the association that seeds the page-size table must succeed first"
    );
    let mut stored = secrets.clone();
    stored.sort();

    // Inside the range: the published minimum, a size short of the stored set,
    // the set's own size, and the published maximum.
    for page_size in [MIN_RESULTS, 4, stored.len() as i32, MAX_RESULTS_LIMIT] {
        let case = format!("maxResults={page_size}");
        let reply = list(&emulator, &cluster, &format!("?maxResults={page_size}")).await;
        let page = listed(&reply, &case);
        let expected_len = (page_size as usize).min(stored.len());
        assert_eq!(
            page,
            stored[..expected_len].to_vec(),
            "case {case}: expected the first {expected_len} stored secret(s), got {page:?}"
        );
        let token = reply.at("/nextToken");
        assert_eq!(
            token.is_string(),
            expected_len < stored.len(),
            "case {case}: expected a token exactly when secrets remain, got {}",
            reply.body
        );
    }

    // Past each published bound, so neither bound is merely clamped into range.
    // The refusal states the service's ONE sentence for a parameter it will not
    // accept — the same text whichever bound was broken, with nothing of the
    // request in it.
    for page_size in [-1, MIN_RESULTS - 1, MAX_RESULTS_LIMIT + 1] {
        let case = format!("maxResults={page_size}");
        let reply = list(&emulator, &cluster, &format!("?maxResults={page_size}")).await;
        assert_bad_request(&reply, &case);
        // The crate's one wording of a refused parameter, shared with the suite
        // that pins this service's refusal wording.
        let expected = super::refusal_wording::PARAMETERS_NOT_VALID;
        assert_eq!(
            reply.error_message(),
            expected,
            "case {case}: expected the refusal to state {expected:?}, got {:?}",
            reply.error_message()
        );
        assert!(
            !reply.error_message().contains("maxResults"),
            "case {case}: the refusal must not name the member, got {:?}",
            reply.error_message()
        );
    }

    // A refused page size changed nothing: the set is still what was stored.
    let after = list_every_page(&emulator, &cluster, None, "after-refused-page-sizes").await;
    assert_eq!(
        after, stored,
        "expected a refused page size to leave the stored set alone, got {after:?}"
    );
}
