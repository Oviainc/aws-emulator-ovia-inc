// SPDX-License-Identifier: CC0-1.0

//! `ListHostedConfigurationVersions` as a REPORT over stored state: it answers one
//! summary per hosted configuration version the addressed configuration profile holds,
//! newest version first, cut into pages a `NextToken` walks exactly once.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The rules
//! under test, each asserted over a SET of inputs rather than one recorded call:
//!
//! 1. a summary reports the stored version — `ApplicationId`, `ConfigurationProfileId`,
//!    `VersionNumber`, `Description`, `ContentType` and `VersionLabel` exactly as
//!    `GetHostedConfigurationVersion` reports them — and carries no `Content`;
//! 2. a listing reports exactly the versions of the profile it addresses, in descending
//!    version order;
//! 3. `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports
//!    every version exactly once, still newest first, at any page size;
//! 4. `VersionLabel` filters the listing: a trailing asterisk makes the rest a prefix,
//!    and anything else is an exact match;
//! 5. an application or a configuration profile that does not exist is 404
//!    `ResourceNotFoundException`, and a `NextToken` this operation did not issue is 400
//!    `BadRequestException`;
//! 6. listing is a read — it changes nothing a later caller sees, and it reports what the
//!    writes and deletes before it left.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{
    create_application, create_profile, create_profile_from, router, send,
    with_required_retrieval_role, Answer,
};

/// The published bounds of `MaxResults` on this operation (`range 1..50`,
/// `smithy_query(query="operation",operation="ListHostedConfigurationVersions")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 50;

/// Where the hosted configuration versions of one profile live, from this operation's
/// modelled URI
/// (`smithy_query(query="operation",operation="ListHostedConfigurationVersions")`).
fn hosted_uri(application: &str, profile: &str) -> String {
    format!("/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions")
}

/// One payload a hosted configuration version is written from: the members
/// `CreateHostedConfigurationVersion` carries beside the content itself.
struct Write {
    content_type: &'static str,
    content: Vec<u8>,
    description: Option<String>,
    /// Must contain a non-digit character: the modelled `VersionLabel` pattern is
    /// `[^0-9]` (`smithy_query(query="shape",shape="VersionLabel")`).
    label: Option<&'static str>,
}

/// `POST` one hosted configuration version and answer the version number it was given.
async fn create_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    write: &Write,
) -> i32 {
    // `Description` and `VersionLabel` are `@httpHeader` members of this write
    // (`smithy_query(query="operation",operation="CreateHostedConfigurationVersion")`).
    let mut headers: Vec<(&str, String)> = Vec::new();
    if let Some(description) = &write.description {
        headers.push(("Description", description.clone()));
    }
    if let Some(label) = write.label {
        headers.push(("VersionLabel", label.to_string()));
    }
    let created = send(
        router,
        "POST",
        &hosted_uri(application, profile),
        write.content_type,
        write.content.clone(),
        &headers,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateHostedConfigurationVersion answered no Version-Number")
        })
        .parse()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Version-Number must be an integer, got {error}")
        })
}

/// `GET` the listing of one profile, with the query members in `query`.
async fn list(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    query: &[(&str, String)],
) -> Answer {
    let mut uri = hosted_uri(application, profile);
    if !query.is_empty() {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        uri.push('?');
        uri.push_str(&rendered.join("&"));
    }
    send(router, "GET", &uri, "application/json", Vec::new(), &[]).await
}

/// The `Items` of a successful listing.
fn items(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListHostedConfigurationVersions expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Items array in {}", answer.json()))
}

/// The `VersionNumber` of every summary in `page`.
fn numbers(page: &[Value], case: &str) -> Vec<i64> {
    page.iter()
        .map(|summary| {
            summary
                .get("VersionNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary carries no VersionNumber: {summary}")
                })
        })
        .collect()
}

