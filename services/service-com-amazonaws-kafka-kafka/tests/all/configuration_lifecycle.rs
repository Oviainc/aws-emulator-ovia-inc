// SPDX-License-Identifier: CC0-1.0

//! The MSK configuration lifecycle (`CreateConfiguration`, `ListConfigurations`,
//! `DescribeConfiguration`), driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (description present
//! or absent, Kafka-version count, `server.properties` size and byte content,
//! configuration count, page size, signing region). Where the rule is an
//! invariant — paging returning every configuration exactly once, a read leaving
//! the registry as it found it, a page size outside the modelled range being
//! refused — the input space is small enough to walk EXHAUSTIVELY, which is
//! strictly stronger than sampling it.

use serde_json::{json, Value};

// The router harness, the signing region, the account id and the URI encoder
// every suite of this crate shares, so this one adds only its own rule and its
// configuration-shaped requests.
use super::kafka_harness::{pct, Emulator, Reply, ACCOUNT, REGION};

/// The page size `ListConfigurations` may be asked for, from the `range` on the
/// model's `MaxResults` shape (1..100). Both bounds are exercised, as is the
/// first value past each of them.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// `CreateConfiguration`, which the model binds to `POST /v1/configurations`.
async fn create(emulator: &Emulator, request: &ConfigurationRequest) -> Reply {
    create_in(emulator, REGION, request).await
}

/// The same create, as an endpoint for `region` serves it — which is where the
/// service must mint the ARN, and the only region that then lists it.
async fn create_in(emulator: &Emulator, region: &str, request: &ConfigurationRequest) -> Reply {
    emulator
        .call_in(region, "POST", "/v1/configurations", Some(request.to_json()))
        .await
}

/// `ListConfigurations`, which the model binds to `GET /v1/configurations`.
async fn list(emulator: &Emulator, query: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/configurations{query}"), None)
        .await
}

/// `DescribeConfiguration`, which the model binds to
/// `GET /v1/configurations/{Arn}`.
async fn describe(emulator: &Emulator, arn: &str) -> Reply {
    describe_in(emulator, REGION, arn).await
}

/// The same read, as an endpoint for `region` serves it.
async fn describe_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/v1/configurations/{}", pct(arn)),
            None,
        )
        .await
}

/// Create `request`, assert it succeeded, and hand back the minted ARN.
async fn create_ok(emulator: &Emulator, request: &ConfigurationRequest) -> String {
    let reply = create(emulator, request).await;
    assert_eq!(
        reply.status, 200,
        "case {request:?}: expected CreateConfiguration to succeed, got {} {}",
        reply.status, reply.body
    );
    reply.body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request:?}: CreateConfiguration returned no arn"))
        .to_string()
}

/// One `CreateConfiguration` request, as a row of a table.
#[derive(Debug, Clone)]
struct ConfigurationRequest {
    name: String,
    description: Option<String>,
    kafka_versions: Vec<String>,
    /// The `server.properties` body, as raw bytes: the member is a blob, so it
    /// is base64 on the wire and may hold bytes that are not UTF-8 at all.
    server_properties: Vec<u8>,
}

impl ConfigurationRequest {
    fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            kafka_versions: Vec::new(),
            server_properties: b"auto.create.topics.enable=true\n".to_vec(),
        }
    }

    fn to_json(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            // A blob member travels base64-encoded in a restJson1 payload.
            "serverProperties": ::aws_smithy_types::base64::encode(&self.server_properties),
        });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.kafka_versions.is_empty() {
            body["kafkaVersions"] = json!(self.kafka_versions);
        }
        body
    }

    /// What `Configuration$Description` must report: the description as given,
    /// and the empty string when the request named none.
    fn expected_description(&self) -> String {
        self.description.clone().unwrap_or_default()
    }
}

