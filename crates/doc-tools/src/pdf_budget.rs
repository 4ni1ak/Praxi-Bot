//! Conservative admission for PDF snapshots indexed on small hosts. The
//! parser remains in-process: these limits bound input, streams and retained
//! objects, not process RSS or the allocator's transient overhead.

use std::sync::Mutex;

use lopdf::{Document, LoadOptions, Object, ObjectId};
use tool_registry::ToolError;

use crate::common::exec;

pub(super) const DECODED_STREAM_BYTES: usize = 1024 * 1024;
const MAX_STREAMS: usize = 64;
const MAX_PAGES: usize = 64;
const MAX_DECODE_WORK: usize = 64 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECT_NODES: usize = 25_000;
const MAX_DEPTH: usize = 64;

// lopdf's filter is a function pointer. Serialize loads and keep the budget in
// a separate mutex so the accounting also works if a dependency enables rayon.
static LOAD: Mutex<()> = Mutex::new(());
static BUDGET: Mutex<Option<Budget>> = Mutex::new(None);

struct Budget {
    bytes: usize,
    nodes: usize,
    work: usize,
    exceeded: bool,
}

impl Budget {
    fn charge(&mut self, bytes: usize) -> bool {
        if self.exceeded || bytes > self.bytes {
            self.exceeded = true;
            return false;
        }
        self.bytes -= bytes;
        true
    }

    fn dictionary(&mut self, dictionary: &lopdf::Dictionary, depth: usize) -> bool {
        dictionary
            .iter()
            .all(|(key, value)| self.charge(key.capacity()) && self.object(value, depth + 1))
    }

    fn object(&mut self, object: &Object, depth: usize) -> bool {
        if self.nodes == 0 || depth > MAX_DEPTH {
            self.exceeded = true;
            return false;
        }
        self.nodes -= 1;
        // Charge structural overhead conservatively as well as owned buffers.
        if !self.charge(256) {
            return false;
        }
        match object {
            Object::Name(value) | Object::String(value, _) => self.charge(value.capacity()),
            Object::Array(values) => {
                self.charge(
                    values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Object>()),
                ) && values.iter().all(|value| self.object(value, depth + 1))
            }
            Object::Dictionary(dictionary) => self.dictionary(dictionary, depth),
            Object::Stream(stream) => {
                self.charge(stream.content.capacity()) && self.dictionary(&stream.dict, depth)
            }
            _ => true,
        }
    }

    fn object_stream(&mut self, object: &Object) -> bool {
        let Object::Stream(stream) = object else {
            return true;
        };
        if !stream.dict.has_type(b"ObjStm") {
            return true;
        }
        let admitted = (|| -> Option<usize> {
            let decoded = stream
                .get_plain_content_with_limit(DECODED_STREAM_BYTES)
                .ok()?;
            let first = usize::try_from(stream.dict.get(b"First").ok()?.as_i64().ok()?).ok()?;
            let declared = usize::try_from(stream.dict.get(b"N").ok()?.as_i64().ok()?).ok()?;
            let header = std::str::from_utf8(decoded.get(..first)?).ok()?;
            let mut count = 0usize;
            for number in header.split(['\r', '\n']).flat_map(|line| {
                line.split('%')
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
            }) {
                number.parse::<u32>().ok()?;
                count += 1;
                if count > MAX_OBJECT_NODES * 2 {
                    return None;
                }
            }
            if !count.is_multiple_of(2) || count / 2 != declared {
                return None;
            }
            // Offsets can overlap, making the parser revisit the same bytes
            // for many objects. Reserve this conservative work before expansion.
            decoded.len().checked_mul(declared.max(1))
        })();
        match admitted {
            Some(work) if work <= self.work => {
                self.work -= work;
                true
            }
            _ => {
                self.exceeded = true;
                false
            }
        }
    }
}

