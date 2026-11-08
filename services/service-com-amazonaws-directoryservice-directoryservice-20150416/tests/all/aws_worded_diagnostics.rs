// SPDX-License-Identifier: CC0-1.0

//! The diagnostics and reported encodings a client READS — asserted as rules over tables,
//! driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the shared harness, `crate::harness`). Nothing
//! here reaches into the state struct, so a value that is not computed from persisted state
//! cannot pass.
//!
//! The rules under test:
//!
//! * a refusal whose text is FIXED states the rule and quotes nothing of the request, so
//!   the same sentence answers every caller that breaks it: the Region-replication rule
//!   that a directory's own home Region is not a replication (`AddRegion`), and the
//!   placement rule that a directory's servers sit in two subnets in two Availability Zones
//!   (every create that places servers in a VPC);
//! * a refusal that reports the VALUES of a list filter names them in the order the filter
//!   sent them, separated by a bare comma, so splitting the message on `,` recovers the ids
//!   the caller has to fix (`DescribeDirectories`);
//! * `DirectoryDescription$AccessUrl` is the HOST the directory's alias names — no scheme,
//!   no path — whichever create made the directory and whether or not an alias was
//!   assigned;
//! * `SettingEntry$AllowedValues` carries the values a setting admits as a JSON array of
//!   strings, so a client enumerates them by decoding the member.

use serde_json::{json, Value};

use crate::ad_connector::connect_body;
use crate::directory_lifecycle::{
    create_directory, describe_one, minimal_create, PAST_ONE_SPAN, REGION,
};
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The FIXED sentence `AddRegion` refuses the directory's own home Region with. Quotes
/// neither the Region nor the directory, so a client can match on it (state.rs states the
/// same text once, as the service's shared diagnostic).
const HOME_REGION_REFUSAL: &str = "The region you specified is the same region where the \
                                   AWS Managed Microsoft AD directory was created. Specify \
                                   a different region and try again.";

/// The FIXED sentence every create that places directory servers in a VPC refuses a
/// settings block whose subnets cannot carry them with.
const SUBNET_PAIR_REFUSAL: &str = "Invalid subnet ID(s). They must correspond to two \
                                   subnets in different Availability Zones.";

/// The two subnets a servable settings block names, in one VPC.
const SERVABLE_SUBNETS: [&str; 2] = ["subnet-0a1b2c3d", "subnet-1a2b3c4d"];

/// The VPC every settings block below places its servers in.
const VPC_ID: &str = "vpc-0a1b2c3d";

/// The DNS suffix an access URL is built on, from `AccessUrl`'s own documentation.
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// Well-formed `DirectoryId`s (`^d-[0-9a-f]{10}$`) no create here ever mints, varied along
/// their digits so no row can pass by matching one recorded id.
const ABSENT_IDS: [&str; 4] = [
    "d-2ee16dd93c",
    "d-52de831cee",
    "d-0000000000",
    "d-ffffffffff",
];

/// The `Message` of a refusal body, and the `RequestId` it reports.
fn refusal(parsed: &Value, case: &str) -> (String, String) {
    let message = parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {parsed}"));
    let request_id = parsed["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RequestId, got {parsed}"));
    (message.to_string(), request_id.to_string())
}

/// This service's refusal envelope: the diagnostic, then the call's own request id.
fn with_request_id(diagnostic: &str, request_id: &str) -> String {
    format!("{diagnostic} : RequestId: {request_id}")
}

/// A `VpcSettings`/`VPCSettings` block for `subnet_ids`.
fn vpc_settings(subnet_ids: &[&str]) -> Value {
    json!({ "VpcId": VPC_ID, "SubnetIds": subnet_ids })
}

