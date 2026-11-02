// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The signed restJson1 operations of every Smithy JSON model in a build, for one suite to sweep.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::slice::from_ref;

const SKIPPED_FOLDERS: [&str; 4] = ["target", "build", "node_modules", "tests"];

/// One operation as a sweep sends it: where it is routed and the name it is signed for.
#[derive(Debug, Clone)]
pub struct SweptOperation {
    pub name: String,
    pub signing_name: String,
    pub host: String,
    pub method: String,
    /// The URI with every label, and every query parameter the input requires, set to `x`.
    pub path: String,
    /// The headers the input requires, without which a request is not routed to the operation.
    pub required_headers: Vec<String>,
}

/// Every `*.json` in a `smithy-model` folder under `root`; a folder it cannot list is a failure,
/// not a tree with no models.
pub fn model_files_under(root: &Path) -> Vec<PathBuf> {
    let json = files_under(root, &|path| path.extension() == Some("json".as_ref()));
    let unlistable: Vec<_> = json.iter().filter(|path| !path.is_file()).collect();
    assert!(unlistable.is_empty(), "cannot list {unlistable:?}");
    let kept = |path: &PathBuf| path.parent().unwrap().ends_with("smithy-model");
    json.into_iter().filter(kept).collect()
}

/// Sorted files `kept` keeps under `folder`, past links, dot entries and [`SKIPPED_FOLDERS`]; an
/// unlistable folder stands in for its files.
fn files_under(folder: &Path, kept: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return vec![folder.to_path_buf()];
    };
    let mut found = Vec::new();
    let paths = entries.flatten().map(|entry| entry.path());
    for path in paths.filter(|path| !is_skipped(path)) {
        if path.is_dir() {
            found.extend(files_under(&path, kept));
        } else if kept(&path) {
            found.push(path);
        }
    }
    found.sort();
    found
}

fn is_skipped(path: &Path) -> bool {
    let name = file_name(path);
    path.is_symlink() || name.starts_with('.') || SKIPPED_FOLDERS.contains(&name)
}

fn file_name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// Every signed operation, resources' included, of every restJson1 service in `models`.
pub fn signed_rest_json_operations(models: &[PathBuf]) -> Vec<SweptOperation> {
    let mut calls = Vec::new();
    for model in models {
        let model: Value = serde_json::from_str(&std::fs::read_to_string(model).unwrap()).unwrap();
        for service in model["shapes"].as_object().unwrap().values() {
            let rest_json = service["traits"].get("aws.protocols#restJson1").is_some();
            let signing_name = service["traits"]["aws.auth#sigv4"]["name"].as_str();
            if let (true, Some(signing_name)) = (rest_json, signing_name) {
                signed_calls(&model["shapes"], service, signing_name, &mut calls);
            }
        }
    }
    calls
}

/// Adds a call for each signed operation `container` binds, directly or through its resources.
fn signed_calls(shapes: &Value, container: &Value, signer: &str, calls: &mut Vec<SweptOperation>) {
    for field in container.as_object().into_iter().flat_map(Map::values) {
        for binding in field.as_array().map_or(from_ref(field), Vec::as_slice) {
            let name = binding["target"].as_str().unwrap_or_default();
            match shapes[name]["type"].as_str() {
                Some("resource") => signed_calls(shapes, &shapes[name], signer, calls),
                Some("operation") => calls.extend(signed_call(shapes, signer, name)),
                _ => {}
            }
        }
    }
}

fn signed_call(shapes: &Value, signing_name: &str, name: &str) -> Option<SweptOperation> {
    let traits = &shapes[name]["traits"];
    let optional = traits.get("smithy.api#optionalAuth").is_some();
    let unsigned = optional || traits["smithy.api#auth"] == json!([]);
    let http = traits.get("smithy.api#http").filter(|_| !unsigned)?;
    let parts: Vec<_> = http["uri"].as_str()?.split(['{', '}']).collect();
    let mut path = parts.into_iter().step_by(2).collect::<Vec<_>>().join("x");
    for query in required_bindings(shapes, name, "smithy.api#httpQuery") {
        let separator = if path.contains('?') { '&' } else { '?' };
        path = format!("{path}{separator}{query}=x");
    }
    Some(SweptOperation {
        name: name.to_string(),
        signing_name: signing_name.to_string(),
        host: format!("{signing_name}.us-west-2.amazonaws.com"),
        method: http["method"].as_str()?.to_string(),
        path,
        required_headers: required_bindings(shapes, name, "smithy.api#httpHeader"),
    })
}

/// The wire names `binding` gives the members the operation's input requires.
fn required_bindings(shapes: &Value, operation: &str, binding: &str) -> Vec<String> {
    let input = shapes[operation]["input"]["target"].as_str();
    let members = shapes[input.unwrap_or_default()]["members"].as_object();
    let members = members.into_iter().flat_map(Map::values);
    let required = members.filter(|member| member["traits"].get("smithy.api#required").is_some());
    let bound = required.filter_map(|member| member["traits"][binding].as_str());
    bound.map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::{json, model_files_under, signed_call};

    #[test]
    fn a_swept_call_fills_the_labels_query_parameters_and_headers_its_input_requires() {
        let members = json!({
            "Dir": {"traits": {"smithy.api#required": {}, "smithy.api#httpQuery": "DirectoryId"}},
            "Version": {"traits": {"smithy.api#required": {}, "smithy.api#httpHeader": "If-Match"}},
            "Maybe": {"traits": {"smithy.api#httpQuery": "maybe"}}});
        let http = json!({"method": "POST", "uri": "/a/{Id}?pinned"});
        let shapes = json!({"s#In": {"members": members},
            "s#Op": {"input": {"target": "s#In"}, "traits": {"smithy.api#http": http}}});
        let swept = signed_call(&shapes, "svc", "s#Op").unwrap();
        assert_eq!(swept.path, "/a/x?pinned&DirectoryId=x");
        assert_eq!(swept.required_headers, ["If-Match"]);
    }

    #[test]
    #[should_panic(expected = "cannot list")]
    fn a_models_folder_that_cannot_be_listed_fails_the_sweep_instead_of_emptying_it() {
        model_files_under("/emulator-no-such-models-folder".as_ref());
    }
}
