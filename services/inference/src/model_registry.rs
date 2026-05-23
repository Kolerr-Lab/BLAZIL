//! Per-tenant model registry — disk storage, versioning, and in-memory model cache.
//!
//! ## Storage layout
//!
//! ```text
//! <model_dir>/
//!   <tenant_id>/
//!     <model_id>/
//!       meta.json          — ModelMeta (all versions, active version)
//!       v1/
//!         model.onnx
//!       v2/
//!         model.onnx
//! ```
//!
//! ## Thread safety
//!
//! All public methods take `&self` and use internal `RwLock`/`Mutex` guards.
//! The registry is meant to be wrapped in `Arc<ModelRegistry>` and shared
//! across Tokio tasks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use blazil_inference::{InferenceConfig, InferenceModel, OnnxModel, OptimizationLevel};

// ── Types ─────────────────────────────────────────────────────────────────────

/// Metadata for one uploaded model version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVersion {
    /// Version tag — auto-generated: "v1", "v2", …
    pub version: String,
    /// SHA-256 hex digest of the .onnx file.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Upload timestamp (seconds since UNIX epoch).
    pub uploaded_at: u64,
}

/// Persisted metadata for a single (tenant_id, model_id) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMeta {
    pub tenant_id: String,
    pub model_id: String,
    /// Human-readable display name (defaults to model_id).
    pub display_name: String,
    /// All uploaded versions, oldest first.
    pub versions: Vec<ModelVersion>,
    /// Currently active version tag (used for inference when no version specified).
    pub active_version: Option<String>,
}

/// Cache key for a loaded OnnxModel instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey(String, String, String); // (tenant_id, model_id, version)

// ── ModelRegistry ─────────────────────────────────────────────────────────────

pub struct ModelRegistry {
    /// Root directory for all model files.
    models_dir: PathBuf,

    /// In-memory metadata index: (tenant_id, model_id) → ModelMeta.
    /// Populated at startup by scanning disk; updated on every upload/activate.
    index: RwLock<HashMap<(String, String), ModelMeta>>,

    /// Loaded model cache: only activated models are eagerly loaded.
    /// Key: (tenant_id, model_id, version).
    cache: Mutex<HashMap<CacheKey, Arc<OnnxModel>>>,

    /// Default optimization level used when loading models.
    optimization_level: OptimizationLevel,
}

impl ModelRegistry {
    /// Create a new registry backed by `models_dir`.
    ///
    /// Scans `models_dir` for existing `meta.json` files and rebuilds the
    /// in-memory index. Does NOT pre-load models into the cache (lazy loading).
    pub fn new(
        models_dir: impl Into<PathBuf>,
        optimization_level: OptimizationLevel,
    ) -> Result<Self> {
        let dir = models_dir.into();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create model_dir: {}", dir.display()))?;

        let registry = Self {
            models_dir: dir,
            index: RwLock::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
            optimization_level,
        };

        registry.scan_disk()?;
        Ok(registry)
    }

    // ── Upload ────────────────────────────────────────────────────────────────

