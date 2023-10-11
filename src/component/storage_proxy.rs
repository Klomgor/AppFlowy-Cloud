use async_trait::async_trait;
use collab::core::collab::MutexCollab;
use database::collab::{CollabPostgresDBStorageImpl, CollabStorage, StorageConfig};
use database_entity::{
  AFCollabSnapshots, InsertCollabParams, InsertSnapshotParams, QueryCollabParams,
  QueryCollabResult, QueryObjectSnapshotParams, QuerySnapshotParams, RawData,
};
use itertools::{Either, Itertools};
use std::{
  collections::HashMap,
  sync::{Arc, Weak},
};
use tokio::sync::RwLock;
use tracing::info;
use validator::Validate;

#[derive(Clone)]
pub struct CollabStorageProxy {
  inner: CollabPostgresDBStorageImpl,
  collab_by_object_id: Arc<RwLock<HashMap<String, Weak<MutexCollab>>>>,
}

impl CollabStorageProxy {
  pub fn new(inner: CollabPostgresDBStorageImpl) -> Self {
    Self {
      inner,
      collab_by_object_id: Arc::new(RwLock::new(HashMap::new())),
    }
  }
}

#[async_trait]
impl CollabStorage for CollabStorageProxy {
  fn config(&self) -> &StorageConfig {
    self.inner.config()
  }

  async fn is_exist(&self, object_id: &str) -> bool {
    self.inner.is_exist(object_id).await
  }

  async fn cache_collab(&self, object_id: &str, collab: Weak<MutexCollab>) {
    tracing::trace!("Cache collab:{} in memory", object_id);
    self
      .collab_by_object_id
      .write()
      .await
      .insert(object_id.to_string(), collab);
  }

  async fn insert_collab(
    &self,
    owner_uid: i64,
    params: InsertCollabParams,
  ) -> database::collab::Result<()> {
    self.inner.insert_collab(owner_uid, params).await
  }

  async fn get_collab(&self, params: QueryCollabParams) -> database::collab::Result<RawData> {
    let collab = self
      .collab_by_object_id
      .read()
      .await
      .get(&params.object_id)
      .and_then(|collab| collab.upgrade());

    match collab {
      None => self.inner.get_collab(params).await,
      Some(collab) => {
        info!("Get collab data:{} from memory", params.object_id);
        let data = collab.encode_as_update_v1().0;
        Ok(data)
      },
    }
  }

  async fn batch_get_collab(
    &self,
    queries: Vec<QueryCollabParams>,
  ) -> HashMap<String, QueryCollabResult> {
    let (valid_queries, mut results): (Vec<_>, HashMap<_, _>) =
      queries
        .into_iter()
        .partition_map(|params| match params.validate() {
          Ok(_) => Either::Left(params),
          Err(err) => Either::Right((
            params.object_id,
            QueryCollabResult::Failed {
              error: err.to_string(),
            },
          )),
        });

    let read_guard = self.collab_by_object_id.read().await;
    let (results_from_memory, queries): (HashMap<_, _>, Vec<_>) =
      valid_queries.into_iter().partition_map(|params| {
        match read_guard
          .get(&params.object_id)
          .and_then(|collab| collab.upgrade())
        {
          Some(collab) => Either::Left((
            params.object_id,
            QueryCollabResult::Success {
              blob: collab.encode_as_update_v1().0,
            },
          )),
          None => Either::Right(params),
        }
      });

    results.extend(results_from_memory);
    results.extend(self.inner.batch_get_collab(queries).await);
    results
  }

  async fn delete_collab(&self, object_id: &str) -> database::collab::Result<()> {
    self.inner.delete_collab(object_id).await
  }

  async fn create_snapshot(&self, params: InsertSnapshotParams) -> database::collab::Result<()> {
    self.inner.create_snapshot(params).await
  }

  async fn get_snapshot_data(
    &self,
    params: QuerySnapshotParams,
  ) -> database::collab::Result<RawData> {
    self.inner.get_snapshot_data(params).await
  }

  async fn get_all_snapshots(
    &self,
    params: QueryObjectSnapshotParams,
  ) -> database::collab::Result<AFCollabSnapshots> {
    self.inner.get_all_snapshots(params).await
  }
}