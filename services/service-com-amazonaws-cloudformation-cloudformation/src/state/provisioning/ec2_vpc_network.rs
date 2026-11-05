// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 route table a stack declares, the subnet association that puts a subnet on
//! it, and the VPC endpoint reached through it — each created in Amazon EC2 itself.
//!
//! ## The rule this module carries
//!
//! A declared resource of a type whose owning service this emulator serves is CREATED in that
//! service, so the physical id the stack publishes for it — the value a `Ref`, an
//! `Fn::GetAtt` and an `Outputs` entry resolve to — is the identifier that service minted,
//! and the resource is readable through that service's own API. A route table the stack only
//! RECORDED would publish an `rtb-…` id `DescribeRouteTables` reports nothing for, a subnet
//! would go on using its VPC's main table, and a gateway endpoint's route would be in no
//! table at all: the deployed network would report success and route nothing.
//!
//! These three types are the rest of the VPC network a deploy-then-operate template declares
//! beside the VPC, the subnet and the security group [`super::super::provisioner`] already
//! creates. They live in their own module beside the algorithm that drives them because that
//! file is far past the length a change to this repository may touch, so a type added now
//! brings its own module rather than growing one that can no longer be edited — the reason
//! [`super::lambda_function`] records. The seam that reaches them is [`super::Owner`].
//!
//! ## What each type's properties are
//!
//! * `AWS::EC2::RouteTable`'s are `CreateRouteTable`'s own `VpcId` plus the tag set, which
//!   reaches EC2 through the one call that tags a resource that exists ([`tag_resource`]).
//!   "Ref returns the route table ID", which EC2 assigns.
//! * `AWS::EC2::SubnetRouteTableAssociation`'s are `AssociateRouteTable`'s own `SubnetId` and
//!   `RouteTableId`. The resource IS the association, and "Ref returns the ID of the subnet
//!   route table association" — the `rtbassoc-…` id that call answers with. It is not a
//!   taggable resource, so no tags are propagated to it.
//! * `AWS::EC2::VPCEndpoint`'s are `CreateVpcEndpoint`'s own members under the same names,
//!   with the tag set again applied by the tagging call, and `PolicyDocument` written as the
//!   JSON text that call takes (a template states it as an object).
//!
//! Each create carries only members of the call it is made with: a declared property this
//! module does not apply is REFUSED ([`refuse_unmapped_properties`]), so a resource is never
//! reported created with less than its declaration asked for.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{call_query, xml_element, xml_elements, xml_scalar_children, Created, Refusal};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource types this module provisions.
pub(super) const EC2_ROUTE_TABLE: &str = "AWS::EC2::RouteTable";
pub(super) const EC2_SUBNET_ROUTE_TABLE_ASSOCIATION: &str =
    "AWS::EC2::SubnetRouteTableAssociation";
pub(super) const EC2_VPC_ENDPOINT: &str = "AWS::EC2::VPCEndpoint";

/// Amazon EC2's signing name and the API version a request names beside the action, from its
/// model (`sigv4Name: ec2`, `version: 2016-11-15`) — the same pair the VPC and subnet arms of
/// [`super::super::provisioner`] call that service with.
const EC2_SIGNING_NAME: &str = "ec2";
const EC2_API_VERSION: &str = "2016-11-15";

/// The suffix of every Amazon EC2 error code that says the resource a request named is not
/// there (`InvalidRouteTableID.NotFound`, `InvalidAssociationID.NotFound`,
/// `InvalidVpcEndpointId.NotFound`): a delete asking for a state that service is already in.
///
/// Judged on the CODE, never on a message, which carries the request's own name.
const NOT_FOUND_SUFFIX: &str = ".NotFound";

/// Which of the three types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`], which is: the algorithm resolves a type's owner once and passes it
/// back to the arms here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::EC2::RouteTable`.
    RouteTable,
    /// `AWS::EC2::SubnetRouteTableAssociation`.
    SubnetRouteTableAssociation,
    /// `AWS::EC2::VPCEndpoint`.
    VpcEndpoint,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        EC2_ROUTE_TABLE => Some(Kind::RouteTable),
        EC2_SUBNET_ROUTE_TABLE_ASSOCIATION => Some(Kind::SubnetRouteTableAssociation),
        EC2_VPC_ENDPOINT => Some(Kind::VpcEndpoint),
        _ => None,
    }
}

/// Every property `AWS::EC2::RouteTable` publishes, from AWS's CloudFormation resource
/// reference for the type.
const ROUTE_TABLE_PROPERTIES: &[&str] = &["Tags", "VpcId"];