    /// Store a new model version from raw .onnx bytes.
    ///
    /// Returns the `ModelMeta` with the new version appended.
    /// Does NOT automatically activate the new version.
    pub fn upload(
        &self,
        tenant_id: &str,
        model_id: &str,
        display_name: Option<&str>,
        onnx_bytes: &[u8],
    ) -> Result<ModelMeta> {
        validate_id(tenant_id)?;
        validate_id(model_id)?;

        let mut index = self.index.write().unwrap();
        let key = (tenant_id.to_owned(), model_id.to_owned());

        let next_version = {
            let existing = index.get(&key);
            let n = existing.map_or(0, |m| m.versions.len());
            format!("v{}", n + 1)
        };

        // Write .onnx to disk.
        let version_dir = self.version_dir(tenant_id, model_id, &next_version);
        std::fs::create_dir_all(&version_dir)
            .with_context(|| format!("create version dir: {}", version_dir.display()))?;

        let onnx_path = version_dir.join("model.onnx");
        std::fs::write(&onnx_path, onnx_bytes)
            .with_context(|| format!("write model file: {}", onnx_path.display()))?;

        // Compute SHA-256.
        let sha256 = hex::encode(Sha256::digest(onnx_bytes));

        let version = ModelVersion {
            version: next_version.clone(),
            sha256,
            size_bytes: onnx_bytes.len() as u64,
            uploaded_at: unix_now(),
        };

        let meta = index.entry(key).or_insert_with(|| ModelMeta {
            tenant_id: tenant_id.to_owned(),
            model_id: model_id.to_owned(),
            display_name: display_name.unwrap_or(model_id).to_owned(),
            versions: Vec::new(),
            active_version: None,
        });

        // Update display_name if provided.
        if let Some(name) = display_name {
            meta.display_name = name.to_owned();
        }

        meta.versions.push(version);
        let meta_snapshot = meta.clone();

        // Persist meta.json.
        drop(index);
        self.persist_meta(&meta_snapshot)?;

        info!(
            tenant = tenant_id,
            model = model_id,
            version = next_version,
            bytes = onnx_bytes.len(),
            "Model version uploaded"
        );

        Ok(meta_snapshot)
    }

    // ── Activate ──────────────────────────────────────────────────────────────

    /// Set the active version for `(tenant_id, model_id)` and load it into cache.
    ///
    /// Loading is done in-process (blocking); callers should invoke this from
    /// a `spawn_blocking` context if called from an async task.
    pub fn activate(
        &self,
        tenant_id: &str,
        model_id: &str,
        version: &str,
    ) -> Result<Arc<OnnxModel>> {
        validate_id(tenant_id)?;
        validate_id(model_id)?;

        let onnx_path = self
            .version_dir(tenant_id, model_id, version)
            .join("model.onnx");

        if !onnx_path.exists() {
            anyhow::bail!(
                "Model file not found: {} (tenant={}, model={}, version={})",
                onnx_path.display(),
                tenant_id,
                model_id,
                version
            );
        }

        // Load model (blocking ONNX parse via Tract).
        let cfg = InferenceConfig::new(&onnx_path).with_optimization(self.optimization_level);
        let model = Arc::new(
            OnnxModel::load(cfg)
                .with_context(|| format!("load ONNX model: {}", onnx_path.display()))?,
        );

        // Update index.
        {
            let mut index = self.index.write().unwrap();
            let key = (tenant_id.to_owned(), model_id.to_owned());
            let meta = index.get_mut(&key).ok_or_else(|| {
                anyhow::anyhow!("Model not found in index: {tenant_id}/{model_id}")
            })?;
            meta.active_version = Some(version.to_owned());
            let snapshot = meta.clone();
            drop(index);
            self.persist_meta(&snapshot)?;
        }

        // Insert into cache (evicts previous active version for this model).
        {
            let mut cache = self.cache.lock().unwrap();
            // Evict any previously loaded versions for this model.
            cache.retain(|k, _| !(k.0 == tenant_id && k.1 == model_id));
            cache.insert(
                CacheKey(
                    tenant_id.to_owned(),
                    model_id.to_owned(),
                    version.to_owned(),
                ),
                Arc::clone(&model),
            );
        }

        info!(
            tenant = tenant_id,
            model = model_id,
            version = version,
            "Model activated and loaded into cache"
        );

        Ok(model)
    }

    // ── Get active model ──────────────────────────────────────────────────────

    /// Return the cached `Arc<OnnxModel>` for the active version of a model.
    ///
    /// Returns `None` if the model doesn't exist, has no active version, or
    /// has not been loaded into cache yet (call `activate` first).
    pub fn get_active_model(&self, tenant_id: &str, model_id: &str) -> Option<Arc<OnnxModel>> {
        let index = self.index.read().unwrap();
        let key = (tenant_id.to_owned(), model_id.to_owned());
        let active_version = index.get(&key)?.active_version.as_ref()?.clone();
        drop(index);

        let cache = self.cache.lock().unwrap();
        cache
            .get(&CacheKey(
                tenant_id.to_owned(),
                model_id.to_owned(),
                active_version,
            ))
            .cloned()
    }

    // ── List / Get ────────────────────────────────────────────────────────────

