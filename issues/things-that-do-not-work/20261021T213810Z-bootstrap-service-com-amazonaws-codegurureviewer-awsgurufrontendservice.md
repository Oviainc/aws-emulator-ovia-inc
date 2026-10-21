Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodeGuru Reviewer emulator

_Filed: 2026-10-21T21:38:10Z_
_Operation: service-com-amazonaws-codegurureviewer-awsgurufrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 14 operations in `src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 with zero stored state (verified in `src/ops/associate_repository.rs`). Implement a FIRST, SMALL stateful slice: the repository-association -> code-review lifecycle of the primary resources, over the `restJson1` protocol (signing name `codeguru-reviewer`).

### Happy path to implement (in order)

1. **AssociateRepository** — `POST /associations`. Input: `Repository` (required; union-ish structure with `CodeCommit`/`Bitbucket`/`GitHubEnterpriseServer`/`S3Bucket`), optional `ClientRequestToken` (@idempotencyToken), `Tags`, `KMSKeyDetails`. Must mint a new association: `AssociationId` (uuid), `AssociationArn` = `arn:aws:codeguru-reviewer:<region>:<account>:association/<AssociationId>`, `Name` and `Owner` taken from the supplied `Repository` member, `ProviderType` derived from which `Repository` member was set (`CodeCommit` -> `CodeCommit`, `Bitbucket` -> `Bitbucket`, `GitHubEnterpriseServer` -> `GitHubEnterpriseServer`, `S3Bucket` -> `S3Bucket`), `State` = `Associated`, `CreatedTimeStamp`/`LastUpdatedTimeStamp` = now, `KMSKeyDetails` echoed from input. Persist it and return `RepositoryAssociation` + `Tags` as stored. Re-associating the same provider/owner/name while an association is active returns `ConflictException` (409); repeating with the same `ClientRequestToken` returns the SAME association rather than a new one.
2. **DescribeRepositoryAssociation** — `GET /associations/{AssociationArn}`. Must look the ARN up in the store and return the stored `RepositoryAssociation` (same `AssociationId`, `Name`, `Owner`, `ProviderType`, `State`, timestamps) plus its stored `Tags`. Unknown ARN -> `NotFoundException` (404).
3. **ListRepositoryAssociations** — `GET /associations`. Returns `RepositoryAssociationSummaries` built from the stored associations (`AssociationArn`, `AssociationId`, `Name`, `Owner`, `ProviderType`, `State`, `LastUpdatedTimeStamp`). Honour the `ProviderType`/`State`/`Name`/`Owner` query filters, `MaxResults` (1..100) and an opaque `NextToken` round-trip.
4. **CreateCodeReview** — `POST /codereviews`. Input: `Name`, `RepositoryAssociationArn`, `Type` (`CodeReviewType` with required `RepositoryAnalysis`, optional `AnalysisTypes`), optional `ClientRequestToken`. Must resolve `RepositoryAssociationArn` against the stored associations (unknown -> `ResourceNotFoundException` 404), then persist a `CodeReview` with `CodeReviewArn` = `arn:aws:codeguru-reviewer:<region>:<account>:code-review/<id>`, `Name` from input, `RepositoryName`/`Owner`/`ProviderType`/`AssociationArn` copied from the resolved association, `Type` = `RepositoryAnalysis`, `State` = `Pending` (may settle to `Completed`), `AnalysisTypes` from input, `SourceCodeType` from `Type.RepositoryAnalysis`, timestamps = now. Return the stored object.
5. **DescribeCodeReview** — `GET /codereviews/{CodeReviewArn}`. Returns the persisted `CodeReview` for that ARN, identical to what `CreateCodeReview` stored; unknown ARN -> `ResourceNotFoundException` (404).
6. **DisassociateRepository** — `DELETE /associations/{AssociationArn}`. Removes (or marks `Disassociated`) the stored association and returns the final `RepositoryAssociation` + `Tags`; a subsequent `DescribeRepositoryAssociation` must fail with `NotFoundException` (404) or report `State = Disassociated`, and the association must no longer appear in `ListRepositoryAssociations` as `Associated`.

NEVER STUBS: each of these six must compute its response from the persisted store. No canned ARNs, no echoing the request back without storing it, no random IDs that are never recorded. A create followed by a describe/list must return exactly what was created.

## State Model

```
CodeGuruReviewerState {
    associations: Map<String /*AssociationArn*/, Association>,
    association_tokens: Map<String /*ClientRequestToken*/, String /*AssociationArn*/>,
    code_reviews: Map<String /*CodeReviewArn*/, CodeReviewRecord>,
    review_tokens: Map<String /*ClientRequestToken*/, String /*CodeReviewArn*/>,
}