/// Every property `AWS::EC2::SubnetRouteTableAssociation` publishes, from the same reference.
const ASSOCIATION_PROPERTIES: &[&str] = &["RouteTableId", "SubnetId"];

/// Every property `AWS::EC2::VPCEndpoint` publishes, from the same reference read against
/// `CreateVpcEndpoint`'s member list
/// (`smithy_query(query="operation",operation="CreateVpcEndpoint")`). `DnsOptions` is not one
/// of them HERE: it is a structure, and this module writes the flat `ec2Query` parameters a
/// scalar and a list take, so a declaration carrying it is refused rather than created
/// without it.
const VPC_ENDPOINT_PROPERTIES: &[&str] = &[
    "IpAddressType",
    "PolicyDocument",
    "PrivateDnsEnabled",
    "ResourceConfigurationArn",
    "RouteTableIds",
    "SecurityGroupIds",
    "ServiceName",
    "ServiceNetworkArn",
    "ServiceRegion",
    "SubnetIds",
    "Tags",
    "VpcEndpointType",
    "VpcId",
];

/// The `AWS::EC2::VPCEndpoint` properties that are `CreateVpcEndpoint`'s own SCALAR members
/// under the same name, each travelling as that member's own `ec2Query` parameter.
const VPC_ENDPOINT_SCALAR_MEMBERS: &[&str] = &[
    "IpAddressType",
    "PrivateDnsEnabled",
    "ResourceConfigurationArn",
    "ServiceName",
    "ServiceNetworkArn",
    "ServiceRegion",
    "VpcEndpointType",
    "VpcId",
];

/// The `AWS::EC2::VPCEndpoint` properties that are LIST members of that call, each against the
/// parameter name `ec2Query` flattens it under — the member's own `xmlName`, numbered from one
/// (`RouteTableId.1`), which is how that protocol spells a list.
const VPC_ENDPOINT_LIST_MEMBERS: &[(&str, &str)] = &[
    ("RouteTableIds", "RouteTableId"),
    ("SecurityGroupIds", "SecurityGroupId"),
    ("SubnetIds", "SubnetId"),
];

/// Create the resource one of these types declares, in Amazon EC2 itself — so a client that
/// deployed the stack and then calls `ec2 describe-route-tables` (or `describe-vpc-endpoints`)
/// with the id the stack published reads it back, and the subnet the stack deployed really
/// uses the routes the template declared.
///
/// `Err` is EC2's own refusal of the create, which is what the resource fails with: a VPC it
/// does not hold, a subnet of another VPC, a table already associated. The resource is never
/// passed off as created on a call that did not answer 2xx.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::RouteTable => create_route_table(ctx, region, resource, propagated).await,
        Kind::SubnetRouteTableAssociation => create_association(ctx, region, resource).await,
        Kind::VpcEndpoint => create_vpc_endpoint(ctx, region, resource, propagated).await,
    }
}

/// Delete the resource a stack created, addressed by the identifier EC2 minted for it — this
/// resource's physical id, which is what `Ref` returns for the type.
///
/// A resource EC2 no longer holds is already in the state the delete asks for
/// ([`NOT_FOUND_SUFFIX`]).
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let id = resource.physical_resource_id.clone();
    match kind {
        Kind::RouteTable => {
            forgiving(
                call_query(
                    ctx,
                    EC2_SIGNING_NAME,
                    region,
                    EC2_API_VERSION,
                    "DeleteRouteTable",
                    &[("RouteTableId".to_string(), id)],
                )
                .await,
            )
            .map(|_| ())
        }
        Kind::SubnetRouteTableAssociation => {
            // "Disassociates a subnet or gateway from a route table" — addressed by the
            // association's own id, which is the resource.
            forgiving(
                call_query(
                    ctx,
                    EC2_SIGNING_NAME,
                    region,
                    EC2_API_VERSION,
                    "DisassociateRouteTable",
                    &[("AssociationId".to_string(), id)],
                )
                .await,
            )
            .map(|_| ())
        }
        Kind::VpcEndpoint => {
            // `DeleteVpcEndpoints` takes a LIST and answers 200 with the endpoints it could
            // not delete, so what says this one went is that answer rather than the status.
            let answered = forgiving(
                call_query(
                    ctx,
                    EC2_SIGNING_NAME,
                    region,
                    EC2_API_VERSION,
                    "DeleteVpcEndpoints",
                    &[("VpcEndpointId.1".to_string(), id)],
                )
                .await,
            )?;
            let Some(document) = answered else {
                return Ok(());
            };
            // "Information about the VPC endpoints that were not successfully deleted"
            // (`Unsuccessful`): each entry carries that endpoint's own error code, and an
            // endpoint EC2 no longer holds is the state this delete asks for.
            for code in xml_elements(&document, "code") {
                if !code.ends_with(NOT_FOUND_SUFFIX) {
                    return Err(Refusal {
                        message: xml_element(&document, "message").unwrap_or_else(|| {
                            format!("Amazon EC2 did not delete the VPC endpoint: {code}")
                        }),
                        code,
                    });
                }
            }
            Ok(())
        }
    }
}