/// The table every round-trip suite below walks. The rows differ along what the
/// rules generalise over: whether a description was given (and whether it is
/// empty or carries non-ASCII text), how many Kafka versions were named, and the
/// size and byte content of the `server.properties` blob.
fn rows() -> Vec<ConfigurationRequest> {
    vec![
        ConfigurationRequest::named("no-description"),
        ConfigurationRequest {
            description: Some("a described configuration".to_string()),
            kafka_versions: vec!["2.8.1".to_string()],
            ..ConfigurationRequest::named("one-version")
        },
        ConfigurationRequest {
            description: Some(String::new()),
            kafka_versions: vec!["3.4.0".to_string(), "3.5.1".to_string(), "3.6.0".to_string()],
            server_properties: Vec::new(),
            ..ConfigurationRequest::named("three-versions-empty-properties")
        },
        ConfigurationRequest {
            description: Some("héllo — ünicode".to_string()),
            kafka_versions: vec!["3.6.0".to_string()],
            // Bytes that are not valid UTF-8: a blob is a byte string, so the
            // round trip must not go through a string conversion.
            server_properties: vec![0x00, 0xff, 0xfe, 0x10, 0x80],
            ..ConfigurationRequest::named("binary_properties-1")
        },
        ConfigurationRequest {
            description: Some("a large properties file".to_string()),
            server_properties: b"num.partitions=1\n".repeat(512),
            ..ConfigurationRequest::named("large-properties")
        },
    ]
}

/// `arn:aws:kafka:<region>:<account>:configuration/<name>/<uuid>-<n>` — the form
/// the real service mints a configuration ARN in.
fn assert_configuration_arn(arn: &str, case: &ConfigurationRequest) {
    let prefix = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/{}/",
        case.name
    );
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
    );
    let tail = &arn[prefix.len()..];
    let (uuid, number) = tail
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("case {case:?}: ARN tail {tail:?} carries no -<n> suffix"));
    assert!(
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a numeric ARN suffix, got {number:?} in {arn:?}"
    );
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a uuid-shaped id, got {uuid:?} in {arn:?}"
    );
    assert!(
        uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "case {case:?}: a uuid-shaped id is hex, got {uuid:?} in {arn:?}"
    );
}

/// The entry `ListConfigurations` reported for `arn`, or `None`.
fn entry_for<'a>(listed: &'a Value, arn: &str) -> Option<&'a Value> {
    listed
        .get("configurations")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("arn").and_then(Value::as_str) == Some(arn))
}

/// RULE: a created configuration is stored and every later `ListConfigurations`
/// reports it from that record — its own name, the description it was given (the
/// empty string when it was given none), the Kafka versions it named, revision 1
/// and state `ACTIVE` — with a distinct ARN of the modelled form.
#[tokio::test]
async fn a_created_configuration_is_reported_by_every_later_list() {
    let emulator = Emulator::new();
    let rows = rows();

    let mut created: Vec<(ConfigurationRequest, Value)> = Vec::new();
    for row in &rows {
        let reply = create(&emulator, row).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected CreateConfiguration to succeed, got {} {}",
            reply.status, reply.body
        );
        let arn = reply.body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: CreateConfiguration returned no arn"))
            .to_string();
        assert_configuration_arn(&arn, row);
        assert_eq!(
            reply.body["name"].as_str(),
            Some(row.name.as_str()),
            "case {row:?}: CreateConfiguration reported the wrong name: {}",
            reply.body
        );
        assert_eq!(
            reply.body["state"].as_str(),
            Some("ACTIVE"),
            "case {row:?}: a created configuration is usable at once, so it is ACTIVE: {}",
            reply.body
        );
        assert_eq!(
            reply.body["latestRevision"]["revision"].as_i64(),
            Some(1),
            "case {row:?}: the body submitted on create is revision 1: {}",
            reply.body
        );
        assert_eq!(
            reply.body["latestRevision"]["description"].as_str(),
            Some(row.expected_description().as_str()),
            "case {row:?}: the revision carries the request's description: {}",
            reply.body
        );
        let created_at = &reply.body["creationTime"];
        assert!(
            created_at.as_f64().is_some_and(|epoch| epoch > 0.0)
                || created_at.as_str().is_some_and(|text| text.contains('T')),
            "case {row:?}: expected a creationTime timestamp, got {}",
            reply.body
        );
        created.push((row.clone(), reply.body));
    }

    let arns: Vec<&str> = created
        .iter()
        .map(|(_, body)| body["arn"].as_str().unwrap_or_default())
        .collect();
    let mut distinct = arns.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "every configuration must get its own ARN, got {arns:?}"
    );

    let listed = list(&emulator, "").await;
    assert_eq!(
        listed.status, 200,
        "expected ListConfigurations to succeed, got {} {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["configurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        rows.len(),
        "expected every created configuration to be listed, got {}",
        listed.body
    );
    assert!(
        listed.body.get("nextToken").is_none(),
        "the last page must carry no nextToken, got {}",
        listed.body
    );

    for (row, create_body) in &created {
        let arn = create_body["arn"].as_str().expect("a created arn");
        let entry = entry_for(&listed.body, arn)
            .unwrap_or_else(|| panic!("case {row:?}: {arn} is missing from {}", listed.body));
        assert_eq!(
            entry["name"].as_str(),
            Some(row.name.as_str()),
            "case {row:?}: listed the wrong name: {entry}"
        );
        assert_eq!(
            entry["description"].as_str(),
            Some(row.expected_description().as_str()),
            "case {row:?}: listed the wrong description: {entry}"
        );
        assert_eq!(
            entry["kafkaVersions"],
            json!(row.kafka_versions),
            "case {row:?}: listed the wrong kafkaVersions: {entry}"
        );
        assert_eq!(
            entry["state"].as_str(),
            Some("ACTIVE"),
            "case {row:?}: listed the wrong state: {entry}"
        );
        assert_eq!(
            entry["latestRevision"]["revision"].as_i64(),
            Some(1),
            "case {row:?}: listed the wrong latest revision: {entry}"
        );
        assert_eq!(
            entry["latestRevision"]["description"].as_str(),
            Some(row.expected_description().as_str()),
            "case {row:?}: listed the wrong revision description: {entry}"
        );
        assert_eq!(
            entry["creationTime"], create_body["creationTime"],
            "case {row:?}: the creation time must be the stored one: {entry}"
        );
        assert_eq!(
            entry["latestRevision"]["creationTime"],
            create_body["latestRevision"]["creationTime"],
            "case {row:?}: the revision's creation time must be the stored one: {entry}"
        );
    }

    // A reporting operation leaves the registry exactly as it found it, so a
    // second read answers identically.
    let again = list(&emulator, "").await;
    assert_eq!(
        again.body, listed.body,
        "ListConfigurations must not change what a later caller sees"
    );

    // The two readers of one stored configuration must agree member for member.
    for (row, create_body) in &created {
        let arn = create_body["arn"].as_str().expect("a created arn");
        let described = describe(&emulator, arn).await;
        assert_eq!(
            described.status, 200,
            "case {row:?}: expected DescribeConfiguration to succeed, got {} {}",
            described.status, described.body
        );
        let listed_entry = entry_for(&listed.body, arn)
            .unwrap_or_else(|| panic!("case {row:?}: {arn} is missing from {}", listed.body));
        for member in [
            "arn",
            "name",
            "description",
            "kafkaVersions",
            "state",
            "creationTime",
            "latestRevision",
        ] {
            assert_eq!(
                described.body[member], listed_entry[member],
                "case {row:?}: DescribeConfiguration and ListConfigurations disagree \
                 about {member}: {} vs {listed_entry}",
                described.body
            );
        }
    }
}

