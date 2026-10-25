// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of how Route 53 REPRESENTS a record set: the canonical
//! form of a DNS name, the identity a record set is keyed on, and the zone-level
//! constraints a change batch is refused under.
//!
//! Three rules are under test, each asserted over a SET of inputs rather than on
//! one recorded name:
//!
//! 1. A DNS name is stored and returned in ONE canonical presentation form —
//!    labels folded to lower case, every octet outside `a-z 0-9 - _` written as
//!    its three-digit octal escape code, terminated by the root dot — so every
//!    spelling of a name (escaped or literal, upper or lower case, with or
//!    without the trailing dot) names the SAME record set. The table rows differ
//!    along the octet that needs escaping, the case of the letters, the number of
//!    labels and whether the caller already sent the escaped spelling.
//! 2. A record set is identified by the TRIPLE (name, type, set identifier), and
//!    a refusal naming a record set names all three parts it has.
//!    One name and type are served EITHER by a single simple record set OR by a
//!    group of record sets told apart by their identifiers, never by both at
//!    once.
//! 3. The apex records are a zone-level CARDINALITY constraint — exactly one SOA
//!    and at least one NS record for the zone itself — so a batch is refused for
//!    the state it would leave, not for touching a particular record.
//!
//! Every test drives the service through the real router, using the SAME harness
//! `hosted_zone_records` uses, so what is asserted is the HTTP/restXml behaviour
//! a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, escape_xml, router, Reply};

/// The zone every table below works inside.
const ZONE: &str = "example.com";

/// A `<Change>` element over one record set.
///
/// The members appear in the order the model declares them (`Name`, `Type`,
/// `SetIdentifier`, `Weight`, …, `TTL`, `ResourceRecords`), which is the order
/// the operation's documentation requires of the XML elements.
fn change(
    action: &str,
    name: &str,
    rr_type: &str,
    routing: Option<(&str, i64)>,
    ttl: i64,
    values: &[&str],
) -> String {
    let routing = routing.map_or_else(String::new, |(set_identifier, weight)| {
        format!(
            "<SetIdentifier>{}</SetIdentifier><Weight>{weight}</Weight>",
            escape_xml(set_identifier)
        )
    });
    let records: String = values
        .iter()
        .map(|value| {
            format!(
                "<ResourceRecord><Value>{}</Value></ResourceRecord>",
                escape_xml(value)
            )
        })
        .collect();
    format!(
        "<Change><Action>{action}</Action><ResourceRecordSet><Name>{}</Name><Type>{rr_type}</Type>\
         {routing}<TTL>{ttl}</TTL><ResourceRecords>{records}</ResourceRecords>\
         </ResourceRecordSet></Change>",
        escape_xml(name)
    )
}

/// Submit already-rendered `<Change>` elements as one batch.
async fn submit(router: &MultiServiceRouter, zone_id: &str, changes: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        &crate::hosted_zone_records::change_batch_body(changes, None),
    )
    .await
}

/// List the zone's record sets.
async fn list(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        "",
    )
    .await
}

/// The `Error/Message` of a refusal, or the whole body when there is none.
fn message(reply: &Reply) -> String {
    reply
        .first("Error/Message")
        .unwrap_or(&reply.body)
        .to_string()
}

/// How many of the listed record sets carry `name`.
///
/// `pub(crate)` so a sibling suite in this test target counts a name through
/// the SAME listing call rather than growing a second copy of it.
pub(crate) async fn count_named(router: &MultiServiceRouter, zone_id: &str, name: &str) -> usize {
    list(router, zone_id)
        .await
        .all("ResourceRecordSet/Name")
        .iter()
        .filter(|listed| **listed == name)
        .count()
}

// ---------------------------------------------------------------------------
// 1. The canonical presentation form of a DNS name
// ---------------------------------------------------------------------------

/// One spelling of a record name and the canonical form it must come back in.
struct NameCase {
    /// The `Name` the change batch is written with.
    submitted: &'static str,
    /// The presentation form the service must store and return.
    canonical: &'static str,
}