/// Create the route table an `AWS::EC2::RouteTable` declares, in Amazon EC2.
///
/// "Ref returns the route table ID" — an id EC2 ASSIGNS (`rtb-` and 17 hexadecimal
/// characters), which is what this answers as the resource's physical id, so the stack's
/// `Ref`, its outputs and its per-resource reads report an id EC2 really holds a table for.
/// The table comes up with the local route of its VPC, which is EC2's to add.
async fn create_route_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, ROUTE_TABLE_PROPERTIES)?;
    let vpc_id = required(resource, "VpcId")?;
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateRouteTable",
        &[("VpcId".to_string(), vpc_id)],
    )
    .await?;
    // The answer's `<routeTable>` structure, read for the id EC2 assigned: its own direct
    // children only, so the `<routeTableId>` of an association entry nested in it cannot be
    // read as the table's.
    let route_table_id = reported(&answered, "routeTable", "routeTableId")?;
    tag_resource(
        ctx,
        region,
        &route_table_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    // The one attribute the type publishes beside its `Ref`: "Fn::GetAtt RouteTableId — the
    // ID of the route table", which IS the id EC2 assigned.
    let mut attributes = BTreeMap::new();
    attributes.insert("RouteTableId".to_string(), route_table_id.clone());
    Ok(Created {
        physical_resource_id: Some(route_table_id),
        attributes,
    })
}

/// Associate the subnet an `AWS::EC2::SubnetRouteTableAssociation` declares with the route
/// table it declares, in Amazon EC2 — so that subnet really uses that table's routes rather
/// than its VPC's main table.
///
/// Both ids are the ones EC2 minted for the resources of the same template: a deployment
/// creates each resource after the ones it depends on and re-points a reference at the
/// identifier the owning service assigned ([`super::reconcile`]).
async fn create_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, ASSOCIATION_PROPERTIES)?;
    let parameters = vec![
        ("RouteTableId".to_string(), required(resource, "RouteTableId")?),
        ("SubnetId".to_string(), required(resource, "SubnetId")?),
    ];
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "AssociateRouteTable",
        &parameters,
    )
    .await?;
    // "The route table association ID" (`AssociateRouteTableResult$AssociationId`), which is
    // what this resource IS and what its removal addresses the association by.
    let association_id = xml_element(&answered, "associationId")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: "InvalidRequest".to_string(),
            message: "AssociateRouteTable answered no associationId".to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), association_id.clone());
    Ok(Created {
        physical_resource_id: Some(association_id),
        attributes,
    })
}

/// Create the VPC endpoint an `AWS::EC2::VPCEndpoint` declares, in Amazon EC2.
///
/// "Ref returns the ID of the VPC endpoint" — the `vpce-…` id EC2 assigns. A GATEWAY endpoint
/// declared with route tables is created WITH them, which is what puts the endpoint's own
/// route in each of those tables, so the subnets on them reach the service through the
/// endpoint.
async fn create_vpc_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, VPC_ENDPOINT_PROPERTIES)?;
    let mut parameters: Vec<(String, String)> = Vec::new();
    for member in VPC_ENDPOINT_SCALAR_MEMBERS {
        if let Some(value) = scalar(resource, member) {
            parameters.push(((*member).to_string(), value));
        }
    }
    // "A policy to attach to the endpoint that controls access to the service. The policy
    // must be in valid JSON format" (`PolicyDocument`), which a template states as the policy
    // OBJECT itself — so an object travels as its JSON text and a string as it was written.
    if let Some(document) = resource.properties.get("PolicyDocument") {
        if !document.is_null() {
            parameters.push(("PolicyDocument".to_string(), json_text(document)));
        }
    }
    for (property, member) in VPC_ENDPOINT_LIST_MEMBERS {
        for (index, entry) in list(resource, property).iter().enumerate() {
            parameters.push((format!("{member}.{}", index + 1), entry.clone()));
        }
    }
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateVpcEndpoint",
        &parameters,
    )
    .await?;
    let endpoint_id = reported(&answered, "vpcEndpoint", "vpcEndpointId")?;
    tag_resource(
        ctx,
        region,
        &endpoint_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    // The type's published `Fn::GetAtt Id`, which IS the id EC2 assigned. The ones that are
    // LISTS (`DnsEntries`, `NetworkInterfaceIds`) are no single value an attribute carries,
    // so none is invented here.
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), endpoint_id.clone());
    Ok(Created {
        physical_resource_id: Some(endpoint_id),
        attributes,
    })
}

