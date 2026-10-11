use crate::model::*;
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::json;
use std::io::Read;

pub fn xml(s: &str) -> Result<roxmltree::Document<'_>> {
    if s.to_ascii_uppercase().contains("<!DOCTYPE") || s.to_ascii_uppercase().contains("<!ENTITY") {
        bail!("unsafe_xml: DTD and entities are disabled");
    }
    roxmltree::Document::parse(s).context("corrupt XML")
}
pub fn drawio(source: &str, limit: u64) -> Result<Vec<(String, Diagram)>> {
    let doc = xml(source)?;
    let pages: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("diagram"))
        .collect();
    if pages.is_empty() {
        return Ok(vec![("Diagram".into(), graph(doc.root_element()))]);
    }
    let mut out = Vec::new();
    for p in pages {
        let name = p.attribute("name").unwrap_or("Diagram").to_owned();
        if let Some(m) = p.children().find(|n| n.has_tag_name("mxGraphModel")) {
            out.push((name, graph(m)));
        } else {
            let compressed = base64::engine::general_purpose::STANDARD
                .decode(p.text().unwrap_or("").trim())
                .context("corrupt draw.io base64")?;
            let mut encoded = String::new();
            flate2::read::DeflateDecoder::new(&compressed[..])
                .take(limit + 1)
                .read_to_string(&mut encoded)?;
            if encoded.len() as u64 > limit {
                bail!("limit_exceeded: inflated diagram");
            }
            let decoded = percent_encoding::percent_decode_str(&encoded).decode_utf8()?;
            let inner = xml(&decoded)?;
            out.push((name, graph(inner.root_element())));
        }
    }
    Ok(out)
}
fn graph(root: roxmltree::Node<'_, '_>) -> Diagram {
    let mut g = Diagram::default();
    for n in root.descendants().filter(|n| n.has_tag_name("mxCell")) {
        let wrapper = n
            .parent()
            .filter(|p| p.has_tag_name("object") || p.has_tag_name("UserObject"));
        let id = n
            .attribute("id")
            .or_else(|| wrapper.and_then(|p| p.attribute("id")))
            .unwrap_or("")
            .to_owned();
        let label = n
            .attribute("value")
            .or_else(|| wrapper.and_then(|p| p.attribute("label")))
            .unwrap_or("")
            .to_owned();
        let attrs = json!({"style": n.attribute("style"), "source_xml": &n.document().input_text()[n.range()], "user_object": wrapper.map(|p| p.attributes().map(|a| (a.name().to_owned(), a.value().to_owned())).collect::<std::collections::BTreeMap<_,_>>())});
        if n.attribute("edge") == Some("1") {
            g.edges.push(DiagramEdge {
                id,
                source: n.attribute("source").map(str::to_owned),
                target: n.attribute("target").map(str::to_owned),
                label,
                method: "native".into(),
                cardinality: None,
                attributes: attrs,
            });
        } else if n.attribute("vertex") == Some("1") {
            let bounds = n
                .children()
                .find(|c| c.has_tag_name("mxGeometry"))
                .map(|c| Bounds {
                    x: num(c, "x"),
                    y: num(c, "y"),
                    width: num(c, "width"),
                    height: num(c, "height"),
                    coordinate_space: "drawio_local_parent".into(),
                });
            g.nodes.push(DiagramNode {
                id,
                label,
                parent: n.attribute("parent").map(str::to_owned),
                bounds,
                attributes: attrs,
            });
        }
    }
    g
}
fn num(n: roxmltree::Node<'_, '_>, a: &str) -> f64 {
    n.attribute(a).and_then(|s| s.parse().ok()).unwrap_or(0.)
}