struct ResetBudget;
impl Drop for ResetBudget {
    fn drop(&mut self) {
        *BUDGET
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

fn filter(id: ObjectId, object: &mut Object) -> Option<(ObjectId, Object)> {
    let mut budget = BUDGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let budget = budget.as_mut()?;
    (budget.object(object, 0) && budget.object_stream(object)).then(|| (id, object.clone()))
}

fn delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            0 | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn skip_space(bytes: &[u8], mut cursor: usize) -> usize {
    loop {
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == 0)
        {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'%') {
            return cursor;
        }
        while bytes
            .get(cursor)
            .is_some_and(|byte| !matches!(*byte, b'\n' | b'\r'))
        {
            cursor += 1;
        }
    }
}

fn integer(bytes: &[u8], cursor: usize) -> Option<(i64, usize)> {
    let start = skip_space(bytes, cursor);
    let mut end = start;
    if bytes
        .get(end)
        .is_some_and(|byte| matches!(*byte, b'+' | b'-'))
    {
        end += 1;
    }
    let digits = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == digits {
        return None;
    }
    Some((
        std::str::from_utf8(&bytes[start..end]).ok()?.parse().ok()?,
        end,
    ))
}

fn indirect_length(bytes: &[u8], cursor: usize) -> bool {
    integer(bytes, cursor)
        .and_then(|(_, end)| integer(bytes, end))
        .is_some_and(|(_, end)| {
            let end = skip_space(bytes, end);
            bytes.get(end) == Some(&b'R') && bytes.get(end + 1).is_none_or(|byte| delimiter(*byte))
        })
}

/// Count every possible stream start, including false positives in strings or
/// compressed bytes. Over-counting can reject a document, but cannot weaken the
/// aggregate 64 MiB decoded-stream allowance. Reject encryption before loading:
/// lopdf's encrypted loader does not call the object filter. PDF #xx name
/// escapes are decoded, so spelling /Encr#79pt cannot bypass this admission.
fn preflight(bytes: &[u8]) -> Result<(), ToolError> {
    let mut streams = 0;
    let mut xref_sizes = 0usize;
    let mut xref_entries = 0usize;
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset..].starts_with(b"stream")
            && (offset == 0 || delimiter(bytes[offset - 1]))
            && bytes.get(offset + 6).is_some_and(|byte| delimiter(*byte))
        {
            streams += 1;
            if streams > MAX_STREAMS {
                return Err(exec("PDF exceeds the source stream budget"));
            }
        }
        if bytes[offset] == b'/' {
            let mut name = [0; 7];
            let mut count = 0;
            let mut cursor = offset + 1;
            while cursor < bytes.len() && !delimiter(bytes[cursor]) {
                let decoded = if bytes[cursor] == b'#' && cursor + 2 < bytes.len() {
                    hex(bytes[cursor + 1])
                        .zip(hex(bytes[cursor + 2]))
                        .map(|(high, low)| (high * 16 + low, 3))
                } else {
                    None
                };
                let (byte, width) = decoded.unwrap_or((bytes[cursor], 1));
                if count < name.len() {
                    name[count] = byte;
                }
                count += 1;
                cursor += width;
            }
            if count == name.len() && &name == b"Encrypt" {
                return Err(exec(
                    "encrypted PDFs are not supported as knowledge sources",
                ));
            }
            if count == 6 && &name[..6] == b"Length" && indirect_length(bytes, cursor) {
                return Err(exec(
                    "PDF source streams require direct lengths; export a simplified PDF",
                ));
            }
            if count == 4 && &name[..4] == b"Size" {
                let size = integer(bytes, cursor)
                    .and_then(|(value, _)| usize::try_from(value).ok())
                    .ok_or_else(|| exec("invalid PDF source cross-reference size"))?;
                xref_sizes = xref_sizes.saturating_add(size);
                if xref_sizes > MAX_OBJECT_NODES {
                    return Err(exec("PDF source cross-reference budget exceeded"));
                }
            }
            if count == 5
                && &name[..5] == b"Index"
                && bytes.get(skip_space(bytes, cursor)) == Some(&b'[')
            {
                let mut cursor = skip_space(bytes, cursor) + 1;
                while bytes.get(skip_space(bytes, cursor)) != Some(&b']') {
                    let (_, end) = integer(bytes, cursor)
                        .ok_or_else(|| exec("invalid PDF source cross-reference index"))?;
                    let (count, end) = integer(bytes, end)
                        .ok_or_else(|| exec("invalid PDF source cross-reference index"))?;
                    let count = usize::try_from(count)
                        .map_err(|_| exec("invalid PDF source cross-reference index"))?;
                    xref_entries = xref_entries.saturating_add(count);
                    if xref_entries > MAX_OBJECT_NODES {
                        return Err(exec("PDF source cross-reference budget exceeded"));
                    }
                    cursor = end;
                }
            }
        }
        offset += 1;
    }
    Ok(())
}

pub(super) fn load(bytes: &[u8]) -> Result<Document, ToolError> {
    load_with_budget(bytes, MAX_OBJECT_BYTES, MAX_OBJECT_NODES)
}

