// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for CodeArtifact's domain/repository lifecycle.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. A create answers with the record it stored, and a later read of the same resource
//!    answers with that same record — byte-identical, including the ARN, the description,
//!    the upstream order and `createdTime`.
//! 2. A resource identifier is unique within its container: a second create of a name
//!    already held is `ConflictException` (409), and addressing a name not held is
//!    `ResourceNotFoundException` (404).
//! 3. A delete removes the resource, answers with it as it was just before removal, and
//!    is observable to the very next call.
//! 4. A `@paginated` list returns every matching resource exactly once in key order,
//!    across any page size, and omits `nextToken` on the last page.
//! 5. A domain's `repositoryCount` is the number of repositories it holds, at every point
//!    in the lifecycle.
//!
//! The `Answer`/`emulator`/`call` plumbing lives in the shared `harness` module, so every
//! suite in this crate drives the same front door; the `create_repository` /
//! `describe_repository` helpers below are `pub(crate)` for the sibling suites that
//! address a repository (`repository_endpoint.rs`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// Used here AND re-exported `pub(crate)`: several of the crate's suites import this
// plumbing from this module (`external_connection_disassociation.rs` and
// `error_message_composition.rs` do), so the re-export is what lets either path —
// `crate::harness::call` or `crate::domain_repository_lifecycle::call` — name the one
// shared harness in `tests/all/harness.rs`.
pub(crate) use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT, REGION};

async fn describe_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await
}

async fn delete_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/domain?domain={domain}"),
        None,
    )
    .await
}

async fn list_domains(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/v1/domains", Some(body)).await
}

pub(crate) async fn create_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    description: Option<&str>,
    upstreams: &[&str],
) -> Answer {
    let mut body = json!({});
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if !upstreams.is_empty() {
        body["upstreams"] = Value::Array(
            upstreams
                .iter()
                .map(|name| json!({ "repositoryName": name }))
                .collect(),
        );
    }
    call(
        router,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(body),
    )
    .await
}

pub(crate) async fn describe_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

async fn delete_repository(router: &MultiServiceRouter, domain: &str, repository: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

async fn list_repositories(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/domain/repositories?{query}"),
        Some(json!({})),
    )
    .await
}