/// RULE: a directory's own home Region is not a replication, and Directory Service refuses
/// an add naming it with one FIXED sentence — it quotes neither the Region nor the
/// directory id, so every caller that breaks the rule reads the same text.
#[tokio::test]
async fn the_home_region_add_is_refused_with_one_fixed_sentence() {
    let router = service();

    // Rows differing in everything the refusal could have interpolated: the directory the
    // request names (a different id per row), whether it carries an alias the description
    // reports, and the placement the add asks for. The Region is the home one in every row
    // because that is the rule, and every row is a directory that CAN replicate (Region
    // replication is an Enterprise-edition feature), so the home-Region guard is what
    // refuses.
    let rows: [(&str, Option<&str>, &[&str]); 3] = [
        ("a directory with no alias", None, &SERVABLE_SUBNETS),
        (
            "a directory carrying an alias",
            Some("home-region-row"),
            &SERVABLE_SUBNETS,
        ),
        (
            "an add placing its servers in three subnets",
            None,
            &["subnet-aaaaaaaa", "subnet-bbbbbbbb", "subnet-cccccccc"],
        ),
    ];

    let mut stated: Vec<String> = Vec::new();
    for (index, (case, alias, subnets)) in rows.iter().enumerate() {
        let directory = create_microsoft_ad(
            &router,
            minimal_microsoft_ad(&format!("home{index}.example.com")),
            case,
        )
        .await;
        if let Some(alias) = alias {
            // An alias is assigned only once the directory has reached its serving stage.
            tokio::time::sleep(PAST_ONE_SPAN).await;
            ok(
                &router,
                "CreateAlias",
                json!({ "DirectoryId": &directory, "Alias": alias }),
                case,
            )
            .await;
        }
        tokio::time::sleep(PAST_ONE_SPAN).await;

        let (status, parsed) = call(
            &router,
            "AddRegion",
            json!({
                "DirectoryId": &directory,
                "RegionName": REGION,
                "VPCSettings": vpc_settings(subnets),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the home Region to be refused, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("UnsupportedOperationException"),
            "case {case:?}: expected the modelled unsupported refusal, got {parsed}"
        );
        let (message, request_id) = refusal(&parsed, case);
        assert_eq!(
            message,
            with_request_id(HOME_REGION_REFUSAL, &request_id),
            "case {case:?}: expected the fixed home-Region sentence, got {message:?}"
        );
        // Nothing of the request is quoted: not the directory, not the Region, not a
        // subnet — only the call's own request id follows the sentence.
        for quoted in [directory.as_str(), REGION, subnets[0]] {
            assert!(
                !message.contains(quoted),
                "case {case:?}: expected the sentence to quote nothing of the request, \
                 found {quoted:?} in {message:?}"
            );
        }
        stated.push(message.replace(&request_id, ""));
    }

    // One sentence for every row: the only part that differs is the call's request id.
    assert!(
        stated.windows(2).all(|pair| pair[0] == pair[1]),
        "case \"every row\": expected one fixed sentence, got {stated:?}"
    );
}

/// RULE: a directory's servers sit in two subnets in two different Availability Zones, so
/// a settings block naming fewer than two DISTINCT subnets is refused — by every create
/// that places servers in a VPC, with the one FIXED sentence Directory Service states that
/// rule in, which quotes no subnet id and no count.
#[tokio::test]
async fn the_subnet_pair_rule_is_stated_identically_by_every_placement() {
    let router = service();

    // The subnet lists that cannot carry a directory's servers, varied along width (the
    // two the modelled `SubnetId` pattern admits), count and repetition.
    let unservable: [(&str, &[&str]); 4] = [
        ("one eight-hex subnet", &["subnet-0a1b2c3d"]),
        ("one seventeen-hex subnet", &["subnet-0a1b2c3d4e5f67890"]),
        (
            "the same subnet twice",
            &["subnet-0a1b2c3d", "subnet-0a1b2c3d"],
        ),
        (
            "the same subnet three times",
            &["subnet-1a2b3c4d", "subnet-1a2b3c4d", "subnet-1a2b3c4d"],
        ),
    ];

    // The directory the add below replicates: an Enterprise Managed Microsoft AD in
    // service, so `AddRegion` reaches the placement check rather than an earlier one.
    let replicated = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("replicated.example.com"),
        "the directory AddRegion replicates",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (index, (case, subnets)) in unservable.iter().enumerate() {
        let mut simple = minimal_create(&format!("simple{index}.example.com"));
        simple["VpcSettings"] = vpc_settings(subnets);
        let mut managed = minimal_microsoft_ad(&format!("managed{index}.example.com"));
        managed["VpcSettings"] = vpc_settings(subnets);
        let connector = connect_body(&format!("connected{index}.example.com"), "Small", subnets);
        let add_region = json!({
            "DirectoryId": &replicated,
            "RegionName": "eu-west-1",
            "VPCSettings": vpc_settings(subnets),
        });

        for (operation, body) in [
            ("CreateDirectory", simple),
            ("CreateMicrosoftAD", managed),
            ("ConnectDirectory", connector),
            ("AddRegion", add_region),
        ] {
            let row = format!("{operation} with {case}");
            let (status, parsed) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {row:?}: expected the placement to be refused, got {status} with {parsed}"
            );
            assert_eq!(
                parsed["__type"],
                json!("InvalidParameterException"),
                "case {row:?}: expected the modelled parameter refusal, got {parsed}"
            );
            let (message, request_id) = refusal(&parsed, &row);
            assert_eq!(
                message,
                with_request_id(SUBNET_PAIR_REFUSAL, &request_id),
                "case {row:?}: expected the shared subnet-pair sentence, got {message:?}"
            );
        }
    }

    // The rule is about the subnets and nothing else: two distinct ones are served.
    let served = create_directory(
        &router,
        {
            let mut body = minimal_create("servable.example.com");
            body["VpcSettings"] = vpc_settings(&SERVABLE_SUBNETS);
            body
        },
        "two distinct subnets",
    )
    .await;
    assert!(
        !served.is_empty(),
        "case \"two distinct subnets\": expected the create to be served"
    );
}

/// RULE: an operation whose filter is a LIST of directory ids reports every id it could not
/// resolve, in the order the filter sent them, separated by a bare comma — so a client that
/// splits the message on `,` recovers exactly the ids it has to fix.
#[tokio::test]
async fn unresolvable_filter_ids_are_named_in_filter_order_with_bare_commas() {
    let router = service();

    // One stored directory, so a row mixing a resolvable id with absent ones reports only
    // the absent ones.
    let stored = create_directory(&router, minimal_create("stored.example.com"), "the stored one")
        .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Rows differing in count, order and whether a resolvable id or a repeat is mixed in.
    let rows: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        (
            "one absent id",
            vec![ABSENT_IDS[0].to_string()],
            vec![ABSENT_IDS[0]],
        ),
        (
            "two absent ids",
            vec![ABSENT_IDS[0].to_string(), ABSENT_IDS[1].to_string()],
            vec![ABSENT_IDS[0], ABSENT_IDS[1]],
        ),
        (
            "the same two in the other order",
            vec![ABSENT_IDS[1].to_string(), ABSENT_IDS[0].to_string()],
            vec![ABSENT_IDS[1], ABSENT_IDS[0]],
        ),
        (
            "four absent ids",
            ABSENT_IDS.iter().map(|id| (*id).to_string()).collect(),
            ABSENT_IDS.to_vec(),
        ),
        (
            "a stored id between two absent ones",
            vec![
                ABSENT_IDS[2].to_string(),
                stored.clone(),
                ABSENT_IDS[3].to_string(),
            ],
            vec![ABSENT_IDS[2], ABSENT_IDS[3]],
        ),
        (
            "an absent id the filter repeats",
            vec![
                ABSENT_IDS[1].to_string(),
                ABSENT_IDS[1].to_string(),
                ABSENT_IDS[0].to_string(),
            ],
            vec![ABSENT_IDS[1], ABSENT_IDS[0]],
        ),
    ];

    for (case, filter, expected) in &rows {
        let (status, parsed) = call(
            &router,
            "DescribeDirectories",
            json!({ "DirectoryIds": filter }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the unresolvable filter to be refused, got {status} \
             with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("EntityDoesNotExistException"),
            "case {case:?}: expected the modelled filter-value refusal, got {parsed}"
        );
        let (message, request_id) = refusal(&parsed, case);
        let named = expected.join(",");
        assert_eq!(
            message,
            with_request_id(&format!("Invalid directory ID(s): {named}."), &request_id),
            "case {case:?}: expected the unresolved ids in filter order, got {message:?}"
        );
        // What a client does with the message: split the list on the separator and get the
        // ids back, with nothing to trim.
        let recovered: Vec<&str> = message
            .split_once("Invalid directory ID(s): ")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once('.'))
            .map(|(ids, _)| ids.split(',').collect())
            .unwrap_or_default();
        assert_eq!(
            recovered, *expected,
            "case {case:?}: expected the ids to be recoverable by splitting on a comma, \
             got {recovered:?}"
        );
    }

    // The refusal is about the ids the filter could not resolve: a filter naming only the
    // stored directory is served.
    let answer = ok(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [&stored] }),
        "a filter that resolves",
    )
    .await;
    assert!(
        answer["DirectoryDescriptions"].is_array(),
        "case \"a filter that resolves\": expected the listing to be served, got {answer}"
    );
}

