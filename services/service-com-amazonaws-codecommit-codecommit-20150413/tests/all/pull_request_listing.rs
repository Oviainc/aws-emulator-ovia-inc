// SPDX-License-Identifier: CC0-1.0

//! A repository's pull requests over the wire: `ListPullRequests`, driven as
//! awsJson1_1 requests through the router that serves the real emulator (the wire
//! harness comes from the sibling `repository_lifecycle` suite, and calling as a
//! NAMED principal from `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: the operation reports the `pullRequestId` of every pull
//! request STORED on the named repository, most recently created first, narrowed by
//! `pullRequestStatus` and by `authorArn` and paged by `maxResults`/`nextToken` — and
//! a listing changes nothing a later caller sees.
//!
//! The tables below span what that rule generalises over: how many pull requests a
//! repository holds (none, one, several), which repository holds them (so a listing
//! is the named repository's rather than the account's), the identity that opened
//! each one, every value of `PullRequestStatusEnum` as a filter — including the one
//! value no pull request here would match on its own — an author filter naming a
//! well-formed ARN that opened nothing, both filters together, page sizes from one id
//! to past the whole list with each published `maxResults` bound and one past it, and
//! each way a request can name a listing that cannot be served.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, ok_as};

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The repository the pull requests are opened against, a SECOND repository with
/// its own pull request, and a third that holds none at all.
const REPOSITORY: &str = "pull-request-listing";
const OTHER_REPOSITORY: &str = "pull-request-listing-other";
const EMPTY_REPOSITORY: &str = "pull-request-listing-empty";

/// The destination every pull request below is opened against: `main` takes each
/// repository's first commit, so it is that repository's default branch.
const DESTINATION: &str = "main";

/// The principals that open the pull requests. A request speaks for the identity the
/// access key id it is signed with resolves to — the IAM user the account holds that
/// key for — so a pull request's `authorArn` is that user's ARN.
const AUTHOR_ONE: &str = "AKIDAUTHORONE";
const AUTHOR_TWO: &str = "AKIDAUTHORTWO";
/// A principal that opens nothing, so no pull request names it.
const BYSTANDER: &str = "AKIDBYSTANDER";

