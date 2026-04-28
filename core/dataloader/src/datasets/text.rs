// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Text dataset for NLP tasks (classification, embedding, similarity).
//!
//! **Expected formats:**
//! - CSV: `text,label` (e.g., IMDB reviews, sentiment analysis)
//! - JSON Lines: `{"text": "...", "label": 0}`
//! - Plain text directory: `class_name/document.txt`
//!
//! **Tokenization:**
//! Uses a simple whitespace + punctuation tokenizer by default.
//! For production, integrate HuggingFace tokenizers via the `tokenizers` crate.
//!
//! **Output:**
//! - `Sample.data`: Token IDs as bytes (Vec<u32> → Vec<u8>)
//! - `Sample.label`: Class label (for classification) or 0 (for embedding tasks)

use crate::{
    readers::{FileReader, MmapReader},
    Dataset, DatasetConfig, Error, Result, Sample,
};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::{collections::HashMap, fs, path::Path, sync::Arc};

#[cfg(target_os = "linux")]
use crate::readers::IoUringReader;

/// Maximum text length in tokens (for truncation/padding).
const MAX_SEQ_LEN: usize = 512;

/// Special tokens.
const PAD_TOKEN_ID: u32 = 0;
const UNK_TOKEN_ID: u32 = 1;
const CLS_TOKEN_ID: u32 = 2; // [CLS] for BERT-style models
const SEP_TOKEN_ID: u32 = 3; // [SEP] for BERT-style models

/// Simple vocabulary for tokenization.
///
/// Maps words → token IDs. In production, use HuggingFace tokenizers
/// or load a pretrained vocabulary from the model.
#[derive(Debug, Clone)]
pub struct Vocabulary {
    word_to_id: HashMap<String, u32>,
    id_to_word: HashMap<u32, String>,
    next_id: u32,
}

impl Vocabulary {
    /// Create an empty vocabulary with special tokens.
    pub fn new() -> Self {
        let mut vocab = Self {
            word_to_id: HashMap::new(),
            id_to_word: HashMap::new(),
            next_id: 4, // Reserve 0-3 for special tokens
        };
        vocab.add_special_tokens();
        vocab
    }

    fn add_special_tokens(&mut self) {
        self.word_to_id.insert("[PAD]".to_string(), PAD_TOKEN_ID);
        self.word_to_id.insert("[UNK]".to_string(), UNK_TOKEN_ID);
        self.word_to_id.insert("[CLS]".to_string(), CLS_TOKEN_ID);
        self.word_to_id.insert("[SEP]".to_string(), SEP_TOKEN_ID);

        self.id_to_word.insert(PAD_TOKEN_ID, "[PAD]".to_string());
        self.id_to_word.insert(UNK_TOKEN_ID, "[UNK]".to_string());
        self.id_to_word.insert(CLS_TOKEN_ID, "[CLS]".to_string());
        self.id_to_word.insert(SEP_TOKEN_ID, "[SEP]".to_string());
    }

    /// Add a word to the vocabulary if not present.
    pub fn add_word(&mut self, word: &str) -> u32 {
        if let Some(&id) = self.word_to_id.get(word) {
            return id;
        }
        let id = self.next_id;
        self.word_to_id.insert(word.to_string(), id);
        self.id_to_word.insert(id, word.to_string());
        self.next_id += 1;
        id
    }

    /// Get token ID for a word (returns UNK_TOKEN_ID if not found).
    pub fn get_id(&self, word: &str) -> u32 {
        self.word_to_id.get(word).copied().unwrap_or(UNK_TOKEN_ID)
    }

    /// Vocabulary size.
    pub fn len(&self) -> usize {
        self.word_to_id.len()
    }

    /// Check if vocabulary is empty.
    pub fn is_empty(&self) -> bool {
        self.word_to_id.is_empty()
    }
}

