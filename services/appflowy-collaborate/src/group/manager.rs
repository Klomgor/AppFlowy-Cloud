use std::sync::Arc;
use std::time::Duration;

use collab::core::collab::DataSource;
use collab::core::origin::CollabOrigin;
use collab::entity::EncodedCollab;
use collab::preclude::Collab;
use collab_entity::CollabType;
use tracing::{instrument, trace};

use access_control::collab::RealtimeAccessControl;
use app_error::AppError;
use collab_rt_entity::user::RealtimeUser;
use collab_rt_entity::CollabMessage;
use collab_stream::client::CollabRedisStream;
use database::collab::{CollabStorage, GetCollabOrigin};
use database_entity::dto::QueryCollabParams;

use crate::client::client_msg_router::ClientMessageRouter;
use crate::error::{CreateGroupFailedReason, RealtimeError};
use crate::group::group_init::CollabGroup;
use crate::group::state::GroupManagementState;
use crate::indexer::IndexerProvider;
use crate::metrics::CollabRealtimeMetrics;

pub struct GroupManager<S> {
  state: GroupManagementState,
  storage: Arc<S>,
  access_control: Arc<dyn RealtimeAccessControl>,
  metrics_calculate: Arc<CollabRealtimeMetrics>,
  collab_redis_stream: Arc<CollabRedisStream>,
  persistence_interval: Duration,
  prune_grace_period: Duration,
  indexer_provider: Arc<IndexerProvider>,
}

impl<S> GroupManager<S>
where
  S: CollabStorage,
{
  #[allow(clippy::too_many_arguments)]
  pub async fn new(
    storage: Arc<S>,
    access_control: Arc<dyn RealtimeAccessControl>,
    metrics_calculate: Arc<CollabRealtimeMetrics>,
    collab_stream: CollabRedisStream,
    persistence_interval: Duration,
    prune_grace_period: Duration,
    indexer_provider: Arc<IndexerProvider>,
  ) -> Result<Self, RealtimeError> {
    let collab_stream = Arc::new(collab_stream);
    Ok(Self {
      state: GroupManagementState::new(metrics_calculate.clone()),
      storage,
      access_control,
      metrics_calculate,
      collab_redis_stream: collab_stream,
      persistence_interval,
      prune_grace_period,
      indexer_provider,
    })
  }

  pub async fn get_inactive_groups(&self) -> Vec<String> {
    self.state.get_inactive_group_ids().await
  }

  pub async fn contains_user(&self, object_id: &str, user: &RealtimeUser) -> bool {
    self.state.contains_user(object_id, user).await
  }

  pub async fn remove_user(&self, user: &RealtimeUser) {
    self.state.remove_user(user).await;
  }

  pub async fn contains_group(&self, object_id: &str) -> bool {
    self.state.contains_group(object_id).await
  }

  pub async fn get_group(&self, object_id: &str) -> Option<Arc<CollabGroup>> {
    self.state.get_group(object_id).await
  }

  #[instrument(skip(self))]
  async fn remove_group(&self, object_id: &str) {
    self.state.remove_group(object_id).await;
  }

  pub async fn subscribe_group(
    &self,
    user: &RealtimeUser,
    object_id: &str,
    message_origin: &CollabOrigin,
    client_msg_router: &mut ClientMessageRouter,
  ) -> Result<(), RealtimeError> {
    // Lock the group and subscribe the user to the group.
    if let Some(mut e) = self.state.get_mut_group(object_id).await {
      let group = e.value_mut();
      trace!("[realtime]: {} subscribe group:{}", user, object_id,);
      let (sink, stream) = client_msg_router.init_client_communication::<CollabMessage>(
        group.workspace_id(),
        user,
        object_id,
        self.access_control.clone(),
      );
      group.subscribe(user, message_origin.clone(), sink, stream);
      // explicitly drop the group to release the lock.
      drop(e);

      self.state.insert_user(user, object_id).await?;
    } else {
      // When subscribing to a group, the group should exist. Otherwise, it's a bug.
      return Err(RealtimeError::GroupNotFound(object_id.to_string()));
    }

    Ok(())
  }

  pub async fn create_group(
    &self,
    user: &RealtimeUser,
    workspace_id: &str,
    object_id: &str,
    collab_type: CollabType,
  ) -> Result<(), RealtimeError> {
    let mut is_new_collab = true;
    // Ensure the workspace_id matches the metadata's workspace_id when creating a collaboration object
    // of type [CollabType::Folder]. In this case, both the object id and the workspace id should be
    // identical.
    if let Ok(metadata) = self
      .storage
      .query_collab_meta(object_id, &collab_type)
      .await
    {
      if metadata.workspace_id != workspace_id {
        let err =
          RealtimeError::CreateGroupFailed(CreateGroupFailedReason::CollabWorkspaceIdNotMatch {
            expect: metadata.workspace_id,
            actual: workspace_id.to_string(),
            detail: format!(
              "user_id:{},app_version:{},object_id:{}:{}",
              user.uid, user.app_version, object_id, collab_type
            ),
          });
        return Err(err);
      }
      is_new_collab = false;
    }

    trace!(
      "[realtime]: create group: uid:{},workspace_id:{},object_id:{}:{}",
      user.uid,
      workspace_id,
      object_id,
      collab_type
    );

    let mut indexer = self.indexer_provider.indexer_for(collab_type.clone());
    if indexer.is_some()
      && !self
        .indexer_provider
        .can_index_workspace(workspace_id)
        .await
        .map_err(|e| RealtimeError::Internal(e.into()))?
    {
      tracing::trace!("workspace {} indexing is disabled", workspace_id);
      indexer = None;
    }
    let group = Arc::new(
      CollabGroup::new(
        user.uid,
        workspace_id.to_string(),
        object_id.to_string(),
        collab_type,
        self.metrics_calculate.clone(),
        self.storage.clone(),
        is_new_collab,
        self.collab_redis_stream.clone(),
        self.persistence_interval,
        self.prune_grace_period,
        indexer,
      )
      .await?,
    );
    self.state.insert_group(object_id, group.clone()).await;
    Ok(())
  }
}

