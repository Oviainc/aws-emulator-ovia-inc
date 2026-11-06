// SPDX-License-Identifier: CC0-1.0

//! Two request members MediaPackage treats as MUTUALLY EXCLUSIVE cannot both be specified.
//!
//! The rule under test: where this service treats two request members as mutually
//! exclusive, a request specifying BOTH is refused with the modelled
//! `UnprocessableEntityException` (HTTP 422, `smithy_query(query="operation",
//! operation="ListHarvestJobs")`) keyed by one of them and naming the other, instead of
//! being served by applying both rules together — and the refusal happens before any store
//! query, so such a request reports no page at all and leaves every stored record as it
//! was. Either member ALONE is served exactly as before.
//!
//! `ListHarvestJobs`' `IncludeChannelId` (`@httpQuery("includeChannelId")`) and
//! `IncludeStatus` (`@httpQuery("includeStatus")`) are this service's one such pair: they
//! are the only two FILTER members any operation here binds (`ListChannels` binds
//! `MaxResults`/`NextToken` only and `ListOriginEndpoints` adds one channel filter, so
//! neither can carry a contradictory pair), which is why the suite drives that operation.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//! The harness (`call`, `emulator`, `error_code`, …) is the one
//! `tests/all/channel_lifecycle.rs` owns.
//!
//! The rows differ along what the rule generalises over: which VALUES the two filters
//! carry (a channel holding jobs, one holding none, a status jobs are in, a terminal
//! status, a value that is no status at all, an empty value), the ORDER the two parameters
//! are sent in, whether a parameter is repeated, and whether the pair travels with other
//! query parameters.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{
    call, create_channel, emulator, error_code, CROSS_ACCOUNT_HARVEST_ROLE,
};

/// The channels this suite harvests from, with the endpoint each one holds and the jobs
/// submitted against that endpoint — two channels, so a channel filter has something to
/// exclude, and two jobs on one of them, so a filtered page is not a single entry.
const FIXTURES: &[(&str, &str, &[&str])] = &[
    ("filters-news", "news-endpoint", &["job-news-a", "job-news-b"]),
    ("filters-sports", "sports-endpoint", &["job-sports"]),
];

/// A startover window every fixture endpoint retains, so a harvest of it is accepted: a
/// day, well above the minimum the service enforces
/// (`state::MIN_ENABLED_STARTOVER_WINDOW_SECONDS`).
const WINDOW_SECONDS: i32 = 86_400;

/// Every job id the fixtures submit, in id order.
fn all_jobs() -> Vec<String> {
    let mut ids: Vec<String> = FIXTURES
        .iter()
        .flat_map(|(_, _, jobs)| jobs.iter().map(|id| id.to_string()))
        .collect();
    ids.sort();
    ids
}

/// The jobs harvesting `channel`, in id order.
fn jobs_of(channel: &str) -> Vec<String> {
    let mut ids: Vec<String> = FIXTURES
        .iter()
        .filter(|(id, _, _)| *id == channel)
        .flat_map(|(_, _, jobs)| jobs.iter().map(|id| id.to_string()))
        .collect();
    ids.sort();
    ids
}