/// Tag one EC2 resource with the set CloudFormation gives it.
///
/// Through `CreateTags` rather than a create's own `TagSpecifications`, for the reason the VPC
/// and subnet arms of [`super::super::provisioner`] use that call: the set carries the three
/// `aws:cloudformation:*` keys, and `CreateTags` is the request EC2 admits them from. Nothing
/// is called for a resource with no tags at all, and a type that is not taggable never
/// reaches here.
async fn tag_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_id: &str,
    tags: &[(String, String)],
) -> Result<(), Refusal> {
    if tags.is_empty() {
        return Ok(());
    }
    let mut parameters = vec![("ResourceId.1".to_string(), resource_id.to_string())];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("Tag.{position}.Key"), key.clone()));
        parameters.push((format!("Tag.{position}.Value"), value.clone()));
    }
    call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateTags",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// A call whose NOT-FOUND refusal is the state it asked for: `Ok(None)` for that case, and the
/// answer document for a call that went through.
fn forgiving(answered: Result<String, Refusal>) -> Result<Option<String>, Refusal> {
    match answered {
        Ok(document) => Ok(Some(document)),
        Err(refusal) if refusal.code.ends_with(NOT_FOUND_SUFFIX) => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One member of the `<parent>` structure an `ec2Query` answer carries, as the non-empty text
/// it holds — the id EC2 assigned.
///
/// Only that structure's own direct children are read ([`xml_scalar_children`]), so a member
/// of the same name nested inside it cannot be mistaken for it.
fn reported(document: &str, parent: &str, member: &str) -> Result<String, Refusal> {
    xml_scalar_children(document, parent)
        .get(member)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Refusal {
            code: "InvalidRequest".to_string(),
            message: format!("Amazon EC2 answered no {member}"),
        })
}

/// The value of the declared property `property`, as the text an `ec2Query` parameter carries:
/// a string as written, and a boolean or a number in its own JSON spelling (`true`, `3`),
/// which is what that protocol's parameters are.
fn scalar(resource: &StoredResource, property: &str) -> Option<String> {
    let value = resource.properties.get(property)?;
    if value.is_null() {
        return None;
    }
    match value.as_str() {
        Some("") => None,
        Some(text) => Some(text.to_string()),
        None => Some(json_text(value)),
    }
}

/// The entries of the declared LIST property `property`, each as that parameter's text. A
/// property that is not a list carries one entry, which is the one value it is.
fn list(resource: &StoredResource, property: &str) -> Vec<String> {
    match resource.properties.get(property) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries
            .iter()
            .filter(|entry| !entry.is_null())
            .map(|entry| entry.as_str().map_or_else(|| json_text(entry), str::to_string))
            .collect(),
        Some(value) => vec![value.as_str().map_or_else(|| json_text(value), str::to_string)],
    }
}