/// The table of writes every suite here builds its versions from. Rows differ along the
/// content type, the byte values and the size of the content, whether a `Description` was
/// given, and whether a `VersionLabel` was — which is what the rules generalise over.
fn writes() -> Vec<Write> {
    vec![
        Write {
            content_type: "application/json",
            content: br#"{"enabled":true}"#.to_vec(),
            description: Some("the first revision".to_string()),
            label: Some("v1-alpha"),
        },
        Write {
            content_type: "application/x-yaml",
            content: b"enabled: true\nlimit: 7\n".to_vec(),
            description: None,
            label: Some("v2-beta"),
        },
        Write {
            content_type: "text/plain",
            content: vec![b'x'],
            // `Description` is bounded at 1..1024 characters
            // (`smithy_query(query="shape",shape="Description")`), so this row sits on
            // the lower end of that published range.
            description: Some("d".to_string()),
            label: None,
        },
        Write {
            content_type: "application/octet-stream",
            content: vec![0, 1, 2, 255, 0, 128],
            description: Some("bytes no text encoding carries".to_string()),
            label: Some("release-candidate"),
        },
        Write {
            content_type: "text/plain",
            content: vec![b'a'; 64 * 1024],
            description: None,
            label: None,
        },
        Write {
            content_type: "application/json",
            content: br#"{"enabled":false,"limit":0}"#.to_vec(),
            // The published 1024-character maximum of the modelled `Description` shape.
            description: Some("d".repeat(1024)),
            label: Some("v10-final"),
        },
    ]
}

/// RULE 1: each summary reports the stored version, exactly as the reader does, and
/// carries no `Content`.
#[tokio::test]
async fn a_summary_reports_the_stored_version_as_the_reader_does() {
    let router = router();
    let application = create_application(&router, "summaries").await;
    let profile = create_profile(&router, &application, "summaries").await;

    let table = writes();
    let mut written: Vec<(String, i32)> = Vec::new();
    for (index, write) in table.iter().enumerate() {
        let case = format!("write {index}");
        written.push((
            case.clone(),
            create_version(&router, &application, &profile, &case, write).await,
        ));
    }

    let listed = list(&router, &application, &profile, &[]).await;
    let page = items(&listed, "every version");
    assert_eq!(
        page.len(),
        table.len(),
        "case \"every version\": expected {} summaries, got {}: {}",
        table.len(),
        page.len(),
        listed.json()
    );

    for ((case, number), write) in written.iter().zip(table.iter()) {
        let summary = page
            .iter()
            .find(|summary| summary.get("VersionNumber").and_then(Value::as_i64) == Some(i64::from(*number)))
            .unwrap_or_else(|| {
                panic!("case {case:?}: no summary for version {number}: {}", listed.json())
            });

        // The reader is the other document describing this same version: whatever it
        // reports for a member the summary also carries must match, so the summary is a
        // report over state rather than values made up at listing time.
        let read = send(
            &router,
            "GET",
            &format!("{}/{number}", hosted_uri(&application, &profile)),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetHostedConfigurationVersion must answer 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );

        let mut expected = serde_json::json!({
            "ApplicationId": application,
            "ConfigurationProfileId": profile,
            "VersionNumber": number,
            "ContentType": write.content_type,
        });
        if let Some(description) = &write.description {
            expected["Description"] = Value::from(description.clone());
        }
        if let Some(label) = write.label {
            expected["VersionLabel"] = Value::from(label);
        }
        assert_eq!(
            summary, &expected,
            "case {case:?}: expected {expected}, got {summary}"
        );

        // Every member the reader answers on the wire agrees with the summary.
        assert_eq!(
            read.header("Content-Type").as_deref(),
            Some(write.content_type),
            "case {case:?}: the reader must report ContentType {:?}, got {:?}",
            write.content_type,
            read.header("Content-Type")
        );
        assert_eq!(
            read.header("VersionLabel"),
            write.label.map(str::to_string),
            "case {case:?}: the reader must report VersionLabel {:?}, got {:?}",
            write.label,
            read.header("VersionLabel")
        );
        assert_eq!(
            read.body.as_ref(),
            write.content.as_slice(),
            "case {case:?}: the reader must answer the content byte for byte, got {} bytes instead of {}",
            read.body.len(),
            write.content.len()
        );

        // `HostedConfigurationVersionSummary` models no `Content`: the listing is a
        // catalog, and the bytes are the reader's answer.
        assert!(
            summary.get("Content").is_none(),
            "case {case:?}: a summary carries no Content, got {summary}"
        );
    }
}

