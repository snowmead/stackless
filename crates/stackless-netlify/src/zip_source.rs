//! Zip a shallow checkout for Netlify's build API.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::NetlifyError;

/// Zip every file under `root` (excluding `.git`) into memory.
pub fn zip_directory(root: &Path) -> Result<Vec<u8>, NetlifyError> {
    let mut buffer = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut count = 0usize;
        add_dir(root, root, &mut zip, options, &mut count)?;
        if count == 0 {
            return Err(provision("zip archive is empty".into()));
        }
        zip.finish()
            .map_err(|err| provision(format!("zip finish: {err}")))?;
    }
    Ok(buffer)
}

fn add_dir(
    base: &Path,
    dir: &Path,
    zip: &mut ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
    options: SimpleFileOptions,
    count: &mut usize,
) -> Result<(), NetlifyError> {
    for entry in std::fs::read_dir(dir).map_err(|err| provision(format!("read_dir: {err}")))? {
        let entry = entry.map_err(|err| provision(format!("dir entry: {err}")))?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            add_dir(base, &path, zip, options, count)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            zip.start_file(rel, options)
                .map_err(|err| provision(format!("zip start: {err}")))?;
            let mut file = File::open(&path).map_err(|err| provision(format!("open: {err}")))?;
            let mut data = Vec::new();
            file.read_to_end(&mut data)
                .map_err(|err| provision(format!("read: {err}")))?;
            zip.write_all(&data)
                .map_err(|err| provision(format!("zip write: {err}")))?;
            *count += 1;
        }
    }
    Ok(())
}

fn provision(detail: String) -> NetlifyError {
    NetlifyError::ProvisionFailed {
        resource: "netlify-build".into(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zips_files_under_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/a.txt"), b"a").unwrap();
        let bytes = zip_directory(dir.path()).unwrap();
        assert!(bytes.len() > 20);
        // ZIP local file header magic
        assert_eq!(&bytes[..2], b"PK");
    }
}
