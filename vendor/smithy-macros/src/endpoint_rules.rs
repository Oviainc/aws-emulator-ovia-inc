// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Host addressing read off a service's endpoint rule set.
//!
//! A model that carries `smithy.rules#endpointRuleSet` states, as URL templates, every
//! address a client builds. The ones that put a request parameter in front of the host
//! (`https://{Bucket}.s3.{Region}.{partitionResult#dnsSuffix}`) are virtual-hosted
//! addresses. This module derives the host templates of those, for the one parameter the
//! operations take as the leading label of their path, so a router can move that label
//! from the host back into the path the operations are modeled on.
//!
//! Only templates the router can recognize are kept: after the leading parameter, the host
//! may use literal labels, the Region (a parameter bound to the `AWS::Region` built-in) and
//! the partition DNS suffix, last. A template that needs anything else (S3 Express's
//! availability zone, an Outposts or access point name, a customer's own endpoint) names
//! an address of another kind and is left out. So is a template whose auth scheme signs
//! under another service's name (`s3-outposts`, `s3express`): a request to it is not this
//! service's.

use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

const RULE_SET: &str = "smithy.rules#endpointRuleSet";
const CONTEXT_PARAM: &str = "smithy.rules#contextParam";
const HTTP: &str = "smithy.api#http";
const HTTP_LABEL: &str = "smithy.api#httpLabel";
const SIGV4: &str = "aws.auth#sigv4";

/// The host templates (the part of a virtual-hosted host after the bucket), sorted and
/// without duplicates, as `s3.{region}.{dnsSuffix}`: literal labels, `{region}`, and
/// `{dnsSuffix}` last. Empty when the model has no endpoint rule set, or none of its
/// templates are of this kind.
pub(crate) fn virtual_host_templates(
    shapes: &HashMap<String, Value>,
    service_traits: Option<&serde_json::Map<String, Value>>,
) -> Vec<String> {
    let Some(rule_set) = service_traits.and_then(|traits| traits.get(RULE_SET)) else {
        return Vec::new();
    };
    let parameters = rule_set.get("parameters").and_then(Value::as_object);
    let region_params: BTreeSet<&str> = parameters
        .into_iter()
        .flatten()
        .filter(|(_, spec)| spec.get("builtIn").and_then(Value::as_str) == Some("AWS::Region"))
        .map(|(name, _)| name.as_str())
        .collect();

    let signing_name = service_traits
        .and_then(|traits| traits.get(SIGV4))
        .and_then(|sigv4| sigv4.get("name"))
        .and_then(Value::as_str);

    let mut templates = BTreeSet::new();
    let mut endpoints = Vec::new();
    endpoint_urls(rule_set, &mut endpoints);
    let urls: Vec<&String> = endpoints
        .iter()
        .filter(|(_, signs_as)| signing_name.is_none_or(|own| signs_as.iter().all(|n| n == own)))
        .map(|(url, _)| url)
        .collect();
    for param in leading_path_params(shapes) {
        if !parameters.is_some_and(|p| p.contains_key(&param)) {
            continue;
        }
        let lead = format!("{{{param}}}.");
        for url in &urls {
            let Some(host) = url
                .strip_prefix("https://")
                .map(|rest| rest.split('/').next().unwrap_or(rest))
                .and_then(|host| host.strip_prefix(lead.as_str()))
            else {
                continue;
            };
            if let Some(template) = normalize(host, &region_params) {
                templates.insert(template);
            }
        }
    }
    templates.into_iter().collect()
}

/// Every endpoint URL template in a rule set, wherever it nests, with the signing names its
/// auth schemes name (none when it states no auth scheme).
fn endpoint_urls(node: &Value, out: &mut Vec<(String, Vec<String>)>) {
    match node {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("endpoint") {
                let endpoint = map.get("endpoint");
                if let Some(url) = endpoint.and_then(|e| e.get("url")).and_then(Value::as_str) {
                    let signs_as = endpoint
                        .and_then(|e| e.get("properties"))
                        .and_then(|p| p.get("authSchemes"))
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|scheme| scheme.get("signingName").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect();
                    out.push((url.to_string(), signs_as));
                }
            }
            map.values().for_each(|child| endpoint_urls(child, out));
        }
        Value::Array(items) => items.iter().for_each(|child| endpoint_urls(child, out)),
        _ => {}
    }
}