pub fn embedded_png(bytes: &[u8], limit: u64) -> Result<Option<String>> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(None);
    }
    let mut i = 8;
    while i + 12 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[i..i + 4].try_into()?) as usize;
        let end = i
            .checked_add(12)
            .and_then(|v| v.checked_add(size))
            .context("PNG overflow")?;
        if end > bytes.len() {
            bail!("corrupt PNG chunk");
        }
        let tag = &bytes[i + 4..i + 8];
        let data = &bytes[i + 8..i + 8 + size];
        if tag == b"tEXt" || tag == b"zTXt" {
            if let Some(split) = data.iter().position(|b| *b == 0) {
                if &data[..split] == b"mxfile" {
                    let mut s = String::new();
                    if tag == b"zTXt" {
                        if data.get(split + 1) != Some(&0) {
                            bail!("unsupported PNG compression");
                        }
                        flate2::read::ZlibDecoder::new(&data[split + 2..])
                            .take(limit + 1)
                            .read_to_string(&mut s)?;
                    } else {
                        s = String::from_utf8(data[split + 1..].to_vec())?;
                    }
                    if s.len() as u64 > limit {
                        bail!("limit_exceeded: embedded diagram");
                    }
                    return Ok(Some(s));
                }
            }
        }
        i = end;
    }
    Ok(None)
}

