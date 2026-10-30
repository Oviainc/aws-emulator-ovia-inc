// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY `ImportHypervisorConfiguration` mints: the
//! `HypervisorArn` a successful import hands back, and the `hypervisor/<id>` resource id
//! inside it.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`
//! — the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour, and
//! the only way a later request can see what an earlier one minted is the shared store.
//!
//! The rule under test: a hypervisor's identity is MINTED BY THE SERVICE, never derived
//! from the request, and what it has to satisfy is the identity's modelled FORM plus the
//! invariants of an identity. The model fixes the form and nothing else:
//! `ImportHypervisorConfigurationOutput$HypervisorArn` targets `ServerArn`,
//! `@length(min: 50, max: 500)` with pattern
//! `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`, and
//! its documentation says only "The Amazon Resource Name (ARN) of the hypervisor"
//! (`smithy_query(query="shape",shape="ServerArn")` /
//! `smithy_query(query="doc",shape="ImportHypervisorConfigurationOutput",member="HypervisorArn")`).
//! So every import must answer with an ARN of that shape, naming this service, the region
//! the caller signed for, the emulator's account and the `hypervisor` resource type; its
//! resource id must be minted fresh for each import, whatever members the request repeats;
//! it must be unique among the identities handed out in its region; and it must be EXACT
//! and stable — every surface that names the record reports that one identity, a read never
//! changes it, and an ARN differing from it in a single segment or a single character of
//! the id names no record at all.
//!
//! WHICH id the service mints for a record is a per-run, service-generated value the model
//! leaves open, so no case below asserts a spelling for it: an assertion on the id's
//! prefix or width would pin the implementation rather than the rule, and would pass for
//! an id the service never minted.
//!
//! The tables span that class rather than one recorded import: rows differing in the region
//! signed for, in the form and length of every member an import carries (`Name` and `Host`
//! at both published bounds, an IP host and an FQDN, a KMS key ARN and a KMS alias ARN,
//! tags, credentials) and a row repeating an earlier row's members verbatim; and, for the
//! exactness half, a near miss of a real minted ARN in every segment it has and in every
//! way its id can be misspelled.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{build_engine, build_router, call_in_region, create, Reply, REGION};

/// A region other than the harness's, so the table spans the region half of an identity.
const OTHER_REGION: &str = "eu-central-1";

/// A third region, whose name is longer than both others: the region segment is caller
/// supplied, and an identity has to carry whichever one the caller signed for.
const LONGER_REGION: &str = "ap-southeast-4";

/// The segments of an ARN of this service, as the modelled pattern spells them:
/// `arn:<partition>:<service>:<region>:<account>:<resource_type>/<id>`.
#[derive(Debug)]
struct ArnSegments<'a> {
    partition: &'a str,
    service: &'a str,
    region: &'a str,
    account: &'a str,
    resource_type: &'a str,
    id: &'a str,
}

/// Split `arn` into its segments, or `None` for anything that is not the six
/// colon-separated fields of an ARN with a `<type>/<id>` resource.
///
/// Spelled out here rather than taken from the crate, so every assertion below is
/// independent of the code that mints the ARN.
fn arn_segments(arn: &str) -> Option<ArnSegments<'_>> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, partition, service, region, account, resource] = fields.as_slice() else {
        return None;
    };
    if *literal != "arn" {
        return None;
    }
    let (resource_type, id) = resource.split_once('/')?;
    Some(ArnSegments {
        partition,
        service,
        region,
        account,
        resource_type,
        id,
    })
}

/// Whether `arn` satisfies the MODELLED `ServerArn` — `@length(min: 50, max: 500)` and
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// (`smithy_query(query="shape",shape="ServerArn")`) — which is the whole of what the model
/// fixes about a hypervisor's identity.
fn matches_modelled_server_arn(arn: &str) -> bool {
    let Some(parts) = arn_segments(arn) else {
        return false;
    };
    (50..=500).contains(&arn.len())
        && matches!(parts.partition, "aws" | "aws-cn" | "aws-us-gov")
        && parts.service == "backup-gateway"
        && [parts.region, parts.account, parts.resource_type, parts.id]
            .into_iter()
            .all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            })
}

/// One import of the table: the region it is signed for and the body it carries.
struct ImportCase {
    case: &'static str,
    region: &'static str,
    body: Value,
}