/// `repositoryCount` as the wire reports it. The member is modelled `@default(0)`, so a
/// zero count may legitimately be omitted from the body rather than sent as `0`.
fn repository_count(domain: &Value) -> i64 {
    domain
        .get("repositoryCount")
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// The `name` of every entry in a list response's items array, in the order returned.
fn names(items: Option<&Value>) -> Vec<String> {
    items
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("<missing name>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One repository to create inside a case's domain.
struct RepoCase {
    name: String,
    description: Option<String>,
    /// Names of already-created repositories in the same domain, in priority order.
    upstreams: Vec<String>,
}

/// One domain, with the repositories to create inside it.
struct DomainCase {
    label: &'static str,
    domain: String,
    encryption_key: Option<String>,
    repositories: Vec<RepoCase>,
}

fn repo(name: &str, description: Option<&str>, upstreams: &[&str]) -> RepoCase {
    RepoCase {
        name: name.to_string(),
        description: description.map(str::to_string),
        upstreams: upstreams.iter().map(|name| (*name).to_string()).collect(),
    }
}

/// The lifecycle table. Rows differ along what the rules generalise over: the number of
/// repositories in a domain (0, 1, 3), whether a description is present / empty / at its
/// modelled maximum length, the number of upstreams (0, 1, 2) and their order, the
/// character classes a name admits, and the extremes of the modelled name lengths.
///
/// The bounds come from the Smithy model (`smithy_query(query="operation",
/// operation="CreateDomain,CreateRepository")`): `DomainName` is len 2..50 pattern
/// `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`, `RepositoryName` is len 2..100 pattern
/// `^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$`, `Description` is len 0..1000.
fn lifecycle_cases() -> Vec<DomainCase> {
    // A 50-character domain name: the modelled maximum.
    let longest_domain = format!("d{}e", "x".repeat(48));
    // A 100-character repository name: the modelled maximum.
    let longest_repo = format!("R{}9", "n".repeat(98));
    // A 1000-character description: the modelled maximum.
    let longest_description = "d".repeat(1000);

    vec![
        DomainCase {
            label: "shortest legal names, empty domain",
            domain: "ab".to_string(),
            encryption_key: None,
            repositories: Vec::new(),
        },
        DomainCase {
            label: "shortest legal names, one bare repository",
            domain: "cd".to_string(),
            encryption_key: None,
            repositories: vec![repo("r1", None, &[])],
        },
        DomainCase {
            label: "caller-supplied key, upstream chain of three",
            domain: "my-domain".to_string(),
            encryption_key: Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/caller-supplied-key"
            )),
            repositories: vec![
                repo("base", Some("demo"), &[]),
                repo("My.Repo_1-2", Some(""), &["base"]),
                // Two upstreams, in a priority order that is NOT the sorted order — the
                // response must preserve the caller's order.
                repo("Zz", None, &["My.Repo_1-2", "base"]),
            ],
        },
        DomainCase {
            label: "modelled maximum name and description lengths",
            domain: longest_domain,
            encryption_key: None,
            repositories: vec![repo(&longest_repo, Some(&longest_description), &[])],
        },
    ]
}

/// RULES 1, 3 and 5: a create answers with what it stored, a read answers with the same
/// record, a delete removes it and is visible to the next call, and `repositoryCount`
/// tracks the repositories actually held throughout.
#[tokio::test]
async fn create_read_and_delete_round_trip_through_stored_state() {
    let router = emulator();

    for case in lifecycle_cases() {
        let label = case.label;
        let domain = case.domain.as_str();

        let created = create_domain(&router, domain, case.encryption_key.as_deref()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected CreateDomain 200, got {created:?}"
        );
        let created_domain = created.body["domain"].clone();
        assert_eq!(
            created_domain["name"].as_str(),
            Some(domain),
            "case {label:?}: expected domain.name {domain:?}, got {created_domain:?}"
        );
        assert_eq!(
            created_domain["owner"].as_str(),
            Some(ACCOUNT),
            "case {label:?}: expected domain.owner {ACCOUNT:?}, got {created_domain:?}"
        );
        assert_eq!(
            created_domain["arn"].as_str(),
            Some(format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}").as_str()),
            "case {label:?}: expected the regional domain ARN, got {created_domain:?}"
        );
        assert_eq!(
            created_domain["status"].as_str(),
            Some("Active"),
            "case {label:?}: expected domain.status Active, got {created_domain:?}"
        );
        assert!(
            created_domain["createdTime"].is_number(),
            "case {label:?}: expected a createdTime timestamp, got {created_domain:?}"
        );
        assert_eq!(
            repository_count(&created_domain),
            0,
            "case {label:?}: a new domain holds no repositories, got {created_domain:?}"
        );
        assert_eq!(
            created_domain["assetSizeBytes"].as_i64().unwrap_or(0),
            0,
            "case {label:?}: a new domain stores no assets, got {created_domain:?}"
        );
        assert!(
            created_domain["s3BucketArn"]
                .as_str()
                .is_some_and(|arn| arn.starts_with("arn:aws:s3:::assets-")),
            "case {label:?}: expected an asset-bucket ARN, got {created_domain:?}"
        );
        match case.encryption_key.as_deref() {
            // A caller-supplied key is reported back verbatim.
            Some(key) => assert_eq!(
                created_domain["encryptionKey"].as_str(),
                Some(key),
                "case {label:?}: expected the caller's encryptionKey, got {created_domain:?}"
            ),
            // With no key named, the domain reports the managed key it was given.
            None => assert!(
                created_domain["encryptionKey"]
                    .as_str()
                    .is_some_and(|arn| arn.starts_with(&format!("arn:aws:kms:{REGION}:"))),
                "case {label:?}: expected a KMS key ARN, got {created_domain:?}"
            ),
        }

        // RULE 1: the read answers with the stored record, not a fresh synthesis.
        let described = describe_domain(&router, domain).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: expected DescribeDomain 200, got {described:?}"
        );
        assert_eq!(
            described.body["domain"], created_domain,
            "case {label:?}: DescribeDomain must answer with the created record, got {:?}",
            described.body
        );

        // Create every repository in the case, checking each round-trips.
        for (index, repo_case) in case.repositories.iter().enumerate() {
            let name = repo_case.name.as_str();
            let upstreams: Vec<&str> = repo_case.upstreams.iter().map(String::as_str).collect();
            let created_repo = create_repository(
                &router,
                domain,
                name,
                repo_case.description.as_deref(),
                &upstreams,
            )
            .await;
            assert_eq!(
                created_repo.status, 200,
                "case {label:?}/{name:?}: expected CreateRepository 200, got {created_repo:?}"
            );
            let repo_body = created_repo.body["repository"].clone();
            assert_eq!(
                repo_body["name"].as_str(),
                Some(name),
                "case {label:?}/{name:?}: expected repository.name {name:?}, got {repo_body:?}"
            );
            assert_eq!(
                repo_body["domainName"].as_str(),
                Some(domain),
                "case {label:?}/{name:?}: expected domainName {domain:?}, got {repo_body:?}"
            );
            assert_eq!(
                repo_body["domainOwner"].as_str(),
                Some(ACCOUNT),
                "case {label:?}/{name:?}: expected domainOwner {ACCOUNT:?}, got {repo_body:?}"
            );
            assert_eq!(
                repo_body["administratorAccount"].as_str(),
                Some(ACCOUNT),
                "case {label:?}/{name:?}: the domain owner administers the repository, got {repo_body:?}"
            );
            assert_eq!(
                repo_body["arn"].as_str(),
                Some(
                    format!(
                        "arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/{domain}/{name}"
                    )
                    .as_str()
                ),
                "case {label:?}/{name:?}: expected the domain-qualified repository ARN, got {repo_body:?}"
            );
            assert_eq!(
                repo_body["description"].as_str(),
                repo_case.description.as_deref(),
                "case {label:?}/{name:?}: expected the description as given, got {repo_body:?}"
            );
            // RULE 1: upstream PRIORITY ORDER is the caller's order, preserved.
            let reported_upstreams: Vec<&str> = repo_body["upstreams"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| entry["repositoryName"].as_str().unwrap_or("<missing>"))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported_upstreams, upstreams,
                "case {label:?}/{name:?}: expected upstreams {upstreams:?} in order, got {repo_body:?}"
            );
            assert_eq!(
                repo_body["externalConnections"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or(0),
                0,
                "case {label:?}/{name:?}: a new repository has no external connection, got {repo_body:?}"
            );
            assert!(
                repo_body["createdTime"].is_number(),
                "case {label:?}/{name:?}: expected a createdTime timestamp, got {repo_body:?}"
            );

            let described_repo = describe_repository(&router, domain, name).await;
            assert_eq!(
                described_repo.status, 200,
                "case {label:?}/{name:?}: expected DescribeRepository 200, got {described_repo:?}"
            );
            assert_eq!(
                described_repo.body["repository"], repo_body,
                "case {label:?}/{name:?}: DescribeRepository must answer with the created record, got {:?}",
                described_repo.body
            );

            // RULE 5: the count follows the repositories actually held.
            let after = describe_domain(&router, domain).await;
            assert_eq!(
                repository_count(&after.body["domain"]),
                (index + 1) as i64,
                "case {label:?}/{name:?}: expected repositoryCount {}, got {:?}",
                index + 1,
                after.body
            );
        }

        // RULE 4: the list shows exactly the repositories created, in name order.
        let mut expected: Vec<String> = case
            .repositories
            .iter()
            .map(|repo| repo.name.clone())
            .collect();
        expected.sort();
        let listed = list_repositories(&router, &format!("domain={domain}")).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected ListRepositoriesInDomain 200, got {listed:?}"
        );
        assert_eq!(
            names(listed.body.get("repositories")),
            expected,
            "case {label:?}: expected {expected:?} in name order, got {:?}",
            listed.body
        );
        assert!(
            listed.body.get("nextToken").is_none_or(Value::is_null),
            "case {label:?}: a single complete page carries no nextToken, got {:?}",
            listed.body
        );

        // A domain is deletable only once it is empty.
        if !case.repositories.is_empty() {
            let before = describe_domain(&router, domain).await;
            let refused = delete_domain(&router, domain).await;
            assert_eq!(
                (refused.status, refused.code()),
                (409, "ConflictException".to_string()),
                "case {label:?}: deleting a non-empty domain must conflict, got {refused:?}"
            );
            let survived = describe_domain(&router, domain).await;
            assert_eq!(
                survived.body["domain"], before.body["domain"],
                "case {label:?}: a refused DeleteDomain must change nothing, got {:?}",
                survived.body
            );
        }

        // RULE 3: each delete answers with the record as it was, then it is gone.
        for repo_case in &case.repositories {
            let name = repo_case.name.as_str();
            let stored = describe_repository(&router, domain, name).await;
            let deleted = delete_repository(&router, domain, name).await;
            assert_eq!(
                deleted.status, 200,
                "case {label:?}/{name:?}: expected DeleteRepository 200, got {deleted:?}"
            );
            assert_eq!(
                deleted.body["repository"], stored.body["repository"],
                "case {label:?}/{name:?}: DeleteRepository must answer with the record as it was, got {:?}",
                deleted.body
            );
            let gone = describe_repository(&router, domain, name).await;
            assert_eq!(
                (gone.status, gone.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {label:?}/{name:?}: a deleted repository must be gone, got {gone:?}"
            );
        }

        let emptied = describe_domain(&router, domain).await;
        assert_eq!(
            repository_count(&emptied.body["domain"]),
            0,
            "case {label:?}: an emptied domain reports no repositories, got {:?}",
            emptied.body
        );

        let removed = delete_domain(&router, domain).await;
        assert_eq!(
            removed.status, 200,
            "case {label:?}: expected DeleteDomain 200, got {removed:?}"
        );
        assert_eq!(
            removed.body["domain"]["name"].as_str(),
            Some(domain),
            "case {label:?}: DeleteDomain must answer with the removed domain, got {:?}",
            removed.body
        );
        let gone = describe_domain(&router, domain).await;
        assert_eq!(
            (gone.status, gone.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a deleted domain must be gone, got {gone:?}"
        );
    }

    // Every case deleted everything it created, so nothing is left behind.
    let remaining = list_domains(&router, json!({})).await;
    assert_eq!(
        names(remaining.body.get("domains")),
        Vec::<String>::new(),
        "after deleting every domain, ListDomains must be empty, got {:?}",
        remaining.body
    );
}

/// RULE 2: identifiers are unique within their container, and addressing one that is not
/// held is a 404 — for every operation that addresses a domain or a repository.
#[tokio::test]
async fn duplicate_names_conflict_and_unknown_names_are_not_found() {
    let router = emulator();

    assert_eq!(
        create_domain(&router, "held", None).await.status,
        200,
        "setup: the domain must be created"
    );
    assert_eq!(
        create_repository(&router, "held", "held-repo", None, &[])
            .await
            .status,
        200,
        "setup: the repository must be created"
    );

    // (operation label, answer, expected status, expected error code)
    let conflicts = vec![
        (
            "CreateDomain for a name already held",
            create_domain(&router, "held", None).await,
        ),
        (
            "CreateRepository for a name already held in the domain",
            create_repository(&router, "held", "held-repo", Some("other"), &[]).await,
        ),
    ];
    for (label, answer) in conflicts {
        assert_eq!(
            (answer.status, answer.code()),
            (409, "ConflictException".to_string()),
            "case {label:?}: expected 409 ConflictException, got {answer:?}"
        );
    }

    let not_found = vec![
        (
            "DescribeDomain for an unknown domain",
            describe_domain(&router, "no-such-domain").await,
        ),
        (
            "DeleteDomain for an unknown domain",
            delete_domain(&router, "no-such-domain").await,
        ),
        (
            "CreateRepository in an unknown domain",
            create_repository(&router, "no-such-domain", "r1", None, &[]).await,
        ),
        (
            "DescribeRepository in an unknown domain",
            describe_repository(&router, "no-such-domain", "held-repo").await,
        ),
        (
            "DescribeRepository for an unknown repository",
            describe_repository(&router, "held", "no-such-repo").await,
        ),
        (
            "DeleteRepository for an unknown repository",
            delete_repository(&router, "held", "no-such-repo").await,
        ),
        (
            "DeleteRepository in an unknown domain",
            delete_repository(&router, "no-such-domain", "held-repo").await,
        ),
        (
            "ListRepositoriesInDomain for an unknown domain",
            list_repositories(&router, "domain=no-such-domain").await,
        ),
        (
            "CreateRepository naming an upstream the domain does not hold",
            create_repository(&router, "held", "with-bad-upstream", None, &["no-such-repo"]).await,
        ),
        (
            "DescribeRepository pinned to a domainOwner that does not own the domain",
            call(
                &router,
                "GET",
                "/v1/repository?domain=held&repository=held-repo&domain-owner=123456789012",
                None,
            )
            .await,
        ),
    ];
    for (label, answer) in not_found {
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 ResourceNotFoundException, got {answer:?}"
        );
    }

    // A refused create must not have stored anything.
    let listed = list_repositories(&router, "domain=held").await;
    assert_eq!(
        names(listed.body.get("repositories")),
        vec!["held-repo".to_string()],
        "a refused CreateRepository must store nothing, got {:?}",
        listed.body
    );

    // Pinning the owner correctly still resolves: the rule is the OWNER must match, not
    // that `domainOwner` must be absent.
    let pinned = call(
        &router,
        "GET",
        &format!("/v1/repository?domain=held&repository=held-repo&domain-owner={ACCOUNT}"),
        None,
    )
    .await;
    assert_eq!(
        pinned.status, 200,
        "a domainOwner that matches the owner must resolve, got {pinned:?}"
    );
}