Association {
    association_id: String,         // uuid
    association_arn: String,
    name: String,                   // from Repository.*.Name
    owner: Option<String>,          // from Repository.*.Owner
    provider_type: ProviderType,    // derived from which Repository member was set
    connection_arn: Option<String>, // ThirdPartySourceRepository.ConnectionArn
    state: RepositoryAssociationState, // Associated | Disassociating | Disassociated | Failed
    state_reason: Option<String>,
    created_timestamp: Timestamp,
    last_updated_timestamp: Timestamp,
    kms_key_details: Option<KMSKeyDetails>,
    s3_repository_details: Option<S3RepositoryDetails>,
    tags: Map<String, String>,
}

CodeReviewRecord {
    name: String,
    code_review_arn: String,
    association_arn: String,        // FK into associations
    repository_name: String,
    owner: Option<String>,
    provider_type: ProviderType,
    state: JobState,                // Pending | Completed | Failed | Deleting
    state_reason: Option<String>,
    type_: Type,                    // RepositoryAnalysis for CreateCodeReview
    analysis_types: Vec<AnalysisType>,
    source_code_type: Option<SourceCodeType>,
    pull_request_id: Option<String>,
    metrics: Option<Metrics>,
    config_file_state: Option<ConfigFileState>,
    created_timestamp: Timestamp,
    last_updated_timestamp: Timestamp,
}
```

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): ListCodeReviews, ListRecommendations, PutRecommendationFeedback, DescribeRecommendationFeedback, ListRecommendationFeedback, TagResource, UntagResource, ListTagsForResource.

## Evidence

OPERATIONS.md (crate root) lists all 14 operations with routes, input/output members and errors: AssociateRepository `POST /associations` -> `RepositoryAssociation`+`Tags`; DescribeRepositoryAssociation `GET /associations/{AssociationArn}` with `NotFoundException (HTTP 404)`; ListRepositoryAssociations `GET /associations` @paginated(items=RepositoryAssociationSummaries); CreateCodeReview `POST /codereviews` requiring `Name`, `RepositoryAssociationArn`, `Type` and declaring `ResourceNotFoundException (HTTP 404)`/`ConflictException (HTTP 409)`; DescribeCodeReview `GET /codereviews/{CodeReviewArn}`; DisassociateRepository `DELETE /associations/{AssociationArn}`. Shape members cited above come from smithy_query on `RepositoryAssociation`, `RepositoryAssociationSummary`, `CodeReview`, `CodeReviewSummary`, `Repository` and `CodeReviewType` in services/service-com-amazonaws-codegurureviewer-awsgurufrontendservice/smithy-model/com-amazonaws-codegurureviewer-awsgurufrontendservice.json. All-stub status confirmed by the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and `Err(crate::scaffolded::Scaffolded::not_implemented("AssociateRepository"))` at src/ops/associate_repository.rs:196; no tests assert any behaviour for this service.

## Steps to Reproduce

1. AssociateRepository with Repository.CodeCommit.Name="my-repo" and Tags={"Env":"dev"} -> 200 with a fresh AssociationId/AssociationArn, ProviderType=CodeCommit, Name="my-repo", State=Associated, and the Tags echoed from storage (today: 501 NotImplemented).
2. DescribeRepositoryAssociation with that AssociationArn -> 200 returning the SAME AssociationId, Name, ProviderType, State and CreatedTimeStamp as step 1.
3. ListRepositoryAssociations (optionally ?Name=my-repo&MaxResults=10) -> 200 with a RepositoryAssociationSummaries entry for that association.
4. CreateCodeReview with Name="review-1", RepositoryAssociationArn from step 1, Type.RepositoryAnalysis.RepositoryHead.BranchName="main" -> 200 with a CodeReview carrying a new CodeReviewArn, AssociationArn = step 1's ARN, RepositoryName="my-repo", Type=RepositoryAnalysis, State=Pending.
5. DescribeCodeReview with that CodeReviewArn -> 200 returning the same persisted CodeReview fields.
6. CreateCodeReview against an unknown association ARN -> 404 ResourceNotFoundException; DescribeCodeReview on an unknown ARN -> 404 ResourceNotFoundException.
7. DisassociateRepository with the AssociationArn -> 200 returning the association and its Tags; then DescribeRepositoryAssociation on the same ARN -> 404 NotFoundException (or State=Disassociated) and it no longer appears as Associated in ListRepositoryAssociations.