/// RULE 2: a listing reports exactly the versions of the profile it addresses, newest
/// first — and RULE 6: it is a read, so repeating it answers the same thing.
#[tokio::test]
async fn a_listing_reports_only_that_profiles_versions_newest_first() {
    let router = router();
    let table = writes();

    // A sibling application holding its own profile and versions, so a listing that read
    // the whole store rather than one profile's subtree fails here.
    let sibling_application = create_application(&router, "sibling-app").await;
    let sibling_profile = create_profile(&router, &sibling_application, "sibling").await;
    for write in table.iter().take(2) {
        create_version(&router, &sibling_application, &sibling_profile, "sibling", write).await;
    }

    // Rows differ along how many versions the profile holds: none at all, one, and
    // several.
    let application = create_application(&router, "listings").await;
    for count in [0_usize, 1, 2, 6] {
        let case = format!("{count} versions");
        let profile = create_profile(&router, &application, &case).await;
        let mut expected: Vec<i64> = Vec::new();
        for write in table.iter().take(count) {
            expected.push(i64::from(
                create_version(&router, &application, &profile, &case, write).await,
            ));
        }
        // AppConfig lists a profile's hosted configuration versions newest first, so the
        // numbers come back in descending order.
        expected.sort_unstable();
        expected.reverse();

        let answer = list(&router, &application, &profile, &[]).await;
        let page = items(&answer, &case);
        assert_eq!(
            numbers(&page, &case),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            numbers(&page, &case)
        );
        assert!(
            answer.json().get("NextToken").is_none(),
            "case {case:?}: a listing that fits one page carries no NextToken, got {}",
            answer.json()
        );
        for summary in &page {
            assert_eq!(
                summary.get("ConfigurationProfileId").and_then(Value::as_str),
                Some(profile.as_str()),
                "case {case:?}: every summary must name the addressed profile, got {summary}"
            );
        }

        // A read changes nothing: the same call answers the same thing.
        let again = list(&router, &application, &profile, &[]).await;
        assert_eq!(
            again.json(),
            answer.json(),
            "case {case:?}: listing twice must answer the same, got {}",
            again.json()
        );
    }

    // The sibling is untouched by every listing above, and still reports only its own
    // two versions.
    let case = "sibling application";
    let answer = list(&router, &sibling_application, &sibling_profile, &[]).await;
    let page = items(&answer, case);
    assert_eq!(
        numbers(&page, case),
        vec![2, 1],
        "case {case:?}: expected the sibling's own versions newest first, got {}",
        answer.json()
    );

    // A profile whose configuration lives OUTSIDE AppConfig's hosted store holds no
    // hosted version, so its listing is empty rather than refused.
    let case = "ssm-parameter backed profile";
    let external = create_profile_from(
        &router,
        &application,
        case,
        &with_required_retrieval_role(serde_json::json!({
            "Name": "external",
            "LocationUri": "ssm-parameter://my-parameter",
        })),
    )
    .await
    .string("Id")
    .expect("a created profile carries an Id");
    let answer = list(&router, &application, &external, &[]).await;
    let page = items(&answer, case);
    assert!(
        page.is_empty(),
        "case {case:?}: expected no summaries, got {}",
        answer.json()
    );
}