/// Every value of `PullRequestStatusEnum`
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`), which is the
/// catalog `pullRequestStatus` narrows the listing by.
const STATUSES: &[&str] = &["OPEN", "CLOSED"];

/// The two `PullRequestStatusEnum` values a pull request here holds: the one it is
/// opened with, and the one the merge below leaves it in
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The `maxResults` range a paginated CodeCommit list operation publishes, which it
/// words its refusal with: "A valid limit is between 1 and 1000".
const MIN_PAGE: usize = 1;
const MAX_PAGE: usize = 1000;

/// One pull request to open: which repository and source branch it is for, who
/// opens it, and whether it is then merged (which closes it).
struct Plan {
    case: &'static str,
    repository: &'static str,
    branch: &'static str,
    author: &'static str,
    merged: bool,
}

/// The pull requests this suite opens, in the order they are opened — so the ids
/// they are minted with ascend in this order.
const PLANS: &[Plan] = &[
    Plan {
        case: "the first author's open pull request",
        repository: REPOSITORY,
        branch: "feature-one",
        author: AUTHOR_ONE,
        merged: false,
    },
    Plan {
        case: "the second author's open pull request",
        repository: REPOSITORY,
        branch: "feature-two",
        author: AUTHOR_TWO,
        merged: false,
    },
    Plan {
        case: "the first author's pull request, merged and so closed",
        repository: REPOSITORY,
        branch: "feature-three",
        author: AUTHOR_ONE,
        merged: true,
    },
    Plan {
        case: "a pull request another repository holds",
        repository: OTHER_REPOSITORY,
        branch: "feature-elsewhere",
        author: AUTHOR_TWO,
        merged: false,
    },
];

/// One pull request as it is STORED: the repository that holds it, its id, the ARN
/// of the caller that opened it and the status it now has.
struct Opened {
    case: &'static str,
    repository: &'static str,
    id: String,
    author_arn: String,
    status: &'static str,
}

/// An emulator whose repositories hold the pull requests [`PLANS`] describes.
struct Listing {
    router: MultiServiceRouter,
    /// In the order they were opened, so reversing it is newest first.
    opened: Vec<Opened>,
}

/// Write one commit on `branch` through `PutFile`, as `caller`, reporting its id.
async fn put_file(
    router: &MultiServiceRouter,
    caller: &str,
    repository: &str,
    branch: &str,
    parent: Option<&str>,
    path: &str,
    content: &[u8],
) -> String {
    let mut body = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": path,
        "fileContent": base64(content),
        "commitMessage": "a revision",
    });
    if let Some(parent) = parent {
        body["parentCommitId"] = json!(parent);
    }
    string_at(&ok_as(router, caller, "PutFile", &body).await, "/commitId")
}

/// The ids and continuation token one `ListPullRequests` call answers.
async fn list(router: &MultiServiceRouter, body: &Value) -> (Vec<String>, Option<String>) {
    let payload = ok(router, "ListPullRequests", body).await;
    let ids = payload
        .get("pullRequestIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .map(|id| {
                    id.as_str()
                        .unwrap_or_else(|| panic!("a pull request id is a string, got {id}"))
                        .to_string()
                })
                .collect::<Vec<String>>()
        })
        .unwrap_or_else(|| panic!("no pullRequestIds in {payload}"));
    let next_token = payload
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next_token)
}

/// The pull request `GetPullRequest` reports.
async fn stored_pull_request(router: &MultiServiceRouter, pull_request_id: &str) -> Value {
    let read = ok(
        router,
        "GetPullRequest",
        &json!({ "pullRequestId": pull_request_id }),
    )
    .await;
    read.get("pullRequest")
        .unwrap_or_else(|| panic!("no pullRequest in {read}"))
        .clone()
}

/// Open every pull request [`PLANS`] describes, and report what the store then
/// holds.
async fn listing() -> Listing {
    let (router, _engine) = emulator_with_callers(&[AUTHOR_ONE, AUTHOR_TWO, BYSTANDER]);
    let mut base: Vec<(&str, String)> = Vec::new();
    for repository in [REPOSITORY, OTHER_REPOSITORY, EMPTY_REPOSITORY] {
        ok_as(
            &router,
            AUTHOR_ONE,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        // The first commit on `main` makes it the repository's default branch, so a
        // pull request can be opened against it.
        let tip = put_file(
            &router,
            AUTHOR_ONE,
            repository,
            DESTINATION,
            None,
            "README.md",
            b"alpha\n",
        )
        .await;
        base.push((repository, tip));
    }
    let tip_of = |repository: &str| -> String {
        base.iter()
            .find(|(name, _)| *name == repository)
            .map(|(_, tip)| tip.clone())
            .unwrap_or_else(|| panic!("no base commit for {repository}"))
    };

    let mut opened = Vec::new();
    for plan in PLANS {
        let destination_tip = tip_of(plan.repository);
        ok_as(
            &router,
            plan.author,
            "CreateBranch",
            &json!({
                "repositoryName": plan.repository,
                "branchName": plan.branch,
                "commitId": destination_tip,
            }),
        )
        .await;
        // Each source branch adds a file of its own, so no two of them conflict and
        // the merge below is a clean one.
        put_file(
            &router,
            plan.author,
            plan.repository,
            plan.branch,
            Some(&destination_tip),
            &format!("{}.txt", plan.branch),
            format!("work on {}\n", plan.branch).as_bytes(),
        )
        .await;
        let created = ok_as(
            &router,
            plan.author,
            "CreatePullRequest",
            &json!({
                "title": format!("review {}", plan.branch),
                "targets": [{
                    "repositoryName": plan.repository,
                    "sourceReference": plan.branch,
                    "destinationReference": DESTINATION,
                }],
            }),
        )
        .await;
        let id = string_at(&created, "/pullRequest/pullRequestId");
        if plan.merged {
            ok_as(
                &router,
                plan.author,
                "MergePullRequestByThreeWay",
                &json!({
                    "repositoryName": plan.repository,
                    "pullRequestId": id,
                    "commitMessage": format!("merge {}", plan.branch),
                    "authorName": "Merge Tester",
                    "email": "merge.tester@example.com",
                }),
            )
            .await;
        }
        opened.push(Opened {
            case: plan.case,
            repository: plan.repository,
            id,
            author_arn: caller_arn(plan.author),
            status: if plan.merged {
                STATUS_CLOSED
            } else {
                STATUS_OPEN
            },
        });
    }

    Listing { router, opened }
}

/// The ids the rule says a listing of `repository` narrowed this way must answer,
/// newest first.
fn expected_ids(
    opened: &[Opened],
    repository: &str,
    status: Option<&str>,
    author_arn: Option<&str>,
) -> Vec<String> {
    opened
        .iter()
        .rev()
        .filter(|stored| {
            stored.repository == repository
                && status.is_none_or(|requested| stored.status == requested)
                && author_arn.is_none_or(|requested| stored.author_arn == requested)
        })
        .map(|stored| stored.id.clone())
        .collect()
}

#[tokio::test]
async fn every_pull_request_a_repository_holds_is_listed_newest_first() {
    let listing = listing().await;

    for repository in [REPOSITORY, OTHER_REPOSITORY, EMPTY_REPOSITORY] {
        let expected = expected_ids(&listing.opened, repository, None, None);
        let (ids, next_token) = list(
            &listing.router,
            &json!({ "repositoryName": repository }),
        )
        .await;
        assert_eq!(
            ids, expected,
            "case {repository:?}: expected the repository's own pull requests newest first, got \
             {ids:?}"
        );
        assert!(
            next_token.is_none(),
            "case {repository:?}: expected no continuation token for a single page, got \
             {next_token:?}"
        );
    }

    // ROUND TRIP: every id listed reads back through `GetPullRequest` as a pull
    // request of that repository, with the author and status the listing filtered on.
    for stored in &listing.opened {
        let case = stored.case;
        let read = stored_pull_request(&listing.router, &stored.id).await;
        assert_eq!(
            read.pointer("/pullRequestTargets/0/repositoryName")
                .and_then(Value::as_str),
            Some(stored.repository),
            "case {case:?}: expected the listed id to read back as a pull request of its \
             repository, got {read}"
        );
        assert_eq!(
            read.get("authorArn").and_then(Value::as_str),
            Some(stored.author_arn.as_str()),
            "case {case:?}: expected the author the pull request was opened by, got {read}"
        );
        assert_eq!(
            read.get("pullRequestStatus").and_then(Value::as_str),
            Some(stored.status),
            "case {case:?}: expected the status the pull request now holds, got {read}"
        );
    }
}

#[tokio::test]
async fn the_listing_is_narrowed_by_status_and_by_author() {
    let listing = listing().await;
    // Every author filter worth asking: each caller that opened something, and a
    // well-formed ARN that opened nothing.
    let authors: Vec<Option<String>> = vec![
        None,
        Some(caller_arn(AUTHOR_ONE)),
        Some(caller_arn(AUTHOR_TWO)),
        Some(caller_arn(BYSTANDER)),
    ];

    for repository in [REPOSITORY, OTHER_REPOSITORY, EMPTY_REPOSITORY] {
        for status in STATUSES.iter().map(Some).chain(std::iter::once(None)) {
            for author in &authors {
                let mut body = json!({ "repositoryName": repository });
                if let Some(status) = status {
                    body["pullRequestStatus"] = json!(status);
                }
                if let Some(author) = author {
                    body["authorArn"] = json!(author);
                }
                let expected = expected_ids(
                    &listing.opened,
                    repository,
                    status.copied(),
                    author.as_deref(),
                );
                let (ids, _) = list(&listing.router, &body).await;
                assert_eq!(
                    ids, expected,
                    "case {body}: expected the pull requests matching every filter, got {ids:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_page_is_at_most_max_results_and_the_token_resumes_after_it() {
    let listing = listing().await;
    let whole = expected_ids(&listing.opened, REPOSITORY, None, None);

    for page_size in [MIN_PAGE, 2, 3, whole.len(), MAX_PAGE] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({
                "repositoryName": REPOSITORY,
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let (ids, next_token) = list(&listing.router, &body).await;
            assert!(
                !ids.is_empty() && ids.len() <= page_size,
                "case {page_size}: expected between 1 and {page_size} ids in a page, got {ids:?}"
            );
            seen.extend(ids);
            match next_token {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, whole,
            "case {page_size}: expected the paged walk to answer the whole listing once, in \
             order, got {seen:?}"
        );
    }
}

/// One refused request: what it names, and the code and message it must answer.
struct Refusal {
    case: &'static str,
    body: Value,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_listing_that_cannot_be_served_is_refused_with_the_modeled_error() {
    let listing = listing().await;
    let refusals = vec![
        Refusal {
            case: "a repository name no repository has",
            body: json!({ "repositoryName": "no-such-repository" }),
            code: "RepositoryDoesNotExistException",
            message: format!(
                "no-such-repository for {}",
                engine::emulator_account_id()
            ),
        },
        Refusal {
            case: "no repository name at all",
            body: json!({}),
            code: "RepositoryNameRequiredException",
            message: "A repository name is required, but was not specified.".to_string(),
        },
        Refusal {
            case: "a repository name outside the pattern the model publishes",
            body: json!({ "repositoryName": "not a name!" }),
            code: "InvalidRepositoryNameException",
            message: "A specified repository name is not valid.".to_string(),
        },
        Refusal {
            case: "an author that is no ARN at all",
            body: json!({ "repositoryName": REPOSITORY, "authorArn": "Mary_Major" }),
            code: "InvalidAuthorArnException",
            message: "The Amazon Resource Name (ARN) is not valid. Make sure that you have \
                      provided the full ARN for the author of the pull request, and then try \
                      again."
                .to_string(),
        },
        Refusal {
            case: "an author ARN whose account is not an account id",
            body: json!({
                "repositoryName": REPOSITORY,
                "authorArn": "arn:aws:iam::12345:user/Mary_Major",
            }),
            code: "InvalidAuthorArnException",
            message: "The Amazon Resource Name (ARN) is not valid. Make sure that you have \
                      provided the full ARN for the author of the pull request, and then try \
                      again."
                .to_string(),
        },
        Refusal {
            case: "an author named by an empty string, which is no ARN at all",
            body: json!({ "repositoryName": REPOSITORY, "authorArn": "" }),
            code: "InvalidAuthorArnException",
            message: "The Amazon Resource Name (ARN) is not valid. Make sure that you have \
                      provided the full ARN for the author of the pull request, and then try \
                      again."
                .to_string(),
        },
        Refusal {
            case: "an author named by blank space, which is no ARN either",
            body: json!({ "repositoryName": REPOSITORY, "authorArn": "   " }),
            code: "InvalidAuthorArnException",
            message: "The Amazon Resource Name (ARN) is not valid. Make sure that you have \
                      provided the full ARN for the author of the pull request, and then try \
                      again."
                .to_string(),
        },
        Refusal {
            case: "a status outside the values the model declares",
            body: json!({ "repositoryName": REPOSITORY, "pullRequestStatus": "MERGED" }),
            code: "InvalidPullRequestStatusException",
            message: "The pull request status is not valid. The only valid values are OPEN and \
                      CLOSED."
                .to_string(),
        },
        Refusal {
            case: "a page size below the published minimum",
            body: json!({ "repositoryName": REPOSITORY, "maxResults": 0 }),
            code: "InvalidMaxResultsException",
            message: format!("A valid limit is between {MIN_PAGE} and {MAX_PAGE}"),
        },
        Refusal {
            case: "a page size one past the published maximum",
            body: json!({ "repositoryName": REPOSITORY, "maxResults": MAX_PAGE + 1 }),
            code: "InvalidMaxResultsException",
            message: format!("A valid limit is between {MIN_PAGE} and {MAX_PAGE}"),
        },
        Refusal {
            case: "a continuation token this service never issued",
            body: json!({ "repositoryName": REPOSITORY, "nextToken": "not-a-token" }),
            code: "InvalidContinuationTokenException",
            message: "Error parsing next token: not-a-token".to_string(),
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let (code, message) = error_with_message(
            &listing.router,
            "ListPullRequests",
            &refusal.body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, refusal.code,
            "case {case:?}: expected the error the model declares for it, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message, refusal.message,
            "case {case:?}: expected the sentence that code is worded with, got {message:?}"
        );
    }

    // A token belongs to the listing it was issued for: carried into another
    // repository's listing it names no position at all.
    let (_, token) = list(
        &listing.router,
        &json!({ "repositoryName": REPOSITORY, "maxResults": MIN_PAGE }),
    )
    .await;
    let token = token.unwrap_or_else(|| panic!("expected a token while a further page exists"));
    let (code, message) = error_with_message(
        &listing.router,
        "ListPullRequests",
        &json!({ "repositoryName": EMPTY_REPOSITORY, "nextToken": token }),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "case {:?}: expected a token from another listing to be refused, got {code:?} with \
         {message:?}",
        "a token issued for another repository's listing"
    );
    assert_eq!(
        message,
        format!("Error parsing next token: {token}"),
        "case {:?}: expected the sentence that code is worded with, got {message:?}",
        "a token issued for another repository's listing"
    );

    // Each published bound is served rather than refused.
    for page_size in [MIN_PAGE, MAX_PAGE] {
        let (ids, _) = list(
            &listing.router,
            &json!({ "repositoryName": REPOSITORY, "maxResults": page_size }),
        )
        .await;
        assert!(
            !ids.is_empty(),
            "case {page_size}: expected a published bound to be served, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn listing_the_pull_requests_changes_nothing_a_later_caller_sees() {
    let listing = listing().await;
    let body = json!({ "repositoryName": REPOSITORY });
    let mut before = Vec::new();
    for stored in &listing.opened {
        before.push(stored_pull_request(&listing.router, &stored.id).await);
    }

    let (first, first_token) = list(&listing.router, &body).await;
    let (second, second_token) = list(&listing.router, &body).await;
    assert_eq!(
        (first, first_token),
        (second, second_token),
        "case {:?}: expected two listings of the same repository to answer the same",
        "the listing is read twice"
    );

    for (stored, was) in listing.opened.iter().zip(before) {
        let case = stored.case;
        let now = stored_pull_request(&listing.router, &stored.id).await;
        assert_eq!(
            now, was,
            "case {case:?}: expected listing the pull requests to leave the stored pull request \
             as it was, got {now}"
        );
    }
}
