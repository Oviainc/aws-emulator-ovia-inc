// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the ENCODING of a package group's ARN.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees.
//!
//! The rule under test: an identifier this service places in an ARN's resource path is
//! percent-encoded, so only RFC 3986's unreserved characters (`ALPHA / DIGIT / "-" / "."
//! / "_" / "~"`) survive literally inside a path segment, the `/` separators of the path
//! stay literal, and each escape is written with lower-case hex digits. A package group's
//! `PackageGroupPattern` is the one identifier of this service that is not already
//! restricted to those characters — it is modelled len 2..520 `^[^\p{C}\p{IsWhitespace}]+$`
//! (`smithy_query(query="shape", shape="PackageGroupPattern")`), so it carries wildcards
//! (`*`), sigils (`$`, `~`) and anything else a package name admits — and `Arn` is
//! `^\S+$` len 1..1011 (`smithy_query(query="shape", shape="Arn")`).
//!
//! It matters because an ARN is an identifier clients carry elsewhere: an unencoded `*`
//! in a resource path is a wildcard to IAM policy matching, an unencoded `:` is an ARN
//! field separator, and an ARN compared against one obtained from AWS has to match
//! character for character.
//!
//! The expectations below are written out BY HAND, character by character, rather than
//! computed, so this suite pins the encoding rather than restating it. The
//! `crate::harness::package_group_arn` helper the other package-group suites share is the
//! same rule in closed form.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, package_group_arn, Answer, ACCOUNT, REGION};
use crate::package_group_hierarchy::create_package_group;
use crate::resource_tags::{list_tags, tag};

/// Percent-encode one query-string value the way an SDK does, so a pattern carrying `%`,
/// `+`, `?` or `#` reaches the handler as the caller wrote it. `NON_ALPHANUMERIC` is the
/// widest set the maintained encoder offers, which is what a value placed in a query
/// needs.
fn encode_query(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `DescribePackageGroup`, with the pattern encoded for the query string it travels in.
async fn describe(router: &MultiServiceRouter, domain: &str, pattern: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/v1/package-group?domain={domain}&package-group={}",
            encode_query(pattern)
        ),
        None,
    )
    .await
}

/// `GetAssociatedPackageGroup`, with every input in the query string.
async fn associated(
    router: &MultiServiceRouter,
    domain: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/get-associated-package-group?domain={domain}&format={format}&package={}",
        encode_query(package)
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={}", encode_query(namespace)));
    }
    call(router, "GET", &uri, None).await
}

/// The `arn` of an answer's `packageGroup`.
fn arn_of(label: &str, answer: &Answer) -> String {
    answer
        .group()
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label:?}: no packageGroup.arn in {answer:?}"))
        .to_string()
}

/// One pattern and the resource path its ARN must carry after `package-group/<domain>/`.
struct EncodingCase {
    label: &'static str,
    pattern: &'static str,
    /// Written out by hand: the bytes of the pattern, with everything outside the
    /// unreserved set escaped in lower-case hex and the `/` separators kept.
    encoded_path: &'static str,
}

/// The table. Rows differ along what the rule generalises over: WHICH character is
/// encoded (a wildcard, a sigil, an ARN separator, a URI delimiter, the escape character
/// itself, a non-ASCII character spanning two UTF-8 bytes), how MANY characters a segment
/// has to encode, WHERE in the segment they sit, and how DEEP the path is.
const ENCODING_CASES: &[EncodingCase] = &[
    EncodingCase {
        label: "the root group every domain holds: the wildcard alone",
        pattern: "/*",
        encoded_path: "%2a",
    },
    EncodingCase {
        label: "a format-wide wildcard, one segment down",
        pattern: "/npm/*",
        encoded_path: "npm/%2a",
    },
    EncodingCase {
        label: "the exact-match sigil, at the end of a name",
        pattern: "/generic/sim-probe/thing$",
        encoded_path: "generic/sim-probe/thing%24",
    },
    EncodingCase {
        label: "the prefix-match sigil, which is unreserved and stays literal",
        pattern: "/maven/com.example~",
        encoded_path: "maven/com.example~",
    },
    EncodingCase {
        label: "an npm scope sigil, at the start of a segment",
        pattern: "/npm/@scope/*",
        encoded_path: "npm/%40scope/%2a",
    },
    EncodingCase {
        label: "every unreserved punctuation character, all kept literal",
        pattern: "/npm/a-b_c.d~e9/*",
        encoded_path: "npm/a-b_c.d~e9/%2a",
    },
    EncodingCase {
        label: "a colon, which is the ARN's own field separator",
        pattern: "/npm/group:artifact$",
        encoded_path: "npm/group%3aartifact%24",
    },
    EncodingCase {
        label: "the escape character itself",
        pattern: "/npm/50%-off/*",
        encoded_path: "npm/50%25-off/%2a",
    },
    EncodingCase {
        label: "several characters in one segment",
        pattern: "/nuget/a+b,c(d)$",
        encoded_path: "nuget/a%2bb%2cc%28d%29%24",
    },
    EncodingCase {
        label: "the URI delimiters a client would otherwise read as query and fragment",
        pattern: "/generic/a?b#c$",
        encoded_path: "generic/a%3fb%23c%24",
    },
    EncodingCase {
        label: "a non-ASCII name: one escape per UTF-8 byte",
        pattern: "/npm/caffè$",
        encoded_path: "npm/caff%c3%a8%24",
    },
    EncodingCase {
        label: "no terminator at all: nothing to encode",
        pattern: "/pypi/requests",
        encoded_path: "pypi/requests",
    },
];

