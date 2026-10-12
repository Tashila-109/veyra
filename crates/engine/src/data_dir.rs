use crate::platform;
#[cfg(any(debug_assertions, test))]
use std::fs;
use std::{
    env, fmt,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct DataDir {
    path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    AlreadyRunning,
    Io(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("Veyra is already running"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LockError {}

impl From<io::Error> for LockError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct InstanceLock {
    _file: File,
}

impl DataDir {
    pub fn resolve() -> io::Result<Self> {
        let path = match env::var_os("VEYRA_DATA_DIR") {
            Some(path) => PathBuf::from(path),
            None => platform::default_data_dir()?,
        };
        Self::at(path)
    }

    pub fn at(path: PathBuf) -> io::Result<Self> {
        let path = if path.is_absolute() {
            path
        } else {
            env::current_dir()?.join(path)
        };
        Self::validate_socket_path(&path)?;
        for folder in [&path, &path.join("logs"), &path.join("run")] {
            platform::private_dir(folder)?;
        }
        Ok(Self { path })
    }

    fn validate_socket_path(path: &Path) -> io::Result<()> {
        #[cfg(debug_assertions)]
        let socket = path.join("control.sock");
        #[cfg(not(debug_assertions))]
        let socket = path.join("veyra.sock");
        if socket.as_os_str().as_encoded_bytes().len() > platform::SOCKET_PATH_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "socket path exceeds {} bytes: {}",
                    platform::SOCKET_PATH_MAX,
                    socket.display()
                ),
            ));
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lock(&self) -> Result<InstanceLock, LockError> {
        let mut file = platform::private_file(
            &self.path.join("veyra.lock"),
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false),
        )?;
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match platform::lock_exclusive(&file) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(LockError::AlreadyRunning);
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        }
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        #[cfg(debug_assertions)]
        match fs::remove_file(self.path.join("control.sock")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(InstanceLock { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        PathBuf::from(format!(
            "/tmp/veyra-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn socket_path_limit() {
        #[cfg(debug_assertions)]
        let suffix = "/control.sock";
        #[cfg(not(debug_assertions))]
        let suffix = "/veyra.sock";
        let path = PathBuf::from(format!("/tmp/{}", "x".repeat(104 - 5 - suffix.len())));
        let error = DataDir::validate_socket_path(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains(&path.display().to_string()));
        let allowed = PathBuf::from(format!("/tmp/{}", "x".repeat(103 - 5 - suffix.len())));
        assert!(DataDir::validate_socket_path(&allowed).is_ok());
    }

    #[test]
    fn lock_refuses_second() {
        let path = scratch();
        let _scratch = crate::tests::Scratch(path.clone());
        let dir = DataDir::at(path.clone()).unwrap();
        let first = dir.lock().unwrap();
        assert!(matches!(dir.lock(), Err(LockError::AlreadyRunning)));
        assert_eq!(
            fs::read_to_string(path.join("veyra.lock")).unwrap(),
            format!("{}\n", std::process::id())
        );
        drop(first);
        assert!(dir.lock().is_ok());
    }

    #[test]
    fn private_folders_and_stale_socket() {
        let path = scratch();
        let _scratch = crate::tests::Scratch(path.clone());
        let dir = DataDir::at(path.clone()).unwrap();
        for folder in [&path, &path.join("logs"), &path.join("run")] {
            assert!(platform::is_private(folder).unwrap());
        }
        #[cfg(debug_assertions)]
        {
            fs::write(path.join("control.sock"), "stale").unwrap();
            let lock = dir.lock().unwrap();
            assert!(platform::is_private(&path.join("veyra.lock")).unwrap());
            assert!(!path.join("control.sock").exists());
            drop(lock);
            fs::write(path.join("control.sock"), "stale").unwrap();
            let listener = platform::bind_socket(&path.join("control.sock")).unwrap();
            assert!(platform::is_private(&path.join("control.sock")).unwrap());
            let stream = platform::connect_socket(&path.join("control.sock")).unwrap();
            let (peer, _) = listener.accept().unwrap();
            drop((stream, peer, listener));
        }
    }
}