impl Default for Vocabulary {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple whitespace + punctuation tokenizer.
///
/// In production, replace with HuggingFace tokenizers:
/// ```ignore
/// use tokenizers::Tokenizer;
/// let tokenizer = Tokenizer::from_pretrained("bert-base-uncased", None)?;
/// let encoding = tokenizer.encode(text, false)?;
/// let token_ids = encoding.get_ids();
/// ```
fn simple_tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Text dataset for NLP tasks.
///
/// **Directory structure (for directory mode):**
/// ```text
/// <root>/
///   class_0/
///     doc1.txt
///     doc2.txt
///   class_1/
///     doc3.txt
/// ```
///
/// **CSV format:**
/// ```csv
/// text,label
/// "This is a review",1
/// "Another document",0
/// ```
///
/// **JSON Lines format:**
/// ```json
/// {"text": "This is a review", "label": 1}
/// {"text": "Another document", "label": 0}
/// ```
pub struct TextDataset {
    config: DatasetConfig,
    /// Flat list of (text, label) for *this shard*.
    entries: Vec<(String, u32)>,
    /// Vocabulary for tokenization.
    vocab: Arc<Vocabulary>,
    /// Maximum sequence length (for truncation/padding).
    max_seq_len: usize,
    /// Whether to add [CLS] and [SEP] tokens (BERT-style).
    add_special_tokens: bool,
    /// File reader (io_uring on Linux, mmap elsewhere).
    #[allow(dead_code)]
    reader: Arc<dyn FileReader>,
}

impl std::fmt::Debug for TextDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextDataset")
            .field("num_entries", &self.entries.len())
            .field("vocab_size", &self.vocab.len())
            .field("max_seq_len", &self.max_seq_len)
            .field("add_special_tokens", &self.add_special_tokens)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl TextDataset {
    /// Open a text dataset from a directory (class folders).
    ///
    /// Directory structure:
    /// ```text
    /// <root>/
    ///   class_0/
    ///     doc1.txt
    ///   class_1/
    ///     doc2.txt
    /// ```
    pub fn from_directory(root: impl AsRef<Path>, config: DatasetConfig) -> Result<Self> {
        config.validate()?;
        let root = root.as_ref();

        if !root.exists() {
            return Err(Error::DatasetNotFound {
                path: root.to_path_buf(),
            });
        }

        let mut all_entries = Vec::new();
        let mut vocab = Vocabulary::new();

        // Scan class directories
        let mut class_dirs: Vec<_> = fs::read_dir(root)
            .map_err(|_e| Error::DatasetNotFound {
                path: root.to_path_buf(),
            })?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        class_dirs.sort_by_key(|e| e.file_name());

        for (class_idx, class_dir) in class_dirs.iter().enumerate() {
            let class_path = class_dir.path();

            for entry in fs::read_dir(&class_path)
                .map_err(|_| Error::InvalidFormat {
                    reason: format!("cannot read class dir: {}", class_path.display()),
                })?
                .filter_map(|e| e.ok())
            {
                let file_path = entry.path();
                if !file_path.is_file() {
                    continue;
                }

                // Read text file
                let text = fs::read_to_string(&file_path).map_err(|e| Error::CorruptedSample {
                    index: all_entries.len(),
                    reason: format!("read '{}': {e}", file_path.display()),
                })?;

                // Build vocabulary
                for word in simple_tokenize(&text) {
                    vocab.add_word(&word);
                }

                all_entries.push((text, class_idx as u32));
            }
        }

        if all_entries.is_empty() {
            return Err(Error::InvalidFormat {
                reason: format!("no text files found under {}", root.display()),
            });
        }

        Self::from_entries(all_entries, vocab, config)
    }

    /// Open a text dataset from a CSV file.
    ///
    /// Expected format: `text,label`
    pub fn from_csv(path: impl AsRef<Path>, config: DatasetConfig) -> Result<Self> {
        config.validate()?;
        let path = path.as_ref();

        if !path.exists() {
            return Err(Error::DatasetNotFound {
                path: path.to_path_buf(),
            });
        }

        let content = fs::read_to_string(path).map_err(|_e| Error::DatasetNotFound {
            path: path.to_path_buf(),
        })?;

        let mut all_entries = Vec::new();
        let mut vocab = Vocabulary::new();

        for (line_idx, line) in content.lines().skip(1).enumerate() {
            // Skip header
            let parts: Vec<&str> = line.splitn(2, ',').collect();
            if parts.len() != 2 {
                continue; // Skip malformed lines
            }

            let text = parts[0].trim_matches('"').to_string();
            let label: u32 = parts[1].trim().parse().map_err(|_| Error::InvalidFormat {
                reason: format!("invalid label on line {}: {}", line_idx + 2, parts[1]),
            })?;

            // Build vocabulary
            for word in simple_tokenize(&text) {
                vocab.add_word(&word);
            }

            all_entries.push((text, label));
        }

        if all_entries.is_empty() {
            return Err(Error::InvalidFormat {
                reason: format!("no valid entries in CSV: {}", path.display()),
            });
        }

        Self::from_entries(all_entries, vocab, config)
    }

    /// Create dataset from pre-loaded entries and vocabulary.
    fn from_entries(
        all_entries: Vec<(String, u32)>,
        vocab: Vocabulary,
        config: DatasetConfig,
    ) -> Result<Self> {
        // Shard: each GPU process keeps only its 1/N slice.
        let entries = if let Some(shard_id) = config.shard_id {
            let n = config.num_shards;
            all_entries
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % n == shard_id)
                .map(|(_, e)| e)
                .collect()
        } else {
            all_entries
        };

        // Select file reader
        #[cfg(target_os = "linux")]
        let reader: Arc<dyn FileReader> = match IoUringReader::new() {
            Ok(r) => {
                tracing::debug!("TextDataset: using IoUringReader");
                Arc::new(r)
            }
            Err(e) => {
                tracing::warn!("io_uring unavailable ({e}), falling back to MmapReader");
                Arc::new(MmapReader)
            }
        };
        #[cfg(not(target_os = "linux"))]
        let reader: Arc<dyn FileReader> = Arc::new(MmapReader);

        tracing::info!(
            total_samples = entries.len(),
            vocab_size = vocab.len(),
            shard = ?config.shard_id,
            "TextDataset loaded",
        );

        Ok(Self {
            config,
            entries,
            vocab: Arc::new(vocab),
            max_seq_len: MAX_SEQ_LEN,
            add_special_tokens: true,
            reader,
        })
    }

