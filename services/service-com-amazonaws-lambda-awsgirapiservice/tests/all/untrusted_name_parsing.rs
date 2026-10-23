// SPDX-License-Identifier: CC0-1.0

//! How this crate reads a `FunctionName` — the UNTRUSTED value almost every Lambda
//! operation binds as a path label.
//!
//! The member admits four documented forms (a bare name, a `name:qualifier`, a partial
//! ARN and a full function ARN, the last three optionally qualified), and all of them
//! name the same record, so every handler reads it through the one parser,
//! `service_com_amazonaws_lambda_awsgirapiservice::state::parse_function_reference`.
//! `capacity_provider_naming.rs` pins the same rule for the other such member.
//!
//! The rule this table pins: the parser answers the record a documented form names,
//! answers `None` for everything else — including every malformed ARN — and NEVER
//! panics, whatever a client sends. It reads no field by index: the ARN form goes through
//! the one shared parser (`service_common::arn`, which walks the six fields with an
//! iterator) and the rest is peeled with `strip_prefix` / `split_once`, so no length
//! check can go stale into an out-of-bounds index.
//!
//! The round-trip rows close the loop with `function_arn`: an ARN this crate writes is
//! one it can read the name back out of, so a function is reachable under every form of
//! its name whatever that name looks like.

use service_com_amazonaws_lambda_awsgirapiservice::state::{
    function_arn, parse_function_reference,
};

#[test]
fn a_function_name_is_read_from_every_documented_form() {
    // (case, the value a client sent, the name it denotes, the qualifier it carries)
    let named: &[(&str, &str, &str, Option<&str>)] = &[
        ("a bare name", "my-function", "my-function", None),
        (
            "a name with an alias",
            "my-function:live",
            "my-function",
            Some("live"),
        ),
        (
            "a name with a version",
            "my-function:2",
            "my-function",
            Some("2"),
        ),
        (
            "the bare resource-type prefix",
            "function:my-function",
            "my-function",
            None,
        ),
        (
            "a partial ARN",
            "000000000000:function:my-function",
            "my-function",
            None,
        ),
        (
            "a qualified partial ARN",
            "000000000000:function:my-function:4",
            "my-function",
            Some("4"),
        ),
        // A function name may itself be twelve digits, and the documented partial ARN
        // always carries the `function:` literal, so this is a qualified name rather
        // than an account id.
        (
            "twelve digits and a version",
            "123456789012:2",
            "123456789012",
            Some("2"),
        ),
        (
            "a function ARN",
            "arn:aws:lambda:us-east-1:000000000000:function:my-function",
            "my-function",
            None,
        ),
        (
            "a function ARN qualified with $LATEST",
            "arn:aws:lambda:us-east-1:000000000000:function:my-function:$LATEST",
            "my-function",
            Some("$LATEST"),
        ),
        (
            "a function ARN in another partition, with no region or account",
            "arn:aws-cn:lambda:::function:my-function:7",
            "my-function",
            Some("7"),
        ),
    ];

    for (case, value, name, qualifier) in named {
        let parsed = parse_function_reference(value);
        let actual = parsed
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.qualifier.as_deref()));
        assert_eq!(
            actual,
            Some((*name, *qualifier)),
            "case {case:?} ({value:?}): expected name {name:?} and qualifier \
             {qualifier:?}, got {actual:?}"
        );
    }

    // Nothing that is no documented form resolves, and nothing panics on the way.
    let refused: &[(&str, &str)] = &[
        ("empty", ""),
        ("a separator alone", ":"),
        ("only separators", ":::::"),
        ("a trailing separator", "my-function:"),
        ("two qualifiers", "my-function:1:2"),
        ("a leading separator", ":my-function"),
        ("the resource type alone", "function:"),
        ("the ARN prefix alone", "arn:"),
        (
            "an ARN of another vendor",
            "arn:aws:s3:us-east-1:000000000000:function:my-function",
        ),
        (
            "an ARN of another resource type",
            "arn:aws:lambda:us-east-1:000000000000:layer:my-layer",
        ),
        (
            "an ARN with no resource type",
            "arn:aws:lambda:us-east-1:000000000000:my-function",
        ),
        (
            "an ARN with an empty partition",
            "arn::lambda:us-east-1:000000000000:function:my-function",
        ),
        ("an ARN with too few fields", "arn:aws:lambda:us-east-1"),
        (
            "an ARN with two qualifiers",
            "arn:aws:lambda:us-east-1:000000000000:function:my-function:1:2",
        ),
    ];

    for (case, value) in refused {
        let actual = parse_function_reference(value);
        assert!(
            actual.is_none(),
            "case {case:?} ({value:?}): expected None, got {actual:?}"
        );
    }

    // The name is read back out of the ARN this crate mints for it, so a function is
    // reachable under every documented form of its name.
    let longest = "n".repeat(64);
    for name in ["my-function", "a", longest.as_str()] {
        let arn = function_arn("eu-west-1", name);
        let parsed = parse_function_reference(&arn);
        let actual = parsed.as_ref().map(|reference| reference.name.as_str());
        assert_eq!(
            actual,
            Some(name),
            "case round trip {name:?}: expected the name back out of {arn}, got {actual:?}"
        );
    }
}