    /// List all models for a tenant.
    pub fn list_models(&self, tenant_id: &str) -> Vec<ModelMeta> {
        let index = self.index.read().unwrap();
        index
            .iter()
            .filter(|((tid, _), _)| tid == tenant_id)
            .map(|(_, m)| m.clone())
            .collect()
    }

    /// Get metadata for a specific model.
    pub fn get_model(&self, tenant_id: &str, model_id: &str) -> Option<ModelMeta> {
        let index = self.index.read().unwrap();
        index
            .get(&(tenant_id.to_owned(), model_id.to_owned()))
            .cloned()
    }

    // ── Delete ────────────────────────────────────────────────────────────────

    /// Delete a model and all its versions from disk and index.
    ///
    /// Fails if the model is currently active (must deactivate first).
    pub fn delete_model(&self, tenant_id: &str, model_id: &str) -> Result<()> {
        validate_id(tenant_id)?;
        validate_id(model_id)?;

        {
            let index = self.index.read().unwrap();
            let key = (tenant_id.to_owned(), model_id.to_owned());
            if let Some(meta) = index.get(&key) {
                if meta.active_version.is_some() {
                    anyhow::bail!(
                        "Cannot delete active model {tenant_id}/{model_id}. Deactivate it first."
                    );
                }
            }
        }

        // Remove from disk.
        let model_dir = self.models_dir.join(tenant_id).join(model_id);
        if model_dir.exists() {
            std::fs::remove_dir_all(&model_dir)
                .with_context(|| format!("remove model dir: {}", model_dir.display()))?;
        }

        // Remove from index and cache.
        {
            let mut index = self.index.write().unwrap();
            index.remove(&(tenant_id.to_owned(), model_id.to_owned()));
        }
        {
            let mut cache = self.cache.lock().unwrap();
            cache.retain(|k, _| !(k.0 == tenant_id && k.1 == model_id));
        }

        info!(tenant = tenant_id, model = model_id, "Model deleted");
        Ok(())
    }

    // ── Disk helpers ──────────────────────────────────────────────────────────

    fn version_dir(&self, tenant_id: &str, model_id: &str, version: &str) -> PathBuf {
        self.models_dir.join(tenant_id).join(model_id).join(version)
    }

    fn meta_path(&self, tenant_id: &str, model_id: &str) -> PathBuf {
        self.models_dir
            .join(tenant_id)
            .join(model_id)
            .join("meta.json")
    }

    fn persist_meta(&self, meta: &ModelMeta) -> Result<()> {
        let path = self.meta_path(&meta.tenant_id, &meta.model_id);
        // Ensure parent dir exists (it always should at this point, but be safe).
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(meta)?;
        std::fs::write(&path, json)
            .with_context(|| format!("write meta.json: {}", path.display()))?;
        Ok(())
    }