    /// Tokenize text and convert to token IDs.
    fn tokenize_and_encode(&self, text: &str) -> Vec<u32> {
        let words = simple_tokenize(text);
        let mut token_ids = Vec::with_capacity(self.max_seq_len);

        if self.add_special_tokens {
            token_ids.push(CLS_TOKEN_ID);
        }

        for word in words.iter().take(self.max_seq_len - 2) {
            // Reserve space for [CLS]/[SEP]
            token_ids.push(self.vocab.get_id(word));
        }

        if self.add_special_tokens {
            token_ids.push(SEP_TOKEN_ID);
        }

        // Pad to max_seq_len
        while token_ids.len() < self.max_seq_len {
            token_ids.push(PAD_TOKEN_ID);
        }

        token_ids
    }

    /// Convert token IDs (Vec<u32>) to bytes (Vec<u8>).
    fn tokens_to_bytes(tokens: Vec<u32>) -> Vec<u8> {
        tokens.into_iter().flat_map(|id| id.to_le_bytes()).collect()
    }
}

impl Dataset for TextDataset {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.entries.len() {
            return Err(Error::IndexOutOfBounds {
                index: idx,
                len: self.entries.len(),
            });
        }

        let (text, label) = &self.entries[idx];

        // Tokenize + encode
        let token_ids = self.tokenize_and_encode(text);
        let data = Self::tokens_to_bytes(token_ids);

        let metadata = Some(serde_json::json!({
            "text_preview": text.chars().take(100).collect::<String>(),
            "length": text.len(),
        }));