/// RULE 3: `MaxResults` bounds a page, `NextToken` resumes after it, and the walk reports
/// every version exactly once and still newest first — at every page size, including the
/// published bounds of `MaxResults`.
#[tokio::test]
async fn a_next_token_walk_reports_every_version_exactly_once_newest_first() {
    let router = router();
    let application = create_application(&router, "walks").await;
    let table = writes();

    // Rows span the version count and, for each, every page size from the published
    // minimum up past the count, plus the published maximum and no `MaxResults` at all.
    for count in [0_usize, 1, 2, 5, 6] {
        let profile = create_profile(&router, &application, &format!("walk-{count}")).await;
        let mut expected: Vec<i64> = Vec::new();
        for write in table.iter().take(count) {
            expected.push(i64::from(
                create_version(&router, &application, &profile, "walk", write).await,
            ));
        }
        expected.sort_unstable();
        expected.reverse();

        let mut page_sizes: Vec<Option<i64>> =
            (MAX_RESULTS_MIN..=(count as i64 + 1)).map(Some).collect();
        page_sizes.push(Some(MAX_RESULTS_MAX));
        page_sizes.push(None);

        for page_size in page_sizes {
            let case = format!("{count} versions, MaxResults {page_size:?}");
            let mut seen: Vec<i64> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query: Vec<(&str, String)> = Vec::new();
                if let Some(page_size) = page_size {
                    query.push(("max_results", page_size.to_string()));
                }
                if let Some(token) = &next_token {
                    query.push(("next_token", token.clone()));
                }
                let answer = list(&router, &application, &profile, &query).await;
                let page = items(&answer, &case);
                if let Some(page_size) = page_size {
                    assert!(
                        page.len() as i64 <= page_size,
                        "case {case:?}: a page must carry at most {page_size} items, got {}",
                        page.len()
                    );
                }
                seen.extend(numbers(&page, &case));
                pages += 1;
                assert!(
                    pages <= count + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                next_token = answer.string("NextToken");
                if next_token.is_none() {
                    break;
                }
                assert!(
                    !page.is_empty(),
                    "case {case:?}: a page that resumes must carry items, got {}",
                    answer.json()
                );
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: the walk must report every version exactly once newest first, expected {expected:?}, got {seen:?}"
            );
        }
    }
}

/// RULE 4: `VersionLabel` filters the listing — a trailing asterisk makes the rest of the
/// value a prefix, anything else is compared whole, and a version written without a label
/// is selected by no filter at all.
#[tokio::test]
async fn the_version_label_filter_matches_exactly_or_by_prefix() {
    let router = router();
    let application = create_application(&router, "labels").await;
    let profile = create_profile(&router, &application, "labels").await;

    // Labels that share prefixes at several depths, plus two versions carrying none, so a
    // prefix row and an exact row select different sets. Written in this order, so version
    // 1 carries the first label, version 2 the second, and so on.
    let labelled: Vec<Option<&str>> = vec![
        Some("v2-alpha"),
        Some("v2-beta"),
        Some("v20-release"),
        Some("v3-alpha"),
        None,
        Some("alpha"),
        None,
    ];
    for (index, label) in labelled.iter().enumerate() {
        let write = Write {
            content_type: "application/json",
            content: format!(r#"{{"n":{index}}}"#).into_bytes(),
            description: None,
            label: *label,
        };
        create_version(&router, &application, &profile, "labels", &write).await;
    }

    // Rows differ along the form of the filter: an exact label, an exact label that is a
    // prefix of another, a prefix at two depths, the bare asterisk, a prefix nothing
    // matches, and a label nothing holds. Each expected set is written out BY HAND from
    // the documented rule — "a trailing asterisk makes the rest a prefix, anything else is
    // an exact match" — newest version first, so the check does not share an expression
    // with the handler.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("exact label", "v2-alpha", vec!["v2-alpha"]),
        ("exact label that prefixes another", "v2", vec![]),
        ("prefix of two labels", "v2-*", vec!["v2-beta", "v2-alpha"]),
        (
            "prefix of three labels",
            "v2*",
            vec!["v20-release", "v2-beta", "v2-alpha"],
        ),
        ("prefix of one label", "v3*", vec!["v3-alpha"]),
        (
            "prefix matching every label",
            "*",
            vec![
                "alpha",
                "v3-alpha",
                "v20-release",
                "v2-beta",
                "v2-alpha",
            ],
        ),
        ("prefix nothing matches", "zz*", vec![]),
        ("label nothing holds", "v9-final", vec![]),
    ];
    for (case, filter, expected) in cases {
        let answer = list(
            &router,
            &application,
            &profile,
            &[("version_label", filter.to_string())],
        )
        .await;
        let page = items(&answer, case);
        let reported: Vec<String> = page
            .iter()
            .map(|summary| {
                summary
                    .get("VersionLabel")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a filtered summary must carry the label it matched, got {summary}"
                        )
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: filter {filter:?} expected {expected:?}, got {reported:?}"
        );

        // The filtered page is still newest first.
        let mut descending = numbers(&page, case);
        descending.sort_unstable_by(|left, right| right.cmp(left));
        assert_eq!(
            numbers(&page, case),
            descending,
            "case {case:?}: a filtered page must still be newest first, got {:?}",
            numbers(&page, case)
        );
    }

    // Without the filter every version is reported, labelled or not — so the rows above
    // narrow the listing rather than the store holding only labelled versions.
    let case = "no filter";
    let answer = list(&router, &application, &profile, &[]).await;
    let page = items(&answer, case);
    assert_eq!(
        page.len(),
        labelled.len(),
        "case {case:?}: expected {} summaries, got {}",
        labelled.len(),
        page.len()
    );

    // The filter is a filter, not a bound: it changes nothing a later unfiltered caller
    // sees, including the next version number.
    let next = create_version(
        &router,
        &application,
        &profile,
        case,
        &Write {
            content_type: "application/json",
            content: br#"{"after":"filtering"}"#.to_vec(),
            description: None,
            label: Some("after-filtering"),
        },
    )
    .await;
    let after = list(&router, &application, &profile, &[]).await;
    let page = items(&after, case);
    assert_eq!(
        numbers(&page, case).first().copied(),
        Some(i64::from(next)),
        "case {case:?}: the newest version must head the listing, got {}",
        after.json()
    );
}

