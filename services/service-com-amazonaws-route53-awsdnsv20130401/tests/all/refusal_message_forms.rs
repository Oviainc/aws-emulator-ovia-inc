// SPDX-License-Identifier: CC0-1.0

//! The FORM of three more Route 53 refusal messages, end to end over the
//! router.
//!
//! Each of these is a sentence built to a rule rather than a sentence about one
//! request, so each is asserted over a table spanning what its rule generalises
//! over:
//!
//! * a `CreateHostedZone` whose `Name` spells a single label asks for a
//!   top-level domain, a level of the DNS namespace that is reserved rather
//!   than delegable to an account, so it is `InvalidDomainName` (HTTP 400)
//!   whose message is the requested name echoed VERBATIM followed by the fixed
//!   text ` is reserved by AWS!` and nothing else — whatever the name is
//!   spelled with, and before any other judgement about the name's spelling;
//! * an `AssociateVPCWithHostedZone` against a public hosted zone is
//!   `PublicZoneVPCAssociation` (HTTP 400) whose message names the two
//!   resources the refused association was between, each by its ID: the hosted
//!   zone by the id the request addressed (bare, however the request spelled
//!   it) and the VPC by the id the request body named;
//! * the record-type constraint of a traffic policy document is reported by ONE
//!   clause covering both ways it can be broken — absent, or naming something
//!   that is not a record type a traffic policy can create — and that clause is
//!   reported LAST, after the endpoint and entry-point clauses, whichever
//!   combination of constraints the document breaks and whichever operation
//!   takes the document.
//!
//! The harness (`router`, `call`, `Reply`) and every request builder come from
//! the suites that own those operations, so these tests drive the service
//! through the same router a socket-served request goes through.

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::TRAFFIC_POLICY_FORMAT_VERSION;

use crate::hosted_zone_records::router;
use crate::refusal_message_rules::{message_of, ENTRY_POINT, NO_ENDPOINT, RECORD_TYPE};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, policy_of, policy_count,
};
use crate::traffic_policy_versions::create_version;
use crate::vpc_associations::{associate, bare_zone_id, create_zone, vpc_block, HOME_REGION};

// ---------------------------------------------------------------------------
// A hosted zone name the service reserves
// ---------------------------------------------------------------------------

/// The FIXED half of the refusal a reserved hosted zone name is answered with
/// (`crate::ops::create_hosted_zone`'s `RESERVED_NAME_FIXED_TEXT`): the whole
/// message is the requested name followed by this and nothing else.
const RESERVED_SUFFIX: &str = " is reserved by AWS!";