/// The RULE, over the whole table: the ARN a group reports carries its pattern as a path
/// whose segments are percent-encoded, the same value on the create and on every later
/// read, while the `pattern` member keeps the identifier the caller sent.
#[tokio::test]
async fn a_package_group_arn_percent_encodes_the_pattern_in_its_resource_path() {
    let router = emulator();
    let domain = "arn-encoding";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "setup: the domain must be created before its package groups"
    );

    for case in ENCODING_CASES {
        let label = case.label;
        let expected =
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/{}", case.encoded_path);

        // The root group is created with the domain, so it is read rather than created.
        let answer = if case.pattern == "/*" {
            describe(&router, domain, case.pattern).await
        } else {
            create_package_group(&router, domain, case.pattern, None, None).await
        };
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {answer:?}"
        );

        let minted = arn_of(label, &answer);
        assert_eq!(
            minted, expected,
            "case {label:?}: expected the ARN {expected:?}, got {minted:?}"
        );
        assert_eq!(
            answer.group().get("pattern").and_then(Value::as_str),
            Some(case.pattern),
            "case {label:?}: the pattern member carries the identifier unencoded, got {:?}",
            answer.group()
        );

        // The same value on a later read: the ARN is a property of the stored record, not
        // of the call that answered.
        let described = describe(&router, domain, case.pattern).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: expected 200 from the read, got {described:?}"
        );
        let read_back = arn_of(label, &described);
        assert_eq!(
            read_back, expected,
            "case {label:?}: expected the read to report {expected:?}, got {read_back:?}"
        );

        // The helper the sibling suites share agrees with the hand-written expectation.
        assert_eq!(
            package_group_arn(domain, case.pattern),
            expected,
            "case {label:?}: the shared helper must mint the same ARN"
        );
    }
}

/// The NESTED references of a description carry the same encoding: `parent.arn` and every
/// restriction's `inheritedFrom.arn` name a group, so they are that group's ARN.
#[tokio::test]
async fn the_parent_and_inherited_from_references_carry_the_same_encoding() {
    let router = emulator();
    let domain = "arn-encoding-refs";
    create_domain(&router, domain, None).await;

    // Hand-written: the root group of this domain, and the intermediate group below it.
    let root_arn =
        format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/%2a");
    let format_arn =
        format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/npm/%2a");

    for pattern in ["/npm/*", "/npm/@scope/*"] {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "setup {pattern}: expected 200, got {created:?}"
        );
    }
    // The leaf: an exact pattern under the scope group, so it has a parent two levels
    // below the root and inherits every restriction from the root.
    let leaf = create_package_group(&router, domain, "/npm/@scope/thing$", None, None).await;
    assert_eq!(leaf.status, 200, "the leaf group: expected 200, got {leaf:?}");

    for (case, answer) in [("the create answer", &leaf), ("the read", &describe(&router, domain, "/npm/@scope/thing$").await)] {
        let group = answer.group();
        let own = arn_of(case, answer);
        let expected = format!(
            "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/npm/%40scope/thing%24"
        );
        assert_eq!(
            own, expected,
            "case {case:?}: expected the leaf ARN {expected:?}, got {own:?}"
        );

        let parent = group
            .get("parent")
            .and_then(|parent| parent.get("arn"))
            .and_then(Value::as_str);
        assert_eq!(
            parent,
            Some(
                format!(
                    "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/npm/%40scope/%2a"
                )
                .as_str()
            ),
            "case {case:?}: expected the parent's encoded ARN, got {group:?}"
        );

        // Source: `smithy_query(query="shape", shape="PackageGroupOriginRestrictionType")`
        // → values ["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"].
        for restriction_type in ["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"] {
            let inherited = group
                .get("originConfiguration")
                .and_then(|configuration| configuration.get("restrictions"))
                .and_then(|restrictions| restrictions.get(restriction_type))
                .and_then(|restriction| restriction.get("inheritedFrom"))
                .and_then(|from| from.get("arn"))
                .and_then(Value::as_str);
            assert_eq!(
                inherited,
                Some(root_arn.as_str()),
                "case {case:?}/{restriction_type}: every restriction here resolves at the root group, so inheritedFrom is its encoded ARN, got {group:?}"
            );
        }
    }

    // The intermediate group's own ARN, read directly: the reference above is that record,
    // not a string built for the answer.
    let format_group = describe(&router, domain, "/npm/*").await;
    assert_eq!(
        arn_of("the intermediate group", &format_group),
        format_arn,
        "the intermediate group must report the ARN its children reference"
    );
}