/// RULE 4: over every page size, a paginated list returns each matching repository
/// exactly once in key order and omits `nextToken` only on the last page. The filters
/// compose with paging rather than replacing it.
#[tokio::test]
async fn repository_pages_cover_every_match_exactly_once() {
    let router = emulator();
    assert_eq!(
        create_domain(&router, "paged", None).await.status,
        200,
        "setup: the domain must be created"
    );

    // Deliberately created out of order, so a list that echoed insertion order would
    // differ from the key order the rule requires.
    let created = ["cc2", "aa1", "bb2", "cc1", "bb3", "aa2", "bb1"];
    for name in created {
        assert_eq!(
            create_repository(&router, "paged", name, None, &[])
                .await
                .status,
            200,
            "setup: repository {name:?} must be created"
        );
    }
    let mut all: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    all.sort();

    // `maxResults` is modelled range 1..1000, so the table walks both published bounds
    // and the sizes in between, plus the absent value (the service's own page size).
    let page_sizes: Vec<Option<i32>> = vec![
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
        Some(8),
        Some(1000),
    ];
    for size in page_sizes {
        let limit = size.unwrap_or(1000);
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = "domain=paged".to_string();
            if let Some(size) = size {
                query.push_str(&format!("&max-results={size}"));
            }
            if let Some(token) = &token {
                query.push_str(&format!("&next-token={token}"));
            }
            let answer = list_repositories(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={size:?} page {pages}: expected 200, got {answer:?}"
            );
            let page = names(answer.body.get("repositories"));
            token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len() as i32,
                    limit,
                    "case maxResults={size:?} page {pages}: a non-final page must be full, got {page:?}"
                );
            } else {
                assert!(
                    (page.len() as i32) <= limit,
                    "case maxResults={size:?} page {pages}: page over the limit, got {page:?}"
                );
            }
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case maxResults={size:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, all,
            "case maxResults={size:?}: expected every repository exactly once in name order, got {collected:?}"
        );
    }

    // `repositoryPrefix` narrows the set the pages cover; it does not change the rule.
    // The prefix is modelled len 2..100, so "bb"/"aa" are the shortest legal prefixes.
    let prefix_cases: Vec<(&str, Vec<&str>)> = vec![
        ("aa", vec!["aa1", "aa2"]),
        ("bb", vec!["bb1", "bb2", "bb3"]),
        ("bb1", vec!["bb1"]),
        ("zz", vec![]),
    ];
    for (prefix, expected) in prefix_cases {
        let answer =
            list_repositories(&router, &format!("domain=paged&repository-prefix={prefix}")).await;
        assert_eq!(
            names(answer.body.get("repositories")),
            expected.iter().map(|n| (*n).to_string()).collect::<Vec<_>>(),
            "case prefix={prefix:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }

    // A filtered page is `maxResults` MATCHING repositories, not `maxResults` scanned.
    let first = list_repositories(
        &router,
        "domain=paged&repository-prefix=bb&max-results=2",
    )
    .await;
    assert_eq!(
        names(first.body.get("repositories")),
        vec!["bb1".to_string(), "bb2".to_string()],
        "a filtered first page must be full of matches, got {:?}",
        first.body
    );
    let token = first.body["nextToken"]
        .as_str()
        .expect("a filtered first page with more matches carries a nextToken")
        .to_string();
    let second = list_repositories(
        &router,
        &format!("domain=paged&repository-prefix=bb&max-results=2&next-token={token}"),
    )
    .await;
    assert_eq!(
        names(second.body.get("repositories")),
        vec!["bb3".to_string()],
        "the filtered final page must hold the remaining match, got {:?}",
        second.body
    );
    assert!(
        second.body.get("nextToken").is_none_or(Value::is_null),
        "the filtered final page must omit nextToken, got {:?}",
        second.body
    );

    // `administratorAccount` matches the domain owner, and nothing else.
    let administered = list_repositories(
        &router,
        &format!("domain=paged&administrator-account={ACCOUNT}"),
    )
    .await;
    assert_eq!(
        names(administered.body.get("repositories")),
        all,
        "the domain owner administers every repository in it, got {:?}",
        administered.body
    );
    let other = list_repositories(&router, "domain=paged&administrator-account=123456789012").await;
    assert_eq!(
        names(other.body.get("repositories")),
        Vec::<String>::new(),
        "no repository is administered by another account, got {:?}",
        other.body
    );

    // One past each published `maxResults` bound is refused by the modelled range.
    for out_of_range in ["0", "1001"] {
        let answer =
            list_repositories(&router, &format!("domain=paged&max-results={out_of_range}")).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={out_of_range:?}: outside the modelled 1..1000 range must be refused, got {answer:?}"
        );
    }

    // A nextToken this service never issued names no place in the key order.
    let bogus = list_repositories(&router, "domain=paged&next-token=not-a-token").await;
    assert_eq!(
        (bogus.status, bogus.code()),
        (400, "ValidationException".to_string()),
        "a foreign nextToken must be refused, got {bogus:?}"
    );

    // ...including one that is well-formed hex decoding to a real key name ("aa1") but
    // carries no cursor marker: only tokens this service minted resolve.
    let unmarked = list_repositories(&router, "domain=paged&next-token=616131").await;
    assert_eq!(
        (unmarked.status, unmarked.code()),
        (400, "ValidationException".to_string()),
        "an unmarked hex nextToken must be refused, got {unmarked:?}"
    );

    // ...and a cursor this service minted for a DIFFERENT listing is foreign to this one:
    // a repository cursor names a place in one domain's repositories, not in the domains.
    let repo_token = list_repositories(&router, "domain=paged&max-results=1")
        .await
        .body["nextToken"]
        .as_str()
        .expect("a first page of seven carries a nextToken")
        .to_string();
    let wrong_listing = list_domains(&router, json!({ "nextToken": repo_token })).await;
    assert_eq!(
        (wrong_listing.status, wrong_listing.code()),
        (400, "ValidationException".to_string()),
        "a repository cursor must not resolve in the domain listing, got {wrong_listing:?}"
    );
}