    /// Scan `models_dir` recursively for `meta.json` files and load them into the index.
    fn scan_disk(&self) -> Result<()> {
        if !self.models_dir.exists() {
            return Ok(());
        }

        let mut count = 0usize;
        let mut index = self.index.write().unwrap();

        for tenant_entry in read_dir_names(&self.models_dir) {
            let tenant_dir = self.models_dir.join(&tenant_entry);
            for model_entry in read_dir_names(&tenant_dir) {
                let meta_path = tenant_dir.join(&model_entry).join("meta.json");
                if meta_path.exists() {
                    match load_meta(&meta_path) {
                        Ok(meta) => {
                            index.insert((meta.tenant_id.clone(), meta.model_id.clone()), meta);
                            count += 1;
                        }
                        Err(e) => {
                            warn!("Skipping corrupt meta.json at {}: {e}", meta_path.display());
                        }
                    }
                }
            }
        }

        info!(
            models_loaded = count,
            "Model registry index rebuilt from disk"
        );
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Validate a tenant_id or model_id: alphanumeric + hyphens/underscores, 1-64 chars.
/// Guards against path traversal attacks.
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        anyhow::bail!("ID must be 1-64 characters: {id:?}");
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("ID must contain only alphanumeric, '-', or '_' characters: {id:?}");
    }
    // Reject path traversal attempts.
    if id.contains("..") || id.starts_with('.') {
        anyhow::bail!("Invalid ID (path traversal attempt): {id:?}");
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn load_meta(path: &Path) -> Result<ModelMeta> {
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(Into::into)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_inference::OptimizationLevel;
    use tempfile::TempDir;

    fn registry(dir: &TempDir) -> ModelRegistry {
        ModelRegistry::new(dir.path(), OptimizationLevel::Disable).unwrap()
    }

    #[test]
    fn test_validate_id_ok() {
        assert!(validate_id("acme-corp").is_ok());
        assert!(validate_id("model_v1").is_ok());
        assert!(validate_id("abc123").is_ok());
    }

    #[test]
    fn test_validate_id_reject_traversal() {
        assert!(validate_id("../etc/passwd").is_err());
        assert!(validate_id(".hidden").is_err());
        assert!(validate_id("a/b").is_err()); // not alphanumeric
    }

    #[test]
    fn test_upload_increments_version() {
        let tmp = TempDir::new().unwrap();
        let reg = registry(&tmp);

        let meta1 = reg
            .upload("tenant1", "squeezenet", None, b"fake-onnx-bytes-1")
            .unwrap();
        assert_eq!(meta1.versions.len(), 1);
        assert_eq!(meta1.versions[0].version, "v1");

        let meta2 = reg
            .upload("tenant1", "squeezenet", None, b"fake-onnx-bytes-2")
            .unwrap();
        assert_eq!(meta2.versions.len(), 2);
        assert_eq!(meta2.versions[1].version, "v2");
    }

    #[test]
    fn test_sha256_stored() {
        let tmp = TempDir::new().unwrap();
        let reg = registry(&tmp);
        let data = b"test-model-data";
        let meta = reg.upload("t1", "m1", None, data).unwrap();
        let expected = hex::encode(Sha256::digest(data));
        assert_eq!(meta.versions[0].sha256, expected);
    }

    #[test]
    fn test_list_models_scoped_to_tenant() {
        let tmp = TempDir::new().unwrap();
        let reg = registry(&tmp);
        reg.upload("tenant-a", "model1", None, b"a1").unwrap();
        reg.upload("tenant-a", "model2", None, b"a2").unwrap();
        reg.upload("tenant-b", "model1", None, b"b1").unwrap();

        let a_models = reg.list_models("tenant-a");
        assert_eq!(a_models.len(), 2);

        let b_models = reg.list_models("tenant-b");
        assert_eq!(b_models.len(), 1);
    }

    #[test]
    fn test_delete_non_active_model() {
        let tmp = TempDir::new().unwrap();
        let reg = registry(&tmp);
        reg.upload("t1", "m1", None, b"data").unwrap();
        reg.delete_model("t1", "m1").unwrap();
        assert!(reg.get_model("t1", "m1").is_none());
    }

    #[test]
    fn test_delete_active_model_fails() {
        let tmp = TempDir::new().unwrap();
        let reg = registry(&tmp);
        // Manually insert a meta with active_version to simulate activated state
        // without actually loading ONNX (no real model file).
        {
            let mut index = reg.index.write().unwrap();
            index.insert(
                ("t1".to_string(), "m1".to_string()),
                ModelMeta {
                    tenant_id: "t1".to_string(),
                    model_id: "m1".to_string(),
                    display_name: "m1".to_string(),
                    versions: vec![],
                    active_version: Some("v1".to_string()),
                },
            );
        }
        assert!(reg.delete_model("t1", "m1").is_err());
    }

    #[test]
    fn test_scan_disk_rebuilds_index() {
        let tmp = TempDir::new().unwrap();
        {
            let reg = registry(&tmp);
            reg.upload("t1", "m1", Some("My Model"), b"fake").unwrap();
        }
        // Create a fresh registry over the same dir — index should rebuild.
        let reg2 = registry(&tmp);
        let meta = reg2.get_model("t1", "m1").unwrap();
        assert_eq!(meta.display_name, "My Model");
        assert_eq!(meta.versions.len(), 1);
    }
}