/// The endpoint parameters that the operations bind (through `contextParam`) only to a
/// member that is the FIRST label of the operation's path, for every operation that binds
/// them: for S3 that is `Bucket`, which every operation that binds it takes as `/{Bucket}/...`,
/// and not `Key`, which comes second.
fn leading_path_params(shapes: &HashMap<String, Value>) -> BTreeSet<String> {
    let mut leading = BTreeSet::new();
    let mut disqualified = BTreeSet::new();
    for operation in shapes.values().filter(|s| s["type"] == "operation") {
        let Some(members) = operation
            .get("input")
            .and_then(|input| input.get("target"))
            .and_then(Value::as_str)
            .and_then(|target| shapes.get(target))
            .and_then(|input| input.get("members"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let uri = operation
            .get("traits")
            .and_then(|t| t.get(HTTP))
            .and_then(|http| http.get("uri"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let first_label = uri
            .trim_start_matches('/')
            .split(['/', '?'])
            .next()
            .and_then(|segment| segment.strip_prefix('{'))
            .and_then(|segment| segment.strip_suffix('}'))
            .map(|label| label.trim_end_matches('+'));
        for (member, shape) in members {
            let traits = shape.get("traits");
            let Some(param) = traits
                .and_then(|t| t.get(CONTEXT_PARAM))
                .and_then(|c| c.get("name"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let is_leading_label = traits.is_some_and(|t| t.get(HTTP_LABEL).is_some())
                && first_label == Some(member.as_str());
            if is_leading_label {
                leading.insert(param.to_string());
            } else {
                disqualified.insert(param.to_string());
            }
        }
    }
    leading.difference(&disqualified).cloned().collect()
}

/// A host template reduced to literal labels, `{region}` and a final `{dnsSuffix}`, or
/// `None` when it uses a variable the router cannot match.
fn normalize(host: &str, region_params: &BTreeSet<&str>) -> Option<String> {
    let mut labels = Vec::new();
    let all: Vec<&str> = host.split('.').collect();
    // `{Region}` stays one label, but the partition's DNS suffix `{x#dnsSuffix}` is the
    // tail of the host, so it must be the last label.
    for (at, label) in all.iter().enumerate() {
        let last = at + 1 == all.len();
        if let Some(variable) = label.strip_prefix('{').and_then(|l| l.strip_suffix('}')) {
            if variable.ends_with("#dnsSuffix") && last {
                labels.push("{dnsSuffix}".to_string());
            } else if region_params.contains(variable) {
                labels.push("{region}".to_string());
            } else {
                return None;
            }
        } else if label.is_empty() || label.contains(['{', '}']) {
            // A variable inside a label (`s3express-{az}`) is a different kind of address.
            return None;
        } else {
            labels.push(label.to_ascii_lowercase());
        }
    }
    (!labels.is_empty()).then(|| labels.join("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An S3-shaped model: `Bucket` is the leading path label of every operation that binds
    /// it, `Key` is the second.
    fn shapes() -> HashMap<String, Value> {
        let members = json!({
            "Bucket": {"target": "smithy.api#String", "traits": {
                "smithy.api#httpLabel": {}, "smithy.rules#contextParam": {"name": "Bucket"}}},
            "Key": {"target": "smithy.api#String", "traits": {
                "smithy.api#httpLabel": {}, "smithy.rules#contextParam": {"name": "Key"}}},
        });
        HashMap::from([
            (
                "t#Put".to_string(),
                json!({"type": "operation", "input": {"target": "t#PutIn"},
                       "traits": {"smithy.api#http": {"method": "PUT", "uri": "/{Bucket}/{Key+}"}}}),
            ),
            (
                "t#List".to_string(),
                json!({"type": "operation", "input": {"target": "t#ListIn"},
                       "traits": {"smithy.api#http": {"method": "GET", "uri": "/{Bucket}?list-type=2"}}}),
            ),
            (
                "t#PutIn".to_string(),
                json!({"type": "structure", "members": members}),
            ),
            (
                "t#ListIn".to_string(),
                json!({"type": "structure", "members": {"Bucket": members["Bucket"].clone()}}),
            ),
        ])
    }

    fn rule_set(urls: &[&str]) -> serde_json::Map<String, Value> {
        rule_set_signing(&urls.iter().map(|url| (*url, "s3")).collect::<Vec<_>>())
    }

    /// A rule set whose endpoints sign as the given names; the service signs as `s3`.
    fn rule_set_signing(urls: &[(&str, &str)]) -> serde_json::Map<String, Value> {
        let rules: Vec<Value> = urls
            .iter()
            .map(|(url, signs_as)| {
                json!({"type": "tree", "conditions": [], "rules": [
                {"type": "endpoint", "conditions": [], "endpoint": {"url": url, "properties": {
                    "authSchemes": [{"name": "sigv4", "signingName": signs_as}]}}}]})
            })
            .collect();
        let mut traits = serde_json::Map::new();
        traits.insert(SIGV4.to_string(), json!({"name": "s3"}));
        traits.insert(
            RULE_SET.to_string(),
            json!({"version": "1.0", "parameters": {
                "Bucket": {"type": "string"}, "Key": {"type": "string"},
                "Region": {"type": "string", "builtIn": "AWS::Region"}},
                "rules": rules}),
        );
        traits
    }

    #[test]
    fn the_templates_are_the_hosts_that_lead_with_the_leading_path_parameter() {
        let traits = rule_set(&[
            "https://{Bucket}.s3.{Region}.{partitionResult#dnsSuffix}",
            "https://{Bucket}.s3.dualstack.{Region}.{partitionResult#dnsSuffix}",
            "https://{Bucket}.s3.{partitionResult#dnsSuffix}",
            "https://{Bucket}.S3-Accelerate.{partitionResult#dnsSuffix}",
            "https://{Bucket}.s3.dualstack.us-east-1.{partitionResult#dnsSuffix}",
            // The same shape again, as the rule tree repeats it.
            "https://{Bucket}.s3.{Region}.{partitionResult#dnsSuffix}",
        ]);
        assert_eq!(
            virtual_host_templates(&shapes(), Some(&traits)),
            [
                "s3-accelerate.{dnsSuffix}",
                "s3.dualstack.us-east-1.{dnsSuffix}",
                "s3.dualstack.{region}.{dnsSuffix}",
                "s3.{dnsSuffix}",
                "s3.{region}.{dnsSuffix}",
            ]
        );
    }

    #[test]
    fn addresses_of_other_kinds_are_left_out() {
        let traits = rule_set(&[
            // path-style: the parameter is in the path, not the host
            "https://s3.{Region}.{partitionResult#dnsSuffix}/{uri_encoded_bucket}",
            // S3 Express, Outposts and access points carry variables of their own
            "https://{Bucket}.s3express-{s3expressAvailabilityZoneId}.{Region}.{partitionResult#dnsSuffix}",
            "https://{Bucket}.op-{outpostId}.s3-outposts.{Region}.{regionPartition#dnsSuffix}",
            "https://{accessPointName}-{bucketArn#accountId}.s3-accesspoint.{bucketArn#region}.{bucketPartition#dnsSuffix}",
            // a customer's own endpoint
            "{url#scheme}://{Bucket}.{url#authority}{url#path}",
            "https://{Bucket}.{url#authority}",
            // a Region that is not the built-in one, and a suffix that is not last
            "https://{Bucket}.s3.{Other}.{partitionResult#dnsSuffix}",
            "https://{Bucket}.{partitionResult#dnsSuffix}.s3",
        ]);
        assert_eq!(
            virtual_host_templates(&shapes(), Some(&traits)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_address_that_signs_as_another_service_is_not_this_services() {
        let traits = rule_set_signing(&[
            (
                "https://{Bucket}.s3.{Region}.{partitionResult#dnsSuffix}",
                "s3",
            ),
            // S3 on Outposts bucket aliases: all literals and a Region, but signed as
            // `s3-outposts`, so the request is claimed by that service, not this one.
            (
                "https://{Bucket}.ec2.s3-outposts.{Region}.{regionPartition#dnsSuffix}",
                "s3-outposts",
            ),
        ]);
        assert_eq!(
            virtual_host_templates(&shapes(), Some(&traits)),
            ["s3.{region}.{dnsSuffix}"]
        );
    }

    #[test]
    fn only_a_parameter_that_is_always_the_leading_path_label_qualifies() {
        // `Key` is a context parameter too, but it is the second label, so a template led
        // by it names nothing the router can move into the path.
        let traits = rule_set(&["https://{Key}.s3.{Region}.{partitionResult#dnsSuffix}"]);
        assert!(virtual_host_templates(&shapes(), Some(&traits)).is_empty());

        // One operation that takes `Bucket` anywhere else disqualifies it.
        let mut shapes = shapes();
        shapes.insert(
            "t#Odd".to_string(),
            json!({"type": "operation", "input": {"target": "t#OddIn"},
                   "traits": {"smithy.api#http": {"method": "GET", "uri": "/x/{Bucket}"}}}),
        );
        shapes.insert(
            "t#OddIn".to_string(),
            json!({"type": "structure", "members": {"Bucket": {"target": "smithy.api#String",
                   "traits": {"smithy.api#httpLabel": {},
                              "smithy.rules#contextParam": {"name": "Bucket"}}}}}),
        );
        let traits = rule_set(&["https://{Bucket}.s3.{Region}.{partitionResult#dnsSuffix}"]);
        assert!(virtual_host_templates(&shapes, Some(&traits)).is_empty());
    }

    #[test]
    fn a_model_with_no_endpoint_rule_set_has_no_virtual_hosts() {
        assert!(virtual_host_templates(&shapes(), Some(&serde_json::Map::new())).is_empty());
        assert!(virtual_host_templates(&shapes(), None).is_empty());
    }
}
