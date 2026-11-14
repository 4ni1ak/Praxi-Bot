//! Linear, bounded Markdown-aware chunking. Oversized blocks split on lines.

use super::{DocumentFormat, KnowledgeLimits, TextChunk};
use crate::{MemoryError, Result};

/// Chunk decoded text, preserving headings, fences and tables when they fit.
///
/// Oversized blocks split on complete lines; a single oversized line splits at
/// Unicode scalar boundaries. Table headers are repeated in split table chunks.
/// Line citations always refer to the supplied text, not an invented PDF page.
///
/// # Errors
/// Rejects invalid limits, excessive input, excessive lines, or too many chunks.
pub fn chunk_text(
    text: &str,
    format: DocumentFormat,
    limits: KnowledgeLimits,
) -> Result<Vec<TextChunk>> {
    validate_limits(limits)?;
    if text.len() > limits.document_bytes {
        return Err(MemoryError::Invalid(
            "knowledge document exceeds byte limit".into(),
        ));
    }
    let lines: Vec<&str> = text.split_inclusive('\n').take(262_145).collect();
    if lines.len() > 262_144 {
        return Err(MemoryError::Invalid(
            "knowledge document exceeds line limit".into(),
        ));
    }
    let mut builder = Builder {
        limits,
        out: Vec::new(),
        pending: None,
        pending_chars: 0,
    };
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        if format == DocumentFormat::Markdown
            && let Some((level, title)) = heading(lines[i])
        {
            builder.flush()?;
            headings.retain(|(depth, _)| *depth < level);
            headings.push((level, title.chars().take(256).collect()));
        }
        let section = headings
            .iter()
            .map(|(_, h)| h.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let start = i;
        let mut table_header = None;
        if format == DocumentFormat::Markdown
            && let Some((mark, width)) = fence(lines[i])
        {
            i += 1;
            while i < lines.len() {
                let closing = closing_fence(lines[i], mark, width);
                i += 1;
                if closing {
                    break;
                }
            }
        } else if format == DocumentFormat::Markdown && is_table(&lines, i) {
            table_header = Some(format!("{}{}", lines[i], lines[i + 1]));
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                i += 1;
            }
        } else {
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() {
                if format == DocumentFormat::Markdown
                    && (heading(lines[i]).is_some()
                        || fence(lines[i]).is_some()
                        || is_table(&lines, i))
                {
                    break;
                }
                i += 1;
            }
        }
        builder.block(
            &lines[start..i],
            start + 1,
            &section,
            table_header.as_deref(),
        )?;
    }
    builder.flush()?;
    Ok(builder.out)
}

pub(super) fn validate_limits(limits: KnowledgeLimits) -> Result<()> {
    if !(1..=16 * 1024 * 1024).contains(&limits.document_bytes)
        || !(128..=8000).contains(&limits.chunk_chars)
        || !(1..=16_384).contains(&limits.chunks_per_document)
        || !(1..=4096).contains(&limits.sources)
        || !(1..=100_000).contains(&limits.documents)
        || !(1..=250_000).contains(&limits.chunks)
    {
        return Err(MemoryError::Invalid(
            "invalid knowledge resource limits".into(),
        ));
    }
    Ok(())
}

fn heading(line: &str) -> Option<(usize, &str)> {
    if line.starts_with('\t') || line.chars().take_while(|c| *c == ' ').count() > 3 {
        return None;
    }
    let line = line.trim_start();
    let depth = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&depth) || !line[depth..].starts_with(char::is_whitespace) {
        return None;
    }
    Some((depth, line[depth..].trim().trim_end_matches('#').trim_end()))
}

fn fence(line: &str) -> Option<(char, usize)> {
    if line.starts_with('\t') || line.chars().take_while(|c| *c == ' ').count() > 3 {
        return None;
    }
    let line = line.trim_start();
    let first = line.chars().next()?;
    if !matches!(first, '`' | '~') {
        return None;
    }
    let width = line.chars().take_while(|c| *c == first).count();
    (width >= 3).then_some((first, width))
}

fn closing_fence(line: &str, mark: char, width: usize) -> bool {
    let line = line.trim();
    line.chars().count() >= width && line.chars().all(|c| c == mark)
}

fn is_table(lines: &[&str], index: usize) -> bool {
    if !lines[index].contains('|') {
        return false;
    }
    let Some(next) = lines.get(index + 1) else {
        return false;
    };
    let separator = next.trim().trim_matches('|');
    !separator.is_empty()
        && separator.split('|').all(|cell| {
            let cell = cell.trim().trim_matches(':');
            cell.len() >= 3 && cell.chars().all(|c| c == '-')
        })
}

struct Builder {
    limits: KnowledgeLimits,
    out: Vec<TextChunk>,
    pending: Option<TextChunk>,
    pending_chars: usize,
}

impl Builder {
    fn flush(&mut self) -> Result<()> {
        self.pending_chars = 0;
        if let Some(chunk) = self.pending.take() {
            if self.out.len() >= self.limits.chunks_per_document {
                return Err(MemoryError::Invalid(
                    "knowledge document exceeds chunk limit".into(),
                ));
            }
            self.out.push(chunk);
        }
        Ok(())
    }

    fn push(&mut self, text: String, start: usize, end: usize, section: &str) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let text_chars = text.chars().count();
        if let Some(pending) = &mut self.pending
            && pending.section == section
            && self.pending_chars + text_chars < self.limits.chunk_chars
        {
            pending.content.push('\n');
            pending.content.push_str(&text);
            pending.line_end = end;
            self.pending_chars += text_chars + 1;
            return Ok(());
        }
        self.flush()?;
        self.pending_chars = text_chars;
        self.pending = Some(TextChunk {
            section: section.into(),
            line_start: start,
            line_end: end,
            content: text,
        });
        Ok(())
    }

    fn block(
        &mut self,
        lines: &[&str],
        start: usize,
        section: &str,
        table_header: Option<&str>,
    ) -> Result<()> {
        let chars: usize = lines.iter().map(|line| line.chars().count()).sum();
        if chars <= self.limits.chunk_chars {
            return self.push(lines.concat(), start, start + lines.len() - 1, section);
        }
        self.flush()?;
        // Very wide headers are split normally rather than leaving no room for
        // data rows or allowing a header to bypass the chunk bound.
        let repeat = table_header
            .filter(|h| h.chars().count() < self.limits.chunk_chars / 2)
            .unwrap_or("");
        let mut part = String::new();
        let mut used = 0;
        let mut part_start = start;
        for (offset, line) in lines.iter().enumerate() {
            let number = start + offset;
            let line_chars = line.chars().count();
            if used > 0 && used + line_chars > self.limits.chunk_chars {
                self.push(
                    std::mem::take(&mut part),
                    part_start,
                    number.saturating_sub(1).max(part_start),
                    section,
                )?;
                self.flush()?;
                part.push_str(repeat);
                used = repeat.chars().count();
                part_start = number;
            }
            for ch in line.chars() {
                if used == self.limits.chunk_chars {
                    self.push(std::mem::take(&mut part), part_start, number, section)?;
                    self.flush()?;
                    part.push_str(repeat);
                    used = repeat.chars().count();
                    part_start = number;
                }
                part.push(ch);
                used += 1;
            }
        }
        self.push(part, part_start, start + lines.len() - 1, section)?;
        self.flush()
    }
}
