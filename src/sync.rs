use anyhow::Result;
use blake3::Hasher;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use trash::delete;
use walkdir::WalkDir;

#[derive(Debug, Clone, Default)]
pub struct SyncStats {
	pub copied: u64,
	pub skipped: u64,
	pub deleted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
	Removed,
	RecycleBinFailed,
}

pub struct SyncOptions {
	pub use_recycle_bin: bool,
	pub verbose: bool,
}

impl Default for SyncOptions {
	fn default() -> Self {
		SyncOptions {
			use_recycle_bin: true,
			verbose: false,
		}
	}
}

/// Mirror `source` directory contents into `dest` directory.
/// After this call `dest` will be a directory whose tree matches `source`.
pub fn sync_folder(source: &Path, dest: &Path, opts: &SyncOptions) -> Result<SyncStats> {
	let mut stats = SyncStats::default();

	if !source.exists() {
		anyhow::bail!("Source does not exist: {}", source.display());
	}

	if !source.is_dir() {
		anyhow::bail!("Source is not a folder: {}", source.display());
	}

	// If dest exists as a file, remove it so we can make a directory.
	if dest.exists() && !dest.is_dir() {
		handle_delete(dest, opts)?;
	}

	fs::create_dir_all(dest)?;

	// 1. Collect everything from the source first (so its WalkDir handles are
	//    released before we start touching dest). This avoids Windows
	//    "Incorrect function" (OS error 1) coming from the shell while a
	//    directory handle is still open.
	let mut src_files: Vec<PathBuf> = Vec::new(); // relative paths
	let mut src_dirs: Vec<PathBuf> = Vec::new();  // relative paths

	for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
		let rel = match entry.path().strip_prefix(source) {
			Ok(r) => r.to_path_buf(),
			Err(_) => continue,
		};

		if rel.as_os_str().is_empty() {
			continue;
		}

		if entry.file_type().is_dir() {
			src_dirs.push(rel);
		} else if entry.file_type().is_file() {
			src_files.push(rel);
		}
	}

	// 2. Create all directories in dest.
	for rel in &src_dirs {
		let dest_dir = dest.join(rel);

		if dest_dir.exists() && !dest_dir.is_dir() {
			handle_delete(&dest_dir, opts)?;
		}

		fs::create_dir_all(&dest_dir)?;
	}

	// 3. Copy / update all files.
	let src_file_set: HashSet<PathBuf> = src_files.iter().cloned().collect();
	let src_dir_set: HashSet<PathBuf> = src_dirs.iter().cloned().collect();

	for rel in &src_files {
		let src_file = source.join(rel);
		let dest_file = dest.join(rel);

		if let Some(parent) = dest_file.parent() {
			if parent.exists() && !parent.is_dir() {
				handle_delete(parent, opts)?;
			}
			fs::create_dir_all(parent)?;
		}

		let needs_copy = if !dest_file.exists() {
			true
		} else if dest_file.is_file() && files_equal(&src_file, &dest_file)? {
			false
		} else {
			true
		};

		if needs_copy {
			if dest_file.exists() {
				handle_delete(&dest_file, opts)?;
			}

			copy_file(&src_file, &dest_file)?;
			stats.copied += 1;

			if opts.verbose {
				eprintln!("COPIED: {}", rel.display());
			}
		} else {
			stats.skipped += 1;
		}
	}

	// 4. Collect everything currently in dest (also drop walker before delete).
	let mut dest_files: Vec<PathBuf> = Vec::new();
	let mut dest_dirs: Vec<PathBuf> = Vec::new();

	for entry in WalkDir::new(dest).min_depth(1).into_iter().filter_map(|e| e.ok()) {
		let rel = match entry.path().strip_prefix(dest) {
			Ok(r) => r.to_path_buf(),
			Err(_) => continue,
		};

		if entry.file_type().is_file() || entry.file_type().is_symlink() {
			dest_files.push(rel);
		} else if entry.file_type().is_dir() {
			dest_dirs.push(rel);
		}
	}

	// 5. Delete files in dest that are not in source.
	for rel in dest_files {
		if !src_file_set.contains(&rel) {
			let dest_file = dest.join(&rel);
			if dest_file.exists() {
				handle_delete(&dest_file, opts)?;
				stats.deleted += 1;

				if opts.verbose {
					eprintln!("DELETED: {}", rel.display());
				}
			}
		}
	}

	// 6. Delete dirs in dest that are not in source (deepest first).
	dest_dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));

	for rel in dest_dirs {
		if !src_dir_set.contains(&rel) {
			let dest_dir = dest.join(&rel);
			if dest_dir.exists() {
				handle_delete(&dest_dir, opts)?;
				stats.deleted += 1;

				if opts.verbose {
					eprintln!("DELETED DIR: {}", rel.display());
				}
			}
		}
	}

	Ok(stats)
}

