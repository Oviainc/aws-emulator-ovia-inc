// SPDX-License-Identifier: CC0-1.0

//! `CreateMicrosoftAD`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body — and
//! reads the created directory back through `DescribeDirectories` and
//! `ListTagsForResource`. Nothing reaches into the state struct, so a response that is
//! not computed from persisted state cannot pass. The harness (router, call helpers,
//! quotas) is the sibling suite's, reached as `crate::directory_lifecycle::<item>`.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the stored
//! record over a table of create requests that differ in edition, short name,
//! description, network type, subnet count and tag count; the default `ShortName` over a
//! table of DNS names differing in label count, separator and case; the admitted
//! editions over the WHOLE modelled `DirectoryEdition` catalog; the admitted network
//! types over the whole modelled `NetworkType` catalog; and the Managed Microsoft AD
//! Region quota — its own allowance, not one shared with Simple AD — at the bound and one
//! past it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    assert_directory_id, create_directory, describe_one, descriptions, minimal_create, ok, refused,
    service, string, tag_map, MICROSOFT_AD_QUOTA, PASSWORD, PAST_ONE_SPAN, STAGE_ACTIVE,
    STAGE_REQUESTED, TAG_QUOTA,
};

/// The `DirectoryType` a `CreateMicrosoftAD` directory reports: the `MicrosoftAD` value
/// of the modelled `DirectoryType` enum
/// (`smithy_query(query="shape",shape="DirectoryType")`).
const TYPE_MICROSOFT_AD: &str = "MicrosoftAD";

/// The whole modelled `DirectoryEdition` catalog
/// (`smithy_query(query="shape",shape="DirectoryEdition")`: `Enterprise`, `Standard`,
/// `Hybrid`), paired with whether `CreateMicrosoftAD` admits it. From
/// `CreateMicrosoftADRequest$Edition`'s own documentation: "Managed Microsoft AD is
/// available in two editions: `Standard` and `Enterprise`." `Hybrid` is therefore not
/// one of this operation's editions.
const EDITION_CATALOG: [(&str, bool); 3] =
    [("Enterprise", true), ("Standard", true), ("Hybrid", false)];

/// The edition a request that names none gets, from the same sentence: "`Enterprise` is
/// the default."
const DEFAULT_EDITION: &str = "Enterprise";

/// The whole modelled `NetworkType` catalog
/// (`smithy_query(query="shape",shape="NetworkType")`). Unlike
/// `CreateDirectoryRequest$NetworkType` ("Simple AD supports IPv4 and Dual-stack only"),
/// `CreateMicrosoftADRequest$NetworkType` narrows none of them.
const NETWORK_TYPE_CATALOG: [&str; 3] = ["Dual-stack", "IPv4", "IPv6"];

/// The domain controllers a freshly created Managed Microsoft AD directory has: the
/// minimum the modelled `DesiredNumberOfDomainControllers` allows (`range { min: 2 }`),
/// one directory server per subnet of the two `DirectoryVpcSettings$SubnetIds` requires.
const FRESH_DOMAIN_CONTROLLERS: i64 = 2;

/// A minimal well-formed `CreateMicrosoftAD` body for `name`: only the three required
/// members, so every other value in the stored record is one the service derived.
pub(crate) fn minimal_microsoft_ad(name: &str) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "VpcSettings": {
            "VpcId": "vpc-0a1b2c3d",
            "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
        }
    })
}

/// `CreateMicrosoftAD` with `body`, asserting it was accepted and returning the id.
pub(crate) async fn create_microsoft_ad(
    router: &MultiServiceRouter,
    body: Value,
    case: &str,
) -> String {
    let answer = ok(router, "CreateMicrosoftAD", body, case).await;
    let id = string(&answer, "DirectoryId", case);
    assert_directory_id(&id, case);
    id
}

/// How many directories the store holds, read back through `DescribeDirectories`.
async fn stored_count(router: &MultiServiceRouter, case: &str) -> usize {
    let answer = ok(router, "DescribeDirectories", json!({}), case).await;
    descriptions(&answer, case).len()
}

