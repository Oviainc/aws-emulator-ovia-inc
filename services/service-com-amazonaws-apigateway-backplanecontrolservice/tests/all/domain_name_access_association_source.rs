// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `accessAssociationSource` a domain name access association is
//! created with: which values name a VPC endpoint and which are refused.
//!
//! The RULE under test, one level above the recorded call:
//!
//! > With `accessAssociationSourceType` `VPCE`, `CreateDomainNameAccessAssociation`'s
//! > `accessAssociationSource` must be the ID of a VPC endpoint — the prefix EC2 mints every
//! > endpoint id under and one of its two hexadecimal widths, in lower case. A value that is
//! > not one is the modelled `BadRequestException` (400) carrying this service's identifier
//! > refusal, and NOTHING is created: no association is stored and the collection still
//! > reports exactly the records the admitted creates made.
//!
//! The rows span what that rule generalises over: each width EC2 mints and one glyph either
//! side of each, the uppercase spelling of an otherwise well-formed id, a non-hexadecimal
//! digit, the id of ANOTHER resource EC2 mints under a longer prefix of the same shape (an
//! endpoint service, an endpoint connection), a bare prefix, an ARN of an endpoint rather
//! than its id, and a value carrying no prefix at all.
//!
//! Every case drives the real wire path through [`Gateway`], this crate's one wire-test
//! harness, and reuses the access-association suite's own create, list and private-domain
//! helpers rather than a second spelling of them.

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::INVALID_ACCESS_ASSOCIATION_SOURCE;

use crate::domain_name_access_associations::{
    association_body, create_association, list_associations, private_domain,
};
use crate::rest_api_lifecycle::{page_items, string_member, Gateway};

const BAD_REQUEST: &str = "BadRequestException";

/// The two hexadecimal widths EC2's VPC endpoint id pattern admits
/// (`^vpce-([0-9a-f]{8}|[0-9a-f]{17})$`), which is what the rule's "well formed" means.
const WIDTHS: &[usize] = &[8, 17];

/// A well-formed id of `width` hexadecimal digits.
fn well_formed(width: usize) -> String {
    let digits: String = (0..width)
        .map(|index| char::from(b"0123456789abcdef"[index % 16]))
        .collect();
    format!("vpce-{digits}")
}

#[tokio::test]
async fn an_access_association_source_must_be_a_vpc_endpoint_id() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "source-forms.example.com").await;

    // Each width EC2 mints is accepted, and each is a DIFFERENT source, so each stores its
    // own association.
    let mut stored = 0usize;
    for width in WIDTHS {
        let source = well_formed(*width);
        let answer = create_association(&gateway, association_body(&domain, &source)).await;
        assert_eq!(
            answer.status, 201,
            "case \"a well-formed id of {width} hexadecimal digits\": expected 201, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["accessAssociationSource"].as_str(),
            Some(source.as_str()),
            "case \"a well-formed id of {width} hexadecimal digits\": expected the source it \
             was created with, got {:?}",
            answer.body
        );
        stored += 1;
    }

    // Rows differ along the FORM of the value that is not a VPC endpoint id.
    let mut refused: Vec<(String, String)> = Vec::new();
    for width in WIDTHS {
        refused.push((
            format!("one digit short of the {width}-digit width"),
            well_formed(width - 1),
        ));
        refused.push((
            format!("one digit past the {width}-digit width"),
            well_formed(width + 1),
        ));
    }
    refused.extend(
        [
            ("the uppercase spelling of a well-formed id", "vpce-0123456789ABCDEF0"),
            ("a digit outside hexadecimal", "vpce-0123456789abcdefg"),
            ("an endpoint SERVICE id", "vpce-svc-0123456789abcdef0"),
            ("an endpoint CONNECTION id", "vpce-conn-0123456789abcdef0"),
            ("the prefix alone", "vpce-"),
            (
                "the ARN of an endpoint rather than its id",
                "arn:aws:ec2:us-east-1:000000000000:vpc-endpoint/vpce-0123456789abcdef0",
            ),
            ("a value carrying no prefix at all", "0123456789abcdef0"),
            ("a value that is not an identifier at all", "not-a-valid-vpce-id"),
            ("a vpc id rather than an endpoint id", "vpc-0123456789abcdef0"),
        ]
        .into_iter()
        .map(|(case, value)| (case.to_string(), value.to_string())),
    );

    for (case, source) in &refused {
        let answer = create_association(&gateway, association_body(&domain, source)).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some(BAD_REQUEST)),
            "case {case:?}: expected 400 {BAD_REQUEST}, got {} {:?} with {:?}",
            answer.status,
            answer.error,
            answer.body
        );
        let message = string_member(&answer.body, "message", case);
        assert_eq!(
            message, INVALID_ACCESS_ASSOCIATION_SOURCE,
            "case {case:?}: expected the identifier refusal, got {message:?}"
        );
    }

    // NOTHING the refusals named was created: the collection still holds exactly the
    // associations the admitted creates made.
    let listed = list_associations(&gateway, "limit=500").await;
    assert_eq!(
        page_items(&listed.body, "GetDomainNameAccessAssociations").len(),
        stored,
        "case \"after every refusal\": expected only the {stored} admitted associations, got \
         {:?}",
        listed.body
    );

    // The source type is what makes the source a VPC endpoint id, so the refusal is stated
    // for that type: a request naming a type outside the modelled enum is refused by the
    // constraint layer in front of the handler instead.
    let unmodelled = create_association(
        &gateway,
        json!({
            "domainNameArn": domain,
            "accessAssociationSourceType": "VPC",
            "accessAssociationSource": "not-a-valid-vpce-id",
        }),
    )
    .await;
    assert_eq!(
        (unmodelled.status, unmodelled.error.as_deref()),
        (400, Some("ValidationException")),
        "case \"a source type outside the modelled enum\": expected 400 ValidationException, \
         got {} {:?}",
        unmodelled.status,
        unmodelled.error
    );
}