/// RULE 4 again, for the other paginated reader in this slice: `ListDomains` covers every
/// stored domain exactly once in name order, over every page size.
#[tokio::test]
async fn domain_pages_cover_every_domain_exactly_once() {
    let router = emulator();

    let created = ["echo", "alfa", "delta", "bravo", "charlie"];
    for domain in created {
        assert_eq!(
            create_domain(&router, domain, None).await.status,
            200,
            "setup: domain {domain:?} must be created"
        );
    }
    let mut all: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    all.sort();

    for size in [None, Some(1), Some(2), Some(4), Some(5), Some(1000)] {
        let limit = size.unwrap_or(1000);
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(size) = size {
                body["maxResults"] = json!(size);
            }
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let answer = list_domains(&router, body).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={size:?} page {pages}: expected 200, got {answer:?}"
            );
            let page = names(answer.body.get("domains"));
            token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len() as i32,
                    limit,
                    "case maxResults={size:?} page {pages}: a non-final page must be full, got {page:?}"
                );
            }
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case maxResults={size:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, all,
            "case maxResults={size:?}: expected every domain exactly once in name order, got {collected:?}"
        );
    }

    // A domain removed mid-listing is simply absent from the remaining pages: the cursor
    // names a KEY, not an offset.
    assert_eq!(
        delete_domain(&router, "bravo").await.status,
        200,
        "setup: the domain must be deleted"
    );
    let after = list_domains(&router, json!({})).await;
    assert_eq!(
        names(after.body.get("domains")),
        vec![
            "alfa".to_string(),
            "charlie".to_string(),
            "delta".to_string(),
            "echo".to_string()
        ],
        "a deleted domain must not be listed, got {:?}",
        after.body
    );

    // A domain cursor is foreign to a repository listing, the mirror of the check in the
    // repository paging suite.
    let domain_token = list_domains(&router, json!({ "maxResults": 1 })).await.body["nextToken"]
        .as_str()
        .expect("a first page of four carries a nextToken")
        .to_string();
    let wrong_listing =
        list_repositories(&router, &format!("domain=alfa&next-token={domain_token}")).await;
    assert_eq!(
        (wrong_listing.status, wrong_listing.code()),
        (400, "ValidationException".to_string()),
        "a domain cursor must not resolve in a repository listing, got {wrong_listing:?}"
    );
}