#[instrument(level = "trace", skip_all)]
async fn load_collab<S>(
  uid: i64,
  object_id: &str,
  params: QueryCollabParams,
  storage: Arc<S>,
) -> Result<(Collab, EncodedCollab), AppError>
where
  S: CollabStorage,
{
  let encode_collab = storage
    .get_encode_collab(GetCollabOrigin::User { uid }, params.clone(), false)
    .await?;
  let result = Collab::new_with_source(
    CollabOrigin::Server,
    object_id,
    DataSource::DocStateV1(encode_collab.doc_state.to_vec()),
    vec![],
    false,
  );
  match result {
    Ok(collab) => Ok((collab, encode_collab)),
    Err(err) => load_collab_from_snapshot(object_id, params, storage)
      .await
      .ok_or_else(|| AppError::Internal(err.into())),
  }
}

async fn load_collab_from_snapshot<S>(
  object_id: &str,
  params: QueryCollabParams,
  storage: Arc<S>,
) -> Option<(Collab, EncodedCollab)>
where
  S: CollabStorage,
{
  let encode_collab = get_latest_snapshot(
    &params.workspace_id,
    object_id,
    &storage,
    &params.collab_type,
  )
  .await?;
  let collab = Collab::new_with_source(
    CollabOrigin::Server,
    object_id,
    DataSource::DocStateV1(encode_collab.doc_state.to_vec()),
    vec![],
    false,
  )
  .ok()?;
  Some((collab, encode_collab))
}

async fn get_latest_snapshot<S>(
  workspace_id: &str,
  object_id: &str,
  storage: &S,
  collab_type: &CollabType,
) -> Option<EncodedCollab>
where
  S: CollabStorage,
{
  let metas = storage.get_collab_snapshot_list(object_id).await.ok()?.0;
  for meta in metas {
    let snapshot_data = storage
      .get_collab_snapshot(workspace_id, &meta.object_id, &meta.snapshot_id)
      .await
      .ok()?;
    if let Ok(encoded_collab) = EncodedCollab::decode_from_bytes(&snapshot_data.encoded_collab_v1) {
      if let Ok(collab) = Collab::new_with_source(
        CollabOrigin::Empty,
        object_id,
        DataSource::DocStateV1(encoded_collab.doc_state.to_vec()),
        vec![],
        false,
      ) {
        // TODO(nathan): this check is not necessary, can be removed in the future.
        collab_type.validate_require_data(&collab).ok()?;
        return Some(encoded_collab);
      }
    }
  }
  None
}
