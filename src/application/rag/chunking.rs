/// Simple overlap chunking service.
///
/// Splits text into fixed-size character windows with configurable overlap.
/// This is a lightweight strategy suitable for early-stage RAG pipelines
/// before switching to semantic or token-based chunking.

pub struct ChunkingService;

impl ChunkingService {
    pub fn new() -> Self {
        Self
    }

    /// Chunk `content` into pieces of `chunk_size` characters, sliding
    /// the window by `chunk_size - overlap` each step.
    ///
    /// # Arguments
    /// * `content`  - The raw text to split.
    /// * `chunk_size` - Maximum number of characters per chunk.
    /// * `overlap`    - Number of characters that overlap between adjacent chunks.
    ///
    /// # Panics
    /// Panics if `overlap >= chunk_size` (the window would not advance).
    pub fn chunk_text(&self, content: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
        assert!(
            overlap < chunk_size,
            "overlap ({overlap}) must be less than chunk_size ({chunk_size})"
        );

        let step = chunk_size - overlap;
        let len = content.len();
        let mut chunks: Vec<String> = Vec::new();

        if len == 0 {
            return chunks;
        }

        let mut start = 0usize;
        while start < len {
            let end = (start + chunk_size).min(len);
            chunks.push(content[start..end].to_string());
            start += step;
        }

        chunks
    }
}

impl Default for ChunkingService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_simple() {
        let service = ChunkingService::new();
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars
        let result = service.chunk_text(text, 10, 3);
        // step = 7: [0..10), [7..17), [14..24), [21..26)
        assert_eq!(
            result,
            vec![
                "abcdefghij".to_string(),
                "hijklmnopq".to_string(),
                "opqrstuvwx".to_string(),
                "vwxyz".to_string(),
            ]
        );
    }

    #[test]
    fn test_chunk_text_no_overlap() {
        let service = ChunkingService::new();
        let text = "abcdefghijklmnop";
        let result = service.chunk_text(text, 8, 0);
        assert_eq!(
            result,
            vec!["abcdefgh".to_string(), "ijklmnop".to_string(),]
        );
    }

    #[test]
    fn test_chunk_text_empty() {
        let service = ChunkingService::new();
        let result = service.chunk_text("", 10, 2);
        assert!(result.is_empty());
    }

    #[test]
    fn test_chunk_text_shorter_than_chunk_size() {
        let service = ChunkingService::new();
        let result = service.chunk_text("hello", 100, 0);
        assert_eq!(result, vec!["hello".to_string()]);
    }

    #[test]
    #[should_panic(expected = "must be less than chunk_size")]
    fn test_chunk_text_overlap_too_large() {
        let service = ChunkingService::new();
        service.chunk_text("abc", 5, 5);
    }
}