/// The imports the table drives, spanning the class the rule generalises over: the region
/// an identity is minted in, and every member an import may carry, at the published bounds
/// of the modelled members — plus a row repeating the first row's members verbatim, which
/// is what shows the identity is minted and not derived from the request.
///
/// `Name` is `@length(min: 1, max: 100)` with pattern `^[a-zA-Z0-9-]*$`, `Host` is
/// `@length(min: 3, max: 128)` with pattern `^.+$`, `KmsKeyArn` is
/// `@length(min: 50, max: 500)` accepting a key or an alias ARN, and `Username` /
/// `Password` are `@sensitive`, `@length(min: 1, max: 100)`
/// (`smithy_query(query="operation",operation="ImportHypervisorConfiguration")`).
fn import_cases() -> Vec<ImportCase> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    let kms_key =
        format!("arn:aws:kms:{REGION}:{account}:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    let kms_alias = format!("arn:aws:kms:{REGION}:{account}:alias/backup-gateway-hypervisors");
    vec![
        ImportCase {
            case: "an IPv4 host in the region the caller signed for",
            region: REGION,
            body: json!({ "Name": "hv-ipv4", "Host": "198.51.100.10" }),
        },
        ImportCase {
            case: "an FQDN host with a KMS key ARN",
            region: REGION,
            body: json!({
                "Name": "hv-fqdn",
                "Host": "vcenter.example.com",
                "KmsKeyArn": kms_key,
            }),
        },
        ImportCase {
            case: "a KMS alias ARN",
            region: REGION,
            body: json!({
                "Name": "hv-alias",
                "Host": "203.0.113.7",
                "KmsKeyArn": kms_alias,
            }),
        },
        ImportCase {
            case: "the 1-character Name minimum and the 3-character Host minimum",
            region: REGION,
            body: json!({ "Name": "a", "Host": "a.b" }),
        },
        ImportCase {
            case: "the 100-character Name maximum and the 128-character Host maximum",
            region: REGION,
            body: json!({
                "Name": "n".repeat(100),
                "Host": format!("{}.example.com", "h".repeat(128 - ".example.com".len())),
            }),
        },
        ImportCase {
            case: "tags and credentials, none of which an identity may come from",
            region: REGION,
            body: json!({
                "Name": "hv-tagged",
                "Host": "198.51.100.11",
                "Username": "Administrator@vsphere.local",
                "Password": "Pa55word!",
                "Tags": [{ "Key": "Name", "Value": "hv-tagged" }],
            }),
        },
        ImportCase {
            case: "an import repeating the first row's members verbatim",
            region: REGION,
            body: json!({ "Name": "hv-ipv4", "Host": "198.51.100.10" }),
        },
        ImportCase {
            case: "the first import of another region",
            region: OTHER_REGION,
            body: json!({ "Name": "hv-other-region", "Host": "198.51.100.12" }),
        },
        ImportCase {
            case: "a region whose name is longer than the others",
            region: LONGER_REGION,
            body: json!({ "Name": "hv-longer-region", "Host": "198.51.100.13" }),
        },
    ]
}

/// Import a hypervisor over the wire, in `region`.
async fn import(router: &MultiServiceRouter, region: &str, body: &Value) -> Reply {
    call_in_region(router, region, "ImportHypervisorConfiguration", body).await
}

