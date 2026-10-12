use crate::{
    cli::{Command, Options},
    copy_engine::{Outcome, Task},
    fsutil::{self, Directory},
    scheduler::Pool,
};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, Metadata, ReadDir},
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

fn trim_slashes(path: &Path) -> PathBuf {
    let bytes = path.as_os_str().as_bytes();
    let mut end = bytes.len();
    while end > 1 && bytes[end - 1] == b'/' {
        end -= 1;
    }
    PathBuf::from(OsString::from_vec(bytes[..end].to_vec()))
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub fn plan(command: &Command) -> io::Result<Vec<Task>> {
    let destination = trim_slashes(&command.destination);
    let destination_metadata = fsutil::optional_metadata(&destination)?;
    let destination_slash = command.destination.as_os_str().as_bytes().ends_with(b"/");
    if destination_slash && destination_metadata.as_ref().is_some_and(|m| !m.is_dir()) {
        return Err(fsutil::invalid(
            "a destination ending in / must be a directory",
        ));
    }
    let container = if destination_metadata.as_ref().is_some_and(Metadata::is_dir) {
        Some(Directory::open(&destination)?)
    } else {
        None
    };
    if command.sources.len() > 1 && container.is_none() {
        return Err(fsutil::invalid(
            "multiple sources require an existing destination directory",
        ));
    }
    let mut roots = Vec::new();
    for path in &command.sources {
        let source = trim_slashes(path);
        let bytes = source.as_os_str().as_bytes();
        let contents = bytes == b"." || bytes.ends_with(b"/.") || bytes == b"/";
        let (source_parent, source_name) = if contents {
            (
                Directory::open(&fs::canonicalize(&source)?)?,
                OsString::from("."),
            )
        } else {
            (
                Directory::open(&fs::canonicalize(parent(&source))?)?,
                source
                    .file_name()
                    .ok_or_else(|| fsutil::invalid("source has no filename"))?
                    .to_os_string(),
            )
        };
        let source_path = if contents {
            source_parent.real_path()?
        } else {
            source_parent.real_path()?.join(&source_name)
        };
        let metadata = fs::symlink_metadata(source_parent.path().join(&source_name))
            .map_err(|error| io::Error::new(error.kind(), format!("{source:?}: {error}")))?;
        if (path.as_os_str().as_bytes().ends_with(b"/") && metadata.is_file())
            || (destination_slash && container.is_none() && !metadata.is_dir())
        {
            return Err(fsutil::invalid("a regular file path must not end in /"));
        }
        let (dest_parent, name) = match &container {
            Some(directory) => (
                directory.clone(),
                if contents {
                    OsString::from(".")
                } else {
                    source_name.clone()
                },
            ),
            None => (
                Directory::open(parent(&destination))?,
                destination
                    .file_name()
                    .ok_or_else(|| fsutil::invalid("destination has no filename"))?
                    .to_os_string(),
            ),
        };
        let dest_path = if name == "." {
            dest_parent.real_path()?
        } else {
            dest_parent.real_path()?.join(&name)
        };
        fsutil::validate_destination(
            &metadata,
            fsutil::optional_metadata(&dest_parent.path().join(&name))?.as_ref(),
        )
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("{source_path:?} -> {dest_path:?}: {error}"),
            )
        })?;
        roots.push(Task {
            source_parent,
            source_name,
            source_path,
            parent: dest_parent,
            name,
            destination: dest_path,
            metadata,
        });
    }
    for (index, root) in roots.iter().enumerate() {
        for source in &roots {
            if source.metadata.is_dir()
                && (root.destination.starts_with(&source.source_path)
                    || source.source_path.starts_with(&root.destination)
                    || root.parent.has_ancestor(&source.metadata)?)
            {
                return Err(fsutil::invalid(format!(
                    "source and destination trees overlap: {:?} -> {:?}",
                    source.source_path, root.destination
                )));
            }
            if root.destination == source.source_path {
                return Err(fsutil::invalid("a destination would overwrite a source"));
            }
        }
        for other in &roots[..index] {
            if root.destination.starts_with(&other.destination)
                || other.destination.starts_with(&root.destination)
            {
                return Err(fsutil::invalid(
                    "source roots map to colliding destinations",
                ));
            }
        }
    }
    Ok(roots)
}

struct Frame {
    source: Directory,
    source_path: PathBuf,
    destination: Option<Directory>,
    destination_path: PathBuf,
    metadata: Metadata,
    entries: ReadDir,
}

