//! OOXML cached chart data is native source evidence, never a recalculation.
use crate::{diagrams, model::*};
use anyhow::Result;
use serde_json::json;
pub fn extract(bytes: &[u8], source: &str, u: &mut Unit) -> Result<()> {
    let doc = diagrams::xml(std::str::from_utf8(bytes)?)?;
    let chart_type = doc
        .descendants()
        .find(|n| {
            [
                "barChart",
                "lineChart",
                "pieChart",
                "scatterChart",
                "areaChart",
                "doughnutChart",
                "bubbleChart",
                "radarChart",
            ]
            .contains(&n.tag_name().name())
        })
        .map(|n| n.tag_name().name());
    let mut series = vec![];
    for node in doc.descendants().filter(|n| n.has_tag_name("ser")) {
        let caches=node.descendants().filter(|n|n.has_tag_name("numCache")||n.has_tag_name("strCache")||n.has_tag_name("numLit")||n.has_tag_name("strLit")).map(|cache|json!({"kind":cache.tag_name().name(),"axis":cache.parent().and_then(|p|p.parent()).map(|p|p.tag_name().name()),"points":cache.children().filter(|n|n.has_tag_name("pt")).map(|n|{let raw=n.children().find(|n|n.has_tag_name("v")).and_then(|n|n.text()).unwrap_or("");json!({"index":n.attribute("idx"),"raw":raw,"value":if cache.tag_name().name().starts_with("num"){raw.parse::<f64>().map(serde_json::Value::from).unwrap_or_else(|_|json!(raw))}else{json!(raw)}})}).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let references = node
            .descendants()
            .filter(|n| n.has_tag_name("f"))
            .filter_map(|n| n.text())
            .collect::<Vec<_>>();
        series.push(json!({"index":node.children().find(|n|n.has_tag_name("idx")).and_then(|n|n.attribute("val")),"cached_data":caches,"formula_references":references}));
    }
    let block = u.text("figure", "Native chart cached data", source);
    block.attributes =
        json!({"chart_type":chart_type,"series":series,"recalculated":false,"rendered":false});
    if chart_type.is_none() {
        u.warnings.push(format!(
            "Unknown OOXML chart type: {source}; source retained"
        ));
    }
    Ok(())
}
