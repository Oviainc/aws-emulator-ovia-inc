Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected
Issue-Key: ecr__eks__logs|CompleteLayerUpload|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# ECR CompleteLayerUpload is an unimplemented scaffold stub, so no image layer can be pushed to a repository

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteLayerUpload

## Description
Cause 1 of 1: CompleteLayerUpload answers NotImplemented.
AWS: CompleteLayerUpload closes an upload started by InitiateLayerUpload and fed by UploadLayerPart, validating the layer digest and returning {registryId, repositoryName, uploadId, layerDigest}; after it succeeds the layer is referenced by a PutImage manifest. Smithy (ecr): CompleteLayerUpload declares ServerException, InvalidParameterException, InvalidLayerException, LayerPartTooSmallException, LayerAlreadyExistsException, EmptyUploadException, UploadNotFoundException, RepositoryNotFoundException, KmsException - NotImplemented is not modeled.
Emulator: ClientError code 'NotImplemented', message 'operation CompleteLayerUpload is scaffolded but not implemented in this emulator'.
Why: an image push can never finish, so any scenario that needs a real image in a repository (including the cross-account pull-permission negative path) cannot run - the client sees a half-finished upload with no way to complete or fail it meaningfully.
Fix: implement the CompleteLayerUpload handler in the ecr service crate's ops module, assembling the parts recorded by UploadLayerPart. The open issue 20261022T154613Z-ecr-eks-logs-ecr-uploadlayerpart-is-a-501-stub-and-a.md covers UploadLayerPart; this is the adjacent operation on the same push path.
Traces: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CompleteLayerUpload operation: operation CompleteLayerUpload is scaffolded but not implemented in this emulator. Expected: 200 with {registryId, repositoryName, uploadId, layerDigest}.

## Steps to Reproduce
ecr.create_repository, ecr.initiate_layer_upload, ecr.upload_layer_part(<gzipped layer>), then ecr.complete_layer_upload(uploadId=..., layerDigests=[sha256:...]) -> NotImplemented.
