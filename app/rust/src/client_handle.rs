use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use taskveil_client::{ClientError, LocalProfileConfig, TaskveilClient};

static CLIENT: OnceLock<TaskveilClient> = OnceLock::new();

pub(crate) fn init_client(db_dir: String, default_inbox_name: String) -> Result<(), ClientError> {
    let requested_path = local_profile_db_path(&db_dir);
    if let Some(existing) = CLIENT.get() {
        return ensure_same_path(existing, &requested_path);
    }

    let candidate = TaskveilClient::open(LocalProfileConfig::new(db_dir, default_inbox_name))?;
    match CLIENT.set(candidate) {
        Ok(()) => Ok(()),
        Err(candidate) => {
            let existing = CLIENT.get().ok_or(ClientError::RuntimeState)?;
            ensure_same_path(existing, candidate.db_path())
        }
    }
}

pub(crate) fn client() -> Result<&'static TaskveilClient, ClientError> {
    CLIENT.get().ok_or(ClientError::RuntimeState)
}

fn local_profile_db_path(db_dir: impl AsRef<Path>) -> PathBuf {
    db_dir.as_ref().join("taskveil.db")
}

fn ensure_same_path(client: &TaskveilClient, requested_path: &Path) -> Result<(), ClientError> {
    if client.db_path() == requested_path {
        Ok(())
    } else {
        Err(ClientError::ProfileBusy)
    }
}