pub fn source(s: &str, format: &str) -> Result<Diagram> {
    if format == "mermaid" && s.lines().any(|l| l.trim() == "erDiagram")
        || format == "plantuml" && s.contains("entity ")
    {
        return er_source(s, format);
    }
    let mut g = Diagram::default();
    let node_re = regex::Regex::new(r#"^([A-Za-z_][\w]*)\s*(?:\[([^\]]*)\]|\(([^)]*)\))?$"#)?;
    let edge_re =
        regex::Regex::new(r#"^([A-Za-z_][\w]*)\s*(-->|---|->|--)\s*([A-Za-z_][\w]*)\s*;?$"#)?;
    let dot_node = regex::Regex::new(r#"^([A-Za-z_][\w]*)\s*\[label="([^"]*)"\]\s*;?$"#)?;
    let mut seen = std::collections::HashSet::new();
    let add =
        |g: &mut Diagram, seen: &mut std::collections::HashSet<String>, id: &str, label: &str| {
            if seen.insert(id.to_owned()) {
                g.nodes.push(DiagramNode {
                    id: id.into(),
                    label: label.into(),
                    parent: None,
                    bounds: None,
                    attributes: json!({}),
                });
            } else if label != id {
                if let Some(n) = g.nodes.iter_mut().find(|n| n.id == id) {
                    n.label = label.into();
                }
            }
        };
    if format == "dbml" {
        let table_re = regex::Regex::new(r"(?s)Table\s+(\w+)\s*\{([^}]*)\}")?;
        let field_re = regex::Regex::new(r"^\s*(\w+)\s+([\w()]+)(?:\s*\[([^\]]*)\])?\s*$")?;
        let mut residual = s.to_owned();
        for c in table_re.captures_iter(s) {
            let mut attrs = vec![];
            for line in c[2].lines().filter(|l| !l.trim().is_empty()) {
                if let Some(f) = field_re.captures(line) {
                    attrs.push(json!({"name": &f[1], "type": &f[2], "evidence": f.get(3).map(|v| v.as_str()), "primary_key": f.get(3).map(|v| v.as_str().split(',').any(|a| a.trim()=="pk")).unwrap_or(false)}));
                } else {
                    g.warnings.push(format!("unsupported DBML field: {line}"));
                }
            }
            g.nodes.push(DiagramNode {
                id: c[1].into(),
                label: c[1].into(),
                parent: None,
                bounds: None,
                attributes: json!({"entity_attributes": attrs}),
            });
            residual = residual.replace(&c[0], "");
        }
        let ref_re = regex::Regex::new(r"^Ref:\s*(\w+\.\w+)\s*([<>-])\s*(\w+\.\w+)\s*$")?;
        for line in residual
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
        {
            if let Some(c) = ref_re.captures(line) {
                g.edges.push(DiagramEdge {
                    id: format!("e{}", g.edges.len() + 1),
                    source: Some(c[1].into()),
                    target: Some(c[3].into()),
                    label: "".into(),
                    method: "native".into(),
                    cardinality: Some(c[2].into()),
                    attributes: json!({"source_statement": line}),
                });
            } else {
                g.warnings
                    .push(format!("unsupported DBML construct: {line}"));
            }
        }
        let inline = regex::Regex::new(r"ref:\s*([<>-])\s*(\w+\.\w+)")?;
        for node in &g.nodes {
            for attribute in node.attributes["entity_attributes"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(evidence) = attribute["evidence"].as_str() {
                    if let Some(reference) = inline.captures(evidence) {
                        g.edges.push(DiagramEdge {
                            id: format!("e{}", g.edges.len() + 1),
                            source: Some(format!(
                                "{}.{}",
                                node.id,
                                attribute["name"].as_str().unwrap_or("")
                            )),
                            target: Some(reference[2].into()),
                            label: "".into(),
                            method: "native".into(),
                            cardinality: Some(reference[1].into()),
                            attributes: json!({"explicit_inline_reference":evidence}),
                        });
                    }
                }
            }
        }
        return Ok(g);
    }
    for line in s.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("graph ") && format == "mermaid"
            || line.starts_with("flowchart ")
            || line == "@startuml"
            || line == "@enduml"
            || line == "}"
            || line.starts_with("//")
            || line.starts_with("%%")
            || line.starts_with("digraph ")
            || line == "graph {"
        {
            continue;
        }
        if let Some(c) = edge_re.captures(line) {
            add(&mut g, &mut seen, &c[1], &c[1]);
            add(&mut g, &mut seen, &c[3], &c[3]);
            g.edges.push(DiagramEdge {
                id: format!("e{}", g.edges.len() + 1),
                source: Some(c[1].into()),
                target: Some(c[3].into()),
                label: "".into(),
                method: "native".into(),
                cardinality: None,
                attributes: json!({"connector": &c[2], "source_statement": line}),
            });
        } else if let Some(c) = dot_node
            .captures(line)
            .or_else(|| node_re.captures(line.trim_end_matches(';')))
        {
            let label = c
                .get(2)
                .or_else(|| c.get(3))
                .map(|v| v.as_str())
                .unwrap_or(&c[1]);
            add(&mut g, &mut seen, &c[1], label);
        } else {
            g.warnings
                .push(format!("unsupported {format} construct: {line}"));
        }
    }
    Ok(g)
}

pub fn vision_graph(text: &str) -> Result<Diagram> {
    let start = text.find('{').context("missing JSON graph")?;
    let end = text.rfind('}').context("missing JSON graph")?;
    let value: serde_json::Value = serde_json::from_str(&text[start..=end])?;
    let mut g = Diagram::default();
    for node in value["nodes"].as_array().context("missing nodes")? {
        let id = node["id"].as_str().context("missing node ID")?;
        if g.nodes.iter().any(|n| n.id == id) {
            bail!("duplicate graph node ID");
        }
        g.nodes.push(DiagramNode {
            id: id.into(),
            label: node["label"].as_str().unwrap_or("").into(),
            parent: None,
            bounds: None,
            attributes: json!({"inferred":true,"raw":node}),
        });
    }
    for edge in value["edges"].as_array().into_iter().flatten() {
        let endpoint = |key| {
            edge[key]
                .as_str()
                .filter(|id| g.nodes.iter().any(|n| n.id == *id))
                .map(str::to_owned)
        };
        g.edges.push(DiagramEdge {
            id: format!("inferred-e{}", g.edges.len() + 1),
            source: endpoint("source"),
            target: endpoint("target"),
            label: edge["label"].as_str().unwrap_or("").into(),
            method: "inferred".into(),
            cardinality: edge["cardinality"].as_str().map(str::to_owned),
            attributes: json!({"evidence":edge["evidence"],"raw":edge,"verified":false}),
        });
    }
    g.warnings.push("Vision relationships and keys are unverified inferences, not source facts or calibrated probabilities".into());
    Ok(g)
}

fn er_source(source: &str, format: &str) -> Result<Diagram> {
    let mut g = Diagram::default();
    let mut active: Option<usize> = None;
    let entity = regex::Regex::new(r#"^(?:entity\s+)?([A-Za-z_]\w*)\s*\{\s*$"#)?;
    let field = regex::Regex::new(
        r#"^([A-Za-z][\w()\[\]-]*)\s+(\*?[\w-]+)(?:\s+((?:PK|FK|UK)(?:\s*,\s*(?:PK|FK|UK))*))?(?:\s+"([^"]*)")?\s*$"#,
    )?;
    let uml_field =
        regex::Regex::new(r#"^(\*?\w+)\s*:\s*([\w()\[\]-]+)(?:\s+<<(PK|FK|UK)>>)?\s*$"#)?;
    let relation =
        regex::Regex::new(r#"^(\w+)\s+([|o}{]+)(--|\.\.|-)([|o}{]+)\s+(\w+)(?:\s*:\s*(.*))?$"#)?;
    for line in source.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(i) = active {
            if line == "}" {
                active = None;
                continue;
            }
            let capture = if format == "plantuml" {
                uml_field.captures(line)
            } else {
                field.captures(line)
            };
            if let Some(c) = capture {
                let (name, typ) = if format == "plantuml" {
                    (&c[1], &c[2])
                } else {
                    (&c[2], &c[1])
                };
                let keys = c.get(3).map(|v| v.as_str()).unwrap_or("");
                let attribute = json!({"name":name.trim_start_matches('*'),"type":typ,"primary_key":keys.split(',').any(|k|k.trim()=="PK"),"foreign_key":keys.split(',').any(|k|k.trim()=="FK"),"required":name.starts_with('*'),"key_evidence":if keys.is_empty(){None}else{Some(keys)},"source_statement":line});
                g.nodes[i].attributes["entity_attributes"]
                    .as_array_mut()
                    .unwrap()
                    .push(attribute);
            } else {
                g.warnings
                    .push(format!("unsupported {format} ER attribute: {line}"));
            }
            continue;
        }
        if line == "erDiagram"
            || line == "@startuml"
            || line == "@enduml"
            || line.starts_with("%%")
            || line.starts_with("//")
        {
            continue;
        }
        if let Some(c) = entity.captures(line) {
            let index = if let Some(i) = g.nodes.iter().position(|n| n.id == c[1]) {
                i
            } else {
                g.nodes.push(DiagramNode {
                    id: c[1].into(),
                    label: c[1].into(),
                    parent: None,
                    bounds: None,
                    attributes: json!({"entity_attributes":[]}),
                });
                g.nodes.len() - 1
            };
            active = Some(index);
        } else if let Some(c) = relation.captures(line) {
            for id in [&c[1], &c[5]] {
                if !g.nodes.iter().any(|n| n.id == id) {
                    g.nodes.push(DiagramNode {
                        id: id.into(),
                        label: id.into(),
                        parent: None,
                        bounds: None,
                        attributes: json!({"entity_attributes":[]}),
                    });
                }
            }
            let left = &c[2];
            let right = &c[4];
            if !["||", "|o", "}o", "}|"].contains(&left)
                || !["||", "o|", "o{", "|{"].contains(&right)
            {
                g.warnings
                    .push(format!("unsupported cardinality marker: {line}"));
                continue;
            }
            g.edges.push(DiagramEdge{id:format!("e{}",g.edges.len()+1),source:Some(c[1].into()),target:Some(c[5].into()),label:c.get(6).map(|v|v.as_str().trim_matches('"')).unwrap_or("").into(),method:"native".into(),cardinality:Some(format!("{}{}{}",left,&c[3],right)),attributes:json!({"left_cardinality":left,"right_cardinality":right,"identifying":&c[3]=="--","direction":null,"source_statement":line})});
        } else {
            g.warnings
                .push(format!("unsupported {format} ER construct: {line}"));
        }
    }
    if active.is_some() {
        g.warnings.push("unclosed entity block".into());
    }
    Ok(g)
}