/// The octets a name may carry literally are `a-z`, `0-9`, `-` and `_`; every
/// other octet is presented as `\` plus its three OCTAL digits (the DNS domain
/// name format `ResourceRecordSet$Name`'s documentation points at — see
/// `smithy_query(query="doc",shape="ResourceRecordSet",member="Name")`). The
/// rows below span that rule: an octet needing no escape, the wildcard `*`
/// (octet 42) the issue cites, three further octets outside the plain set, upper
/// case folding, an already-escaped spelling, and more than one label.
const NAME_CASES: &[NameCase] = &[
    NameCase {
        submitted: "plain.example.com",
        canonical: "plain.example.com.",
    },
    NameCase {
        submitted: "with-hyphen9.example.com",
        canonical: "with-hyphen9.example.com.",
    },
    NameCase {
        // `_` is a plain octet: an SRV or validation label keeps it literally.
        submitted: "_dmarc.example.com",
        canonical: "_dmarc.example.com.",
    },
    NameCase {
        submitted: "*.example.com",
        canonical: "\\052.example.com.",
    },
    NameCase {
        // The escaped spelling is already canonical, so it round-trips unchanged.
        submitted: "\\052.escaped.example.com",
        canonical: "\\052.escaped.example.com.",
    },
    NameCase {
        submitted: "MiXeD.example.com",
        canonical: "mixed.example.com.",
    },
    NameCase {
        submitted: "UPPER.CASE.example.com.",
        canonical: "upper.case.example.com.",
    },
    NameCase {
        // A space is octet 32, `\040`.
        submitted: "two words.example.com",
        canonical: "two\\040words.example.com.",
    },
    NameCase {
        // `@` is octet 64, `\100`, and sits beside plain octets in one label.
        submitted: "at@sign.example.com",
        canonical: "at\\100sign.example.com.",
    },
    NameCase {
        // Case folding and an escape in the SAME label; `*` away from the
        // leftmost position is an ordinary octet, escaped like any other.
        submitted: "Wild*Mix.example.com",
        canonical: "wild\\052mix.example.com.",
    },
    NameCase {
        // `~` is octet 126, `\176` — the top of the printable range.
        submitted: "tilde~.deep.example.com",
        canonical: "tilde\\176.deep.example.com.",
    },
    NameCase {
        // A three-digit code above one octet is no escape code: the backslash
        // is an octet of its own (`\134`) and the digits follow it.
        submitted: "\\777.example.com",
        canonical: "\\134777.example.com.",
    },
    NameCase {
        // A backslash in front of a dot escapes the dot INTO the label, so this
        // is one label `a.b`, presented with the dot as `\056`.
        submitted: "a\\.b.example.com",
        canonical: "a\\056b.example.com.",
    },
];

#[tokio::test]
async fn record_names_are_returned_in_canonical_form() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-canonical-names").await;

    for case in NAME_CASES {
        let label = case.submitted;
        let created = submit(
            &router,
            &zone_id,
            &change("CREATE", case.submitted, "A", None, 300, &["192.0.2.1"]),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected 200, got {} with body {}",
            created.status, created.body
        );

        let listed = list(&router, &zone_id).await;
        let names = listed.all("ResourceRecordSet/Name");
        assert!(
            names.contains(&case.canonical),
            "case {label:?}: expected the canonical name {:?} in the listing, got {names:?}",
            case.canonical
        );
        if case.submitted != case.canonical {
            assert!(
                !names.contains(&case.submitted),
                "case {label:?}: the submitted spelling must not be returned, got {names:?}"
            );
        }

        // The canonical name the service handed back names the same record set:
        // a DELETE written with it removes the record created under the other
        // spelling.
        let deleted = submit(
            &router,
            &zone_id,
            &change("DELETE", case.canonical, "A", None, 300, &["192.0.2.1"]),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: a DELETE naming the canonical form must be accepted, got {} with \
             body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            count_named(&router, &zone_id, case.canonical).await,
            0,
            "case {label:?}: the record set must be gone after the DELETE"
        );
    }
}

/// Two spellings of one name, and the canonical form they share.
struct AliasCase {
    first: &'static str,
    second: &'static str,
    canonical: &'static str,
}