#[tokio::test]
async fn a_name_the_service_reserves_is_refused_as_reserved() {
    // Every row is a name of ONE label, which is what makes it a top-level
    // domain, and the rows differ along everything else a name can differ
    // along: an existing TLD and a label no registry carries, the label's
    // length (one octet up to the 63-octet maximum of the documented
    // domain-name format), its character class (letters, digits, hyphens,
    // underscores and punctuation outside the plain DNS set), its letter case,
    // and whether the request already wrote the trailing root dot. The message
    // echoes the name as the REQUEST wrote it, so a row whose canonical form
    // differs from its spelling (upper case, a trailing dot) fails for a
    // message built from the stored form.
    let longest_label = "l".repeat(63);
    let names = [
        "com",
        "io",
        "x",
        "42",
        "localhost",
        "COM",
        "Example-TLD",
        "org.",
        "under_score",
        "reserved_name_??",
        longest_label.as_str(),
    ];

    let router = router();
    for (index, name) in names.iter().enumerate() {
        let reply = create_zone(&router, name, &format!("reserved-{index}"), "").await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidDomainName",
            "case {name:?}: expected InvalidDomainName, got body {}",
            reply.body
        );
        let got = message_of(&reply, name);
        let expected = format!("{name}{RESERVED_SUFFIX}");
        assert_eq!(
            got, expected,
            "case {name:?}: expected message {expected:?}, got {got:?}"
        );
        // The two halves of the rule, asserted apart from the whole: the fixed
        // text is a SUFFIX, and the variable part is the name verbatim.
        assert!(
            got.ends_with(RESERVED_SUFFIX) && got.starts_with(name),
            "case {name:?}: the message must be the requested name followed by \
             {RESERVED_SUFFIX:?}, got {got:?}"
        );
    }

    // The control: a name of two labels or more is not in the reserved level of
    // the namespace, so it is created rather than refused — including names
    // whose first label is one of the reserved spellings above.
    let accepted = [
        "example.com",
        "com.example.net",
        "Example-TLD.example.org",
        "under_score.example.com.",
    ];
    for (index, name) in accepted.iter().enumerate() {
        let reply = create_zone(&router, name, &format!("delegable-{index}"), "").await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: a name of more than one label must be created, got {} with body {}",
            reply.status, reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// A VPC association against a public hosted zone
// ---------------------------------------------------------------------------

/// One refused association: the zone, how the request spells its id, and the
/// VPC named in the body.
struct PublicZoneRow {
    case: &'static str,
    /// The zone's DNS name, which the refusal must NOT be built from.
    zone_name: &'static str,
    /// The `HostedZoneConfig` block the zone is created with — every one of
    /// them leaves the zone public.
    config: &'static str,
    /// How the association request spells the zone id: the bare id, or the
    /// `/hostedzone/`-prefixed form a response hands back (URI-encoded).
    prefixed: bool,
    vpc_id: &'static str,
    region: &'static str,
}

#[tokio::test]
async fn a_public_zone_association_names_the_zone_id_and_the_vpc() {
    // The rows differ along how the zone came to be public, the spelling the
    // request addresses its id with, the VPC id form (the 8- and 17-character
    // forms) and the region named — a value of the modelled `VPCRegion` enum
    // (smithy_query(query="shape",shape="VPCRegion")) — so a message built from
    // the zone's name, or carrying no VPC, fails every row.
    let rows = [
        PublicZoneRow {
            case: "no HostedZoneConfig at all, addressed by its bare id",
            zone_name: "public-text-one.example.com",
            config: "",
            prefixed: false,
            vpc_id: "vpc-1a2b3c4d",
            region: HOME_REGION,
        },
        PublicZoneRow {
            case: "PrivateZone sent explicitly false, addressed by its bare id",
            zone_name: "public-text-two.example.org",
            config: "<HostedZoneConfig><PrivateZone>false</PrivateZone></HostedZoneConfig>",
            prefixed: false,
            vpc_id: "vpc-0a1b2c3d4e5f60718",
            region: "eu-west-1",
        },
        PublicZoneRow {
            case: "an empty HostedZoneConfig, addressed by the prefixed id",
            zone_name: "public-text-three.example.net.",
            config: "<HostedZoneConfig></HostedZoneConfig>",
            prefixed: true,
            vpc_id: "vpc-feedface",
            region: "us-gov-west-1",
        },
        PublicZoneRow {
            case: "a comment-only config, addressed by the prefixed id",
            zone_name: "public-text-four.example.com",
            config: "<HostedZoneConfig><Comment>public on purpose</Comment></HostedZoneConfig>",
            prefixed: true,
            vpc_id: "vpc-1122334455667788a",
            region: "ap-southeast-2",
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let created = create_zone(
            &router,
            row.zone_name,
            &format!("public-text-{index}"),
            row.config,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
            row.case, created.status, created.body
        );
        let zone_id = bare_zone_id(&created);
        let addressed = if row.prefixed {
            format!("%2Fhostedzone%2F{zone_id}")
        } else {
            zone_id.clone()
        };

        let reply = associate(
            &router,
            &addressed,
            &vpc_block(Some(row.vpc_id), Some(row.region)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "PublicZoneVPCAssociation",
            "case {:?}: expected PublicZoneVPCAssociation, got body {}",
            row.case,
            reply.body
        );

        let got = message_of(&reply, row.case);
        let expected = format!(
            "Attempting to associate public zone: {zone_id} with vpc: {}",
            row.vpc_id
        );
        assert_eq!(
            got, expected,
            "case {:?}: expected message {expected:?}, got {got:?}",
            row.case
        );
        // The zone is named by its ID, not by the DNS name the caller already
        // knows, and the VPC the request named is part of the refusal.
        assert!(
            !got.contains(row.zone_name.trim_end_matches('.')),
            "case {:?}: the message must name the zone id {zone_id} rather than the name {}, got \
             {got:?}",
            row.case,
            row.zone_name
        );
        assert!(
            got.contains(row.vpc_id),
            "case {:?}: the message must name the VPC {}, got {got:?}",
            row.case,
            row.vpc_id
        );
    }
}

// ---------------------------------------------------------------------------
// The traffic policy document's record-type clause
// ---------------------------------------------------------------------------

/// One document to submit, and every clause its refusal must carry, in order.
struct ClauseRow {
    case: &'static str,
    document: String,
    clauses: Vec<&'static str>,
}

/// A document declaring `record_type` as its `RecordType`, written into the
/// document exactly as given — so a row can declare a value that is not even a
/// JSON string.
fn document_with_raw_record_type(record_type: &str) -> String {
    format!(
        "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
         \"RecordType\":{record_type},\"StartEndpoint\":\"only\",\
         \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}}}}"
    )
}

#[tokio::test]
async fn the_record_type_clause_covers_both_faults_and_is_reported_last() {
    // The rows differ along WHY the record type is unusable (absent, a value
    // outside the record types a policy can create, a value that is not a JSON
    // string at all) and along how many OTHER constraints the same document
    // breaks (none, one, two) — so a message that worded the two faults
    // differently, or that reported the clause before the others, fails.
    let rows = vec![
        ClauseRow {
            case: "no RecordType at all",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"StartEndpoint\":\"only\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}}}}"
            ),
            clauses: vec![RECORD_TYPE],
        },
        ClauseRow {
            case: "a RecordType outside the record types a policy can create",
            document: document_with_start_endpoint("NOTATYPE", "192.0.2.1"),
            clauses: vec![RECORD_TYPE],
        },
        ClauseRow {
            case: "a lower-case spelling of a supported record type",
            document: document_with_start_endpoint("aaaa", "2001:db8::1"),
            clauses: vec![RECORD_TYPE],
        },
        ClauseRow {
            case: "a RecordType that is a number rather than a string",
            document: document_with_raw_record_type("7"),
            clauses: vec![RECORD_TYPE],
        },
        ClauseRow {
            case: "a RecordType that is JSON null",
            document: document_with_raw_record_type("null"),
            clauses: vec![RECORD_TYPE],
        },
        ClauseRow {
            case: "no RecordType and no endpoints",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"StartEndpoint\":\"only\"}}"
            ),
            clauses: vec![NO_ENDPOINT, RECORD_TYPE],
        },
        ClauseRow {
            case: "an unsupported RecordType, no endpoints and no entry point",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"NOTATYPE\"}}"
            ),
            clauses: vec![NO_ENDPOINT, ENTRY_POINT, RECORD_TYPE],
        },
        ClauseRow {
            case: "no RecordType, an empty Endpoints block and both entry points",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"Endpoints\":{{}},\"StartEndpoint\":\"first\",\"StartRule\":\"second\"}}"
            ),
            clauses: vec![
                NO_ENDPOINT,
                ENTRY_POINT,
                "StartEndpoint first does not reference a declared endpoint.",
                "StartRule second does not reference a declared rule.",
                RECORD_TYPE,
            ],
        },
    ];

    let router = router();
    // The policy whose further VERSIONS take the same documents: the version
    // create runs the same document validator, so every row is asserted through
    // both operations that take a document.
    let base = create_policy(
        &router,
        "record-type-clause-base",
        // An entry of `RRType` the refusal rows do not name
        // (service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES).
        &document_with_start_endpoint("MX", "10 mail.example.com"),
        None,
    )
    .await;
    assert_eq!(
        base.status, 201,
        "the base policy must be created, got {} with body {}",
        base.status, base.body
    );
    let base_id = policy_of(&base, "the base policy").id;

    for row in &rows {
        let expected = row.clauses.join(";");
        let from_create = create_policy(&router, row.case, &row.document, None).await;
        let from_version = create_version(&router, &base_id, &row.document, None).await;
        for (operation, reply) in [
            ("CreateTrafficPolicy", &from_create),
            ("CreateTrafficPolicyVersion", &from_version),
        ] {
            let case = format!("{operation}: {}", row.case);
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidTrafficPolicyDocument",
                "case {case:?}: expected InvalidTrafficPolicyDocument, got body {}",
                reply.body
            );
            let got = message_of(reply, &case);
            assert_eq!(
                got, expected,
                "case {case:?}: expected message {expected:?}, got {got:?}"
            );
            // The clause is the LAST one, whatever else the document earned.
            let clauses: Vec<&str> = got.split(';').collect();
            assert_eq!(
                got.split(';').next_back(),
                Some(RECORD_TYPE),
                "case {case:?}: the record-type clause must be reported last, got {clauses:?}"
            );
        }
    }

    // The control: a document declaring a record type the service supports
    // earns no clause at all, so it is created. Every entry of the modelled
    // `RRType` enum is one such type.
    for record_type in R_R_TYPE_VALUES {
        let reply = create_version(
            &router,
            &base_id,
            &document_with_start_endpoint(record_type, "192.0.2.1"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {record_type:?}: a supported record type must be accepted, got {} with body {}",
            reply.status, reply.body
        );
    }
    // The refused documents persisted nothing: one policy, the base one.
    let count = policy_count(&router).await;
    assert_eq!(
        count, "1",
        "a refused document must persist no policy, got Count {count:?}"
    );
}
