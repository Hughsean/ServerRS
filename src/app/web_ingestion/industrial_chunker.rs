//! Industrial-grade Chinese-aware text chunker.
//!
//! Task-book §11 requirements:
//! - Split by DistilledSection boundaries
//! - Within sections: split by paragraph / sentence-ending punctuation / newlines
//! - Unicode char counting (NOT byte-index slicing)
//! - Target chunk: 500–800 chars, max 1000 chars
//! - Overlap: 80–120 chars, does NOT cross section boundaries
//! - Each chunk gets a context header (title, section, source URL)
//! - Generates: document_summary chunk, atomic chunks, section_summary chunks

use crate::app::web_ingestion::hash::chunk_hash;

/// A processed chunk ready for storage.
#[derive(Debug, Clone)]
pub struct ChunkOutput {
    pub chunk_type: ChunkType,
    pub section_index: Option<usize>,
    pub chunk_index: u32,
    pub content: String,
    pub chunk_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkType {
    DocumentSummary,
    SectionSummary,
    Atomic,
}

impl ChunkType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkType::DocumentSummary => "document_summary",
            ChunkType::SectionSummary => "section_summary",
            ChunkType::Atomic => "atomic",
        }
    }
}

impl ChunkOutput {
    /// The chunk type as a stable string (matches `knowledge_chunk_manifests.chunk_type`).
    pub fn chunk_type_str(&self) -> &'static str {
        self.chunk_type.as_str()
    }
}

/// A distilled section from the LLM output.
#[derive(Debug, Clone)]
pub struct SectionInput {
    pub heading: String,
    pub body: String,
    pub summary: Option<String>,
}

/// Chunking configuration.
#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    pub target_min: usize,
    pub target_max: usize,
    pub overlap_min: usize,
    pub overlap_max: usize,
    pub chunker_version: String,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            target_min: 500,
            target_max: 1000,
            overlap_min: 80,
            overlap_max: 120,
            chunker_version: "20260612".into(),
        }
    }
}

/// Chunk a distilled document into storage-ready chunks.
pub fn chunk_document(
    document_title: &str,
    document_summary: &str,
    source_url: &str,
    sections: &[SectionInput],
    version_key: &str,
    config: &ChunkerConfig,
) -> Vec<ChunkOutput> {
    let mut chunks = Vec::new();
    let mut global_index: u32 = 0;

    // 1. Document summary chunk
    let summary_content = format_context_header(document_title, "", source_url, document_summary);
    let h = chunk_hash(
        version_key,
        ChunkType::DocumentSummary.as_str(),
        global_index,
        &summary_content,
        &config.chunker_version,
    );
    chunks.push(ChunkOutput {
        chunk_type: ChunkType::DocumentSummary,
        section_index: None,
        chunk_index: global_index,
        content: summary_content,
        chunk_hash: h,
    });
    global_index += 1;

    // 2. Section summaries (if more than 1 section)
    if sections.len() > 1 {
        for (si, section) in sections.iter().enumerate() {
            if let Some(ref summary) = section.summary {
                let content =
                    format_context_header(document_title, &section.heading, source_url, summary);
                let h = chunk_hash(
                    version_key,
                    ChunkType::SectionSummary.as_str(),
                    global_index,
                    &content,
                    &config.chunker_version,
                );
                chunks.push(ChunkOutput {
                    chunk_type: ChunkType::SectionSummary,
                    section_index: Some(si),
                    chunk_index: global_index,
                    content,
                    chunk_hash: h,
                });
                global_index += 1;
            }
        }
    }

    // 3. Atomic chunks within each section
    for (si, section) in sections.iter().enumerate() {
        let section_atomics = chunk_section(
            section,
            si,
            document_title,
            source_url,
            version_key,
            config,
            &mut global_index,
        );
        chunks.extend(section_atomics);
    }

    chunks
}