fn load_with_budget(
    bytes: &[u8],
    object_bytes: usize,
    object_nodes: usize,
) -> Result<Document, ToolError> {
    preflight(bytes)?;
    let _load = LOAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *BUDGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Budget {
        bytes: object_bytes,
        nodes: object_nodes,
        work: MAX_DECODE_WORK,
        exceeded: false,
    });
    let _reset = ResetBudget;
    let document = Document::load_mem_with_options(
        bytes,
        LoadOptions {
            filter: Some(filter),
            strict: true,
            max_decompressed_size: Some(DECODED_STREAM_BYTES),
            ..LoadOptions::default()
        },
    );
    let exceeded = BUDGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_none_or(|budget| budget.exceeded);
    if exceeded {
        return Err(exec("PDF exceeds the aggregate source object budget"));
    }
    let document =
        document.map_err(|_| exec("PDF source is malformed or exceeds a stream limit"))?;
    if document.is_encrypted() {
        return Err(exec(
            "encrypted PDFs are not supported as knowledge sources",
        ));
    }
    if document.get_pages().len() > MAX_PAGES {
        return Err(exec(
            "PDF source exceeds the 64-page limit; select a smaller document",
        ));
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};
    use std::io::Write;

    #[test]
    fn review_81_parser_panic_does_not_poison_future_pdf_loads() {
        let panic = std::panic::catch_unwind(|| {
            let _load = LOAD
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _budget = BUDGET
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            panic!("simulated dependency panic while accounting PDF objects");
        });
        assert!(panic.is_err());
        let bytes = crate::pdf::tests::make_pdf(&[&["Healthy document after parser failure"]]);
        assert_eq!(load(&bytes).unwrap().get_pages().len(), 1);
    }

    fn compressed_objects(streams: u32, bytes_per_object: usize) -> Vec<u8> {
        let mut output = b"%PDF-1.5\n".to_vec();
        let mut offsets = vec![0];
        for (number, body) in [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>"),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>"),
        ] {
            offsets.push(output.len());
            writeln!(output, "{number} 0 obj\n{body}\nendobj").unwrap();
        }
        for index in 0..streams {
            let header = format!("{} 0 ", 10_000 + index);
            let content = format!("{header}({})", "x".repeat(bytes_per_object));
            let mut stream = Stream::new(
                dictionary! {"Type"=>"ObjStm", "N"=>1, "First"=>i64::try_from(header.len()).unwrap()},
                content.into_bytes(),
            );
            stream.compress().unwrap();
            offsets.push(output.len());
            writeln!(output, "{} 0 obj\n{:?}\nstream", index + 4, stream.dict).unwrap();
            output.extend_from_slice(&stream.content);
            output.extend_from_slice(b"\nendstream\nendobj\n");
        }
        let xref = output.len();
        writeln!(output, "xref\n0 {}\n0000000000 65535 f ", offsets.len()).unwrap();
        for offset in offsets.iter().skip(1) {
            writeln!(output, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            output,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .unwrap();
        output
    }

    #[test]
    fn many_small_compressed_object_streams_share_one_budget() {
        let one = compressed_objects(1, 2048);
        assert!(load_with_budget(&one, 16_384, 1000).is_ok());
        let many = compressed_objects(12, 2048);
        assert!(many.len() < 16_384, "fixture remains small on disk");
        assert!(
            load_with_budget(&many, 16_384, 1000)
                .unwrap_err()
                .to_string()
                .contains("aggregate")
        );
        assert!(
            load_with_budget(&one, 64_000, 2).is_err(),
            "object nodes are independently capped"
        );
    }

    #[test]
    fn encrypted_names_and_stream_count_fail_before_parser_expansion() {
        for name in [b"/Encrypt".as_slice(), b"/Encr#79pt", b"/#45ncrypt"] {
            assert!(preflight(name).is_err());
        }
        assert!(preflight(&b"stream\n".repeat(MAX_STREAMS + 1)).is_err());
        assert!(load(&compressed_objects(1, DECODED_STREAM_BYTES + 1)).is_err());
        assert!(preflight(b"/Length 5 % comment\n 0 R").is_err());
        assert!(preflight(b"/Len#67th +5 +0R").is_err());
        assert!(preflight(b"/Size 25001").is_err());
        assert!(preflight(b"/Size 2 /Index [0 25001]").is_err());
        assert!(load(b"not a PDF").is_err());
    }
}