/// RULE: `AccessUrl` reports the HOST the directory's alias names — the alias, or the
/// directory id when no alias exists, plus the access-URL suffix — with no scheme and no
/// path, so a client builds a URL by prefixing its own scheme.
#[tokio::test]
async fn the_access_url_is_the_bare_host_of_the_directorys_alias() {
    let router = service();

    // One row per create that makes a directory a description reports an access URL for,
    // crossed with whether an alias was assigned.
    let rows: [(&str, &str, Value, Option<&str>); 6] = [
        (
            "a Simple AD with no alias",
            "CreateDirectory",
            minimal_create("simple-host.example.com"),
            None,
        ),
        (
            "a Simple AD carrying an alias",
            "CreateDirectory",
            minimal_create("simple-alias.example.com"),
            Some("simple-ad-row"),
        ),
        (
            "a Managed Microsoft AD with no alias",
            "CreateMicrosoftAD",
            minimal_microsoft_ad("managed-host.example.com"),
            None,
        ),
        (
            "a Managed Microsoft AD carrying an alias",
            "CreateMicrosoftAD",
            minimal_microsoft_ad("managed-alias.example.com"),
            Some("managed-ad-row"),
        ),
        (
            "an AD Connector with no alias",
            "ConnectDirectory",
            connect_body("connected-host.example.com", "Small", &SERVABLE_SUBNETS),
            None,
        ),
        (
            "an AD Connector carrying an alias",
            "ConnectDirectory",
            connect_body("connected-alias.example.com", "Large", &SERVABLE_SUBNETS),
            Some("connected-row"),
        ),
    ];

    for (case, operation, body, alias) in rows {
        let created = ok(&router, operation, body, case).await;
        let directory = string(&created, "DirectoryId", case);
        if let Some(alias) = alias {
            // An alias is assigned only once the directory has reached its serving stage.
            tokio::time::sleep(PAST_ONE_SPAN).await;
            ok(
                &router,
                "CreateAlias",
                json!({ "DirectoryId": &directory, "Alias": alias }),
                case,
            )
            .await;
        }
        tokio::time::sleep(PAST_ONE_SPAN).await;

        let described = describe_one(&router, &directory, case).await;
        let reported_alias = string(&described, "Alias", case);
        let access_url = string(&described, "AccessUrl", case);
        assert_eq!(
            reported_alias,
            alias.unwrap_or(&directory),
            "case {case:?}: expected the alias to default to the directory id, got {described}"
        );
        assert_eq!(
            access_url,
            format!("{reported_alias}{ACCESS_URL_SUFFIX}"),
            "case {case:?}: expected the host the alias names, got {access_url:?}"
        );
        // A HOST: letters, digits, dots and hyphens, optionally with a port — no scheme
        // and no path, so prefixing a scheme cannot produce `https://http://...`.
        assert!(
            access_url
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '.'
                    || character == '-'),
            "case {case:?}: expected a bare host, got {access_url:?}"
        );
        for forbidden in ["://", "http", "/", ":"] {
            assert!(
                !access_url.contains(forbidden),
                "case {case:?}: expected no {forbidden:?} in the access URL, got \
                 {access_url:?}"
            );
        }
    }
}

