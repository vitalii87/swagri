//! Immutable, content-addressed artifact storage used by Swagri nodes.

use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Stable block size used by the first Swagri artifact protocol.
pub const BLOCK_BYTES: usize = 256 * 1024;
const MANIFEST_VERSION: u16 = 1;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ContentId([u8; 32]);

impl ContentId {
    pub fn hex(self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sha256:{}", hex::encode(self.0))
    }
}

impl FromStr for ContentId {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.strip_prefix("sha256:").unwrap_or(value);
        let bytes = hex::decode(value).map_err(|_| StorageError::InvalidContentId)?;
        let digest: [u8; 32] = bytes
            .try_into()
            .map_err(|_| StorageError::InvalidContentId)?;
        Ok(Self(digest))
    }
}

impl Serialize for ContentId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ContentId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlockRef {
    pub id: ContentId,
    pub size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactManifest {
    pub version: u16,
    pub id: ContentId,
    pub size: u64,
    pub block_size: u32,
    pub blocks: Vec<BlockRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreStats {
    pub artifacts: u64,
    pub blocks: u64,
    pub used_bytes: u64,
    pub quota_bytes: u64,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("content ID must be a 32-byte SHA-256 digest")]
    InvalidContentId,
    #[error("artifact {0} was not found")]
    ArtifactNotFound(ContentId),
    #[error("artifact {0} failed integrity verification: {1}")]
    Integrity(ContentId, String),
    #[error("artifact needs {needed} bytes but only {available} bytes remain in the Swagri quota")]
    QuotaExceeded { needed: u64, available: u64 },
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid artifact manifest at {path}: {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub struct ArtifactStore {
    root: PathBuf,
    quota_bytes: u64,
}

impl ArtifactStore {
    pub fn open(root: impl Into<PathBuf>, quota_bytes: u64) -> Result<Self, StorageError> {
        let root = root.into();
        create_dir(&root)?;
        create_dir(&root.join("blocks").join("sha256"))?;
        create_dir(&root.join("manifests").join("sha256"))?;
        Ok(Self { root, quota_bytes })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn import(&self, source: &Path) -> Result<ArtifactManifest, StorageError> {
        let manifest = describe(source)?;
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).expect("manifest is serializable");
        let mut unique_new = BTreeSet::new();
        let mut required = if self.manifest_path(manifest.id).is_file() {
            0
        } else {
            manifest_bytes.len() as u64
        };
        for block in &manifest.blocks {
            if !self.block_path(block.id).is_file() && unique_new.insert(block.id) {
                required = required.saturating_add(u64::from(block.size));
            }
        }
        let stats = self.stats()?;
        let available = self.quota_bytes.saturating_sub(stats.used_bytes);
        if required > available {
            return Err(StorageError::QuotaExceeded {
                needed: required,
                available,
            });
        }

        let mut file = open_file(source)?;
        let mut buffer = vec![0_u8; BLOCK_BYTES];
        for expected in &manifest.blocks {
            let read = read_block(&mut file, &mut buffer, source)?;
            let actual = digest(&buffer[..read]);
            if read != expected.size as usize || actual != expected.id {
                return Err(StorageError::Integrity(
                    manifest.id,
                    "source changed while it was being imported".into(),
                ));
            }
            let target = self.block_path(expected.id);
            if target.is_file() {
                let existing = fs::read(&target).map_err(|source| StorageError::Io {
                    path: target.clone(),
                    source,
                })?;
                if existing.len() != expected.size as usize || digest(&existing) != expected.id {
                    return Err(StorageError::Integrity(
                        manifest.id,
                        format!("existing block {} is damaged", expected.id),
                    ));
                }
            } else {
                write_atomic(&target, &buffer[..read])?;
            }
        }
        if read_block(&mut file, &mut buffer, source)? != 0 {
            return Err(StorageError::Integrity(
                manifest.id,
                "source grew while it was being imported".into(),
            ));
        }
        write_atomic(&self.manifest_path(manifest.id), &manifest_bytes)?;
        Ok(manifest)
    }

    pub fn manifest(&self, id: ContentId) -> Result<ArtifactManifest, StorageError> {
        let path = self.manifest_path(id);
        if !path.is_file() {
            return Err(StorageError::ArtifactNotFound(id));
        }
        let bytes = fs::read(&path).map_err(|source| StorageError::Io {
            path: path.clone(),
            source,
        })?;
        let manifest = serde_json::from_slice::<ArtifactManifest>(&bytes).map_err(|source| {
            StorageError::Manifest {
                path: path.clone(),
                source,
            }
        })?;
        if manifest.version != MANIFEST_VERSION
            || manifest.id != id
            || manifest.block_size != BLOCK_BYTES as u32
            || manifest
                .blocks
                .iter()
                .any(|block| block.size == 0 || block.size as usize > BLOCK_BYTES)
        {
            return Err(StorageError::Integrity(
                id,
                "invalid manifest metadata".into(),
            ));
        }
        Ok(manifest)
    }

    pub fn verify(&self, id: ContentId) -> Result<ArtifactManifest, StorageError> {
        let manifest = self.manifest(id)?;
        let mut artifact_hasher = Sha256::new();
        let mut total = 0_u64;
        for block in &manifest.blocks {
            let path = self.block_path(block.id);
            let bytes = fs::read(&path).map_err(|source| StorageError::Io {
                path: path.clone(),
                source,
            })?;
            if bytes.len() != block.size as usize || digest(&bytes) != block.id {
                return Err(StorageError::Integrity(
                    id,
                    format!("damaged block {}", block.id),
                ));
            }
            total += bytes.len() as u64;
            artifact_hasher.update(&bytes);
        }
        if total != manifest.size || finalize(artifact_hasher) != id {
            return Err(StorageError::Integrity(
                id,
                "artifact digest mismatch".into(),
            ));
        }
        Ok(manifest)
    }

    pub fn export(&self, id: ContentId, destination: &Path) -> Result<(), StorageError> {
        if destination.exists() {
            return Err(StorageError::DestinationExists(destination.to_owned()));
        }
        let manifest = self.verify(id)?;
        if let Some(parent) = destination.parent() {
            create_dir(parent)?;
        }
        let temporary = temporary_path(destination);
        let result = (|| {
            let mut output = create_new_file(&temporary)?;
            for block in &manifest.blocks {
                let path = self.block_path(block.id);
                let mut input = open_file(&path)?;
                std::io::copy(&mut input, &mut output).map_err(|source| StorageError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            }
            output.sync_all().map_err(|source| StorageError::Io {
                path: temporary.clone(),
                source,
            })?;
            fs::rename(&temporary, destination).map_err(|source| StorageError::Io {
                path: destination.to_owned(),
                source,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn list(&self) -> Result<Vec<ArtifactManifest>, StorageError> {
        let directory = self.root.join("manifests").join("sha256");
        let mut manifests = Vec::new();
        for entry in read_dir(&directory)? {
            let entry = entry.map_err(|source| StorageError::Io {
                path: directory.clone(),
                source,
            })?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|source| StorageError::Io {
                path: entry.path(),
                source,
            })?;
            manifests.push(serde_json::from_slice(&bytes).map_err(|source| {
                StorageError::Manifest {
                    path: entry.path(),
                    source,
                }
            })?);
        }
        manifests.sort_by_key(|manifest: &ArtifactManifest| manifest.id);
        Ok(manifests)
    }

    pub fn stats(&self) -> Result<StoreStats, StorageError> {
        let block_directory = self.root.join("blocks").join("sha256");
        let manifest_directory = self.root.join("manifests").join("sha256");
        let (blocks, block_bytes) = directory_usage(&block_directory)?;
        let (artifacts, manifest_bytes) = directory_usage(&manifest_directory)?;
        Ok(StoreStats {
            artifacts,
            blocks,
            used_bytes: block_bytes.saturating_add(manifest_bytes),
            quota_bytes: self.quota_bytes,
        })
    }

    fn block_path(&self, id: ContentId) -> PathBuf {
        self.root.join("blocks").join("sha256").join(id.hex())
    }

    fn manifest_path(&self, id: ContentId) -> PathBuf {
        self.root
            .join("manifests")
            .join("sha256")
            .join(format!("{}.json", id.hex()))
    }
}

fn describe(path: &Path) -> Result<ArtifactManifest, StorageError> {
    let mut file = open_file(path)?;
    let mut artifact_hasher = Sha256::new();
    let mut blocks = Vec::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; BLOCK_BYTES];
    loop {
        let read = read_block(&mut file, &mut buffer, path)?;
        if read == 0 {
            break;
        }
        artifact_hasher.update(&buffer[..read]);
        size += read as u64;
        blocks.push(BlockRef {
            id: digest(&buffer[..read]),
            size: read as u32,
        });
    }
    Ok(ArtifactManifest {
        version: MANIFEST_VERSION,
        id: finalize(artifact_hasher),
        size,
        block_size: BLOCK_BYTES as u32,
        blocks,
    })
}

fn digest(bytes: &[u8]) -> ContentId {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    finalize(hasher)
}

fn finalize(hasher: Sha256) -> ContentId {
    ContentId(hasher.finalize().into())
}

fn read_block(file: &mut File, buffer: &mut [u8], path: &Path) -> Result<usize, StorageError> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = file
            .read(&mut buffer[filled..])
            .map_err(|source| StorageError::Io {
                path: path.to_owned(),
                source,
            })?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = create_new_file(&temporary)?;
        file.write_all(bytes).map_err(|source| StorageError::Io {
            path: temporary.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| StorageError::Io {
            path: temporary.clone(),
            source,
        })?;
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(_) if path.is_file() => Ok(()),
            Err(source) => Err(StorageError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    })();
    if result.is_err() || path.is_file() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("part-{}-{nonce}", std::process::id()))
}

fn create_dir(path: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(path).map_err(|source| StorageError::Io {
        path: path.to_owned(),
        source,
    })
}

fn open_file(path: &Path) -> Result<File, StorageError> {
    File::open(path).map_err(|source| StorageError::Io {
        path: path.to_owned(),
        source,
    })
}

fn create_new_file(path: &Path) -> Result<File, StorageError> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| StorageError::Io {
            path: path.to_owned(),
            source,
        })
}

fn read_dir(path: &Path) -> Result<fs::ReadDir, StorageError> {
    fs::read_dir(path).map_err(|source| StorageError::Io {
        path: path.to_owned(),
        source,
    })
}

fn directory_usage(path: &Path) -> Result<(u64, u64), StorageError> {
    let mut count = 0;
    let mut bytes = 0_u64;
    for entry in read_dir(path)? {
        let entry = entry.map_err(|source| StorageError::Io {
            path: path.to_owned(),
            source,
        })?;
        let metadata = entry.metadata().map_err(|source| StorageError::Io {
            path: entry.path(),
            source,
        })?;
        if metadata.is_file() {
            count += 1;
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok((count, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_deduplicates_blocks_and_detects_damage() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        let mut bytes = vec![7_u8; BLOCK_BYTES];
        bytes.extend(vec![7_u8; BLOCK_BYTES]);
        bytes.extend(b"tail");
        fs::write(&source, &bytes).unwrap();

        let store = ArtifactStore::open(temporary.path().join("store"), 10_000_000).unwrap();
        let manifest = store.import(&source).unwrap();
        assert_eq!(manifest.blocks.len(), 3);
        assert_eq!(manifest.blocks[0].id, manifest.blocks[1].id);
        assert!(
            fs::read_to_string(store.manifest_path(manifest.id))
                .unwrap()
                .contains("sha256:")
        );
        assert_eq!(store.stats().unwrap().blocks, 2);
        assert_eq!(store.verify(manifest.id).unwrap(), manifest);

        let output = temporary.path().join("restored.bin");
        store.export(manifest.id, &output).unwrap();
        assert_eq!(fs::read(output).unwrap(), bytes);

        fs::write(store.block_path(manifest.blocks[0].id), b"damaged").unwrap();
        assert!(matches!(
            store.verify(manifest.id),
            Err(StorageError::Integrity(_, _))
        ));
    }

    #[test]
    fn refuses_an_import_that_exceeds_quota_without_writing_blocks() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("large.bin");
        fs::write(&source, vec![1_u8; 2048]).unwrap();
        let store = ArtifactStore::open(temporary.path().join("store"), 1024).unwrap();

        assert!(matches!(
            store.import(&source),
            Err(StorageError::QuotaExceeded { .. })
        ));
        assert_eq!(store.stats().unwrap().blocks, 0);
    }

    #[test]
    fn content_id_accepts_prefixed_and_plain_hex() {
        let id = digest(b"swagri");
        assert_eq!(ContentId::from_str(&id.hex()).unwrap(), id);
        assert_eq!(ContentId::from_str(&id.to_string()).unwrap(), id);
    }
}