/// One row of the create table: a request and what the stored record must then report.
struct CreateCase {
    name: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn a_created_microsoft_ad_directory_is_reported_from_persisted_state() {
    let router = service();

    // The rows differ along everything `CreateMicrosoftAD` persists: whether an
    // edition / short name / description / network type was given, how many subnets the
    // VPC settings carry and how many tags came with the request.
    let cases = vec![
        CreateCase {
            name: "minimal",
            body: minimal_microsoft_ad("corp.example.com"),
            expect: vec![
                ("Name", json!("corp.example.com")),
                // No edition in the request: the default one.
                ("Edition", json!(DEFAULT_EDITION)),
                // No short name in the request: the first label of the DNS name.
                ("ShortName", json!("CORP")),
                ("Description", json!(null)),
                ("NetworkType", json!(null)),
            ],
        },
        CreateCase {
            name: "Standard edition with short name and description",
            body: json!({
                "Name": "sales.example.org",
                "Password": PASSWORD,
                "Edition": "Standard",
                "ShortName": "SALESNB",
                "Description": "Sales directory",
                "VpcSettings": {
                    "VpcId": "vpc-11223344",
                    "SubnetIds": ["subnet-11223344", "subnet-55667788"]
                }
            }),
            expect: vec![
                ("Edition", json!("Standard")),
                ("ShortName", json!("SALESNB")),
                ("Description", json!("Sales directory")),
                (
                    "VpcSettings",
                    json!({
                        "VpcId": "vpc-11223344",
                        "SubnetIds": ["subnet-11223344", "subnet-55667788"]
                    }),
                ),
            ],
        },
        CreateCase {
            name: "IPv6 with three subnets",
            body: json!({
                "Name": "eng-dept.example.com",
                "Password": PASSWORD,
                "Edition": "Enterprise",
                "NetworkType": "IPv6",
                "VpcSettings": {
                    "VpcId": "vpc-abcdef01",
                    "SubnetIds": ["subnet-aaaaaaaa", "subnet-bbbbbbbb", "subnet-cccccccc"]
                }
            }),
            expect: vec![
                ("Edition", json!("Enterprise")),
                ("NetworkType", json!("IPv6")),
                ("ShortName", json!("ENG-DEPT")),
                (
                    "VpcSettings",
                    json!({
                        "VpcId": "vpc-abcdef01",
                        "SubnetIds": ["subnet-aaaaaaaa", "subnet-bbbbbbbb", "subnet-cccccccc"]
                    }),
                ),
            ],
        },
        CreateCase {
            name: "with three tags",
            body: json!({
                "Name": "ops.example.com",
                "Password": PASSWORD,
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                },
                "Tags": [
                    { "Key": "env", "Value": "dev" },
                    { "Key": "owner", "Value": "team-a" },
                    { "Key": "cost-centre", "Value": "" }
                ]
            }),
            expect: vec![("Name", json!("ops.example.com"))],
        },
    ];

    let mut ids: Vec<String> = Vec::new();
    for case in &cases {
        ids.push(create_microsoft_ad(&router, case.body.clone(), case.name).await);
    }

    // Two creates must yield two distinct ids and two distinct records.
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "expected {} distinct directory ids, got {ids:?}",
        ids.len()
    );

    for (case, id) in cases.iter().zip(&ids) {
        let described = describe_one(&router, id, case.name).await;
        for (member, expected) in &case.expect {
            assert_eq!(
                &described[*member], expected,
                "case {:?}: expected {member} to be {expected}, got {described}",
                case.name
            );
        }
        assert_eq!(
            described["DirectoryId"],
            json!(id),
            "case {:?}: expected DirectoryId {id}, got {described}",
            case.name
        );
        // What makes the record a Managed Microsoft AD rather than a Simple AD: its
        // type, an edition the request did not have to give, and the domain controllers
        // the modelled `DesiredNumberOfDomainControllers` reports "if the directory is
        // Microsoft AD".
        assert_eq!(
            described["Type"],
            json!(TYPE_MICROSOFT_AD),
            "case {:?}: expected Type {TYPE_MICROSOFT_AD}, got {described}",
            case.name
        );
        // `CreateMicrosoftAD` takes no `Size`, so the reported one is DERIVED from the
        // edition the directory runs: the Standard edition is the smaller capacity, every
        // other edition the larger one.
        let expected_size = if described["Edition"] == json!("Standard") {
            "Small"
        } else {
            "Large"
        };
        assert_eq!(
            described["Size"],
            json!(expected_size),
            "case {:?}: expected the Size derived from Edition {} to be {expected_size}, \
             got {described}",
            case.name,
            described["Edition"]
        );
        assert_eq!(
            described["DesiredNumberOfDomainControllers"],
            json!(FRESH_DOMAIN_CONTROLLERS),
            "case {:?}: expected {FRESH_DOMAIN_CONTROLLERS} domain controllers, got {described}",
            case.name
        );
        assert_eq!(
            described["SsoEnabled"],
            json!(false),
            "case {:?}: expected single sign-on off on a fresh directory, got {described}",
            case.name
        );
        // "If no alias exists, the alias is the directory identifier", and "The access
        // URL for the directory, such as http://<alias>.awsapps.com".
        assert_eq!(
            described["Alias"],
            json!(id),
            "case {:?}: expected Alias to default to the directory id, got {described}",
            case.name
        );
        assert_eq!(
            described["AccessUrl"],
            json!(format!("http://{id}.awsapps.com")),
            "case {:?}: expected AccessUrl derived from the alias, got {described}",
            case.name
        );
    }

    // The tags submitted with the fourth create are the tags the store holds for it.
    let tags = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": ids[3] }),
        "with three tags",
    )
    .await;
    assert_eq!(
        tag_map(&tags, "with three tags"),
        BTreeMap::from([
            ("env".to_string(), "dev".to_string()),
            ("owner".to_string(), "team-a".to_string()),
            ("cost-centre".to_string(), String::new()),
        ]),
        "expected CreateMicrosoftAD's tags to be the tags ListTagsForResource reports"
    );
    // A directory created without tags reports none, not another directory's.
    let untagged = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": ids[0] }),
        "minimal",
    )
    .await;
    assert!(
        tag_map(&untagged, "minimal").is_empty(),
        "case {:?}: expected no tags on a directory created without any, got {untagged}",
        "minimal"
    );
}

