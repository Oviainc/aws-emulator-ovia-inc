Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# DescribeDataset never resolves workspaceName, so a request naming a workspace that does not exist is refused as a missing dataset

_Filed: 2026-10-29T00:26:49Z_
_Operation: DescribeDataset | Class: error-response_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeDataset uses `workspaceName` only as a filter over the dataset it looked up by id; it never asks whether the named workspace exists. A request naming an absent workspace is therefore answered with `ResourceNotFoundException` whose message names the DATASET, hiding the fact that the workspace the caller addressed is the thing that is missing. Its sibling listing over the same optional member states the opposite rule in a comment and enforces it before anything else, so the two reads of the same resource family disagree about which absence a request that is wrong in both ways reports.

## Evidence

DescribeDataset filters but never resolves the workspace — services/.../src/ops/describe_dataset.rs:188-206:
```
        let dataset = store
            .datasets
            .get(&input.dataset_id)
            .filter(|dataset| {
                // `workspaceName` names the workspace that contains the dataset, so a
                // request that names one scopes the lookup: ...
                input.workspace_name.is_none() || input.workspace_name == dataset.workspace_name
            })
            ...
            .ok_or_else(|| not_found(&input.dataset_id))?;
```
The sibling listing, over the SAME optional member, states the crate's rule and applies it first — list_datasets.rs:170-186:
```
        // `workspaceName` names the workspace whose datasets the request wants, ... so a workspace
        // this service holds no record of is the modelled not-found refusal rather
        // than an empty page, which would report absence as success. ... Resolved BEFORE the
        // page is cut, so a request that is wrong in both ways reports the resource
        // that is absent.
        if let Some(workspace) = input.workspace_name.as_deref() {
            if !store.workspaces.contains_key(workspace) {
                return Err(not_found(crate::state::not_found_message(
                    crate::state::RESOURCE_WORKSPACE,
                    workspace,
                )));
            }
        }
```
DescribeDataset models the same error it would need — smithy_query DescribeDataset errors include `{"shape": "ResourceNotFoundException", "httpError": 404}`; the member doc is "The name of the workspace that contains the dataset" (describe_dataset.rs:56-57).

## Steps to Reproduce

1. CreateDataset without a workspace (or in workspace W) and note its datasetId.
2. ListDatasets with workspaceName = "ghost" (no such workspace): 404 naming the WORKSPACE ghost.
3. DescribeDataset with that datasetId and workspaceName = "ghost": 404 naming the DATASET id instead of the absent workspace, so the two reads disagree about which resource is missing.