/// The `HypervisorArn`s a `ListHypervisors` reply reports, in the order it reported them.
fn listed_identities(reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("Hypervisors")
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get("HypervisorArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A member of an import body, as the request carried it.
fn member_of(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the import body must carry {member}: {body:?}"))
        .to_string()
}

/// RULE (the minting half): every import mints its OWN identity, in the form the model
/// fixes for it — an ARN satisfying `ServerArn` that names this service, the region the
/// caller signed for, the emulator's account and the `hypervisor` resource type — unique
/// among the identities its region has handed out whatever members the request repeats, and
/// the identity every surface of this service reports for the record it stored.
#[tokio::test]
async fn every_import_mints_its_own_identity_in_the_modelled_form() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Every identity this run has seen, and every (region, resource id) pair it minted: a
    // bare id is scoped to the region of the ARN that carries it, so that is the pair a
    // repeat would show up in.
    let mut identities: BTreeSet<String> = BTreeSet::new();
    let mut regional_ids: BTreeSet<(String, String)> = BTreeSet::new();

    let cases = import_cases();
    for ImportCase { case, region, body } in &cases {
        let reply = import(&router, region, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let arn = reply
            .str_at("/HypervisorArn")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a successful import must carry a HypervisorArn, got {:?}",
                    reply.body
                )
            })
            .to_string();
        assert!(
            matches_modelled_server_arn(&arn),
            "case {case:?}: the minted identity must satisfy the modelled ServerArn \
             (50..=500 characters, the ARN pattern), got {arn:?}"
        );
        let parts = arn_segments(&arn).unwrap_or_else(|| {
            panic!("case {case:?}: {arn:?} is not an ARN of this service at all")
        });
        assert_eq!(
            (
                parts.partition,
                parts.service,
                parts.region,
                parts.account,
                parts.resource_type,
            ),
            (
                "aws",
                "backup-gateway",
                *region,
                engine::EMULATOR_ACCOUNT_ID,
                "hypervisor",
            ),
            "case {case:?}: the minted identity must name this service, the region the \
             caller signed for, the emulator's account and the hypervisor resource type, \
             got {arn:?}"
        );
        assert!(
            identities.insert(arn.clone()),
            "case {case:?}: the service handed out the identity {arn:?} twice"
        );
        assert!(
            regional_ids.insert((parts.region.to_string(), parts.id.to_string())),
            "case {case:?}: the service minted the resource id {:?} twice in {region}",
            parts.id
        );

        // The identity names the record THIS import stored, and reading it reports the same
        // identity every time: a read resolves an identity, it does not change one.
        let name = member_of(body, "Name");
        let host = member_of(body, "Host");
        for attempt in ["the first read", "the second read"] {
            let read = call_in_region(
                &router,
                region,
                "GetHypervisor",
                &json!({ "HypervisorArn": arn }),
            )
            .await;
            assert_eq!(
                (
                    read.status,
                    read.str_at("/Hypervisor/HypervisorArn"),
                    read.str_at("/Hypervisor/Name"),
                    read.str_at("/Hypervisor/Host"),
                ),
                (
                    200,
                    Some(arn.as_str()),
                    Some(name.as_str()),
                    Some(host.as_str()),
                ),
                "case {case:?} / {attempt}: the minted identity must name the record the \
                 import stored and report that identity unchanged, got {} {:?}",
                read.status,
                read.body
            );
        }

        // The listing of that region reports the identity exactly once — the same string,
        // not a second spelling of the same record.
        let listing = call_in_region(&router, region, "ListHypervisors", &json!({})).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: ListHypervisors must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        let listed = listed_identities(&listing);
        assert_eq!(
            listed.iter().filter(|reported| *reported == &arn).count(),
            1,
            "case {case:?}: the region's listing must report the minted identity exactly \
             once, got {listed:?}"
        );

        // And the one `ResourceArn` member of the tagging surface resolves that identity to
        // the same record, echoing the ARN the store holds it under.
        let tags = call_in_region(
            &router,
            region,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
        )
        .await;
        assert_eq!(
            (tags.status, tags.str_at("/ResourceArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: the minted identity must resolve on the tagging surface too, \
             got {} {:?}",
            tags.status,
            tags.body
        );
    }

    assert_eq!(
        identities.len(),
        cases.len(),
        "case table: every import of the table must have handed out its own identity, got \
         {identities:?}"
    );
}

/// The near misses of `arn`: one row per segment an ARN has and per way its resource id can
/// be misspelled, each a well-formed `ServerArn` (so it reaches the handler rather than the
/// boundary validator) differing from the minted identity in exactly one place.
fn near_misses(arn: &str) -> Vec<(String, String)> {
    // The rows below are built by slicing the id, so the identity they are derived from has
    // to be one the modelled pattern accepts: an ASCII id of at least one character.
    assert!(
        matches_modelled_server_arn(arn),
        "a minted identity must satisfy the modelled ServerArn before a near miss of it can \
         be built, got {arn:?}"
    );
    let parts = arn_segments(arn).expect("a minted identity is an ARN of this service");
    let rebuild = |partition: &str, region: &str, account: &str, resource_type: &str, id: &str| {
        format!("arn:{partition}:backup-gateway:{region}:{account}:{resource_type}/{id}")
    };
    let here = |id: &str| {
        rebuild(
            parts.partition,
            parts.region,
            parts.account,
            parts.resource_type,
            id,
        )
    };
    let id = parts.id;
    let mut rows = vec![
        (
            "the china partition".to_string(),
            rebuild(
                "aws-cn",
                parts.region,
                parts.account,
                parts.resource_type,
                id,
            ),
        ),
        (
            "the govcloud partition".to_string(),
            rebuild(
                "aws-us-gov",
                parts.region,
                parts.account,
                parts.resource_type,
                id,
            ),
        ),
        (
            "another region".to_string(),
            rebuild(
                parts.partition,
                OTHER_REGION,
                parts.account,
                parts.resource_type,
                id,
            ),
        ),
        (
            "another account".to_string(),
            rebuild(
                parts.partition,
                parts.region,
                "999999999999",
                parts.resource_type,
                id,
            ),
        ),
        (
            "the gateway resource type".to_string(),
            rebuild(
                parts.partition,
                parts.region,
                parts.account,
                "gateway",
                id,
            ),
        ),
        (
            "the virtual-machine resource type".to_string(),
            rebuild(parts.partition, parts.region, parts.account, "vm", id),
        ),
        (
            "the id with one more character".to_string(),
            here(&format!("{id}0")),
        ),
        (
            // Inserted by POSITION rather than after any character the id happens to
            // carry, so the row says nothing about how an id is spelled.
            "the id with a character inserted inside it".to_string(),
            here(&format!("{}0{}", &id[..1], &id[1..])),
        ),
        (
            // The last character changed for another the pattern's class accepts: the
            // nearest miss there is.
            "the id with its last character changed".to_string(),
            here(&format!(
                "{}{}",
                &id[..id.len() - 1],
                if id.ends_with('z') { 'y' } else { 'z' }
            )),
        ),
    ];
    // The pattern's character class accepts upper case, so an upper-cased id is well formed
    // — and it is a different id, which is the point of the row. Skipped for an id that has
    // no lower-case letter to raise, which would leave the identity itself.
    let upper = id.to_ascii_uppercase();
    if upper != id {
        rows.push(("the id in upper case".to_string(), here(&upper)));
    }
    // An id one character SHORTER leaves a shorter ARN, which the row needs to still clear
    // the modelled 50-character minimum to reach the handler at all.
    let shortened = here(&id[..id.len() - 1]);
    if matches_modelled_server_arn(&shortened) {
        rows.push((
            "the id with its last character dropped".to_string(),
            shortened,
        ));
    }
    rows
}

/// RULE (the exactness half): the minted identity is the record's ONLY name. An ARN
/// differing from it in a single segment or a single character of its id names no record,
/// and is refused as a missing hypervisor; the identity itself keeps resolving, and the bare
/// `HypervisorId` this service reports for the record is exactly the id segment of that one
/// ARN — so a consumer that keeps the identity it was handed resolves the record, and one
/// that reconstructs a near-enough identity resolves nothing.
#[tokio::test]
async fn nothing_but_the_exact_minted_identity_names_the_hypervisor_record() {
    let engine = build_engine();
    let router = build_router(&engine);

    let reply = import(
        &router,
        REGION,
        &json!({ "Name": "hv-exact", "Host": "198.51.100.20" }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let arn = reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string();
    let id = arn_segments(&arn)
        .expect("a minted identity is an ARN of this service")
        .id
        .to_string();

    let rows = near_misses(&arn);
    assert!(
        // The six segment rows plus the three misspellings every id form admits; the two
        // that depend on the id this run minted are extra.
        rows.len() >= 9,
        "case table: the near misses must span every segment of the identity and every way \
         its id can be misspelled, got {rows:?}"
    );
    for (case, candidate) in &rows {
        assert_ne!(
            candidate, &arn,
            "case {case:?}: a near miss must differ from the minted identity"
        );
        assert!(
            matches_modelled_server_arn(candidate),
            "case {case:?}: a near miss must satisfy the modelled ServerArn so it reaches \
             the handler, got {candidate:?}"
        );
        let refused = call_in_region(
            &router,
            REGION,
            "GetHypervisor",
            &json!({ "HypervisorArn": candidate }),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
                refused.str_at("/Message"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("HypervisorNotFound"),
                Some("The hypervisor is not found."),
            ),
            "case {case:?}: an ARN that is not the minted identity must be refused as a \
             missing hypervisor, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.body.pointer("/Hypervisor").is_none(),
            "case {case:?}: a refused read must report no Hypervisor, got {:?}",
            refused.body
        );
    }

    // The identity itself still resolves, so every refusal above is a fact about that ARN
    // and not about the operation or the store.
    let read = call_in_region(
        &router,
        REGION,
        "GetHypervisor",
        &json!({ "HypervisorArn": arn }),
    )
    .await;
    assert_eq!(
        (read.status, read.str_at("/Hypervisor/HypervisorArn")),
        (200, Some(arn.as_str())),
        "the minted identity must still resolve after every near miss was refused, got {} \
         {:?}",
        read.status,
        read.body
    );

    // The bare id this service reports for the record — the `HypervisorId` a gateway
    // associated with it carries — is the id segment of that identity and nothing else, so
    // the two never name different hypervisors.
    let gateway = create(&router, "gw-exact").await;
    let associated = call_in_region(
        &router,
        REGION,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway, "ServerArn": arn }),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "AssociateGatewayToServer must answer 200, got {} {:?}",
        associated.status, associated.body
    );
    let fetched = call_in_region(
        &router,
        REGION,
        "GetGateway",
        &json!({ "GatewayArn": gateway }),
    )
    .await;
    assert_eq!(
        (fetched.status, fetched.str_at("/Gateway/HypervisorId")),
        (200, Some(id.as_str())),
        "the bare HypervisorId must be the id segment of the minted identity, got {} {:?}",
        fetched.status,
        fetched.body
    );
}
