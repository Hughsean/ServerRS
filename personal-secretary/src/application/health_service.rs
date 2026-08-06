//! 健康状态应用层：快照生产端口与有界缓存聚合。
//!
//! 本模块只依赖领域对象（[`crate::health`]）和抽象端口，不依赖 NapCat、SeaORM、
//! MySQL 或 `qqbot-server`。
//!
//! 用例职责：
//! 1. 各子系统实现 `HealthSnapshotProducer`，提供自身健康快照；
//! 2. `HealthAggregator` 聚合所有子系统状态为 `HealthSnapshot`，有界缓存；
//! 3. `snapshot()` 返回缓存快照，不触发外部 API 调用（任务九）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::{HealthSnapshot, SubsystemHealth};

/// 健康快照生产端口：各子系统实现，提供自身健康状态。
#[async_trait]
pub trait HealthSnapshotProducer: Send + Sync {
    /// 子系统名称。
    fn name(&self) -> &'static str;

    /// 读取当前健康快照。不应调用昂贵外部 API（任务九）。
    async fn health(&self) -> SubsystemHealth;
}

/// 健康聚合器：聚合所有子系统状态为 `HealthSnapshot`，有界缓存。
///
/// 缓存 TTL 内不重新收集子系统状态，避免每次读取都调用外部 API。
pub struct HealthAggregator {
    producers: Vec<Arc<dyn HealthSnapshotProducer>>,
    cache: Mutex<Option<CachedSnapshot>>,
    cache_ttl: Duration,
}

struct CachedSnapshot {
    snapshot: HealthSnapshot,
    cached_at: Instant,
}

impl HealthAggregator {
    /// 构造聚合器。`cache_ttl` 控制缓存有效期。
    pub fn new(cache_ttl: Duration) -> Self {
        Self {
            producers: Vec::new(),
            cache: Mutex::new(None),
            cache_ttl,
        }
    }

    /// 添加一个子系统健康生产者。
    pub fn add_producer(&mut self, producer: Arc<dyn HealthSnapshotProducer>) {
        // 添加生产者时清除缓存，确保下次读取收集新生产者的状态。
        self.cache.lock().unwrap().take();
        self.producers.push(producer);
    }

    /// 读取健康快照。缓存 TTL 内返回缓存；过期后重新收集。
    ///
    /// 不调用昂贵外部 API（各生产者只读自身内存状态）。
    pub async fn snapshot(&self, now_unix_secs: i64) -> HealthSnapshot {
        // 检查缓存。
        {
            let cache = self.cache.lock().unwrap();
            if let Some(cached) = cache.as_ref()
                && cached.cached_at.elapsed() < self.cache_ttl
            {
                return cached.snapshot.clone();
            }
        }

        // 收集各子系统状态。
        let mut subsystems = Vec::new();
        for producer in &self.producers {
            subsystems.push(producer.health().await);
        }

        let snapshot = HealthSnapshot::new(subsystems, now_unix_secs);

        // 更新缓存。
        {
            let mut cache = self.cache.lock().unwrap();
            *cache = Some(CachedSnapshot {
                snapshot: snapshot.clone(),
                cached_at: Instant::now(),
            });
        }

        snapshot
    }

    /// 清除缓存，强制下次读取重新收集。
    pub fn invalidate_cache(&self) {
        self.cache.lock().unwrap().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HealthStatus;
    use async_trait::async_trait;

    struct FakeProducer {
        name: &'static str,
        status: HealthStatus,
    }

    #[async_trait]
    impl HealthSnapshotProducer for FakeProducer {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn health(&self) -> SubsystemHealth {
            SubsystemHealth {
                name: self.name.into(),
                status: self.status,
                last_success_at_unix_secs: Some(1000),
                last_error: None,
                metrics: std::collections::BTreeMap::new(),
            }
        }
    }

    #[tokio::test]
    async fn aggregator_collects_and_caches() {
        let mut aggregator = HealthAggregator::new(Duration::from_secs(60));
        aggregator.add_producer(Arc::new(FakeProducer {
            name: "test1",
            status: HealthStatus::Healthy,
        }));
        aggregator.add_producer(Arc::new(FakeProducer {
            name: "test2",
            status: HealthStatus::Degraded,
        }));

        let snapshot = aggregator.snapshot(1000).await;
        assert_eq!(snapshot.overall_status, HealthStatus::Degraded);
        assert_eq!(snapshot.subsystems.len(), 2);
    }

    #[tokio::test]
    async fn aggregator_returns_cached_within_ttl() {
        let mut aggregator = HealthAggregator::new(Duration::from_secs(60));
        aggregator.add_producer(Arc::new(FakeProducer {
            name: "test",
            status: HealthStatus::Healthy,
        }));

        let snap1 = aggregator.snapshot(1000).await;
        let snap2 = aggregator.snapshot(2000).await;
        // 缓存命中：snapshot_at_unix_secs 应相同（缓存值）。
        assert_eq!(snap1.snapshot_at_unix_secs, snap2.snapshot_at_unix_secs);
    }

    #[tokio::test]
    async fn aggregator_recollects_after_invalidate() {
        let mut aggregator = HealthAggregator::new(Duration::from_secs(60));
        aggregator.add_producer(Arc::new(FakeProducer {
            name: "test",
            status: HealthStatus::Healthy,
        }));

        let snap1 = aggregator.snapshot(1000).await;
        aggregator.invalidate_cache();
        let snap2 = aggregator.snapshot(2000).await;
        // 失效后重新收集：snapshot_at_unix_secs 应不同。
        assert_ne!(snap1.snapshot_at_unix_secs, snap2.snapshot_at_unix_secs);
    }
}