/// RULE: a configuration ARN resolves only at the regional endpoint that holds
/// it — a value that is not an MSK configuration ARN is a bad request (400), and
/// a well-formed one this region holds no configuration for is not found (404).
#[tokio::test]
async fn an_unknown_or_malformed_configuration_arn_is_refused() {
    let emulator = Emulator::new();
    let arn = create_ok(&emulator, &ConfigurationRequest::named("present")).await;

    // `(arn, expected status, expected error code)`: a well-formed ARN of this
    // kind that names no stored configuration (including one in another
    // region), then values that are not configuration ARNs at all — not an ARN,
    // another service, another resource type, too many segments, and an empty
    // region field.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/absent/11111111-2222-4333-8444-555555555555-9"),
            404,
            "NotFoundException",
        ),
        (
            format!("arn:aws:kafka:eu-central-1:{ACCOUNT}:configuration/present/11111111-2222-4333-8444-555555555555-1"),
            404,
            "NotFoundException",
        ),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:configuration/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/present/abc/extra"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:configuration/present/abc-1"),
            400,
            "BadRequestException",
        ),
    ];

    for (candidate, status, code) in &rows {
        let described = describe(&emulator, candidate).await;
        assert_eq!(
            (described.status, described.error_code()),
            (*status, *code),
            "case arn={candidate:?}: expected {status} {code}, got {} {}",
            described.status,
            described.body
        );
    }

    // The configuration that does exist is still readable at its own endpoint,
    // and only there.
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described.status, 200,
        "the stored configuration must still resolve, got {} {}",
        described.status, described.body
    );
    let elsewhere = describe_in(&emulator, "eu-central-1", &arn).await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (404, "NotFoundException"),
        "case region=eu-central-1: another region's endpoint must not resolve it, got {} {}",
        elsewhere.status,
        elsewhere.body
    );
}