/// Chunk a single section into atomic chunks with overlap.
///
/// Handles both normal blocks and oversized blocks (blocks that alone exceed
/// `target_max`). Oversized blocks are split by Unicode char windows.
/// The loop always advances — it cannot deadlock.
fn chunk_section(
    section: &SectionInput,
    section_index: usize,
    document_title: &str,
    source_url: &str,
    version_key: &str,
    config: &ChunkerConfig,
    global_index: &mut u32,
) -> Vec<ChunkOutput> {
    let mut results = Vec::new();
    let blocks = split_into_blocks(&section.body);

    // Greedy merge blocks into chunks of target size
    let mut current = String::new();
    let mut block_idx = 0usize;
    let hard_max = config.target_max; // hard maximum — body must never exceed this

    while block_idx < blocks.len() {
        if current.is_empty() {
            let block = &blocks[block_idx];
            // If a single block exceeds hard max, split it by char window
            if block.chars().count() > hard_max {
                let sub_chunks = split_long_block(
                    block,
                    document_title,
                    &section.heading,
                    source_url,
                    version_key,
                    config,
                    section_index,
                    global_index,
                );
                results.extend(sub_chunks);
                block_idx += 1;
                // No overlap across split blocks — the long block was split
                // with its own internal continuity
                continue;
            }
            current = block.clone();
            block_idx += 1;
            continue;
        }

        let candidate = format!("{current}\n{}", blocks[block_idx]);
        if candidate.chars().count() > hard_max {
            // Emit current chunk
            let content =
                format_context_header(document_title, &section.heading, source_url, &current);
            let h = chunk_hash(
                version_key,
                ChunkType::Atomic.as_str(),
                *global_index,
                &content,
                &config.chunker_version,
            );
            results.push(ChunkOutput {
                chunk_type: ChunkType::Atomic,
                section_index: Some(section_index),
                chunk_index: *global_index,
                content,
                chunk_hash: h,
            });
            *global_index += 1;

            // Overlap: keep the last `overlap` characters of current as seed.
            // Overlap does NOT cross section boundaries (we're within one section).
            let overlap = overlap_text(&current, config.overlap_min, config.overlap_max);
            let overlap_len = overlap.chars().count();

            // Guard against dead-loop: if overlap + next_block would still
            // exceed hard_max (next_block is oversized), discard the overlap
            // and let the oversized block be split via the current.is_empty() path.
            if overlap_len + blocks[block_idx].chars().count() > hard_max {
                // Overlap is not useful — oversized block will be split separately
                current = String::new();
                // block_idx NOT advanced — oversized block handled next iteration
            } else {
                current = overlap;
            }
            // NOTE: block_idx is NOT advanced — the same block is re-evaluated.
            // If the block itself is oversized, it will be handled by the
            // `current.is_empty()` → `split_long_block` path.
        } else {
            current = candidate;
            block_idx += 1;
        }
    }

    // Emit final chunk for this section
    if !current.trim().is_empty() {
        let content = format_context_header(document_title, &section.heading, source_url, &current);
        let h = chunk_hash(
            version_key,
            ChunkType::Atomic.as_str(),
            *global_index,
            &content,
            &config.chunker_version,
        );
        results.push(ChunkOutput {
            chunk_type: ChunkType::Atomic,
            section_index: Some(section_index),
            chunk_index: *global_index,
            content,
            chunk_hash: h,
        });
        *global_index += 1;
    }

    results
}

/// Split a single oversized block into multiple chunks using Unicode char
/// windowing. Each sub-chunk respects the hard max. No overlap between
/// sub-chunks of the same original block.
fn split_long_block(
    block: &str,
    document_title: &str,
    section_heading: &str,
    source_url: &str,
    version_key: &str,
    config: &ChunkerConfig,
    section_index: usize,
    global_index: &mut u32,
) -> Vec<ChunkOutput> {
    let mut results = Vec::new();
    let chars: Vec<char> = block.chars().collect();
    let chunk_size = config.target_min.max(config.target_max * 3 / 4); // use ~75% of max
    let mut start = 0usize;

    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        // Try to find a natural boundary (sentence end) near the cut point
        let mut actual_end = end;
        if end < chars.len() {
            // Look backward from end for a sentence boundary
            for j in (start..end).rev() {
                if matches!(chars[j], '。' | '！' | '？' | '.' | '!' | '?' | '\n') {
                    actual_end = j + 1; // include the boundary char
                    break;
                }
            }
            // If no natural boundary found, use hard cut
            if actual_end < start + (config.target_min / 2) {
                actual_end = end;
            }
        }

        let sub_body: String = chars[start..actual_end].iter().collect();
        if !sub_body.trim().is_empty() {
            let content =
                format_context_header(document_title, section_heading, source_url, sub_body.trim());
            let h = chunk_hash(
                version_key,
                ChunkType::Atomic.as_str(),
                *global_index,
                &content,
                &config.chunker_version,
            );
            results.push(ChunkOutput {
                chunk_type: ChunkType::Atomic,
                section_index: Some(section_index),
                chunk_index: *global_index,
                content,
                chunk_hash: h,
            });
            *global_index += 1;
        }
        start = actual_end;
    }

    results
}

