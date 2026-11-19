//! Plain-text extraction from `.pptx` slides (reading only).

use std::fmt::Write as _;
use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::events::Event;
use tool_registry::ToolError;

use crate::common::exec;

const MAX_SLIDES: usize = 500;
const MAX_SLIDE_XML_BYTES: u64 = 8 * 1024 * 1024;

/// Text of every slide, in slide order, as `## Slide N` sections.
pub(crate) fn plain_text(bytes: &[u8], max_bytes: usize) -> Result<String, ToolError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| exec(format!("not a valid pptx archive: {e}")))?;
    let mut slides: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| {
            let n = n.ok()?;
            let num = n
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse::<u32>()
                .ok()?;
            Some((num, n.into_owned()))
        })
        .collect();
    slides.sort_unstable();
    slides.truncate(MAX_SLIDES);
    let mut out = String::new();
    for (num, name) in slides {
        if out.len() >= max_bytes {
            break;
        }
        let mut xml = Vec::new();
        zip.by_name(&name)
            .map_err(|e| exec(format!("cannot read {name}: {e}")))?
            .take(MAX_SLIDE_XML_BYTES + 1)
            .read_to_end(&mut xml)
            .map_err(|e| exec(format!("cannot read {name}: {e}")))?;
        if xml.len() as u64 > MAX_SLIDE_XML_BYTES {
            return Err(exec(format!("{name} expands beyond the size limit")));
        }
        let text = slide_text(&xml)?;
        if !text.trim().is_empty() {
            let _ = write!(out, "## Slide {num}\n\n{}\n\n", text.trim_end());
        }
    }
    Ok(out)
}

fn slide_text(xml: &[u8]) -> Result<String, ToolError> {
    let mut reader = Reader::from_reader(xml);
    let mut out = String::new();
    let mut in_t = false;
    loop {
        match reader
            .read_event()
            .map_err(|e| exec(format!("invalid slide xml: {e}")))?
        {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == "t" => in_t = true,
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "p" => out.push('\n'),
                _ => {}
            },
            Event::Text(t) if in_t => {
                let s: &str = &t;
                out.push_str(s);
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn reads_slide_text_in_order() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (n, t) in [(2, "Second"), (1, "First")] {
            zip.start_file(format!("ppt/slides/slide{n}.xml"), opts)
                .unwrap();
            write!(
                zip,
                "<p:sld xmlns:a=\"a\" xmlns:p=\"p\"><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"
            )
            .unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let text = plain_text(&bytes, 10_000).unwrap();
        assert!(text.find("First").unwrap() < text.find("Second").unwrap());
        assert!(text.contains("## Slide 1"));
    }
}