/// RULE: `SettingEntry$AllowedValues` carries the values a setting admits as a JSON array of
/// strings, so a client enumerates the legal values of a setting by decoding the member —
/// and the value applied to the directory is one of them.
#[tokio::test]
async fn allowed_values_are_reported_as_a_json_array_of_strings() {
    let router = service();

    // Two directories of the type that carries settings, created differently, so the
    // encoding is the catalogue's rather than one request's.
    let rows: [(&str, Value); 2] = [
        (
            "the minimal directory",
            minimal_microsoft_ad("settings-minimal.example.com"),
        ),
        (
            "the Standard directory",
            json!({
                "Name": "settings-standard.example.com",
                "Password": crate::directory_lifecycle::PASSWORD,
                "Edition": "Standard",
                "ShortName": "SETTINGS",
                "VpcSettings": vpc_settings(&SERVABLE_SUBNETS),
            }),
        ),
    ];

    for (case, body) in rows {
        let directory = create_microsoft_ad(&router, body, case).await;
        tokio::time::sleep(PAST_ONE_SPAN).await;

        let answer = ok(
            &router,
            "DescribeSettings",
            json!({ "DirectoryId": &directory }),
            case,
        )
        .await;
        let entries = answer["SettingEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected SettingEntries, got {answer}"))
            .clone();
        assert!(
            !entries.is_empty(),
            "case {case:?}: expected at least one setting entry, got {answer}"
        );

        for entry in &entries {
            let name = string(entry, "Name", case);
            let encoded = string(entry, "AllowedValues", case);
            let row = format!("{case}/{name}");
            let allowed: Vec<String> = serde_json::from_str(&encoded).unwrap_or_else(|error| {
                panic!(
                    "case {row:?}: expected AllowedValues to decode as a JSON array of \
                     strings, got {encoded:?} ({error})"
                )
            });
            assert!(
                allowed.len() > 1,
                "case {row:?}: expected the values the setting admits, got {allowed:?}"
            );
            // Each value is one the modelled `DirectoryConfigurationSettingValue` pattern
            // `^[a-zA-Z0-9_]*$` admits, so no entry carries a joined list as one value.
            for value in &allowed {
                assert!(
                    value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                    "case {row:?}: expected a single modelled value, got {value:?}"
                );
            }
            let applied = string(entry, "AppliedValue", case);
            assert!(
                allowed.contains(&applied),
                "case {row:?}: expected the applied value {applied:?} among {allowed:?}"
            );
        }
    }
}