/// One declared value as the text a parameter carries, for a value that is not a string: its
/// JSON spelling, which is what a policy document and a boolean are on the wire.
fn json_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// The declared property `property`, which the create cannot be made without.
///
/// A declaration that states none is REFUSED rather than sent without it, so the resource is
/// never reported created somewhere the template did not ask for.
fn required(resource: &StoredResource, property: &str) -> Result<String, Refusal> {
    scalar(resource, property).ok_or_else(|| Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "{} requires the {property} property",
            resource.resource_type
        ),
    })
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was.
fn refuse_unmapped_properties(
    resource: &StoredResource,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource record of one of these types, carrying `properties` — what the arms above
    /// read, built once for every case below.
    fn declared(resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Declared".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: "rtb-0123456789abcdef0".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
        }
    }

    /// Every member an arm sends is a published property of the type it sends it for.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in VPC_ENDPOINT_SCALAR_MEMBERS {
            assert!(
                VPC_ENDPOINT_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {EC2_VPC_ENDPOINT}, got none"
            );
        }
        for (property, _) in VPC_ENDPOINT_LIST_MEMBERS {
            assert!(
                VPC_ENDPOINT_PROPERTIES.contains(property),
                "case {property:?}: expected the sent list to be a published property of \
                 {EC2_VPC_ENDPOINT}, got none"
            );
        }
    }

    /// Each of the three types is owned by this module, and nothing else is.
    #[test]
    fn each_type_names_its_kind() {
        let cases: &[(&str, Option<Kind>)] = &[
            (EC2_ROUTE_TABLE, Some(Kind::RouteTable)),
            (
                EC2_SUBNET_ROUTE_TABLE_ASSOCIATION,
                Some(Kind::SubnetRouteTableAssociation),
            ),
            (EC2_VPC_ENDPOINT, Some(Kind::VpcEndpoint)),
            ("AWS::EC2::VPC", None),
            ("AWS::EC2::Route", None),
        ];
        for (resource_type, expected) in cases {
            let answered = kind_of(resource_type);
            assert_eq!(
                answered, *expected,
                "case {resource_type:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }

    /// A declaration that states no required property is refused rather than sent without it,
    /// and a property this module does not apply is refused rather than dropped.
    #[test]
    fn an_incomplete_declaration_is_refused() {
        let cases: &[(&str, &str, Value)] = &[
            (
                "a route table with no VPC",
                EC2_ROUTE_TABLE,
                serde_json::json!({}),
            ),
            (
                "an association with no subnet",
                EC2_SUBNET_ROUTE_TABLE_ASSOCIATION,
                serde_json::json!({ "RouteTableId": "rtb-0123456789abcdef0" }),
            ),
        ];
        for (case, resource_type, properties) in cases {
            let resource = declared(resource_type, properties.clone());
            let refused = match kind_of(resource_type) {
                Some(Kind::RouteTable) => required(&resource, "VpcId"),
                _ => required(&resource, "SubnetId"),
            };
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {refused:?}"
            );
        }
        let unmapped = refuse_unmapped_properties(
            &declared(
                EC2_VPC_ENDPOINT,
                serde_json::json!({ "DnsOptions": { "DnsRecordIpType": "ipv4" } }),
            ),
            VPC_ENDPOINT_PROPERTIES,
        );
        assert!(
            unmapped.is_err(),
            "case \"an endpoint with DnsOptions\": expected a refusal naming the property, got \
             {unmapped:?}"
        );
    }

    /// A declared value reaches a parameter as the text that protocol carries: a string as
    /// written, a boolean and a policy object in their JSON spelling, and a list one entry per
    /// parameter.
    #[test]
    fn a_declared_value_is_sent_as_its_parameter_text() {
        let resource = declared(
            EC2_VPC_ENDPOINT,
            serde_json::json!({
                "ServiceName": "com.amazonaws.us-east-1.s3",
                "PrivateDnsEnabled": false,
                "RouteTableIds": ["rtb-0123456789abcdef0", "rtb-00112233445566778"],
                "PolicyDocument": { "Statement": [] },
                "VpcEndpointType": "",
            }),
        );
        let cases: &[(&str, Option<String>)] = &[
            (
                "ServiceName",
                Some("com.amazonaws.us-east-1.s3".to_string()),
            ),
            ("PrivateDnsEnabled", Some("false".to_string())),
            // An empty string is no value: the member is left out rather than sent empty.
            ("VpcEndpointType", None),
            ("IpAddressType", None),
        ];
        for (property, expected) in cases {
            let answered = scalar(&resource, property);
            assert_eq!(
                answered, *expected,
                "case {property:?}: expected {expected:?}, got {answered:?}"
            );
        }
        let tables = list(&resource, "RouteTableIds");
        assert_eq!(
            tables,
            vec![
                "rtb-0123456789abcdef0".to_string(),
                "rtb-00112233445566778".to_string()
            ],
            "case \"RouteTableIds\": expected both declared tables, got {tables:?}"
        );
        let policy = json_text(resource.properties.get("PolicyDocument").unwrap());
        assert_eq!(
            policy, "{\"Statement\":[]}",
            "case \"PolicyDocument\": expected the declared object as JSON text, got {policy:?}"
        );
    }

    /// The id an answer reports is read off the structure's own members, and an answer that
    /// reports none is a refusal rather than an empty id.
    #[test]
    fn the_assigned_id_is_read_off_the_answer() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "a route table",
                "<CreateRouteTableResponse><routeTable><routeTableId>rtb-0a1b2c3d4e5f60718\
                 </routeTableId><associationSet/></routeTable></CreateRouteTableResponse>",
                Some("rtb-0a1b2c3d4e5f60718"),
            ),
            (
                "an answer with no id",
                "<CreateRouteTableResponse><routeTable/></CreateRouteTableResponse>",
                None,
            ),
        ];
        for (case, document, expected) in cases {
            let answered = reported(document, "routeTable", "routeTableId").ok();
            assert_eq!(
                answered.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }
}
