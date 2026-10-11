//! Preserve PDF attachments and source metadata without writing attachment filenames to disk.
use crate::{diagrams, extract::ContextData, model::*};
use anyhow::{bail, Context, Result};
use lopdf::Object;
use std::{io::Read, path::Path};
pub fn embedded(c: &mut ContextData, input: &Path, u: &mut Unit) -> Result<()> {
    let pdf = lopdf::Document::load(input).context("corrupt PDF source objects")?;
    let mut count = 0;
    for (id, object) in &pdf.objects {
        match object {
            Object::Stream(stream) => {
                let kind = stream.dict.get(b"Type").ok().and_then(|o| o.as_name().ok());
                if kind != Some(b"EmbeddedFile".as_slice()) && kind != Some(b"Metadata".as_slice())
                {
                    continue;
                }
                count += 1;
                if count > c.limits.archive_entries {
                    bail!("limit_exceeded: PDF attachments");
                }
                let filters = stream.filters().unwrap_or_default();
                let mut bytes = vec![];
                if filters.is_empty() {
                    bytes = stream.content.clone();
                } else if filters == vec![b"FlateDecode".as_slice()] {
                    flate2::read::ZlibDecoder::new(&stream.content[..])
                        .take(c.limits.archive_bytes + 1)
                        .read_to_end(&mut bytes)?;
                } else {
                    bytes = stream.content.clone();
                    u.warnings.push(format!("PDF object {} uses an unsupported attachment filter; encoded bytes retained",id.0));
                }
                if bytes.len() as u64 > c.limits.archive_bytes {
                    bail!("limit_exceeded: PDF source bytes");
                }
                let name = format!(
                    "PDF-object-{}-{}.{}",
                    id.0,
                    id.1,
                    if kind == Some(b"Metadata".as_slice()) {
                        "xml"
                    } else {
                        "bin"
                    }
                );
                let asset = c.asset(&bytes, &name, u)?;
                u.text("unknown", "Embedded PDF source object", &name)
                    .asset_id = Some(asset);
                source_graphs(c, &bytes, u, &name)?;
            }
            Object::Dictionary(dictionary) => {
                for (key, value) in dictionary.iter() {
                    if let Object::String(bytes, _) = value {
                        if [b"Keywords".as_slice(), b"Subject", b"mxfile"].contains(&key.as_slice())
                        {
                            source_graphs(c, bytes, u, "PDF metadata string")?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn source_graphs(c: &ContextData, bytes: &[u8], u: &mut Unit, source: &str) -> Result<()> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(());
    };
    let decoded = percent_encoding::percent_decode_str(text).decode_utf8_lossy();
    if let (Some(start), Some(end)) = (decoded.find("<mxfile"), decoded.rfind("</mxfile>")) {
        match diagrams::drawio(&decoded[start..end + 9], c.limits.archive_bytes) {
            Ok(pages) => {
                for (name, graph) in pages {
                    u.text("diagram", name, source).diagram = Some(graph);
                }
            }
            Err(e) => u
                .warnings
                .push(format!("Embedded diagram source invalid: {e}")),
        }
    }
    Ok(())
}
