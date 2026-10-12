//! Source-order Markdown events, without executing embedded code or fetching links.
use crate::model::*;
use pulldown_cmark::{Event, Tag, TagEnd};
use serde_json::json;
pub fn markdown(u: &mut Unit, source: &str) {
    let mut text = String::new();
    let mut kind = "text";
    let mut links = vec![];
    let mut table: Option<Table> = None;
    let mut row = 0;
    let mut column = 0;
    let mut cell = String::new();
    let mut in_cell = false;
    let mut image: Option<(String, String)> = None;
    for event in pulldown_cmark::Parser::new_ext(source, pulldown_cmark::Options::all()) {
        match event {
            Event::Start(Tag::Table(_)) => {
                table = Some(Table {
                    cells: vec![],
                    rows: 0,
                    columns: 0,
                    headers: vec![0],
                    compact: false,
                })
            }
            Event::Start(Tag::TableHead) => {
                row = 0;
                column = 0;
            }
            Event::Start(Tag::TableRow) => column = 0,
            Event::Start(Tag::TableCell) => {
                in_cell = true;
                cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(t) = &mut table {
                    t.cells.push(Cell {
                        row,
                        column,
                        raw: cell.clone(),
                        value: json!(cell),
                        formula: None,
                        cached_value: None,
                        row_span: 1,
                        column_span: 1,
                    });
                    t.rows = t.rows.max(row as usize + 1);
                    t.columns = t.columns.max(column as usize + 1);
                    column += 1;
                }
                in_cell = false;
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => row += 1,
            Event::End(TagEnd::Table) => {
                u.text("table", "", "Markdown table").table = table.take();
            }
            Event::Start(Tag::Heading { .. }) => kind = "heading",
            Event::Start(Tag::Item) => kind = "list",
            Event::Start(Tag::CodeBlock(_)) => kind = "text",
            Event::Start(Tag::FootnoteDefinition(_)) => kind = "footnote",
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => links.push(json!({"href":dest_url.to_string(),"title":title.to_string()})),
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => image = Some((dest_url.to_string(), title.to_string())),
            Event::End(TagEnd::Image) => {
                if let Some((src, title)) = image.take() {
                    let b = u.text("figure", "", "Markdown image reference");
                    b.attributes = json!({"src":src,"title":title,"fetched":false});
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if in_cell {
                    cell.push_str(&t);
                } else {
                    text.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_cell {
                    cell.push('\n');
                } else {
                    text.push('\n');
                }
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::FootnoteDefinition,
            ) => {
                if !text.is_empty() {
                    let b = u.text(kind, std::mem::take(&mut text), "Markdown");
                    b.attributes = json!({"links":std::mem::take(&mut links)});
                }
                kind = "text";
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                u.text("unknown", html.to_string(), "Embedded Markdown HTML");
                u.warnings.push(
                    "Embedded Markdown HTML retained as source; scripts are not executed".into(),
                );
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        u.text(kind, text, "Markdown").attributes = json!({"links":links});
    }
}
