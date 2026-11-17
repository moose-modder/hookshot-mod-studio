//! Small file helpers for builds and installs.

use std::path::{Path, PathBuf};

use crate::{Context, Result};

/// Every file under `dir`, as paths relative to it, sorted. A missing folder has none.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Copies every file under `src` that `keep` accepts (by relative path) to the same place under `dst`.
pub fn copy_tree(src: &Path, dst: &Path, keep: impl Fn(&Path) -> bool) -> Result<usize> {
    let mut n = 0;
    for rel in files_under(src) {
        if !keep(&rel) {
            continue;
        }
        let to = dst.join(&rel);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).context(dir.display())?;
        }
        std::fs::copy(src.join(&rel), &to).context(to.display())?;
        n += 1;
    }
    Ok(n)
}

/// Removes a folder and everything in it, if it's there.
pub fn remove_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        std::fs::remove_dir_all(dir).context(format!("removing {}", dir.display()))?;
    }
    Ok(())
}

/// Removes a file if it's there.
pub fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(crate::Error(format!("removing {}: {e}", path.display())))
        }
        _ => Ok(()),
    }
}

/// A resource size table file (`System/Resource/*.rsizetable*`), by its path relative to a romfs.
pub fn is_size_table(rel: &Path) -> bool {
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dir: Vec<String> = rel
        .parent()
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    dir == ["system", "resource"] && name.contains(".rsizetable")
}

/// Seconds since 1970 of a file's last change.
pub fn modified(path: &Path) -> Option<u64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_tables_by_path() {
        assert!(is_size_table(Path::new(
            "System/Resource/ResourceSizeTable.Product.131.rsizetable.zs"
        )));
        assert!(is_size_table(Path::new("system\\resource\\x.rsizetable.zs")) == cfg!(windows));
        assert!(!is_size_table(Path::new(
            "Tex/ResourceSizeTable.rsizetable.zs"
        )));
        assert!(!is_size_table(Path::new("Model/x.bfres.zs")));
    }

    #[test]
    fn copies_what_it_keeps() {
        let dir = std::env::temp_dir().join(format!("modstudio-fsutil-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("a/b")).unwrap();
        std::fs::write(src.join("a/b/keep.txt"), b"1").unwrap();
        std::fs::write(src.join("a/skip.txt"), b"2").unwrap();
        let n = copy_tree(&src, &dir.join("dst"), |p| !p.ends_with("skip.txt")).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            files_under(&dir.join("dst")),
            [Path::new("a").join("b").join("keep.txt")]
        );
        remove_dir(&dir).unwrap();
        assert!(!dir.exists());
    }
}