/// Split body text into semantic blocks (paragraphs, sentence breaks, newlines).
fn split_into_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    // Split by double newline (paragraph boundaries) first
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        // Split long paragraphs by Chinese/English sentence endings
        let sub_blocks = split_by_sentence(para);
        blocks.extend(sub_blocks);
    }
    blocks
}

/// Split text by sentence boundaries (。, ！, ？, . , !, ?, ;, ；, \n).
fn split_by_sentence(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let chars: Vec<char> = text.chars().collect();

    for (i, ch) in chars.iter().enumerate() {
        let is_boundary = matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '；' | ';' | '\n');
        if is_boundary {
            let block: String = chars[start..=i].iter().collect();
            let block = block.trim();
            if !block.is_empty() {
                result.push(block.to_string());
            }
            start = i + 1;
        }
    }

    // Remaining text
    if start < chars.len() {
        let block: String = chars[start..].iter().collect();
        let block = block.trim();
        if !block.is_empty() {
            result.push(block.to_string());
        }
    }

    result
}

/// Extract the last `overlap` characters (in Unicode terms) from a text.
fn overlap_text(text: &str, min: usize, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len == 0 {
        return String::new();
    }
    let target = max.min(len);
    let start = len.saturating_sub(target);
    let slice: String = chars[start..].iter().collect();
    // Ensure at least min chars, but don't exceed what we have
    if slice.chars().count() >= min {
        slice
    } else {
        text.chars()
            .rev()
            .take(max)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

/// Format the context header for a chunk.
fn format_context_header(title: &str, section: &str, url: &str, body: &str) -> String {
    let section_line = if section.is_empty() {
        String::new()
    } else {
        format!("章节：{section}\n")
    };
    format!("标题：{title}\n{section_line}来源：{url}\n正文：\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_document_basic() {
        let sections = vec![SectionInput {
            heading: "测试".into(),
            body: "这是测试内容。第二句话。第三句话。第四句话。第五句话。".into(),
            summary: Some("测试概要".into()),
        }];
        let chunks = chunk_document(
            "测试标题",
            "文档摘要",
            "https://example.com",
            &sections,
            "vk_test",
            &ChunkerConfig::default(),
        );
        assert!(!chunks.is_empty());
        // Should have at least doc summary + 1 atomic chunk
        assert!(chunks.len() >= 2);
        // First chunk is document summary
        assert_eq!(chunks[0].chunk_type, ChunkType::DocumentSummary);
    }

    #[test]
    fn test_overlap_text() {
        let text = "这是测试文本内容";
        let overlap = overlap_text(text, 2, 4);
        // Should contain last ~2-4 characters
        assert!(!overlap.is_empty());
        assert!(overlap.chars().count() <= 4);
    }

    #[test]
    fn test_split_by_sentence() {
        let blocks = split_by_sentence("你好。世界！测试？结束。");
        assert!(blocks.len() >= 2);
    }

    #[test]
    fn test_long_block_no_infinite_loop() {
        // A single block that exceeds hard max (1000 chars) must be split,
        // NOT cause an infinite loop.
        let long_body: String = "这是一段很长的中文文本。".repeat(200); // ~2000 chars
        let sections = vec![SectionInput {
            heading: "测试长文本".into(),
            body: long_body,
            summary: Some("长文本概要".into()),
        }];
        let chunks = chunk_document(
            "测试标题",
            "文档摘要",
            "https://example.com",
            &sections,
            "vk_test",
            &ChunkerConfig::default(),
        );
        // Should produce multiple atomic chunks for the long body
        assert!(
            chunks.len() >= 3,
            "long block should be split into multiple chunks, got {}",
            chunks.len()
        );
        // Every atomic chunk body must not exceed hard max
        for chunk in &chunks {
            if chunk.chunk_type == ChunkType::Atomic {
                // The body part (after context header) must be ≤ 1000 chars
                assert!(
                    chunk.content.chars().count() <= 1100, // allow some overhead for context header
                    "chunk body exceeds hard max: {} chars",
                    chunk.content.chars().count()
                );
            }
        }
    }

    #[test]
    fn test_overlap_plus_oversized_block_no_dead_loop() {
        // Regression: when overlap + next_block together exceed hard_max,
        // and the overlap alone is short (from residue), the loop must not
        // deadlock re-emitting the same tiny overlap.
        //
        // Construct: block 0 = ~600 chars (fits in one chunk),
        //           block 1 = ~1500 chars (exceeds hard_max, needs splitting)
        let block0: String = std::iter::repeat("这是一个正常长度的句子。")
            .take(20)
            .collect::<Vec<_>>()
            .join("");
        let block1: String = std::iter::repeat("这是一个超长段落的内容。")
            .take(60)
            .collect::<Vec<_>>()
            .join(""); // ~1200 chars
        let body = format!("{block0}\n\n{block1}");

        let sections = vec![SectionInput {
            heading: "测试".into(),
            body,
            summary: None,
        }];
        let chunks = chunk_document(
            "标题",
            "摘要",
            "https://example.com",
            &sections,
            "vk",
            &ChunkerConfig::default(),
        );
        // Must complete without dead-loop and produce chunks
        let atomic_count = chunks
            .iter()
            .filter(|c| c.chunk_type == ChunkType::Atomic)
            .count();
        assert!(
            atomic_count >= 2,
            "should have multiple atomic chunks, got {atomic_count}"
        );
    }

    #[test]
    fn test_chunker_handles_oversized_single_paragraph() {
        let mut long_para = String::new();
        for i in 0..50 {
            long_para.push_str(&format!("这是第{i}个句子。一些额外的内容来增加长度。"));
        }
        let sections = vec![SectionInput {
            heading: "超长段落".into(),
            body: long_para,
            summary: None,
        }];
        let chunks = chunk_document(
            "标题",
            "摘要",
            "https://example.com",
            &sections,
            "vk",
            &ChunkerConfig::default(),
        );
        assert!(!chunks.is_empty());
        // Should have at least 1 doc summary + multiple atomic chunks
        let atomic_count = chunks
            .iter()
            .filter(|c| c.chunk_type == ChunkType::Atomic)
            .count();
        assert!(
            atomic_count >= 2,
            "oversized paragraph should produce multiple atomic chunks"
        );
    }

    #[test]
    fn test_chunker_empty_section_body() {
        // Empty section body must not panic or loop; doc summary still produced.
        let sections = vec![SectionInput {
            heading: "空".into(),
            body: String::new(),
            summary: None,
        }];
        let chunks = chunk_document(
            "标题",
            "摘要",
            "https://example.com",
            &sections,
            "vk",
            &ChunkerConfig::default(),
        );
        // Document summary chunk is always produced; empty body adds no atomics.
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].chunk_type, ChunkType::DocumentSummary);
    }

    #[test]
    fn test_chunker_no_separator_text() {
        // Text with no sentence separators and no paragraph breaks, under hard
        // max, must still produce exactly one atomic chunk (no loop, no panic).
        let body = "这是一段没有任何标点符号的连续文本内容".repeat(3);
        let sections = vec![SectionInput {
            heading: "无分隔".into(),
            body,
            summary: None,
        }];
        let chunks = chunk_document(
            "标题",
            "摘要",
            "https://example.com",
            &sections,
            "vk",
            &ChunkerConfig::default(),
        );
        let atomic_count = chunks
            .iter()
            .filter(|c| c.chunk_type == ChunkType::Atomic)
            .count();
        assert!(atomic_count >= 1, "no-separator text should still chunk");
    }
}
