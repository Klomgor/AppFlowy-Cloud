use collab::error::CollabError;
use collab_stream::error::StreamError;
use std::fmt::Display;

#[derive(Debug, thiserror::Error)]
pub enum RealtimeError {
  #[error(transparent)]
  YSync(#[from] collab_rt_protocol::RTProtocolError),

  #[error(transparent)]
  YAwareness(#[from] collab::core::awareness::Error),

  #[error("failed to deserialize message: {0}")]
  YrsDecodingError(#[from] yrs::encoding::read::Error),

  #[error(transparent)]
  SerdeError(#[from] serde_json::Error),

  #[error(transparent)]
  TokioTask(#[from] tokio::task::JoinError),

  #[error(transparent)]
  IO(#[from] std::io::Error),

  #[error("Unexpected data: {0}")]
  UnexpectedData(&'static str),

  #[error("Expected init sync message, but received: {0}")]
  ExpectInitSync(String),

  #[error(transparent)]
  CollabError(#[from] CollabError),

  #[error("Received message from client:{0}, but the client does not have sufficient permissions to write")]
  NotEnoughPermissionToWrite(i64),

  #[error("Client:{0} does not have enough permission to read")]
  NotEnoughPermissionToRead(i64),

  #[error("{0}")]
  UserNotFound(String),

  #[error("group is not exist: {0}")]
  GroupNotFound(String),

  #[error("Create group failed:{0}")]
  CreateGroupFailed(CreateGroupFailedReason),

  #[error("Lack of required collab data: {0}")]
  NoRequiredCollabData(String),

  #[error("{0} send too many messages")]
  TooManyMessage(String),

  #[error("Acquire lock timeout")]
  LockTimeout,

  #[error("Internal failure: {0}")]
  Internal(#[from] anyhow::Error),

  #[error("Collab redis stream error: {0}")]
  StreamError(#[from] StreamError),

  #[error("Cannot create group: {0}")]
  CannotCreateGroup(String),

  #[error("BinCodeCollab error: {0}")]
  BincodeEncode(String),

  #[error("Failed to create snapshot: {0}")]
  CreateSnapshotFailed(String),

  #[error("Failed to get latest snapshot: {0}")]
  GetLatestSnapshotFailed(String),

  #[error("Collab Schema Error: {0}")]
  CollabSchemaError(String),

  #[error("failed to obtain lease: {0}")]
  Lease(Box<dyn std::error::Error + Send + Sync>),

  #[error("failed to send ws message: {0}")]
  SendWSMessageFailed(String),

  #[error("failed to parse UUID: {0}")]
  Uuid(#[from] uuid::Error),
}

#[derive(Debug)]
pub enum CreateGroupFailedReason {
  CollabWorkspaceIdNotMatch { expect: String, detail: String },
  CannotGetCollabData,
}

impl Display for CreateGroupFailedReason {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      CreateGroupFailedReason::CollabWorkspaceIdNotMatch { expect, detail } => {
        write!(
          f,
          "Collab workspace id not match: expect {}, detail: {}",
          expect, detail
        )
      },
      CreateGroupFailedReason::CannotGetCollabData => {
        write!(f, "Cannot get collab data")
      },
    }
  }
}

impl RealtimeError {
  pub fn is_too_many_message(&self) -> bool {
    matches!(self, RealtimeError::TooManyMessage(_))
  }

  pub fn is_lock_timeout(&self) -> bool {
    matches!(self, RealtimeError::LockTimeout)
  }
  pub fn is_create_group_failed(&self) -> bool {
    matches!(self, RealtimeError::CreateGroupFailed(_))
  }
}