/// RULE: paging enumerates every stored configuration exactly once for any page
/// size the model allows, each page holds at most the size asked for, and only a
/// page that left something behind carries a `NextToken`.
#[tokio::test]
async fn paging_covers_every_configuration_exactly_once() {
    for count in 0..=5usize {
        let emulator = Emulator::new();
        let mut stored: Vec<String> = Vec::new();
        for index in 0..count {
            stored.push(
                create_ok(
                    &emulator,
                    &ConfigurationRequest::named(&format!("cfg-{index}")),
                )
                .await,
            );
        }
        // The registry is ordered by ARN, which is the order a page walks.
        stored.sort();

        for page_size in 1..=6usize {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case count={count} maxResults={page_size}: pagination did not end \
                     after {pages} pages"
                );
                let query = match &token {
                    Some(value) => {
                        format!("?maxResults={page_size}&nextToken={}", pct(value))
                    }
                    None => format!("?maxResults={page_size}"),
                };
                let reply = list(&emulator, &query).await;
                assert_eq!(
                    reply.status, 200,
                    "case count={count} maxResults={page_size}: expected 200, got {} {}",
                    reply.status, reply.body
                );
                let page = reply.body["configurations"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case count={count} maxResults={page_size}: page carried {} entries",
                    page.len()
                );
                for entry in &page {
                    seen.push(
                        entry["arn"]
                            .as_str()
                            .expect("every Configuration carries an arn")
                            .to_string(),
                    );
                }
                match reply.body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert!(
                            !page.is_empty(),
                            "case count={count} maxResults={page_size}: an empty page must \
                             close the list"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, stored,
                "case count={count} maxResults={page_size}: the pages did not cover every \
                 configuration exactly once"
            );
        }
    }
}

/// RULE: a `MaxResults` inside the modelled 1..100 range is served; one outside
/// it is refused with HTTP 400 and the operation's modelled client-input error,
/// never clamped to the nearest legal size.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    for index in 0..3 {
        create_ok(
            &emulator,
            &ConfigurationRequest::named(&format!("bounded-{index}")),
        )
        .await;
    }

    // Both published bounds, and the first value past each of them.
    for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MIN + 1, MAX_RESULTS_MAX] {
        let reply = list(&emulator, &format!("?maxResults={accepted}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={accepted}: a value inside 1..100 must be served, got {} {}",
            reply.status, reply.body
        );
        let page = reply.body["configurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert!(
            page <= usize::try_from(accepted).unwrap_or(usize::MAX),
            "case maxResults={accepted}: page carried {page} entries"
        );
    }

    for refused in [
        MAX_RESULTS_MIN - 1,
        -1,
        MAX_RESULTS_MAX + 1,
        1_000,
        i64::from(i32::MAX),
    ] {
        let reply = list(&emulator, &format!("?maxResults={refused}")).await;
        assert_eq!(
            reply.status, 400,
            "case maxResults={refused}: a value outside 1..100 must be refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case maxResults={refused}: expected the modelled BadRequestException, got {}",
            reply.body
        );
    }

    // None of those refusals disturbed the registry.
    let listed = list(&emulator, "").await;
    assert_eq!(
        listed.body["configurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        3,
        "a refused page size must leave the registry alone, got {}",
        listed.body
    );
}

/// RULE: a request the model itself refuses — an absent required member, or a
/// value outside a declared bound — is answered with the error code this service
/// models for a bad request, `BadRequestException` (HTTP 400), for every
/// operation: MSK's model declares no `ValidationException` anywhere.
#[tokio::test]
async fn a_request_the_model_refuses_is_a_bad_request() {
    let emulator = Emulator::new();

    // Rows spanning the violation classes the framework can report and the
    // operations they arrive on: a value below a declared minimum, one above the
    // declared maximum, an absent required member (each of the two
    // `CreateConfiguration` requires), and an absent required member of another
    // operation entirely.
    let rows: Vec<(&str, &str, &str, Option<Value>)> = vec![
        (
            "maxResults below the minimum",
            "GET",
            "/v1/configurations?maxResults=0",
            None,
        ),
        (
            "maxResults above the maximum",
            "GET",
            "/v1/configurations?maxResults=101",
            None,
        ),
        (
            "CreateConfiguration without a name",
            "POST",
            "/v1/configurations",
            Some(json!({ "serverProperties": "" })),
        ),
        (
            "CreateConfiguration without serverProperties",
            "POST",
            "/v1/configurations",
            Some(json!({ "name": "no-properties" })),
        ),
        (
            "CreateCluster without its required members",
            "POST",
            "/v1/clusters",
            Some(json!({ "clusterName": "incomplete" })),
        ),
    ];

    for (case, method, uri, body) in rows {
        let reply = emulator.call(method, uri, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {case}: expected the modelled BadRequestException, got {}",
            reply.body
        );
        assert!(
            reply.body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case}: a refusal must say what failed, got {}",
            reply.body
        );
    }
}

