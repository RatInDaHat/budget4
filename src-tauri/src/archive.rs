//! Single-file archives of a whole budget folder: a zip holding the folder (its .ynab4 name at the
//! top), so a budget can be moved or kept as one file and opened again from it.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Zips `folder` into `dest`. `dest` must not be inside `folder`.
pub fn write(folder: &Path, dest: &Path) -> Result<(), String> {
    folder.file_name().and_then(|n| n.to_str()).ok_or("The budget folder has no name")?;
    if dest.starts_with(folder) {
        return Err("Choose a place outside the budget folder for the archive".into());
    }
    let tmp = dest.with_extension("zip.partial");
    let result = (|| -> Result<(), String> {
        let file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut stack = vec![folder.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path.strip_prefix(folder.parent().unwrap_or(folder)).map_err(|e| e.to_string())?;
                let name = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                zip.start_file(name, options).map_err(|e| e.to_string())?;
                let mut src = fs::File::open(&path).map_err(|e| e.to_string())?;
                io::copy(&mut src, &mut zip).map_err(|e| e.to_string())?;
            }
        }
        zip.finish().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())?;

        fs::rename(&tmp, dest).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Unzips an archive made by `write` into `parent` and returns the restored budget folder.
/// Refuses if the archive isn't a budget or the folder already exists; never overwrites.
pub fn restore(archive: &Path, parent: &Path) -> Result<PathBuf, String> {
    let mut zip = ZipArchive::new(fs::File::open(archive).map_err(|e| e.to_string())?).map_err(|_| "That file isn't a Budget4 archive")?;
    let top = {
        let first = zip.by_index(0).map_err(|_| "The archive is empty")?;
        let name = first.enclosed_name().ok_or("The archive has an unsafe file name")?;
        name.components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .ok_or("The archive has an unsafe file name")?
    };
    if !top.to_lowercase().ends_with(".ynab4") {
        return Err("That archive doesn't hold a YNAB 4 budget folder".into());
    }
    let target = parent.join(&top);
    if target.exists() {
        return Err(format!("A folder named {top} already exists there. Choose another folder, or open that budget."));
    }
    if !(0..zip.len()).any(|i| zip.by_index(i).map(|f| f.name().eq_ignore_ascii_case(&format!("{top}/Budget.ymeta"))).unwrap_or(false)) {
        return Err("That archive doesn't hold a YNAB 4 budget folder".into());
    }
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let rel = f.enclosed_name().ok_or("The archive has an unsafe file name")?;
        if !rel.starts_with(&top) {
            return Err("The archive has files outside its budget folder".into());
        }
        let out = parent.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(dir) = out.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut dst = fs::File::create(&out).map_err(|e| e.to_string())?;
        io::copy(&mut f, &mut dst).map_err(|e| e.to_string())?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("budget4-archive-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn archive_round_trips_a_budget_folder() {
        let root = temp_dir("roundtrip");
        let budget = root.join("My Budget~1A2B.ynab4");
        fs::create_dir_all(budget.join("sub")).unwrap();
        fs::write(budget.join("Budget.ymeta"), "meta").unwrap();
        fs::write(budget.join("sub").join("data.txt"), "data").unwrap();

        let zip_path = root.join("out.zip");
        write(&budget, &zip_path).unwrap();
        assert!(!root.join("out.zip.partial").exists());

        let restore_into = temp_dir("restore");
        let restored = restore(&zip_path, &restore_into).unwrap();
        assert_eq!(restored, restore_into.join("My Budget~1A2B.ynab4"));
        assert_eq!(fs::read_to_string(restored.join("Budget.ymeta")).unwrap(), "meta");
        assert_eq!(fs::read_to_string(restored.join("sub").join("data.txt")).unwrap(), "data");

        // Restoring over an existing folder is refused, and nothing is overwritten.
        assert!(restore(&zip_path, &restore_into).is_err());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&restore_into);
    }

    #[test]
    fn archive_refuses_a_destination_inside_the_folder_and_non_budgets() {
        let root = temp_dir("refuse");
        let budget = root.join("B~1.ynab4");
        fs::create_dir_all(&budget).unwrap();
        fs::write(budget.join("Budget.ymeta"), "meta").unwrap();
        assert!(write(&budget, &budget.join("inside.zip")).is_err());

        let plain = root.join("plain.zip");
        {
            let mut z = ZipWriter::new(fs::File::create(&plain).unwrap());
            z.start_file("notes/readme.txt", SimpleFileOptions::default()).unwrap();
            z.write_all(b"hi").unwrap();
            z.finish().unwrap();
        }
        assert!(restore(&plain, &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