/// Create every fixture channel, endpoint and harvest job.
async fn provision(router: &MultiServiceRouter) {
    for (channel, endpoint, jobs) in FIXTURES {
        create_channel(router, channel).await;
        let (status, body) = call(
            router,
            "POST",
            "/origin_endpoints",
            Some(&json!({
                "channelId": channel,
                "id": endpoint,
                "hlsPackage": {},
                "startoverWindowSeconds": WINDOW_SECONDS
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "creating the fixture endpoint {endpoint:?} expected 200, got {status} {body:?}"
        );
        for job in *jobs {
            let (status, body) = call(
                router,
                "POST",
                "/harvest_jobs",
                Some(&json!({
                    "id": job,
                    "originEndpointId": endpoint,
                    "startTime": "2026-01-01T00:00:00Z",
                    "endTime": "2026-01-01T01:00:00Z",
                    "s3Destination": {
                        "bucketName": "filter-harvests",
                        "manifestKey": "harvests/index.m3u8",
                        "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
                    }
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "submitting the fixture job {job:?} expected 200, got {status} {body:?}"
            );
        }
    }
}

/// The `harvestJobs` entries of a listing body.
fn listed(body: &Value) -> Vec<Value> {
    match body.get("harvestJobs") {
        None | Some(Value::Null) => Vec::new(),
        Some(jobs) => jobs
            .as_array()
            .unwrap_or_else(|| panic!("harvestJobs must be a list, got {jobs:?}"))
            .clone(),
    }
}

/// The ids a listing body reports, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|job| {
            job.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `GET /harvest_jobs` with `query` appended verbatim.
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/harvest_jobs{query}"), None).await
}

/// Every id the UNFILTERED listing reports, in id order.
async fn every_listed_id(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = list(router, "").await;
    assert_eq!(
        status, 200,
        "the unfiltered ListHarvestJobs expected 200, got {status} {body:?}"
    );
    let mut ids = listed_ids(&body);
    ids.sort();
    ids
}

/// One request whose two filters contradict each other.
struct Both {
    case: &'static str,
    /// The query string as sent, `?`-prefixed.
    query: String,
}

/// The requests that carry BOTH filters and nothing else a request could be refused for,
/// so the whole refusal envelope is the pair's.
fn both_filters() -> Vec<Both> {
    let channel = FIXTURES[0].0;
    vec![
        Both {
            case: "a channel holding jobs and the status a submitted job starts in",
            query: format!(
                "?includeChannelId={channel}&includeStatus={}",
                state::HARVEST_IN_PROGRESS
            ),
        },
        Both {
            case: "a channel holding jobs and the terminal status a failed harvest reaches",
            query: format!(
                "?includeChannelId={channel}&includeStatus={}",
                state::HARVEST_FAILED
            ),
        },
        Both {
            case: "a channel the service does not hold, and a real status",
            query: format!(
                "?includeChannelId=holds-nothing&includeStatus={}",
                state::HARVEST_IN_PROGRESS
            ),
        },
        Both {
            case: "a value that is no status at all",
            query: format!("?includeChannelId={channel}&includeStatus=NOT_A_STATUS"),
        },
        Both {
            case: "the two parameters in the other order",
            query: format!(
                "?includeStatus={}&includeChannelId={channel}",
                state::HARVEST_IN_PROGRESS
            ),
        },
        Both {
            case: "an empty status value",
            query: format!("?includeChannelId={channel}&includeStatus="),
        },
        Both {
            case: "an empty channel value",
            query: format!("?includeChannelId=&includeStatus={}", state::HARVEST_FAILED),
        },
        Both {
            case: "the channel parameter repeated",
            query: format!(
                "?includeChannelId={channel}&includeChannelId=filters-sports&includeStatus={}",
                state::HARVEST_IN_PROGRESS
            ),
        },
        Both {
            case: "both filters alongside the largest page size the service serves",
            query: format!(
                "?includeChannelId={channel}&includeStatus={}&maxResults={}",
                state::HARVEST_IN_PROGRESS,
                state::MAX_PAGE_SIZE
            ),
        },
        Both {
            case: "both filters alongside the smallest page size",
            query: format!(
                "?maxResults={}&includeChannelId={channel}&includeStatus={}",
                state::MIN_PAGE_SIZE,
                state::HARVEST_SUCCEEDED
            ),
        },
    ]
}

#[tokio::test]
async fn specifying_both_filters_is_refused_and_reports_nothing() {
    let router = emulator();
    provision(&router).await;
    let before = every_listed_id(&router).await;
    assert_eq!(
        before,
        all_jobs(),
        "the fixtures must be listable before the refusals, got {before:?}"
    );

    // The requirement the refusal states: the member it is keyed by, and the other member
    // named by its modelled name — taken from the service's own wording rather than
    // restated here.
    let expected = state::harvest_job_filters_mutually_exclusive();

    for row in both_filters() {
        let case = format!("{} ({})", row.case, row.query);
        let (status, refusal) = list(&router, &row.query).await;
        assert_eq!(
            status,
            expected.status(),
            "case {case:?}: expected {}, got {status} {refusal:?}",
            expected.status()
        );
        assert_eq!(
            error_code(&refusal),
            expected.code(),
            "case {case:?}: expected {}, got {refusal:?}",
            expected.code()
        );
        assert_eq!(
            refusal.get("message").and_then(Value::as_str),
            Some(expected.message().as_str()),
            "case {case:?}: the refusal must state the exclusion, got {refusal:?}"
        );
        let reason = refusal
            .pointer(&format!("/errors/{}", state::INCLUDE_STATUS_QUERY))
            .and_then(Value::as_str);
        assert!(
            reason.is_some_and(|reason| reason.contains(state::INCLUDE_CHANNEL_ID_MEMBER)),
            "case {case:?}: the refusal must be keyed by {:?} and name the other member \
             {:?}, got {refusal:?}",
            state::INCLUDE_STATUS_QUERY,
            state::INCLUDE_CHANNEL_ID_MEMBER
        );
        // Nothing was reported: a refused list serves no page and no token.
        assert!(
            listed(&refusal).is_empty(),
            "case {case:?}: a refused list must report no job, got {refusal:?}"
        );
        assert_eq!(
            refusal.get("nextToken"),
            None,
            "case {case:?}: a refused list must report no token, got {refusal:?}"
        );
    }

    // Nothing changed: every stored job is still listable, exactly as before.
    let after = every_listed_id(&router).await;
    assert_eq!(
        after, before,
        "a refused list must leave every stored job as it was, got {after:?}"
    );
}

#[tokio::test]
async fn both_filters_are_refused_even_beside_another_unacceptable_parameter() {
    let router = emulator();
    provision(&router).await;

    // A page size outside the bound and a token this service never issued are each refused
    // on their own; carrying the contradictory pair as well does not make the request
    // acceptable, and the answer is still this service's 422.
    for query in [
        format!(
            "?includeChannelId={}&includeStatus={}&maxResults=0",
            FIXTURES[0].0,
            state::HARVEST_IN_PROGRESS
        ),
        format!(
            "?includeChannelId={}&includeStatus={}&nextToken=not-a-token",
            FIXTURES[0].0,
            state::HARVEST_IN_PROGRESS
        ),
    ] {
        let (status, refusal) = list(&router, &query).await;
        assert_eq!(
            status, 422,
            "case {query:?}: expected 422, got {status} {refusal:?}"
        );
        assert_eq!(
            error_code(&refusal),
            "UnprocessableEntityException",
            "case {query:?}: expected UnprocessableEntityException, got {refusal:?}"
        );
        assert!(
            listed(&refusal).is_empty(),
            "case {query:?}: a refused list must report no job, got {refusal:?}"
        );
    }
}

#[tokio::test]
async fn either_filter_alone_is_still_served() {
    let router = emulator();
    provision(&router).await;

    // A channel filter alone: exactly the jobs harvesting that channel, whatever each
    // one's status is by now.
    for channel in [FIXTURES[0].0, FIXTURES[1].0, "holds-nothing"] {
        let query = format!("?includeChannelId={channel}");
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} {body:?}"
        );
        let mut ids = listed_ids(&body);
        ids.sort();
        assert_eq!(
            ids,
            jobs_of(channel),
            "case {query:?}: expected {:?}, got {body:?}",
            jobs_of(channel)
        );
    }

    // A status filter alone: every entry carries the status asked for, and the transient
    // and terminal statuses between them account for every stored job, whichever side of
    // the transition the clock has taken each one past. These jobs name a bucket no object
    // store holds, so the harvest of each one writes nothing and the terminal status it
    // reaches is the failure one.
    let mut from_status: Vec<String> = Vec::new();
    for status_filter in [
        state::HARVEST_IN_PROGRESS,
        state::HARVEST_FAILED,
        "NOT_A_STATUS",
        "",
    ] {
        let query = format!("?includeStatus={status_filter}");
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} {body:?}"
        );
        for job in listed(&body) {
            assert_eq!(
                job.get("status").and_then(Value::as_str),
                Some(status_filter),
                "case {query:?}: every entry must carry the status asked for, got {job:?}"
            );
        }
        if status_filter == state::HARVEST_IN_PROGRESS || status_filter == state::HARVEST_FAILED {
            from_status.extend(listed_ids(&body));
        } else {
            assert!(
                listed(&body).is_empty(),
                "case {query:?}: no job carries that status, got {body:?}"
            );
        }
    }
    from_status.sort();
    from_status.dedup();
    assert_eq!(
        from_status,
        all_jobs(),
        "the transient and terminal statuses must account for every stored job, got \
         {from_status:?}"
    );
}
