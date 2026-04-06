use crate::{
    manager::SshSessionManager,
    model::{SftpEntry, SshError},
};

impl SshSessionManager {
    pub fn sftp_canonicalize(
        &self,
        session_id: u64,
        path: &str,
    ) -> std::result::Result<String, SshError> {
        let sftp = self.get_sftp(session_id)?;
        let path = path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            session
                .canonicalize(path)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))
        })
    }

    pub fn sftp_list_dir(
        &self,
        session_id: u64,
        path: &str,
    ) -> std::result::Result<Vec<SftpEntry>, SshError> {
        let sftp = self.get_sftp(session_id)?;
        let path = path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            let dir = session
                .read_dir(&path)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))?;

            let mut entries = Vec::new();
            for entry in dir {
                let name = entry.file_name();
                if name == "." {
                    continue;
                }
                let entry_path = if path == "/" {
                    format!("/{name}")
                } else {
                    format!("{path}/{name}")
                };
                let is_dir = entry.metadata().is_dir();
                let size = entry.metadata().size.unwrap_or(0);
                let modified = entry.metadata().mtime;
                let permissions = entry.metadata().permissions;
                entries.push(SftpEntry {
                    name,
                    path: entry_path,
                    is_dir,
                    size,
                    modified,
                    permissions,
                });
            }
            Ok(entries)
        })
    }

    pub fn sftp_read_file(
        &self,
        session_id: u64,
        remote_path: &str,
    ) -> std::result::Result<Vec<u8>, SshError> {
        let sftp = self.get_sftp(session_id)?;
        let remote_path = remote_path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            session
                .read(remote_path)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))
        })
    }

    pub fn sftp_write_file(
        &self,
        session_id: u64,
        remote_path: &str,
        data: &[u8],
    ) -> std::result::Result<(), SshError> {
        let sftp = self.get_sftp(session_id)?;
        let remote_path = remote_path.to_string();
        let data = data.to_vec();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            session
                .write(remote_path, &data)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))
        })
    }

    pub fn sftp_mkdir(&self, session_id: u64, path: &str) -> std::result::Result<(), SshError> {
        let sftp = self.get_sftp(session_id)?;
        let path = path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            session
                .create_dir(path)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))
        })
    }

    pub fn sftp_remove(
        &self,
        session_id: u64,
        path: &str,
        is_dir: bool,
    ) -> std::result::Result<(), SshError> {
        let sftp = self.get_sftp(session_id)?;
        let path = path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            if is_dir {
                session
                    .remove_dir(path)
                    .await
                    .map_err(|err| SshError::SftpOperation(err.to_string()))
            } else {
                session
                    .remove_file(path)
                    .await
                    .map_err(|err| SshError::SftpOperation(err.to_string()))
            }
        })
    }

    pub fn sftp_rename(
        &self,
        session_id: u64,
        old_path: &str,
        new_path: &str,
    ) -> std::result::Result<(), SshError> {
        let sftp = self.get_sftp(session_id)?;
        let old_path = old_path.to_string();
        let new_path = new_path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            session
                .rename(old_path, new_path)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))
        })
    }

    pub fn sftp_metadata(
        &self,
        session_id: u64,
        path: &str,
    ) -> std::result::Result<SftpEntry, SshError> {
        let sftp = self.get_sftp(session_id)?;
        let path_str = path.to_string();
        self.runtime.block_on(async {
            let session = sftp.lock().await;
            let meta = session
                .metadata(&path_str)
                .await
                .map_err(|err| SshError::SftpOperation(err.to_string()))?;
            let name = path_str.rsplit('/').next().unwrap_or(&path_str).to_string();
            Ok(SftpEntry {
                name,
                path: path_str,
                is_dir: meta.is_dir(),
                size: meta.size.unwrap_or(0),
                modified: meta.mtime,
                permissions: meta.permissions,
            })
        })
    }
}