        Ok(Sample {
            data,
            label: *label,
            metadata,
        })
    }

    fn iter_shuffled(&self, seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        if seed == 0 || !self.config.shuffle {
            // Sequential iteration
            return Box::new((0..self.len()).map(move |idx| self.get(idx)));
        }

        // Shuffled iteration
        let mut indices: Vec<usize> = (0..self.len()).collect();
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        indices.shuffle(&mut rng);

        Box::new(indices.into_iter().map(move |idx| self.get(idx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_vocabulary() {
        let mut vocab = Vocabulary::new();
        assert_eq!(vocab.len(), 4); // Special tokens

        let id1 = vocab.add_word("hello");
        let id2 = vocab.add_word("world");
        let id3 = vocab.add_word("hello"); // Duplicate

        assert_eq!(id1, id3); // Same word → same ID
        assert_ne!(id1, id2);
        assert_eq!(vocab.len(), 6); // 4 special + 2 words
    }

    #[test]
    fn test_simple_tokenize() {
        let tokens = simple_tokenize("Hello, world! This is a test.");
        assert_eq!(tokens, vec!["hello", "world", "this", "is", "a", "test"]);
    }

    #[test]
    fn test_text_dataset_from_directory() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create class directories
        let class0 = root.join("positive");
        let class1 = root.join("negative");
        fs::create_dir_all(&class0).unwrap();
        fs::create_dir_all(&class1).unwrap();

        // Write sample files
        fs::write(class0.join("doc1.txt"), "This is great!").unwrap();
        fs::write(class0.join("doc2.txt"), "I love this product.").unwrap();
        fs::write(class1.join("doc3.txt"), "This is terrible.").unwrap();

        let config = DatasetConfig::default().with_shuffle(false);
        let dataset = TextDataset::from_directory(root, config)?;

        assert_eq!(dataset.len(), 3);
        assert!(dataset.vocab.len() > 4); // At least some words

        // Test sample retrieval (note: directories are scanned alphabetically)
        let sample = dataset.get(0)?;
        // "negative" comes before "positive" alphabetically → class 0
        // "positive" → class 1
        assert!(!sample.data.is_empty());

        Ok(())
    }

    #[test]
    fn test_text_dataset_from_csv() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("data.csv");

        let csv_content = r#"text,label
"This is great",1
"This is terrible",0
"I love it",1
"#;
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();
        let dataset = TextDataset::from_csv(&csv_path, config)?;

        assert_eq!(dataset.len(), 3);

        let sample = dataset.get(0)?;
        assert_eq!(sample.label, 1);

        Ok(())
    }

    #[test]
    fn test_tokenize_and_encode() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let class0 = root.join("class0");
        fs::create_dir_all(&class0).unwrap();
        fs::write(class0.join("doc.txt"), "hello world").unwrap();

        let config = DatasetConfig::default();
        let dataset = TextDataset::from_directory(root, config)?;

        let sample = dataset.get(0)?;

        // Token IDs: [CLS, hello_id, world_id, SEP, PAD, PAD, ...]
        // Should be 512 * 4 bytes = 2048 bytes
        assert_eq!(sample.data.len(), MAX_SEQ_LEN * 4);

        Ok(())
    }

    #[test]
    fn test_sharding() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let class0 = root.join("class0");
        fs::create_dir_all(&class0).unwrap();

        // Create 10 samples
        for i in 0..10 {
            fs::write(class0.join(format!("doc{i}.txt")), format!("text {i}")).unwrap();
        }

        // Shard 0 of 2
        let config0 = DatasetConfig::default().with_shard(0, 2);
        let dataset0 = TextDataset::from_directory(root, config0)?;

        // Shard 1 of 2
        let config1 = DatasetConfig::default().with_shard(1, 2);
        let dataset1 = TextDataset::from_directory(root, config1)?;

        assert_eq!(dataset0.len(), 5);
        assert_eq!(dataset1.len(), 5);
        assert_eq!(dataset0.len() + dataset1.len(), 10);

        Ok(())
    }
}
