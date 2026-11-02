Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-m6i-r4
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.medium"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.metal"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gd.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.medium"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c6gn.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].gpuInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].memoryInfo.sizeInMiB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.efaInfo.maximumEfaInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].processorInfo.manufacturer
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].supportedUsageClasses.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.48xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].gpuInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].memoryInfo.sizeInMiB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.efaInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.efaSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].processorInfo.manufacturer
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="p5.4xlarge"].supportedUsageClasses.item[*]
Issue-Commit: 86294b81c3bdab5f6a26606f5f9507599ec174dd

# EC2 DescribeInstanceTypes multi-type responses omit modeled members and report synthesized hardware values

> _Note: this issue was identified against the code base at commit `86294b81c3bdab5f6a26606f5f9507599ec174dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
Every DescribeInstanceTypes call that returns MORE THAN ONE instanceTypeSet item diverges from the recording: the emulator answers 200 with a well-formed ec2Query XML body, but the per-instance-type attributes in that body are generated from a generic formula instead of being the instance type's real attributes, and a fixed set of modeled members is never emitted at all. The replay reports 138 such divergence clusters, all of operation DescribeInstanceTypes, all in service ec2, spread over the ec2-catalog-describe-instance-types-* fixtures (families a1..c9g, c9gd..m6gd, m6i..r4 and the later family sweeps); each cluster is one step whose response covers a whole instance family. The already-open issues 20261030T124526Z-ec2-describeinstancetypes-serves-a-synthesized-instance-type-catalog.md, 20261030T210852Z-ec2-describeinstancetypes-synthesizes-instance-type-attributes-instead-of.md, 20261031T044610Z-ec2-describeinstancetypes-serves-emulator-authored-instance-type-metadata.md, 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md and 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md own only the single-item (one instanceTypeSet.item) clusters and do not carry the replay keys below; the multi-item clusters are reported unowned and are what this issue is filed for.

Three causes, all in the same read path:

Cause 1 of 3: modeled InstanceTypeInfo/NetworkInfo members are omitted from every item of a multi-type response. Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#12.
AWS: Smithy com.amazonaws.ec2#InstanceTypeInfo models phcSupport (PhcSupport), rebootMigrationSupport (RebootMigrationSupport), gpuInfo (GpuInfo), inferenceAcceleratorInfo (InferenceAcceleratorInfo), instanceStorageInfo (InstanceStorageInfo), nitroTpmSupport/nitroEnclavesSupport; com.amazonaws.ec2#NetworkInfo models maximumNetworkCards, defaultNetworkCardIndex, networkCards (NetworkCardInfoList), enaSrdSupported, flexibleEnaQueuesSupport, secondaryNetworkSupported and ipv4AddressesPerSecondaryInterface. AWS populates these for every instance type it returns; the recording shows them present on each item (e.g. defaultNetworkCardIndex '0', enaSrdSupported 'false', flexibleEnaQueuesSupport 'unsupported', maximumNetworkCards '1', networkCards with baselineBandwidthInGbps/defaultEnaQueueCount entries). cpuBurstModel and networkInfo.trafficMirroringSupport are not in this checkout's model - recorded behavior only: the recording always carries them (cpuBurstModel 'none' for non-burstable types, 'credit' for T types).
Emulator: those members are absent (replay verdict "missing", got None) on every item of the response, for all instance types in the family.
Why: a client that reads networkCards/maximumNetworkCards to size ENI placement, phcSupport/rebootMigrationSupport for feature gating, or gpuInfo/inferenceAcceleratorInfo to select accelerated capacity sees the member unset and concludes the instance type lacks the capability, so capacity selection and feature detection silently pick the wrong instance type.
Fix: the DescribeInstanceTypes read path - the instance-type catalog record it serves from and the serializer that projects it into instanceTypeSet.item (services/service-com-amazonaws-ec2-amazonec2, ops/describe_instance_types and the catalog module it reads).

Cause 2 of 3: ebsInfo is computed from a size-scaling formula rather than the instance type's published EBS performance. Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#13.
AWS: Smithy com.amazonaws.ec2#EbsOptimizedInfo models baselineBandwidthInMbps, baselineIops, baselineThroughputInMBps, maximumBandwidthInMbps, maximumIops, maximumThroughputInMBps; EbsInfo models maximumEbsAttachments and attachmentLimitType. These are per-instance-type published figures, not a function of vCPU/memory. The recording states them per type (c6gn.medium: baselineBandwidthInMbps 760, baselineIops 2500, baselineThroughputInMBps 95.0, maximumBandwidthInMbps 9500, maximumIops 40000, maximumThroughputInMBps 1187.5, maximumEbsAttachments 27; p5.48xlarge: 80000/260000/10000.0 and maximumEbsAttachments 64 with attachmentLimitType 'dedicated').
Emulator: it returns formula values that repeat across unrelated types - 4750 maximumBandwidthInMbps, 18750 maximumIops, 593.75 maximumThroughputInMBps and maximumEbsAttachments 28 appear for c6gn.medium, c6gd.xlarge, m6i/r4 and p5 types alike, and attachmentLimitType is always 'shared'. Replay examples: c6gn.medium expected 760 got 287 (baselineBandwidthInMbps), expected 2500 got 1500 (baselineIops), expected 95.0 got 35.875, expected 9500 got 4750, expected 40000 got 18750, expected 1187.5 got 593.75, maximumEbsAttachments expected 27 got 28; c6gd.xlarge expected 1188 got 1150, 148.5 got 143.75, 20000 got 18750, maximumEbsAttachments expected 26 got 28.
Why: EBS-optimized throughput and attachment limits are exactly what callers use to choose an instance type for a volume layout; a uniform synthetic value makes every type look identical and makes attachment-limit checks (28 vs the real 10/19/26/27/32/64) wrong in both directions.
Fix: replace the derived ebsInfo computation in the instance-type catalog with per-type stored values in the same read path as cause 1.

Cause 3 of 3: per-type hardware and capability facts are emulator-authored and contradict the recording. Fixture ec2-catalog-describe-instance-types-m6i-r4, step task#38.
AWS: the same InstanceTypeInfo closure models currentGeneration, freeTierEligible, dedicatedHostsSupported, autoRecoverySupported, memoryInfo.sizeInMiB, processorInfo (manufacturer, sustainedClockSpeedInGhz), vCpuInfo.validCores, instanceStorageSupported/instanceStorageInfo (disks count/sizeInGB/type, totalSizeInGB), networkInfo.networkPerformance, ipv4AddressesPerInterface, ipv6AddressesPerInterface, maximumNetworkInterfaces, encryptionInTransitSupported, efaInfo.maximumEfaInterfaces, supportedUsageClasses, supportedBootModes, placementGroupInfo.supportedStrategies and nitroTpmSupport - each a fact about the named instance type.
Emulator: these are wrong per type. Replay examples: p5.48xlarge dedicatedHostsSupported expected 'false' got 'true', gpuInfo missing (recording: 8 x NVIDIA H100, memoryInfo.sizeInMiB 81920, totalGpuMemoryInMiB 655360), plus divergences on memoryInfo.sizeInMiB, processorInfo.manufacturer, supportedUsageClasses, vCpuInfo.validCores, instanceStorageInfo.disks count/sizeInGB and totalSizeInGB, networkInfo.efaInfo.maximumEfaInterfaces; c6gd.xlarge instanceStorageInfo.disks.item.sizeInGB and totalSizeInGB expected 237 got 100 and networkInfo.encryptionInTransitSupported expected 'false' got 'true'; c6gn.medium currentGeneration expected 'false' got 'true'.
Why: callers treat DescribeInstanceTypes as the authoritative hardware inventory - GPU presence, local NVMe size, memory size, valid core counts and usage classes drive scheduler and autoscaling decisions. Values invented per request make an instance type look like hardware it is not.
Fix: same read path - back the catalog with the recorded per-type attribute set instead of deriving fields from the instance type name/size, and emit the stored value for every modeled member.

Traces (one fixture#step per cause, replay keys copied verbatim): cause 1 ec2-catalog-describe-instance-types-a1-c9g#task#12; cause 2 ec2-catalog-describe-instance-types-a1-c9g#task#13; cause 3 ec2-catalog-describe-instance-types-m6i-r4#task#38. The same defect recurs in the remaining 135 unowned DescribeInstanceTypes clusters of the ec2-catalog-describe-instance-types-* fixtures.

## Evidence
Replay of simparity-wire-aws-v1 (285 fixtures, 2159 steps, score 54.2%): 138 divergence clusters are operation DescribeInstanceTypes, service ec2, verdict mismatch, with no stub_path and no owning issue.

Cause 1 - fixture ec2-catalog-describe-instance-types-a1-c9g, step task#12, expected 200 / actual 200:
  $...item[instanceType="c6gd.xlarge"].cpuBurstModel: missing (expected 'none', got None)
  $...item[instanceType="c6gd.xlarge"].networkInfo.defaultNetworkCardIndex: missing (expected '0', got None)
  $...item[instanceType="c6gd.xlarge"].networkInfo.enaSrdSupported: missing (expected 'false', got None)
  $...item[instanceType="c6gd.xlarge"].networkInfo.flexibleEnaQueuesSupport: missing (expected 'unsupported', got None)
  $...item[instanceType="c6gd.xlarge"].networkInfo.maximumNetworkCards: missing (expected '1', got None)
  same keys missing for every other item of the family (c6gd.medium/large/2xlarge/4xlarge/8xlarge/12xlarge/16xlarge/metal), plus networkCards, phcSupport, rebootMigrationSupport, secondaryNetworkSupported, trafficMirroringSupport.

Cause 2 - fixture ec2-catalog-describe-instance-types-a1-c9g, step task#13, expected 200 / actual 200:
  $...item[instanceType="c6gn.medium"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: value_mismatch (expected '760', got '287')
  ...baselineIops: value_mismatch (expected '2500', got '1500')
  ...baselineThroughputInMBps: value_mismatch (expected '95.0', got '35.875')
  ...maximumBandwidthInMbps: value_mismatch (expected '9500', got '4750')
  ...maximumIops: value_mismatch (expected '40000', got '18750')
  ...maximumThroughputInMBps: value_mismatch (expected '1187.5', got '593.75')
  ...ebsInfo.maximumEbsAttachments: value_mismatch (expected '27', got '28')
  and in task#12 for c6gd.xlarge: baselineBandwidthInMbps expected '1188' got '1150', baselineThroughputInMBps expected '148.5' got '143.75', maximumIops expected '20000' got '18750', maximumEbsAttachments expected '26' got '28'.

Cause 3 - fixture ec2-catalog-describe-instance-types-m6i-r4, step task#38, expected 200 / actual 200:
  $...item[instanceType="p5.48xlarge"].dedicatedHostsSupported: value_mismatch (expected 'false', got 'true')
  $...item[instanceType="p5.48xlarge"].ebsInfo.attachmentLimitType: value_mismatch (expected 'dedicated', got 'shared')
  $...item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: value_mismatch (expected '80000', got '40800')
  $...item[instanceType="p5.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops: value_mismatch (expected '260000', got '180000')
  $...item[instanceType="p5.48xlarge"].ebsInfo.maximumEbsAttachments: value_mismatch (expected '64', got '28')
  $...item[instanceType="p5.48xlarge"].gpuInfo: missing (expected 8 x NVIDIA H100, memoryInfo.sizeInMiB 81920, totalGpuMemoryInMiB 655360, got None)
  divergence_key also lists memoryInfo.sizeInMiB, processorInfo.manufacturer, processorInfo.sustainedClockSpeedInGhz, instanceStorageInfo.disks.item.count/sizeInGB, instanceStorageInfo.totalSizeInGB, networkInfo.efaInfo.maximumEfaInterfaces, networkInfo.maximumNetworkInterfaces, networkInfo.networkPerformance, supportedUsageClasses.item[0..2] and vCpuInfo.validCores.item[0..95] for p5.48xlarge and p5.4xlarge.
  c6gd.xlarge (task#12): instanceStorageInfo.disks.item.sizeInGB and instanceStorageInfo.totalSizeInGB expected '237' got '100'; networkInfo.encryptionInTransitSupported expected 'false' got 'true'.
  c6gn.medium (task#13): currentGeneration expected 'false' got 'true'.

Status and error contract are not at issue here: every one of these steps returns the recorded 200 with no error code; the divergence is confined to the response body.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture ec2-catalog-describe-instance-types-a1-c9g step task#12: POST the ec2Query DescribeInstanceTypes request that selects the c6gd family (multiple InstanceType filters, so the response carries several instanceTypeSet.item entries). Compare each item against the recording: cpuBurstModel, networkInfo.defaultNetworkCardIndex, networkInfo.enaSrdSupported, networkInfo.flexibleEnaQueuesSupport, networkInfo.maximumNetworkCards, networkInfo.networkCards, phcSupport and rebootMigrationSupport are absent from the emulator's XML (cause 1).
3. In the same response read ebsInfo.ebsOptimizedInfo and ebsInfo.maximumEbsAttachments per item; then replay step task#13 (c6gn family) and do the same. The emulator returns the same formula numbers (4750 / 18750 / 593.75 / 28) across unrelated types instead of the per-type figures in the recording (cause 2).
4. Replay fixture ec2-catalog-describe-instance-types-m6i-r4 step task#38 (p5 family) and compare dedicatedHostsSupported, ebsInfo.attachmentLimitType, gpuInfo, memoryInfo.sizeInMiB, processorInfo.*, instanceStorageInfo.* and vCpuInfo.validCores against the recording (cause 3).
5. All three steps return HTTP 200 in both the recording and the emulator, so the diff is body-only; the remaining 135 unowned DescribeInstanceTypes clusters in the ec2-catalog-describe-instance-types-* fixtures reproduce the same three causes for other families.