pub fn sync_file(source: &Path, dest: &Path, opts: &SyncOptions) -> Result<SyncStats> {
	let mut stats = SyncStats::default();

	if source.is_dir() {
		return sync_folder(source, dest, opts);
	}

	if !source.is_file() {
		anyhow::bail!("Source is not a file: {}", source.display());
	}

	if let Some(parent) = dest.parent() {
		if parent.exists() && !parent.is_dir() {
			handle_delete(parent, opts)?;
		}
		fs::create_dir_all(parent)?;
	}

	let needs_copy = if !dest.exists() {
		true
	} else if dest.is_file() && files_equal(source, dest)? {
		false
	} else {
		true
	};

	if needs_copy {
		if dest.exists() {
			handle_delete(dest, opts)?;
		}

		copy_file(source, dest)?;
		stats.copied += 1;
	} else {
		stats.skipped += 1;
	}

	Ok(stats)
}

/// After all sources have been placed into `dest_dir`, delete any top-level
/// entry in `dest_dir` whose name is not in `keep_names`.
pub fn prune_top_level(
	dest_dir: &Path,
	keep_names: &HashSet<std::ffi::OsString>,
	opts: &SyncOptions,
) -> Result<u64> {
	let mut deleted = 0u64;

	// Collect first, then delete -- never delete while the ReadDir iterator
	// is still open (this is what triggers Windows OS error 1 with the shell).
	let mut to_remove: Vec<PathBuf> = Vec::new();

	for entry in fs::read_dir(dest_dir)? {
		let entry = entry?;
		let name = entry.file_name();
		if !keep_names.contains(&name) {
			to_remove.push(entry.path());
		}
	}

	for path in to_remove {
		if path.exists() {
			handle_delete(&path, opts)?;
			deleted += 1;

			if opts.verbose {
				eprintln!("PRUNED: {}", path.display());
			}
		}
	}

	Ok(deleted)
}

pub fn handle_delete(path: &Path, opts: &SyncOptions) -> Result<()> {
	match try_delete(path, opts) {
		DeleteOutcome::Removed => Ok(()),
		DeleteOutcome::RecycleBinFailed => {
			if opts.use_recycle_bin {
				anyhow::bail!(
					"RECYCLE_BIN_ERROR: Could not move {} to the Recycle Bin",
					path.display()
				);
			} else {
				anyhow::bail!("Failed to delete {} permanently", path.display());
			}
		}
	}
}

fn try_delete(path: &Path, opts: &SyncOptions) -> DeleteOutcome {
	if !path.exists() {
		return DeleteOutcome::Removed;
	}

	if opts.use_recycle_bin {
		// The `trash` crate's Windows backend (IFileOperation) refuses
		// extended-length paths like `\\?\A:\...`. Normalize first.
		let normalized = normalize_for_shell(path);

		if delete(&normalized).is_err() {
			return DeleteOutcome::RecycleBinFailed;
		}

		if path.exists() {
			return DeleteOutcome::RecycleBinFailed;
		}

		return DeleteOutcome::Removed;
	}

	match remove_path_permanently(path) {
		Ok(_) => DeleteOutcome::Removed,
		Err(_) => DeleteOutcome::RecycleBinFailed,
	}
}