/// RULE 5a: a listing is addressed under its application and profile, so an unresolvable
/// level of that path is 404 `ResourceNotFoundException` — never an empty listing.
#[tokio::test]
async fn listing_under_a_path_the_store_does_not_hold_is_not_found() {
    let router = router();
    let application = create_application(&router, "misses").await;
    let profile = create_profile(&router, &application, "held").await;
    let held = create_version(
        &router,
        &application,
        &profile,
        "held",
        &writes()[0],
    )
    .await;

    // Rows differ along which level of the path is unresolvable and along the shape of the
    // id: an id no resource holds, a near miss of a real one, the id of the wrong kind of
    // resource, and a single character.
    let near_miss_application = format!("{application}x");
    let near_miss_profile = format!("{profile}x");
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an application id no resource holds",
            "nosuchapp".to_string(),
            profile.clone(),
        ),
        (
            "a near miss of a real application",
            near_miss_application,
            profile.clone(),
        ),
        (
            "the id of a profile where an application goes",
            profile.clone(),
            profile.clone(),
        ),
        (
            "a single-character application id",
            "a".to_string(),
            profile.clone(),
        ),
        (
            "a profile id no resource holds",
            application.clone(),
            "nosuchprofile".to_string(),
        ),
        (
            "a near miss of a real profile",
            application.clone(),
            near_miss_profile,
        ),
        (
            "the id of an application where a profile goes",
            application.clone(),
            application.clone(),
        ),
    ];
    for (case, target_application, target_profile) in &cases {
        let answer = list(&router, target_application, target_profile, &[]).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        assert!(
            answer
                .string("Message")
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must carry a Message, got {}",
            answer.json()
        );
    }

    // RULE 6: every refused listing above left the store alone.
    let case = "the profile that exists";
    let answer = list(&router, &application, &profile, &[]).await;
    let page = items(&answer, case);
    assert_eq!(
        numbers(&page, case),
        vec![i64::from(held)],
        "case {case:?}: expected the one written version, got {}",
        answer.json()
    );
}

/// RULE 5b: a `MaxResults` outside the published range is refused by the modelled
/// constraint rather than answered.
#[tokio::test]
async fn a_max_results_past_the_published_bounds_is_refused() {
    let router = router();
    let application = create_application(&router, "bad-max").await;
    let profile = create_profile(&router, &application, "bad-max").await;
    create_version(&router, &application, &profile, "bad-max", &writes()[0]).await;

    // One past each published bound, and a value that is not a number at all.
    let cases: [(&str, &str); 3] = [
        ("one below the minimum", "0"),
        ("one above the maximum", "51"),
        ("not a number", "many"),
    ];
    for (case, max_results) in cases {
        let answer = list(
            &router,
            &application,
            &profile,
            &[("max_results", max_results.to_string())],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for MaxResults={max_results}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // Each published bound itself is accepted.
    for bound in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let case = format!("MaxResults at the bound {bound}");
        let answer = list(
            &router,
            &application,
            &profile,
            &[("max_results", bound.to_string())],
        )
        .await;
        let page = items(&answer, &case);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one written version, got {}",
            answer.json()
        );
    }
}

