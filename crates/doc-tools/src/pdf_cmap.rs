//! Source-only admission before lopdf expands ToUnicode font mappings.
//!
//! lopdf 0.47 eagerly enumerates source ranges while constructing a reverse map,
//! even when only decoding is requested. A decompressed-byte limit alone does
//! not bound that expansion. Inspect the mapping operators without constructing
//! a CMap, including unused fonts and inline dictionaries. This deliberately
//! conservative subset may reject uncommon maps; it never truncates a mapping.

use lopdf::{Document, Object};
use tool_registry::ToolError;

use crate::common::exec;

const MAX_MAPPINGS: usize = 4096;
const MAX_TARGET_UNITS: usize = 8;
const MAX_CMAP_BYTES: usize = 4 * 1024 * 1024;
const MAX_SCAN_WORK: usize = 8 * 1024 * 1024;
const MAX_NODES: usize = 25_000;

struct Budget {
    mappings: usize,
    target_units: usize,
    bytes: usize,
    scan_work: usize,
    nodes: usize,
}

fn rejected() -> ToolError {
    exec("PDF source font mapping is malformed or exceeds its expansion budget")
}

fn charge(remaining: &mut usize, amount: usize) -> Result<(), ToolError> {
    *remaining = remaining.checked_sub(amount).ok_or_else(rejected)?;
    Ok(())
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            mappings: MAX_MAPPINGS,
            target_units: MAX_MAPPINGS * MAX_TARGET_UNITS,
            bytes: MAX_CMAP_BYTES,
            scan_work: MAX_SCAN_WORK,
            nodes: MAX_NODES,
        }
    }
}

pub(super) fn validate_document(document: &Document) -> Result<(), ToolError> {
    let mut budget = Budget::default();
    for object in document.objects.values() {
        visit(document, object, 0, &mut budget)?;
    }
    Ok(())
}

fn visit(
    document: &Document,
    object: &Object,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), ToolError> {
    charge(&mut budget.nodes, 1)?;
    if depth > 64 {
        return Err(rejected());
    }
    let dictionary = match object {
        Object::Dictionary(dictionary) => Some(dictionary),
        Object::Stream(stream) => Some(&stream.dict),
        Object::Array(objects) => {
            for object in objects {
                visit(document, object, depth + 1, budget)?;
            }
            None
        }
        // Every referenced object is visited once through document.objects.
        // Only the specific ToUnicode edge below is dereferenced, with lopdf's
        // own bounded reference resolution and no recursive graph traversal.
        _ => None,
    };
    if let Some(dictionary) = dictionary {
        if let Ok(value) = dictionary.get(b"ToUnicode") {
            let (_, value) = document.dereference(value).map_err(|_| rejected())?;
            let stream = value.as_stream().map_err(|_| rejected())?;
            let bytes = stream
                .get_plain_content_with_limit(crate::pdf_budget::DECODED_STREAM_BYTES)
                .map_err(|_| rejected())?;
            charge(&mut budget.bytes, bytes.len())?;
            validate_cmap(&bytes, budget)?;
        }
        for (_, value) in dictionary {
            visit(document, value, depth + 1, budget)?;
        }
    }
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn space(&mut self) {
        loop {
            while self
                .bytes
                .get(self.at)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == 0)
            {
                self.at += 1;
            }
            if self.bytes.get(self.at) != Some(&b'%') {
                return;
            }
            while self
                .bytes
                .get(self.at)
                .is_some_and(|byte| !matches!(byte, b'\r' | b'\n'))
            {
                self.at += 1;
            }
        }
    }

    fn hex(&mut self, max_digits: usize) -> Result<([u8; 32], usize), ToolError> {
        self.space();
        if self.bytes.get(self.at) != Some(&b'<') {
            return Err(rejected());
        }
        self.at += 1;
        let mut digits = [0; 32];
        let mut count = 0;
        loop {
            self.space();
            let byte = *self.bytes.get(self.at).ok_or_else(rejected)?;
            self.at += 1;
            if byte == b'>' {
                return if count == 0 {
                    Err(rejected())
                } else {
                    Ok((digits, count))
                };
            }
            if count >= max_digits {
                return Err(rejected());
            }
            digits[count] = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(rejected()),
            };
            count += 1;
        }
    }

    fn source(&mut self) -> Result<(u32, usize), ToolError> {
        let (digits, count) = self.hex(8)?;
        if !count.is_multiple_of(2) {
            return Err(rejected());
        }
        Ok((
            digits[..count]
                .iter()
                .fold(0_u32, |code, digit| code * 16 + u32::from(*digit)),
            count,
        ))
    }

    fn target(&mut self) -> Result<usize, ToolError> {
        let (_, digits) = self.hex(MAX_TARGET_UNITS * 4)?;
        if !digits.is_multiple_of(4) {
            return Err(rejected());
        }
        Ok(digits / 4)
    }
}

fn validate_cmap(bytes: &[u8], budget: &mut Budget) -> Result<(), ToolError> {
    // Scan every byte, including operator-like text inside comments/metadata.
    // False positives fail closed. No lexical skip can hide a real mapping
    // operator from this admission pass. Repeated parsing work is also capped.
    for start in 0..bytes.len() {
        let range = if bytes[start..].starts_with(b"beginbfrange") {
            true
        } else if bytes[start..].starts_with(b"beginbfchar") {
            false
        } else {
            continue;
        };
        let mut cursor = Cursor {
            bytes,
            at: start + if range { 12 } else { 11 },
        };
        section(&mut cursor, budget, range)?;
        charge(&mut budget.scan_work, cursor.at - start)?;
    }
    Ok(())
}