/// Convert a path into a form that Windows shell APIs (used by `trash`) accept:
/// absolute, without the `\\?\` extended-length prefix.
fn normalize_for_shell(path: &Path) -> PathBuf {
	let abs = if path.is_absolute() {
		path.to_path_buf()
	} else {
		match std::env::current_dir() {
			Ok(cwd) => cwd.join(path),
			Err(_) => path.to_path_buf(),
		}
	};

	#[cfg(windows)]
	{
		let s = abs.to_string_lossy().to_string();
		let stripped = s
			.strip_prefix(r"\\?\UNC\")
			.map(|rest| format!(r"\\{}", rest))
			.or_else(|| s.strip_prefix(r"\\?\").map(|rest| rest.to_string()))
			.unwrap_or(s);
		return PathBuf::from(stripped);
	}

	#[cfg(not(windows))]
	{
		abs
	}
}

fn remove_path_permanently(path: &Path) -> Result<()> {
	let meta = match fs::symlink_metadata(path) {
		Ok(meta) => meta,
		Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
		Err(e) => return Err(e.into()),
	};

	if meta.file_type().is_dir() {
		fs::remove_dir_all(path)?;
	} else {
		fs::remove_file(path)?;
	}

	Ok(())
}

fn files_equal(a: &Path, b: &Path) -> Result<bool> {
	if !a.is_file() || !b.is_file() {
		return Ok(false);
	}

	let meta_a = fs::metadata(a)?;
	let meta_b = fs::metadata(b)?;

	if meta_a.len() != meta_b.len() {
		return Ok(false);
	}

	let hash_a = hash_file(a)?;
	let hash_b = hash_file(b)?;

	Ok(hash_a == hash_b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NEW,
    UPDATED,
    DELETED,
    UNCHANGED,
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub action: Action,
    pub rel: String,
    pub is_dir: bool,
}

pub fn hash_file(path: &Path) -> Result<[u8; 32]> {
	let mut file = File::open(path)?;
	let mut hasher = Hasher::new();
	let mut buf = [0u8; 65536];

	loop {
		let n = file.read(&mut buf)?;

		if n == 0 {
			break;
		}

		hasher.update(&buf[..n]);
	}

	Ok(*hasher.finalize().as_bytes())
}

/// Copy file contents using std::fs::copy, then preserve timestamps and permissions
/// to match shutil.copy2 behavior as closely as possible on the current platform.
fn copy_file_with_metadata(src: &Path, dst: &Path) -> Result<()> {
	std::fs::copy(src, dst)?;

#[cfg(unix)]
{
	use std::os::unix::fs::PermissionsExt;
	if let Ok(src_meta) = std::fs::metadata(src) {
		let perms = src_meta.permissions();
		if let Err(e) = std::fs::set_permissions(dst, perms) {
			eprintln!("Warning: could not set permissions on {}: {}", dst.display(), e);
		}
	}
}

	if let (Ok(src_meta), Ok(_dst_meta)) = (std::fs::metadata(src), std::fs::metadata(dst)) {
		if let Ok(src_modified) = src_meta.modified() {
			let _ = std::fs::File::options()
				.write(true)
				.open(dst)
				.and_then(|f| f.set_times(
					std::fs::FileTimes::new()
						.set_modified(src_modified)
						.set_accessed(src_modified),
				));
		}
	}

	Ok(())
}

/// Copy file contents, preferring `std::fs::copy` and falling back to a manual
/// read/write when `std::fs::copy` fails (e.g. `ERROR_INVALID_FUNCTION` on
/// some volumes where replicating metadata via `CopyFileExW` is not supported).
fn copy_file(src: &Path, dst: &Path) -> Result<()> {
	match std::fs::copy(src, dst) {
		Ok(_) => {
			if let Err(e) = copy_file_with_metadata(src, dst) {
				eprintln!("Warning: could not preserve metadata on {}: {}", dst.display(), e);
			}
			Ok(())
		}
		Err(_) => {
			let mut reader = File::open(src)?;
			let mut writer = File::create(dst)?;
			std::io::copy(&mut reader, &mut writer)?;
			writer.flush()?;
			if let Err(e) = copy_file_with_metadata(src, dst) {
				eprintln!("Warning: could not preserve metadata on {}: {}", dst.display(), e);
			}
			Ok(())
		}
	}
}

/// Build a sync plan comparing source paths against destination directory.
/// Returns a list of planned actions with relative paths.
pub fn compute_sync_plan(
    source_paths: &[PathBuf],
    dest_path: &Path,
    skip_unchanged: bool,
) -> Result<Vec<PlanItem>> {
    use Action::DELETED;

    if !dest_path.exists() || !dest_path.is_dir() {
        return Ok(Vec::new());
    }

    let mut plan = Vec::new();
    let mut keep_names = HashSet::new();

    for src_path in source_paths {
        if src_path.is_dir() {
            for entry in WalkDir::new(src_path).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                keep_names.insert(name.clone());
                let dst_item = dest_path.join(&name);
                plan.extend(plan_sync_item(entry.path(), &dst_item, skip_unchanged, ""));
            }
        } else {
            let name = src_path.file_name().unwrap().to_string_lossy().to_string();
            keep_names.insert(name.clone());
            let dst_item = dest_path.join(&name);
            plan.extend(plan_sync_item(src_path, &dst_item, skip_unchanged, ""));
        }
    }

    for dst_item in std::fs::read_dir(dest_path)? {
        let dst_item = dst_item?.path();
        let name = dst_item.file_name().unwrap().to_string_lossy().to_string();
        if !keep_names.contains(name.as_str()) {
            let rel = name.clone();
            plan.push(PlanItem {
                action: DELETED,
                rel,
                is_dir: dst_item.is_dir(),
            });
            if dst_item.is_dir() {
                add_deleted_rec(&dst_item, &name, &mut plan);
            }
        }
    }

    Ok(plan)
}

fn plan_sync_item(
    src_path: &Path,
    dst_path: &Path,
    skip_unchanged: bool,
    prefix: &str,
) -> Vec<PlanItem> {
    use Action::{DELETED, NEW, UPDATED, UNCHANGED};

    let mut plan = Vec::new();
    let name = src_path.file_name().unwrap().to_string_lossy().to_string();
    let full_rel = if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) };

    if src_path.is_dir() {
        if !dst_path.exists() {
            plan.push(PlanItem { action: NEW, rel: full_rel.clone(), is_dir: true });
            for entry in WalkDir::new(src_path).min_depth(1).into_iter().filter_map(|e| e.ok()) {
                let child_name = entry.file_name().to_string_lossy().to_string();
                plan.extend(plan_sync_item(entry.path(), &dst_path.join(&child_name), skip_unchanged, &full_rel));
            }
        } else if dst_path.is_dir() {
            let src_names: HashSet<String> = WalkDir::new(src_path).min_depth(1)
                .into_iter().filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .chain(
                    WalkDir::new(src_path).min_depth(1)
                        .into_iter().filter_map(|e| e.ok())
                        .filter(|e| e.file_type().is_file())
                        .map(|e| e.file_name().to_string_lossy().to_string())
                )
                .collect();

            if let Ok(entries) = dst_path.read_dir() {
                for entry in entries.filter_map(|e| e.ok()) {
                    let child_name = entry.file_name().to_string_lossy().to_string();
                    if !src_names.contains(&child_name) {
                        let child_rel = format!("{}/{}", full_rel, child_name);
                        plan.push(PlanItem { action: DELETED, rel: child_rel, is_dir: entry.path().is_dir() });
                    }
                }
            }

            for entry in WalkDir::new(src_path).min_depth(1).into_iter().filter_map(|e| e.ok()) {
                let child_name = entry.file_name().to_string_lossy().to_string();
                plan.extend(plan_sync_item(entry.path(), &dst_path.join(&child_name), skip_unchanged, &full_rel));
            }
        } else {
            plan.push(PlanItem { action: DELETED, rel: full_rel.clone(), is_dir: false });
            plan.push(PlanItem { action: NEW, rel: full_rel.clone(), is_dir: true });
            for entry in WalkDir::new(src_path).min_depth(1).into_iter().filter_map(|e| e.ok()) {
                let child_name = entry.file_name().to_string_lossy().to_string();
                plan.extend(plan_sync_item(entry.path(), &dst_path.join(&child_name), skip_unchanged, &full_rel));
            }
        }
    } else {
        if !dst_path.exists() {
            plan.push(PlanItem { action: NEW, rel: full_rel.clone(), is_dir: false });
        } else if dst_path.is_dir() {
            plan.push(PlanItem { action: DELETED, rel: full_rel.clone(), is_dir: true });
            plan.push(PlanItem { action: NEW, rel: full_rel.clone(), is_dir: false });
        } else {
            let same = files_equal(src_path, dst_path).unwrap_or(false);
            if same {
                if !skip_unchanged {
                    plan.push(PlanItem { action: UNCHANGED, rel: full_rel.clone(), is_dir: false });
                }
            } else {
                plan.push(PlanItem { action: UPDATED, rel: full_rel.clone(), is_dir: false });
            }
        }
    }

    plan
}

fn add_deleted_rec(dir: &Path, prefix: &str, plan: &mut Vec<PlanItem>) {
    use Action::DELETED;
    if let Ok(entries) = dir.read_dir() {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) };
            let rel_clone = rel.clone();
            let is_dir = entry.path().is_dir();
            plan.push(PlanItem { action: DELETED, rel, is_dir });
            if is_dir {
                add_deleted_rec(&entry.path(), &rel_clone, plan);
            }
        }
    }
}

/// Analyze a plan into counts, action_map, and deleted_map.
pub fn analyze_plan(plan: &[PlanItem]) -> (
    std::collections::HashMap<String, i32>,
    std::collections::HashMap<String, PlanItem>,
    std::collections::HashMap<String, Vec<String>>,
) {
    use Action::DELETED;

    let mut counts: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
    let mut action_map: std::collections::HashMap<String, PlanItem> = std::collections::HashMap::new();
    let mut deleted_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for item in plan {
        *counts.entry(action_label(item.action).to_string()).or_insert(0) += 1;
        action_map.insert(item.rel.clone(), item.clone());

        if item.action == DELETED {
            let rel = &item.rel;
            let parts: Vec<&str> = rel.rsplitn(2, '/').collect();
            let parent = if parts.len() > 1 { parts[1] } else { "" };
            deleted_map.entry(parent.to_string()).or_default().push(
                if parts.len() > 1 { parts[0].to_string() } else { rel.clone() }
            );
        }
    }

    (counts, action_map, deleted_map)
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::NEW => "new",
        Action::UPDATED => "updated",
        Action::DELETED => "deleted",
        Action::UNCHANGED => "unchanged",
    }
}

pub fn action_label_public(action: Action) -> &'static str {
    action_label(action)
}