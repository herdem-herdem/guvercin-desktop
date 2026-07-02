use std::path::PathBuf;

#[derive(Debug)]
pub enum KeyStoreError {
    NotFound,
    Other(String),
}

pub async fn load_master_key(_prompt: &str) -> Result<Vec<u8>, KeyStoreError> {
    let path = get_key_path();
    match tokio::fs::read(&path).await {
        Ok(data) => Ok(data),
        Err(_) => Err(KeyStoreError::NotFound),
    }
}

pub async fn store_master_key(_prompt: &str, key: &[u8]) -> Result<(), KeyStoreError> {
    let path = get_key_path();
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    tokio::fs::write(&path, key).await.map_err(|e| KeyStoreError::Other(e.to_string()))
}

fn get_key_path() -> PathBuf {
    let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("com.guvercin.app");
    path.push("master.key");
    path
}