/// RULE 5c: a `NextToken` this operation did not issue is 400 `BadRequestException`, and
/// one it did issue resumes the walk it was minted for.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_a_bad_request() {
    let router = router();
    let application = create_application(&router, "bad-tokens").await;
    let profile = create_profile(&router, &application, "bad-tokens").await;
    for write in writes().iter().take(3) {
        create_version(&router, &application, &profile, "bad-tokens", write).await;
    }

    // A token this operation DID issue is accepted, so the rows below are refused for
    // being unminted rather than for the operation refusing every token.
    let first = list(
        &router,
        &application,
        &profile,
        &[("max_results", "1".to_string())],
    )
    .await;
    let issued = first
        .string("NextToken")
        .expect("three versions at MaxResults=1 must leave a NextToken");
    let resumed = list(
        &router,
        &application,
        &profile,
        &[("next_token", issued)],
    )
    .await;
    let page = items(&resumed, "an issued token");
    assert_eq!(
        numbers(&page, "an issued token"),
        vec![2, 1],
        "case \"an issued token\": expected the 2 remaining versions, got {}",
        resumed.json()
    );

    // Rows differ along HOW the token fails to be one this operation minted: not the
    // codec's alphabet at all, the right alphabet at the wrong length, the codec but not
    // the payload shape, and the payload shape under another listing's name.
    let cases: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex of an odd length", "abc".to_string()),
        ("hex with no listing kind in it", hex::encode("abcd")),
        (
            "hex naming another listing",
            hex::encode("ListConfigurationProfiles\u{1f}abcd"),
        ),
        (
            "hex naming this listing with no key at all",
            hex::encode("ListHostedConfigurationVersions"),
        ),
    ];
    for (case, token) in &cases {
        let answer = list(
            &router,
            &application,
            &profile,
            &[("next_token", token.clone())],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.error_code()
        );
    }
}

/// RULE 6: the listing reports what the writes and deletes before it left — a deleted
/// version leaves it, and the number it freed never comes back.
#[tokio::test]
async fn the_listing_reports_what_the_writes_and_deletes_before_it_left() {
    let router = router();
    let application = create_application(&router, "deletes").await;
    let table = writes();

    // Rows differ along which version of the profile is deleted: the only one, the
    // newest, the oldest, and one in the middle.
    let cases: Vec<(&str, usize, usize)> = vec![
        ("only version", 1, 1),
        ("newest of four", 4, 4),
        ("oldest of four", 4, 1),
        ("middle of four", 4, 2),
    ];
    for (case, count, delete_position) in cases {
        let profile = create_profile(&router, &application, case).await;
        let mut written: Vec<i64> = Vec::new();
        for write in table.iter().take(count) {
            written.push(i64::from(
                create_version(&router, &application, &profile, case, write).await,
            ));
        }

        let doomed = written[delete_position - 1];
        let deleted = send(
            &router,
            "DELETE",
            &format!("{}/{doomed}", hosted_uri(&application, &profile)),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: DeleteHostedConfigurationVersion must answer 204, got {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );

        let mut expected: Vec<i64> = written.iter().copied().filter(|n| *n != doomed).collect();
        expected.sort_unstable();
        expected.reverse();

        let answer = list(&router, &application, &profile, &[]).await;
        let page = items(&answer, case);
        assert_eq!(
            numbers(&page, case),
            expected,
            "case {case:?}: after deleting {doomed} expected {expected:?}, got {:?}",
            numbers(&page, case)
        );

        // The next write heads the listing with a number past every one ever assigned to
        // this profile, so the deleted number cannot reappear.
        let next = create_version(&router, &application, &profile, case, &table[0]).await;
        let answer = list(&router, &application, &profile, &[]).await;
        let page = items(&answer, case);
        assert_eq!(
            numbers(&page, case).first().copied(),
            Some(i64::from(next)),
            "case {case:?}: the newest version must head the listing, got {}",
            answer.json()
        );
        assert!(
            i64::from(next) > written.iter().copied().max().expect("each row writes a version"),
            "case {case:?}: the write after a delete must be numbered past {written:?}, got {next}"
        );
    }
}