/// RULE: a `NextToken` this operation did not mint is refused with HTTP 400 and
/// the modelled `BadRequestException` — including one minted by another list of
/// the same service.
#[tokio::test]
async fn a_next_token_this_list_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    for index in 0..2 {
        create_ok(
            &emulator,
            &ConfigurationRequest::named(&format!("tokened-{index}")),
        )
        .await;
    }

    // Rows: not base64 at all, valid base64 of unrelated bytes, the empty
    // token, and base64 of another list's marked plaintext.
    let foreign = ::aws_smithy_types::base64::encode("kafka:clusters:arn:aws:kafka:x");
    for token in [
        "!!!not-base64!!!",
        "aGVsbG8gd29ybGQ=",
        "%20",
        foreign.as_str(),
    ] {
        let reply = list(&emulator, &format!("?nextToken={}", pct(token))).await;
        assert_eq!(
            reply.status, 400,
            "case token={token:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case token={token:?}: expected BadRequestException, got {}",
            reply.body
        );
    }
}

/// RULE: a configuration name identifies a configuration within an account and
/// region — a repeat in the same region is a conflict (HTTP 409), the same name
/// in another region is a different configuration, and each region's list
/// reports only its own.
#[tokio::test]
async fn a_configuration_name_is_unique_per_region() {
    let emulator = Emulator::new();
    // Rows differing in what else the repeat submits, so the conflict turns on
    // the name alone.
    let first = ConfigurationRequest::named("shared-name");
    let arn = create_ok(&emulator, &first).await;

    let repeats = [
        first.clone(),
        ConfigurationRequest {
            description: Some("a different description".to_string()),
            kafka_versions: vec!["3.6.0".to_string()],
            server_properties: b"log.retention.hours=48\n".to_vec(),
            ..first.clone()
        },
    ];
    for repeat in &repeats {
        let reply = create(&emulator, repeat).await;
        assert_eq!(
            reply.status, 409,
            "case {repeat:?}: a repeated name must conflict, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {repeat:?}: expected ConflictException, got {}",
            reply.body
        );
    }

    // The same name in another region is a different configuration, minted in
    // that region.
    let other_region = "eu-central-1";
    let elsewhere = create_in(&emulator, other_region, &first).await;
    assert_eq!(
        elsewhere.status, 200,
        "case region={other_region}: the same name in another region is a different \
         configuration, got {} {}",
        elsewhere.status, elsewhere.body
    );
    let other_arn = elsewhere.body["arn"]
        .as_str()
        .expect("a created arn")
        .to_string();
    assert!(
        other_arn.contains(&format!(":{other_region}:")),
        "case region={other_region}: expected the ARN minted in that region, got {other_arn:?}"
    );

    // Each region lists its own configuration and only that one.
    for (region, expected) in [(REGION, arn.as_str()), (other_region, other_arn.as_str())] {
        let listed = emulator
            .call_in(region, "GET", "/v1/configurations", None)
            .await;
        let page = listed.body["configurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            page.len(),
            1,
            "case region={region}: expected exactly its own configuration, got {}",
            listed.body
        );
        assert_eq!(
            page[0]["arn"].as_str(),
            Some(expected),
            "case region={region}: listed another region's configuration: {}",
            listed.body
        );
    }
}

/// RULE: `Name` is the configuration's identity, so a blank one names no
/// resource and is refused with HTTP 400, registering nothing.
#[tokio::test]
async fn a_blank_configuration_name_is_refused() {
    let emulator = Emulator::new();
    for name in ["", " ", "\t", "   "] {
        let reply = create(&emulator, &ConfigurationRequest::named(name)).await;
        assert_eq!(
            reply.status, 400,
            "case name={name:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case name={name:?}: expected BadRequestException, got {}",
            reply.body
        );
    }

    let listed = list(&emulator, "").await;
    assert_eq!(
        listed.body["configurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "a refused create must register nothing, got {}",
        listed.body
    );
}