#[tokio::test]
async fn an_absent_short_name_defaults_to_the_first_label_of_the_dns_name() {
    let router = service();

    // `ShortName`'s own documentation: "If you don't specify a NetBIOS name, it will
    // default to the first part of your directory DNS. For example, CORP for the
    // directory DNS corp.example.com." The rows differ in label count, separator and
    // case — including a name the issue did not cite.
    let cases: [(&str, &str); 6] = [
        ("corp.example.com", "CORP"),
        ("sales.example.org", "SALES"),
        ("ENG.example.com", "ENG"),
        ("ops-eu.example.com", "OPS-EU"),
        ("mixedCase.test.internal", "MIXEDCASE"),
        ("d1.example.com", "D1"),
    ];

    for (name, expected) in cases {
        let id = create_microsoft_ad(&router, minimal_microsoft_ad(name), name).await;
        let described = describe_one(&router, &id, name).await;
        assert_eq!(
            described["ShortName"],
            json!(expected),
            "case {name:?}: expected the default ShortName {expected:?}, got {described}"
        );
    }

    // A request that DOES name a short name keeps it, derivation or not.
    let id = create_microsoft_ad(
        &router,
        json!({
            "Name": "corp.example.net",
            "Password": PASSWORD,
            "ShortName": "CHOSEN",
            "VpcSettings": {
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
            }
        }),
        "explicit short name",
    )
    .await;
    let described = describe_one(&router, &id, "explicit short name").await;
    assert_eq!(
        described["ShortName"],
        json!("CHOSEN"),
        "case {:?}: expected the requested ShortName to be kept, got {described}",
        "explicit short name"
    );
}

