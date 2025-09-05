use crate::encryption::EncryptedReadWrite;
use crate::progress::{ProgressReader, ProgressWriter};
use crate::share_store::update_progress;
use crate::BLE_BUFFER_SIZE;
use crate::{SendProgressDelegate, SendProgressState};
use log::info;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use tar::{Archive, Builder, EntryType};

fn normalize_path(path: &Path) -> String {
    use std::path::Component;

    // Normal case: return the file or directory name
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }

    for comp in path.components().rev() {
        if let Component::Normal(seg) = comp {
            return seg.to_string_lossy().into_owned();
        }
    }

    // Fallback
    ".".to_string()
}

fn get_unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let mut counter = 1;
    let file_stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy())
        .unwrap_or_default();

    loop {
        let new_file_name = if extension.is_empty() {
            format!("{} ({})", file_stem, counter)
        } else {
            format!("{} ({}).{}", file_stem, counter, extension)
        };

        let new_path = path.with_file_name(new_file_name);

        if !new_path.exists() {
            return new_path;
        }

        counter += 1;
    }
}

pub fn stream_tar(
    output_stream: &mut Box<dyn EncryptedReadWrite>,
    file_paths: &Vec<String>,
    total_bytes: u64,
    progress_delegate: &Option<Box<dyn SendProgressDelegate>>,
) -> std::io::Result<()> {
    let progress_writer = ProgressWriter::new(output_stream, |sent_bytes| {
        if sent_bytes > 0 {
            let mut frac = (sent_bytes as f64) / (total_bytes as f64);
            if frac > 0.999 {
                frac = 0.999;
            } // avoid hitting 1.0 early

            update_progress(
                progress_delegate,
                SendProgressState::Transferring { progress: frac },
            )
        }
    });

    let buf_out = BufWriter::with_capacity(BLE_BUFFER_SIZE, progress_writer);
    let mut tar = Builder::new(buf_out);

    for file_path in file_paths {
        let path = Path::new(file_path);
        let normalized_path = normalize_path(path);
        info!("Normalized path: {}", normalized_path);

        if path.is_dir() {
            tar.append_dir_all(&normalized_path, path)?;
        } else {
            let mut file = File::open(path)?;
            tar.append_file(&normalized_path, &mut file)?;
        }
    }

    let buf_writer = tar.into_inner()?;
    let progress_writer = buf_writer.into_inner()?;
    let stream = progress_writer.into_inner().0;
    stream.flush()?;

    update_progress(
        progress_delegate,
        SendProgressState::Transferring { progress: 1.0 },
    );

    return Ok(());
}

// Keep only safe components (drop RootDir, CurDir, ParentDir, Prefix).
fn sanitize_rel_path(p: &Path) -> PathBuf {
    use std::path::Component::*;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Normal(seg) => out.push(seg),
            _ => {} // skip RootDir, CurDir, ParentDir, Prefix
        }
    }
    out
}

pub fn untar_stream<T: FnMut(f64)>(
    stream: &mut Box<dyn EncryptedReadWrite>,
    dest_dir: &Path,
    total_bytes: u64,
    mut progress_cb: T,
    cancel_flag: &AtomicBool,
) -> std::io::Result<Vec<String>> {
    let progress_reader = ProgressReader::new(
        stream,
        move |bytes_read| {
            if total_bytes > 0 {
                let mut frac = (bytes_read as f64) / (total_bytes as f64);
                if frac > 0.999 {
                    frac = 0.999;
                }

                progress_cb(frac);
            }
        },
        || cancel_flag.load(std::sync::atomic::Ordering::Relaxed),
    );

    let mut archive = Archive::new(progress_reader);
    let mut restored_paths = Vec::new();
    let mut top_level_map: HashMap<OsString, PathBuf> = HashMap::new();

    for entry_result in archive.entries()? {
        if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        let mut entry = entry_result?;
        let raw_rel_path = entry.path().map(|p| p.into_owned()).unwrap_or_default();
        let clean_rel_path = sanitize_rel_path(&raw_rel_path);
        if clean_rel_path.as_os_str().is_empty() {
            continue;
        }

        let mut components = clean_rel_path.components();
        let root_component = match components.next() {
            Some(std::path::Component::Normal(seg)) => OsString::from(seg),
            _ => continue,
        };
        let sub_path: PathBuf = components.as_path().to_path_buf();
        let entry_type = entry.header().entry_type();

        let target_path = if sub_path.as_os_str().is_empty()
            && matches!(
                entry_type,
                EntryType::Regular | EntryType::GNUSparse | EntryType::Continuous
            ) {
            let mut file_path = dest_dir.join(&root_component);
            if file_path.exists() {
                file_path = get_unique_path(&file_path);
            }
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            file_path
        } else {
            let root_target = top_level_map
                .entry(root_component.clone())
                .or_insert_with(|| {
                    let candidate = dest_dir.join(&root_component);
                    if candidate.exists() {
                        get_unique_path(&candidate)
                    } else {
                        candidate
                    }
                });

            let full_path = if sub_path.as_os_str().is_empty() {
                root_target.clone()
            } else {
                root_target.join(&sub_path)
            };

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }

            full_path
        };

        restored_paths.push(target_path.to_string_lossy().to_string());

        match entry_type {
            EntryType::Directory => {
                fs::create_dir_all(&target_path)?;
            }
            EntryType::Regular | EntryType::GNUSparse | EntryType::Continuous => {
                entry.unpack(&target_path)?;
            }
            _ => {}
        }
    }

    Ok(restored_paths)
}