fn section(cursor: &mut Cursor<'_>, budget: &mut Budget, range: bool) -> Result<(), ToolError> {
    let end = if range {
        b"endbfrange".as_slice()
    } else {
        b"endbfchar".as_slice()
    };
    loop {
        cursor.space();
        if cursor.bytes[cursor.at..].starts_with(end) {
            cursor.at += end.len();
            return Ok(());
        }
        let (first, width) = cursor.source()?;
        let count = if range {
            let (last, last_width) = cursor.source()?;
            if last_width != width || last < first {
                return Err(rejected());
            }
            usize::try_from(u64::from(last) - u64::from(first) + 1).map_err(|_| rejected())?
        } else {
            1
        };
        charge(&mut budget.mappings, count)?;
        cursor.space();
        if range && cursor.bytes.get(cursor.at) == Some(&b'[') {
            cursor.at += 1;
            for _ in 0..count {
                let units = cursor.target()?;
                charge(&mut budget.target_units, units)?;
            }
            cursor.space();
            if cursor.bytes.get(cursor.at) != Some(&b']') {
                return Err(rejected());
            }
            cursor.at += 1;
        } else {
            let units = cursor.target()?;
            charge(&mut budget.target_units, count * units)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    fn font(mappings: &str) -> (Document, lopdf::ObjectId) {
        let cmap = format!(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
             /CMapName /Bounded def\n/CMapType 2 def\n\
             1 begincodespacerange\n<00000000> <ffffffff>\nendcodespacerange\n\
             {mappings}\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"
        );
        let mut document = Document::with_version("1.7");
        let cmap_id = document.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
        let font_id = document.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Bounded",
            "Encoding" => "Identity-H", "ToUnicode" => cmap_id,
        });
        (document, font_id)
    }

    #[test]
    fn rejects_tiny_cmap_before_eager_full_range_expansion() {
        let (document, _) = font("1 beginbfrange\n<00000000> <ffffffff> <0041>\nendbfrange");
        // Never hand this range to lopdf: its reverse-map constructor enumerates
        // all source codes before text extraction can return an error or output.
        assert!(validate_document(&document).is_err());
    }

    #[test]
    fn bounded_range_and_ligature_remain_extractable() {
        let (document, id) = font(
            "1 beginbfrange\n<00000001> <00000020> <0041>\nendbfrange\n\
             1 beginbfchar\n<00000040> <00660069>\nendbfchar",
        );
        validate_document(&document).unwrap();
        let encoding = document
            .get_dictionary(id)
            .unwrap()
            .get_font_encoding_with_limit(&document, 1024 * 1024)
            .unwrap();
        assert_eq!(
            encoding
                .bytes_to_string(&[0, 0, 0, 1, 0, 0, 0, 64])
                .unwrap(),
            "Afi"
        );
    }

    #[test]
    fn range_coverage_is_aggregated_across_sections_and_fonts() {
        let (document, _) = font(
            "1 beginbfrange\n<00000000> <00000800> <0041>\nendbfrange\n\
             1 beginbfrange\n<00001000> <00001800> <0041>\nendbfrange",
        );
        assert!(validate_document(&document).is_err());
        let (mut document, id) = font("1 beginbfrange\n<00000000> <00000800> <0041>\nendbfrange");
        let duplicate = document.get_object(id).unwrap().clone();
        document.add_object(duplicate);
        assert!(validate_document(&document).is_err());
    }

    #[test]
    fn destination_arrays_comments_and_ligatures_are_bounded() {
        for mappings in [
            "1 beginbfchar\n<01> <004100410041004100410041004100410041>\nendbfchar",
            "1 beginbfrange\n<01><03>[<0041><0042>]\nendbfrange",
            "1 beginbfrange\n<01><02>[<0041><0042><0043>]\nendbfrange",
            "1 beginbfrange\n<ff><00><0041>\nendbfrange",
        ] {
            let (document, _) = font(mappings);
            assert!(validate_document(&document).is_err(), "{mappings}");
        }
        let (document, _) =
            font("1 beginbfrange\n<01><02>[<0041> % line comment\n <0066 0069>]\nendbfrange");
        validate_document(&document).unwrap();
    }

    #[test]
    fn inline_fonts_and_decoded_bytes_share_the_document_budget() {
        let (mut document, id) = font("1 beginbfrange\n<00000000> <ffffffff> <0041>\nendbfrange");
        let inline = document.objects.remove(&id).unwrap();
        document.add_object(dictionary! {"Font" => dictionary! {"F1" => inline}});
        assert!(validate_document(&document).is_err());

        let (mut document, id) = font("0 beginbfchar\nendbfchar");
        let stream_id = document
            .get_dictionary(id)
            .unwrap()
            .get(b"ToUnicode")
            .unwrap()
            .as_reference()
            .unwrap();
        document
            .objects
            .get_mut(&stream_id)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .set_content(vec![b' '; crate::pdf_budget::DECODED_STREAM_BYTES]);
        let duplicate = document.get_object(id).unwrap().clone();
        for _ in 0..4 {
            document.add_object(duplicate.clone());
        }
        assert!(validate_document(&document).is_err());
    }
}