#[tokio::test]
async fn only_the_documented_editions_are_admitted() {
    let router = service();

    for (edition, admitted) in EDITION_CATALOG {
        let body = json!({
            "Name": format!("{}.example.com", edition.to_lowercase()),
            "Password": PASSWORD,
            "Edition": edition,
            "VpcSettings": {
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
            }
        });
        if admitted {
            let id = create_microsoft_ad(&router, body, edition).await;
            let described = describe_one(&router, &id, edition).await;
            assert_eq!(
                described["Edition"],
                json!(edition),
                "case {edition:?}: expected the requested Edition to be stored, got {described}"
            );
        } else {
            let before = stored_count(&router, edition).await;
            refused(
                &router,
                "CreateMicrosoftAD",
                body,
                "InvalidParameterException",
                edition,
            )
            .await;
            assert_eq!(
                stored_count(&router, edition).await,
                before,
                "case {edition:?}: expected the refused create to persist nothing"
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_network_type_is_admitted_and_stored() {
    let router = service();

    for network_type in NETWORK_TYPE_CATALOG {
        let case = network_type;
        let id = create_microsoft_ad(
            &router,
            json!({
                "Name": format!("net{}.example.com", network_type.replace('-', "")),
                "Password": PASSWORD,
                "NetworkType": network_type,
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            case,
        )
        .await;
        let described = describe_one(&router, &id, case).await;
        assert_eq!(
            described["NetworkType"],
            json!(network_type),
            "case {case:?}: expected the requested NetworkType to be stored, got {described}"
        );
    }

    // `NetworkType`'s own documentation: "The default value is IPv4 or IPv6 based on the
    // provided subnet capabilities" — a choice of the VPC data plane this emulator does
    // not have, so a request that names none leaves the member absent rather than
    // reporting a network type nothing determined.
    let id = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("nodefault.example.com"),
        "no network type",
    )
    .await;
    let described = describe_one(&router, &id, "no network type").await;
    assert_eq!(
        described["NetworkType"],
        json!(null),
        "case {:?}: expected no NetworkType when the request named none, got {described}",
        "no network type"
    );
}

#[tokio::test]
async fn a_created_microsoft_ad_directory_leaves_the_requested_stage_on_the_clock() {
    let case = "stage transition";
    let router = service();

    let id = create_microsoft_ad(&router, minimal_microsoft_ad("stage.example.com"), case).await;

    // A caller acting straight after the create still sees the transient stage the
    // modelled `DirectoryStage` enum opens with.
    let fresh = describe_one(&router, &id, case).await;
    assert_eq!(
        fresh["Stage"],
        json!(STAGE_REQUESTED),
        "case {case:?}: expected the transient stage straight after the create, got {fresh}"
    );

    tokio::time::sleep(PAST_ONE_SPAN).await;

    let settled = describe_one(&router, &id, case).await;
    assert_eq!(
        settled["Stage"],
        json!(STAGE_ACTIVE),
        "case {case:?}: expected the stage the clock owed, got {settled}"
    );
    // Reading twice does not move the stage on again.
    let again = describe_one(&router, &id, case).await;
    assert_eq!(
        again["Stage"], settled["Stage"],
        "case {case:?}: expected a second read to report the same stage, got {again}"
    );
    assert_eq!(
        again["StageLastUpdatedDateTime"], settled["StageLastUpdatedDateTime"],
        "case {case:?}: expected a second read to report the same stage timestamp, got {again}"
    );
}

#[tokio::test]
async fn the_managed_microsoft_ad_quota_is_its_own_allowance() {
    let case = "Managed Microsoft AD quota";
    let router = service();

    // Managed Microsoft AD carries its OWN published Region quota, so this create is
    // bounded by that allowance alone and not by the Simple AD one: filling the Simple
    // AD quota first must leave the Managed Microsoft AD allowance untouched.
    for index in 0..crate::directory_lifecycle::CLOUD_ONLY_QUOTA {
        create_directory(
            &router,
            minimal_create(&format!("simple{index}.example.com")),
            case,
        )
        .await;
    }
    for index in 0..MICROSOFT_AD_QUOTA {
        create_microsoft_ad(
            &router,
            minimal_microsoft_ad(&format!("managed{index}.example.com")),
            case,
        )
        .await;
    }
    assert_eq!(
        stored_count(&router, case).await,
        crate::directory_lifecycle::CLOUD_ONLY_QUOTA + MICROSOFT_AD_QUOTA,
        "case {case:?}: expected both allowances to be fillable at once"
    );

    refused(
        &router,
        "CreateMicrosoftAD",
        minimal_microsoft_ad("overflow.example.com"),
        "DirectoryLimitExceededException",
        "one past the Managed Microsoft AD quota",
    )
    .await;
    assert_eq!(
        stored_count(&router, case).await,
        crate::directory_lifecycle::CLOUD_ONLY_QUOTA + MICROSOFT_AD_QUOTA,
        "case {case:?}: expected the refused create to persist nothing"
    );
}

#[tokio::test]
async fn the_tags_a_create_carries_are_bounded_and_stored() {
    let router = service();

    // The rows differ in how many tags the create carries: none, one, several, and the
    // whole per-resource allowance.
    for count in [0_usize, 1, 3, TAG_QUOTA] {
        let case = format!("{count} tags");
        let requested: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("key{index}"), "Value": format!("v{index}") }))
            .collect();
        let mut body = minimal_microsoft_ad(&format!("tags{count}.example.com"));
        body["Tags"] = json!(requested);
        let id = create_microsoft_ad(&router, body, &case).await;

        let listed = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceId": &id, "Limit": TAG_QUOTA }),
            &case,
        )
        .await;
        let stored = tag_map(&listed, &case);
        let expected: BTreeMap<String, String> = (0..count)
            .map(|index| (format!("key{index}"), format!("v{index}")))
            .collect();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the create's tags to be the tags stored, got {stored:?}"
        );
    }

    // One past the allowance is refused on the request's own size, and persists nothing.
    let case = "one past the tag quota";
    let over_quota: Vec<Value> = (0..=TAG_QUOTA)
        .map(|index| json!({ "Key": format!("bulk{index}"), "Value": "v" }))
        .collect();
    let mut body = minimal_microsoft_ad("overtags.example.com");
    body["Tags"] = json!(over_quota);
    let before = stored_count(&router, case).await;
    refused(
        &router,
        "CreateMicrosoftAD",
        body,
        "InvalidParameterException",
        case,
    )
    .await;
    assert_eq!(
        stored_count(&router, case).await,
        before,
        "case {case:?}: expected the refused create to persist nothing"
    );
}