/// Every record in this store is created by a request and freed only by an explicit
/// delete, so each growth site carries CodeArtifact's published quota and answers
/// `ServiceQuotaExceededException` (402) at the ceiling rather than growing without
/// limit. The quotas are the AWS CodeArtifact "Quotas" defaults: 10 domains per account,
/// 1,000 repositories per domain, 10 upstream repositories per repository.
#[tokio::test]
async fn store_growth_is_bounded_by_the_published_quotas() {
    let router = emulator();

    // Domains per account: the 10th is accepted, the 11th is refused.
    for index in 1..=10 {
        let domain = format!("quota-{index:02}");
        assert_eq!(
            create_domain(&router, &domain, None).await.status,
            200,
            "domain {domain:?} is within the quota and must be created"
        );
    }
    let over_domains = create_domain(&router, "quota-11", None).await;
    assert_eq!(
        (over_domains.status, over_domains.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "the 11th domain must exceed the account quota, got {over_domains:?}"
    );
    assert_eq!(
        names(list_domains(&router, json!({})).await.body.get("domains")).len(),
        10,
        "a refused CreateDomain must store nothing"
    );

    // Upstreams per repository: 10 is accepted, 11 is refused, and a repeat is refused
    // whatever the length (it would otherwise pass the existence check twice).
    let upstreams: Vec<String> = (1..=11).map(|index| format!("u{index:02}")).collect();
    for name in &upstreams {
        assert_eq!(
            create_repository(&router, "quota-01", name, None, &[])
                .await
                .status,
            200,
            "setup: upstream candidate {name:?} must be created"
        );
    }
    let borrowed: Vec<&str> = upstreams.iter().map(String::as_str).collect();

    let at_quota = create_repository(&router, "quota-01", "at-quota", None, &borrowed[..10]).await;
    assert_eq!(
        at_quota.status, 200,
        "10 upstreams is within the quota, got {at_quota:?}"
    );

    let over_upstreams = create_repository(&router, "quota-01", "over-quota", None, &borrowed).await;
    assert_eq!(
        (over_upstreams.status, over_upstreams.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "11 upstreams must exceed the per-repository quota, got {over_upstreams:?}"
    );

    let repeated = create_repository(
        &router,
        "quota-01",
        "repeated-upstream",
        None,
        &[borrowed[0], borrowed[0]],
    )
    .await;
    assert_eq!(
        (repeated.status, repeated.code()),
        (400, "ValidationException".to_string()),
        "a repeated upstream must be refused, got {repeated:?}"
    );

    // Neither refusal stored a repository.
    for refused in ["over-quota", "repeated-upstream"] {
        let gone = describe_repository(&router, "quota-01", refused).await;
        assert_eq!(
            (gone.status, gone.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {refused:?}: a refused CreateRepository must store nothing, got {gone:?}"
        );
    }
}

/// The repositories-per-domain quota, at its exact ceiling. Separated from the other
/// quotas because it fills a whole domain.
#[tokio::test]
async fn repository_count_is_bounded_by_the_published_quota() {
    let router = emulator();
    assert_eq!(
        create_domain(&router, "full", None).await.status,
        200,
        "setup: the domain must be created"
    );

    // 1,000 repositories per domain: the 1000th is accepted, the 1001st is refused.
    for index in 1..=1000 {
        let name = format!("r{index:04}");
        let answer = create_repository(&router, "full", &name, None, &[]).await;
        assert_eq!(
            answer.status, 200,
            "repository {name:?} is within the quota and must be created, got {answer:?}"
        );
    }
    let over = create_repository(&router, "full", "r1001", None, &[]).await;
    assert_eq!(
        (over.status, over.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "the 1001st repository must exceed the per-domain quota, got {over:?}"
    );
    assert_eq!(
        repository_count(&describe_domain(&router, "full").await.body["domain"]),
        1000,
        "a refused CreateRepository must not change repositoryCount"
    );
}