fn enter(
    source_parent: &Directory,
    source_name: &OsStr,
    destination_parent: Option<&Directory>,
    name: &OsStr,
    destination_path: PathBuf,
    metadata: Metadata,
    dry_run: bool,
) -> io::Result<Frame> {
    let source = source_parent.child(source_name)?;
    let source_path = source.real_path()?;
    if !fsutil::same_file(&metadata, &source.metadata()?) {
        return Err(fsutil::invalid("source directory changed before traversal"));
    }
    let entries = fs::read_dir(source.path())?;
    let destination = if let Some(directory) = destination_parent {
        if directory.has_ancestor(&metadata)? {
            return Err(fsutil::invalid(
                "destination directory is inside the source directory",
            ));
        }
        let existing = fsutil::optional_metadata(&directory.path().join(name))?;
        fsutil::validate_destination(&metadata, existing.as_ref())?;
        if let Some(existing) = existing.as_ref().filter(|m| m.is_dir()) {
            if source.has_ancestor(existing)? {
                return Err(fsutil::invalid(
                    "destination directory contains the source directory",
                ));
            }
        }
        if dry_run && existing.is_none() {
            println!("mkdir {destination_path:?}");
            None
        } else if dry_run {
            Some(directory.child(name)?)
        } else {
            Some(directory.ensure_child(name)?)
        }
    } else {
        if dry_run {
            println!("mkdir {destination_path:?}");
        }
        None
    };
    if let Some(directory) = &destination {
        if fsutil::same_file(&metadata, &directory.metadata()?) {
            return Err(fsutil::invalid(
                "source and destination are the same directory",
            ));
        }
    }
    Ok(Frame {
        source,
        source_path,
        destination,
        destination_path,
        metadata,
        entries,
    })
}

fn preview(
    source: &Path,
    destination: &Path,
    metadata: &Metadata,
    parent: Option<&Directory>,
    name: &OsStr,
    options: Options,
    pool: &Pool,
) -> io::Result<()> {
    if !metadata.is_file() && !metadata.file_type().is_symlink() {
        return Err(fsutil::invalid(format!(
            "{source:?}: unsupported special file"
        )));
    }
    let existing = match parent {
        Some(directory) => fsutil::optional_metadata(&directory.path().join(name))?,
        None => None,
    };
    fsutil::validate_destination(metadata, existing.as_ref())?;
    let skip = options.sync
        && metadata.is_file()
        && existing.is_some_and(|m| m.is_file() && m.len() == metadata.len());
    println!(
        "{} {source:?} -> {destination:?}",
        if skip { "skip" } else { "copy" }
    );
    pool.summary.observe(if skip {
        Outcome::Skipped
    } else {
        Outcome::Copied(if metadata.is_file() {
            metadata.len()
        } else {
            0
        })
    });
    Ok(())
}

pub fn scan(root: Task, options: Options, pool: &Pool) -> io::Result<()> {
    if !root.metadata.is_dir() {
        return if options.dry_run {
            preview(
                &root.source_path,
                &root.destination,
                &root.metadata,
                Some(&root.parent),
                &root.name,
                options,
                pool,
            )
        } else {
            pool.submit(root)
        };
    }
    let mut stack = vec![enter(
        &root.source_parent,
        &root.source_name,
        Some(&root.parent),
        &root.name,
        root.destination.clone(),
        root.metadata,
        options.dry_run,
    )?];
    while let Some(frame) = stack.last_mut() {
        fsutil::check_cancelled()?;
        let entry = match frame.entries.next() {
            Some(Ok(entry)) => entry,
            Some(Err(error)) => {
                pool.summary
                    .fail(format!("{:?}: {error}", frame.source_path));
                continue;
            }
            None => {
                let frame = stack.pop().unwrap();
                if !options.dry_run {
                    pool.drain();
                    if let Err(error) = frame.destination.unwrap().preserve(&frame.metadata) {
                        pool.summary
                            .fail(format!("{:?}: metadata: {error}", frame.destination_path));
                    }
                }
                continue;
            }
        };
        let name = entry.file_name();
        let source_path = frame.source_path.join(&name);
        let destination = frame.destination_path.join(&name);
        let metadata = match fs::symlink_metadata(frame.source.path().join(&name)) {
            Ok(metadata) => metadata,
            Err(error) => {
                pool.summary.fail(format!("{source_path:?}: {error}"));
                continue;
            }
        };
        if metadata.is_dir() {
            match enter(
                &frame.source,
                &name,
                frame.destination.as_ref(),
                &name,
                destination.clone(),
                metadata,
                options.dry_run,
            ) {
                Ok(child) => stack.push(child),
                Err(error) => pool
                    .summary
                    .fail(format!("{source_path:?} -> {destination:?}: {error}")),
            }
        } else if options.dry_run {
            if let Err(error) = preview(
                &source_path,
                &destination,
                &metadata,
                frame.destination.as_ref(),
                &name,
                options,
                pool,
            ) {
                pool.summary.fail(error);
            }
        } else {
            pool.submit(Task {
                source_parent: frame.source.clone(),
                source_name: name.clone(),
                source_path,
                parent: frame.destination.as_ref().unwrap().clone(),
                name,
                destination,
                metadata,
            })?;
        }
    }
    Ok(())
}
