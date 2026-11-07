use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// A file inside a model repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub name: &'static str,
    /// Lowercase hex SHA-256 of the file, when known. Verified after download.
    pub sha256: Option<&'static str>,
    /// Exact size in bytes, when known. Used to skip work and validate resumes.
    pub size: Option<u64>,
}

/// A model pinned to an immutable repository revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// Hugging Face repository, e.g. `minishlab/potion-multilingual-128M`.
    pub repo: &'static str,
    /// Full git commit hash of the repository revision.
    pub revision: &'static str,
    pub files: &'static [ModelFile],
}

impl ModelSpec {
    pub fn url(&self, file: &str) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{file}",
            self.repo, self.revision
        )
    }

    /// Directory the files of this spec are stored in, below the cache root.
    pub fn dir_in(&self, cache_dir: &Path) -> PathBuf {
        cache_dir
            .join(self.repo.replace('/', "--"))
            .join(self.revision)
    }
}

/// Byte progress for one file being downloaded.
#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress<'a> {
    pub file: &'a str,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Local paths of a downloaded, verified model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    pub dir: PathBuf,
}

impl ModelPaths {
    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// Downloads model files from Hugging Face into a cache directory.
///
/// A file is only ever visible under its final name after it was fully received
/// and its checksum verified (written to `<name>.part`, then renamed). An
/// interrupted download resumes from the partial file with an HTTP range request.
pub struct ModelStore;

impl ModelStore {
    /// Return the model's local paths, downloading missing files first.
    /// Blocking; call it from a background thread.
    pub fn ensure(
        cache_dir: &Path,
        spec: &ModelSpec,
        mut progress: impl FnMut(DownloadProgress<'_>),
    ) -> Result<ModelPaths> {
        let dir = spec.dir_in(cache_dir);
        fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
        let agent = agent();
        spec.files
            .iter()
            .filter(|file| !is_present(&dir, file))
            .try_for_each(|file| fetch(&agent, spec, file, &dir, &mut progress))?;
        Ok(ModelPaths { dir })
    }

    /// Paths of the model if every file is already present; never touches the network.
    pub fn local(cache_dir: &Path, spec: &ModelSpec) -> Option<ModelPaths> {
        let dir = spec.dir_in(cache_dir);
        spec.files
            .iter()
            .all(|file| is_present(&dir, file))
            .then_some(ModelPaths { dir })
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .user_agent(concat!("schlau-embed/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn is_present(dir: &Path, file: &ModelFile) -> bool {
    fs::metadata(dir.join(file.name))
        .map(|meta| meta.is_file() && file.size.is_none_or(|size| size == meta.len()))
        .unwrap_or(false)
}

fn part_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.part"))
}

fn fetch(
    agent: &ureq::Agent,
    spec: &ModelSpec,
    file: &ModelFile,
    dir: &Path,
    progress: &mut impl FnMut(DownloadProgress<'_>),
) -> Result<()> {
    let part = part_path(dir, file.name);
    let url = spec.url(file.name);
    let resume_from = fs::metadata(&part)
        .map(|meta| meta.len())
        .ok()
        .filter(|&len| len > 0 && file.size.is_none_or(|size| len < size));
    match download(agent, &url, &part, file, resume_from, progress) {
        Err(Error::Checksum { .. }) if resume_from.is_some() => {
            restart(agent, &url, &part, file, progress)?
        }
        Err(Error::Download { .. }) if resume_from.is_some() && file.size.is_none() => {
            restart(agent, &url, &part, file, progress)?
        }
        other => other?,
    }
    let target = dir.join(file.name);
    fs::rename(&part, &target).map_err(Error::io(&target))
}

fn restart(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    file: &ModelFile,
    progress: &mut impl FnMut(DownloadProgress<'_>),
) -> Result<()> {
    fs::remove_file(part).map_err(Error::io(part))?;
    download(agent, url, part, file, None, progress)
}

fn download(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    file: &ModelFile,
    resume_from: Option<u64>,
    progress: &mut impl FnMut(DownloadProgress<'_>),
) -> Result<()> {
    let fail = |message: String| Error::Download {
        url: url.to_owned(),
        message,
    };
    let request = agent.get(url);
    let request = match resume_from {
        Some(offset) => request.header("Range", format!("bytes={offset}-")),
        None => request,
    };
    let response = request.call().map_err(|e| fail(e.to_string()))?;
    let resumed = resume_from.is_some() && response.status().as_u16() == 206;
    let start = if resumed { resume_from.unwrap_or(0) } else { 0 };
    let total = file.size.or_else(|| {
        response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|len| len + start)
    });
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)
        .map_err(Error::io(part))?;
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut downloaded = start;
    progress(DownloadProgress {
        file: file.name,
        downloaded,
        total,
    });
    loop {
        let n = reader.read(&mut buf).map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(Error::io(part))?;
        downloaded += n as u64;
        progress(DownloadProgress {
            file: file.name,
            downloaded,
            total,
        });
    }
    out.sync_all().map_err(Error::io(part))?;
    drop(out);
    if let Some(size) = file.size.filter(|&size| size != downloaded) {
        return Err(fail(format!(
            "expected {size} bytes, received {downloaded}"
        )));
    }
    file.sha256
        .map_or(Ok(()), |expected| verify(part, file.name, expected))
}

fn verify(path: &Path, name: &str, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Checksum {
            file: name.to_owned(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path).map_err(Error::io(path))?);
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher).map_err(Error::io(path))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: ModelSpec = ModelSpec {
        repo: "org/model",
        revision: "abc123",
        files: &[ModelFile {
            name: "config.json",
            sha256: None,
            size: Some(2),
        }],
    };

    #[test]
    fn url_and_dir_layout() {
        assert_eq!(
            SPEC.url("config.json"),
            "https://huggingface.co/org/model/resolve/abc123/config.json"
        );
        assert_eq!(
            SPEC.dir_in(Path::new("/cache")),
            PathBuf::from("/cache/org--model/abc123")
        );
    }

    #[test]
    fn local_requires_matching_size() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ModelStore::local(tmp.path(), &SPEC).is_none());
        let dir = SPEC.dir_in(tmp.path());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), b"{}{}").unwrap();
        assert!(ModelStore::local(tmp.path(), &SPEC).is_none());
        fs::write(dir.join("config.json"), b"{}").unwrap();
        assert_eq!(
            ModelStore::local(tmp.path(), &SPEC),
            Some(ModelPaths { dir })
        );
    }

    #[test]
    fn sha256_of_known_input() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(verify(&path, "f", "00").is_err());
    }
}
