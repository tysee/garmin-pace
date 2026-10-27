//! src/private_fs.rs enforces the shared Unix filesystem boundary for credentials and SQLite.
//! It exposes owner-only directory, descriptor and database checks without reading secret values.

use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub(crate) fn directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .context("creating private storage directory")?;
    check_directory(path)
}

pub(crate) fn check_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("reading storage directory metadata")?;
    ensure!(
        metadata.is_dir() && !metadata.is_symlink(),
        "storage directory must be a real directory"
    );
    private_owner(&metadata)?;
    Ok(())
}

fn private_owner(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw(),
            "private storage must belong to the current OS user"
        );
        ensure!(
            metadata.mode() & 0o077 == 0,
            "private storage permissions required: directories 0700, files 0600"
        );
    }
    #[cfg(not(unix))]
    anyhow::bail!("private storage requires a supported Unix filesystem");
    Ok(())
}

pub(crate) fn open(path: &Path, write: bool, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path).context("opening private file")?;
    check_file(&file.metadata().context("checking private file")?)?;
    Ok(file)
}

fn check_file(metadata: &fs::Metadata) -> Result<()> {
    ensure!(metadata.is_file(), "private storage must be a regular file");
    private_owner(metadata)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1,
            "private files must not have hard-link aliases"
        );
    }
    Ok(())
}

pub(crate) fn create_new(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .context("creating private output; target must not exist")
}

pub(crate) fn database(path: &Path, create: bool) -> Result<()> {
    let parent = path.parent().context("database path has no parent")?;
    if create {
        directory(parent)?;
    } else {
        check_directory(parent)?;
    }
    // Closing a second descriptor to an existing SQLite file releases the process's
    // POSIX locks. Inspect metadata only; SQLite opens with NOFOLLOW in this private directory.
    match fs::symlink_metadata(path) {
        Ok(metadata) => check_file(&metadata)?,
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            drop(create_new(path)?);
        }
        Err(error) => return Err(error).context("checking private database"),
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        match fs::symlink_metadata(Path::new(&sidecar)) {
            Ok(metadata) => check_file(&metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error).context("checking SQLite sidecar"),
        }
    }
    Ok(())
}