/// Each row writes the same name two ways. The axes are the ones the canonical
/// form generalises over: an escape against the octet it stands for (in both
/// directions), letter case, and the optional trailing dot.
const ALIAS_CASES: &[AliasCase] = &[
    AliasCase {
        first: "*.wild.example.com",
        second: "\\052.wild.example.com",
        canonical: "\\052.wild.example.com.",
    },
    AliasCase {
        first: "\\052.wildtwo.example.com",
        second: "*.wildtwo.example.com",
        canonical: "\\052.wildtwo.example.com.",
    },
    AliasCase {
        first: "MiXeD.example.com",
        second: "mixed.example.com",
        canonical: "mixed.example.com.",
    },
    AliasCase {
        first: "case.example.com",
        second: "CASE.example.com.",
        canonical: "case.example.com.",
    },
    AliasCase {
        first: "two words.example.com",
        second: "two\\040words.example.com",
        canonical: "two\\040words.example.com.",
    },
    AliasCase {
        first: "dot.example.com",
        second: "dot.example.com.",
        canonical: "dot.example.com.",
    },
];

#[tokio::test]
async fn two_spellings_of_one_name_are_one_record_set() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-name-identity").await;

    for case in ALIAS_CASES {
        let label = format!("{:?} then {:?}", case.first, case.second);
        assert_eq!(
            submit(
                &router,
                &zone_id,
                &change("CREATE", case.first, "A", None, 300, &["192.0.2.1"]),
            )
            .await
            .status,
            200,
            "case {label}: the first spelling must create the record set"
        );

        // A CREATE of the other spelling is a DUPLICATE, refused in the full
        // identity of the record set that already holds the name.
        let duplicate = submit(
            &router,
            &zone_id,
            &change("CREATE", case.second, "A", None, 300, &["192.0.2.2"]),
        )
        .await;
        assert_eq!(
            duplicate.status, 400,
            "case {label}: expected 400, got {} with body {}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code(),
            "InvalidChangeBatch",
            "case {label}: expected InvalidChangeBatch, got {}",
            duplicate.body
        );
        let expected = format!(
            "[Tried to create resource record set [name='{}', type='A'] but it already exists]",
            case.canonical
        );
        let actual = message(&duplicate);
        assert_eq!(
            actual, expected,
            "case {label}: expected {expected:?}, got {actual:?}"
        );

        // An UPSERT of the other spelling updates that SAME record set rather
        // than adding a second one.
        assert_eq!(
            submit(
                &router,
                &zone_id,
                &change("UPSERT", case.second, "A", None, 600, &["192.0.2.3"]),
            )
            .await
            .status,
            200,
            "case {label}: the UPSERT must be accepted"
        );
        assert_eq!(
            count_named(&router, &zone_id, case.canonical).await,
            1,
            "case {label}: both spellings must name ONE record set"
        );

        // And a DELETE written with the other spelling removes it.
        let deleted = submit(
            &router,
            &zone_id,
            &change("DELETE", case.second, "A", None, 600, &["192.0.2.3"]),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {label}: expected the DELETE to be accepted, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            count_named(&router, &zone_id, case.canonical).await,
            0,
            "case {label}: the record set must be gone after the DELETE"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Identity is the triple (name, type, set identifier)
// ---------------------------------------------------------------------------

/// A `<Change>` element for an ALIAS record set, which routes to another DNS
/// name instead of carrying record data (so no TTL and no `ResourceRecords`).
fn alias_change(action: &str, name: &str, target: &str) -> String {
    format!(
        "<Change><Action>{action}</Action><ResourceRecordSet><Name>{}</Name><Type>A</Type>\
         <AliasTarget><HostedZoneId>Z2FDTNDATAQYW2</HostedZoneId><DNSName>{}</DNSName>\
         <EvaluateTargetHealth>false</EvaluateTargetHealth></AliasTarget>\
         </ResourceRecordSet></Change>",
        escape_xml(name),
        escape_xml(target)
    )
}

#[tokio::test]
async fn an_alias_targets_dns_name_is_stored_in_canonical_form() {
    // The canonical form is a rule about DNS NAMES, not about one member: the
    // name an alias routes to is held in it too, so it comes back canonical and
    // a DELETE naming the target in any other spelling still matches the stored
    // record set. The rows differ along the spelling axis the rule generalises
    // over: letter case and a missing trailing dot, an already canonical name,
    // and a target carrying an octet outside the literal set.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-alias-canonical").await;

    for (index, (label, submitted, canonical)) in [
        (
            "mixed case with no trailing dot",
            "My-LB-123456.us-east-1.ELB.amazonaws.com",
            "my-lb-123456.us-east-1.elb.amazonaws.com.",
        ),
        (
            "already canonical",
            "d111111abcdef8.cloudfront.net.",
            "d111111abcdef8.cloudfront.net.",
        ),
        (
            "an octet outside the literal set",
            "*.example.net",
            "\\052.example.net.",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("alias{index}.example.com");
        let stored_name = format!("alias{index}.example.com.");
        let created = submit(&router, &zone_id, &alias_change("CREATE", &name, submitted)).await;
        assert_eq!(
            created.status, 200,
            "case {label}: expected 200, got {} with body {}",
            created.status, created.body
        );

        let listed = list(&router, &zone_id).await;
        let targets = listed.all("AliasTarget/DNSName");
        assert!(
            targets.contains(&canonical),
            "case {label}: expected the alias target {canonical:?} in the listing, got {targets:?}"
        );

        // "To delete a resource record set, you must specify all the same values
        // that you specified when you created it" — and the canonical form is
        // what makes the OTHER spelling those same values.
        let deleted = submit(&router, &zone_id, &alias_change("DELETE", &name, canonical)).await;
        assert_eq!(
            deleted.status, 200,
            "case {label}: a DELETE naming the canonical target must be accepted, got {} with \
             body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            count_named(&router, &zone_id, &stored_name).await,
            0,
            "case {label}: the alias record set must be gone after the DELETE"
        );
    }
}

#[tokio::test]
async fn an_escaped_dot_is_one_label_and_not_a_label_boundary() {
    // The rule's other edge: a dot escaped INTO a label is an octet of that
    // label, so a name whose label contains a dot and the name whose labels are
    // split there are DIFFERENT names — they must not collapse onto one record
    // set, however alike their text looks.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-escaped-dot").await;

    for (label, name, canonical, value) in [
        (
            "the dot escaped into one label",
            "a\\.b.example.com",
            "a\\056b.example.com.",
            "192.0.2.50",
        ),
        (
            "the same text split into two labels",
            "b.a.example.com",
            "b.a.example.com.",
            "192.0.2.51",
        ),
    ] {
        let created = submit(
            &router,
            &zone_id,
            &change("CREATE", name, "A", None, 300, &[value]),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {label}: expected 200, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            count_named(&router, &zone_id, canonical).await,
            1,
            "case {label}: expected one record set named {canonical:?}"
        );
    }

    let listed = list(&router, &zone_id).await;
    let names = listed.all("ResourceRecordSet/Name");
    assert!(
        names.contains(&"a\\056b.example.com.") && names.contains(&"b.a.example.com."),
        "both names must exist as separate record sets, got {names:?}"
    );
    assert_eq!(
        listed.all("ResourceRecord/Value").len(),
        // the apex NS record's four values, the SOA's one, and one each for the
        // two record sets created above
        7,
        "neither name may have overwritten the other, got {}",
        listed.body
    );
}

#[tokio::test]
async fn a_refused_change_names_the_whole_record_set_identity() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-identity-triple").await;

    // A weighted group: siblings share name and type and are told apart by
    // their set identifiers, so all three coexist.
    let siblings = ["blue", "green", "canary"];
    let batch: String = siblings
        .iter()
        .enumerate()
        .map(|(index, set_identifier)| {
            change(
                "CREATE",
                "wt.example.com",
                "A",
                Some((set_identifier, 10 * (index as i64 + 1))),
                60,
                &["192.0.2.10"],
            )
        })
        .collect();
    let created = submit(&router, &zone_id, &batch).await;
    assert_eq!(
        created.status, 200,
        "the weighted siblings must be created, got {} with body {}",
        created.status, created.body
    );
    let listed = list(&router, &zone_id).await;
    assert_eq!(
        listed.all("ResourceRecordSet/SetIdentifier"),
        vec!["blue", "canary", "green"],
        "every sibling must round-trip under its own identifier, got {}",
        listed.body
    );

    // Each row is a change refused over one record set; the message names the
    // identity that record set HAS — including the set identifier, for a member
    // of a routing group, and only name and type for a simple record set.
    assert_eq!(
        submit(
            &router,
            &zone_id,
            &change("CREATE", "simple.example.com", "A", None, 60, &["192.0.2.20"]),
        )
        .await
        .status,
        200,
        "the simple record set must be created"
    );
    let cases: [(&str, String, String); 5] = [
        (
            "a duplicate CREATE of a weighted sibling",
            change(
                "CREATE",
                "wt.example.com",
                "A",
                Some(("blue", 99)),
                60,
                &["192.0.2.11"],
            ),
            "[Tried to create resource record set [name='wt.example.com.', type='A', \
             set-identifier='blue'] but it already exists]"
                .to_string(),
        ),
        (
            "a duplicate CREATE of another sibling of the same group",
            change(
                "CREATE",
                "wt.example.com",
                "A",
                Some(("canary", 99)),
                60,
                &["192.0.2.12"],
            ),
            "[Tried to create resource record set [name='wt.example.com.', type='A', \
             set-identifier='canary'] but it already exists]"
                .to_string(),
        ),
        (
            "a DELETE of a sibling identifier the group does not hold",
            change(
                "DELETE",
                "wt.example.com",
                "A",
                Some(("red", 10)),
                60,
                &["192.0.2.10"],
            ),
            "[Tried to delete resource record set [name='wt.example.com.', type='A', \
             set-identifier='red'] but it was not found]"
                .to_string(),
        ),
        (
            "a duplicate CREATE of a simple record set, which has no identifier",
            change("CREATE", "simple.example.com", "A", None, 60, &["192.0.2.21"]),
            "[Tried to create resource record set [name='simple.example.com.', type='A'] but it \
             already exists]"
                .to_string(),
        ),
        (
            "a duplicate CREATE under a non-canonical spelling of the name",
            change(
                "CREATE",
                "WT.example.com",
                "A",
                Some(("green", 99)),
                60,
                &["192.0.2.13"],
            ),
            "[Tried to create resource record set [name='wt.example.com.', type='A', \
             set-identifier='green'] but it already exists]"
                .to_string(),
        ),
    ];

    for (label, batch, expected) in cases {
        let refused = submit(&router, &zone_id, &batch).await;
        assert_eq!(
            refused.status, 400,
            "case {label}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidChangeBatch",
            "case {label}: expected InvalidChangeBatch, got {}",
            refused.body
        );
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {label}: expected {expected:?}, got {actual:?}"
        );
    }

    // None of the refusals touched the group: every sibling kept its weight.
    let after = list(&router, &zone_id).await;
    assert_eq!(
        after.all("ResourceRecordSet/SetIdentifier"),
        vec!["blue", "canary", "green"],
        "a refused batch must leave the group as it was, got {}",
        after.body
    );
    assert_eq!(
        after.all("ResourceRecordSet/Weight"),
        vec!["10", "30", "20"],
        "a refused batch must leave each sibling's weight as it was, got {}",
        after.body
    );
}

// ---------------------------------------------------------------------------
// 2b. One name and type are served by one routing policy
// ---------------------------------------------------------------------------

/// One name and type, the routing of the record set created there first, and the
/// routing that may not join it.
struct RoutingCase {
    label: &'static str,
    name: &'static str,
    canonical: &'static str,
    rr_type: &'static str,
    /// A record data value that suits `rr_type`.
    value: &'static str,
    /// `None` for a simple record set, `Some((identifier, weight))` for a
    /// member of a routing group.
    seeded: Option<(&'static str, i64)>,
    /// The routing the SAME name and type may not also hold.
    conflicting: Option<(&'static str, i64)>,
    /// A type the conflicting record set IS free to take at that name, because
    /// the constraint is per name AND type.
    free_type: &'static str,
    /// A record data value that suits `free_type`, since a value is parsed
    /// against the syntax of the type it is submitted for.
    free_value: &'static str,
}

/// The rows span the rule's axes: which routing was there first, the record
/// type, and the action the conflicting record set is written with (each row is
/// tried as a CREATE and as an UPSERT).
const ROUTING_CASES: &[RoutingCase] = &[
    RoutingCase {
        label: "a weighted sibling joining a simple record set",
        name: "simple-first.example.com",
        canonical: "simple-first.example.com.",
        rr_type: "A",
        value: "192.0.2.30",
        seeded: None,
        conflicting: Some(("blue", 10)),
        free_type: "AAAA",
        free_value: "2001:db8::30",
    },
    RoutingCase {
        label: "a simple record set joining a weighted group",
        name: "group-first.example.com",
        canonical: "group-first.example.com.",
        rr_type: "A",
        value: "192.0.2.31",
        seeded: Some(("blue", 10)),
        conflicting: None,
        free_type: "TXT",
        free_value: "\"group-first\"",
    },
    RoutingCase {
        label: "a simple TXT record joining a weighted TXT group",
        name: "txt-group.example.com",
        canonical: "txt-group.example.com.",
        rr_type: "TXT",
        value: "\"owned\"",
        seeded: Some(("green", 20)),
        conflicting: None,
        free_type: "MX",
        free_value: "10 mail.example.net.",
    },
    RoutingCase {
        label: "a weighted sibling joining a simple AAAA record set",
        name: "v6-simple.example.com",
        canonical: "v6-simple.example.com.",
        rr_type: "AAAA",
        value: "2001:db8::1",
        seeded: None,
        conflicting: Some(("canary", 30)),
        free_type: "A",
        free_value: "192.0.2.33",
    },
];

#[tokio::test]
async fn one_name_and_type_hold_either_a_simple_record_set_or_a_routing_group() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-routing-exclusive").await;

    for case in ROUTING_CASES {
        let label = case.label;
        assert_eq!(
            submit(
                &router,
                &zone_id,
                &change(
                    "CREATE",
                    case.name,
                    case.rr_type,
                    case.seeded,
                    300,
                    &[case.value],
                ),
            )
            .await
            .status,
            200,
            "case {label}: the first record set must be created"
        );

        // Each direction of the mix is refused in its own wording: a record set
        // with a routing policy conflicts with what the name and type already
        // answer, while a simple record set cannot be created at a name and
        // type other record sets already hold.
        let expected = if case.conflicting.is_some() {
            format!(
                "[RRSet of type {} with DNS name {} is not permitted as it conflicts with other \
                 records with the same DNS name and type in zone {ZONE}.]",
                case.rr_type, case.canonical
            )
        } else {
            format!(
                "[RRSet with DNS name {}, type {} cannot be created as other RRSets exist with \
                 the same name and type.]",
                case.canonical, case.rr_type
            )
        };
        for action in ["CREATE", "UPSERT"] {
            let refused = submit(
                &router,
                &zone_id,
                &change(
                    action,
                    case.name,
                    case.rr_type,
                    case.conflicting,
                    300,
                    &[case.value],
                ),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {label} ({action}): expected 400, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "InvalidChangeBatch",
                "case {label} ({action}): expected InvalidChangeBatch, got {}",
                refused.body
            );
            let actual = message(&refused);
            assert_eq!(
                actual, expected,
                "case {label} ({action}): expected {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            count_named(&router, &zone_id, case.canonical).await,
            1,
            "case {label}: the refusals must leave the one record set that was there"
        );

        // The constraint is per name AND type: the same routing is free to take
        // another type at that very name.
        let other_type = submit(
            &router,
            &zone_id,
            &change(
                "CREATE",
                case.name,
                case.free_type,
                case.conflicting,
                300,
                &[case.free_value],
            ),
        )
        .await;
        assert_eq!(
            other_type.status, 200,
            "case {label}: the same routing at type {} must be accepted, got {} with body {}",
            case.free_type, other_type.status, other_type.body
        );
        assert_eq!(
            count_named(&router, &zone_id, case.canonical).await,
            2,
            "case {label}: the zone must now hold both types at that name"
        );
    }

    // Replacing one routing with the other IN ONE BATCH is accepted whatever
    // order the changes are written in: the rule is about the state the batch
    // would leave, not about the order it is applied in.
    for (label, name, canonical, reversed) in [
        (
            "the DELETE written before the CREATE",
            "swap-a.example.com",
            "swap-a.example.com.",
            false,
        ),
        (
            "the CREATE written before the DELETE",
            "swap-b.example.com",
            "swap-b.example.com.",
            true,
        ),
    ] {
        assert_eq!(
            submit(
                &router,
                &zone_id,
                &change("CREATE", name, "A", None, 300, &["192.0.2.40"]),
            )
            .await
            .status,
            200,
            "case {label}: the simple record set must be created"
        );
        let drop_simple = change("DELETE", name, "A", None, 300, &["192.0.2.40"]);
        let add_member = change("CREATE", name, "A", Some(("blue", 10)), 300, &["192.0.2.41"]);
        let batch = if reversed {
            format!("{add_member}{drop_simple}")
        } else {
            format!("{drop_simple}{add_member}")
        };
        let swapped = submit(&router, &zone_id, &batch).await;
        assert_eq!(
            swapped.status, 200,
            "case {label}: expected 200, got {} with body {}",
            swapped.status, swapped.body
        );
        assert_eq!(
            count_named(&router, &zone_id, canonical).await,
            1,
            "case {label}: the name must hold just the group member that replaced it"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. The apex records are a zone-level cardinality constraint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_apex_records_are_refused_by_zone_cardinality() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-apex-cardinality").await;

    // The apex records as the zone holds them: the NS record's four name
    // servers, then the SOA record's single value.
    let stored = list(&router, &zone_id).await;
    let values = stored.all("ResourceRecord/Value");
    let ns_values: Vec<&str> = values[..4].to_vec();
    let soa_value = values[4];

    // Removing either leaves a zone that violates a CARDINALITY rule, and the
    // refusal states the rule rather than the record it protects.
    for (label, batch, expected) in [
        (
            "the apex SOA record",
            change("DELETE", ZONE, "SOA", None, 900, &[soa_value]),
            "[A HostedZone must contain exactly one SOA record.]",
        ),
        (
            "the apex NS record",
            change("DELETE", ZONE, "NS", None, 172_800, &ns_values),
            "[A HostedZone must contain at least one NS record for the zone itself.]",
        ),
        (
            "the apex SOA record named in upper case",
            change("DELETE", "EXAMPLE.COM.", "SOA", None, 900, &[soa_value]),
            "[A HostedZone must contain exactly one SOA record.]",
        ),
    ] {
        let refused = submit(&router, &zone_id, &batch).await;
        assert_eq!(
            refused.status, 400,
            "case {label}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidChangeBatch",
            "case {label}: expected InvalidChangeBatch, got {}",
            refused.body
        );
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {label}: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            list(&router, &zone_id).await.all("ResourceRecordSet/Type"),
            vec!["NS", "SOA"],
            "case {label}: the zone's apex records must still be there"
        );
    }

    // It IS a cardinality rule: a batch that replaces an apex record leaves the
    // zone with exactly one SOA and an NS record, so it is accepted.
    let replacement_soa = format!("{} 2 7200 900 1209600 86400", ns_values[0]);
    for (label, batch) in [
        (
            "an UPSERT of the apex SOA",
            change("UPSERT", ZONE, "SOA", None, 300, &[&replacement_soa]),
        ),
        (
            "a DELETE of the apex NS followed by a CREATE of it in the same batch",
            format!(
                "{}{}",
                change("DELETE", ZONE, "NS", None, 172_800, &ns_values),
                change("CREATE", ZONE, "NS", None, 3_600, &ns_values),
            ),
        ),
    ] {
        let accepted = submit(&router, &zone_id, &batch).await;
        assert_eq!(
            accepted.status, 200,
            "case {label}: expected 200, got {} with body {}",
            accepted.status, accepted.body
        );
    }

    let after = list(&router, &zone_id).await;
    assert_eq!(
        after.all("ResourceRecordSet/Type"),
        vec!["NS", "SOA"],
        "the zone must still hold exactly its two apex record sets, got {}",
        after.body
    );
    assert_eq!(
        after.all("ResourceRecordSet/TTL"),
        vec!["3600", "300"],
        "the replaced apex records must report their new TTLs, got {}",
        after.body
    );
    assert_eq!(
        after.all("ResourceRecord/Value").last(),
        Some(&replacement_soa.as_str()),
        "the replaced SOA must report its new value, got {}",
        after.body
    );
}