/// One package to resolve, the group it must resolve to, and the resource path that
/// group's ARN must carry.
struct AssociationCase {
    label: &'static str,
    format: &'static str,
    namespace: Option<&'static str>,
    package: &'static str,
    pattern: &'static str,
    encoded_path: &'static str,
}

/// `GetAssociatedPackageGroup` reports the matched group as the record holds it, encoding
/// included — including the root group, which every domain holds.
#[tokio::test]
async fn the_associated_group_reports_the_encoded_arn() {
    let router = emulator();
    let domain = "arn-encoding-assoc";
    create_domain(&router, domain, None).await;
    for pattern in ["/npm/*", "/npm/@scope/*", "/generic/probe/thing$"] {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "setup {pattern}: expected 200, got {created:?}"
        );
    }

    let rows: &[AssociationCase] = &[
        AssociationCase {
            label: "a package no group names: the root group",
            format: "cargo",
            namespace: None,
            package: "serde",
            pattern: "/*",
            encoded_path: "%2a",
        },
        AssociationCase {
            label: "the format-wide wildcard",
            format: "npm",
            namespace: None,
            package: "left-pad",
            pattern: "/npm/*",
            encoded_path: "npm/%2a",
        },
        AssociationCase {
            label: "a scoped namespace, whose segment encodes its sigil",
            format: "npm",
            namespace: Some("@scope"),
            package: "widget",
            pattern: "/npm/@scope/*",
            encoded_path: "npm/%40scope/%2a",
        },
        AssociationCase {
            label: "an exact pattern, whose segment encodes its terminator",
            format: "generic",
            namespace: Some("probe"),
            package: "thing",
            pattern: "/generic/probe/thing$",
            encoded_path: "generic/probe/thing%24",
        },
    ];

    for row in rows {
        let AssociationCase {
            label: case,
            format,
            namespace,
            package,
            pattern,
            encoded_path,
        } = row;
        let answer = associated(&router, domain, format, *namespace, package).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            answer.group().get("pattern").and_then(Value::as_str),
            Some(*pattern),
            "case {case:?}: expected the pattern {pattern}, got {:?}",
            answer.group()
        );
        let expected =
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/{encoded_path}");
        let got = arn_of(case, &answer);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

/// The ARN this service mints ROUND-TRIPS: a client that carries it back into an
/// ARN-addressed operation reaches the group it was minted for, so the encoding is
/// reversed where an ARN is read rather than written.
#[tokio::test]
async fn a_minted_arn_addresses_the_group_it_was_minted_for() {
    let router = emulator();
    let domain = "arn-encoding-tags";
    create_domain(&router, domain, None).await;

    for (case, pattern) in [
        ("a wildcard pattern", "/npm/@scope/*"),
        ("an exact pattern", "/generic/probe/thing$"),
        ("a pattern carrying the escape character", "/npm/50%-off/*"),
    ] {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {created:?}"
        );
        let minted = arn_of(case, &created);

        let tagged = tag(
            &router,
            &minted,
            &[("group".to_string(), pattern.to_string())],
        )
        .await;
        assert_eq!(
            (tagged.status, tagged.code()),
            (200, String::new()),
            "case {case:?}: TagResource on {minted:?} must reach the group, got {tagged:?}"
        );

        let listed = list_tags(&router, &minted).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTagsForResource on {minted:?} must reach the group, got {listed:?}"
        );
        assert_eq!(
            listed.body["tags"],
            json!([{ "key": "group", "value": pattern }]),
            "case {case:?}: expected the tag written through the minted ARN, got {listed:?}"
        );
    }

    // A pattern the domain does not hold is still a not-found when it is addressed by an
    // encoded ARN: decoding resolves an identifier, it does not invent one.
    let absent = format!(
        "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/maven/absent/%2a"
    );
    let listed = list_tags(&router, &absent).await;
    assert_eq!(
        (listed.status, listed.code()),
        (404, "ResourceNotFoundException".to_string()),
        "an encoded ARN naming no group must be a 404, got {listed:?}"
    );
}
